//! Spec 114 Task 1: the engine's verdict, whose failure a check is, and the
//! baselines a report compared against.
//!
//! Each fixture is a folded event log, so the verdict is pinned exactly as a
//! live turn and a reload compute it. The model-facing `summary()` is pinned
//! byte for byte: the verdict is for the user and must not change what a model
//! reads (spec 114 D1).

use std::collections::BTreeMap;

use workspace_engine::finding::{Finding, FindingDraft, FindingSource, Severity, SourceRange};
use workspace_engine::plan::TaskPlan;
use workspace_engine::{
    CheckBaseline, CheckId, CheckOutcome, CheckProvenanceRecord, CheckTermination,
    FailureFingerprint, FailureProvenance, FindingEvidence, SecretScanner, SkipReason, VerdictKind,
    VerificationEvent, VerificationOutcome, VerificationReport, fold_verification,
};

/// What one selected check did in a fixture.
enum Fixture {
    Passed,
    /// Ran, failed, and was classified; with the baseline it was compared
    /// against and the tests that baseline reran by name.
    Classified(FailureProvenance, Option<CheckBaseline>, Vec<String>),
    /// Could not run, with no classification recorded.
    Environment,
    /// Selected, but the log holds no result.
    NoResult,
    Skipped,
}

fn check(directory: &str) -> CheckId {
    CheckId::new(directory, "cargo test")
}

fn exited(code: i32) -> CheckTermination {
    CheckTermination::Exited { exit_code: code }
}

fn evidence(summary: &str) -> FindingEvidence {
    let finding = Finding::new(
        FindingDraft {
            source: FindingSource::Test,
            severity: Severity::Error,
            summary: summary.to_string(),
            details: None,
            range: Some(SourceRange {
                path: "src/lib.rs".to_string(),
                start_line: 1,
                start_column: None,
                end_line: None,
                end_column: None,
            }),
            code: None,
        },
        &SecretScanner::new(Vec::new()),
    );
    FindingEvidence::from_finding(&finding)
}

fn failed_baseline(check: &CheckId, findings: &[FindingEvidence]) -> CheckBaseline {
    CheckBaseline::Failed {
        fingerprint: FailureFingerprint::compute(check, findings, &exited(1)),
        termination: exited(1),
        findings: findings.to_vec(),
    }
}

