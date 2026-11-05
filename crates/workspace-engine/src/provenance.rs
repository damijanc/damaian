//! Post-patch provenance classification and repair targets (spec 102 Task 3).
//!
//! The classification rules live in the engine, not in a shell or a CLI. A
//! [`PostPatchResult`] carries only redacted [`FindingEvidence`] and a
//! [`CheckTermination`]; nothing here reads command output, so the evidence a
//! model receives can never be a raw log.
//!
//! Only two classifications may start automatic repair:
//!
//! * `new_regression`: the check passed before the patch and fails now, so all
//!   of its evidence was introduced by the patch.
//! * `worsened`: the check already failed before the patch, and the result
//!   contains evidence the baseline did not, so only that delta is introduced.
//!
//! Environment failures and unknown provenance are facts for the user. They
//! never become a repair target, because guessing that the task caused them is
//! exactly the failure this specification exists to prevent.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::finding::Severity;
use crate::verification::{
    CheckBaseline, CheckId, CheckProvenanceRecord, CheckTermination, FailureFingerprint,
    FailureProvenance, FindingEvidence,
};

/// One live, post-patch check result in the form classification uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostPatchResult {
    pub check: CheckId,
    pub termination: CheckTermination,
    pub findings: Vec<FindingEvidence>,
}

impl PostPatchResult {
    pub fn new(
        check: CheckId,
        termination: CheckTermination,
        findings: Vec<FindingEvidence>,
    ) -> Self {
        Self {
            check,
            termination,
            findings,
        }
    }

    /// The canonical fingerprint from Task 1. No second canonicalisation rule
    /// lives here: the fingerprint is exactly the engine's model.
    pub fn fingerprint(&self) -> FailureFingerprint {
        FailureFingerprint::compute(&self.check, &self.findings, &self.termination)
    }
}

/// A classified result, with the full redacted evidence and the evidence a
/// repair turn may receive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceClassification {
    pub provenance: FailureProvenance,
    pub check: CheckId,
    pub termination: CheckTermination,
    pub fingerprint: FailureFingerprint,
    /// The complete normalised result, kept for the user's report.
    pub findings: Vec<FindingEvidence>,
    /// Only the part the patch introduced: all findings for a new regression,
    /// or the delta for a worsened result. Empty for every other class.
    pub introduced: Vec<FindingEvidence>,
}

impl ProvenanceClassification {
    /// Requirement 4: only these two classes enter automatic repair.
    pub fn repair_eligible(&self) -> bool {
        matches!(
            self.provenance,
            FailureProvenance::NewRegression | FailureProvenance::Worsened
        )
    }

    pub fn record(&self) -> CheckProvenanceRecord {
        CheckProvenanceRecord {
            check: self.check.clone(),
            provenance: self.provenance,
            termination: self.termination.clone(),
            fingerprint: self.fingerprint.clone(),
            findings: self.findings.clone(),
            introduced: self.introduced.clone(),
        }
    }
}

