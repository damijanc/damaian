//! Spec 102 Task 3: provenance classification and repair targets.
//!
//! These tests pin the engine rules that a shell, CLI or worker must not
//! reproduce: what each live result means against its baseline, which classes
//! may start repair, and how much evidence a repair request receives.

use std::collections::BTreeMap;

use workspace_engine::finding::{Finding, FindingDraft, FindingSource, Severity, SourceRange};
use workspace_engine::plan::TaskPlan;
use workspace_engine::{
    CheckBaseline, CheckId, CheckOutcome, CheckProvenanceRecord, CheckTermination,
    FailureFingerprint, FailureProvenance, FindingEvidence, PostPatchResult, ProvenanceCheck,
    ProvenanceClassification, SecretScanner, VerificationEvent, VerificationReport, classify,
    fold_verification, provenance_reason, repair_targets,
};

fn check(directory: &str, command: &str) -> CheckId {
    CheckId::new(directory, command)
}

fn exited(code: i32) -> CheckTermination {
    CheckTermination::Exited { exit_code: code }
}

fn evidence(summary: &str, path: &str, line: u32) -> FindingEvidence {
    let finding = Finding::new(
        FindingDraft {
            source: FindingSource::Compiler,
            severity: Severity::Error,
            summary: summary.to_string(),
            details: None,
            range: Some(SourceRange {
                path: path.to_string(),
                start_line: line,
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
