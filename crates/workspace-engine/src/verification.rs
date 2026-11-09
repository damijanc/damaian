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
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::command_policy::relative_location;
use crate::error::{ClientError, Result};
use crate::finding::{Finding, FindingSource, Severity};
use crate::plan::{PlanReport, TaskPlan};
use crate::provenance::{ProvenanceCheck, build_partitions, provenance_reason, repair_targets};
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
        /// The run's exact termination. Absent from events written before spec
        /// 102 Task 3. [`Self::termination`] then derives only an ordinary
        /// `Exited` (or `Signalled` without a code) from what the old event
        /// recorded; it deliberately does not infer an environment reason such
        /// as `MissingExecutable` that the log never stored. A historical exit
        /// 127 therefore stays an ordinary failed exit of unknown provenance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        termination: Option<CheckTermination>,
    },
    Skipped(SkipReason),
    /// The check could not run for an environment reason: the process did not
    /// start, the executable is missing, policy refused it, or its test
    /// infrastructure was unavailable (spec 102 requirement 5). It is a
    /// failure for completion, but never a code-repair target.
    #[serde(rename_all = "camelCase")]
    Environment {
        termination: CheckTermination,
    },
}

impl CheckOutcome {
    /// The failure termination, or `None` when the check did not fail in a way
    /// that has a termination (a user decline or a missing browser target).
    pub fn termination(&self) -> Option<CheckTermination> {
        match self {
            Self::Ran {
                exit_code,
                termination,
                ..
            } => Some(termination.clone().unwrap_or(match exit_code {
                Some(code) => CheckTermination::Exited { exit_code: *code },
                None => CheckTermination::Signalled,
            })),
            Self::Skipped(SkipReason::Blocked { .. }) => Some(CheckTermination::PolicyRefused),
            Self::Skipped(SkipReason::DeclinedByUser | SkipReason::NoRunningTarget) => None,
            Self::Environment { termination } => Some(termination.clone()),
        }
    }

    /// `Some(0)` and nothing else.
    pub fn is_passed(&self) -> bool {
        matches!(
            self,
            Self::Ran {
                exit_code: Some(0),
                ..
            }
        )
    }
}

/// Why a check stopped, in a form that survives reload (spec 102).
///
/// Distinct from [`crate::command_runner::CommandTermination`]: a check can
/// also fail before a process starts, and that is an environment fact rather
/// than a code failure. `Exited` keeps the exit code because a different code
/// is a materially different termination for the fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckTermination {
    #[serde(rename_all = "camelCase")]
    Exited {
        exit_code: i32,
    },
    /// The command was killed or signalled; its exit code is unknown.
    Signalled,
    TimedOut,
    Cancelled,
    /// The process could not be started at all.
    SpawnFailed,
    /// The command names an executable that is not installed.
    MissingExecutable,
    /// The command policy refused to run it.
    PolicyRefused,
    /// Test infrastructure the check needs was unavailable.
    UnavailableInfrastructure,
}

impl CheckTermination {
    /// The wire and report name. `Exited` is the only variant with detail;
    /// callers that need the code read it from the variant.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Exited { .. } => "exited",
            Self::Signalled => "signalled",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::SpawnFailed => "spawn_failed",
            Self::MissingExecutable => "missing_executable",
            Self::PolicyRefused => "policy_refused",
            Self::UnavailableInfrastructure => "unavailable_infrastructure",
        }
    }

    /// Requirement 5: these never start a code repair turn. The helper states
    /// the rule once in the engine rather than letting every consumer re-derive
    /// it from the variant list.
    pub fn is_environment_failure(&self) -> bool {
        matches!(
            self,
            Self::SpawnFailed
                | Self::MissingExecutable
                | Self::PolicyRefused
                | Self::UnavailableInfrastructure
        )
    }

    /// The engine's wording for this termination.
    pub fn description(&self) -> String {
        match self {
            Self::Exited { exit_code } => format!("exit code {exit_code}"),
            Self::Signalled => "killed or signalled".to_string(),
            Self::TimedOut => "timed out".to_string(),
            Self::Cancelled => "cancelled".to_string(),
            Self::SpawnFailed => "the command could not be started".to_string(),
            Self::MissingExecutable => "the executable is not installed".to_string(),
            Self::PolicyRefused => "the command policy refused it".to_string(),
            Self::UnavailableInfrastructure => {
                "the test infrastructure was unavailable".to_string()
            }
        }
    }

    /// The one canonical mapping from a command result to check termination.
    /// Baseline construction and live verification both use it, so a
    /// fingerprint cannot disagree about what a run was.
    pub fn from_command(
        termination: crate::command_runner::CommandTermination,
        exit_code: Option<i32>,
    ) -> Self {
        match termination {
            crate::command_runner::CommandTermination::Exited => match exit_code {
                Some(127) => Self::MissingExecutable,
                Some(exit_code) => Self::Exited { exit_code },
                None => Self::Signalled,
            },
            crate::command_runner::CommandTermination::TimedOut => Self::TimedOut,
            crate::command_runner::CommandTermination::Cancelled => Self::Cancelled,
        }
    }
}

/// The environment termination a proposal execution error justifies.
///
/// Only two errors say something about the command's environment: the policy
/// refused it, or [`crate::command_runner::CommandRunner`] could not start its
/// process. Everything else — an unreadable proposal, an execution record that
/// could not be written, an audit failure, a cancelled turn — is an internal
/// failure. Returning `None` makes the caller propagate it rather than
/// disguising it as an environment report (spec 102 Task 3).
pub(crate) fn environment_termination(error: &ClientError) -> Option<CheckTermination> {
    match error {
        ClientError::PolicyBlocked(_) | ClientError::ApprovalRequired(_) => {
            Some(CheckTermination::PolicyRefused)
        }
        ClientError::CommandSpawnFailed(_) => Some(CheckTermination::SpawnFailed),
        _ => None,
    }
}

