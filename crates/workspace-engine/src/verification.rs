//! Verification of applied changes, and the completion report that says what
//! was actually checked.
//!
//! See `docs/specs/23_verification_loop/proposal.md` and its `context.md`. This
//! module is the pure core of the loop: the identity of a check, the outcomes a
//! run can have, the session events that record them, one fold over those
//! events, and the report built from the folded state. No session, model or
//! filesystem, because the rules in here are requirement 7, and a rule worth
//! stating is worth testing without a turn built around it.
//!
//! The one rule everything else serves: **an unrun check is never represented
//! as passed.** `passed` means a [`CheckOutcome::Ran`] with
//! `exit_code == Some(0)` and nothing else. `None` means the command was killed
//! or signalled, which is not a zero, and `!is_error`, the absence of a failure,
//! and `exit_code.unwrap_or(0)` are all the same defect wearing different
//! clothes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::command_policy::relative_location;
use crate::error::Result;
use crate::plan::{PlanReport, TaskPlan};
use crate::repository_map::RepositoryMap;
use crate::validation::{CommandProposal, ValidationOrchestrator};

/// The command prefix that distinguishes a browser behaviour check from a
/// project check. A browser check is still a [`CheckId`], so it is selected,
/// recorded and staleness-checked like any other; its label is the page.
pub const BROWSER_CHECK_PREFIX: &str = "browser diagnostic: ";

/// A check's identity: the command and the root it runs in.
///
/// Repository-relative, with `""` for the repository root, so `cargo test` at
/// the root and `cargo test` in `crates/engine` are two checks — which they are
/// (proposal §5.7). Ordering and hashing are on both fields so a report, a
/// regression rerun and a deduplicated selection all agree on identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckId {
    pub working_directory: String,
    pub command: String,
}

impl CheckId {
    pub fn new(working_directory: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            working_directory: working_directory.into(),
            command: command.into(),
        }
    }

    /// The browser check for a page the task already used (proposal §5.4).
    pub fn browser(url: impl AsRef<str>) -> Self {
        Self::new("", format!("{BROWSER_CHECK_PREFIX}{}", url.as_ref()))
    }

    pub fn is_browser(&self) -> bool {
        self.command.starts_with(BROWSER_CHECK_PREFIX)
    }

    /// How the report names this check: ``cargo test (crates/engine)`` or
    /// ``cargo test (repository root)``.
    pub fn label(&self) -> String {
        if let Some(url) = self.command.strip_prefix(BROWSER_CHECK_PREFIX) {
            return format!("browser check {url}");
        }
        let root = if self.working_directory.is_empty() {
            "repository root"
        } else {
            self.working_directory.as_str()
        };
        format!("{} ({root})", self.command)
    }
}

/// Why a selected check did not run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// The user declined it on the check card. Not an error: the task continues
    /// and the report states the check did not run.
    DeclinedByUser,
    /// The command policy refuses it. Never retried, never worked around.
    Blocked {
        /// The policy's own reason, rendered verbatim into the report. Renamed
        /// on the wire because `reason` is the tag.
        #[serde(rename = "policyReason")]
        reason: String,
    },
    /// A web change whose behaviour could not be checked because nothing was
    /// serving the page. Never inferred as verified.
    NoRunningTarget,
}

impl SkipReason {
    /// The report's wording for this reason.
    pub fn description(&self) -> String {
        match self {
            Self::DeclinedByUser => "declined by you".to_string(),
            Self::Blocked { reason } => format!("blocked by policy: {reason}"),
            Self::NoRunningTarget => "no running target".to_string(),
        }
    }
}

/// What a check run came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CheckOutcome {
    #[serde(rename_all = "camelCase")]
    Ran {
        /// `None` when the command was killed or signalled. Not a zero.
        exit_code: Option<i32>,
        marker_id: String,
    },
    Skipped(SkipReason),
}

/// A check's latest recorded run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRun {
    pub outcome: CheckOutcome,
    /// The hash of every file the task had changed when the check ran, so a
    /// later edit, rollback or patch can make the result stale (proposal §5.6).
    pub file_hashes: BTreeMap<String, String>,
}

/// What the whole verification came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    Passed,
    /// Nothing failed, but no check ran either: every selected check was
    /// skipped, declined by the user or blocked by policy. Recorded instead
    /// of `Passed`, because an unrun check is never represented as passed
    /// (proposal §3, spec 78).
    NothingVerified,
    Failed,
    RepairLimitExhausted,
    NoRepairApplied,
    NoChecksAvailable,
    OnlyDocumentationChanged,
}