/// Classify one post-patch result against its task-local baseline.
///
/// `previous` is the task's earlier classification for the same check, when
/// there is one. It is what makes `pre_existing` and `unchanged_failure`
/// deterministic: the first result that matches a failed baseline is
/// `pre_existing`, and an identical result after a repair is
/// `unchanged_failure`. Both are excluded from repair.
pub fn classify(
    result: &PostPatchResult,
    baseline: Option<&CheckBaseline>,
    previous: Option<&CheckProvenanceRecord>,
) -> ProvenanceClassification {
    let fingerprint = result.fingerprint();
    if result.termination.is_environment_failure() {
        return classification(
            FailureProvenance::EnvironmentFailure,
            result,
            fingerprint,
            Vec::new(),
        );
    }
    let Some(baseline) = baseline else {
        return classification(
            FailureProvenance::UnknownProvenance,
            result,
            fingerprint,
            Vec::new(),
        );
    };
    match baseline {
        CheckBaseline::Passed => classification(
            FailureProvenance::NewRegression,
            result,
            fingerprint,
            result.findings.clone(),
        ),
        CheckBaseline::Declined | CheckBaseline::Unavailable { .. } => classification(
            FailureProvenance::UnknownProvenance,
            result,
            fingerprint,
            Vec::new(),
        ),
        CheckBaseline::Failed {
            termination: baseline_termination,
            findings: baseline_findings,
            ..
        } => {
            // Compare the findings themselves, not the stored hash. That keeps
            // a location shift from looking like a new failure and lets a
            // baseline written with the previous fingerprint format classify
            // from its durable evidence. Multiplicity is part of the
            // comparison: one occurrence plus a new identical occurrence is a
            // change even though the fingerprint's set is the same.
            if result.termination == *baseline_termination
                && same_findings(&result.findings, baseline_findings)
            {
                // A previous *code* classification with the same evidence
                // means an earlier attempt already saw this failure. An
                // unknown or environment record is not such a proof: now that
                // a failed baseline exists, the first matching result is the
                // pre-existing one.
                let previous_matches = previous.is_some_and(|previous| {
                    matches!(
                        previous.provenance,
                        FailureProvenance::PreExisting
                            | FailureProvenance::UnchangedFailure
                            | FailureProvenance::Worsened
                            | FailureProvenance::NewRegression
                    ) && (previous.fingerprint == fingerprint
                        || (previous.termination == result.termination
                            && same_findings(&previous.findings, &result.findings)))
                });
                let provenance = if previous_matches {
                    FailureProvenance::UnchangedFailure
                } else {
                    FailureProvenance::PreExisting
                };
                return classification(provenance, result, fingerprint, Vec::new());
            }
            // A different fingerprint is only "worsened" when the patch added
            // evidence. A subset of the baseline's failures — fewer findings,
            // or the same findings with a different exit code — is not a new
            // failure, so it gets no repair target.
            let introduced = introduced_findings(&result.findings, baseline_findings);
            if introduced.is_empty() {
                classification(
                    FailureProvenance::UnchangedFailure,
                    result,
                    fingerprint,
                    Vec::new(),
                )
            } else {
                classification(FailureProvenance::Worsened, result, fingerprint, introduced)
            }
        }
    }
}

fn classification(
    provenance: FailureProvenance,
    result: &PostPatchResult,
    fingerprint: FailureFingerprint,
    introduced: Vec<FindingEvidence>,
) -> ProvenanceClassification {
    ProvenanceClassification {
        provenance,
        check: result.check.clone(),
        termination: result.termination.clone(),
        fingerprint,
        findings: result.findings.clone(),
        introduced,
    }
}

/// Whether two results contain the same findings with the same multiplicity.
///
/// The comparison key excludes line and column (spec 102 R5), so a finding that
/// moved because the patch added lines above it is still the same finding.
fn same_findings(current: &[FindingEvidence], baseline: &[FindingEvidence]) -> bool {
    counts(current) == counts(baseline)
}

fn counts(findings: &[FindingEvidence]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for finding in findings {
        *counts.entry(finding.canonical_key()).or_insert(0) += 1;
    }
    counts
}

/// Evidence present now that the baseline did not have.
///
/// Multiplicity matters: if the baseline had one occurrence and the result has
/// two identical ones, exactly one occurrence is introduced. Occurrences that
/// still sit at the same location are matched first, so the repaired finding is
/// the one that actually moved or multiplied rather than an unchanged twin.
///
/// Only errors can be introduced repair evidence. A new warning or info line on
/// a check that already failed is visible in the report but does not make the
/// result `worsened` and is never handed to automatic repair.
fn introduced_findings(
    current: &[FindingEvidence],
    baseline: &[FindingEvidence],
) -> Vec<FindingEvidence> {
    let mut baseline_by_key = counts(baseline);
    let mut baseline_by_location = BTreeMap::<String, usize>::new();
    for finding in baseline {
        *baseline_by_location
            .entry(finding.location_key())
            .or_insert(0) += 1;
    }

    let mut matched = vec![false; current.len()];
    for (index, finding) in current.iter().enumerate() {
        let location = finding.location_key();
        if let Some(remaining) = baseline_by_location.get_mut(&location)
            && *remaining > 0
        {
            *remaining -= 1;
            matched[index] = true;
            if let Some(remaining) = baseline_by_key.get_mut(&finding.canonical_key()) {
                *remaining -= 1;
            }
        }
    }
    for (index, finding) in current.iter().enumerate() {
        if matched[index] {
            continue;
        }
        if let Some(remaining) = baseline_by_key.get_mut(&finding.canonical_key())
            && *remaining > 0
        {
            *remaining -= 1;
            matched[index] = true;
        }
    }

    current
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched[*index])
        .map(|(_, finding)| finding)
        .filter(|finding| finding.severity() == Severity::Error)
        .cloned()
        .collect()
}