/// A redacted, canonical finding for fingerprinting and repair evidence
/// (spec 102 requirements 3 and 10).
///
/// Fields are private so [`FindingEvidence::from_finding`] is the normal
/// constructor. It copies only the stable, already-redacted parts of a
/// [`Finding`]: the transient id, creation time, origin reference and status
/// are deliberately absent, so a log reload cannot change a fingerprint.
/// `details` is kept for the model's repair evidence but is excluded from the
/// fingerprint and the comparison keys. `identity_tail` is the normalized,
/// bounded, redacted tail of an unparsed check's output; unlike `details` it is
/// part of the comparison key, because a generic finding has no other way to
/// distinguish a new failing test or a repair that removed one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingEvidence {
    source: FindingSource,
    severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_column: Option<u32>,
    summary: String,
    /// The finding's redacted [`Finding::details`], if any. Recorded so a
    /// reload and a worker restart render the same repair evidence; it is not
    /// part of a failure's identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    /// A normalized, bounded, already-redacted tail of a generic (unparsed)
    /// check's output. It is identity, not display: R10 added it because a
    /// generic finding's summary is only the first output line. Absent for a
    /// parsed finding and for records written before the fingerprint format
    /// version 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_tail: Option<String>,
}

impl FindingEvidence {
    /// Normalise a redacted finding into the form the fingerprint hashes.
    ///
    /// Whitespace is collapsed and a machine-local absolute range path is
    /// reduced to its final component, so moving the checkout does not change
    /// a fingerprint. Repository-relative paths are kept in full.
    pub fn from_finding(finding: &Finding) -> Self {
        let range = finding.range();
        Self {
            source: finding.source(),
            severity: finding.severity(),
            code: finding.code().map(normalized_text),
            path: range.map(|range| normalized_repository_path(&range.path)),
            start_line: range.map(|range| range.start_line),
            start_column: range.and_then(|range| range.start_column),
            summary: normalized_text(finding.summary()),
            // Already redacted and bounded by `Finding::new`; whitespace is
            // meaningful in a compiler block, so it is kept verbatim.
            details: finding.details().map(str::to_string),
            // `Finding::new` redacted the details before they reached here.
            // Normalizing after redaction is what keeps a secret out of the
            // identity tail even when it sits inside an otherwise volatile
            // token.
            identity_tail: if finding.source() == FindingSource::Command {
                finding.details().and_then(generic_identity_tail)
            } else {
                None
            },
        }
    }

    pub fn source(&self) -> FindingSource {
        self.source
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub fn start_line(&self) -> Option<u32> {
        self.start_line
    }

    pub fn start_column(&self) -> Option<u32> {
        self.start_column
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// The finding's redacted, bounded details as the repair request may show
    /// them. Not an input to the failure fingerprint or comparison keys.
    pub fn details(&self) -> Option<&str> {
        self.details.as_deref()
    }

    /// The normalized generic-output tail that is part of this finding's
    /// identity, when it has one. Absent for parsed findings and for records
    /// written before fingerprint format version 3.
    pub fn identity_tail(&self) -> Option<&str> {
        self.identity_tail.as_deref()
    }

    /// The summary as identity sees it. A generic summary embeds the check's
    /// first output line, which can contain the same volatile tokens as the
    /// tail; display keeps the original line.
    fn comparison_summary(&self) -> String {
        if self.source == FindingSource::Command {
            normalized_generic_text(&self.summary)
        } else {
            self.summary.clone()
        }
    }

    /// The stable comparison key for provenance and stagnation (spec 102).
    ///
    /// It deliberately omits `start_line` and `start_column`: a patch that
    /// adds or removes lines above a finding moves its location without
    /// creating a new failure. The location stays on the evidence, so the
    /// report and the repair request still show it. Checks that genuinely
    /// differ in source, severity, code, normalised path or normalised summary
    /// still produce different keys. A generic finding also contributes its
    /// normalized, bounded, redacted identity tail (R10), because its summary
    /// is only the first output line.
    pub(crate) fn canonical_key(&self) -> String {
        self.canonical_key_including_identity_tail(true)
    }

    /// The comparison key as it was for a record written before R10's identity
    /// tail existed. Provenance uses it only when the baseline has no tail at
    /// all, so an old baseline still classifies from its durable findings
    /// instead of treating every generic failure as new.
    pub(crate) fn legacy_canonical_key(&self) -> String {
        self.canonical_key_including_identity_tail(false)
    }

    fn canonical_key_including_identity_tail(&self, include_identity_tail: bool) -> String {
        let identity_tail = include_identity_tail
            .then_some(self.identity_tail.as_deref())
            .flatten();
        serde_json::to_string(&serde_json::json!({
            "source": self.source,
            "severity": self.severity,
            "code": self.code,
            "path": self.path,
            "summary": self.comparison_summary(),
            "identityTail": identity_tail,
        }))
        .expect("finding evidence is serializable")
    }

    /// The location-inclusive key. It is used only to recognise the occurrence
    /// that stayed put when the same finding also moved or multiplied, so the
    /// newly introduced occurrence is the one reported to repair.
    ///
    /// It is deliberately built from the same fields as [`Self::canonical_key`]
    /// plus the location. `details` is excluded: the repair text a model sees
    /// must not decide which occurrence is pre-existing.
    pub(crate) fn location_key(&self) -> String {
        self.location_key_including_identity_tail(true)
    }

    pub(crate) fn legacy_location_key(&self) -> String {
        self.location_key_including_identity_tail(false)
    }

    fn location_key_including_identity_tail(&self, include_identity_tail: bool) -> String {
        let identity_tail = include_identity_tail
            .then_some(self.identity_tail.as_deref())
            .flatten();
        serde_json::to_string(&serde_json::json!({
            "source": self.source,
            "severity": self.severity,
            "code": self.code,
            "path": self.path,
            "startLine": self.start_line,
            "startColumn": self.start_column,
            "summary": self.comparison_summary(),
            "identityTail": identity_tail,
        }))
        .expect("finding evidence is serializable")
    }
}

impl PartialEq for FindingEvidence {
    /// Equality is the finding's identity, not its display: `details` is
    /// transient output and is excluded exactly as it is from the fingerprint.
    /// Keeping it on the value lets a repair request render it without making
    /// two runs of the same failure unequal.
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.severity == other.severity
            && self.code == other.code
            && self.path == other.path
            && self.start_line == other.start_line
            && self.start_column == other.start_column
            && self.summary == other.summary
            && self.identity_tail == other.identity_tail
    }
}

impl Eq for FindingEvidence {}

/// A stable digest of a failed check result (spec 102 requirement 3).
///
/// It is derived only from [`CheckId`], normalised [`FindingEvidence`] and
/// [`CheckTermination`]. Raw output, secrets, marker ids, timestamps and
/// machine-local paths are not inputs, so an equivalent result produces the
/// same value regardless of the order the findings arrived in.
///
/// Version 3 adds a generic finding's normalized identity tail to the hashed
/// comparison key. Version 2 excluded a finding's line and column from it;
/// version 1 also hashed them. All older values remain loadable as opaque
/// strings. A stored baseline or provenance record still classifies from its
/// own durable findings: a version 1 or 2 generic baseline has no identity
/// tail, so comparison falls back to the pre-R10 key for that record instead of
/// reading every generic failure as new. A version 1 or 2 repair-history entry
/// cannot be re-derived, so it does not compare equal to a version 3 value and
/// the next attempt starts a fresh stagnation window.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FailureFingerprint(String);

/// The canonical fingerprint schema version written by [`FailureFingerprint::compute`].
pub const FAILURE_FINGERPRINT_VERSION: u32 = 3;

impl FailureFingerprint {
    /// The canonical fingerprint of one failed check result.
    pub fn compute(
        check: &CheckId,
        findings: &[FindingEvidence],
        termination: &CheckTermination,
    ) -> Self {
        // Set semantics: a duplicate report of the same finding is not a new
        // failure. Provenance separately compares multiplicity, so one
        // pre-existing occurrence plus a newly introduced identical occurrence
        // is still a new repair target.
        let canonical_findings = findings
            .iter()
            .map(FindingEvidence::canonical_key)
            .collect::<BTreeSet<_>>();
        let canonical = serde_json::json!({
            "version": FAILURE_FINGERPRINT_VERSION,
            "check": {
                "workingDirectory": normalized_repository_path(&check.working_directory),
                "command": check.command,
            },
            "termination": termination,
            "findings": canonical_findings,
        });
        let bytes = serde_json::to_vec(&canonical).expect("fingerprint input is serializable");
        Self(crate::hash::sha256(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// One fingerprint for a whole repair attempt, from the failures that made
    /// it due.
    ///
    /// A repair can target more than one check; the attempt's identity is the
    /// canonical set of `(CheckId, fingerprint)` pairs, not the order the shell
    /// happened to list them in. This is the value the bounded repair history
    /// compares, so two attempts with the same failing checks produce the same
    /// value and a materially different result does not.
    pub fn for_attempt(entries: &[(CheckId, FailureFingerprint)]) -> Self {
        let mut entries = entries.to_vec();
        entries.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.as_str().cmp(right.1.as_str()))
        });
        entries.dedup();
        let failures = entries
            .iter()
            .map(|(check, fingerprint)| {
                serde_json::json!({
                    "workingDirectory": normalized_repository_path(&check.working_directory),
                    "command": check.command,
                    "fingerprint": fingerprint,
                })
            })
            .collect::<Vec<_>>();
        let canonical = serde_json::json!({
            "version": FAILURE_FINGERPRINT_VERSION,
            "failures": failures,
        });
        let bytes = serde_json::to_vec(&canonical).expect("fingerprint input is serializable");
        Self(crate::hash::sha256(bytes))
    }
}

/// The task-local result recorded for a check before the patch that first
/// selected it was applied (spec 102 requirements 1–2, 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CheckBaseline {
    Passed,
    #[serde(rename_all = "camelCase")]
    Failed {
        termination: CheckTermination,
        fingerprint: FailureFingerprint,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        findings: Vec<FindingEvidence>,
    },
    /// The user declined the baseline. A later failure is unknown provenance,
    /// never assumed to be caused by the task.
    Declined,
    /// The check became required after the task had already changed files, so
    /// it has no task-start baseline. A later failure is `uncertain`: it is
    /// reported and may be repaired only after the user says so (spec 102 D2).
    Uncertain,
    /// The baseline could not run. Also unknown provenance, with the
    /// environment fact recorded for the user.
    #[serde(rename_all = "camelCase")]
    Unavailable {
        termination: CheckTermination,
        fingerprint: FailureFingerprint,
    },
}