impl VerificationOutcome {
    /// The wire and report name, matching the serde representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::NothingVerified => "nothing_verified",
            Self::Failed => "failed",
            Self::RepairLimitExhausted => "repair_limit_exhausted",
            Self::NoRepairApplied => "no_repair_applied",
            Self::NoChecksAvailable => "no_checks_available",
            Self::OnlyDocumentationChanged => "only_documentation_changed",
        }
    }
}

/// A session event, appended to the log beside the plan it populates.
///
/// One event kind per variant; the wire names are the session-event kinds
/// (`verification_due`, and so on) so a reader of the log sees what appended it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum VerificationEvent {
    #[serde(rename = "verification_due", rename_all = "camelCase")]
    Due {
        task_id: String,
        /// Empty when an approved command, not a patch, made the run due.
        #[serde(default)]
        patch_id: String,
        /// The files an approved command changed (spec 85). A patch's files
        /// are read from its decisions instead, so a patch's `Due` has none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
    },
    #[serde(rename = "verification_started", rename_all = "camelCase")]
    Started {
        task_id: String,
        attempt: u32,
        checks: Vec<CheckId>,
    },
    #[serde(rename = "verification_check", rename_all = "camelCase")]
    Check {
        task_id: String,
        check: CheckId,
        outcome: CheckOutcome,
        file_hashes: BTreeMap<String, String>,
    },
    #[serde(rename = "verification_finished", rename_all = "camelCase")]
    Finished {
        task_id: String,
        outcome: VerificationOutcome,
    },
}

impl VerificationEvent {
    /// The task this event belongs to. A `Due` names the task that proposed the
    /// patch; a `Started`/`Check`/`Finished` names the verification turn's own
    /// task, which is why readers take a task chain.
    pub fn task_id(&self) -> &str {
        match self {
            Self::Due { task_id, .. }
            | Self::Started { task_id, .. }
            | Self::Check { task_id, .. }
            | Self::Finished { task_id, .. } => task_id,
        }
    }
}

/// A due verification's origin, kept so recovery can say what is waiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationDue {
    pub task_id: String,
    pub patch_id: String,
    /// Set when an approved command made the run due (spec 85).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

impl VerificationDue {
    /// An approved command, not a patch, made this run due (spec 85).
    pub fn from_command(&self) -> bool {
        self.patch_id.is_empty()
    }
}

/// Everything the events say, folded once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerificationState {
    /// A `Due` with no later `Finished`. Starting a run does not clear it: an
    /// interrupted verification is still due, which is what makes recovery
    /// offer it again.
    pub due: Option<VerificationDue>,
    /// The attempt of the latest `Started`. Attempt 0 is the first
    /// verification; repairs are 1 to `agent_tool_retry_limit`.
    pub attempt: u32,
    /// Selected checks in first-selected order.
    pub selected: Vec<CheckId>,
    /// The latest outcome and file hashes per check.
    pub checks: BTreeMap<CheckId, CheckRun>,
    /// Checks that passed at an earlier attempt and are failing now.
    pub regressions: Vec<CheckId>,
    /// The last `Finished` outcome, cleared by a new `Started`.
    pub finished: Option<VerificationOutcome>,
    /// Private so the state cannot be constructed by hand into a shape the
    /// events do not support; the report's honesty rests on the fold.
    passed_before: BTreeSet<CheckId>,
}

impl VerificationState {
    /// Apply one event. Public so a reader that replays a log in batches gets
    /// exactly the state a single fold produces.
    pub fn apply(&mut self, event: &VerificationEvent) {
        match event {
            VerificationEvent::Due {
                task_id,
                patch_id,
                paths,
            } => {
                self.due = Some(VerificationDue {
                    task_id: task_id.clone(),
                    patch_id: patch_id.clone(),
                    paths: paths.clone(),
                });
            }
            VerificationEvent::Started {
                attempt, checks, ..
            } => {
                self.attempt = *attempt;
                self.finished = None;
                for check in checks {
                    if !self.selected.contains(check) {
                        self.selected.push(check.clone());
                    }
                }
            }
            VerificationEvent::Check {
                check,
                outcome,
                file_hashes,
                ..
            } => {
                if passing(outcome) {
                    self.passed_before.insert(check.clone());
                }
                self.checks.insert(
                    check.clone(),
                    CheckRun {
                        outcome: outcome.clone(),
                        file_hashes: file_hashes.clone(),
                    },
                );
            }
            VerificationEvent::Finished { outcome, .. } => {
                self.due = None;
                self.finished = Some(*outcome);
            }
        }
        self.regressions = self
            .checks
            .iter()
            .filter(|(check, run)| !passing(&run.outcome) && self.passed_before.contains(*check))
            .map(|(check, _)| check.clone())
            .collect();
    }
}

