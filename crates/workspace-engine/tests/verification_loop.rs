use std::collections::BTreeMap;

use serde_json::json;
use workspace_engine::plan::StepStatus;
use workspace_engine::{
    CheckId, CheckOutcome, Evidence, PlanReport, PlanStep, SkipReason, TaskPlan, VerificationEvent,
    VerificationOutcome, VerificationReport, VerificationState, fold_verification,
};

fn check(directory: &str, command: &str) -> CheckId {
    CheckId::new(directory, command)
}

fn started(attempt: u32, checks: &[CheckId]) -> VerificationEvent {
    VerificationEvent::Started {
        task_id: "task".to_string(),
        attempt,
        checks: checks.to_vec(),
    }
}

fn ran(check: CheckId, exit_code: Option<i32>) -> VerificationEvent {
    VerificationEvent::Check {
        task_id: "task".to_string(),
        check,
        outcome: CheckOutcome::Ran {
            exit_code,
            marker_id: "marker".to_string(),
        },
        file_hashes: BTreeMap::new(),
    }
}

fn ran_with_hashes(
    check: CheckId,
    exit_code: Option<i32>,
    hashes: &[(&str, &str)],
) -> VerificationEvent {
    VerificationEvent::Check {
        task_id: "task".to_string(),
        check,
        outcome: CheckOutcome::Ran {
            exit_code,
            marker_id: "marker".to_string(),
        },
        file_hashes: hashes
            .iter()
            .map(|(path, hash)| ((*path).to_string(), (*hash).to_string()))
            .collect(),
    }
}

fn skipped(check: CheckId, reason: SkipReason) -> VerificationEvent {
    VerificationEvent::Check {
        task_id: "task".to_string(),
        check,
        outcome: CheckOutcome::Skipped(reason),
        file_hashes: BTreeMap::new(),
    }
}

fn finished(outcome: VerificationOutcome) -> VerificationEvent {
    VerificationEvent::Finished {
        task_id: "task".to_string(),
        outcome,
    }
}

fn due() -> VerificationEvent {
    VerificationEvent::Due {
        task_id: "task".to_string(),
        patch_id: "patch".to_string(),
    }
}

fn empty_plan() -> TaskPlan {
    TaskPlan::new("task", 0)
}

/// A plan with one verified step. The plan is "complete" by spec 21's rule, so a
/// summary that withholds the word has to be withholding it for the
/// verification's own reason.
fn complete_plan() -> TaskPlan {
    let mut plan = empty_plan();
    plan.steps.push(PlanStep {
        id: "step-1".to_string(),
        title: "understand existing retry logic".to_string(),
        detail: None,
        status: StepStatus::Completed,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: vec![Evidence::FileRead {
            path: "src/retry.rs".to_string(),
            hash: "sha256:abc".to_string(),
        }],
    });
    plan
}

fn complete_plan_report() -> PlanReport {
    let report = complete_plan().report();
    assert!(report.is_complete);
    report
}

fn state(events: &[VerificationEvent]) -> VerificationState {
    fold_verification(events)
}

fn report_for(events: &[VerificationEvent]) -> VerificationReport {
    VerificationReport::build(&state(events), &empty_plan(), &[], &BTreeMap::new())
}

#[test]
fn a_check_passes_only_on_exit_code_zero() {
    let passing = check("crates/engine", "cargo test");
    let failing = check("", "cargo test");
    let signalled = check("packages/web", "npm run test");
    let report = report_for(&[
        started(0, &[passing.clone(), failing.clone(), signalled.clone()]),
        ran(passing.clone(), Some(0)),
        ran(failing.clone(), Some(1)),
        ran(signalled.clone(), None),
    ]);

    assert_eq!(report.passed, vec![passing]);
    assert!(
        report
            .failed
            .iter()
            .any(|item| item.check == failing && item.reason == "exit code 1")
    );
    assert!(
        report
            .failed
            .iter()
            .any(|item| item.check == signalled && item.reason == "killed or signalled"),
        "exit_code None means killed or signalled and belongs in failed, never passed"
    );
    assert!(
        !report.passed.iter().any(|item| item == &signalled),
        "None is not a zero"
    );
}

