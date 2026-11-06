//! Review fix R10 (spec 102): a check with no parser is one generic finding, so
//! its identity must include more than the command, the first output line and
//! the exit code. Otherwise a newly failing test over a failed baseline looks
//! pre-existing, and a repair that removes one failing test at a time looks
//! like the same repeated failure.
//!
//! These tests use the real generic fallback (`findings_from_execution` with
//! the shipped parsers) over output the fixture controls. They do not call a
//! parser or construct a `FindingEvidence` by hand, so the tail must survive
//! redaction, bounding and the generic finding's own details construction.

use workspace_engine::finding::{FindingSource, default_parsers, findings_from_execution};
use workspace_engine::{
    CheckBaseline, CheckId, CheckTermination, CommandExecution, CommandPurpose, CommandRisk,
    FailureFingerprint, FailureProvenance, FindingEvidence, PostPatchResult, ProvenanceCheck,
    RepairStallReason, RepairStateEntry, SecretScanner, classify, detect_repair_stall,
    provenance_reason, render_repair_request, repair_targets,
};

fn exited(code: i32) -> CheckTermination {
    CheckTermination::Exited { exit_code: code }
}

fn execution(output: &str) -> CommandExecution {
    CommandExecution {
        id: "cmd_generic".to_string(),
        command: "pytest".to_string(),
        working_directory: "/repo".to_string(),
        risk: CommandRisk::Low,
        approved_by: None,
        started_at_ms: 1,
        completed_at_ms: 2,
        exit_code: Some(1),
        termination: workspace_engine::CommandTermination::Exited,
        stdout: String::new(),
        stderr: output.to_string(),
        purpose: CommandPurpose::Other,
    }
}

/// The one generic finding for a failed unparsed check. `pytest` matches no
/// shipped parser, so this exercises the fallback that has no line/column and
/// no code.
fn generic_evidence(output: &str) -> FindingEvidence {
    let findings = findings_from_execution(
        &execution(output),
        &default_parsers(),
        &SecretScanner::default(),
    );
    assert_eq!(
        findings.len(),
        1,
        "one generic finding per unparsed failed check: {findings:?}"
    );
    assert_eq!(findings[0].source(), FindingSource::Command);
    FindingEvidence::from_finding(&findings[0])
}

fn check() -> CheckId {
    CheckId::new("", "pytest")
}

fn failed_baseline(findings: &[FindingEvidence]) -> CheckBaseline {
    CheckBaseline::Failed {
        termination: exited(1),
        fingerprint: FailureFingerprint::compute(&check(), findings, &exited(1)),
        findings: findings.to_vec(),
    }
}

fn entry(classification: &workspace_engine::ProvenanceClassification) -> ProvenanceCheck {
    ProvenanceCheck::new(
        classification.check.clone(),
        provenance_reason(&classification.termination, classification.findings.len()),
        classification.termination.clone(),
        Some(classification.fingerprint.clone()),
        classification.findings.clone(),
        classification.introduced.clone(),
    )
}

/// (a) The failed baseline sees test A. After the patch tests A and B fail.
/// B is a new failing test and must be introduced, not folded into the
/// pre-existing failure because both results have the same first output line.
#[test]
fn a_new_failing_test_over_a_failed_generic_baseline_is_introduced() {
    let before = generic_evidence("FAILED test_a\nassert 1 == 2\n");
    let baseline = failed_baseline(std::slice::from_ref(&before));
    let after = generic_evidence("FAILED test_a\nassert 1 == 2\nFAILED test_b\nassert 3 == 4\n");
    let result = PostPatchResult::new(check(), exited(1), vec![after]);

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(
        classification.provenance,
        FailureProvenance::Worsened,
        "adding test B to a failed baseline is a new failure, not pre_existing"
    );
    assert_eq!(
        classification.introduced.len(),
        1,
        "the whole generic result is the introduced evidence: {:?}",
        classification.introduced
    );
    let details = classification.introduced[0]
        .details()
        .expect("the generic finding carries its redacted output tail");
    assert!(
        details.contains("test_b"),
        "the repair target names the newly failing test: {details}"
    );
    let targets = repair_targets(&[(classification.provenance, entry(&classification))]);
    assert_eq!(targets.len(), 1);
}