impl CheckBaseline {
    /// The failure evidence, when the baseline ran and failed.
    pub fn failure_fingerprint(&self) -> Option<&FailureFingerprint> {
        match self {
            Self::Passed | Self::Declined | Self::Uncertain => None,
            Self::Failed { fingerprint, .. } | Self::Unavailable { fingerprint, .. } => {
                Some(fingerprint)
            }
        }
    }
}

/// The post-patch classifications (spec 102 requirement 4, plus D2's
/// `uncertain`). Only `new_regression` and the new portion of `worsened` may
/// enter automatic repair; this type only names the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureProvenance {
    NewRegression,
    PreExisting,
    Worsened,
    UnchangedFailure,
    EnvironmentFailure,
    /// A required check that first became required after the task had already
    /// changed files, so it has no task-start baseline. It is reported and may
    /// be repaired only after the user answers a question; it is never
    /// silently pre-existing or automatically repaired (spec 102 D2).
    Uncertain,
    UnknownProvenance,
}

impl FailureProvenance {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NewRegression => "new_regression",
            Self::PreExisting => "pre_existing",
            Self::Worsened => "worsened",
            Self::UnchangedFailure => "unchanged_failure",
            Self::EnvironmentFailure => "environment_failure",
            Self::Uncertain => "uncertain",
            Self::UnknownProvenance => "unknown_provenance",
        }
    }
}

/// One post-patch classification, with the evidence it was derived from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckProvenanceRecord {
    pub check: CheckId,
    pub provenance: FailureProvenance,
    pub termination: CheckTermination,
    pub fingerprint: FailureFingerprint,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<FindingEvidence>,
    /// The part of `findings` the patch introduced: all of it for a new
    /// regression, or the delta for a worsened result. Empty for every other
    /// classification, so a repair turn cannot accidentally receive a
    /// pre-existing failure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub introduced: Vec<FindingEvidence>,
}

/// One repair attempt's comparable state (spec 102 requirement 7).
///
/// The digests are canonical, content-safe values supplied by the repair
/// controller in later tasks. Keeping the history bounded is the engine's rule,
/// not a controller convention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairStateEntry {
    pub failure_fingerprint: FailureFingerprint,
    pub patch_digest: String,
    pub task_diff_digest: String,
}