#[test]
fn every_selected_check_is_in_exactly_one_section() {
    let passing = check("", "cargo test");
    let failing = check("crates/engine", "cargo test");
    let declined = check("packages/web", "npm run lint");
    let blocked = check("packages/web", "npm run build");
    let no_target = CheckId::browser("http://127.0.0.1:3000/");
    let stale = check("crates/cli", "cargo test");

    let mut current = BTreeMap::new();
    current.insert("src/a.rs".to_string(), "new".to_string());
    let events = [
        started(
            0,
            &[
                passing.clone(),
                failing.clone(),
                declined.clone(),
                blocked.clone(),
                no_target.clone(),
                stale.clone(),
            ],
        ),
        ran(passing.clone(), Some(0)),
        ran(failing.clone(), Some(1)),
        skipped(declined.clone(), SkipReason::DeclinedByUser),
        skipped(
            blocked.clone(),
            SkipReason::Blocked {
                reason: "command_blocklist".to_string(),
            },
        ),
        skipped(no_target.clone(), SkipReason::NoRunningTarget),
        ran_with_hashes(stale.clone(), Some(0), &[("src/a.rs", "old")]),
    ];
    let report = VerificationReport::build(&state(&events), &empty_plan(), &[], &current);

    let mut union: Vec<CheckId> = Vec::new();
    union.extend(report.passed.iter().cloned());
    union.extend(report.failed.iter().map(|item| item.check.clone()));
    union.extend(report.skipped.iter().map(|item| item.check.clone()));
    union.extend(report.stale.iter().cloned());
    union.sort();
    union.dedup();

    let mut selected = vec![passing, failing, declined, blocked, no_target, stale];
    selected.sort();
    assert_eq!(
        union, selected,
        "every selected check is accounted for once"
    );
    assert_eq!(
        report.passed.len() + report.failed.len() + report.skipped.len() + report.stale.len(),
        selected.len(),
        "no check appears in two sections"
    );
    assert_eq!(report.stale, vec![check("crates/cli", "cargo test")]);
}

#[test]
fn no_checks_available_is_not_checks_passed() {
    let events = [
        due(),
        started(0, &[]),
        finished(VerificationOutcome::NoChecksAvailable),
    ];
    let report = report_for(&events);

    assert_eq!(report.outcome, Some(VerificationOutcome::NoChecksAvailable));
    assert!(report.passed.is_empty());
    assert!(report.failed.is_empty());
    assert!(report.render().contains("No checks were available"));
    let summary = report.summary(&complete_plan_report());
    assert!(
        !summary.contains("passed"),
        "no checks available must not read as checks passing: {summary}"
    );
    assert!(
        !summary.contains("complete"),
        "nothing was verified, so nothing completed: {summary}"
    );
}

#[test]
fn only_documentation_changed_is_its_own_outcome() {
    let events = [
        due(),
        started(0, &[]),
        finished(VerificationOutcome::OnlyDocumentationChanged),
    ];
    let report = report_for(&events);
    assert_eq!(
        report.outcome,
        Some(VerificationOutcome::OnlyDocumentationChanged)
    );
    assert!(report.passed.is_empty());
    assert!(report.render().contains("only documentation changed"));
}

#[test]
fn the_latest_attempt_decides_a_check() {
    let fixed = check("crates/engine", "cargo test");
    let broke = check("", "cargo test");
    let events = [
        started(0, &[fixed.clone(), broke.clone()]),
        ran(fixed.clone(), Some(1)),
        ran(broke.clone(), Some(0)),
        started(1, &[fixed.clone(), broke.clone()]),
        ran(fixed.clone(), Some(0)),
        ran(broke.clone(), Some(1)),
        finished(VerificationOutcome::Failed),
    ];
    let report = report_for(&events);

    assert_eq!(report.passed, vec![fixed.clone()]);
    assert!(
        report.failed.iter().any(|item| item.check == broke),
        "the latest failing outcome decides"
    );
    assert_eq!(
        report.regressions,
        vec![broke],
        "a check that passed earlier and fails now is a regression"
    );
}

#[test]
fn a_check_whose_files_changed_is_stale_not_passed() {
    let edited = check("crates/engine", "cargo test");
    let deleted = check("", "cargo test");
    let events = [
        started(0, &[edited.clone(), deleted.clone()]),
        ran_with_hashes(edited.clone(), Some(0), &[("src/a.rs", "h1")]),
        ran_with_hashes(deleted.clone(), Some(0), &[("src/gone.rs", "h2")]),
    ];

    let mut current = BTreeMap::new();
    current.insert("src/a.rs".to_string(), "h2".to_string());

    let report = VerificationReport::build(&state(&events), &empty_plan(), &[], &current);
    assert!(report.passed.is_empty());
    assert!(report.stale.contains(&edited));
    assert!(
        report.stale.contains(&deleted),
        "a file absent from the working tree is a mismatch, not a pass"
    );
}

#[test]
fn the_summary_never_says_complete_when_anything_is_wrong() {
    let plan_report = complete_plan_report();
    let failing = check("", "cargo test");

    let failed = report_for(&[
        started(0, std::slice::from_ref(&failing)),
        ran(failing.clone(), Some(1)),
        finished(VerificationOutcome::Failed),
    ]);
    let exhausted = report_for(&[
        started(0, std::slice::from_ref(&failing)),
        ran(failing.clone(), Some(1)),
        finished(VerificationOutcome::RepairLimitExhausted),
    ]);
    let no_repair = report_for(&[
        started(0, std::slice::from_ref(&failing)),
        ran(failing.clone(), Some(1)),
        finished(VerificationOutcome::NoRepairApplied),
    ]);
    let due = report_for(&[
        // A second patch was applied after the first verification passed, so
        // there is a passing history *and* a new verification that has not run.
        started(0, std::slice::from_ref(&failing)),
        ran(failing.clone(), Some(0)),
        finished(VerificationOutcome::Passed),
        due(),
    ]);

    for (label, report) in [
        ("failed check", failed),
        ("repair limit exhausted", exhausted),
        ("no repair applied", no_repair),
        ("verification due", due),
    ] {
        let summary = report.summary(&plan_report);
        assert!(
            !summary.contains("complete"),
            "{label} must withhold the word: {summary}"
        );
    }

    let passed = report_for(&[
        started(0, &[check("", "cargo test")]),
        ran(check("", "cargo test"), Some(0)),
        finished(VerificationOutcome::Passed),
    ]);
    assert!(
        passed.summary(&plan_report).contains("complete"),
        "the gating must not withhold the word when verification actually passed"
    );
}