/// (b) A passing baseline, then repairs remove one failing test at a time:
/// A+B+C, then A+B, then A. Each result is progress, so the stagnation window
/// must not call the second attempt `repeated_failure`.
#[test]
fn progressing_generic_repairs_are_not_a_repeated_failure_stall() {
    let entry = |output: &str, patch: &str, diff: &str| RepairStateEntry {
        failure_fingerprint: FailureFingerprint::compute(
            &check(),
            &[generic_evidence(output)],
            &exited(1),
        ),
        patch_digest: patch.to_string(),
        task_diff_digest: diff.to_string(),
    };
    let first = entry(
        "FAILED test_a\nassert a\nFAILED test_b\nassert b\nFAILED test_c\nassert c\n",
        "patch-1",
        "diff-1",
    );
    let second = entry(
        "FAILED test_a\nassert a\nFAILED test_b\nassert b\n",
        "patch-2",
        "diff-2",
    );
    let third = entry("FAILED test_a\nassert a\n", "patch-3", "diff-3");

    assert_eq!(
        detect_repair_stall(std::slice::from_ref(&first), &second),
        None,
        "removing test C is progress, not the same failure"
    );
    assert_eq!(
        detect_repair_stall(&[first, second], &third),
        None,
        "removing test B is still progress, not a repeated failure"
    );
}

/// (c) Guard: identical generic output twice is still a repeated failure.
/// The added tail must not defeat the existing stagnation stop.
#[test]
fn identical_generic_output_twice_is_still_a_repeated_failure() {
    let output = "FAILED test_a\nassert a\nFAILED test_b\nassert b\n";
    let first = RepairStateEntry {
        failure_fingerprint: FailureFingerprint::compute(
            &check(),
            &[generic_evidence(output)],
            &exited(1),
        ),
        patch_digest: "patch-1".to_string(),
        task_diff_digest: "diff-1".to_string(),
    };
    let repeated = RepairStateEntry {
        failure_fingerprint: FailureFingerprint::compute(
            &check(),
            &[generic_evidence(output)],
            &exited(1),
        ),
        // Different digests so the stop is `repeated_failure`, not
        // `identical_state`.
        patch_digest: "patch-2".to_string(),
        task_diff_digest: "diff-2".to_string(),
    };

    assert_eq!(
        detect_repair_stall(&[first], &repeated),
        Some(RepairStallReason::RepeatedFailure)
    );
}

/// (d) Guard: values that change on every run but say nothing about the code
/// are not identity: timestamps, durations, temporary paths, PIDs, ANSI colour
/// codes and the absolute checkout path.
#[test]
fn volatile_generic_output_values_are_not_identity() {
    let first = generic_evidence(
        "FAILED test_a at 2026-11-06T10:00:00Z (0.12s) pid=1234 \
         /tmp/pytest-abc/test_a.py:12: AssertionError at \
         /Users/alice/repo/src/a.py:12:5 \x1b[31mFAILED\x1b[0m\n",
    );
    let second = generic_evidence(
        "FAILED test_a at 2026-11-06T10:05:59Z (0.98s) pid=9876 \
         /private/var/folders/xy/T/pytest-def/test_a.py:99: AssertionError at \
         /Users/bob/checkout/src/a.py:42:9 \x1b[32mFAILED\x1b[0m\n",
    );

    assert_eq!(
        FailureFingerprint::compute(&check(), std::slice::from_ref(&first), &exited(1)),
        FailureFingerprint::compute(&check(), std::slice::from_ref(&second), &exited(1)),
        "volatile run details must not change a generic failure's identity"
    );
}

/// (e) Requirement 3: a secret in the raw output is redacted before it can be
/// normalized into identity, stored as durable evidence, or rendered into the
/// repair request.
#[test]
fn a_secret_in_generic_output_never_reaches_its_identity_report_or_repair_request() {
    const SECRET: &str = "AKIAIOSFODNN7EXAMPLE";
    let output = format!("FAILED test_a\nleaked credential {SECRET}\nassert a\n");
    let evidence = generic_evidence(&output);

    let identity_tail = evidence
        .identity_tail()
        .expect("a generic finding has an identity tail");
    assert!(
        !identity_tail.contains(SECRET),
        "the fingerprint input must not contain the secret: {identity_tail}"
    );
    assert!(
        !format!("{evidence:?}").contains(SECRET),
        "durable evidence must not contain the secret"
    );
    assert!(
        !serde_json::to_string(&evidence).unwrap().contains(SECRET),
        "serialized evidence must not contain the secret"
    );

    let baseline = CheckBaseline::Passed;
    let result = PostPatchResult::new(check(), exited(1), vec![evidence]);
    let classification = classify(&result, Some(&baseline), None);
    assert_eq!(classification.provenance, FailureProvenance::NewRegression);
    assert!(
        !classification.fingerprint.as_str().contains(SECRET),
        "a fingerprint is a hash and never carries raw text"
    );

    let targets = repair_targets(&[(classification.provenance, entry(&classification))]);
    let request = render_repair_request(&targets).expect("a repair request");
    assert!(
        !request.contains(SECRET),
        "the repair request must not carry the secret: {request}"
    );
}
