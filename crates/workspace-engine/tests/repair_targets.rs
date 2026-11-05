//! Spec 102 Task 3: provenance classification and repair targets.
//!
//! These tests pin the engine rules that a shell, CLI or worker must not
//! reproduce: what each live result means against its baseline, which classes
//! may start repair, and how much evidence a repair request receives.

use std::collections::BTreeMap;

use serde_json::json;
use workspace_engine::finding::{Finding, FindingDraft, FindingSource, Severity, SourceRange};
use workspace_engine::plan::TaskPlan;
use workspace_engine::{
    CheckBaseline, CheckId, CheckOutcome, CheckProvenanceRecord, CheckTermination,
    FailureFingerprint, FailureProvenance, FindingEvidence, MAX_REPAIR_FINDING_DETAILS_BYTES,
    MAX_REPAIR_TOTAL_DETAILS_BYTES, PostPatchResult, ProvenanceCheck, ProvenanceClassification,
    REPAIR_FINDING_DETAILS_TRUNCATED, REPAIR_TOTAL_DETAILS_TRUNCATED, SecretScanner,
    VerificationEvent, VerificationReport, classify, fold_verification, provenance_reason,
    repair_targets,
};

fn check(directory: &str, command: &str) -> CheckId {
    CheckId::new(directory, command)
}

fn exited(code: i32) -> CheckTermination {
    CheckTermination::Exited { exit_code: code }
}

fn evidence_with(
    severity: Severity,
    code: &str,
    summary: &str,
    path: &str,
    line: u32,
) -> FindingEvidence {
    let finding = Finding::new(
        FindingDraft {
            source: FindingSource::Compiler,
            severity,
            summary: summary.to_string(),
            details: None,
            range: Some(SourceRange {
                path: path.to_string(),
                start_line: line,
                start_column: Some(1),
                end_line: None,
                end_column: None,
            }),
            code: Some(code.to_string()),
        },
        &SecretScanner::new(Vec::new()),
    );
    FindingEvidence::from_finding(&finding)
}

fn evidence(summary: &str, path: &str, line: u32) -> FindingEvidence {
    evidence_with(Severity::Error, "E0308", summary, path, line)
}

fn evidence_with_details(summary: &str, path: &str, details: &str) -> FindingEvidence {
    let finding = Finding::new(
        FindingDraft {
            source: FindingSource::Compiler,
            severity: Severity::Error,
            summary: summary.to_string(),
            details: Some(details.to_string()),
            range: Some(SourceRange {
                path: path.to_string(),
                start_line: 1,
                start_column: Some(1),
                end_line: None,
                end_column: None,
            }),
            code: Some("E0308".to_string()),
        },
        &SecretScanner::new(Vec::new()),
    );
    FindingEvidence::from_finding(&finding)
}

fn failed_baseline(
    check: &CheckId,
    termination: CheckTermination,
    findings: &[FindingEvidence],
) -> CheckBaseline {
    CheckBaseline::Failed {
        fingerprint: FailureFingerprint::compute(check, findings, &termination),
        termination,
        findings: findings.to_vec(),
    }
}

fn entry(classification: &ProvenanceClassification) -> (FailureProvenance, ProvenanceCheck) {
    (
        classification.provenance,
        ProvenanceCheck::new(
            classification.check.clone(),
            provenance_reason(&classification.termination, classification.findings.len()),
            classification.termination.clone(),
            Some(classification.fingerprint.clone()),
            classification.findings.clone(),
            classification.introduced.clone(),
        ),
    )
}

fn record(classification: &ProvenanceClassification) -> CheckProvenanceRecord {
    classification.record()
}

#[test]
fn a_passing_baseline_makes_the_first_failure_a_new_regression() {
    let check = check("", "cargo test");
    let found = evidence("mismatched types", "src/a.rs", 4);
    let result = PostPatchResult::new(check.clone(), exited(1), vec![found.clone()]);
    let classification = classify(&result, Some(&CheckBaseline::Passed), None);

    assert_eq!(classification.provenance, FailureProvenance::NewRegression);
    assert!(classification.repair_eligible());
    assert_eq!(classification.introduced, vec![found.clone()]);

    let targets = repair_targets(&[entry(&classification)]);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].check, check);
    assert_eq!(targets[0].evidence, vec![found]);
}