/// The bounded window of repair states kept in [`VerificationState`]. Eight
/// entries cover the A→B→A oscillation plus the identical-consecutive cases
/// without retaining the whole session's history.
pub const MAX_REPAIR_STATE_ENTRIES: usize = 8;

/// A check's latest recorded run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRun {
    pub outcome: CheckOutcome,
    /// The hash of every file the task had changed when the check ran, so a
    /// later edit, rollback or patch can make the result stale (proposal §5.6).
    pub file_hashes: BTreeMap<String, String>,
    /// The mutation-batch generation this run validated (spec 105 §4). A later
    /// generation marks the run stale without deleting it. Absent from runs
    /// recorded before this spec, so it reads as generation 0.
    #[serde(default)]
    pub generation: u32,
}

impl CheckRun {
    /// Whether later mutations made this run's evidence stale (spec 105 §4).
    ///
    /// Stale when the mutation-batch generation advanced, or when a file this
    /// run validated changed. A deleted file is absent from `current_hashes`,
    /// which is a mismatch.
    pub fn is_stale(
        &self,
        current_hashes: &BTreeMap<String, String>,
        current_generation: u32,
    ) -> bool {
        self.generation != current_generation
            || self
                .file_hashes
                .iter()
                .any(|(path, hash)| current_hashes.get(path) != Some(hash))
    }
}

/// The most bytes of a generic check's normalized output tail that are part of
/// its failure identity (review fix R10, spec 102 requirement 3).
///
/// A generic finding is one finding for a whole unparsed check, so its summary
/// is only the first output line. The tail is what distinguishes "test A
/// failed" from "tests A and B failed" and what lets a repair that removes one
/// failing test look like progress rather than a repeated failure. It is
/// deliberately bounded: an unbounded tail would make identity depend on log
/// length.
pub const MAX_GENERIC_IDENTITY_TAIL_BYTES: usize = 4_000;

/// Prefix on a generic identity tail cut at
/// [`MAX_GENERIC_IDENTITY_TAIL_BYTES`].
pub const GENERIC_IDENTITY_TAIL_TRUNCATED: &str = "… [identity tail truncated]";

fn normalized_text(text: &str) -> String {
    let text = replace_absolute_paths(text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reduce an absolute POSIX path to its final component.
///
/// An absolute path is machine-local: the same result under another checkout
/// must fingerprint identically. The final component is repository content;
/// any `:line:column` suffix that follows it is preserved for the display
/// normaliser and removed separately for a generic identity tail, where R5
/// says location is not identity.
fn replace_absolute_paths(text: &str) -> String {
    static ABSOLUTE_PATH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(^|[\s(\[{<'"=])(/[^\s:()\[\]{}<>,'"]+)"#)
            .expect("the absolute-path normaliser is valid")
    });
    ABSOLUTE_PATH
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let path = &captures[2];
            let basename = path
                .rsplit('/')
                .find(|segment| !segment.is_empty())
                .unwrap_or(path);
            format!("{}{basename}", &captures[1])
        })
        .into_owned()
}

/// Normalize a generic check's text for identity comparison (spec 102 R10).
///
/// The shipped parsers explain `cargo`/`rustc`/`biome` failures; every other
/// check (`pytest`, `npm test`, `go test`, a shell script) becomes one generic
/// finding whose summary is only the command, the first non-empty output line
/// and the exit code. These rules make the rest of the redacted output stable
/// enough to be identity, in this order:
///
/// 1. ANSI colour/CSI and OSC escape sequences are removed.
/// 2. CRLF and lone CR line endings become LF.
/// 3. A `:line` / `:line:column` suffix on a file-like token is removed, so a
///    finding that moves because lines were added above it is not new (R5).
/// 4. A path under a temporary root (`/tmp`, `/private/tmp`, `/var/tmp`,
///    `/var/folders`, `/private/var/folders`) becomes `<TMPPATH>`, including a
///    line/column suffix.
/// 5. Every other absolute path is reduced to its final component, so a
///    different checkout root is the same failure.
/// 6. ISO 8601 timestamps become `<TIMESTAMP>` and `HH:MM:SS` clock values
///    become `<TIME>`.
/// 7. A number with a duration unit (`ns`, `ms`, `s`, `sec`, `min`, ...)
///    becomes `<DURATION>`.
/// 8. `pid 1234`, `pid=1234` and `process id: 1234` become `<PID>`.
/// 9. Per-line whitespace is collapsed, blank lines are dropped, and lines are
///    re-joined with `\n`.
///
/// Callers pass text that `Finding::new` already redacted. Nothing here can
/// reintroduce a secret, and the result is not the raw output.
fn normalized_generic_text(text: &str) -> String {
    static ANSI_CSI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\x1B\[[0-9;?]*[ -/]*[@-~]").expect("the ANSI CSI normaliser is valid")
    });
    static ANSI_OSC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\x1B\][^\x07\x1B]*(?:\x07|\x1B\\)").expect("the ANSI OSC normaliser is valid")
    });
    static LINE_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?P<file>[A-Za-z0-9_./\\-]+\.[A-Za-z0-9]+):\d{1,6}(?::\d{1,6})?")
            .expect("the location normaliser is valid")
    });
    static TEMP_PATH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?:/private)?/(?:tmp|var/tmp|var/folders)/[^\s:()\[\]{}<>,'"]*(?::\d{1,6}){0,2}"#,
        )
        .expect("the temporary-path normaliser is valid")
    });
    static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?\b")
            .expect("the timestamp normaliser is valid")
    });
    static TIME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b\d{1,2}:\d{2}:\d{2}(?:\.\d+)?\b").expect("the clock normaliser is valid")
    });
    static DURATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b\d+(?:\.\d+)?\s?(?:ns|µs|us|ms|s|sec|secs|seconds?|min|mins|minutes?|h|hr|hrs|hours?)\b",
        )
        .expect("the duration normaliser is valid")
    });
    static PID: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:pid|process id)\s*[:=#]?\s*\d+\b")
            .expect("the pid normaliser is valid")
    });

    let text = ANSI_CSI.replace_all(text, "");
    let text = ANSI_OSC.replace_all(&text, "");
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = LINE_SUFFIX.replace_all(&text, "${file}");
    let text = TEMP_PATH.replace_all(&text, "<TMPPATH>");
    let text = replace_absolute_paths(&text);
    let text = TIMESTAMP.replace_all(&text, "<TIMESTAMP>");
    let text = TIME.replace_all(&text, "<TIME>");
    let text = DURATION.replace_all(&text, "<DURATION>");
    let text = PID.replace_all(&text, "<PID>");
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn generic_identity_tail(text: &str) -> Option<String> {
    let normalized = normalized_generic_text(text);
    if normalized.is_empty() {
        return None;
    }
    if normalized.len() <= MAX_GENERIC_IDENTITY_TAIL_BYTES {
        return Some(normalized);
    }
    let mut start = normalized.len() - MAX_GENERIC_IDENTITY_TAIL_BYTES;
    while !normalized.is_char_boundary(start) {
        start += 1;
    }
    Some(format!(
        "{GENERIC_IDENTITY_TAIL_TRUNCATED}\n{}",
        &normalized[start..]
    ))
}