fn report(
    fixtures: Vec<(CheckId, Fixture)>,
    outcome: Option<VerificationOutcome>,
) -> VerificationReport {
    let mut events = vec![VerificationEvent::Started {
        task_id: "task".to_string(),
        attempt: 0,
        checks: fixtures.iter().map(|(check, _)| check.clone()).collect(),
    }];
    for (check, fixture) in fixtures {
        let ran = |code: i32| VerificationEvent::Check {
            task_id: "task".to_string(),
            check: check.clone(),
            outcome: CheckOutcome::Ran {
                exit_code: Some(code),
                marker_id: "marker".to_string(),
                termination: Some(exited(code)),
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        };
        match fixture {
            Fixture::Passed => events.push(ran(0)),
            Fixture::Classified(provenance, baseline, tests) => {
                events.push(ran(1));
                if let Some(baseline) = baseline {
                    events.push(VerificationEvent::Baseline {
                        task_id: "task".to_string(),
                        check: check.clone(),
                        baseline,
                        tests,
                    });
                }
                let findings = vec![evidence("tests::a failed")];
                let introduced = if matches!(
                    provenance,
                    FailureProvenance::NewRegression | FailureProvenance::Worsened
                ) {
                    findings.clone()
                } else {
                    Vec::new()
                };
                events.push(VerificationEvent::Provenance {
                    task_id: "task".to_string(),
                    record: CheckProvenanceRecord {
                        fingerprint: FailureFingerprint::compute(&check, &findings, &exited(1)),
                        check: check.clone(),
                        provenance,
                        termination: exited(1),
                        findings,
                        introduced,
                    },
                });
            }
            Fixture::Environment => events.push(VerificationEvent::Check {
                task_id: "task".to_string(),
                check: check.clone(),
                outcome: CheckOutcome::Environment {
                    termination: CheckTermination::MissingExecutable,
                },
                file_hashes: BTreeMap::new(),
                generation: 0,
            }),
            Fixture::NoResult => {}
            Fixture::Skipped => events.push(VerificationEvent::Check {
                task_id: "task".to_string(),
                check: check.clone(),
                outcome: CheckOutcome::Skipped(SkipReason::DeclinedByUser),
                file_hashes: BTreeMap::new(),
                generation: 0,
            }),
        }
    }
    if let Some(outcome) = outcome {
        events.push(VerificationEvent::Finished {
            task_id: "task".to_string(),
            outcome,
        });
    }
    VerificationReport::build(
        &fold_verification(events.iter()),
        &TaskPlan::new("task", 0),
        &[],
        &BTreeMap::new(),
    )
}

fn classified(provenance: FailureProvenance) -> Fixture {
    Fixture::Classified(provenance, None, Vec::new())
}

fn kind(report: &VerificationReport) -> VerdictKind {
    report
        .verdict
        .as_ref()
        .expect("every built report has a verdict")
        .kind
}

fn summary(report: &VerificationReport) -> String {
    report.summary(&TaskPlan::new("task", 0).report())
}

#[test]
fn a_passing_check_reads_checks_passed() {
    let report = report(
        vec![(check(""), Fixture::Passed)],
        Some(VerificationOutcome::Passed),
    );
    let verdict = report.verdict.clone().unwrap();
    assert_eq!(verdict.kind, VerdictKind::Passed);
    assert_eq!(verdict.headline, "Checks passed");
    assert!(!verdict.detail.is_empty());
}

#[test]
fn a_skipped_or_unrecorded_check_is_not_verified() {
    let report = report(
        vec![
            (check(""), Fixture::Passed),
            (check("a"), Fixture::Skipped),
            (check("b"), Fixture::NoResult),
        ],
        None,
    );
    let verdict = report.verdict.clone().unwrap();
    assert_eq!(verdict.kind, VerdictKind::NotVerified);
    assert_eq!(
        verdict.headline,
        "Not verified: 2 checks did not run or finish"
    );
}

#[test]
fn only_pre_existing_or_unchanged_failures_are_not_caused_by_the_task() {
    let report = report(
        vec![
            (check(""), classified(FailureProvenance::PreExisting)),
            (check("a"), classified(FailureProvenance::UnchangedFailure)),
        ],
        Some(VerificationOutcome::OnlyPreExistingFailures),
    );
    let verdict = report.verdict.clone().unwrap();
    assert_eq!(verdict.kind, VerdictKind::NotCausedByTask);
    assert_eq!(
        verdict.headline,
        "Checks fail, but not because of this task's changes"
    );
}

#[test]
fn an_environment_or_unknown_failure_is_cause_unknown() {
    for fixture in [
        Fixture::Environment,
        classified(FailureProvenance::UnknownProvenance),
        classified(FailureProvenance::Uncertain),
    ] {
        let report = report(
            vec![(check(""), fixture)],
            Some(VerificationOutcome::Failed),
        );
        let verdict = report.verdict.clone().unwrap();
        assert_eq!(verdict.kind, VerdictKind::CauseUnknown);
        assert_eq!(
            verdict.headline,
            "Checks fail; Damaian could not tell whether this task caused it"
        );
    }
}

#[test]
fn introduced_findings_are_caused_by_the_task_and_counted_in_checks() {
    let report = report(
        vec![
            (check(""), classified(FailureProvenance::NewRegression)),
            (check("a"), classified(FailureProvenance::Worsened)),
        ],
        Some(VerificationOutcome::Failed),
    );
    let verdict = report.verdict.clone().unwrap();
    assert_eq!(verdict.kind, VerdictKind::CausedByTask);
    assert_eq!(verdict.headline, "This task's changes break 2 checks");
}

#[test]
fn a_build_failure_reads_does_not_build() {
    let report = report(
        vec![(check(""), classified(FailureProvenance::NewRegression))],
        Some(VerificationOutcome::BuildFailed),
    );
    let verdict = report.verdict.clone().unwrap();
    assert_eq!(verdict.kind, VerdictKind::BuildFailed);
    assert_eq!(verdict.headline, "The change does not build");
}

/// D1's order: `build_failed` > `caused_by_task` > `cause_unknown` >
/// `not_caused_by_task` > `not_verified` > `passed`.
#[test]
fn the_verdict_follows_its_precedence() {
    let caused_over_rest = report(
        vec![
            (check(""), classified(FailureProvenance::PreExisting)),
            (check("a"), classified(FailureProvenance::NewRegression)),
            (check("b"), Fixture::Environment),
            (check("c"), Fixture::Skipped),
        ],
        Some(VerificationOutcome::Failed),
    );
    assert_eq!(kind(&caused_over_rest), VerdictKind::CausedByTask);
    assert_eq!(
        caused_over_rest.verdict.unwrap().headline,
        "This task's changes break 1 check"
    );

    let unknown_over_not_caused = report(
        vec![
            (check(""), classified(FailureProvenance::PreExisting)),
            (check("b"), Fixture::Environment),
        ],
        Some(VerificationOutcome::Failed),
    );
    assert_eq!(kind(&unknown_over_not_caused), VerdictKind::CauseUnknown);

    let not_caused_over_not_verified = report(
        vec![
            (check(""), classified(FailureProvenance::UnchangedFailure)),
            (check("c"), Fixture::Skipped),
        ],
        Some(VerificationOutcome::Failed),
    );
    assert_eq!(
        kind(&not_caused_over_not_verified),
        VerdictKind::NotCausedByTask
    );

    let not_verified_over_passed = report(
        vec![(check(""), Fixture::Passed), (check("c"), Fixture::Skipped)],
        Some(VerificationOutcome::Passed),
    );
    assert_eq!(kind(&not_verified_over_passed), VerdictKind::NotVerified);
}

#[test]
fn the_text_copy_starts_with_the_headline_and_the_summary_is_unchanged() {
    let report = report(
        vec![
            (check(""), classified(FailureProvenance::NewRegression)),
            (check("a"), classified(FailureProvenance::PreExisting)),
            (check("b"), Fixture::Environment),
        ],
        Some(VerificationOutcome::Failed),
    );
    let verdict = report.verdict.clone().unwrap();
    let rendered = report.render();
    assert!(
        rendered.starts_with(&format!("{}\n{}\n\n", verdict.headline, verdict.detail)),
        "{rendered}"
    );
    // D7's group labels in the text copy.
    assert!(rendered.contains("Caused by this task"), "{rendered}");
    assert!(
        rendered.contains("Failed before this task too"),
        "{rendered}"
    );
    // D1: the model-facing summary keeps its exact wording.
    assert_eq!(
        summary(&report),
        "Verification failed: 1 introduced regression, 1 pre-existing failure, 1 environment failure. 0 of 0 plan steps finished."
    );

    let only_pre_existing = self::report(
        vec![
            (check(""), classified(FailureProvenance::PreExisting)),
            (check("a"), classified(FailureProvenance::UnchangedFailure)),
        ],
        Some(VerificationOutcome::OnlyPreExistingFailures),
    );
    assert_eq!(
        summary(&only_pre_existing),
        "Only failures that were already there before this task remain: 1 pre-existing failure, 1 unchanged failure. The task did not cause or change them. 0 of 0 plan steps finished."
    );

    let limit = self::report(
        vec![(check(""), classified(FailureProvenance::Worsened))],
        Some(VerificationOutcome::RepairLimitExhausted),
    );
    assert_eq!(
        summary(&limit),
        "Verification failed: the repair limit was exhausted and 1 worsened failure. 0 of 0 plan steps finished."
    );
    assert!(
        limit.verdict.unwrap().detail.contains("Continue repair"),
        "the limit's verdict points at spec 96's action"
    );
}

#[test]
fn each_failed_check_says_what_its_baseline_found() {
    let narrowed = check("");
    let whole = check("a");
    let missing = check("b");
    let pre_findings = vec![evidence("tests::a failed")];
    let report = report(
        vec![
            (
                narrowed.clone(),
                Fixture::Classified(
                    FailureProvenance::NewRegression,
                    Some(CheckBaseline::Passed),
                    vec!["tests::a".to_string(), "tests::b".to_string()],
                ),
            ),
            (
                whole.clone(),
                Fixture::Classified(
                    FailureProvenance::PreExisting,
                    Some(failed_baseline(&whole, &pre_findings)),
                    Vec::new(),
                ),
            ),
            (
                missing.clone(),
                classified(FailureProvenance::UnknownProvenance),
            ),
        ],
        Some(VerificationOutcome::Failed),
    );
    let line_for = |target: &CheckId| {
        report
            .provenance
            .iter()
            .flat_map(|partition| &partition.checks)
            .find(|entry| &entry.check == target)
            .map(|entry| entry.baseline.clone())
            .expect("every failed check is in a group")
    };
    assert_eq!(
        line_for(&narrowed),
        "Before this task's changes: the 2 failing tests passed when run alone"
    );
    assert_eq!(
        line_for(&whole),
        "Before this task's changes: failed the same way"
    );
    assert_eq!(
        line_for(&missing),
        "Before this task's changes: no baseline was recorded, so Damaian cannot compare"
    );

    // D5: the report lists the baselines it compared against, in selected
    // order, with the engine's line.
    let baselines = &report.baselines;
    assert_eq!(baselines.len(), 2, "{baselines:?}");
    assert_eq!(baselines[0].check, narrowed);
    assert_eq!(baselines[0].result, "passed");
    assert_eq!(baselines[0].tests, vec!["tests::a", "tests::b"]);
    assert_eq!(
        baselines[0].label,
        "Before this task's changes: cargo test (repository root) passed"
    );
    assert_eq!(baselines[1].result, "failed");
    assert_eq!(
        baselines[1].label,
        "Before this task's changes: cargo test (a) failed"
    );
    let rendered = report.render();
    assert!(rendered.contains("Baselines"), "{rendered}");
    assert!(
        rendered.contains("Before this task's changes: failed the same way"),
        "{rendered}"
    );
}

#[test]
fn a_stored_report_without_a_verdict_reads_back_without_one() {
    let report = report(
        vec![(check(""), Fixture::Passed)],
        Some(VerificationOutcome::Passed),
    );
    let mut value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["verdict"]["kind"], "passed");
    value.as_object_mut().unwrap().remove("verdict");
    let stored: VerificationReport = serde_json::from_value(value).unwrap();
    assert_eq!(
        stored.verdict, None,
        "reading an old report invents nothing"
    );
    assert!(stored.render().starts_with("Files changed"));
}