#[test]
fn a_matching_failed_baseline_is_pre_existing_and_has_no_repair_target() {
    let check = check("", "cargo test");
    let found = evidence("mismatched types", "src/a.rs", 4);
    let baseline = failed_baseline(&check, exited(1), std::slice::from_ref(&found));
    let result = PostPatchResult::new(check.clone(), exited(1), vec![found.clone()]);

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(classification.provenance, FailureProvenance::PreExisting);
    assert!(!classification.repair_eligible());
    assert!(classification.introduced.is_empty());
    assert!(repair_targets(&[entry(&classification)]).is_empty());
}

/// R5 (spec 102): the patch added lines above an existing error, so its line
/// moved. Same source, severity, code, path and summary is the same failure.
#[test]
fn a_pre_existing_error_that_shifts_lines_is_still_pre_existing() {
    let check = check("", "cargo test");
    let before = evidence("mismatched types", "src/a.rs", 4);
    let after = evidence("mismatched types", "src/a.rs", 7);
    let baseline = failed_baseline(&check, exited(1), std::slice::from_ref(&before));
    let result = PostPatchResult::new(check, exited(1), vec![after]);

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(classification.provenance, FailureProvenance::PreExisting);
    assert!(!classification.repair_eligible());
    assert!(classification.introduced.is_empty());
    assert!(repair_targets(&[entry(&classification)]).is_empty());
}

/// R5 guard: the comparison key still includes the normalized path. A new
/// failure with the same code and summary in another file was introduced.
#[test]
fn a_new_identical_error_in_a_different_file_is_introduced() {
    let check = check("", "cargo test");
    let old = evidence("mismatched types", "src/a.rs", 4);
    let new = evidence("mismatched types", "src/b.rs", 4);
    let baseline = failed_baseline(&check, exited(1), std::slice::from_ref(&old));
    let result = PostPatchResult::new(check, exited(1), vec![old, new.clone()]);

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(classification.provenance, FailureProvenance::Worsened);
    assert_eq!(classification.introduced, vec![new.clone()]);
    let targets = repair_targets(&[entry(&classification)]);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].evidence, vec![new]);
}

/// R5 guard: a second, genuinely different error in the same file is still
/// introduced. Both a different summary under the same code and a different
/// code must count.
#[test]
fn a_second_different_error_in_the_same_file_is_introduced() {
    let check = check("", "cargo test");
    let old = evidence("mismatched types", "src/a.rs", 4);
    let different_summary = evidence("cannot find value `x`", "src/a.rs", 9);
    let different_code =
        evidence_with(Severity::Error, "E0425", "mismatched types", "src/a.rs", 12);
    let baseline = failed_baseline(&check, exited(1), std::slice::from_ref(&old));
    let result = PostPatchResult::new(
        check,
        exited(1),
        vec![old, different_summary.clone(), different_code.clone()],
    );

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(classification.provenance, FailureProvenance::Worsened);
    assert_eq!(
        classification.introduced,
        vec![different_summary, different_code],
        "a different summary or code is new evidence"
    );
}

/// R5 decision (spec 102): a check that already failed is not made worse by a
/// new warning. Warnings remain in the full report but never become a repair
/// target, so the result is an unchanged failure rather than `worsened`.
#[test]
fn a_new_warning_on_a_failed_baseline_is_not_worsened_or_a_repair_target() {
    let check = check("", "cargo test");
    let old = evidence("mismatched types", "src/a.rs", 4);
    let warning = evidence_with(
        Severity::Warning,
        "unused_variables",
        "unused variable `x`",
        "src/a.rs",
        9,
    );
    let baseline = failed_baseline(&check, exited(1), std::slice::from_ref(&old));
    let result = PostPatchResult::new(check, exited(1), vec![old, warning.clone()]);

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(
        classification.provenance,
        FailureProvenance::UnchangedFailure
    );
    assert!(!classification.repair_eligible());
    assert!(classification.introduced.is_empty());
    assert!(
        classification.findings.contains(&warning),
        "the report still shows the warning"
    );
    assert!(repair_targets(&[entry(&classification)]).is_empty());
}