pub(crate) fn normalized_repository_path(path: &str) -> String {
    let mut path = path.trim().replace('\\', "/");
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    let path = path.trim_start_matches("./");
    if path.starts_with('/') {
        // A machine-local absolute path must not make the fingerprint depend
        // on where the checkout lives. The final component is the only part
        // that is repository content.
        path.rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or("")
            .to_string()
    } else {
        path.to_string()
    }
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
    /// Automatic repair stopped because progress stalled before the numeric
    /// limit (spec 102 requirement 7). The report carries the typed reason.
    RepairStalled,
    NoRepairApplied,
    NoChecksAvailable,
    OnlyDocumentationChanged,
    /// Checks still fail, but every failure is one the task started with and
    /// did not change: classified `pre_existing` or `unchanged_failure`
    /// against a real task-start baseline, with nothing introduced (spec 106
    /// requirement 11). Never a pass; it settles through the completion gate,
    /// which names each such check in `accepted_failures`.
    OnlyPreExistingFailures,
}

impl VerificationOutcome {
    /// The wire and report name, matching the serde representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::NothingVerified => "nothing_verified",
            Self::Failed => "failed",
            Self::RepairLimitExhausted => "repair_limit_exhausted",
            Self::RepairStalled => "repair_stalled",
            Self::NoRepairApplied => "no_repair_applied",
            Self::NoChecksAvailable => "no_checks_available",
            Self::OnlyDocumentationChanged => "only_documentation_changed",
            Self::OnlyPreExistingFailures => "only_pre_existing_failures",
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
        /// The exact checks a completion verification must run (review follow-up
        /// to R16). Empty for an ordinary patch or command due, which selects
        /// its checks normally. A non-empty list is what keeps an answer-time
        /// floor run from also auto-running a plan-declared check.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        checks: Vec<CheckId>,
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
        /// The mutation-batch generation this run validated (spec 105 §4).
        /// Absent from events written before this spec, so it replays as 0.
        /// Generation 0 is omitted on the wire so a check at the first step
        /// keeps the pre-spec event shape.
        #[serde(default, skip_serializing_if = "is_zero")]
        generation: u32,
    },
    #[serde(rename = "verification_finished", rename_all = "camelCase")]
    Finished {
        task_id: String,
        outcome: VerificationOutcome,
    },
    /// A check's task-local baseline, recorded before the patch that first
    /// selected it was applied (spec 102 Task 2). Absent from every historical
    /// log, so replay must not invent one.
    #[serde(rename = "verification_baseline", rename_all = "camelCase")]
    Baseline {
        task_id: String,
        check: CheckId,
        baseline: CheckBaseline,
    },
    /// One post-patch provenance classification (spec 102 Task 3).
    #[serde(rename = "verification_provenance", rename_all = "camelCase")]
    Provenance {
        task_id: String,
        record: CheckProvenanceRecord,
    },
    /// One repair attempt's comparable state (spec 102 Task 4).
    #[serde(rename = "verification_repair_state", rename_all = "camelCase")]
    RepairState {
        task_id: String,
        entry: RepairStateEntry,
    },
    /// The terminal repair-stall transition (spec 102 Task 4). One event
    /// records the current state and the typed reason, and replays as a
    /// finished verification with no due or in-progress run. A separate
    /// state-then-finish pair would leave a crash window in which replay sees
    /// the state but not the stop and could silently restart repair.
    #[serde(rename = "verification_repair_stalled", rename_all = "camelCase")]
    RepairStalled {
        task_id: String,
        entry: RepairStateEntry,
        stall: crate::stagnation::RepairStall,
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
            | Self::Finished { task_id, .. }
            | Self::Baseline { task_id, .. }
            | Self::Provenance { task_id, .. }
            | Self::RepairState { task_id, .. }
            | Self::RepairStalled { task_id, .. } => task_id,
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
    /// The exact checks this due must run. Empty means select by the normal
    /// per-change or completion rule; non-empty is a completion floor's own
    /// list (review follow-up to R16).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckId>,
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
    /// The task-local baseline per check (spec 102). Empty for every log
    /// written before this spec, and replay leaves it empty rather than
    /// guessing a provenance.
    pub baselines: BTreeMap<CheckId, CheckBaseline>,
    /// The latest post-patch classification per check (spec 102).
    pub provenance: BTreeMap<CheckId, CheckProvenanceRecord>,
    /// The bounded repair-state window, oldest first (spec 102 requirement 7).
    pub repair_history: Vec<RepairStateEntry>,
    /// The latest terminal repair stall, cleared by a new `Started`. Only a
    /// `RepairStalled` event sets it; no reader derives it from a check count.
    pub repair_stall: Option<crate::stagnation::RepairStall>,
    /// A `Started` with no later `Finished`. Distinct from `due`, which stays
    /// set until a `Finished` too: a run can be both due and in progress, and
    /// the completion gate reports each fact it can see.
    pub in_progress: bool,
    /// Private so the state cannot be constructed by hand into a shape the
    /// events do not support; the report's honesty rests on the fold.
    passed_before: BTreeSet<CheckId>,
    /// Checks whose `provenance` record was written after their latest run.
    /// A newer `Check` with no classification of its own (an interrupted
    /// verification) leaves the older verdict in `provenance` but not here, so
    /// it can never vouch for a run it did not see (spec 106 Task 1).
    provenance_current: BTreeSet<CheckId>,
}