/// Replay the log into the state the report reads.
pub fn fold_verification<'a>(
    events: impl IntoIterator<Item = &'a VerificationEvent>,
) -> VerificationState {
    let mut state = VerificationState::default();
    for event in events {
        state.apply(event);
    }
    state
}

/// `Some(0)` and nothing else. The comment is the rule: `None` means killed or
/// signalled, so it is unknown, and unknown is not passed.
fn passing(outcome: &CheckOutcome) -> bool {
    matches!(
        outcome,
        CheckOutcome::Ran {
            exit_code: Some(0),
            ..
        }
    )
}

fn failure_reason(exit_code: Option<i32>) -> String {
    match exit_code {
        Some(code) => format!("exit code {code}"),
        None => "killed or signalled".to_string(),
    }
}

/// A check that ran and did not pass, with the reason the report prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedCheck {
    pub check: CheckId,
    pub reason: String,
}

/// A check that did not run, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedCheck {
    pub check: CheckId,
    pub reason: SkipReason,
}

/// Proposal §5.7's report, as data and as text.
///
/// The four partitions — `passed`, `failed`, `skipped`, `stale` — together hold
/// every selected check exactly once. `behaviour_verified` is an additional
/// statement about browser checks, not a fifth partition, so a check can be
/// both passed and behaviour-verified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationReport {
    pub files_changed: Vec<String>,
    pub passed: Vec<CheckId>,
    pub failed: Vec<FailedCheck>,
    pub skipped: Vec<SkippedCheck>,
    pub stale: Vec<CheckId>,
    pub behaviour_verified: Vec<String>,
    pub unverified_steps: Vec<String>,
    pub regressions: Vec<CheckId>,
    pub outcome: Option<VerificationOutcome>,
    /// Verification is due and has not run. The summary must never say
    /// "complete" while this holds.
    pub due: bool,
}

impl VerificationReport {
    /// Build the report from the folded state.
    ///
    /// Staleness is decided **before** a ran check is sorted into passed or
    /// failed: a check whose files no longer match is in `stale` whatever its
    /// exit code was, because it did not verify the current state.
    pub fn build(
        state: &VerificationState,
        plan: &TaskPlan,
        files_changed: &[String],
        current_hashes: &BTreeMap<String, String>,
    ) -> Self {
        let mut report = Self {
            files_changed: files_changed.to_vec(),
            passed: Vec::new(),
            failed: Vec::new(),
            skipped: Vec::new(),
            stale: Vec::new(),
            behaviour_verified: Vec::new(),
            unverified_steps: plan
                .steps
                .iter()
                .filter(|step| step.is_unverified())
                .map(|step| step.title.clone())
                .collect(),
            regressions: state.regressions.clone(),
            outcome: state.finished,
            due: state.due.is_some(),
        };
        for check in &state.selected {
            let Some(run) = state.checks.get(check) else {
                // Selected, but the log holds no result — an interrupted
                // verification. It belongs in `failed`, never in `passed`.
                report.failed.push(FailedCheck {
                    check: check.clone(),
                    reason: "no result was recorded".to_string(),
                });
                continue;
            };
            match &run.outcome {
                CheckOutcome::Skipped(reason) => report.skipped.push(SkippedCheck {
                    check: check.clone(),
                    reason: reason.clone(),
                }),
                CheckOutcome::Ran { exit_code, .. } => {
                    if is_stale(run, current_hashes) {
                        report.stale.push(check.clone());
                        continue;
                    }
                    if passing(&run.outcome) {
                        report.passed.push(check.clone());
                        if check.is_browser() {
                            report.behaviour_verified.push(check.label());
                        }
                    } else {
                        // A browser check's "exit code" is the diagnostic's
                        // verdict, not a process exit; saying "exit code 1"
                        // would describe a command that never ran.
                        let reason = if check.is_browser() {
                            "the page reported errors".to_string()
                        } else {
                            failure_reason(*exit_code)
                        };
                        report.failed.push(FailedCheck {
                            check: check.clone(),
                            reason,
                        });
                    }
                }
            }
        }
        report
    }