/// R5 duplicate accounting: one pre-existing occurrence plus one newly
/// introduced identical occurrence (same path/code/summary at another line) is
/// exactly one introduced finding, not zero and not two.
#[test]
fn an_extra_identical_occurrence_beyond_the_baseline_is_introduced() {
    let check = check("", "cargo test");
    let old = evidence("mismatched types", "src/a.rs", 4);
    let duplicate = evidence("mismatched types", "src/a.rs", 40);
    let baseline = failed_baseline(&check, exited(1), std::slice::from_ref(&old));
    let result = PostPatchResult::new(check, exited(1), vec![old.clone(), duplicate.clone()]);

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(classification.provenance, FailureProvenance::Worsened);
    assert_eq!(
        classification.introduced,
        vec![duplicate],
        "the occurrence that already existed is not a repair target"
    );
    assert_eq!(
        repair_targets(&[entry(&classification)])[0].evidence.len(),
        1
    );
}

/// R5 duplicate accounting: if every occurrence moved but the count did not
/// grow, nothing was introduced.
#[test]
fn shifted_duplicates_that_do_not_grow_the_count_are_not_introduced() {
    let check = check("", "cargo test");
    let baseline_findings = vec![
        evidence("mismatched types", "src/a.rs", 4),
        evidence("mismatched types", "src/a.rs", 40),
    ];
    let baseline = failed_baseline(&check, exited(1), &baseline_findings);
    let result = PostPatchResult::new(
        check,
        exited(1),
        vec![
            evidence("mismatched types", "src/a.rs", 7),
            evidence("mismatched types", "src/a.rs", 43),
        ],
    );

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(classification.provenance, FailureProvenance::PreExisting);
    assert!(classification.introduced.is_empty());
}

/// R5 compatibility: a baseline written with the previous fingerprint format
/// still classifies from its durable findings. The stored hash cannot be
/// recomputed, but the findings can be compared.
#[test]
fn a_baseline_written_with_a_v1_fingerprint_still_classifies_by_its_findings() {
    let check = check("", "cargo test");
    let before = evidence("mismatched types", "src/a.rs", 4);
    let baseline = CheckBaseline::Failed {
        termination: exited(1),
        fingerprint: serde_json::from_value(json!("sha256:legacy-v1")).unwrap(),
        findings: vec![before],
    };
    let result = PostPatchResult::new(
        check,
        exited(1),
        vec![evidence("mismatched types", "src/a.rs", 7)],
    );

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(classification.provenance, FailureProvenance::PreExisting);
    assert!(classification.introduced.is_empty());
}

#[test]
fn a_repeated_matching_result_is_unchanged_failure() {
    let check = check("", "cargo test");
    let found = evidence("mismatched types", "src/a.rs", 4);
    let baseline = failed_baseline(&check, exited(1), std::slice::from_ref(&found));
    let result = PostPatchResult::new(check, exited(1), vec![found]);
    let first = classify(&result, Some(&baseline), None);
    let previous = record(&first);

    let second = classify(&result, Some(&baseline), Some(&previous));

    assert_eq!(first.provenance, FailureProvenance::PreExisting);
    assert_eq!(second.provenance, FailureProvenance::UnchangedFailure);
    assert!(!second.repair_eligible());
    assert!(second.introduced.is_empty());
    assert!(repair_targets(&[entry(&second)]).is_empty());
}

#[test]
fn a_failed_baseline_with_new_evidence_is_worsened_with_only_the_delta() {
    let check = check("", "cargo test");
    let old = evidence("mismatched types", "src/a.rs", 4);
    let new = evidence("cannot find value `x`", "src/b.rs", 9);
    let baseline = failed_baseline(&check, exited(1), std::slice::from_ref(&old));
    let result = PostPatchResult::new(check.clone(), exited(1), vec![old.clone(), new.clone()]);

    let classification = classify(&result, Some(&baseline), None);
    assert_eq!(classification.provenance, FailureProvenance::Worsened);
    assert!(classification.repair_eligible());
    assert_eq!(classification.findings, vec![old.clone(), new.clone()]);
    assert_eq!(
        classification.introduced,
        vec![new.clone()],
        "only the evidence the baseline did not have is introduced"
    );

    let targets = repair_targets(&[entry(&classification)]);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].evidence, vec![new.clone()]);
    let request = targets[0].clone();
    let rendered = workspace_engine::render_repair_request(&targets).unwrap();
    assert!(rendered.contains(new.summary()));
    assert!(
        !rendered.contains(old.summary()),
        "pre-existing evidence is not a repair target: {rendered}"
    );
    assert_eq!(request.provenance, FailureProvenance::Worsened);
}