/// The engine's label for one classification. Renderers print this; they do not
/// derive it from the enum or from check counts.
pub fn provenance_label(provenance: FailureProvenance) -> &'static str {
    match provenance {
        FailureProvenance::NewRegression => "Introduced regressions",
        FailureProvenance::Worsened => "Worsened failures",
        FailureProvenance::PreExisting => "Pre-existing failures",
        FailureProvenance::UnchangedFailure => "Unchanged failures",
        FailureProvenance::EnvironmentFailure => "Environment failures",
        FailureProvenance::UnknownProvenance => "Unknown provenance",
    }
}

/// One sentence explaining why this group starts no repair, or which part it
/// does hand to repair.
pub fn provenance_summary(provenance: FailureProvenance) -> &'static str {
    match provenance {
        FailureProvenance::NewRegression => {
            "These checks passed before the patch and fail now. Automatic repair receives only these findings."
        }
        FailureProvenance::Worsened => {
            "These checks failed before the patch and now report new findings. Automatic repair receives only the new findings."
        }
        FailureProvenance::PreExisting => {
            "These checks failed before the patch too, so no repair was attempted."
        }
        FailureProvenance::UnchangedFailure => {
            "These checks still fail the same way, so no repair was attempted."
        }
        FailureProvenance::EnvironmentFailure => {
            "These checks could not run for an environment reason. No code repair was attempted."
        }
        FailureProvenance::UnknownProvenance => {
            "There is no usable pre-patch baseline, so Damaian cannot tell whether the patch caused these failures. No repair was attempted."
        }
    }
}

/// The engine-owned action for a partition. A renderer only displays it.
pub fn provenance_action(
    provenance: FailureProvenance,
    checks: &[CheckId],
) -> Option<VerificationAction> {
    let names = checks
        .iter()
        .map(CheckId::label)
        .collect::<Vec<_>>()
        .join(", ");
    let (kind, label, detail) = match provenance {
        FailureProvenance::NewRegression => (
            VerificationActionKind::ReviewIntroducedFailure,
            "Review the repair",
            "Damaian started a repair turn for the introduced failures.",
        ),
        FailureProvenance::Worsened => (
            VerificationActionKind::ReviewIntroducedFailure,
            "Review the repair of the new failures",
            "Damaian started a repair turn for the new findings only.",
        ),
        FailureProvenance::PreExisting => (
            VerificationActionKind::ReviewPreExistingFailure,
            "Review the pre-existing failure",
            "The check failed before the patch. Fix it separately if you want it addressed.",
        ),
        FailureProvenance::UnchangedFailure => (
            VerificationActionKind::ReviewUnchangedFailure,
            "Review the unchanged failure",
            "The check still fails the same way. Damaian did not start another repair.",
        ),
        FailureProvenance::EnvironmentFailure => (
            VerificationActionKind::FixEnvironment,
            "Fix the environment",
            "Make the command available, restore the refused policy, or provide the missing test infrastructure, then run verification again.",
        ),
        FailureProvenance::UnknownProvenance => (
            VerificationActionKind::ProvideBaseline,
            "Review the failure and provide a baseline",
            "A later patch can offer a baseline for this check. Damaian will not assume this failure was caused by the task.",
        ),
    };
    Some(VerificationAction {
        kind,
        label: label.to_string(),
        detail: format!("{detail} Affected: {names}."),
    })
}