    /// One line for the top of the panel or the end of a turn.
    ///
    /// The word "complete" appears only when the plan's
    /// [`PlanReport::is_complete`] holds, no check failed, nothing is stale, the
    /// repair limit was not exhausted, and nothing is due. Negative wording
    /// avoids the word entirely rather than using "incomplete": a substring
    /// check is the cheapest guard against a later optimistic rewrite.
    pub fn summary(&self, plan_report: &PlanReport) -> String {
        let plan_note = format!(
            "{} of {} plan steps finished.",
            plan_report.verified + plan_report.unverified,
            plan_report.steps.len()
        );
        if self.due {
            return format!("Verification is due and has not run. {plan_note}");
        }
        if let Some(VerificationOutcome::NoChecksAvailable) = self.outcome {
            return format!("No checks were available, so nothing was verified. {plan_note}");
        }
        if let Some(VerificationOutcome::OnlyDocumentationChanged) = self.outcome {
            return format!("Only documentation changed, so no checks ran. {plan_note}");
        }
        if let Some(VerificationOutcome::RepairLimitExhausted) = self.outcome {
            return format!(
                "Verification failed: the repair limit was exhausted and {} still {}. {plan_note}",
                count(self.failed.len(), "check"),
                if self.failed.len() == 1 {
                    "fails"
                } else {
                    "fail"
                }
            );
        }
        if let Some(VerificationOutcome::NoRepairApplied) = self.outcome {
            return format!(
                "Verification stopped because no repair patch was applied and {} still {}. \
                 {plan_note}",
                count(self.failed.len(), "check"),
                if self.failed.len() == 1 {
                    "fails"
                } else {
                    "fail"
                }
            );
        }
        if !self.failed.is_empty() || matches!(self.outcome, Some(VerificationOutcome::Failed)) {
            return format!(
                "Verification failed: {} still {}. {plan_note}",
                count(self.failed.len(), "check"),
                if self.failed.len() == 1 {
                    "fails"
                } else {
                    "fail"
                }
            );
        }
        if !self.stale.is_empty() {
            return format!(
                "Verification passed, but {} no longer {} the current state. {plan_note}",
                count(self.stale.len(), "check"),
                if self.stale.len() == 1 {
                    "matches"
                } else {
                    "match"
                }
            );
        }
        if self.passed.is_empty() {
            if !self.skipped.is_empty() {
                return format!(
                    "No check ran: {} skipped, so nothing was verified. {plan_note}",
                    count(self.skipped.len(), "check")
                );
            }
            return format!("No verification has run. {plan_note}");
        }
        if plan_report.is_complete {
            format!(
                "Verification complete: {} passed. {plan_note}",
                count(self.passed.len(), "check")
            )
        } else {
            format!(
                "Verification passed: {}. {plan_note}",
                count(self.passed.len(), "check")
            )
        }
    }

    /// Proposal §5.7's layout. Paths are plain repository-relative text; the
    /// renderer that makes them clickable is the shell's.
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        lines.push(section("Files changed", &list_or_none(&self.files_changed)));

        let passed = if !self.passed.is_empty() {
            format!("{}   (all ran, exit 0)", labels(&self.passed).join(", "))
        } else {
            match self.outcome {
                Some(VerificationOutcome::NoChecksAvailable) => {
                    "No checks were available".to_string()
                }
                Some(VerificationOutcome::OnlyDocumentationChanged) => {
                    "No checks were selected: only documentation changed".to_string()
                }
                _ => "none".to_string(),
            }
        };
        lines.push(section("Checks passed", &passed));

        let failed = if self.failed.is_empty() {
            "none".to_string()
        } else {
            self.failed
                .iter()
                .map(|item| format!("{} — {}", item.check.label(), item.reason))
                .collect::<Vec<_>>()
                .join("\n                     ")
        };
        lines.push(section("Checks failed", &failed));

        let skipped = if self.skipped.is_empty() {
            "none".to_string()
        } else {
            self.skipped
                .iter()
                .map(|item| format!("{} — {}", item.check.label(), item.reason.description()))
                .collect::<Vec<_>>()
                .join("\n                     ")
        };
        lines.push(section("Checks skipped", &skipped));

        let stale = if self.stale.is_empty() {
            "none".to_string()
        } else {
            labels(&self.stale).join(", ")
        };
        lines.push(section("Checks stale", &stale));

        let behaviour = if self.behaviour_verified.is_empty() {
            "none".to_string()
        } else {
            self.behaviour_verified.join(", ")
        };
        lines.push(section("Behaviour verified", &behaviour));