#[test]
fn the_fold_does_not_depend_on_how_events_were_batched() {
    let a = check("", "cargo test");
    let b = check("crates/engine", "cargo test");
    let events = [
        due(),
        started(0, &[a.clone(), b.clone()]),
        ran(a.clone(), Some(1)),
        ran(b.clone(), Some(0)),
        started(1, &[a.clone(), b.clone()]),
        ran(a.clone(), Some(0)),
        finished(VerificationOutcome::Passed),
    ];

    let all_at_once = fold_verification(events.iter());

    let prefix = 4;
    let mut in_batches = fold_verification(events[..prefix].iter());
    for event in &events[prefix..] {
        in_batches.apply(event);
    }

    assert_eq!(all_at_once, in_batches);
}

#[test]
fn events_serialise_in_the_shape_the_log_holds() {
    assert_eq!(
        serde_json::to_value(VerificationEvent::Due {
            task_id: "t".to_string(),
            patch_id: "p".to_string(),
        })
        .unwrap(),
        json!({"kind": "verification_due", "taskId": "t", "patchId": "p"})
    );
    assert_eq!(
        serde_json::to_value(started(0, &[check("crates/engine", "cargo test")])).unwrap(),
        json!({
            "kind": "verification_started",
            "taskId": "task",
            "attempt": 0,
            "checks": [{"workingDirectory": "crates/engine", "command": "cargo test"}],
        })
    );
    assert_eq!(
        serde_json::to_value(ran_with_hashes(
            check("", "cargo test"),
            Some(0),
            &[("src/a.rs", "h")]
        ))
        .unwrap(),
        json!({
            "kind": "verification_check",
            "taskId": "task",
            "check": {"workingDirectory": "", "command": "cargo test"},
            "outcome": {"outcome": "ran", "exitCode": 0, "markerId": "marker"},
            "fileHashes": {"src/a.rs": "h"},
        })
    );
    assert_eq!(
        serde_json::to_value(skipped(
            check("packages/web", "npm run lint"),
            SkipReason::DeclinedByUser
        ))
        .unwrap()["outcome"],
        json!({"outcome": "skipped", "reason": "declined_by_user"})
    );
    assert_eq!(
        serde_json::to_value(SkipReason::Blocked {
            reason: "command_blocklist".to_string()
        })
        .unwrap(),
        json!({"reason": "blocked", "policyReason": "command_blocklist"})
    );
    assert_eq!(
        serde_json::to_value(SkipReason::NoRunningTarget).unwrap(),
        json!({"reason": "no_running_target"})
    );
    assert_eq!(
        serde_json::to_value(finished(VerificationOutcome::RepairLimitExhausted)).unwrap(),
        json!({
            "kind": "verification_finished",
            "taskId": "task",
            "outcome": "repair_limit_exhausted",
        })
    );
}

#[test]
fn a_check_is_labelled_by_its_command_and_root() {
    assert_eq!(
        check("crates/engine", "cargo test").label(),
        "cargo test (crates/engine)"
    );
    assert_eq!(
        check("", "cargo test").label(),
        "cargo test (repository root)"
    );
    assert_eq!(
        CheckId::browser("http://127.0.0.1:3000/").label(),
        "browser check http://127.0.0.1:3000/"
    );
}

#[test]
fn a_passing_browser_check_is_behaviour_verified_as_well_as_passed() {
    let browser = CheckId::browser("http://127.0.0.1:3000/");
    let report = report_for(&[
        started(0, std::slice::from_ref(&browser)),
        ran(browser.clone(), Some(0)),
    ]);

    assert_eq!(report.passed, vec![browser]);
    assert_eq!(
        report.behaviour_verified,
        vec!["browser check http://127.0.0.1:3000/".to_string()]
    );
}

#[test]
fn an_interrupted_check_is_never_reported_as_passed() {
    let interrupted = check("", "cargo test");
    let report = report_for(&[due(), started(0, std::slice::from_ref(&interrupted))]);

    assert!(report.passed.is_empty());
    assert!(
        report
            .failed
            .iter()
            .any(|item| item.check == interrupted && item.reason == "no result was recorded")
    );
}
