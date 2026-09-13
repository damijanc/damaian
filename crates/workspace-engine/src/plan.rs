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

impl StepStatus {
    /// Whether this step's outcome is already settled.
    ///
    /// `Blocked` counts: the step is not going to finish, and treating it as
    /// still open would let a plan whose prerequisite failed look like one
    /// still making progress. What a settled step mainly earns is protection —
    /// a plan revision may not delete one, because its evidence is tied to a
    /// state of the repository (§5.5).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Blocked | Self::Skipped)
    }
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

impl PlanStep {
    /// Completed with nothing observable behind it — requirement 6's second
    /// sentence, and what the completion report prints as "completed
    /// unverified".
    ///
    /// A `Blocked` step is deliberately **not** unverified. It is not completed
    /// at all, and reporting it as completed-unverified would turn a failure
    /// into a soft pass, which is precisely the laundering §5.3 forbids.
    pub fn is_unverified(&self) -> bool {
        self.status == StepStatus::Completed && self.evidence.is_empty()
    }
}

/// What the work is about, as distinct from which stage of the agent loop is
/// executing.
///
/// Deliberately **not** an extension of `chat::PhaseKind`
/// (`Context`/`Model`/`Tool`/`Finalizing`), which drives the spinner. Those are
/// orthogonal axes: a `PhaseKind::Model` occurs during every one of these six,
/// and merging them would produce a type whose variants are not mutually
/// exclusive. See `context.md` §3.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Understanding,
    Planning,
    Editing,
    Validating,
    Reviewing,
    Complete,
}

/// Requirement 6's rule, mechanically.
///
/// The model does not appear in this function's inputs. That is the whole
/// design: a step's status is derived from what Damaian observed, so there is
/// no code path by which "the model said it was done" becomes `Completed`.
///
/// Empty evidence completes the step. A step like "understand the existing
/// retry logic" genuinely has nothing observable behind it, and forcing a fake
/// observation would be worse than admitting the gap —
/// [`PlanStep::is_unverified`] is how the completion report admits it.
pub fn status_from_evidence(evidence: &[Evidence]) -> StepStatus {
    let blocked = evidence.iter().any(|item| match item {
        // `Some(0)` and nothing else. `None` means the command was killed or
        // signalled, so nothing is known — and "nothing is known" is not "it
        // passed". Writing this as `matches!(code, Some(c) if *c != 0)` would
        // let `None` fall through as success, which is the same defect as an
        // `unwrap_or(0)` wearing different clothes.
        Evidence::CommandExit { exit_code, .. } => *exit_code != Some(0),
        // Neither has a failure mode to encode: a patch that did not apply
        // returns an error and produces no evidence, and a file that could not
        // be read produces none either.
        Evidence::PatchApplied { .. } | Evidence::FileRead { .. } => false,
    });
    if blocked {
        StepStatus::Blocked
    } else {
        StepStatus::Completed
    }
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

    /// Requirement 3's phase, **derived** from step state and never stored.
    ///
    /// §5.1: derived rather than set independently, so the phase cannot say
    /// "validating" while every validation step is still pending. There is no
    /// setter, and that is the design — a stored phase is a second source of
    /// truth that can disagree with the first.
    ///
    /// `awaiting_review` is passed in because it is the one phase a plan cannot
    /// see for itself: a patch waiting on a human is a fact about the *task*,
    /// not about its steps. Guessing it from the steps would mean inventing it;
    /// taking it as an argument makes the dependency visible at every call site.
    ///
    /// The rule, in precedence order:
    ///
    /// 1. Every step terminal (`Completed` or `Skipped`) → `Complete`. A
    ///    `Blocked` step is *not* terminal here: work that failed is work
    ///    outstanding, and §5.6 requires the summary never to say "complete"
    ///    for a plan holding one.
    /// 2. `awaiting_review` → `Reviewing`. Ranked below completion because a
    ///    finished plan has no work left for a review to gate.
    /// 3. Otherwise the **newest** evidence anywhere in the plan, in step
    ///    order: a command → `Validating`, an applied patch → `Editing`, a file
    ///    read → `Understanding`.
    /// 4. Work started but nothing observed yet → `Understanding`.
    /// 5. No steps at all → `Planning`.
    ///
    /// Newest rather than "any", because "any" pins the phase to whatever the
    /// turn did first: a step that read a file and then ran a check is
    /// validating, not understanding.
    ///
    /// Two honest limits, recorded in proposal §7 rather than hidden:
    /// `Planning` is nearly unreachable, because a plan is created with its
    /// first step already open in the same event; and `Editing` cannot be
    /// reached until `Evidence::PatchApplied` can be attached, which needs the
    /// cross-turn continuation from the resume work — a patch is applied after
    /// the turn that proposed it has ended.
    pub fn phase(&self, awaiting_review: bool) -> TaskPhase {
        if self.steps.is_empty() {
            return TaskPhase::Planning;
        }
        let all_terminal = self
            .steps
            .iter()
            .all(|step| matches!(step.status, StepStatus::Completed | StepStatus::Skipped));
        if all_terminal {
            return TaskPhase::Complete;
        }
        if awaiting_review {
            return TaskPhase::Reviewing;
        }
        match self
            .steps
            .iter()
            .flat_map(|step| step.evidence.iter())
            .next_back()
        {
            Some(Evidence::CommandExit { .. }) => TaskPhase::Validating,
            Some(Evidence::PatchApplied { .. }) => TaskPhase::Editing,
            Some(Evidence::FileRead { .. }) | None => TaskPhase::Understanding,
        }
    }

    /// What to tell the user this plan came to (§5.6).
    pub fn report(&self) -> PlanReport {
        let steps: Vec<ReportedStep> = self
            .steps
            .iter()
            .map(|step| ReportedStep {
                id: step.id.clone(),
                title: step.title.clone(),
                outcome: StepOutcome::of(step),
            })
            .collect();
        let count =
            |wanted: StepOutcome| steps.iter().filter(|step| step.outcome == wanted).count();
        let verified = count(StepOutcome::Verified);
        let unverified = count(StepOutcome::Unverified);
        let blocked = count(StepOutcome::Blocked);
        let skipped = count(StepOutcome::Skipped);
        let outstanding = count(StepOutcome::Outstanding);
        PlanReport {
            // A plan with no steps is not complete. "All zero steps finished"
            // is true and useless: it would make an empty plan the
            // best-looking outcome in the report.
            is_complete: !steps.is_empty() && blocked == 0 && outstanding == 0,
            verified,
            unverified,
            blocked,
            skipped,
            outstanding,
            steps,
        }
    }
}