        let unverified = if self.unverified_steps.is_empty() {
            "none".to_string()
        } else {
            self.unverified_steps
                .iter()
                .map(|step| format!("\"{step}\" — no observable evidence"))
                .collect::<Vec<_>>()
                .join("\n                     ")
        };
        lines.push(section("Unverified steps", &unverified));

        let regressions = if self.regressions.is_empty() {
            "none".to_string()
        } else {
            labels(&self.regressions).join(", ")
        };
        lines.push(section("Regressions", &regressions));

        lines.push(section("Remaining", &self.remaining_note()));
        lines.push(section("User action needed", &self.user_action_note()));
        lines.join("\n")
    }

    fn remaining_note(&self) -> String {
        let mut notes = Vec::new();
        if self.due {
            notes.push("Verification has not run".to_string());
        }
        if !self.failed.is_empty() {
            notes.push(format!("{} still fail", count(self.failed.len(), "check")));
        }
        if !self.stale.is_empty() {
            notes.push(format!(
                "{} no longer {} the current state",
                count(self.stale.len(), "check"),
                if self.stale.len() == 1 {
                    "matches"
                } else {
                    "match"
                }
            ));
        }
        if notes.is_empty() {
            "none".to_string()
        } else {
            notes.join("; ")
        }
    }

    fn user_action_note(&self) -> String {
        if self.due {
            return "Run the verification.".to_string();
        }
        let declined: Vec<String> = self
            .skipped
            .iter()
            .filter(|item| item.reason == SkipReason::DeclinedByUser)
            .map(|item| item.check.label())
            .collect();
        if !declined.is_empty() {
            return format!(
                "Approve or allow {} to check {}.",
                declined.join(", "),
                if declined.len() == 1 { "it" } else { "them" }
            );
        }
        "none".to_string()
    }
}

fn is_stale(run: &CheckRun, current_hashes: &BTreeMap<String, String>) -> bool {
    // A deleted file is absent from `current_hashes`, which is a mismatch.
    run.file_hashes
        .iter()
        .any(|(path, hash)| current_hashes.get(path) != Some(hash))
}

fn labels(checks: &[CheckId]) -> Vec<String> {
    checks.iter().map(CheckId::label).collect()
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn section(label: &str, value: &str) -> String {
    format!("{label:<20} {value}")
}

fn count(number: usize, noun: &str) -> String {
    if number == 1 {
        format!("{number} {noun}")
    } else {
        format!("{number} {noun}s")
    }
}

/// Whether a changed path is documentation, and so selects no checks.
///
/// Deliberately an extension list rather than `detect_language`: that maps every
/// unknown extension to `text`, which would classify `Makefile` and `build.sh`
/// as documentation and skip the checks for a change that can break the build
/// (spec 23 `context.md` §2).
pub fn is_documentation(path: &str) -> bool {
    let Some((_, extension)) = path.rsplit_once('.') else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "md" | "txt" | "rst" | "adoc"
    )
}

/// The roots whose checks a change selects.
///
/// Selection is by owning root and every non-removed root that encloses it, not
/// by file category: a member change is a workspace change, and a workspace
/// root's check is what catches a member breaking a sibling (proposal §5.2).
/// Documentation selects nothing. Sorted and de-duplicated, so the caller can
/// compare selections directly.
pub fn select_roots(map: &RepositoryMap, changed: &[String]) -> Vec<String> {
    let mut selected = BTreeSet::new();
    for path in changed {
        if is_documentation(path) {
            continue;
        }
        for root in map.enclosing_roots(path) {
            selected.insert(root.to_string());
        }
    }
    selected.into_iter().collect()
}

/// What [`propose_checks`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Only documentation changed: nothing is selected, and that is not a
    /// failure or an empty success.
    OnlyDocumentation,
    /// The selected roots detect no project check at all. Distinct from
    /// `Proposed(Vec::new())`, because "no checks were available" must read
    /// differently from "checks passed" (proposal §5.2).
    NoChecksAvailable { roots: Vec<String> },
    /// Checks to propose, in the restricted map's order, each with the
    /// policy's own proposal.
    Proposed(Vec<(CheckId, CommandProposal)>),
}