#[test]
fn a_failed_baseline_with_no_new_evidence_is_unchanged_failure() {
    let check = check("", "cargo test");
    let a = evidence("mismatched types", "src/a.rs", 4);
    let b = evidence("cannot find value `x`", "src/b.rs", 9);
    let baseline = failed_baseline(&check, exited(1), &[a.clone(), b]);
    let result = PostPatchResult::new(check, exited(1), vec![a]);

    let classification = classify(&result, Some(&baseline), None);

    assert_eq!(
        classification.provenance,
        FailureProvenance::UnchangedFailure
    );
    assert!(classification.introduced.is_empty());
    assert!(repair_targets(&[entry(&classification)]).is_empty());
}

#[test]
fn every_environment_termination_is_never_a_repair_target() {
    let check = check("", "cargo test");
    for termination in [
        CheckTermination::SpawnFailed,
        CheckTermination::MissingExecutable,
        CheckTermination::PolicyRefused,
        CheckTermination::UnavailableInfrastructure,
    ] {
        let result = PostPatchResult::new(check.clone(), termination.clone(), Vec::new());
        let classification = classify(&result, Some(&CheckBaseline::Passed), None);
        assert_eq!(
            classification.provenance,
            FailureProvenance::EnvironmentFailure,
            "{termination:?}"
        );
        assert!(!classification.repair_eligible());
        assert!(repair_targets(&[entry(&classification)]).is_empty());
    }
}

#[test]
fn a_timed_out_or_signalled_result_is_a_code_failure_not_an_environment_one() {
    let check = check("", "cargo test");
    for termination in [CheckTermination::TimedOut, CheckTermination::Signalled] {
        let result = PostPatchResult::new(check.clone(), termination.clone(), Vec::new());
        let classification = classify(&result, Some(&CheckBaseline::Passed), None);
        assert_eq!(
            classification.provenance,
            FailureProvenance::NewRegression,
            "{termination:?}"
        );
    }
}

#[test]
fn a_declined_unavailable_or_absent_baseline_is_unknown_provenance() {
    let check = check("", "cargo test");
    let result = PostPatchResult::new(
        check.clone(),
        exited(1),
        vec![evidence("mismatched types", "src/a.rs", 4)],
    );
    let unavailable = CheckBaseline::Unavailable {
        termination: CheckTermination::SpawnFailed,
        fingerprint: FailureFingerprint::compute(&check, &[], &CheckTermination::SpawnFailed),
    };
    for baseline in [None, Some(CheckBaseline::Declined), Some(unavailable)] {
        let classification = classify(&result, baseline.as_ref(), None);
        assert_eq!(
            classification.provenance,
            FailureProvenance::UnknownProvenance,
            "{baseline:?}"
        );
        assert!(!classification.repair_eligible());
        assert!(classification.introduced.is_empty());
        assert!(repair_targets(&[entry(&classification)]).is_empty());
    }
}