/// A count phrase the summary can use. Singular and plural are the engine's
/// wording, not a renderer's.
pub fn provenance_count(provenance: FailureProvenance, count: usize) -> String {
    let (singular, plural) = match provenance {
        FailureProvenance::NewRegression => ("introduced regression", "introduced regressions"),
        FailureProvenance::Worsened => ("worsened failure", "worsened failures"),
        FailureProvenance::PreExisting => ("pre-existing failure", "pre-existing failures"),
        FailureProvenance::UnchangedFailure => ("unchanged failure", "unchanged failures"),
        FailureProvenance::EnvironmentFailure => ("environment failure", "environment failures"),
        FailureProvenance::UnknownProvenance => (
            "failure of unknown provenance",
            "failures of unknown provenance",
        ),
    };
    format!("{count} {}", if count == 1 { singular } else { plural })
}

/// One check inside a provenance partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceCheck {
    pub check: CheckId,
    pub label: String,
    /// The engine's one-line description of why the check is in this group.
    pub reason: String,
    pub termination: CheckTermination,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<FailureFingerprint>,
    /// The full redacted result, which the user's report may show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<FindingEvidence>,
    /// The full redacted evidence as the engine's canonical formatter renders
    /// it, one string per finding. A shell prints these verbatim; it must not
    /// rebuild the text from [`FindingEvidence`]'s fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_lines: Vec<String>,
    /// The part a repair turn may receive. Empty outside the two eligible
    /// classes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub introduced: Vec<FindingEvidence>,
}

impl ProvenanceCheck {
    /// Build one group entry, deriving the display label and the canonical
    /// redacted evidence lines so no renderer has to derive them again.
    pub fn new(
        check: CheckId,
        reason: String,
        termination: CheckTermination,
        fingerprint: Option<FailureFingerprint>,
        evidence: Vec<FindingEvidence>,
        introduced: Vec<FindingEvidence>,
    ) -> Self {
        let label = check.label();
        let evidence_lines = evidence.iter().map(evidence_line).collect();
        Self {
            check,
            label,
            reason,
            termination,
            fingerprint,
            evidence,
            evidence_lines,
            introduced,
        }
    }

    /// The report line for this check, with its full redacted evidence. The
    /// repair request is rendered separately from `introduced`, so this full
    /// view never reaches the model through that path.
    pub fn render(&self) -> String {
        let mut text = format!("{} — {}", self.label, self.reason);
        for line in &self.evidence_lines {
            text.push_str(&format!("\n                       - {line}"));
        }
        text
    }
}

/// One typed partition of the verification report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenancePartition {
    pub provenance: FailureProvenance,
    /// The engine's heading. A renderer prints this verbatim.
    pub label: String,
    /// The engine's explanation of the group and its repair treatment.
    pub summary: String,
    pub repair_eligible: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<ProvenanceCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<VerificationAction>,
}

/// An action the report offers the user. It is a fact from the engine, not a
/// decision a shell makes from a check outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationAction {
    pub kind: VerificationActionKind,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationActionKind {
    ReviewIntroducedFailure,
    ReviewPreExistingFailure,
    ReviewUnchangedFailure,
    FixEnvironment,
    ProvideBaseline,
    ReviewStalledRepair,
}

impl VerificationActionKind {
    /// The wire and report name, matching the serde representation. Renderers
    /// and evaluations name an action from this value rather than re-deriving
    /// one from its label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReviewIntroducedFailure => "review_introduced_failure",
            Self::ReviewPreExistingFailure => "review_pre_existing_failure",
            Self::ReviewUnchangedFailure => "review_unchanged_failure",
            Self::FixEnvironment => "fix_environment",
            Self::ProvideBaseline => "provide_baseline",
            Self::ReviewStalledRepair => "review_stalled_repair",
        }
    }
}

/// The repair request's view of one eligible check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairTarget {
    pub check: CheckId,
    pub label: String,
    pub provenance: FailureProvenance,
    pub termination: CheckTermination,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<FailureFingerprint>,
    /// Introduced evidence only. This is what a repair turn receives.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<FindingEvidence>,
    pub reason: String,
}