/// Select the roots a change touched and ask the validation orchestrator for
/// their detected checks.
///
/// `propose_detected_validations` is the only discovery path: this function
/// restricts the map first, so nothing is stored and audited that will not run,
/// and then calls it unchanged (proposal §5.2, `context.md` §2).
pub fn propose_checks(
    orchestrator: &ValidationOrchestrator,
    repository_root: &Path,
    map: &RepositoryMap,
    changed: &[String],
) -> Result<Selection> {
    let roots = select_roots(map, changed);
    if roots.is_empty() {
        return Ok(Selection::OnlyDocumentation);
    }
    let mut restricted = map.restricted_to(&roots);
    drop_covered_member_checks(repository_root, &mut restricted);
    let proposals = orchestrator.propose_detected_validations(repository_root, &restricted)?;
    if proposals.is_empty() {
        return Ok(Selection::NoChecksAvailable { roots });
    }
    Ok(Selection::Proposed(
        proposals
            .into_iter()
            .map(|proposal| {
                // Check identity is repository-relative. Proposals carry the
                // absolute directory they will run in; the map only ever
                // proposes roots inside the repository, so the relative form
                // exists. The fallback keeps the identity unique rather than
                // silently collapsing it onto the repository root.
                let working_directory =
                    relative_location(repository_root, Path::new(&proposal.working_directory))
                        .unwrap_or_else(|| proposal.working_directory.clone());
                let check = CheckId::new(working_directory, proposal.command.clone());
                (check, proposal)
            })
            .collect(),
    ))
}

/// A member's `cargo test` repeats what its workspace root's `cargo test`
/// already runs (spec 90), so it is removed from the restricted map before
/// anything is proposed: nothing is stored that will not run (§5.2).
fn drop_covered_member_checks(repository_root: &Path, map: &mut RepositoryMap) {
    let runs_cargo_test = |root: &crate::repository_map::ProjectRoot| {
        root.commands
            .iter()
            .any(|item| item.command == "cargo test")
    };
    let workspace_roots: Vec<String> = map
        .roots
        .iter()
        .filter(|root| runs_cargo_test(root))
        .map(|root| root.path.clone())
        .collect();
    for root in &mut map.roots {
        let covered = workspace_roots.iter().any(|workspace| {
            workspace != &root.path
                && cargo_workspace_runs_member(repository_root, workspace, &root.path)
        });
        if covered {
            root.commands.retain(|item| item.command != "cargo test");
        }
    }
}

/// Whether `cargo test` in `root` runs the tests of the package in `member`
/// (both repository-relative). Read conservatively, without a TOML parser:
/// only a virtual workspace (`[workspace]` and no `[package]`) with no
/// `default-members` and no `exclude`, whose `members` patterns name the
/// member, and whose member is not a workspace of its own. Anything else keeps
/// both checks, which is the old behaviour and never misses a test.
fn cargo_workspace_runs_member(repository_root: &Path, root: &str, member: &str) -> bool {
    let relative = if root.is_empty() {
        member
    } else {
        match member
            .strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
        {
            Some(rest) => rest,
            None => return false,
        }
    };
    let manifest = |directory: &str| {
        std::fs::read_to_string(repository_root.join(directory).join("Cargo.toml")).ok()
    };
    let (Some(root_manifest), Some(member_manifest)) = (manifest(root), manifest(member)) else {
        return false;
    };
    let sections = |text: &str| {
        text.lines()
            .map(str::trim)
            .filter(|line| line.starts_with('['))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    if sections(&member_manifest)
        .iter()
        .any(|section| section == "[workspace]")
    {
        return false;
    }
    let root_sections = sections(&root_manifest);
    if !root_sections.iter().any(|section| section == "[workspace]")
        || root_sections.iter().any(|section| section == "[package]")
    {
        return false;
    }
    let workspace: String = root_manifest
        .lines()
        .skip_while(|line| line.trim() != "[workspace]")
        .skip(1)
        // A multi-line `members = [` array's own lines do not start with `[`,
        // so the section is read up to the next table header.
        .take_while(|line| !line.trim().starts_with('['))
        .collect::<Vec<_>>()
        .join("\n");
    let key_lines = |key: &str| {
        workspace
            .lines()
            .any(|line| line.trim_start().starts_with(key))
    };
    if key_lines("default-members") || key_lines("exclude") {
        return false;
    }
    let Some(start) = workspace.find("members") else {
        return false;
    };
    let Some(open) = workspace[start..].find('[') else {
        return false;
    };
    let Some(close) = workspace[start + open..].find(']') else {
        return false;
    };
    workspace[start + open + 1..start + open + close]
        .split(',')
        .map(|entry| {
            entry
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim_end_matches('/')
        })
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| crate::ignore::glob_match(pattern, relative))
}