fn state_and_report() -> VerificationReport {
    let new = check("", "cargo test");
    let pre = check("crates/a", "cargo test");
    let env = check("crates/b", "cargo test");
    let unknown = check("crates/c", "cargo test");
    let new_evidence = evidence("mismatched types", "src/a.rs", 4);
    let pre_evidence = evidence("test failed", "src/pre.rs", 2);

    let new_classification = classify(
        &PostPatchResult::new(new.clone(), exited(1), vec![new_evidence.clone()]),
        Some(&CheckBaseline::Passed),
        None,
    );
    let pre_classification = classify(
        &PostPatchResult::new(pre.clone(), exited(1), vec![pre_evidence.clone()]),
        Some(&failed_baseline(
            &pre,
            exited(1),
            std::slice::from_ref(&pre_evidence),
        )),
        None,
    );
    let env_classification = classify(
        &PostPatchResult::new(env.clone(), CheckTermination::MissingExecutable, Vec::new()),
        Some(&CheckBaseline::Passed),
        None,
    );

    let events = vec![
        VerificationEvent::Started {
            task_id: "task".to_string(),
            attempt: 0,
            checks: vec![new.clone(), pre.clone(), env.clone(), unknown.clone()],
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check: new.clone(),
            outcome: CheckOutcome::Ran {
                exit_code: Some(1),
                marker_id: "new".to_string(),
                termination: Some(exited(1)),
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check: pre.clone(),
            outcome: CheckOutcome::Ran {
                exit_code: Some(1),
                marker_id: "pre".to_string(),
                termination: Some(exited(1)),
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check: env.clone(),
            outcome: CheckOutcome::Environment {
                termination: CheckTermination::MissingExecutable,
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check: unknown.clone(),
            outcome: CheckOutcome::Ran {
                exit_code: Some(1),
                marker_id: "unknown".to_string(),
                termination: Some(exited(1)),
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::Provenance {
            task_id: "task".to_string(),
            record: record(&new_classification),
        },
        VerificationEvent::Provenance {
            task_id: "task".to_string(),
            record: record(&pre_classification),
        },
        VerificationEvent::Provenance {
            task_id: "task".to_string(),
            record: record(&env_classification),
        },
        VerificationEvent::Finished {
            task_id: "task".to_string(),
            outcome: workspace_engine::VerificationOutcome::Failed,
        },
    ];
    let state = fold_verification(events.iter());
    VerificationReport::build(&state, &TaskPlan::new("task", 0), &[], &BTreeMap::new())
}

#[test]
fn the_report_partitions_every_failure_and_only_repairs_introduced_evidence() {
    let report = state_and_report();
    let groups = report.provenance.iter().collect::<Vec<_>>();

    assert!(
        groups
            .iter()
            .any(|group| group.provenance == FailureProvenance::NewRegression)
    );
    assert!(
        groups
            .iter()
            .any(|group| group.provenance == FailureProvenance::PreExisting)
    );
    assert!(
        groups
            .iter()
            .any(|group| group.provenance == FailureProvenance::EnvironmentFailure)
    );
    assert!(
        groups
            .iter()
            .any(|group| group.provenance == FailureProvenance::UnknownProvenance),
        "a historical failure without a baseline is unknown, not invented"
    );
    assert!(
        !groups
            .iter()
            .any(|group| group.provenance == FailureProvenance::Worsened)
    );
    assert!(
        !groups
            .iter()
            .any(|group| group.provenance == FailureProvenance::UnchangedFailure)
    );

    assert_eq!(report.repair_targets.len(), 1);
    assert_eq!(report.repair_targets[0].check, check("", "cargo test"));
    assert_eq!(
        report.repair_targets[0].evidence,
        vec![evidence("mismatched types", "src/a.rs", 4)]
    );
    assert_eq!(report.actions.len(), 4);

    let rendered = report.render();
    for expected in [
        "Introduced regressions",
        "Pre-existing failures",
        "Environment failures",
        "Unknown provenance",
        "What you can do",
        "Review the pre-existing failure",
        "Fix the environment",
        "Review the failure and provide a baseline",
    ] {
        assert!(
            rendered.contains(expected),
            "{expected} missing:\n{rendered}"
        );
    }
    let summary = report.summary(&TaskPlan::new("task", 0).report());
    assert!(
        summary.contains("1 introduced regression")
            && summary.contains("1 pre-existing failure")
            && summary.contains("1 environment failure")
            && summary.contains("1 failure of unknown provenance"),
        "the summary is not one generic failed sentence: {summary}"
    );
    assert!(!summary.contains("complete"), "{summary}");

    let request = report.repair_request().expect("a repair request");
    assert!(request.contains("mismatched types"));
    assert!(
        !request.contains("test failed"),
        "pre-existing evidence must not reach repair: {request}"
    );
}

#[test]
fn an_environment_only_report_has_no_repair_target() {
    let check = check("", "cargo test");
    let events = [
        VerificationEvent::Started {
            task_id: "task".to_string(),
            attempt: 0,
            checks: vec![check.clone()],
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check,
            outcome: CheckOutcome::Environment {
                termination: CheckTermination::SpawnFailed,
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::Finished {
            task_id: "task".to_string(),
            outcome: workspace_engine::VerificationOutcome::Failed,
        },
    ];
    let report = VerificationReport::build(
        &fold_verification(events.iter()),
        &TaskPlan::new("task", 0),
        &[],
        &BTreeMap::new(),
    );

    assert_eq!(report.failed.len(), 1);
    assert!(report.repair_targets.is_empty());
    assert!(report.repair_request().is_none());
    assert_eq!(report.provenance.len(), 1);
    assert_eq!(
        report.provenance[0].provenance,
        FailureProvenance::EnvironmentFailure
    );
    assert_eq!(
        report.provenance[0].action.as_ref().unwrap().label,
        "Fix the environment"
    );
}

#[test]
fn a_historical_failure_without_recorded_termination_is_unknown_not_environment() {
    let check = check("", "cargo test");
    let events = [
        VerificationEvent::Started {
            task_id: "task".to_string(),
            attempt: 0,
            checks: vec![check.clone()],
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check,
            outcome: CheckOutcome::Ran {
                exit_code: Some(127),
                marker_id: "marker".to_string(),
                // An event written before Task 3 has no termination; exit 127
                // is not proof enough to invent an environment classification.
                termination: None,
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::Finished {
            task_id: "task".to_string(),
            outcome: workspace_engine::VerificationOutcome::Failed,
        },
    ];
    let report = VerificationReport::build(
        &fold_verification(events.iter()),
        &TaskPlan::new("task", 0),
        &[],
        &BTreeMap::new(),
    );

    assert_eq!(report.provenance.len(), 1);
    assert_eq!(
        report.provenance[0].provenance,
        FailureProvenance::UnknownProvenance
    );
    assert!(report.repair_targets.is_empty());
    assert!(report.repair_request().is_none());
}

/// R6 (spec 102): one verbose finding cannot spend the whole repair request.
/// Its `details()` is cut to the per-finding budget and the request says so.
#[test]
fn a_repair_request_bounds_one_findings_details() {
    let check = check("", "cargo test");
    let long = "x".repeat(workspace_engine::finding::MAX_DETAILS_BYTES);
    let found = evidence_with_details("verbose compiler error", "src/a.rs", &long);
    let result = PostPatchResult::new(check, exited(1), vec![found]);
    let classification = classify(&result, Some(&CheckBaseline::Passed), None);
    let targets = repair_targets(&[entry(&classification)]);
    let request = workspace_engine::render_repair_request(&targets).unwrap();

    let detail_bytes: usize = request
        .lines()
        .filter_map(|line| line.strip_prefix("      "))
        .map(str::len)
        .sum();
    assert!(
        detail_bytes <= MAX_REPAIR_FINDING_DETAILS_BYTES,
        "one finding's details exceeded the per-finding budget: {detail_bytes} bytes"
    );
    assert!(
        request.contains(REPAIR_FINDING_DETAILS_TRUNCATED),
        "the request must say the finding's details were cut: {request}"
    );
    assert!(
        !request.contains(REPAIR_TOTAL_DETAILS_TRUNCATED),
        "one finding did not exhaust the request budget: {request}"
    );
}

/// R6 (spec 102): a check with many introduced findings shares one bounded
/// details budget. The request marks where later details were omitted, and
/// still carries every finding's summary line.
#[test]
fn a_repair_request_bounds_total_details_across_many_findings() {
    let check = check("", "cargo test");
    let long = "y".repeat(workspace_engine::finding::MAX_DETAILS_BYTES);
    let findings = (0..10)
        .map(|index| {
            evidence_with_details(
                &format!("failure {index}"),
                &format!("src/f{index}.rs"),
                &long,
            )
        })
        .collect::<Vec<_>>();
    let result = PostPatchResult::new(check, exited(1), findings.clone());
    let classification = classify(&result, Some(&CheckBaseline::Passed), None);
    let targets = repair_targets(&[entry(&classification)]);
    let request = workspace_engine::render_repair_request(&targets).unwrap();

    let detail_lines = request
        .lines()
        .filter_map(|line| line.strip_prefix("      "))
        .collect::<Vec<_>>();
    let detail_bytes: usize = detail_lines.iter().map(|line| line.len()).sum();
    assert!(
        detail_bytes <= MAX_REPAIR_TOTAL_DETAILS_BYTES,
        "the request's details exceeded the total budget: {detail_bytes} bytes"
    );
    assert!(
        detail_lines
            .iter()
            .all(|line| line.len() <= MAX_REPAIR_FINDING_DETAILS_BYTES),
        "no finding may exceed the per-finding budget"
    );
    assert!(
        request.contains(REPAIR_TOTAL_DETAILS_TRUNCATED),
        "the request must say the total details budget was reached: {request}"
    );
    for index in 0..10 {
        assert!(
            request.contains(&format!("failure {index}")),
            "finding {index}'s summary line is never dropped: {request}"
        );
    }
}