/// Build the typed partitions in their stable order. Empty groups are omitted:
/// a report says "none" by having no entry, not by naming a class with no
/// checks.
pub fn build_partitions(
    entries: Vec<(FailureProvenance, ProvenanceCheck)>,
) -> Vec<ProvenancePartition> {
    const ORDER: [FailureProvenance; 6] = [
        FailureProvenance::NewRegression,
        FailureProvenance::Worsened,
        FailureProvenance::PreExisting,
        FailureProvenance::UnchangedFailure,
        FailureProvenance::EnvironmentFailure,
        FailureProvenance::UnknownProvenance,
    ];
    let mut partitions = Vec::new();
    for provenance in ORDER {
        let mut checks = entries
            .iter()
            .filter(|(entry, _)| *entry == provenance)
            .map(|(_, check)| check.clone())
            .collect::<Vec<_>>();
        checks.sort_by(|a, b| a.check.cmp(&b.check));
        if checks.is_empty() {
            continue;
        }
        let affected = checks
            .iter()
            .map(|check| check.check.clone())
            .collect::<Vec<_>>();
        partitions.push(ProvenancePartition {
            provenance,
            label: provenance_label(provenance).to_string(),
            summary: provenance_summary(provenance).to_string(),
            repair_eligible: matches!(
                provenance,
                FailureProvenance::NewRegression | FailureProvenance::Worsened
            ),
            action: provenance_action(provenance, &affected),
            checks,
        });
    }
    partitions
}

/// The repair targets implied by the classifications. Only introduced evidence
/// is copied into the target.
pub fn repair_targets(entries: &[(FailureProvenance, ProvenanceCheck)]) -> Vec<RepairTarget> {
    let mut targets = entries
        .iter()
        .filter(|(provenance, check)| match provenance {
            FailureProvenance::NewRegression => true,
            // A worsened record with no introduced delta would carry no new
            // evidence; refusing it is safer than starting an empty repair.
            FailureProvenance::Worsened => !check.introduced.is_empty(),
            _ => false,
        })
        .map(|(provenance, check)| RepairTarget {
            check: check.check.clone(),
            label: check.label.clone(),
            provenance: *provenance,
            termination: check.termination.clone(),
            fingerprint: check.fingerprint.clone(),
            evidence: match provenance {
                FailureProvenance::Worsened => check.introduced.clone(),
                _ => check.evidence.clone(),
            },
            reason: check.reason.clone(),
        })
        .collect::<Vec<_>>();
    targets.sort_by(|a, b| a.check.cmp(&b.check));
    targets
}

/// Render the normalized evidence a repair turn may receive. Raw command output
/// is never part of a target, so it cannot reach the model through here.
pub fn render_repair_request(targets: &[RepairTarget]) -> Option<String> {
    if targets.is_empty() {
        return None;
    }
    let mut text = String::from(
        "Repair only the failures this patch introduced. Do not try to fix failures that \
         predate the patch.\n\n",
    );
    for target in targets {
        text.push_str(&format!("- {} — {}\n", target.label, target.reason));
        for evidence in &target.evidence {
            text.push_str(&format!("  - {}\n", evidence_line(evidence)));
        }
    }
    Some(text.trim_end().to_string())
}

/// One finding as the repair request states it, without output or ids that
/// change on reload.
fn evidence_line(evidence: &FindingEvidence) -> String {
    let severity = match evidence.severity() {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    };
    let code = evidence
        .code()
        .map(|code| format!("[{code}] "))
        .unwrap_or_default();
    let location = match (evidence.path(), evidence.start_line()) {
        (Some(path), Some(line)) => match evidence.start_column() {
            Some(column) => format!(" at {path}:{line}:{column}"),
            None => format!(" at {path}:{line}"),
        },
        (Some(path), None) => format!(" at {path}"),
        _ => String::new(),
    };
    format!("{severity}: {code}{}{location}", evidence.summary())
}

/// The engine's reason phrase for one classified result.
pub fn provenance_reason(termination: &CheckTermination, finding_count: usize) -> String {
    let base = termination.description();
    match finding_count {
        0 => base,
        1 => format!("{base}; 1 finding"),
        count => format!("{base}; {count} findings"),
    }
}