impl VerificationState {
    /// The provenance record classifying `check`'s latest run, or `None` when
    /// the check has no classification or ran again after its latest one.
    pub fn current_provenance(&self, check: &CheckId) -> Option<&CheckProvenanceRecord> {
        if self.provenance_current.contains(check) {
            self.provenance.get(check)
        } else {
            None
        }
    }

    /// Apply one event. Public so a reader that replays a log in batches gets
    /// exactly the state a single fold produces.
    pub fn apply(&mut self, event: &VerificationEvent) {
        match event {
            VerificationEvent::Due {
                task_id,
                patch_id,
                paths,
                checks,
            } => {
                self.due = Some(VerificationDue {
                    task_id: task_id.clone(),
                    patch_id: patch_id.clone(),
                    paths: paths.clone(),
                    checks: checks.clone(),
                });
            }
            VerificationEvent::Started {
                attempt, checks, ..
            } => {
                self.attempt = *attempt;
                self.finished = None;
                self.repair_stall = None;
                self.in_progress = true;
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
                generation,
                ..
            } => {
                if passing(outcome) {
                    self.passed_before.insert(check.clone());
                }
                self.provenance_current.remove(check);
                self.checks.insert(
                    check.clone(),
                    CheckRun {
                        outcome: outcome.clone(),
                        file_hashes: file_hashes.clone(),
                        generation: *generation,
                    },
                );
            }
            VerificationEvent::Finished { outcome, .. } => {
                self.due = None;
                self.finished = Some(*outcome);
                self.repair_stall = None;
                self.in_progress = false;
            }
            VerificationEvent::Baseline {
                check, baseline, ..
            } => {
                self.baselines.insert(check.clone(), baseline.clone());
            }
            VerificationEvent::Provenance { record, .. } => {
                self.provenance.insert(record.check.clone(), record.clone());
                self.provenance_current.insert(record.check.clone());
            }
            VerificationEvent::RepairState { entry, .. }
            | VerificationEvent::RepairStalled { entry, .. } => {
                self.repair_history.push(entry.clone());
                if self.repair_history.len() > MAX_REPAIR_STATE_ENTRIES {
                    let excess = self.repair_history.len() - MAX_REPAIR_STATE_ENTRIES;
                    self.repair_history.drain(0..excess);
                }
                if let VerificationEvent::RepairStalled { stall, .. } = event {
                    // One event: the state, the typed stop, and the cleared
                    // due/in-progress facts are all durable at the same log
                    // position, so replay cannot see half of them.
                    self.repair_stall = Some(stall.clone());
                    self.finished = Some(VerificationOutcome::RepairStalled);
                    self.due = None;
                    self.in_progress = false;
                }
            }
        }
        self.regressions = self
            .checks
            .iter()
            .filter(|(check, run)| {
                !passing(&run.outcome)
                    && !run
                        .outcome
                        .termination()
                        .is_some_and(|termination| termination.is_environment_failure())
                    && self.passed_before.contains(*check)
            })
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
    outcome.is_passed()
}

fn is_zero(generation: &u32) -> bool {
    *generation == 0
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
    /// The engine's wording for `reason`. A renderer prints this and does not
    /// re-derive a label from the enum.
    #[serde(default)]
    pub reason_label: String,
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
    /// The engine-owned provenance partitions (spec 102 requirement 9). Every
    /// failing result and every environment refusal appears in exactly one
    /// group; only the two eligible groups produce repair targets.
    #[serde(default)]
    pub provenance: Vec<crate::provenance::ProvenancePartition>,
    /// The normalized introduced evidence a repair turn may receive. Empty means
    /// no automatic repair: pre-existing, environment and unknown failures all
    /// stay out of this list.
    #[serde(default)]
    pub repair_targets: Vec<crate::provenance::RepairTarget>,
    /// Flattened engine-owned actions for the panel and CLI.
    #[serde(default)]
    pub actions: Vec<crate::provenance::VerificationAction>,
    /// The typed stop when automatic repair stalled (spec 102 requirement 7).
    /// The reason, label, explanation and action are engine data; a shell
    /// prints them and never infers a stall kind from a check count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_stall: Option<crate::stagnation::RepairStall>,
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
        let current_generation = plan.mutation_generation();
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
            provenance: Vec::new(),
            repair_targets: Vec::new(),
            actions: Vec::new(),
            repair_stall: state.repair_stall.clone(),
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
                    reason_label: reason.description(),
                }),
                CheckOutcome::Environment { termination } => {
                    if run.is_stale(current_hashes, current_generation) {
                        report.stale.push(check.clone());
                        continue;
                    }
                    report.failed.push(FailedCheck {
                        check: check.clone(),
                        reason: termination.description(),
                    });
                }
                CheckOutcome::Ran { exit_code, .. } => {
                    if run.is_stale(current_hashes, current_generation) {
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
        // The provenance view is built from durable classifications. A failing
        // check with no record is a historical log or an interrupted write: it
        // is reported as unknown (or environment, when its stored termination
        // says so) rather than guessed from a baseline that replay cannot read.
        let mut entries = Vec::new();
        for check in &state.selected {
            let Some(run) = state.checks.get(check) else {
                continue;
            };
            if passing(&run.outcome) || run.is_stale(current_hashes, current_generation) {
                continue;
            }
            let Some(termination) = run.outcome.termination() else {
                // Declined by the user, or no running browser target: not a
                // failure with a provenance.
                continue;
            };
            if let Some(record) = state.provenance.get(check) {
                entries.push((
                    record.provenance,
                    ProvenanceCheck::new(
                        check.clone(),
                        provenance_reason(&record.termination, record.findings.len()),
                        record.termination.clone(),
                        Some(record.fingerprint.clone()),
                        record.findings.clone(),
                        record.introduced.clone(),
                    ),
                ));
            } else {
                let provenance = if termination.is_environment_failure() {
                    FailureProvenance::EnvironmentFailure
                } else {
                    FailureProvenance::UnknownProvenance
                };
                entries.push((
                    provenance,
                    ProvenanceCheck::new(
                        check.clone(),
                        provenance_reason(&termination, 0),
                        termination,
                        None,
                        Vec::new(),
                        Vec::new(),
                    ),
                ));
            }
        }
        // A stall is terminal for automatic repair: the introduced failures
        // stay visible in their provenance partition, but no target can reach
        // a repair turn and the redundant proposal is never offered.
        report.repair_targets = if report.repair_stall.is_some() {
            Vec::new()
        } else {
            repair_targets(&entries)
        };
        report.provenance = build_partitions(entries);
        report.actions = report
            .provenance
            .iter()
            .filter_map(|partition| partition.action.clone())
            .collect();
        if let Some(stall) = &report.repair_stall {
            // The part of the report that says a repair started is false once
            // it stalled; the engine's stall action replaces it.
            report.actions.retain(|action| {
                !matches!(
                    action.kind,
                    crate::provenance::VerificationActionKind::ReviewIntroducedFailure
                )
            });
            report.actions.push(stall.action.clone());
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
                "Verification failed: the repair limit was exhausted and {}. {plan_note}",
                self.failure_phrase()
            );
        }
        if let Some(stall) = &self.repair_stall {
            // The engine's explanation already ends with a sentence period;
            // adding another would print double punctuation.
            return format!("{}: {} {plan_note}", stall.label, stall.explanation);
        }
        if let Some(VerificationOutcome::NoRepairApplied) = self.outcome {
            return format!(
                "Verification stopped because no repair patch was applied and {}. {plan_note}",
                self.failure_phrase()
            );
        }
        if let Some(VerificationOutcome::OnlyPreExistingFailures) = self.outcome {
            return format!(
                "Only failures that were already there before this task remain: {}. The task did not cause or change them. {plan_note}",
                self.failure_phrase()
            );
        }
        if !self.failed.is_empty() || matches!(self.outcome, Some(VerificationOutcome::Failed)) {
            return format!(
                "Verification failed: {}. {plan_note}",
                self.failure_phrase()
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
            if !self.provenance.is_empty() {
                return format!("No check passed: {}. {plan_note}", self.failure_phrase());
            }
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

    /// The engine-owned breakdown for the failed summary. It names introduced
    /// regressions separately from pre-existing, environment and unknown
    /// failures instead of collapsing every failure into one noun.
    fn failure_phrase(&self) -> String {
        if self.provenance.is_empty() {
            return format!(
                "{} still {}",
                count(self.failed.len(), "check"),
                if self.failed.len() == 1 {
                    "fails"
                } else {
                    "fail"
                }
            );
        }
        self.provenance
            .iter()
            .map(|partition| {
                crate::provenance::provenance_count(partition.provenance, partition.checks.len())
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The normalized repair request for the eligible partitions. `None` means
    /// automatic repair must not start, and is the only representation a
    /// caller needs to check.
    pub fn repair_request(&self) -> Option<String> {
        if self.repair_stall.is_some() {
            return None;
        }
        crate::provenance::render_repair_request(&self.repair_targets)
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

        // Spec 102 requirement 9: each provenance group is its own typed
        // section. The labels, reasons and actions are engine-owned; a
        // renderer prints them without deciding what a check's outcome means.
        for partition in &self.provenance {
            let value = partition
                .checks
                .iter()
                .map(ProvenanceCheck::render)
                .collect::<Vec<_>>()
                .join("\n                     ");
            lines.push(section(&partition.label, &value));
        }
        if let Some(stall) = &self.repair_stall {
            lines.push(section(&stall.label, &stall.explanation));
        }
        let actions = if self.actions.is_empty() {
            "none".to_string()
        } else {
            self.actions
                .iter()
                .map(|action| format!("{}: {}", action.label, action.detail))
                .collect::<Vec<_>>()
                .join("\n                     ")
        };
        lines.push(section("What you can do", &actions));

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
        if self.repair_stall.is_some() {
            notes.push("automatic repair stopped because it stalled".to_string());
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
        if !self.actions.is_empty() {
            return self
                .actions
                .iter()
                .map(|action| format!("{}: {}", action.label, action.detail))
                .collect::<Vec<_>>()
                .join("; ");
        }
        "none".to_string()
    }
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
/// Since review fix R16 (spec 23 decision D2 piece 2), per-change selection is
/// the **owning root only** (`RepositoryMap::root_for_path`). The enclosing
/// roots are no longer run after every change: full checks belong at task
/// completion, where [`floor_checks`] keeps them, and at spec/CI level. Making
/// a member edit run the workspace suite on every patch was the behaviour this
/// split removes. Documentation selects nothing. Sorted and de-duplicated, so
/// the caller can compare selections directly.
pub fn select_roots(map: &RepositoryMap, changed: &[String]) -> Vec<String> {
    let mut selected = BTreeSet::new();
    for path in changed {
        if is_documentation(path) {
            continue;
        }
        selected.insert(map.root_for_path(path).to_string());
    }
    selected.into_iter().collect()
}

/// The roots whose checks the task-completion floor requires.
///
/// The floor keeps the owning root **and every enclosing root**: a change to a
/// member is a change to its workspace, and the workspace root's check is what
/// catches a member breaking a sibling (spec 23 §5.2, spec 101/105 R15). This
/// is deliberately separate from [`select_roots`] so narrowing per-change
/// selection cannot shrink the floor. Documentation selects nothing.
pub fn select_floor_roots(map: &RepositoryMap, changed: &[String]) -> Vec<String> {
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

/// The detected checks of the roots a set of touched paths selects.
///
/// The completion gate's floor (spec 101/105 review fix R15): a task that
/// changed files has required checks even when its plan declares none. This is
/// the owning root plus every enclosing root ([`select_floor_roots`]), with
/// documentation selecting nothing, but it only reads the persisted map, so a
/// gate report stores no command proposal and executes nothing. The result is
/// sorted by [`CheckId`], so merging it with the plan's required checks is
/// deterministic.
pub fn floor_checks(
    repository_root: &Path,
    map: &RepositoryMap,
    touched_paths: &[String],
) -> Vec<CheckId> {
    let roots = select_floor_roots(map, touched_paths);
    if roots.is_empty() {
        return Vec::new();
    }
    let mut restricted = map.restricted_to(&roots);
    drop_covered_member_checks(repository_root, &mut restricted);
    let mut checks = BTreeSet::new();
    for root in &restricted.roots {
        for command in &root.commands {
            checks.insert(CheckId::new(root.path.as_str(), command.command.as_str()));
        }
    }
    checks.into_iter().collect()
}

/// The detected checks of the roots a command run in `working_directory` could
/// touch (spec 102 review fix R18).
///
/// A command's effects are only known after it runs, so the command baseline
/// covers the floor of every root that owns or encloses its working directory
/// plus every root beneath it ([`RepositoryMap::roots_for_command`]). That is
/// the same detected-check set the completion gate would require for a file
/// written anywhere in that subtree. The plan's required checks are merged
/// separately; this function reads only the persisted map and runs nothing.
pub fn command_floor_checks(
    repository_root: &Path,
    map: &RepositoryMap,
    working_directory: &str,
) -> Vec<CheckId> {
    let roots = map.roots_for_command(working_directory);
    if roots.is_empty() {
        return Vec::new();
    }
    let mut restricted = map.restricted_to(&roots);
    drop_covered_member_checks(repository_root, &mut restricted);
    let mut checks = BTreeSet::new();
    for root in &restricted.roots {
        for command in &root.commands {
            checks.insert(CheckId::new(root.path.as_str(), command.command.as_str()));
        }
    }
    checks.into_iter().collect()
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
/// Per-change selection is the owning root only since review fix R16; the
/// task-completion floor uses [`propose_completion_checks`]. `propose_detected_validations`
/// is the only discovery path: this function restricts the map first, so
/// nothing is stored and audited that will not run, and then calls it
/// unchanged (proposal §5.2, `context.md` §2).
pub fn propose_checks(
    orchestrator: &ValidationOrchestrator,
    repository_root: &Path,
    map: &RepositoryMap,
    changed: &[String],
) -> Result<Selection> {
    propose_checks_for_roots(
        orchestrator,
        repository_root,
        map,
        &select_roots(map, changed),
        true,
    )
}

/// Select the task-completion floor's roots and ask for their detected checks.
///
/// This is the same set [`floor_checks`] reports, kept as proposals so a
/// completion verification can run it through the ordinary approval and
/// command-policy path. Per-change selection uses [`propose_checks`] instead;
/// the split is what lets a member change run only the member's check while the
/// enclosing roots still gate completion (review fix R16).
///
/// The completion run does not drop a member's `cargo test` as covered by its
/// workspace root. That de-duplication is safe for a one-off obligation, but a
/// completion run also refreshes checks the task already selected: a repair
/// patch that changed a member after its per-change check passed would
/// otherwise leave that check stale with nothing left to rerun it.
pub fn propose_completion_checks(
    orchestrator: &ValidationOrchestrator,
    repository_root: &Path,
    map: &RepositoryMap,
    changed: &[String],
) -> Result<Selection> {
    propose_checks_for_roots(
        orchestrator,
        repository_root,
        map,
        &select_floor_roots(map, changed),
        false,
    )
}

fn propose_checks_for_roots(
    orchestrator: &ValidationOrchestrator,
    repository_root: &Path,
    map: &RepositoryMap,
    roots: &[String],
    drop_covered: bool,
) -> Result<Selection> {
    if roots.is_empty() {
        return Ok(Selection::OnlyDocumentation);
    }
    let mut restricted = map.restricted_to(roots);
    if drop_covered {
        drop_covered_member_checks(repository_root, &mut restricted);
    }
    let proposals = orchestrator.propose_detected_validations(repository_root, &restricted)?;
    Ok(selection_from_proposals(repository_root, roots, proposals))
}

/// Propose exactly the requested checks, with no other command from their
/// roots.
///
/// Review follow-up to R16: an answer-time completion run must run only the
/// floor checks that are missing or stale, never a plan-declared check the
/// model owns. Restricting the map to the identity's root and then retaining
/// only the requested commands means nothing is stored that will not run.
pub fn propose_checks_exactly(
    orchestrator: &ValidationOrchestrator,
    repository_root: &Path,
    map: &RepositoryMap,
    checks: &[CheckId],
) -> Result<Selection> {
    if checks.is_empty() {
        return Ok(Selection::OnlyDocumentation);
    }
    let roots: Vec<String> = checks
        .iter()
        .map(|check| check.working_directory.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut restricted = map.restricted_to(&roots);
    for root in &mut restricted.roots {
        root.commands.retain(|command| {
            checks.iter().any(|check| {
                check.working_directory == root.path && check.command == command.command
            })
        });
    }
    let proposals = orchestrator.propose_detected_validations(repository_root, &restricted)?;
    Ok(selection_from_proposals(repository_root, &roots, proposals))
}

/// Adds the plan-declared checks the gate says must run again to a selection.
///
/// Spec 106 Task 6: a finished step's required check can only be refreshed by
/// the verification loop (see `CompletionGateService::plan_checks_needing_rerun`).
/// A check the selection already proposes is not proposed twice. Each added
/// check keeps the gate's identity, so its `CheckRun` is the one the gate
/// reads, and is proposed through the ordinary command policy.
pub fn with_plan_checks(
    orchestrator: &ValidationOrchestrator,
    repository_root: &Path,
    selection: Selection,
    checks: &[CheckId],
) -> Result<Selection> {
    let mut proposed = match selection {
        Selection::Proposed(proposed) => proposed,
        other if checks.is_empty() => return Ok(other),
        _ => Vec::new(),
    };
    for check in checks {
        if proposed.iter().any(|(existing, _)| existing == check) {
            continue;
        }
        let working_directory = if check.working_directory.is_empty() {
            repository_root.to_path_buf()
        } else {
            repository_root.join(&check.working_directory)
        };
        let proposal =
            orchestrator.propose_plan_check(repository_root, &working_directory, &check.command)?;
        proposed.push((check.clone(), proposal));
    }
    Ok(Selection::Proposed(proposed))
}

fn selection_from_proposals(
    repository_root: &Path,
    roots: &[String],
    proposals: Vec<CommandProposal>,
) -> Selection {
    if proposals.is_empty() {
        return Selection::NoChecksAvailable {
            roots: roots.to_vec(),
        };
    }
    Selection::Proposed(
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
    )
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