/// What became of one step, as the completion report puts it.
///
/// Four terminal outcomes plus `Outstanding`, which is not an outcome at all
/// but the absence of one — a turn can end with a step still open (a token
/// stop, the stop button, a plan held for review), and reporting that step
/// beside the finished ones would claim it reached somewhere it has not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    /// Completed, with evidence behind it.
    Verified,
    /// Completed, with nothing observable behind it. Said out loud rather than
    /// folded into `Verified`: requirement 6's whole point is that "the model
    /// said it was done" is not confirmation.
    Unverified,
    Blocked,
    Skipped,
    /// Still pending or in progress when the turn ended.
    Outstanding,
}

impl StepOutcome {
    pub fn of(step: &PlanStep) -> Self {
        match step.status {
            StepStatus::Completed if step.evidence.is_empty() => Self::Unverified,
            StepStatus::Completed => Self::Verified,
            StepStatus::Blocked => Self::Blocked,
            StepStatus::Skipped => Self::Skipped,
            StepStatus::Pending | StepStatus::InProgress => Self::Outstanding,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Unverified => "unverified",
            Self::Blocked => "blocked",
            Self::Skipped => "skipped",
            Self::Outstanding => "outstanding",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedStep {
    pub id: String,
    pub title: String,
    pub outcome: StepOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReport {
    pub steps: Vec<ReportedStep>,
    pub is_complete: bool,
    pub verified: usize,
    pub unverified: usize,
    pub blocked: usize,
    pub skipped: usize,
    pub outstanding: usize,
}

impl PlanReport {
    /// One line for the top of the panel.
    ///
    /// Written here rather than in the UI so there is one place that decides
    /// what a plan "came to" — a second wording in the frontend is how the
    /// panel and the report start disagreeing about the same plan.
    ///
    /// The word "complete" appears only when
    /// [`Self::is_complete`] holds. An unverified step does not withhold it —
    /// the step did finish — but the count is always named, because a plan
    /// reported as complete with three unverified steps and one reported as
    /// complete with none are different results.
    pub fn summary(&self) -> String {
        if self.steps.is_empty() {
            return "No steps planned.".to_string();
        }
        let mut parts = Vec::new();
        if self.is_complete {
            parts.push(format!(
                "{} complete",
                pluralise(self.verified + self.unverified, "step")
            ));
        } else {
            if self.blocked > 0 {
                parts.push(format!("{} blocked", pluralise(self.blocked, "step")));
            }
            if self.outstanding > 0 {
                parts.push(format!(
                    "{} outstanding",
                    pluralise(self.outstanding, "step")
                ));
            }
            let finished = self.verified + self.unverified;
            if finished > 0 {
                parts.push(format!("{finished} finished"));
            }
        }
        if self.unverified > 0 {
            parts.push(format!("{} unverified", self.unverified));
        }
        if self.skipped > 0 {
            parts.push(format!("{} skipped", self.skipped));
        }
        format!("{}.", parts.join(", "))
    }
}

fn pluralise(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}
