//! The verification loop's pure rules, per
//! `docs/specs/23_verification_loop/`.
//!
//! These tests build states from event vectors, never from a store. Requirement
//! 7's rules — what "passed" means, that every selected check is accounted for,
//! that "no checks available" is not "checks passed", and that the summary never
//! claims completion it cannot support — are properties of the fold and the
//! report, so they are tested without a session, a model or a filesystem.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use workspace_engine::plan::StepStatus;
use workspace_engine::{
    CancelToken, ChatTurnResult, CheckBaseline, CheckId, CheckOutcome, CheckReuse, CommandPurpose,
    CompletionReasonCode, Config, ContextDecision, Evidence, FailureProvenance, HookConfig,
    HookEvent, HookMode, HookSource, MockModelAdapter, PLANNED_CHECK_VERSION, PausedTurns,
    PlanReport, PlanStep, PlannedCheck, ReattachedApproval, RepairStallReason, RoundBudgetDecision,
    Selection, SessionStore, SkipReason, TaskExecutionBudget, TaskPlan, TaskStatus, TurnProgress,
    TurnSink, VerificationEvent, VerificationOutcome, VerificationReport, VerificationState,
    WebDiagnosticCall, WebDiagnosticDetails, WebDiagnosticRecord, WebDiagnosticReport,
    WebDiagnosticsRunner, WebDiagnosticsRunnerHandle, WorkerRecord, WorkspaceEngine,
    detected_checks_of_roots, fold_verification, is_documentation, pending_verifications,
    propose_checks, propose_scoped_checks, reattach_pending_approvals, scoped_selection,
    select_roots,
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
            termination: None,
        },
        file_hashes: BTreeMap::new(),
        generation: 0,
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
            termination: None,
        },
        file_hashes: hashes
            .iter()
            .map(|(path, hash)| ((*path).to_string(), (*hash).to_string()))
            .collect(),
        generation: 0,
    }
}

/// A check that ran against a named generation and file digest (spec 105).
fn ran_at_generation(
    check: CheckId,
    exit_code: Option<i32>,
    hashes: &[(&str, &str)],
    generation: u32,
) -> VerificationEvent {
    VerificationEvent::Check {
        task_id: "task".to_string(),
        check,
        outcome: CheckOutcome::Ran {
            exit_code,
            marker_id: "marker".to_string(),
            termination: None,
        },
        file_hashes: hashes
            .iter()
            .map(|(path, hash)| ((*path).to_string(), (*hash).to_string()))
            .collect(),
        generation,
    }
}

fn skipped(check: CheckId, reason: SkipReason) -> VerificationEvent {
    VerificationEvent::Check {
        task_id: "task".to_string(),
        check,
        outcome: CheckOutcome::Skipped(reason),
        file_hashes: BTreeMap::new(),
        generation: 0,
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
        paths: Vec::new(),
        checks: Vec::new(),
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
        check: None,
        build: None,
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
            paths: Vec::new(),
            checks: Vec::new(),
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

// --- Mutation-batch generation (spec 105 Task 4) --------------------------

fn step(id: &str, title: &str, status: StepStatus) -> PlanStep {
    PlanStep {
        id: id.to_string(),
        title: title.to_string(),
        detail: None,
        status,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
        check: None,
        build: None,
    }
}

#[test]
fn two_patches_in_one_implementation_step_are_one_generation() {
    let mut plan = empty_plan();
    let mut implement = step("implement", "Implement the change", StepStatus::InProgress);
    implement.evidence.extend([
        Evidence::PatchApplied {
            marker_id: "patch-a".to_string(),
            files: Vec::new(),
        },
        Evidence::PatchApplied {
            marker_id: "patch-b".to_string(),
            files: Vec::new(),
        },
    ]);
    plan.steps.push(implement);
    plan.steps
        .push(step("test", "Validate the change", StepStatus::Pending));

    // Two patches in one open primary step are one mutation batch: the
    // generation does not count patches.
    assert_eq!(plan.mutation_generation(), 0);

    // Closing the implementation step and opening the final step advances the
    // generation, not the number of patches.
    plan.steps[0].status = StepStatus::Completed;
    plan.steps[1].status = StepStatus::InProgress;
    assert_eq!(plan.mutation_generation(), 1);
}

#[test]
fn closing_the_final_check_step_records_the_current_generation() {
    let mut plan = empty_plan();
    plan.steps.push(step(
        "implement",
        "Implement the change",
        StepStatus::Completed,
    ));
    plan.steps
        .push(step("test", "Validate the change", StepStatus::InProgress));
    let generation = plan.mutation_generation();
    assert_eq!(generation, 1);

    let check = CheckId::new("", "cargo test");
    let events = [
        started(0, std::slice::from_ref(&check)),
        ran_at_generation(check.clone(), Some(0), &[], generation),
        finished(VerificationOutcome::Passed),
    ];
    let report = VerificationReport::build(&state(&events), &plan, &[], &BTreeMap::new());
    assert_eq!(
        report.passed,
        vec![check],
        "evidence recorded at the final step's generation is current, not stale"
    );
    assert!(report.stale.is_empty());
}

#[test]
fn a_mutation_or_an_advanced_generation_makes_a_check_stale_without_deleting_history() {
    let check = CheckId::new("", "cargo test");
    let ran = ran_at_generation(check.clone(), Some(0), &[("src/a.rs", "h1")], 0);
    let state = state(&[started(0, std::slice::from_ref(&check)), ran]);

    // Advancing to the next primary step (generation 1) makes the evidence
    // stale even though the file digest is unchanged.
    let mut advanced = empty_plan();
    advanced.steps.push(step(
        "implement",
        "Implement the change",
        StepStatus::Completed,
    ));
    advanced
        .steps
        .push(step("test", "Validate the change", StepStatus::InProgress));
    let mut current = BTreeMap::new();
    current.insert("src/a.rs".to_string(), "h1".to_string());
    let report = VerificationReport::build(&state, &advanced, &[], &current);
    assert!(report.passed.is_empty());
    assert_eq!(report.stale, vec![check.clone()]);

    // A changed digest makes it stale at the same generation too.
    let mut same_generation = empty_plan();
    same_generation.steps.push(step(
        "implement",
        "Implement the change",
        StepStatus::InProgress,
    ));
    let mut changed = BTreeMap::new();
    changed.insert("src/a.rs".to_string(), "h2".to_string());
    let report = VerificationReport::build(&state, &same_generation, &[], &changed);
    assert_eq!(report.stale, vec![check.clone()]);

    // Staleness does not delete history: the original run is still in the fold.
    let run = state.checks.get(&check).expect("history is preserved");
    assert_eq!(run.generation, 0);
    assert_eq!(
        run.file_hashes,
        BTreeMap::from([("src/a.rs".to_string(), "h1".to_string())])
    );
}

// --- Selection by root (Task 2) -------------------------------------------

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-verify-{name}-{now}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

const MONOREPO: &[(&str, &str)] = &[
    ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
    ("crates/engine/Cargo.toml", "[package]\nname = \"engine\"\n"),
    ("crates/engine/src/lib.rs", "pub fn engine() {}\n"),
    ("crates/cli/Cargo.toml", "[package]\nname = \"cli\"\n"),
    ("crates/cli/src/main.rs", "fn main() {}\n"),
    (
        "packages/web/package.json",
        "{\"scripts\":{\"test\":\"node test.js\"}}",
    ),
    ("packages/web/src/index.ts", "export {};\n"),
    ("docs/guide.md", "# Guide\n"),
    ("Makefile", "all:\n\ttrue\n"),
    ("build.sh", "#!/bin/sh\ntrue\n"),
];

struct Monorepo {
    root: PathBuf,
    data_dir: PathBuf,
}

impl Monorepo {
    fn new(name: &str) -> Self {
        let root = temp_dir(name);
        for (path, content) in MONOREPO {
            let file = root.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, content).unwrap();
        }
        Self {
            root,
            data_dir: temp_dir(&format!("{name}-data")),
        }
    }

    fn config(&self) -> Config {
        Config {
            data_dir: self.data_dir.clone(),
            enable_index_watcher: false,
            shell: "/usr/bin/true".to_string(),
            ..Config::default()
        }
    }

    fn engine(&self) -> WorkspaceEngine {
        WorkspaceEngine::new(self.config())
    }

    fn map(&self, engine: &WorkspaceEngine) -> workspace_engine::RepositoryMap {
        engine
            .repository_map(&self.root)
            .expect("the fixture should map")
            .0
    }

    fn pending_proposals(&self) -> usize {
        let directory = self.data_dir.join("commands").join("pending");
        match fs::read_dir(directory) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "dcmd"))
                .count(),
            Err(_) => 0,
        }
    }
}

impl Drop for Monorepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.data_dir);
    }
}

/// Review fix R16 (spec 23 D2 piece 2): a change selects its owning root only.
/// The enclosing roots belong to the task-completion floor, not to every
/// change, so a member edit no longer runs the whole workspace suite.
#[test]
fn a_member_change_selects_only_its_owning_root() {
    let repo = Monorepo::new("member");
    let engine = repo.engine();
    let map = repo.map(&engine);
    let changed = vec!["crates/engine/src/lib.rs".to_string()];

    assert_eq!(
        select_roots(&map, &changed),
        vec!["crates/engine".to_string()],
        "per-change selection is the owning root only"
    );

    let selection = propose_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &changed,
        &mut CheckReuse::none(),
    )
    .unwrap();
    let Selection::Proposed(checks) = selection else {
        panic!("expected proposed checks, got {selection:?}");
    };
    let directories: Vec<&str> = checks
        .iter()
        .map(|(check, _)| check.working_directory.as_str())
        .collect();
    // Only the member's own root is proposed. Its `cargo test` is kept because
    // the workspace root is no longer in the restricted map (spec 90's
    // de-duplication applies only when both roots are selected).
    assert_eq!(directories, vec!["crates/engine"], "{directories:?}");
    assert!(!directories.contains(&""));
    assert!(!directories.contains(&"crates/cli"));
    assert!(!directories.contains(&"packages/web"));
}

/// Spec 109 D4: a plan's required check covers a change's owning root when its
/// directory is that root or an enclosing one; a covered root selects no
/// detected check, an uncovered root keeps its own, and a directory that is not
/// a root covers nothing.
#[test]
fn a_plan_check_covers_its_own_and_enclosed_roots_only() {
    let repo = Monorepo::new("d4-scoped-selection");
    let engine = repo.engine();
    let map = repo.map(&engine);
    let engine_change = vec!["crates/engine/src/lib.rs".to_string()];
    let both = vec![
        "crates/engine/src/lib.rs".to_string(),
        "packages/web/src/index.ts".to_string(),
    ];
    let at_root = CheckId::new("", "cargo nextest run -p engine --test a --test b");
    let in_crate = CheckId::new("crates/engine", "cargo nextest run --test a");
    let elsewhere = CheckId::new("crates/cli", "cargo nextest run -p cli");
    let not_a_root = CheckId::new("crates/engine/src", "cargo nextest run --test a");

    let scoped = scoped_selection(
        &map,
        &engine_change,
        std::slice::from_ref(&at_root),
        std::slice::from_ref(&at_root),
    );
    assert_eq!(scoped.plan_checks, vec![at_root.clone()], "{scoped:?}");
    assert!(scoped.detected_roots.is_empty(), "{scoped:?}");

    let scoped = scoped_selection(
        &map,
        &engine_change,
        std::slice::from_ref(&in_crate),
        std::slice::from_ref(&in_crate),
    );
    assert_eq!(scoped.plan_checks, vec![in_crate.clone()], "{scoped:?}");

    let scoped = scoped_selection(
        &map,
        &engine_change,
        &[elsewhere.clone(), not_a_root.clone()],
        &[elsewhere, not_a_root],
    );
    assert!(scoped.plan_checks.is_empty(), "{scoped:?}");
    assert_eq!(scoped.detected_roots, vec!["crates/engine".to_string()]);

    // The workspace-root check also encloses `packages/web`: one plan check
    // covers both roots and appears once.
    let scoped = scoped_selection(
        &map,
        &both,
        std::slice::from_ref(&at_root),
        std::slice::from_ref(&at_root),
    );
    assert_eq!(scoped.plan_checks, vec![at_root.clone()], "{scoped:?}");
    assert!(scoped.detected_roots.is_empty(), "{scoped:?}");
    let scoped = scoped_selection(
        &map,
        &both,
        std::slice::from_ref(&in_crate),
        std::slice::from_ref(&in_crate),
    );
    assert_eq!(scoped.detected_roots, vec!["packages/web".to_string()]);

    // Proposed: the plan check stands in for the crate's `cargo test`, the
    // uncovered web root keeps its detected check.
    let Selection::Proposed(checks) = propose_scoped_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &both,
        &scoped_selection(
            &map,
            &both,
            std::slice::from_ref(&in_crate),
            std::slice::from_ref(&in_crate),
        ),
        &|_| false,
        &mut CheckReuse::none(),
    )
    .unwrap() else {
        panic!("expected proposed checks");
    };
    let proposed: Vec<&CheckId> = checks.iter().map(|(check, _)| check).collect();
    assert!(proposed.contains(&&in_crate), "{proposed:?}");
    assert!(
        !proposed.contains(&&CheckId::new("crates/engine", "cargo test")),
        "the covered root's detected check is not proposed per change: {proposed:?}"
    );
    assert!(
        proposed
            .iter()
            .any(|check| check.working_directory == "packages/web"),
        "{proposed:?}"
    );

    // Deferred to its open step: it still displaces the detected check, is
    // not proposed itself, and nothing is stored.
    let before = repo.pending_proposals();
    let selection = propose_scoped_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &engine_change,
        &scoped_selection(
            &map,
            &engine_change,
            std::slice::from_ref(&in_crate),
            std::slice::from_ref(&in_crate),
        ),
        &|_| true,
        &mut CheckReuse::none(),
    )
    .unwrap();
    assert_eq!(selection, Selection::Proposed(Vec::new()));
    assert_eq!(repo.pending_proposals(), before, "nothing is stored");

    // The covering check is a finished step's and the change's own step has
    // none: nothing stands in per change, the crate's detected check is left
    // to the completion floor, and nothing is proposed or stored.
    let scoped = scoped_selection(&map, &engine_change, std::slice::from_ref(&at_root), &[]);
    assert!(scoped.plan_checks.is_empty(), "{scoped:?}");
    assert!(scoped.detected_roots.is_empty(), "{scoped:?}");
    assert_eq!(scoped.completion_roots, vec!["crates/engine".to_string()]);
    assert_eq!(
        detected_checks_of_roots(&map, &scoped.completion_roots),
        vec![CheckId::new("crates/engine", "cargo test")]
    );
    // A current check covering only the crate stands in for it; the web root,
    // covered by the finished step's workspace-root check alone, waits.
    let scoped = scoped_selection(
        &map,
        &both,
        &[at_root.clone(), in_crate.clone()],
        std::slice::from_ref(&in_crate),
    );
    assert_eq!(scoped.plan_checks, vec![in_crate.clone()], "{scoped:?}");
    assert_eq!(scoped.completion_roots, vec!["packages/web".to_string()]);
    let before = repo.pending_proposals();
    let selection = propose_scoped_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &engine_change,
        &scoped_selection(&map, &engine_change, std::slice::from_ref(&at_root), &[]),
        &|_| false,
        &mut CheckReuse::none(),
    )
    .unwrap();
    assert_eq!(selection, Selection::Proposed(Vec::new()));
    assert_eq!(repo.pending_proposals(), before, "nothing is stored");

    // No plan check: exactly today's per-change selection.
    let today = propose_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &engine_change,
        &mut CheckReuse::none(),
    )
    .unwrap();
    let scoped = propose_scoped_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &engine_change,
        &scoped_selection(&map, &engine_change, &[], &[]),
        &|_| false,
        &mut CheckReuse::none(),
    )
    .unwrap();
    let ids = |selection: &Selection| match selection {
        Selection::Proposed(checks) => checks.iter().map(|(check, _)| check.clone()).collect(),
        _ => Vec::new(),
    };
    assert_eq!(ids(&scoped), ids(&today));
    assert_eq!(
        ids(&today),
        vec![CheckId::new("crates/engine", "cargo test")]
    );
}

/// Review fix R16: the per-change verification turn runs the owning root's
/// check and not the enclosing workspace root's. The floor still holds the
/// enclosing root for task completion (see the R15 tests below).
#[test]
fn a_member_change_runs_only_the_members_check_per_change() {
    let repo = Monorepo::new("r16-member-per-change");
    let engine = WorkspaceEngine::new(Config {
        data_dir: repo.data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        command_allowlist: vec![
            "cargo test".to_string(),
            "cd crates/engine && cargo check --tests".to_string(),
        ],
        ..Config::default()
    });
    let (session, task) = applied_change(
        &engine,
        &repo.root,
        &[(
            "crates/engine/src/lib.rs",
            "pub fn engine() {}\n",
            "pub fn engine() { let _ = 1; }\n",
        )],
    );
    // An open step keeps this a mid-task per-change verification, not the
    // completion run the floor owns.
    open_plan(&engine, &session, &task, &["Keep working"]);
    let mut adapter = MockModelAdapter::new("Done.");

    let _ = run_verification(&engine, &repo.root, &session, &mut adapter);

    let events = engine
        .session_store
        .read_verification_events(&session, &[])
        .expect("the log should read");
    let selected = events
        .iter()
        .rev()
        .find_map(|event| match event {
            VerificationEvent::Started { checks, .. } => Some(checks.clone()),
            _ => None,
        })
        .expect("a verification started");
    assert_eq!(
        selected,
        vec![check("crates/engine", "cargo test")],
        "per change only the member's own check runs"
    );
}

/// Spec 109 D1 replaces review fix R16 (f): a member's patch takes no
/// baseline before it applies, neither the member's per-change check nor the
/// enclosing root's floor check. A baseline is taken only when one of them
/// fails afterwards.
#[test]
fn a_member_change_takes_no_baseline_before_it_applies() {
    let repo = Monorepo::new("r16-member-baseline");
    let engine = WorkspaceEngine::new(Config {
        data_dir: repo.data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        command_allowlist: vec!["cargo test".to_string()],
        ..Config::default()
    });
    write(
        &repo.root,
        "crates/engine/src/lib.rs",
        "pub fn engine() {}\n",
    );
    let proposal = propose(
        &engine,
        &repo.root,
        &edit_response(&[(
            "crates/engine/src/lib.rs",
            "pub fn engine() { let _ = 1; }\n",
        )]),
    );
    engine
        .edit_orchestrator
        .apply_stored_patch(&repo.root, &proposal.patch.id, None, None, "tester", false)
        .expect("the patch applies with no baseline gate");

    let baselines = engine
        .session_store
        .task_verification_baselines(&proposal.session.id, &proposal.task.id)
        .expect("the baselines should read");
    assert!(baselines.is_empty(), "{baselines:?}");
}

/// Review fix R16 (c): a file owned directly by the root still runs the root
/// check per change. Only the enclosing-root part of selection is removed.
#[test]
fn a_root_owned_change_still_runs_the_root_check_per_change() {
    let repo = Monorepo::new("r16-root-per-change");
    let engine = WorkspaceEngine::new(Config {
        data_dir: repo.data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        command_allowlist: vec!["cargo test".to_string(), "cargo check --tests".to_string()],
        ..Config::default()
    });
    let (session, task) = applied_change(
        &engine,
        &repo.root,
        &[("Makefile", "all:\n\ttrue\n", "all:\n\tfalse\n")],
    );
    open_plan(&engine, &session, &task, &["Keep working"]);
    let mut adapter = MockModelAdapter::new("Done.");

    let _ = run_verification(&engine, &repo.root, &session, &mut adapter);

    let events = engine
        .session_store
        .read_verification_events(&session, &[])
        .expect("the log should read");
    let selected = events
        .iter()
        .rev()
        .find_map(|event| match event {
            VerificationEvent::Started { checks, .. } => Some(checks.clone()),
            _ => None,
        })
        .expect("a verification started");
    assert_eq!(
        selected,
        vec![check("", "cargo test")],
        "a root-owned change still runs the root check per change"
    );
}

/// Spec 90 + R16: a root that is a package as well as a workspace runs only
/// its own package's tests, so the member's `cargo test` is kept in the
/// completion floor beside the root's. Per change, R16 selects the owning
/// member root only.
#[test]
fn a_member_check_is_kept_in_the_completion_floor_when_the_root_does_not_run_it() {
    let repo = Monorepo::new("member-kept");
    fs::write(
        repo.root.join("Cargo.toml"),
        "[package]\nname = \"root\"\n\n[workspace]\nmembers = [\"crates/*\"]\n",
    )
    .unwrap();
    let engine = repo.engine();
    let map = repo.map(&engine);
    let changed = vec!["crates/engine/src/lib.rs".to_string()];

    assert_eq!(select_roots(&map, &changed), vec!["crates/engine"]);

    let selection = workspace_engine::verification::propose_completion_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &changed,
        &mut CheckReuse::none(),
    )
    .unwrap();
    let Selection::Proposed(checks) = selection else {
        panic!("expected proposed checks, got {selection:?}");
    };
    let directories: Vec<&str> = checks
        .iter()
        .map(|(check, _)| check.working_directory.as_str())
        .collect();
    assert!(directories.contains(&""), "{directories:?}");
    assert!(directories.contains(&"crates/engine"), "{directories:?}");
}

#[test]
fn a_web_change_does_not_duplicate_the_test_script() {
    let repo = Monorepo::new("web");
    let engine = repo.engine();
    let map = repo.map(&engine);
    let changed = vec!["packages/web/src/index.ts".to_string()];

    // R16: the owning root only per change; the enclosing root stays in the
    // completion floor below.
    assert_eq!(
        select_roots(&map, &changed),
        vec!["packages/web".to_string()]
    );

    let selection = propose_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &changed,
        &mut CheckReuse::none(),
    )
    .unwrap();
    let Selection::Proposed(checks) = selection else {
        panic!("expected proposed checks, got {selection:?}");
    };
    let directories: Vec<&str> = checks
        .iter()
        .map(|(check, _)| check.working_directory.as_str())
        .collect();
    assert_eq!(directories, vec!["packages/web"], "{directories:?}");
    let web_commands: Vec<&str> = checks
        .iter()
        .filter(|(check, _)| check.working_directory == "packages/web")
        .map(|(check, _)| check.command.as_str())
        .collect();
    assert!(
        web_commands.contains(&"npm run test"),
        "the detected test script should be proposed: {web_commands:?}"
    );
    assert!(
        !web_commands.contains(&"npm test"),
        "test-shortcut runs the same script and is dropped within the root: {web_commands:?}"
    );

    // The floor still keeps the enclosing root: the workspace's `cargo test`
    // and the web root's npm script are both task-completion obligations.
    let floor = workspace_engine::verification::floor_checks(&repo.root, &map, &changed);
    assert!(floor.contains(&check("", "cargo test")), "{floor:?}");
    assert!(
        floor.contains(&check("packages/web", "npm run test")),
        "{floor:?}"
    );

    let restricted = map.restricted_to(&["packages/web".to_string()]);
    let names: Vec<&str> = restricted.roots[0]
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert!(names.contains(&"test"));
    assert!(!names.contains(&"test-shortcut"));
}

#[test]
fn only_documentation_changes_select_nothing() {
    let repo = Monorepo::new("docs");
    let engine = repo.engine();
    let map = repo.map(&engine);

    assert_eq!(
        propose_checks(
            &engine.validation_orchestrator,
            &repo.root,
            &map,
            &["docs/guide.md".to_string()],
            &mut CheckReuse::none(),
        )
        .unwrap(),
        Selection::OnlyDocumentation
    );
    assert!(select_roots(&map, &["docs/guide.md".to_string()]).is_empty());
    // R16 (d): the completion floor also selects nothing for documentation.
    assert!(
        workspace_engine::verification::floor_checks(
            &repo.root,
            &map,
            &["docs/guide.md".to_string()]
        )
        .is_empty()
    );

    assert!(is_documentation("docs/guide.md"));
    assert!(is_documentation("NOTES.MD"));
    assert!(!is_documentation("Makefile"));
    assert!(!is_documentation("build.sh"));
    assert!(!is_documentation("src/main.rs"));
    assert_eq!(
        select_roots(&map, &["Makefile".to_string()]),
        vec!["".to_string()],
        "a Makefile is not documentation, so the root that owns it is selected"
    );
}

#[test]
fn a_removed_root_is_never_selected() {
    let repo = Monorepo::new("removed");
    let engine = WorkspaceEngine::new(Config {
        project_roots_removed: vec!["packages/web".to_string()],
        ..repo.config()
    });
    let map = repo.map(&engine);

    assert_eq!(
        select_roots(&map, &["packages/web/src/index.ts".to_string()]),
        vec!["".to_string()],
        "a removed root owns nothing; its files belong to the nearest live root"
    );
}

#[test]
fn only_the_selected_roots_proposals_are_stored() {
    let repo = Monorepo::new("stored");
    let engine = repo.engine();
    let map = repo.map(&engine);
    let changed = vec!["crates/engine/src/lib.rs".to_string()];

    let before = repo.pending_proposals();
    let selection = propose_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &changed,
        &mut CheckReuse::none(),
    )
    .unwrap();
    let Selection::Proposed(checks) = selection else {
        panic!("expected proposed checks, got {selection:?}");
    };
    // The member's `cargo test` is covered by the workspace root's (spec 90),
    // so it is neither proposed nor stored.
    assert_eq!(checks.len(), 1, "only the repository root's cargo test");
    assert_eq!(
        repo.pending_proposals() - before,
        1,
        "every proposed check is stored once and nothing else is"
    );
}

#[test]
fn test_shortcut_is_dropped_only_within_the_root_that_has_the_script() {
    let command = |name: &str, command: &str| workspace_engine::RootCommand {
        name: name.to_string(),
        command: command.to_string(),
        risk: workspace_engine::CommandRisk::Low,
        working_directory: String::new(),
    };
    let root =
        |path: &str, commands: Vec<workspace_engine::RootCommand>| workspace_engine::ProjectRoot {
            path: path.to_string(),
            detected_by: workspace_engine::RootEvidence::RepositoryRoot,
            languages: Vec::new(),
            manifests: Vec::new(),
            entry_points: Vec::new(),
            test_paths: Vec::new(),
            generated_paths: Vec::new(),
            major_directories: Vec::new(),
            instruction_files: Vec::new(),
            commands,
            user_override: None,
        };
    let map = workspace_engine::RepositoryMap {
        repository_id: "repo".to_string(),
        schema_version: workspace_engine::REPOSITORY_MAP_SCHEMA_VERSION,
        generated_at_ms: 0,
        fingerprint: "fingerprint".to_string(),
        roots: vec![
            root(
                "",
                vec![
                    command("test", "npm run test"),
                    command("test-shortcut", "npm test"),
                ],
            ),
            // Only the shortcut, as a hand-built map can hold even though
            // detection would pair it with the script.
            root("packages/api", vec![command("test-shortcut", "npm test")]),
        ],
        excluded: Vec::new(),
    };

    let restricted = map.restricted_to(&["".to_string(), "packages/api".to_string()]);
    let root_names = |path: &str| -> Vec<String> {
        restricted
            .roots
            .iter()
            .find(|root| root.path == path)
            .unwrap()
            .commands
            .iter()
            .map(|command| command.name.clone())
            .collect()
    };
    assert!(!root_names("").contains(&"test-shortcut".to_string()));
    assert!(
        root_names("packages/api").contains(&"test-shortcut".to_string()),
        "dropping the shortcut in one root must not drop it in another"
    );
}

#[test]
fn a_repository_with_no_detected_checks_says_so() {
    let root = temp_dir("no-checks");
    let data_dir = temp_dir("no-checks-data");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        ..Config::default()
    });
    let map = engine.repository_map(&root).unwrap().0;

    let selection = propose_checks(
        &engine.validation_orchestrator,
        &root,
        &map,
        &["src/main.rs".to_string()],
        &mut CheckReuse::none(),
    )
    .unwrap();
    assert_eq!(
        selection,
        Selection::NoChecksAvailable {
            roots: vec!["".to_string()]
        }
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// --- Patch decisions and verification due (Task 3) ------------------------

fn write(repo: &std::path::Path, relative_path: &str, content: &str) {
    let path = repo.join(relative_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn edit_engine(root: &std::path::Path, data_dir: &std::path::Path) -> WorkspaceEngine {
    fs::create_dir_all(root).unwrap();
    WorkspaceEngine::new(Config {
        data_dir: data_dir.to_path_buf(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        ..Config::default()
    })
}

fn edit_response(files: &[(&str, &str)]) -> String {
    let mut response = String::from("DAMAIAN_EDIT_V1\nSUMMARY: change\n");
    for (path, content) in files {
        response.push_str(&format!(
            "FILE: {path}\nSTATUS: modified\nCONTENT:\n{content}END_FILE\n"
        ));
    }
    response.push_str("END_PATCH\n");
    response
}

fn propose(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    response: &str,
) -> workspace_engine::EditProposalResult {
    let mut adapter = MockModelAdapter::new(response);
    engine
        .edit_orchestrator
        .propose_edit(root, "change it", &[], None, &mut adapter)
        .expect("the edit should be proposed")
}

fn verification_events(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
) -> Vec<VerificationEvent> {
    engine
        .session_store
        .read_verification_events(session_id, &[task_id])
        .expect("the log should read")
}

fn due_count(engine: &WorkspaceEngine, session_id: &str, task_id: &str) -> usize {
    verification_events(engine, session_id, task_id)
        .iter()
        .filter(|event| matches!(event, VerificationEvent::Due { .. }))
        .count()
}

fn decisions(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
) -> Vec<workspace_engine::PatchFileDecision> {
    engine
        .session_store
        .read_patch_file_decisions(session_id, task_id)
        .expect("the log should read")
}

#[test]
fn applying_every_file_of_a_code_mode_patch_records_one_due() {
    let root = temp_dir("due-all");
    let data_dir = temp_dir("due-all-data");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = edit_engine(&root, &data_dir);
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    let session = proposal.session.id.clone();
    let task = proposal.task.id.clone();

    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &proposal.patch.id, None, None, "tester", false)
        .expect("the patch should apply");

    assert_eq!(due_count(&engine, &session, &task), 1);
    let recorded = decisions(&engine, &session, &task);
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].applied);
    assert_eq!(recorded[0].paths, vec!["src/a.rs".to_string()]);
    let state = fold_verification(verification_events(&engine, &session, &task).iter());
    assert!(
        state.due.is_some(),
        "the log should say verification is due"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn two_of_three_applied_is_not_settled_and_the_last_rejection_settles_it() {
    let root = temp_dir("due-partial");
    let data_dir = temp_dir("due-partial-data");
    write(&root, "src/a.rs", "let a = 1;\n");
    write(&root, "src/b.rs", "let b = 1;\n");
    write(&root, "src/c.rs", "let c = 1;\n");
    let engine = edit_engine(&root, &data_dir);
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[
            ("src/a.rs", "let a = 2;\n"),
            ("src/b.rs", "let b = 2;\n"),
            ("src/c.rs", "let c = 2;\n"),
        ]),
    );
    let session = proposal.session.id.clone();
    let task = proposal.task.id.clone();

    let approved = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
    engine
        .edit_orchestrator
        .apply_stored_patch(
            &root,
            &proposal.patch.id,
            Some(&approved),
            None,
            "tester",
            false,
        )
        .expect("the selected files should apply");
    assert_eq!(
        due_count(&engine, &session, &task),
        0,
        "an undecided file leaves the patch unsettled"
    );

    engine
        .edit_orchestrator
        .reject_stored_patch_files(&proposal.patch.id, &["src/c.rs".to_string()], "tester")
        .expect("the remaining file should be rejected");
    assert_eq!(due_count(&engine, &session, &task), 1);

    // The stored patch still holds the content statuses; the decision is the
    // session event, not a rewrite of the patch.
    let stored = engine.patch_store.load(&proposal.patch.id).unwrap();
    assert!(stored.files.iter().all(|file| file.status == "modified"));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn rejecting_every_file_of_a_first_patch_is_not_settled() {
    let root = temp_dir("due-rejected-first");
    let data_dir = temp_dir("due-rejected-first-data");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = edit_engine(&root, &data_dir);
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    let session = proposal.session.id.clone();
    let task = proposal.task.id.clone();

    engine
        .edit_orchestrator
        .reject_stored_patch_files(&proposal.patch.id, &["src/a.rs".to_string()], "tester")
        .expect("the file should be rejected");

    assert_eq!(due_count(&engine, &session, &task), 0);
    let recorded = decisions(&engine, &session, &task);
    assert_eq!(recorded.len(), 1);
    assert!(!recorded[0].applied);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn rejecting_every_file_of_a_repair_patch_still_records_a_due() {
    let root = temp_dir("due-rejected-repair");
    let data_dir = temp_dir("due-rejected-repair-data");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = edit_engine(&root, &data_dir);
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    let session = proposal.session.id.clone();
    let task = proposal.task.id.clone();

    // The repair turn began an attempt before proposing this patch.
    engine
        .session_store
        .append_verification_event(
            &session,
            &VerificationEvent::Started {
                task_id: task.clone(),
                attempt: 0,
                checks: vec![CheckId::new("", "cargo test")],
            },
        )
        .unwrap();

    engine
        .edit_orchestrator
        .reject_stored_patch_files(&proposal.patch.id, &["src/a.rs".to_string()], "tester")
        .expect("the file should be rejected");

    assert_eq!(
        due_count(&engine, &session, &task),
        1,
        "the attempt must be closeable with no_repair_applied"
    );
    let state = fold_verification(verification_events(&engine, &session, &task).iter());
    assert!(state.due.is_some());
    assert!(
        state.finished.is_none(),
        "rejecting the repair patch does not finish the attempt"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn an_ask_mode_session_never_records_a_due() {
    let root = temp_dir("due-ask");
    let data_dir = temp_dir("due-ask-data");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = edit_engine(&root, &data_dir);
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    let session = proposal.session.id.clone();
    let task = proposal.task.id.clone();
    engine
        .session_store
        .set_session_mode(&session, workspace_engine::SessionMode::Ask, "user")
        .unwrap();

    let refused = engine.edit_orchestrator.apply_stored_patch(
        &root,
        &proposal.patch.id,
        None,
        None,
        "tester",
        false,
    );
    assert!(refused.is_err(), "Ask mode must refuse the apply");
    assert_eq!(due_count(&engine, &session, &task), 0);
    assert!(decisions(&engine, &session, &task).is_empty());
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 1;\n"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_patch_with_no_session_applies_and_records_nothing() {
    let root = temp_dir("due-no-session");
    let data_dir = temp_dir("due-no-session-data");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = edit_engine(&root, &data_dir);
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    let session = proposal.session.id.clone();
    let task = proposal.task.id.clone();

    let mut orphan = proposal.patch.clone();
    orphan.session_id = String::new();
    orphan.task_id = None;
    engine.patch_store.save(&orphan).unwrap();

    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &orphan.id, None, None, "tester", false)
        .expect("a patch with no session still applies");
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 2;\n"
    );
    assert!(decisions(&engine, &session, &task).is_empty());
    assert!(verification_events(&engine, &session, &task).is_empty());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn an_ask_mode_session_never_records_a_due_for_a_repair_patch_either() {
    let root = temp_dir("due-ask-repair");
    let data_dir = temp_dir("due-ask-repair-data");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = edit_engine(&root, &data_dir);
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    let session = proposal.session.id.clone();
    let task = proposal.task.id.clone();
    engine
        .session_store
        .append_verification_event(
            &session,
            &VerificationEvent::Started {
                task_id: task.clone(),
                attempt: 0,
                checks: vec![CheckId::new("", "cargo test")],
            },
        )
        .unwrap();
    engine
        .session_store
        .set_session_mode(&session, workspace_engine::SessionMode::Ask, "user")
        .unwrap();

    engine
        .edit_orchestrator
        .reject_stored_patch_files(&proposal.patch.id, &["src/a.rs".to_string()], "tester")
        .expect("rejecting writes no repository file, so the mode does not refuse it");

    assert_eq!(due_count(&engine, &session, &task), 0);
    assert_eq!(
        decisions(&engine, &session, &task).len(),
        1,
        "the decision is recorded; only the verification trigger is withheld"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_decision_survives_a_repeated_apply_and_does_not_duplicate_the_due() {
    let root = temp_dir("due-repeat");
    let data_dir = temp_dir("due-repeat-data");
    write(&root, "src/a.rs", "let a = 1;\n");
    write(&root, "src/b.rs", "let b = 1;\n");
    let engine = edit_engine(&root, &data_dir);
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n"), ("src/b.rs", "let b = 2;\n")]),
    );
    let session = proposal.session.id.clone();
    let task = proposal.task.id.clone();

    engine
        .edit_orchestrator
        .apply_stored_patch(
            &root,
            &proposal.patch.id,
            Some(&["src/a.rs".to_string()]),
            None,
            "tester",
            false,
        )
        .expect("the first file should apply");
    assert_eq!(due_count(&engine, &session, &task), 0);

    // Applying the same file again cannot conflict-check against its old hash.
    assert!(
        engine
            .edit_orchestrator
            .apply_stored_patch(
                &root,
                &proposal.patch.id,
                Some(&["src/a.rs".to_string()]),
                None,
                "tester",
                false,
            )
            .is_err()
    );
    assert_eq!(
        decisions(&engine, &session, &task).len(),
        1,
        "the first decision survives the repeated call"
    );

    engine
        .edit_orchestrator
        .reject_stored_patch_files(&proposal.patch.id, &["src/b.rs".to_string()], "tester")
        .expect("the second file should be rejected");
    assert_eq!(due_count(&engine, &session, &task), 1);

    // A later settle of the same patch — here by applying a file after
    // rejecting it — records another decision but never a second `Due`.
    engine
        .edit_orchestrator
        .apply_stored_patch(
            &root,
            &proposal.patch.id,
            Some(&["src/b.rs".to_string()]),
            None,
            "tester",
            false,
        )
        .expect("the rejected file can still be applied later");
    assert_eq!(due_count(&engine, &session, &task), 1);
    assert_eq!(decisions(&engine, &session, &task).len(), 3);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn verification_events_round_trip_through_a_fresh_store() {
    let data_dir = temp_dir("events-round-trip");
    let session = {
        let store = workspace_engine::SessionStore::new(&data_dir);
        let session = store.create_session("repo", "title").unwrap();
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Due {
                    task_id: "task".to_string(),
                    patch_id: "patch".to_string(),
                    paths: Vec::new(),
                    checks: Vec::new(),
                },
            )
            .unwrap();
        session
    };

    let reopened = workspace_engine::SessionStore::new(&data_dir);
    let events = reopened
        .read_verification_events(&session.id, &["task"])
        .expect("the log should read");
    assert_eq!(events.len(), 1);
    assert!(fold_verification(events.iter()).due.is_some());

    let _ = fs::remove_dir_all(&data_dir);
}

// --- The verification turn (Task 4) ---------------------------------------

fn verify_engine(
    root: &std::path::Path,
    data_dir: &std::path::Path,
    shell: &str,
    allow: &[&str],
    block: &[&str],
) -> WorkspaceEngine {
    verify_engine_with_retry(root, data_dir, shell, allow, block, 0)
}

fn verify_engine_with_retry(
    root: &std::path::Path,
    data_dir: &std::path::Path,
    shell: &str,
    allow: &[&str],
    block: &[&str],
    retry_limit: u32,
) -> WorkspaceEngine {
    fs::create_dir_all(root).unwrap();
    WorkspaceEngine::new(Config {
        data_dir: data_dir.to_path_buf(),
        enable_index_watcher: false,
        shell: shell.to_string(),
        command_allowlist: allow.iter().map(|entry| (*entry).to_string()).collect(),
        command_blocklist: block.iter().map(|entry| (*entry).to_string()).collect(),
        agent_tool_retry_limit: retry_limit,
        ..Config::default()
    })
}

fn cargofile(root: &std::path::Path) {
    write(root, "Cargo.toml", "[workspace]\nmembers = []\n");
}

fn try_run_verification(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    session_id: &str,
    adapter: &mut dyn workspace_engine::ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .verify_due(root, session_id, adapter, &mut sink)
}

fn run_verification(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    session_id: &str,
    adapter: &mut dyn workspace_engine::ModelAdapter,
) -> workspace_engine::ChatTurnResult {
    try_run_verification(engine, root, session_id, adapter)
        .expect("the verification turn should run")
}

fn try_continue_repair(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    session_id: &str,
    adapter: &mut dyn workspace_engine::ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .continue_repair(root, session_id, "tester", adapter, &mut sink)
}

fn continue_repair(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    session_id: &str,
    adapter: &mut dyn workspace_engine::ModelAdapter,
) -> ChatTurnResult {
    try_continue_repair(engine, root, session_id, adapter)
        .expect("the explicit repair continuation should run")
}

fn resume_context_review(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    decision: ContextDecision,
    adapter: &mut dyn workspace_engine::ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.resume_after_context_decision(
        proposal_id,
        decision,
        "tester",
        adapter,
        &mut sink,
    )
}

fn resume_checks(
    engine: &WorkspaceEngine,
    batch_id: &str,
    decisions: &[(String, bool)],
    adapter: &mut dyn workspace_engine::ModelAdapter,
) -> workspace_engine::ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_after_check_decision(batch_id, decisions, "tester", adapter, &mut sink)
        .expect("the check batch should resume")
}

fn applied_change(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    files: &[(&str, &str, &str)],
) -> (String, String) {
    applied_change_with_baselines(engine, root, files, true)
}

/// Apply a change and let the real failure-time baseline run (spec 109 D1)
/// instead of writing a fixture value. The fixture becomes a Git repository
/// whose commit is the task-start state, so a failing check reruns on that
/// tree and a failed baseline carries the real findings and line numbers the
/// check reports there.
fn applied_change_keeping_baseline(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    files: &[(&str, &str, &str)],
) -> (String, String) {
    for (path, before, _) in files {
        write(root, path, before);
    }
    for args in [
        &["init", "--quiet"][..],
        &["config", "user.email", "test@damaian.invalid"],
        &["config", "user.name", "Damaian Test"],
        &["config", "commit.gpgsign", "false"],
        &["add", "."],
        &["commit", "--quiet", "-m", "task start"],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }
    let changes: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, _, after)| (*path, *after))
        .collect();
    let proposal = propose(engine, root, &edit_response(&changes));
    engine
        .edit_orchestrator
        .apply_stored_patch(root, &proposal.patch.id, None, None, "tester", false)
        .expect("the patch should apply");
    (proposal.session.id, proposal.task.id)
}

/// The same fixture with a declined baseline for every check. The approval
/// path records the same `Declined` baseline when the user skips the baseline
/// card; this writes it directly so the test stays about Task 3's
/// classification rather than Task 2's card transport.
fn applied_change_with_declined_baseline(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    files: &[(&str, &str, &str)],
) -> (String, String) {
    applied_change_with_baselines(engine, root, files, false)
}

fn applied_change_with_baselines(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    files: &[(&str, &str, &str)],
    pass_baselines: bool,
) -> (String, String) {
    for (path, before, _) in files {
        write(root, path, before);
    }
    let changes: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, _, after)| (*path, *after))
        .collect();
    let proposal = propose(engine, root, &edit_response(&changes));
    // Spec 102 Task 2: these fixtures force check approval to exercise the
    // verification card. Decline the pre-apply baselines here so the patch
    // applies and the later verification pause is the one under test.
    engine
        .edit_orchestrator
        .apply_stored_patch(root, &proposal.patch.id, None, None, "tester", false)
        .expect("the patch should apply");
    // Spec 102 Task 3: these fixtures exercise the repair loop, so the check
    // must have passed before the patch. Task 2's coordinator establishes the
    // baseline through the ordinary policy path; the declined/failed baseline
    // cases have their own tests.
    if pass_baselines {
        mark_baselines_passed(engine, root, &proposal.session.id, &proposal.task.id);
    } else {
        mark_baselines_declined(engine, root, &proposal.session.id, &proposal.task.id);
    }
    (proposal.session.id, proposal.task.id)
}

/// Make the patch task's recorded baselines passing for a fixture that is
/// about post-patch repair rather than baseline acquisition. The latest event
/// for a check wins in the fold, so this is deterministic and still replays as
/// durable evidence.
fn mark_baselines_passed(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    session_id: &str,
    task_id: &str,
) {
    for check in recorded_checks(engine, root, session_id, task_id) {
        engine
            .session_store
            .append_verification_event(
                session_id,
                &VerificationEvent::Baseline {
                    task_id: task_id.to_string(),
                    check,
                    baseline: workspace_engine::CheckBaseline::Passed,
                },
            )
            .expect("the passing baseline should record");
    }
}

/// Record a declined baseline for every check the patch task selected. The
/// ordinary approval path writes the same event when the user skips the card;
/// this fixture writes it directly so the test is about Task 3.
fn mark_baselines_declined(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    session_id: &str,
    task_id: &str,
) {
    for check in recorded_checks(engine, root, session_id, task_id) {
        engine
            .session_store
            .append_verification_event(
                session_id,
                &VerificationEvent::Baseline {
                    task_id: task_id.to_string(),
                    check,
                    baseline: workspace_engine::CheckBaseline::Declined,
                },
            )
            .expect("the declined baseline should record");
    }
}

/// Every check the task's changed files can select, per change or at
/// completion: the floor of every root they touch, a superset of what a
/// verification runs. Spec 109 takes no baseline before a patch, so a fixture
/// that wants one recorded writes it for these checks.
fn recorded_checks(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    session_id: &str,
    task_id: &str,
) -> std::collections::BTreeSet<CheckId> {
    let touched = engine
        .session_store
        .task_touched_paths(session_id, task_id)
        .expect("the touched paths should read");
    let index = workspace_engine::index_cache::IndexCache::get_or_build(&engine.indexer, root)
        .expect("the index should build");
    let (map, _) = workspace_engine::repository_map::RepositoryMapStore::new(
        &engine.config.data_dir,
        engine.audit_log.clone(),
    )
    .load_or_build(&index, &engine.command_policy)
    .expect("the repository map should build");
    workspace_engine::verification::floor_checks(root, &map, &touched)
        .into_iter()
        .collect()
}

/// Marks the session as spec 57's worker, whose worktree is the fixture root.
fn mark_worker(engine: &WorkspaceEngine, session_id: &str, root: &std::path::Path) {
    engine
        .session_store
        .record_worker_spawned(
            session_id,
            &WorkerRecord {
                parent_session_id: None,
                parent_task_id: None,
                inherited_protected_paths: Vec::new(),
                source_repository: root.to_path_buf(),
                worktree_path: root.to_path_buf(),
                branch: "damaian/fixture".to_string(),
                base_commit: "HEAD".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .expect("the worker record is written");
}

fn outcome_events(engine: &WorkspaceEngine, session_id: &str) -> Vec<VerificationEvent> {
    engine
        .session_store
        .read_verification_events(session_id, &[])
        .expect("the log should read")
}

#[test]
fn an_auto_runnable_check_runs_and_the_report_says_passed() {
    let root = temp_dir("verify-pass");
    let data_dir = temp_dir("verify-pass-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert!(result.check_batch.is_none(), "the check was auto-runnable");
    let report = result
        .verification_report
        .as_ref()
        .expect("a report is returned");
    assert_eq!(report.outcome, Some(VerificationOutcome::Passed));
    assert_eq!(
        report.passed,
        vec![CheckId::new("", "cargo test")],
        "{report:?}"
    );
    assert!(
        report.skipped.iter().all(|item| !item.check.is_browser()),
        "a Rust-only change has no browser check at all: {report:?}"
    );
    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(result.response.contains("Verification complete"));

    let plan = engine
        .session_store
        .read_task_plan(&session, &result.task.id)
        .unwrap()
        .expect("the verification task has a plan");
    let step = plan
        .steps
        .iter()
        .find(|step| step.title.contains("cargo test"))
        .expect("a step per check");
    assert_eq!(step.status, StepStatus::Completed);
    assert!(step.evidence.iter().any(|evidence| matches!(
        evidence,
        Evidence::CommandExit {
            exit_code: Some(0),
            purpose: CommandPurpose::Check,
            ..
        }
    )));

    let events = outcome_events(&engine, &session);
    assert!(events.iter().any(|event| matches!(
        event,
        VerificationEvent::Finished {
            outcome: VerificationOutcome::Passed,
            ..
        }
    )));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// Spec 105 §4: a required whole-suite check carried on a plan step must not
/// run after every patch. The deferral keys on the step's structured
/// `PlannedCheck` identity — command and directory — not on a command named in
/// prose. It stays pending for the worker's final plan step, so post-patch
/// verification neither spends the suite nor opens a competing `verify_*` step
/// beside the implementation work. Replay every durable log prefix: the
/// finished plan alone would miss the transient state shown in the UI.
#[test]
fn verification_defers_a_planned_final_check_without_two_steps_in_progress() {
    let root = temp_dir("planned-check-invariant");
    let data_dir = temp_dir("planned-check-invariant-data");
    cargofile(&root);
    let engine = verify_engine(
        &root,
        &data_dir,
        "/usr/bin/true",
        &["cargo test", "cargo check --tests"],
        &[],
    );
    let (session, task_id) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let task = engine
        .session_store
        .read_tasks(&session)
        .unwrap()
        .into_iter()
        .find(|task| task.id == task_id)
        .expect("the patch task");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.extend([
        PlanStep {
            id: "implement".to_string(),
            title: "Implement the change".to_string(),
            detail: None,
            status: StepStatus::InProgress,
            depends_on: Vec::new(),
            started_at_ms: Some(1),
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
            build: None,
        },
        PlanStep {
            id: "test".to_string(),
            title: "Validate the change".to_string(),
            detail: None,
            status: StepStatus::Pending,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: Some(PlannedCheck {
                version: PLANNED_CHECK_VERSION,
                command: "cargo test".to_string(),
                working_directory: String::new(),
                purpose: "validates the final tree".to_string(),
                required: true,
            }),
            build: None,
        },
    ]);
    engine.session_store.create_plan(&task, &plan).unwrap();

    let mut adapter = MockModelAdapter::new("");
    let result = run_verification(&engine, &root, &session, &mut adapter);
    let carried = engine
        .session_store
        .read_task_plan(&session, &result.task.id)
        .unwrap()
        .expect("the verification carries the plan");
    assert_eq!(
        carried
            .steps
            .iter()
            .find(|step| step.id == "test")
            .unwrap()
            .status,
        StepStatus::Pending,
        "the final validation waits for the worker to reach its planned step"
    );
    assert!(
        carried
            .steps
            .iter()
            .all(|step| !step.id.starts_with("verify_")),
        "the planned command is deferred instead of duplicated: {carried:?}"
    );
    let report = result
        .verification_report
        .as_ref()
        .expect("the deferred run still reports that no check ran");
    assert_eq!(report.outcome, Some(VerificationOutcome::NothingVerified));
    assert!(
        report.passed.is_empty(),
        "cargo test was deferred instead of run: {report:?}"
    );
    assert!(
        outcome_events(&engine, &session)
            .iter()
            .all(|event| !matches!(
                event,
                VerificationEvent::Check { check, .. } if check.command == "cargo test"
            )),
        "the deferred suite must not have a check-run event"
    );

    let log =
        fs::read_to_string(data_dir.join("sessions").join(format!("{session}.jsonl"))).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    let replay_session = "replay";
    let replay_path = data_dir
        .join("sessions")
        .join(format!("{replay_session}.jsonl"));
    let store = SessionStore::new(&data_dir);
    let mut checked = 0;
    for end in 1..=lines.len() {
        fs::write(&replay_path, lines[..end].join("\n")).unwrap();
        let Some(prefix) = store
            .read_task_plan(replay_session, &result.task.id)
            .unwrap()
        else {
            continue;
        };
        checked += 1;
        assert!(
            !prefix.violates_single_in_progress(),
            "two steps are in progress after event {end}: {:?}",
            prefix
                .steps
                .iter()
                .map(|step| (&step.id, step.status))
                .collect::<Vec<_>>()
        );
    }
    assert!(checked > 1, "the replay saw the carried plan evolve");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// Spec 105 §4 counter-cases: the deferral matches the *full* identity, so a
/// planned check at a different directory, or with a different command, does
/// not defer the discovered root `cargo test`. Only an exact
/// `CheckId { working_directory, command }` match defers; anything else still
/// runs as immediate repair feedback.
#[test]
fn a_planned_check_with_a_different_directory_or_command_is_not_deferred() {
    for planned in [
        PlannedCheck {
            version: PLANNED_CHECK_VERSION,
            command: "cargo test".to_string(),
            working_directory: "crates/engine".to_string(),
            purpose: "same command, different directory".to_string(),
            required: true,
        },
        PlannedCheck {
            version: PLANNED_CHECK_VERSION,
            command: "cargo nextest run".to_string(),
            working_directory: String::new(),
            purpose: "different command, same directory".to_string(),
            required: true,
        },
        PlannedCheck {
            version: PLANNED_CHECK_VERSION,
            command: "cargo test".to_string(),
            working_directory: String::new(),
            purpose: "same identity, but optional".to_string(),
            required: false,
        },
    ] {
        let root = temp_dir("planned-check-counter");
        let data_dir = temp_dir("planned-check-counter-data");
        cargofile(&root);
        let engine = verify_engine(
            &root,
            &data_dir,
            "/usr/bin/true",
            &["cargo test", "cargo check --tests"],
            &[],
        );
        let (session, task_id) = applied_change(
            &engine,
            &root,
            &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
        );
        let task = engine
            .session_store
            .read_tasks(&session)
            .unwrap()
            .into_iter()
            .find(|task| task.id == task_id)
            .expect("the patch task");
        let mut plan = TaskPlan::new(&task.id, 0);
        plan.steps.extend([
            PlanStep {
                id: "implement".to_string(),
                title: "Implement the change".to_string(),
                detail: None,
                status: StepStatus::InProgress,
                depends_on: Vec::new(),
                started_at_ms: Some(1),
                completed_at_ms: None,
                evidence: Vec::new(),
                check: None,
                build: None,
            },
            PlanStep {
                id: "test".to_string(),
                title: "Validate the change".to_string(),
                detail: None,
                status: StepStatus::Pending,
                depends_on: Vec::new(),
                started_at_ms: None,
                completed_at_ms: None,
                evidence: Vec::new(),
                check: Some(planned),
                build: None,
            },
        ]);
        engine.session_store.create_plan(&task, &plan).unwrap();

        let mut adapter = MockModelAdapter::new("");
        let result = run_verification(&engine, &root, &session, &mut adapter);
        let report = result
            .verification_report
            .as_ref()
            .expect("the root cargo test still runs rather than defers");
        assert_eq!(
            report.passed,
            vec![CheckId::new("", "cargo test")],
            "a check at a different identity must not be deferred: {report:?}"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&data_dir);
    }
}

/// Spec 105 §2: a structured check is authoritative only once the user approves
/// the plan. The deferral reads `read_task_plan`, which replays the settled
/// `plan_approved` plan, so an approval that removes a proposed check makes the
/// deferral stop deferring it — the check runs as immediate feedback. The
/// proposed plan (`plan_created`, what a fresh `propose_plan` call writes) is
/// not what drives the deferral.
#[test]
fn the_deferral_follows_the_settled_plan_not_the_proposed_plan() {
    let root = temp_dir("planned-check-approval");
    let data_dir = temp_dir("planned-check-approval-data");
    cargofile(&root);
    let engine = verify_engine(
        &root,
        &data_dir,
        "/usr/bin/true",
        &["cargo test", "cargo check --tests"],
        &[],
    );
    let (session, task_id) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let task = engine
        .session_store
        .read_tasks(&session)
        .unwrap()
        .into_iter()
        .find(|task| task.id == task_id)
        .expect("the patch task");

    // The model proposes a plan whose final step requires `cargo test`.
    let required = PlannedCheck {
        version: PLANNED_CHECK_VERSION,
        command: "cargo test".to_string(),
        working_directory: String::new(),
        purpose: "validates the final tree".to_string(),
        required: true,
    };
    let mut proposed = TaskPlan::new(&task.id, 0);
    proposed.steps.extend([
        PlanStep {
            id: "implement".to_string(),
            title: "Implement the change".to_string(),
            detail: None,
            status: StepStatus::InProgress,
            depends_on: Vec::new(),
            started_at_ms: Some(1),
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
            build: None,
        },
        PlanStep {
            id: "test".to_string(),
            title: "Validate the change".to_string(),
            detail: None,
            status: StepStatus::Pending,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: Some(required),
            build: None,
        },
    ]);
    engine.session_store.create_plan(&task, &proposed).unwrap();

    // The user approves a settled plan that removes the check from the final
    // step. The proposal still names it, but the settled plan does not.
    let mut approved = proposed;
    approved.steps[1].check = None;
    engine
        .session_store
        .approve_plan_with_budget_and_plan(
            &task,
            "tester",
            &TaskExecutionBudget::try_new(100, 100, 1_000_000, 100_000).unwrap(),
            &approved,
        )
        .unwrap();

    // Verification reads the settled plan, so the removed check is not a
    // deferral target and runs now as immediate feedback.
    let mut adapter = MockModelAdapter::new("");
    let result = run_verification(&engine, &root, &session, &mut adapter);
    let report = result
        .verification_report
        .as_ref()
        .expect("the removed check still runs rather than defers");
    assert_eq!(
        report.passed,
        vec![CheckId::new("", "cargo test")],
        "the deferral must follow the approved plan, not the proposal: {report:?}"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn two_checks_needing_approval_produce_one_batch_and_one_resume() {
    let root = temp_dir("verify-batch");
    let data_dir = temp_dir("verify-batch-data");
    cargofile(&root);
    write(
        &root,
        "packages/web/package.json",
        "{\"scripts\":{\"test\":\"node test.js\"}}\n",
    );
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[
            ("src/a.rs", "let a = 1;\n", "let a = 2;\n"),
            (
                "packages/web/src/index.ts",
                "export const x = 1;\n",
                "export const x = 2;\n",
            ),
        ],
    );
    let mut adapter = MockModelAdapter::new("");

    let paused = run_verification(&engine, &root, &session, &mut adapter);
    let batch = paused
        .check_batch
        .as_ref()
        .expect("both checks need approval");
    assert_eq!(batch.checks.len(), 2, "{batch:?}");
    assert!(
        paused.command_proposal.is_none(),
        "one card, not one per check"
    );
    assert!(paused.verification_report.is_none());
    assert_eq!(paused.task.status, TaskStatus::WaitingForApproval);
    // Both entries carry the policy's cause, not a blank.
    assert!(batch.checks.iter().all(|check| !check.cause.is_empty()));

    let root_check = batch
        .checks
        .iter()
        .find(|check| check.command == "cargo test")
        .expect("the root's cargo test is on the card");
    let web_check = batch
        .checks
        .iter()
        .find(|check| check.command == "npm run test")
        .expect("the web package's test is on the card");
    let decisions = vec![(root_check.id.clone(), true), (web_check.id.clone(), false)];
    let resumed = resume_checks(&engine, &batch.id, &decisions, &mut adapter);

    let report = resumed
        .verification_report
        .as_ref()
        .expect("the resumed turn returns the report");
    assert_eq!(report.outcome, Some(VerificationOutcome::Passed));
    assert_eq!(report.passed, vec![CheckId::new("", "cargo test")]);
    let declined = report
        .skipped
        .iter()
        .find(|item| item.check.command == "npm run test")
        .expect("the declined check is reported as skipped");
    assert_eq!(declined.reason, SkipReason::DeclinedByUser);
    // Spec 101/105 as amended by review fix R15: the discovered checks of the
    // roots the task touched are the completion floor. The declined
    // `npm run test` is therefore a named waiver, not simply an ignored
    // discovered check. The browser check is not a root-detected command, so it
    // stays a verification report fact rather than a floor obligation.
    let settled = engine
        .session_store
        .read_completion_report(&session, &resumed.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert_eq!(resumed.task.status, TaskStatus::Complete);
    assert!(settled.eligible, "{settled:?}");
    assert!(settled.reasons.is_empty());
    assert_eq!(settled.waivers.len(), 1, "{settled:?}");
    assert_eq!(settled.waivers[0].check, declined.check.clone());

    let plan = engine
        .session_store
        .read_task_plan(&session, &resumed.task.id)
        .unwrap()
        .expect("the plan survives the pause");
    let web_step = plan
        .steps
        .iter()
        .find(|step| step.title.contains("npm run test"))
        .expect("the declined check has a step");
    assert_eq!(web_step.status, StepStatus::Skipped);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_blocked_check_is_skipped_and_never_runs() {
    let root = temp_dir("verify-blocked");
    let data_dir = temp_dir("verify-blocked-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &["cargo test"]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert!(
        result.check_batch.is_none(),
        "a blocked check never reaches the card"
    );
    let report = result.verification_report.as_ref().unwrap();
    assert!(report.passed.is_empty());
    let skipped = report
        .skipped
        .iter()
        .find(|item| item.check.command == "cargo test")
        .expect("the blocked check is reported as skipped");
    assert!(matches!(skipped.reason, SkipReason::Blocked { .. }));

    let plan = engine
        .session_store
        .read_task_plan(&session, &result.task.id)
        .unwrap()
        .unwrap();
    let step = plan
        .steps
        .iter()
        .find(|step| step.title.contains("cargo test"))
        .unwrap();
    assert_eq!(step.status, StepStatus::Skipped);
    assert!(step.evidence.is_empty(), "a blocked check observed nothing");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn require_approval_for_all_commands_puts_an_allowlisted_check_on_the_card() {
    let root = temp_dir("verify-all-approval");
    let data_dir = temp_dir("verify-all-approval-data");
    cargofile(&root);
    fs::create_dir_all(&root).unwrap();
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        command_allowlist: vec!["cargo test".to_string()],
        require_approval_for_all_commands: true,
        ..Config::default()
    });
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let batch = result
        .check_batch
        .as_ref()
        .expect("the flag puts even an allowlisted check on the card");
    assert_eq!(batch.checks.len(), 1);
    assert_eq!(batch.checks[0].command, "cargo test");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn verify_due_refuses_a_session_that_is_no_longer_code() {
    let root = temp_dir("verify-ask");
    let data_dir = temp_dir("verify-ask-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    engine
        .session_store
        .set_session_mode(&session, workspace_engine::SessionMode::Ask, "user")
        .unwrap();
    let before = outcome_events(&engine, &session).len();
    let mut adapter = MockModelAdapter::new("");
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };

    let error = engine
        .chat_orchestrator
        .verify_due(&root, &session, &mut adapter, &mut sink)
        .expect_err("Ask mode must refuse verification");
    assert!(matches!(
        error,
        workspace_engine::ClientError::AccessDenied(_)
    ));
    assert_eq!(
        outcome_events(&engine, &session).len(),
        before,
        "nothing ran"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_failing_check_blocks_its_step_and_fails_the_report() {
    let root = temp_dir("verify-fail");
    let data_dir = temp_dir("verify-fail-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert_eq!(
        report.outcome,
        Some(VerificationOutcome::RepairLimitExhausted)
    );
    assert!(report.passed.is_empty());
    assert_eq!(
        report.failed[0].reason, "exit code 1",
        "the exit code is reported, not laundered"
    );
    assert_eq!(result.task.status, TaskStatus::Failed);
    let plan = engine
        .session_store
        .read_task_plan(&session, &result.task.id)
        .unwrap()
        .unwrap();
    let step = plan
        .steps
        .iter()
        .find(|step| step.title.contains("cargo test"))
        .unwrap();
    assert_eq!(step.status, StepStatus::Blocked);
    assert!(step.evidence.iter().any(|evidence| matches!(
        evidence,
        Evidence::CommandExit {
            exit_code: Some(1),
            purpose: CommandPurpose::Check,
            ..
        }
    )));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_check_batch_cannot_be_resumed_through_the_command_route() {
    let root = temp_dir("verify-wrong-route");
    let data_dir = temp_dir("verify-wrong-route-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");
    let paused = run_verification(&engine, &root, &session, &mut adapter);
    let batch = paused.check_batch.as_ref().unwrap();

    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let error = engine
        .chat_orchestrator
        .resume_after_command_decision(&batch.id, true, "tester", &mut adapter, &mut sink)
        .expect_err("the command route must refuse a check batch");
    assert!(
        error.to_string().contains("resume_after_check_decision"),
        "{error}"
    );

    // The refusal put the pause back, so the right route still works.
    let resumed = resume_checks(
        &engine,
        &batch.id,
        &batch
            .checks
            .iter()
            .map(|check| (check.id.clone(), true))
            .collect::<Vec<_>>(),
        &mut adapter,
    );
    assert_eq!(
        resumed.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::Passed)
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_command_pause_cannot_be_resumed_through_the_check_route() {
    let root = temp_dir("verify-wrong-route-reverse");
    let data_dir = temp_dir("verify-wrong-route-reverse-data");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    let mut adapter = MockModelAdapter::new_sequence(vec![
        "DAMAIAN_COMMAND_V1\nCOMMAND: sed -i '' 's/1/2/' src/a.rs\nREASON: Bump the constant.\nEND_COMMAND\n"
            .to_string(),
        "Done.".to_string(),
    ]);
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let turn = engine
        .chat_orchestrator
        .ask_with_session(&root, "Bump it.", &[], None, &mut adapter, &mut sink)
        .expect("the turn should pause for approval");
    let proposal = turn.command_proposal.as_ref().expect("a command pause");

    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let error = engine
        .chat_orchestrator
        .resume_after_check_decision(&proposal.id, &[], "tester", &mut adapter, &mut sink)
        .expect_err("the check route must refuse a command pause");
    assert!(
        error.to_string().contains("resume_after_command_decision"),
        "{error}"
    );

    let resumed = engine
        .chat_orchestrator
        .resume_after_command_decision(&proposal.id, true, "tester", &mut adapter, &mut sink)
        .expect("the command pause survives and resumes");
    assert_eq!(resumed.task.status, TaskStatus::Complete);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn verify_due_with_no_detected_checks_says_so() {
    let root = temp_dir("verify-no-checks");
    let data_dir = temp_dir("verify-no-checks-data");
    // No manifest anywhere, so the selected root detects nothing.
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert_eq!(report.outcome, Some(VerificationOutcome::NoChecksAvailable));
    assert!(report.passed.is_empty());
    assert!(result.response.contains("No checks were available"));
    assert!(!result.response.contains("Verification complete"));
    assert_eq!(result.task.status, TaskStatus::Complete);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_documentation_only_patch_verifies_nothing_and_says_so() {
    let root = temp_dir("verify-docs");
    let data_dir = temp_dir("verify-docs-data");
    cargofile(&root);
    write(&root, "docs/guide.md", "# Guide\n");
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("docs/guide.md", "# Guide\n", "# Guide, revised\n")],
    );
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert_eq!(
        report.outcome,
        Some(VerificationOutcome::OnlyDocumentationChanged)
    );
    assert!(report.passed.is_empty());
    assert!(result.response.contains("only documentation changed"));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn an_after_check_hook_sees_the_verification_run() {
    let root = temp_dir("verify-after-check");
    let data_dir = temp_dir("verify-after-check-data");
    cargofile(&root);
    let envelope = root.join("after-check.json");
    let hook_script = root.join("hooks").join("capture.sh");
    fs::create_dir_all(hook_script.parent().unwrap()).unwrap();
    fs::write(
        &hook_script,
        format!(
            "cat > {}\necho '{{\"hookApiVersion\":1,\"verdict\":\"allow\"}}'\n",
            envelope.display()
        ),
    )
    .unwrap();
    let hook = HookConfig {
        id: "capture_check".to_string(),
        command: "/bin/sh".to_string(),
        args: vec![hook_script.to_string_lossy().to_string()],
        events: vec![HookEvent::AfterCheck],
        mode: HookMode::Mandatory,
        order: 0,
        timeout_ms: Some(5_000),
        enabled: true,
        source: HookSource::User,
    };
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/false".to_string(),
        command_allowlist: vec!["cargo test".to_string()],
        agent_tool_retry_limit: 0,
        hooks: vec![hook],
        ..Config::default()
    });
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(
        result.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::RepairLimitExhausted)
    );

    let captured: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&envelope).expect("the hook wrote its envelope"))
            .unwrap();
    assert_eq!(captured["event"], "after_check");
    assert_eq!(captured["payload"]["command"], "cargo test");
    assert_eq!(captured["payload"]["exitCode"], 1);
    assert!(
        captured["payload"]["findingIds"].is_array(),
        "the payload carries the finding ids field: {captured}"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// --- Repair, rerun and the limit (Task 5) ----------------------------------

fn patch_call(path: &str, content: &str) -> workspace_engine::ToolCall {
    workspace_engine::ToolCall {
        id: "repair_patch".to_string(),
        name: "propose_patch".to_string(),
        arguments_json: serde_json::json!({
            "summary": "Fix the failing check",
            "files": [{"path": path, "content": content, "status": "modified"}],
        })
        .to_string(),
    }
}

fn repaired_shell(root: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("bin").join("check-shell.sh");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!(
            "#!/bin/sh\ncase \"$2\" in\n  *\"npm run test\"*)\n    if [ -f \"{root}/web-fail\" ]; then\n      echo 'error[E0308]: mismatched types' >&2\n      echo ' --> packages/web/src/index.ts:1:1' >&2\n      exit 1\n    fi\n    exit 0\n    ;;\n  *)\n    if [ -f \"{root}/cargo-fail\" ]; then\n      echo 'error[E0425]: cannot find value x in this scope' >&2\n      echo ' --> src/a.rs:1:1' >&2\n      exit 1\n    fi\n    exit 0\n    ;;\nesac\n",
            root = root.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

/// A `cargo test` shell whose whole stdout and stderr are fixed files. The
/// repair-request tests need the exact bytes a parser reads, without relying
/// on how a compiler or test runner formats them today.
fn scripted_cargo_shell(
    root: &std::path::Path,
    data_dir: &std::path::Path,
    stdout: &str,
    stderr: &str,
) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("bin").join("scripted-cargo-shell.sh");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Keep the captured streams outside the repository. Context assembly
    // retrieves repository files, and a fixture's raw output must not reach the
    // request except through the finding under test.
    let stdout_path = data_dir.join("scripted-stdout.txt");
    let stderr_path = data_dir.join("scripted-stderr.txt");
    fs::write(&stdout_path, stdout).unwrap();
    fs::write(&stderr_path, stderr).unwrap();
    fs::write(
        &path,
        format!(
            "#!/bin/sh\ncase \"$2\" in\n  *\"cargo test\"*)\n    cat '{}'\n    cat '{}' >&2\n    exit 1\n    ;;\n  *)\n    exit 0\n    ;;\nesac\n",
            stdout_path.display(),
            stderr_path.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path.to_string_lossy().to_string()
}

fn started_attempts(events: &[VerificationEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|event| match event {
            VerificationEvent::Started { attempt, .. } => Some(*attempt),
            _ => None,
        })
        .collect()
}

#[test]
fn a_failed_check_is_repaired_and_the_rerun_passes() {
    let root = temp_dir("repair-pass");
    let data_dir = temp_dir("repair-pass-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );

    let first = run_verification(&engine, &root, &session, &mut repair);
    let patch = first
        .patch_proposal
        .as_ref()
        .expect("a failing check asks the model for a repair patch");
    assert!(
        first.verification_report.is_none(),
        "the report waits for the rerun"
    );
    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect("the repair patch applies");
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 3;\n"
    );

    // A new orchestrator over the same data dir proves the attempt count comes
    // from the log, not from memory, and flips the check's result.
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[], 2);
    let mut adapter = MockModelAdapter::new("");
    let second = run_verification(&engine, &root, &session, &mut adapter);

    let report = second.verification_report.as_ref().unwrap();
    assert_eq!(report.outcome, Some(VerificationOutcome::Passed));
    assert_eq!(report.passed, vec![CheckId::new("", "cargo test")]);
    assert_eq!(second.task.status, TaskStatus::Complete);
    let events = outcome_events(&engine, &session);
    assert_eq!(started_attempts(&events), vec![0, 1], "{events:?}");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// Spec 92: the rerun reuses the step of a check it already has and adds one
/// only for a check that is new, so the root's pass lands on its one step and
/// nothing selected is left open after a passed run.
#[test]
fn a_rerun_reuses_the_step_of_a_check_it_already_has() {
    let root = temp_dir("repair-steps");
    let data_dir = temp_dir("repair-steps-data");
    cargofile(&root);
    write(
        &root,
        "packages/web/package.json",
        "{\"scripts\":{\"test\":\"node test.js\"}}\n",
    );
    write(&root, "packages/web/src/index.ts", "export const x = 1;\n");
    let allow = ["cargo test", "cd packages/web && npm run test"];
    let engine = verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &allow, &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    // The repair touches the web package too, so the rerun selects a check
    // attempt 0 did not have alongside the one it did.
    let repair_call = workspace_engine::ToolCall {
        id: "repair_patch".to_string(),
        name: "propose_patch".to_string(),
        arguments_json: json!({
            "summary": "Fix the failing check",
            "files": [
                {"path": "src/a.rs", "content": "let a = 3;\n", "status": "modified"},
                {
                    "path": "packages/web/src/index.ts",
                    "content": "export const x = 2;\n",
                    "status": "modified",
                },
            ],
        })
        .to_string(),
    };
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![repair_call]],
    );
    let first = run_verification(&engine, &root, &session, &mut repair);
    let patch = first.patch_proposal.as_ref().expect("a repair patch");
    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect("the repair patch applies");

    let engine = verify_engine_with_retry(&root, &data_dir, "/usr/bin/true", &allow, &[], 2);
    let mut adapter = MockModelAdapter::new("");
    let second = run_verification(&engine, &root, &session, &mut adapter);
    let report = second.verification_report.as_ref().unwrap();
    assert_eq!(
        report.outcome,
        Some(VerificationOutcome::Passed),
        "{report:?}"
    );
    let events = outcome_events(&engine, &session);
    let rerun_checks = events
        .iter()
        .filter_map(|event| match event {
            VerificationEvent::Started {
                attempt: 1, checks, ..
            } => Some(checks.clone()),
            _ => None,
        })
        .next()
        .expect("attempt 1 started")
        .into_iter()
        .filter(|check| !check.is_browser())
        .collect::<Vec<_>>();
    assert_eq!(
        rerun_checks,
        vec![
            CheckId::new("", "cargo test"),
            CheckId::new("packages/web", "npm run test"),
        ]
    );

    let plan = engine
        .session_store
        .read_task_plan(&session, &second.task.id)
        .unwrap()
        .expect("the verification task has a plan");
    let verify_steps: Vec<&PlanStep> = plan
        .steps
        .iter()
        .filter(|step| step.id.starts_with("verify_") && step.id != "verify_browser")
        .collect();
    let mut ids: Vec<&str> = verify_steps.iter().map(|step| step.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        verify_steps.len(),
        "duplicate step ids: {plan:?}"
    );
    for check in &rerun_checks {
        let steps: Vec<&&PlanStep> = verify_steps
            .iter()
            .filter(|step| {
                step.detail.as_deref() == Some(&*format!("Verification check: {}", check.label()))
            })
            .collect();
        assert_eq!(steps.len(), 1, "one step for {}: {plan:?}", check.label());
        assert_eq!(steps[0].status, StepStatus::Completed, "{plan:?}");
    }
    assert_eq!(verify_steps.len(), 2, "{plan:?}");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn exhausting_the_repair_limit_reports_what_still_fails() {
    let root = temp_dir("repair-limit");
    let data_dir = temp_dir("repair-limit-data");
    cargofile(&root);
    // The failure fingerprint must progress on each attempt: a repeated
    // fingerprint is a stall (spec 102 Task 4), and this fixture is about the
    // numeric limit remaining the fallback while repair makes progress.
    let shell = progress_shell(&root).to_string_lossy().to_string();
    let mut session = String::new();
    for attempt in 0..2 {
        let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 2);
        if attempt == 0 {
            let (created, _task) = applied_change(
                &engine,
                &root,
                &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
            );
            session = created;
        }
        // Each attempt must make a different change: an equivalent patch is a
        // stall (spec 102 Task 4), and this fixture is about the numeric limit.
        let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new()],
            vec![vec![patch_call(
                "src/a.rs",
                &format!("let a = {};\n", attempt + 3),
            )]],
        );
        let turn = run_verification(&engine, &root, &session, &mut repair);
        let patch = turn
            .patch_proposal
            .as_ref()
            .expect("each attempt below the limit asks for a repair");
        engine
            .edit_orchestrator
            .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
            .expect("the repair patch applies");
    }

    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 2);
    let mut adapter = MockModelAdapter::new("");
    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert_eq!(
        report.outcome,
        Some(VerificationOutcome::RepairLimitExhausted)
    );
    assert!(
        report.repair_stall.is_none(),
        "progressing repair is stopped by the count, not a stall"
    );
    assert_eq!(result.task.status, TaskStatus::Failed);
    assert!(!result.response.contains("complete"));
    assert!(
        result.response.contains("repair limit was exhausted"),
        "{}",
        result.response
    );
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0, 1, 2]
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// Spec 102 Task 4: an ineffective repair stops on the second identical result
/// even though the configured limit is sixteen. The redundant patch is not
/// exposed, another attempt is not spent, and the typed stop is durable.
#[test]
fn a_repeated_ineffective_repair_stalls_before_the_numeric_limit() {
    let root = temp_dir("repair-stall-repeat");
    let data_dir = temp_dir("repair-stall-repeat-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 16);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut first_repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let first = run_verification(&engine, &root, &session, &mut first_repair);
    let patch = first
        .patch_proposal
        .as_ref()
        .expect("the first attempt is progress and asks for a repair");
    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect("the first repair applies");

    // The check still fails the same way, and the model repeats the same
    // repair. That is a stall, not one of the remaining fourteen attempts.
    let mut repeated = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let stalled = run_verification(&engine, &root, &session, &mut repeated);

    assert!(
        stalled.patch_proposal.is_none(),
        "a stalled repair must not expose its redundant patch"
    );
    let report = stalled
        .verification_report
        .as_ref()
        .expect("the stall is a terminal report");
    assert_eq!(report.outcome, Some(VerificationOutcome::RepairStalled));
    assert_eq!(
        report.repair_stall.as_ref().map(|stall| stall.reason),
        Some(RepairStallReason::RepeatedFailure)
    );
    assert!(
        report.repair_targets.is_empty(),
        "no target may reach a repair turn after a stall"
    );
    assert_eq!(report.repair_request(), None);
    assert_eq!(stalled.task.status, TaskStatus::Failed);
    assert!(
        stalled.response.contains("Repair stalled"),
        "{}",
        stalled.response
    );
    assert!(!stalled.response.contains("complete"));
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0, 1],
        "a stall does not spend another automatic attempt"
    );
    assert_eq!(
        engine
            .session_store
            .task_verification_repair_history(&session, &stalled.task.id)
            .unwrap()
            .len(),
        2,
        "both attempts, including the stalled one, are in the durable window"
    );

    // The same typed stop survives a new store, and it is not the numeric
    // limit's explicit continuation.
    let reopened =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 16);
    let events = outcome_events(&reopened, &session);
    let state = fold_verification(events.iter());
    assert_eq!(state.finished, Some(VerificationOutcome::RepairStalled));
    assert_eq!(
        state.repair_stall.as_ref().map(|stall| stall.reason),
        Some(RepairStallReason::RepeatedFailure)
    );
    let mut refused_adapter = MockModelAdapter::new("");
    let refused = try_continue_repair(&reopened, &root, &session, &mut refused_adapter)
        .expect_err("a stall does not enable continue repair");
    assert!(refused.to_string().contains("repair limit"), "{refused}");
    assert!(refused_adapter.requests.is_empty());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// R5 (spec 102): an ineffective repair that only moves the failing line is
/// still the same failure. The second attempt stops as `repeated_failure`
/// instead of spending the remaining attempts because the fingerprint used to
/// include the location.
#[test]
fn a_repair_that_only_moves_the_failing_line_stalls_as_repeated_failure() {
    let root = temp_dir("repair-stall-shift");
    let data_dir = temp_dir("repair-stall-shift-data");
    cargofile(&root);
    let shell = shifting_line_shell(&root);
    let engine = verify_engine_with_retry(
        &root,
        &data_dir,
        &shell.to_string_lossy(),
        &["cargo test"],
        &[],
        16,
    );
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 1;\nbroken();\n")],
    );

    // Attempt 0 fails at line 2 and proposes a repair that inserts a line
    // above the failure, moving it to line 3.
    let mut first_repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call(
            "src/a.rs",
            "// pad\nlet a = 1;\nbroken();\n",
        )]],
    );
    let first = run_verification(&engine, &root, &session, &mut first_repair);
    let patch = first
        .patch_proposal
        .as_ref()
        .expect("the first attempt is progress and asks for a repair");
    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect("the first repair applies");

    // Attempt 1 fails at line 3 with the same error, and the model proposes
    // another patch that only moves the line again. That is a repeated
    // failure, not progress.
    let mut repeated = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call(
            "src/a.rs",
            "// pad\n// pad\nlet a = 1;\nbroken();\n",
        )]],
    );
    let stalled = run_verification(&engine, &root, &session, &mut repeated);

    assert!(
        stalled.patch_proposal.is_none(),
        "a stalled repair must not expose its line-moving patch"
    );
    let report = stalled
        .verification_report
        .as_ref()
        .expect("the stall is a terminal report");
    assert_eq!(report.outcome, Some(VerificationOutcome::RepairStalled));
    assert_eq!(
        report.repair_stall.as_ref().map(|stall| stall.reason),
        Some(RepairStallReason::RepeatedFailure)
    );
    assert!(report.repair_targets.is_empty());
    assert_eq!(report.repair_request(), None);
    assert_eq!(stalled.task.status, TaskStatus::Failed);
    assert!(
        stalled.response.contains("Repair stalled"),
        "{}",
        stalled.response
    );
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0, 1],
        "a line shift is not a new failure and does not spend the numeric limit"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_worker_stops_on_the_same_typed_repair_stall() {
    let root = temp_dir("repair-stall-worker");
    let data_dir = temp_dir("repair-stall-worker-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 16);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    mark_worker(&engine, &session, &root);
    let mut first_repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let first = run_verification(&engine, &root, &session, &mut first_repair);
    let patch = first.patch_proposal.as_ref().expect("a repair patch");
    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect("the repair applies");

    let mut repeated = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let stalled = run_verification(&engine, &root, &session, &mut repeated);
    assert!(stalled.patch_proposal.is_none());
    assert_eq!(
        stalled.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::RepairStalled)
    );
    assert_eq!(stalled.task.status, TaskStatus::Failed);
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0, 1],
        "a worker does not start another automatic repair after a stall"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// Spec 102 Task 4 and acceptance: an A → B → A fingerprint/diff oscillation
/// stops with `repair_stalled`, even with a much larger numeric limit.
#[test]
fn an_a_to_b_to_a_repair_oscillation_stalls() {
    let root = temp_dir("repair-stall-oscillation");
    let data_dir = temp_dir("repair-stall-oscillation-data");
    cargofile(&root);
    let shell = content_shell(&root);
    let engine = verify_engine_with_retry(
        &root,
        &data_dir,
        &shell.to_string_lossy(),
        &["cargo test"],
        &[],
        16,
    );
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 0;\n", "let a = 2;\n")],
    );

    // Attempt 0 sees failure B and proposes A.
    let mut first = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 1;\n")]],
    );
    let first = run_verification(&engine, &root, &session, &mut first);
    let patch = first.patch_proposal.as_ref().expect("a repair patch");
    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect("the repair applies");

    // Attempt 1 sees failure A and proposes B.
    let mut second = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 2;\n")]],
    );
    let second = run_verification(&engine, &root, &session, &mut second);
    let patch = second.patch_proposal.as_ref().expect("a repair patch");
    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect("the repair applies");

    // Attempt 2 sees failure B again with B's net diff. The patch is new, so
    // only the window-level fingerprint/diff pair repeat is the stop.
    let mut third = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let stalled = run_verification(&engine, &root, &session, &mut third);

    assert!(stalled.patch_proposal.is_none());
    let report = stalled.verification_report.as_ref().unwrap();
    assert_eq!(report.outcome, Some(VerificationOutcome::RepairStalled));
    assert_eq!(
        report.repair_stall.as_ref().map(|stall| stall.reason),
        Some(RepairStallReason::Oscillation)
    );
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0, 1, 2],
        "the limit was sixteen; the oscillation stopped at three attempts"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 2;\n",
        "the redundant third patch was not applied"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A shell whose failure summary includes the current content of `src/a.rs`,
/// so every distinct repair changes the fingerprint. This is the progressing
/// case: only the numeric limit should stop it.
fn progress_shell(root: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("bin").join("progress-check.sh");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "#!/bin/sh\nvalue=$(cat src/a.rs 2>/dev/null | tr -d '\\n')\necho \"error: value $value is wrong\" >&2\necho ' --> src/a.rs:1:1' >&2\nexit 1\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

/// A shell that emits the same compiler error from whatever line holds
/// `broken()` in `src/a.rs`, and passes when there is no such line. Inserting
/// a line above the failure moves the reported location without changing the
/// error's identity.
fn shifting_line_shell(root: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("bin").join("shifting-line-check.sh");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "#!/bin/sh\nline=$(grep -n 'broken()' src/a.rs 2>/dev/null | head -n 1 | cut -d: -f1)\nif [ -z \"$line\" ]; then\n  exit 0\nfi\necho 'error[E0308]: mismatched types' >&2\necho \" --> src/a.rs:${line}:1\" >&2\nexit 1\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

/// A shell whose failure fingerprint depends on the current content of
/// `src/a.rs`, so the repair loop can produce distinct A and B results.
fn content_shell(root: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("bin").join("content-check.sh");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "#!/bin/sh\ncontent=$(cat src/a.rs 2>/dev/null)\ncase \"$content\" in\n  *\"let a = 1;\"*)\n    echo 'error[E0001]: alpha failure' >&2\n    echo ' --> src/a.rs:1:1' >&2\n    ;;\n  *\"let a = 2;\"*)\n    echo 'error[E0002]: beta failure' >&2\n    echo ' --> src/a.rs:1:1' >&2\n    ;;\n  *)\n    echo 'error[E0003]: gamma failure' >&2\n    echo ' --> src/a.rs:1:1' >&2\n    ;;\nesac\nexit 1\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

/// Spec 96: at the limit every session stops, and the user's explicit
/// continuation starts exactly one repair turn. Its plan carries the failing
/// exit and findings, its patch is only a proposal, and applying it leads to
/// the next verification attempt.
#[test]
fn an_explicit_continue_runs_one_more_repair_after_the_limit() {
    let root = temp_dir("limited-continue");
    let data_dir = temp_dir("limited-continue-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 0);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");
    let limited = run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(
        limited.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::RepairLimitExhausted)
    );
    assert_eq!(limited.task.status, TaskStatus::Failed);
    assert!(
        adapter.requests.is_empty(),
        "the limit itself starts no repair turn"
    );
    let limited_task = limited.task.id.clone();

    // A fresh orchestrator over the same log proves the action reads durable
    // state, not a turn left in memory.
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 0);
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let continued = continue_repair(&engine, &root, &session, &mut repair);

    assert_eq!(repair.requests.len(), 1, "one explicit action, one attempt");
    let patch = continued
        .patch_proposal
        .as_ref()
        .expect("the repair turn asks for a patch");
    assert_ne!(
        continued.task.id, limited_task,
        "the explicit attempt is a new task carrying the plan"
    );
    assert!(
        continued.verification_report.is_none(),
        "the report waits for the repaired check to rerun"
    );
    assert_ne!(
        continued.task.status,
        TaskStatus::Failed,
        "the repair task stays alive for the patch review"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 2;\n",
        "a proposed repair patch changes nothing until it is approved"
    );

    // The failed check's evidence is still in the plan the repair turn carried.
    let plan = engine
        .session_store
        .read_task_plan(&session, &continued.task.id)
        .unwrap()
        .expect("the continuation carries the plan");
    let verify = plan
        .steps
        .iter()
        .find(|step| step.id == "verify_1")
        .expect("the failing check has its step");
    assert!(
        verify.evidence.iter().any(|evidence| matches!(
            evidence,
            Evidence::CommandExit {
                exit_code: Some(code),
                purpose: CommandPurpose::Check,
                ..
            } if *code != 0
        )),
        "the failing exit is preserved: {verify:?}"
    );
    assert!(
        verify
            .evidence
            .iter()
            .any(|evidence| matches!(evidence, Evidence::Findings { .. })),
        "the findings are preserved: {verify:?}"
    );
    // The request is the normalized introduced evidence, not raw check output.
    let prompt = first_user_message(&repair);
    assert!(
        prompt.contains("Repair only the failures this patch introduced"),
        "the repair request names only introduced failures: {prompt}"
    );
    assert!(
        prompt.contains("cargo test (repository root)"),
        "the repair request names the check: {prompt}"
    );
    assert!(
        prompt.contains("cargo test exited with code 1"),
        "the repair request carries the normalized finding: {prompt}"
    );

    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect("the repair patch applies");
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 3;\n"
    );

    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[], 0);
    let mut adapter = MockModelAdapter::new("");
    let rerun = run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(
        rerun.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::Passed)
    );
    assert_eq!(rerun.task.status, TaskStatus::Complete);
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0, 1],
        "the continuation became the next verification attempt"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A worker gets the same stop and the same explicit action: no repair turn
/// until the user asks for one, and then exactly one.
#[test]
fn a_worker_at_the_limit_waits_for_an_explicit_continue() {
    let root = temp_dir("worker-limit-continue");
    let data_dir = temp_dir("worker-limit-continue-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 0);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    mark_worker(&engine, &session, &root);
    let mut adapter = MockModelAdapter::new("");
    let limited = run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(
        limited.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::RepairLimitExhausted)
    );
    assert_eq!(limited.task.status, TaskStatus::Failed);
    assert!(
        adapter.requests.is_empty(),
        "a worker does not repair past the limit by itself"
    );

    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let continued = continue_repair(&engine, &root, &session, &mut repair);
    assert_eq!(repair.requests.len(), 1);
    assert!(
        continued.patch_proposal.is_some(),
        "the explicit action starts the worker's repair turn"
    );
    assert_ne!(continued.task.status, TaskStatus::Failed);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// Each explicit continuation is one attempt. A second click while that repair
/// is pending is refused, and after the next verification hits the limit again
/// the action is available once more.
#[test]
fn repeated_limit_exhaustion_needs_one_explicit_decision_each_time() {
    let root = temp_dir("limited-repeat");
    let data_dir = temp_dir("limited-repeat-data");
    cargofile(&root);
    // A changing failure is progress, so each explicit continuation reaches
    // the limit again instead of stopping as a stall (spec 102 Task 4).
    let shell = progress_shell(&root).to_string_lossy().to_string();
    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 0);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");
    let first = run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(
        first.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::RepairLimitExhausted)
    );
    let first_limited_task = first.task.id.clone();

    let mut first_repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let continued = continue_repair(&engine, &root, &session, &mut first_repair);
    let patch = continued.patch_proposal.as_ref().expect("a repair patch");
    assert_eq!(first_repair.requests.len(), 1);

    let mut second_click = MockModelAdapter::new("");
    let refused = try_continue_repair(&engine, &root, &session, &mut second_click)
        .expect_err("another explicit action while the repair is pending is refused");
    assert!(
        refused.to_string().contains("already in progress"),
        "{refused}"
    );
    assert!(
        second_click.requests.is_empty(),
        "a refused continuation spends no model call"
    );

    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect("the repair patch applies");
    let mut adapter = MockModelAdapter::new("");
    let second = run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(
        second.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::RepairLimitExhausted)
    );
    assert_ne!(second.task.id, first_limited_task);
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0, 1]
    );

    let mut second_repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 4;\n")]],
    );
    let continued = continue_repair(&engine, &root, &session, &mut second_repair);
    assert!(
        continued.patch_proposal.is_some(),
        "the exhausted limit offers the action again"
    );
    assert_eq!(second_repair.requests.len(), 1);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// The explicit continuation must not widen approvals: a command the repair
/// turn asks for still pauses on its card and does not run.
#[test]
fn an_explicit_repair_turn_still_waits_for_command_approval() {
    let root = temp_dir("limited-approval");
    let data_dir = temp_dir("limited-approval-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 0);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");
    run_verification(&engine, &root, &session, &mut adapter);
    let command = workspace_engine::ToolCall {
        id: "repair_command".to_string(),
        name: "run_command".to_string(),
        arguments_json: json!({
            "command": "touch approved.txt",
            "reason": "Write a marker as part of the repair",
        })
        .to_string(),
    };
    let mut repair =
        MockModelAdapter::new_sequence_with_tool_calls(vec![String::new()], vec![vec![command]]);

    let result = continue_repair(&engine, &root, &session, &mut repair);

    assert_eq!(repair.requests.len(), 1, "the repair turn ran");
    assert!(
        result.command_proposal.is_some(),
        "the command waits on its card"
    );
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    assert!(
        !root.join("approved.txt").exists(),
        "an unapproved command did not run"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_repair_turn_that_answers_without_a_patch_stops() {
    let root = temp_dir("repair-no-patch");
    let data_dir = temp_dir("repair-no-patch-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new("I cannot fix this without more information.");

    let result = run_verification(&engine, &root, &session, &mut repair);

    let report = result.verification_report.as_ref().unwrap();
    assert_eq!(report.outcome, Some(VerificationOutcome::NoRepairApplied));
    assert_eq!(result.task.status, TaskStatus::Failed);
    assert_eq!(repair.requests.len(), 1, "one repair question, not two");
    assert!(result.response.contains("no repair patch was applied"));
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0]
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_fully_rejected_repair_patch_closes_the_attempt_without_another_round() {
    let root = temp_dir("repair-rejected");
    let data_dir = temp_dir("repair-rejected-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let first = run_verification(&engine, &root, &session, &mut repair);
    let patch = first.patch_proposal.as_ref().unwrap();
    engine
        .edit_orchestrator
        .reject_stored_patch_files(&patch.patch_id, &["src/a.rs".to_string()], "tester")
        .expect("the repair patch is rejected in full");

    let mut adapter = MockModelAdapter::new("");
    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert_eq!(report.outcome, Some(VerificationOutcome::NoRepairApplied));
    assert_eq!(result.task.status, TaskStatus::Failed);
    assert!(
        adapter.requests.is_empty(),
        "a rejected repair patch does not spend another model call"
    );
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0]
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_repair_that_breaks_a_previously_passing_check_is_a_regression() {
    let root = temp_dir("repair-regression");
    let data_dir = temp_dir("repair-regression-data");
    cargofile(&root);
    write(
        &root,
        "packages/web/package.json",
        "{\"scripts\":{\"test\":\"node test.js\"}}\n",
    );
    let shell = repaired_shell(&root);
    fs::write(root.join("cargo-fail"), "fail\n").unwrap();
    let shell = shell.to_string_lossy().to_string();
    let allow = ["cargo test", "cd packages/web && npm run test"];
    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &allow, &[], 1);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[
            ("src/a.rs", "let a = 1;\n", "let a = 2;\n"),
            (
                "packages/web/src/index.ts",
                "export const x = 1;\n",
                "export const x = 2;\n",
            ),
        ],
    );
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );
    let first = run_verification(&engine, &root, &session, &mut repair);
    let patch = first.patch_proposal.as_ref().expect("a repair patch");
    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .unwrap();

    // The repair fixed A and broke B.
    fs::remove_file(root.join("cargo-fail")).unwrap();
    fs::write(root.join("web-fail"), "fail\n").unwrap();

    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &allow, &[], 1);
    let mut adapter = MockModelAdapter::new("");
    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert_ne!(report.outcome, Some(VerificationOutcome::Passed));
    assert_eq!(
        report.passed,
        vec![CheckId::new("", "cargo test")],
        "{report:?}"
    );
    assert!(
        report
            .failed
            .iter()
            .any(|item| item.check == CheckId::new("packages/web", "npm run test"))
    );
    assert_eq!(
        report.regressions,
        vec![CheckId::new("packages/web", "npm run test")]
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn the_repair_request_carries_normalized_evidence_not_the_raw_log() {
    const SECRET: &str = "AKIAIOSFODNN7EXAMPLE";
    let root = temp_dir("repair-request");
    let data_dir = temp_dir("repair-request-data");
    cargofile(&root);
    // A real `cargo test` failure: the test's assertion carries the left and
    // right values, and the output also leaks a secret-looking value. Only the
    // normalized finding, its location and its redacted details may reach the
    // model.
    let stdout = format!(
        "---- repro stdout ----\n\
         thread 'repro' panicked at src/a.rs:1:1:\n\
         assertion `left == right` failed\n\
         \x20 left: 41\n\
         \x20 right: 42\n\
         leaked credential {SECRET}\n\
         \n\
         failures:\n\
         \x20   repro\n"
    );
    let shell = scripted_cargo_shell(&root, &data_dir, &stdout, "");
    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new("I cannot fix this.");

    let result = run_verification(&engine, &root, &session, &mut repair);

    assert_eq!(
        result.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::NoRepairApplied)
    );
    let findings = engine.session_store.read_findings(&session, &root).unwrap();
    let finding = findings
        .first()
        .expect("the failing check recorded a finding");
    let request = repair
        .requests
        .first()
        .expect("the repair turn called the model");
    let user = request
        .messages
        .iter()
        .find(|message| message.role == "user")
        .expect("a user message carries the request");
    assert!(
        user.content.contains(finding.summary()),
        "the repair request carries the normalized finding: {}",
        user.content
    );
    assert!(
        user.content.contains("src/a.rs:1:1"),
        "the repair request names the normalized location: {}",
        user.content
    );
    assert!(
        user.content
            .contains("Repair only the failures this patch introduced"),
        "the repair request states its scope: {}",
        user.content
    );
    // R6 (spec 102): a finding's already-redacted details are part of the
    // repair evidence. An `assert_eq` failure's left and right values are the
    // action the worker needs, and they were lost when the request switched to
    // the normalized summary line.
    assert!(
        user.content.contains("left: 41") && user.content.contains("right: 42"),
        "the repair request carries the assertion's left and right values: {}",
        user.content
    );
    // The secret in the raw output must not reach the model, even though the
    // surrounding details do.
    assert!(
        !user.content.contains(SECRET),
        "a raw-output secret reached the repair request: {}",
        user.content
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// R6 (spec 102): a rustc type error's `expected`/`found` lines live in the
/// finding's details, not its summary. They must reach the repair request
/// while the normalized summary and location stay intact.
#[test]
fn the_repair_request_carries_a_rustc_type_error_expected_and_found() {
    let root = temp_dir("repair-request-rustc-details");
    let data_dir = temp_dir("repair-request-rustc-details-data");
    cargofile(&root);
    let stderr = "error[E0308]: mismatched types\n\
                  \x20--> src/a.rs:1:1\n\
                  \x20 |\n\
                  \x201 | let a: u8 = \"text\";\n\
                  \x20 |              ^^^^^^ expected `u8`, found `&str`\n\
                  \n\
                  error: could not compile `fixture` (bin \"fixture\") due to 1 previous error\n";
    let shell = scripted_cargo_shell(&root, &data_dir, "", stderr);
    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new("I cannot fix this.");

    run_verification(&engine, &root, &session, &mut repair);

    let request = repair
        .requests
        .first()
        .expect("the repair turn called the model");
    let user = request
        .messages
        .iter()
        .find(|message| message.role == "user")
        .expect("a user message carries the request");
    assert!(
        user.content.contains("mismatched types") && user.content.contains("src/a.rs:1:1"),
        "the normalized summary and location stay: {}",
        user.content
    );
    assert!(
        user.content.contains("expected `u8`, found `&str`"),
        "the repair request carries rustc's expected and found detail: {}",
        user.content
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// R6 (spec 102): an unparsed check falls back to one generic finding whose
/// details are a bounded stderr tail. The repair request carries that tail and
/// still leaves out the lines the finding itself dropped.
#[test]
fn the_repair_request_carries_a_generic_checks_bounded_stderr_tail() {
    let root = temp_dir("repair-request-generic-details");
    let data_dir = temp_dir("repair-request-generic-details-data");
    cargofile(&root);
    let mut lines = vec!["BEGIN UNPARSED CHECK".to_string()];
    for index in 1..=44 {
        lines.push(format!("line {index:02}"));
    }
    let stderr = format!("{}\n", lines.join("\n"));
    let shell = scripted_cargo_shell(&root, &data_dir, "", &stderr);
    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new("I cannot fix this.");

    run_verification(&engine, &root, &session, &mut repair);

    let findings = engine.session_store.read_findings(&session, &root).unwrap();
    let finding = findings
        .first()
        .expect("the generic fallback recorded a finding");
    assert_eq!(
        finding.source(),
        workspace_engine::finding::FindingSource::Command
    );
    let details = finding.details().expect("a generic finding keeps a tail");
    assert!(
        details.contains("line 44") && !details.contains("line 01"),
        "the finding itself keeps only the bounded tail: {details}"
    );
    let request = repair
        .requests
        .first()
        .expect("the repair turn called the model");
    let user = request
        .messages
        .iter()
        .find(|message| message.role == "user")
        .expect("a user message carries the request");
    assert!(
        user.content.contains("line 44"),
        "the repair request carries the generic stderr tail: {}",
        user.content
    );
    assert!(
        !user.content.contains("line 01"),
        "the repair request does not carry lines the finding dropped: {}",
        user.content
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn before_task_complete_fires_for_a_repair_answer_but_not_for_a_report() {
    use std::os::unix::fs::PermissionsExt;
    let marker = |root: &std::path::Path| root.join("before-complete-ran");
    let hook_for = |root: &std::path::Path| {
        let script = root.join("hooks").join("before-complete.sh");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(
            &script,
            format!(
                "echo invoked >> {}\necho '{{\"hookApiVersion\":1,\"verdict\":\"allow\"}}'\n",
                marker(root).display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        HookConfig {
            id: "before_complete".to_string(),
            command: "/bin/sh".to_string(),
            args: vec![script.to_string_lossy().to_string()],
            events: vec![HookEvent::BeforeTaskComplete],
            mode: HookMode::Mandatory,
            order: 0,
            timeout_ms: Some(5_000),
            enabled: true,
            source: HookSource::User,
        }
    };

    // A report-only verification turn never calls the model, so the hook does
    // not fire.
    let root = temp_dir("complete-report");
    let data_dir = temp_dir("complete-report-data");
    cargofile(&root);
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        command_allowlist: vec!["cargo test".to_string()],
        agent_tool_retry_limit: 0,
        hooks: vec![hook_for(&root)],
        ..Config::default()
    });
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");
    run_verification(&engine, &root, &session, &mut adapter);
    assert!(!marker(&root).exists(), "a report is not a model answer");

    // A repair turn that answers does call the model, so it fires.
    let root = temp_dir("complete-repair");
    let data_dir = temp_dir("complete-repair-data");
    cargofile(&root);
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/false".to_string(),
        command_allowlist: vec!["cargo test".to_string()],
        agent_tool_retry_limit: 1,
        hooks: vec![hook_for(&root)],
        ..Config::default()
    });
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("I cannot fix this.");
    run_verification(&engine, &root, &session, &mut adapter);
    assert!(
        marker(&root).exists(),
        "the repair answer dispatches the hook"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_repair_turn_that_hits_the_round_budget_pauses_instead_of_failing() {
    let root = temp_dir("repair-round-budget");
    let data_dir = temp_dir("repair-round-budget-data");
    cargofile(&root);
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/false".to_string(),
        command_allowlist: vec!["cargo test".to_string()],
        agent_tool_retry_limit: 2,
        agent_max_tool_rounds: 1,
        ..Config::default()
    });
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let complete_step = workspace_engine::ToolCall {
        id: "step_done".to_string(),
        name: "complete_step".to_string(),
        arguments_json: "{}".to_string(),
    };
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "I cannot fix this.".to_string()],
        vec![vec![complete_step], Vec::new()],
    );

    let paused = run_verification(&engine, &root, &session, &mut adapter);
    let budget = paused
        .round_budget_proposal
        .as_ref()
        .expect("the repair turn pauses at its round budget");
    assert!(
        paused.verification_report.is_none(),
        "the attempt is not closed"
    );
    assert_eq!(adapter.requests.len(), 1);

    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let resumed = engine
        .chat_orchestrator
        .resume_round_budget(
            &budget.id,
            RoundBudgetDecision::Continue,
            "tester",
            &mut adapter,
            &mut sink,
        )
        .expect("Continue resumes the repair turn");

    assert_eq!(
        resumed.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::NoRepairApplied)
    );
    assert_eq!(
        started_attempts(&outcome_events(&engine, &session)),
        vec![0],
        "a continuation is not another attempt"
    );
    assert_eq!(adapter.requests.len(), 2, "Continue runs one more round");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_repair_turn_pauses_for_context_review_and_resumes_as_a_repair() {
    let root = temp_dir("repair-context-review");
    let data_dir = temp_dir("repair-context-review-data");
    cargofile(&root);
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/false".to_string(),
        command_allowlist: vec!["cargo test".to_string()],
        agent_tool_retry_limit: 2,
        pause_for_context_review: true,
        ..Config::default()
    });
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("I cannot fix this.");

    let paused = run_verification(&engine, &root, &session, &mut adapter);
    let review = paused
        .context_review
        .as_ref()
        .expect("the repair turn pauses before sending");
    assert!(
        adapter.requests.is_empty(),
        "a paused repair turn must not call the model"
    );
    assert!(
        paused.verification_report.is_none(),
        "a pause is not the end of the attempt"
    );

    let resumed = resume_context_review(&engine, &review.id, ContextDecision::Send, &mut adapter)
        .expect("Send resumes the repair turn");
    assert_eq!(
        resumed.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::NoRepairApplied),
        "a resumed repair that answers without a patch closes as no repair"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// --- Browser behaviour verification (Task 6) -------------------------------

struct StubBrowserRunner {
    text: String,
    is_error: bool,
    details: Option<WebDiagnosticDetails>,
    unreachable: bool,
}

impl StubBrowserRunner {
    fn clean() -> Self {
        Self {
            text: "Page loaded with no errors.".to_string(),
            is_error: false,
            details: None,
            unreachable: false,
        }
    }

    fn page_errors() -> Self {
        Self {
            text: "Browser diagnostic found 1 page error.\n- pageerror: boom".to_string(),
            is_error: false,
            details: Some(WebDiagnosticDetails {
                page_errors: vec!["boom".to_string()],
                ..Default::default()
            }),
            unreachable: false,
        }
    }

    fn unreachable() -> Self {
        Self {
            text: String::new(),
            is_error: false,
            details: None,
            unreachable: true,
        }
    }
}

impl WebDiagnosticsRunner for StubBrowserRunner {
    fn inspect(&self, _call: &WebDiagnosticCall) -> workspace_engine::Result<WebDiagnosticReport> {
        if self.unreachable {
            return Err(workspace_engine::ClientError::Io(
                "no server is listening".to_string(),
            ));
        }
        let mut report = WebDiagnosticReport::from_text(self.text.clone(), self.is_error);
        report.details = self.details.clone();
        Ok(report)
    }

    fn run_scenario(
        &self,
        call: &WebDiagnosticCall,
    ) -> workspace_engine::Result<WebDiagnosticReport> {
        self.inspect(call)
    }
}

fn seed_browser_target(engine: &WorkspaceEngine, session_id: &str) {
    engine
        .session_store
        .append_web_diagnostic(
            session_id,
            &WebDiagnosticRecord {
                id: "webdiagrec_seed".to_string(),
                task_id: "earlier_task".to_string(),
                tool: "inspect_web_page".to_string(),
                url: "http://127.0.0.1:3000/".to_string(),
                recorded_at_ms: 1,
                report: WebDiagnosticReport::from_text("earlier run", false),
            },
        )
        .unwrap();
}

fn web_change(engine: &WorkspaceEngine, root: &std::path::Path) -> String {
    let (session, _task) = applied_change(
        engine,
        root,
        &[(
            "src/app.js",
            "export const answer = 1;\n",
            "export const answer = 2;\n",
        )],
    );
    session
}

#[test]
fn a_clean_browser_check_is_behaviour_verified() {
    let root = temp_dir("browser-clean");
    let data_dir = temp_dir("browser-clean-data");
    let mut engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    engine
        .chat_orchestrator
        .set_web_diagnostics_runner(WebDiagnosticsRunnerHandle::new(StubBrowserRunner::clean()));
    let session = web_change(&engine, &root);
    engine
        .session_store
        .allow_browser_diagnostics_for_session(&session, "tester")
        .unwrap();
    seed_browser_target(&engine, &session);
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert_eq!(report.outcome, Some(VerificationOutcome::Passed));
    assert_eq!(
        report.behaviour_verified,
        vec!["browser check http://127.0.0.1:3000/".to_string()]
    );
    assert_eq!(
        report.passed,
        vec![CheckId::browser("http://127.0.0.1:3000/")]
    );
    assert!(report.skipped.is_empty());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_page_with_errors_is_failed_and_not_behaviour_verified() {
    let root = temp_dir("browser-errors");
    let data_dir = temp_dir("browser-errors-data");
    let mut engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    engine
        .chat_orchestrator
        .set_web_diagnostics_runner(WebDiagnosticsRunnerHandle::new(
            StubBrowserRunner::page_errors(),
        ));
    let session = web_change(&engine, &root);
    engine
        .session_store
        .allow_browser_diagnostics_for_session(&session, "tester")
        .unwrap();
    seed_browser_target(&engine, &session);
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert!(report.behaviour_verified.is_empty());
    let failed = report
        .failed
        .iter()
        .find(|item| item.check.is_browser())
        .expect("the browser check failed");
    assert_eq!(failed.reason, "the page reported errors");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn an_unreachable_page_is_skipped_with_no_running_target() {
    let root = temp_dir("browser-unreachable");
    let data_dir = temp_dir("browser-unreachable-data");
    let mut engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    engine
        .chat_orchestrator
        .set_web_diagnostics_runner(WebDiagnosticsRunnerHandle::new(
            StubBrowserRunner::unreachable(),
        ));
    let session = web_change(&engine, &root);
    engine
        .session_store
        .allow_browser_diagnostics_for_session(&session, "tester")
        .unwrap();
    seed_browser_target(&engine, &session);
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert!(report.behaviour_verified.is_empty());
    let skipped = report
        .skipped
        .iter()
        .find(|item| item.check.is_browser())
        .expect("the browser check is skipped");
    assert_eq!(skipped.reason, SkipReason::NoRunningTarget);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_web_change_with_no_stored_target_is_skipped_with_no_running_target() {
    let root = temp_dir("browser-no-target");
    let data_dir = temp_dir("browser-no-target-data");
    let mut engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    engine
        .chat_orchestrator
        .set_web_diagnostics_runner(WebDiagnosticsRunnerHandle::new(StubBrowserRunner::clean()));
    let session = web_change(&engine, &root);
    engine
        .session_store
        .allow_browser_diagnostics_for_session(&session, "tester")
        .unwrap();
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert!(report.behaviour_verified.is_empty());
    let skipped = report
        .skipped
        .iter()
        .find(|item| item.check.is_browser())
        .expect("the browser check is skipped");
    assert_eq!(skipped.reason, SkipReason::NoRunningTarget);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn without_the_session_approval_the_browser_check_is_skipped_by_policy() {
    let root = temp_dir("browser-unapproved");
    let data_dir = temp_dir("browser-unapproved-data");
    let mut engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    engine
        .chat_orchestrator
        .set_web_diagnostics_runner(WebDiagnosticsRunnerHandle::new(StubBrowserRunner::clean()));
    let session = web_change(&engine, &root);
    seed_browser_target(&engine, &session);
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().unwrap();
    assert!(report.behaviour_verified.is_empty());
    let skipped = report
        .skipped
        .iter()
        .find(|item| item.check.is_browser())
        .expect("the browser check is skipped");
    assert!(matches!(skipped.reason, SkipReason::Blocked { .. }));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// --- Recovery (Task 7) -----------------------------------------------------

#[test]
fn a_due_verification_is_offered_after_a_reopen() {
    let root = temp_dir("recover-due");
    let data_dir = temp_dir("recover-due-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );

    let reopened = SessionStore::new(&data_dir);
    let offers = pending_verifications(&reopened, &session).expect("the log reads");

    assert_eq!(offers.len(), 1, "one due verification is offered");
    assert_eq!(
        offers[0].patch_id,
        engine
            .session_store
            .read_all_patch_file_decisions(&session)
            .unwrap()
            .first()
            .unwrap()
            .patch_id
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_check_batch_is_reattached_and_resumes_after_a_reopen() {
    let root = temp_dir("recover-batch");
    let data_dir = temp_dir("recover-batch-data");
    cargofile(&root);
    write(
        &root,
        "packages/web/package.json",
        "{\"scripts\":{\"test\":\"node test.js\"}}\n",
    );
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[
            ("src/a.rs", "let a = 1;\n", "let a = 2;\n"),
            (
                "packages/web/src/index.ts",
                "export const x = 1;\n",
                "export const x = 2;\n",
            ),
        ],
    );
    let mut adapter = MockModelAdapter::new("");
    let paused = run_verification(&engine, &root, &session, &mut adapter);
    let batch = paused.check_batch.as_ref().unwrap();

    // Nothing is running now: recovery re-presents the batch, not the due
    // event behind it.
    assert!(
        pending_verifications(&engine.session_store, &session)
            .unwrap()
            .is_empty(),
        "a waiting batch is the offer, not a second verification offer"
    );

    let reopened = SessionStore::new(&data_dir);
    let paused_turns = PausedTurns::new(&data_dir);
    let reattached = reattach_pending_approvals(
        &reopened,
        &engine.audit_log,
        &engine.command_store,
        &engine.patch_store,
        &paused_turns,
        &session,
    )
    .expect("recovery should run");
    let check_batch = reattached
        .iter()
        .find_map(|item| match item {
            ReattachedApproval::CheckBatch {
                proposal_id,
                checks,
                ..
            } => Some((proposal_id, checks)),
            _ => None,
        })
        .expect("the batch is reattached");
    assert_eq!(check_batch.0, &batch.id);
    assert_eq!(check_batch.1.len(), 2);

    // A fresh orchestrator over the same data dir resumes it.
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    let mut adapter = MockModelAdapter::new("");
    let decisions = batch
        .checks
        .iter()
        .map(|check| (check.id.clone(), true))
        .collect::<Vec<_>>();
    let resumed = resume_checks(&engine, check_batch.0, &decisions, &mut adapter);
    assert_eq!(
        resumed.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::Passed)
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_verification_interrupted_mid_check_is_offered_and_nothing_is_passed() {
    let root = temp_dir("recover-interrupted");
    let data_dir = temp_dir("recover-interrupted-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let check = CheckId::new("", "cargo test");
    engine
        .session_store
        .append_verification_event(
            &session,
            &VerificationEvent::Started {
                task_id: "verification_turn".to_string(),
                attempt: 0,
                checks: vec![check.clone()],
            },
        )
        .unwrap();

    let reopened = SessionStore::new(&data_dir);
    let offers = pending_verifications(&reopened, &session).unwrap();
    assert_eq!(
        offers.len(),
        1,
        "the interrupted verification is offered again"
    );

    let events = reopened.read_verification_events(&session, &[]).unwrap();
    let state = fold_verification(events.iter());
    let report = VerificationReport::build(&state, &empty_plan(), &[], &BTreeMap::new());
    assert!(report.passed.is_empty());
    assert!(report.failed.iter().any(|item| item.check == check));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// Spec 77: a verification that leaves the task's plan open continues the task
// in the same turn instead of ending it with the report.

/// Gives `task_id` a plan with one finished step and the given open ones, as an
/// agentic turn that proposed its first patch would leave it.
fn open_plan(engine: &WorkspaceEngine, session_id: &str, task_id: &str, open: &[&str]) {
    let task = engine
        .session_store
        .read_tasks(session_id)
        .unwrap()
        .into_iter()
        .find(|task| task.id == task_id)
        .expect("the patch's task");
    let mut plan = complete_plan();
    plan.task_id = task_id.to_string();
    for (index, title) in open.iter().enumerate() {
        plan.steps.push(PlanStep {
            id: format!("step-{}", index + 2),
            title: (*title).to_string(),
            detail: None,
            status: StepStatus::Pending,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
            build: None,
        });
    }
    engine.session_store.create_plan(&task, &plan).unwrap();
}

fn first_user_message(adapter: &MockModelAdapter) -> String {
    adapter
        .requests
        .first()
        .expect("the model was called")
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

#[test]
fn a_passed_verification_with_open_steps_continues_the_task() {
    let root = temp_dir("continue-open-plan");
    let data_dir = temp_dir("continue-open-plan-data");
    cargofile(&root);
    let engine = verify_engine(
        &root,
        &data_dir,
        "/usr/bin/true",
        &["cargo test", "cargo check --tests"],
        &[],
    );
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    open_plan(&engine, &session, &task, &["Update the docs", "Run clippy"]);
    let mut adapter = MockModelAdapter::new("Docs updated; clippy is left.");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    // The continuation is the first call; spec 66's open-plan reminder may
    // follow it, since this answer leaves steps open.
    assert!(!adapter.requests.is_empty(), "the task was continued");
    let prompt = first_user_message(&adapter);
    assert!(prompt.contains("The task is not finished"), "{prompt}");
    assert!(prompt.contains("- Update the docs (step-2)"), "{prompt}");
    assert!(prompt.contains("- Run clippy (step-3)"), "{prompt}");
    assert!(
        !prompt.contains("understand existing retry logic ("),
        "{prompt}"
    );
    assert!(
        result.response.contains("Docs updated"),
        "{}",
        result.response
    );
    let report = result
        .verification_report
        .as_ref()
        .expect("the report stays with the turn");
    assert_eq!(report.outcome, Some(VerificationOutcome::Passed));
    // Spec 101: the successful verification settles `incomplete` because the
    // task's own plan work remains, and the continuation's answer leaves it
    // open again. No transient `complete` is ever written.
    assert_eq!(result.task.status, TaskStatus::Incomplete);
    let settled = engine
        .session_store
        .read_completion_report(&session, &result.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert_eq!(
        settled.reason_codes(),
        vec![CompletionReasonCode::OpenPlanSteps]
    );
    let log = fs::read_to_string(data_dir.join("sessions").join(format!("{session}.jsonl")))
        .expect("the session log should read");
    let transient_complete = log
        .lines()
        .filter(|line| {
            line.contains("\"eventType\":\"task_status_updated\"")
                && line.contains(&result.task.id)
                && line.contains("\"status\":\"complete\"")
        })
        .count();
    assert_eq!(
        transient_complete, 0,
        "open plan work must never pass through a transient complete"
    );
    // The report was still recorded before the continuation answered.
    let messages = engine.session_store.read_messages(&session).unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.content.starts_with("Verification passed: 1 check")),
        "{messages:?}"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_plan_with_nothing_open_ends_with_the_report() {
    let root = temp_dir("continue-closed-plan");
    let data_dir = temp_dir("continue-closed-plan-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[]);
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    open_plan(&engine, &session, &task, &[]);
    let mut adapter = MockModelAdapter::new("unused");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert!(adapter.requests.is_empty(), "nothing to continue");
    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(result.response.contains("Verification complete"));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_failed_verification_is_not_continued() {
    let root = temp_dir("continue-failed");
    let data_dir = temp_dir("continue-failed-data");
    cargofile(&root);
    // Retry limit 0: a failure reports at once instead of starting a repair.
    let engine = verify_engine(
        &root,
        &data_dir,
        "/usr/bin/false",
        &["cargo test"],
        &["cargo check"],
    );
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    open_plan(&engine, &session, &task, &["Update the docs"]);
    let mut adapter = MockModelAdapter::new("unused");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert!(adapter.requests.is_empty(), "a failure is not continued");
    assert_eq!(result.task.status, TaskStatus::Failed);
    let settled = engine
        .session_store
        .read_completion_report(&session, &result.task.id)
        .unwrap()
        .expect("a failed verification still carries the gate report");
    assert!(!settled.eligible);
    // R16: the discovered floor is a task-completion obligation, so while this
    // plan still has an open step the report names that work, not the failed
    // discovered check. The failed status still comes from the verification
    // settlement, so a failed check can never be continued as success.
    assert!(
        settled
            .reason_codes()
            .contains(&CompletionReasonCode::OpenPlanSteps),
        "{settled:?}"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// Spec 78: a verification whose every check was skipped is not a pass.

#[test]
fn declining_every_check_is_nothing_verified_not_passed() {
    let root = temp_dir("all-skipped");
    let data_dir = temp_dir("all-skipped-data");
    cargofile(&root);
    // No allowlist: `cargo test` needs approval, so it arrives on the card.
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");
    let paused = run_verification(&engine, &root, &session, &mut adapter);
    let batch = paused.check_batch.expect("the check needs approval");
    let declined: Vec<(String, bool)> = batch
        .checks
        .iter()
        .map(|check| (check.id.clone(), false))
        .collect();

    let result = resume_checks(&engine, &batch.id, &declined, &mut adapter);

    let report = result.verification_report.expect("a report");
    assert_eq!(report.outcome, Some(VerificationOutcome::NothingVerified));
    assert!(report.passed.is_empty());
    assert!(!report.skipped.is_empty());
    assert!(
        result.response.starts_with("No check ran:"),
        "{}",
        result.response
    );
    let summary = result.response.lines().next().unwrap_or_default();
    assert!(!summary.contains("passed"), "{summary}");
    // Declining is the user's choice, not a failure of the task.
    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(
        !outcome_events(&engine, &session)
            .iter()
            .any(|event| matches!(
                event,
                VerificationEvent::Finished {
                    outcome: VerificationOutcome::Passed,
                    ..
                }
            )),
        "no `passed` was recorded"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A real repository, because the command census asks git what changed.
fn git_repository(root: &std::path::Path) {
    for args in [
        &["init", "--quiet"][..],
        &["config", "user.email", "test@damaian.local"],
        &["config", "user.name", "Damaian Test"],
        &["config", "gc.auto", "0"],
        &["config", "maintenance.auto", "false"],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git should run");
        assert!(output.status.success(), "git {args:?} failed");
    }
}

fn command_call(command: &str) -> workspace_engine::ToolCall {
    workspace_engine::ToolCall {
        id: "command".to_string(),
        name: "run_command".to_string(),
        arguments_json: json!({"command": command, "reason": "Make the change"}).to_string(),
    }
}

/// Asks for a turn that runs `command`, approves it, and returns the result
/// of the resumed turn, which answers "Done." once the command has run.
fn approve_command_turn(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    command: &str,
) -> (ChatTurnResult, ChatTurnResult) {
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Done.".to_string()],
        vec![vec![command_call(command)], Vec::new()],
    );
    let mut on_token = |_token: &str| {};
    let asked = engine
        .chat_orchestrator
        .ask(root, "Make the change.", &[], &mut adapter, &mut on_token)
        .expect("the turn should run");
    let proposal = asked
        .command_proposal
        .as_ref()
        .expect("a command that writes asks for approval")
        .id
        .clone();
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let resumed = engine
        .chat_orchestrator
        .resume_after_command_decision(&proposal, true, "tester", &mut adapter, &mut sink)
        .expect("the approved command should resume the turn");
    (asked, resumed)
}

/// Spec 85: an approved command that changes a file makes verification due,
/// and the turn ends with that verification, as a settled patch does.
#[test]
fn an_approved_command_that_changes_a_file_is_verified_when_the_turn_ends() {
    let root = temp_dir("command-verify");
    let data_dir = temp_dir("command-verify-data");
    git_repository(&root);
    let engine = verify_engine(&root, &data_dir, "/bin/sh", &[], &[]);

    let (asked, resumed) = approve_command_turn(&engine, &root, "touch notes.txt");

    assert!(root.join("notes.txt").exists(), "the command ran");
    let events = outcome_events(&engine, &asked.session.id);
    assert!(
        events.iter().any(|event| matches!(
            event,
            VerificationEvent::Due { patch_id, paths, .. }
                if patch_id.is_empty() && paths == &vec!["notes.txt".to_string()]
        )),
        "the command's file makes verification due: {events:?}"
    );
    let report = resumed
        .verification_report
        .as_ref()
        .expect("the turn ends with the verification");
    assert_eq!(
        report.outcome,
        Some(VerificationOutcome::OnlyDocumentationChanged),
        "{report:?}"
    );
    assert_eq!(report.files_changed, vec!["notes.txt".to_string()]);
    assert!(
        resumed.response.starts_with("Done."),
        "the turn's own answer stays in front of the report: {:?}",
        resumed.response
    );
    assert!(
        pending_verifications(&engine.session_store, &asked.session.id)
            .unwrap()
            .is_empty(),
        "nothing is left due"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A command that changes nothing git sees leaves verification alone.
#[test]
fn an_approved_command_that_changes_nothing_is_not_verified() {
    let root = temp_dir("command-no-change");
    let data_dir = temp_dir("command-no-change-data");
    git_repository(&root);
    let engine = verify_engine(&root, &data_dir, "/bin/sh", &[], &[]);

    let (asked, resumed) = approve_command_turn(&engine, &root, "mkdir -p empty");

    assert!(root.join("empty").is_dir(), "the command ran");
    assert!(resumed.verification_report.is_none());
    assert_eq!(resumed.response, "Done.");
    assert!(
        !outcome_events(&engine, &asked.session.id)
            .iter()
            .any(|event| matches!(event, VerificationEvent::Due { .. })),
        "an empty directory is no change to verify"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A census that could not be taken claims no change, rather than "nothing
/// changed": the command's effects are then unknown, not absent (spec 85).
#[test]
fn an_unavailable_census_reports_no_changed_paths() {
    let census = workspace_engine::CommandCensus::unavailable();
    assert_eq!(census.changed_paths(&census), None);
}

// --- Spec 102 Task 3: provenance gates repair -------------------------------

fn shell_returning(root: &std::path::Path, code: i32) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join(format!("exit-{code}.sh"));
    fs::write(&path, format!("#!/bin/sh\nexit {code}\n")).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path.to_string_lossy().to_string()
}

fn provenance_group(
    report: &VerificationReport,
    provenance: FailureProvenance,
) -> Option<&workspace_engine::ProvenancePartition> {
    report
        .provenance
        .iter()
        .find(|group| group.provenance == provenance)
}

#[test]
fn a_declined_baseline_makes_the_failure_unknown_and_starts_no_repair() {
    let root = temp_dir("declined-baseline");
    let data_dir = temp_dir("declined-baseline-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change_with_declined_baseline(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("unused");

    let result = run_verification(&engine, &root, &session, &mut adapter);
    let report = result
        .verification_report
        .as_ref()
        .expect("a no-repair verification still reports");

    assert_eq!(report.outcome, Some(VerificationOutcome::Failed));
    assert!(
        adapter.requests.is_empty(),
        "a declined baseline must never start a code repair"
    );
    assert!(result.patch_proposal.is_none());
    assert!(report.repair_targets.is_empty());
    let group = provenance_group(report, FailureProvenance::UnknownProvenance)
        .expect("the failure is unknown provenance");
    assert_eq!(group.checks.len(), 1);
    assert!(group.action.is_some());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_matching_failed_baseline_is_pre_existing_and_starts_no_repair() {
    let root = temp_dir("pre-existing-baseline");
    let data_dir = temp_dir("pre-existing-baseline-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change_with_declined_baseline(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    // The first run has no usable baseline: it is unknown, and records the
    // exact post-patch fingerprint and evidence a failed baseline would have.
    let mut first = MockModelAdapter::new("unused");
    let first_result = run_verification(&engine, &root, &session, &mut first);
    assert!(first_result.patch_proposal.is_none());
    assert!(first.requests.is_empty());

    let events = outcome_events(&engine, &session);
    let state = fold_verification(events.iter());
    let record = state
        .provenance
        .values()
        .next()
        .expect("the first run recorded its fingerprint")
        .clone();
    let (patch_task, patch_id) = events
        .iter()
        .find_map(|event| match event {
            VerificationEvent::Due {
                task_id, patch_id, ..
            } => Some((task_id.clone(), patch_id.clone())),
            _ => None,
        })
        .expect("the patch made verification due");
    engine
        .session_store
        .append_verification_event(
            &session,
            &VerificationEvent::Baseline {
                task_id: patch_task.clone(),
                check: record.check.clone(),
                baseline: CheckBaseline::Failed {
                    termination: record.termination.clone(),
                    fingerprint: record.fingerprint.clone(),
                    findings: record.findings.clone(),
                },
            },
        )
        .unwrap();
    engine
        .session_store
        .append_verification_event(
            &session,
            &VerificationEvent::Due {
                task_id: patch_task,
                patch_id,
                paths: Vec::new(),
                checks: Vec::new(),
            },
        )
        .unwrap();

    let mut second = MockModelAdapter::new("unused");
    let result = run_verification(&engine, &root, &session, &mut second);
    let report = result
        .verification_report
        .as_ref()
        .expect("a pre-existing failure still reports");

    // Spec 106 requirement 11: when the only failure is the one the task
    // started with, the run is not a failure of this task. It is still never
    // `Passed`, and it still starts no repair.
    assert_eq!(
        report.outcome,
        Some(VerificationOutcome::OnlyPreExistingFailures)
    );
    assert!(result.patch_proposal.is_none());
    assert!(
        second.requests.is_empty(),
        "a pre-existing failure must never start a code repair"
    );
    assert!(report.repair_targets.is_empty());
    let group = provenance_group(report, FailureProvenance::PreExisting)
        .expect("the matching failed baseline is pre-existing");
    assert_eq!(group.checks.len(), 1);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// R5 (spec 102): a pre-existing failure whose line moved because the patch
/// added lines above it is still the same failure. Finding location is not part
/// of provenance identity, so no repair turn starts.
#[test]
fn a_pre_existing_error_that_shifts_lines_stays_pre_existing() {
    let root = temp_dir("pre-existing-shift");
    let data_dir = temp_dir("pre-existing-shift-data");
    cargofile(&root);
    let shell = shifting_line_shell(&root);
    let engine = verify_engine_with_retry(
        &root,
        &data_dir,
        &shell.to_string_lossy(),
        &["cargo test"],
        &[],
        2,
    );
    let (session, _task) = applied_change_keeping_baseline(
        &engine,
        &root,
        &[(
            "src/a.rs",
            "let first = 1;\nlet second = 2;\nlet third = 3;\nbroken();\n",
            "// added one\n// added two\n// added three\nlet first = 1;\nlet second = 2;\nlet third = 3;\nbroken();\n",
        )],
    );

    let mut adapter = MockModelAdapter::new("unused");
    let result = run_verification(&engine, &root, &session, &mut adapter);

    // Prove the line really moved: the real pre-patch baseline failed at line
    // 4 and the post-patch result reports line 7.
    let state = fold_verification(outcome_events(&engine, &session).iter());
    let baseline_line = state
        .baselines
        .values()
        .find_map(|baseline| match baseline {
            CheckBaseline::Failed { findings, .. } => {
                findings.first().and_then(|finding| finding.start_line())
            }
            _ => None,
        })
        .expect("the pre-apply baseline ran and failed");
    let current_line = state
        .provenance
        .values()
        .find_map(|record| {
            record
                .findings
                .first()
                .and_then(|finding| finding.start_line())
        })
        .expect("the post-patch result recorded its findings");
    assert_eq!(baseline_line, 4);
    assert_eq!(current_line, 7);

    let report = result
        .verification_report
        .as_ref()
        .expect("a pre-existing failure still reports");
    // Spec 106 requirement 11: the only remaining failure is the task-start
    // one, so the run finishes `only_pre_existing_failures`, never `Passed`.
    assert_eq!(
        report.outcome,
        Some(VerificationOutcome::OnlyPreExistingFailures)
    );
    assert!(result.patch_proposal.is_none());
    assert!(
        adapter.requests.is_empty(),
        "a shifted pre-existing failure must never start a code repair"
    );
    assert!(report.repair_targets.is_empty());
    let group = provenance_group(report, FailureProvenance::PreExisting)
        .expect("a shifted line is still the same pre-existing failure");
    assert_eq!(group.checks.len(), 1);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_spawn_failure_is_an_environment_failure_and_starts_no_repair() {
    let root = temp_dir("spawn-failure");
    let data_dir = temp_dir("spawn-failure-data");
    cargofile(&root);
    let missing_shell = root.join("no-such-shell").to_string_lossy().to_string();
    let engine =
        verify_engine_with_retry(&root, &data_dir, &missing_shell, &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("unused");

    let result = run_verification(&engine, &root, &session, &mut adapter);
    let report = result
        .verification_report
        .as_ref()
        .expect("an environment failure still reports");

    assert!(result.patch_proposal.is_none());
    assert!(
        adapter.requests.is_empty(),
        "a spawn failure must never start a code repair"
    );
    assert!(report.repair_targets.is_empty());
    let group = provenance_group(report, FailureProvenance::EnvironmentFailure)
        .expect("a spawn failure is an environment failure");
    assert_eq!(group.checks.len(), 1);
    assert!(group.action.as_ref().unwrap().label.contains("environment"));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// Spec 102 Task 3 review: an I/O failure while saving the run's execution
/// record says nothing about the environment. It must propagate as the error
/// it is, rather than being disguised as a spawn failure.
#[test]
fn an_execution_store_failure_propagates_instead_of_becoming_environment() {
    let root = temp_dir("execution-store-failure");
    let data_dir = temp_dir("execution-store-failure-data");
    cargofile(&root);
    let shell = shell_returning(&root, 1);
    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    // `commands/output` is where the execution record is written. Replace it
    // with a file after the patch settles, so the check itself runs but saving
    // its record fails.
    let output = data_dir.join("commands").join("output");
    let _ = fs::remove_dir_all(&output);
    fs::create_dir_all(data_dir.join("commands")).unwrap();
    fs::write(&output, "not a directory").unwrap();
    let mut adapter = MockModelAdapter::new("unused");

    let error = try_run_verification(&engine, &root, &session, &mut adapter)
        .expect_err("an internal store failure must not become provenance");

    assert!(
        matches!(error, workspace_engine::ClientError::Io(_)),
        "{error:?}"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_missing_executable_is_an_environment_failure_and_starts_no_repair() {
    let root = temp_dir("missing-executable");
    let data_dir = temp_dir("missing-executable-data");
    cargofile(&root);
    let shell = shell_returning(&root, 127);
    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("unused");

    let result = run_verification(&engine, &root, &session, &mut adapter);
    let report = result
        .verification_report
        .as_ref()
        .expect("an environment failure still reports");

    assert!(result.patch_proposal.is_none());
    assert!(
        adapter.requests.is_empty(),
        "a missing executable must never start a code repair"
    );
    assert!(report.repair_targets.is_empty());
    let group = provenance_group(report, FailureProvenance::EnvironmentFailure)
        .expect("exit 127 is a missing executable");
    assert!(
        group.checks[0]
            .reason
            .contains("executable is not installed"),
        "{:?}",
        group.checks[0].reason
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_policy_refused_check_is_reported_as_environment_and_starts_no_repair() {
    let root = temp_dir("policy-refused");
    let data_dir = temp_dir("policy-refused-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &[], &["cargo test"], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("unused");

    let result = run_verification(&engine, &root, &session, &mut adapter);
    let report = result
        .verification_report
        .as_ref()
        .expect("a policy refusal still reports");

    assert!(result.patch_proposal.is_none());
    assert!(
        adapter.requests.is_empty(),
        "a policy refusal must never start a code repair"
    );
    assert!(report.repair_targets.is_empty());
    let group = provenance_group(report, FailureProvenance::EnvironmentFailure)
        .expect("a policy refusal is an environment failure");
    assert_eq!(group.checks.len(), 1);
    assert!(group.action.as_ref().unwrap().label.contains("environment"));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_worker_session_does_not_repair_an_unknown_failure() {
    let root = temp_dir("worker-unknown");
    let data_dir = temp_dir("worker-unknown-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change_with_declined_baseline(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    mark_worker(&engine, &session, &root);
    let mut adapter = MockModelAdapter::new("unused");

    let result = run_verification(&engine, &root, &session, &mut adapter);
    let report = result
        .verification_report
        .as_ref()
        .expect("a worker's no-repair verification reports");

    assert!(result.patch_proposal.is_none());
    assert!(
        adapter.requests.is_empty(),
        "a worker must not repair an unknown failure"
    );
    assert!(report.repair_targets.is_empty());
    assert!(
        provenance_group(report, FailureProvenance::UnknownProvenance).is_some(),
        "{report:?}"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn provenance_and_repair_targets_survive_a_fresh_store() {
    let root = temp_dir("provenance-reload");
    let data_dir = temp_dir("provenance-reload-data");
    cargofile(&root);
    let shell = repaired_shell(&root).to_string_lossy().to_string();
    fs::write(root.join("cargo-fail"), "fail\n").unwrap();
    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new("I cannot fix this.");
    let first = run_verification(&engine, &root, &session, &mut repair);
    assert!(
        first.patch_proposal.is_none(),
        "the repair answer proposed no patch"
    );

    // A new store over the same data dir is the worker/desktop reload path.
    let reopened = SessionStore::new(&data_dir);
    let events = reopened.read_verification_events(&session, &[]).unwrap();
    let state = fold_verification(events.iter());
    let mut hashes = BTreeMap::new();
    for run in state.checks.values() {
        hashes.extend(run.file_hashes.clone());
    }
    let report = VerificationReport::build(&state, &TaskPlan::new("task", 0), &[], &hashes);
    let group = provenance_group(&report, FailureProvenance::NewRegression)
        .expect("the durable classification survives the reload");
    assert_eq!(group.checks.len(), 1);
    assert!(!report.repair_targets.is_empty());
    assert!(!report.repair_targets[0].evidence.is_empty());
    assert!(report.render().contains("Introduced regressions"));
    // R6 (spec 102): redacted details are durable on the evidence, so the
    // reloaded repair request is not poorer than the live one.
    assert!(
        report
            .repair_request()
            .expect("the reloaded report still repairs")
            .contains("--> src/a.rs:1:1"),
        "the reloaded repair request keeps the finding's details: {}",
        report.repair_request().unwrap()
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// --- Repair scope and thin presentation (spec 102 Task 5) ------------------

/// A repair that stays inside the task's changed paths is ordinary: no
/// expansion fact and nothing for a card to warn about.
#[test]
fn a_repair_inside_the_task_scope_has_no_scope_review() {
    let root = temp_dir("repair-scope-inside");
    let data_dir = temp_dir("repair-scope-inside-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );

    let first = run_verification(&engine, &root, &session, &mut repair);
    let patch = first.patch_proposal.as_ref().expect("a repair patch");
    assert!(
        patch.repair_scope.is_none(),
        "a task-changed path is not an expansion: {:?}",
        patch.repair_scope
    );
    let stored = engine.patch_store.load(&patch.patch_id).expect("stored");
    assert!(stored.repair_scope.is_none());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A repair may touch a path the failing finding names even when the task
/// changed a different file. That path is in the existing scope, so neither
/// the live proposal nor the stored patch carries an expansion review.
#[test]
fn a_repair_may_touch_a_finding_path_outside_the_task_changed_paths() {
    let root = temp_dir("repair-scope-finding");
    let data_dir = temp_dir("repair-scope-finding-data");
    cargofile(&root);
    let shell = repaired_shell(&root).to_string_lossy().to_string();
    fs::write(root.join("cargo-fail"), "fail\n").unwrap();
    // The finding's location must resolve to an existing file, or the parser
    // keeps the message but drops the path and this scenario would prove
    // nothing about finding locations.
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = verify_engine_with_retry(&root, &data_dir, &shell, &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/app.rs", "let app = 1;\n", "let app = 2;\n")],
    );
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/a.rs", "let a = 3;\n")]],
    );

    let first = run_verification(&engine, &root, &session, &mut repair);
    let request = repair
        .requests
        .first()
        .expect("the repair turn called the model");
    let user = request
        .messages
        .iter()
        .find(|message| message.role == "user")
        .expect("a user message carries the request");
    assert!(
        user.content.contains("src/a.rs:1:1"),
        "the finding is in a different file from the task change: {}",
        user.content
    );
    let patch = first.patch_proposal.as_ref().expect("a repair patch");
    assert_eq!(patch.files.len(), 1);
    assert_eq!(patch.files[0].path, "src/a.rs");
    assert!(
        patch.repair_scope.is_none(),
        "a finding path is already in the repair scope: {:?}",
        patch.repair_scope
    );
    let stored = engine.patch_store.load(&patch.patch_id).expect("stored");
    assert!(
        stored.repair_scope.is_none(),
        "the stored patch carries no expansion either: {:?}",
        stored.repair_scope
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A new unprotected path is a typed one-shot expansion, and the stored patch
/// carries exactly the same review the live proposal returned.
#[test]
fn a_new_unprotected_repair_path_is_a_typed_one_shot_expansion() {
    let root = temp_dir("repair-scope-expand");
    let data_dir = temp_dir("repair-scope-expand-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/new.rs", "fn new() {}\n")]],
    );

    let first = run_verification(&engine, &root, &session, &mut repair);
    let patch = first.patch_proposal.as_ref().expect("a repair patch");
    let scope = patch
        .repair_scope
        .as_ref()
        .expect("a new path is an expansion");
    assert!(scope.one_shot);
    assert_eq!(scope.expansion_paths, vec!["src/new.rs".to_string()]);
    assert!(!scope.label.is_empty());
    assert!(!scope.explanation.is_empty());
    assert!(scope.reason_groups.iter().any(|group| {
        group.kind == workspace_engine::RepairScopeReasonKind::OutsideExistingScope
            && group.paths == vec!["src/new.rs".to_string()]
    }));
    assert_eq!(scope.actions.len(), 1);
    assert_eq!(
        scope.actions[0].kind,
        workspace_engine::RepairScopeActionKind::ApproveScopeExpansionOnce
    );

    let stored = engine.patch_store.load(&patch.patch_id).expect("stored");
    assert_eq!(
        stored.repair_scope.as_ref(),
        Some(scope),
        "a reload must render the same typed fact"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// Spec 98 outranks the scope review: a protected path refuses the whole
/// proposal, so no expansion card can exist for it.
#[test]
fn a_protected_repair_path_is_refused_without_a_patch() {
    let root = temp_dir("repair-scope-protected");
    let data_dir = temp_dir("repair-scope-protected-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    // Protection is canonicalised against an existing file or directory.
    write(&root, "src/new.rs", "fn old() {}\n");
    engine
        .add_task_protected_paths(&root, &session, &task, &["src/new.rs".to_string()])
        .expect("the task can protect a path");
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new()],
        vec![vec![patch_call("src/new.rs", "fn new() {}\n")], Vec::new()],
    );

    let result = run_verification(&engine, &root, &session, &mut repair);
    assert!(
        result.patch_proposal.is_none(),
        "a protected path never produces a review card"
    );
    let conversation = engine
        .session_store
        .read_conversation(&session)
        .expect("the conversation should read");
    assert!(
        conversation.messages.iter().any(|message| message
            .content
            .contains("Patch would modify protected task path")),
        "the refusal is visible in the turn: {:?}",
        conversation
            .messages
            .iter()
            .map(|message| &message.content)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fs::read_to_string(root.join("src/new.rs")).unwrap(),
        "fn old() {}\n",
        "a refused proposal writes nothing"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A worker's repair proposal and a reloaded/recovered card read the same
/// engine-owned expansion from the stored patch.
#[test]
fn a_worker_and_a_reload_preserve_the_same_scope_review() {
    let root = temp_dir("repair-scope-worker");
    let data_dir = temp_dir("repair-scope-worker-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    mark_worker(&engine, &session, &root);
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/new.rs", "fn new() {}\n")]],
    );

    let first = run_verification(&engine, &root, &session, &mut repair);
    let patch = first.patch_proposal.as_ref().expect("a repair patch");
    let live_scope = patch.repair_scope.clone().expect("an expansion");

    let reopened =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let stored = reopened.patch_store.load(&patch.patch_id).expect("stored");
    assert_eq!(stored.repair_scope.as_ref(), Some(&live_scope));

    let paused = PausedTurns::new(&data_dir);
    let reattached = reattach_pending_approvals(
        &reopened.session_store,
        &reopened.audit_log,
        &reopened.command_store,
        &reopened.patch_store,
        &paused,
        &session,
    )
    .expect("the pending approval should reattach");
    let recovered = reattached
        .iter()
        .find_map(|approval| match approval {
            ReattachedApproval::Patch { patch, .. } => Some(patch),
            _ => None,
        })
        .expect("the repair patch is reattached");
    assert_eq!(recovered.repair_scope.as_ref(), Some(&live_scope));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A card whose path became protected after the preview is refused at apply
/// time, and nothing is written even though the expansion review is stored.
#[test]
fn a_stale_expansion_card_newly_protected_is_refused_at_apply() {
    let root = temp_dir("repair-scope-stale");
    let data_dir = temp_dir("repair-scope-stale-data");
    cargofile(&root);
    let engine =
        verify_engine_with_retry(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[], 2);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut repair = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![patch_call("src/new.rs", "fn new() {}\n")]],
    );
    let first = run_verification(&engine, &root, &session, &mut repair);
    let patch = first.patch_proposal.as_ref().expect("a repair patch");
    assert!(patch.repair_scope.is_some());

    // The card was built before this file existed; it becomes protected only
    // now, so the apply-time recheck is the fence under test.
    write(&root, "src/new.rs", "fn someone_else() {}\n");
    engine
        .add_task_protected_paths(&root, &session, &first.task.id, &["src/new.rs".to_string()])
        .expect("the repair task can protect a path");
    let error = engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.patch_id, None, None, "tester", false)
        .expect_err("a newly protected path must refuse the stale card");
    assert!(error.to_string().contains("protected task path"), "{error}");
    assert_eq!(
        fs::read_to_string(root.join("src/new.rs")).unwrap(),
        "fn someone_else() {}\n",
        "a refused apply writes nothing"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// --- R15: the discovered-check floor at the completion gate ----------------
//
// Spec 101/105 as amended by review fix R15 (decision D1): a task that changed
// files has required checks even when its plan declares none. The floor is the
// detected checks of the owning and enclosing roots of every file the task
// touched. Plan-declared required checks are added to it, merged by `CheckId`;
// they never replace it.

/// (a) A plan with no check does not excuse the source change it validates.
/// The root's discovered `cargo test` is required, so a blocked (skipped) run
/// settles the task incomplete with the skip reason.
#[test]
fn a_changed_source_without_a_plan_check_requires_the_discovered_floor_check() {
    let root = temp_dir("floor-no-plan-check");
    let data_dir = temp_dir("floor-no-plan-check-data");
    cargofile(&root);
    // `cargo test` is refused by policy, so the run records a skip rather than
    // a pass. The floor must still treat it as an unmet obligation.
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &["cargo test"]);
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    // A closed plan that carries no structured check at all.
    open_plan(&engine, &session, &task, &[]);
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert_eq!(
        result.task.status,
        TaskStatus::Incomplete,
        "a skipped floor check must not close the task: {result:?}"
    );
    let settled = engine
        .session_store
        .read_completion_report(&session, &result.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert_eq!(
        settled.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckSkipped],
        "{settled:?}"
    );
    assert!(
        settled.reasons[0].explanation.contains("cargo test"),
        "{}",
        settled.reasons[0].explanation
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// (c) A plan check identical to a floor check is one obligation, not two.
/// The gate merges by `CheckId`, so the blocked check produces one reason.
#[test]
fn a_plan_check_identical_to_a_floor_check_is_one_obligation() {
    let root = temp_dir("floor-plan-duplicate");
    let data_dir = temp_dir("floor-plan-duplicate-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &["cargo test"]);
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut plan = complete_plan();
    plan.task_id = task.clone();
    plan.steps[0].check = Some(PlannedCheck {
        version: PLANNED_CHECK_VERSION,
        command: "cargo test".to_string(),
        working_directory: String::new(),
        purpose: "validates the final tree".to_string(),
        required: true,
    });
    let stored_task = engine
        .session_store
        .read_tasks(&session)
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.id == task)
        .expect("the patch task");
    engine
        .session_store
        .create_plan(&stored_task, &plan)
        .unwrap();
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let settled = engine
        .session_store
        .read_completion_report(&session, &result.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert_eq!(
        settled
            .reason_codes()
            .iter()
            .filter(|code| **code == CompletionReasonCode::RequiredCheckSkipped)
            .count(),
        1,
        "the same CheckId is one obligation: {settled:?}"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// (d) Documentation-only work selects no root, so it has no floor and still
/// completes normally even though `cargo test` would need approval.
#[test]
fn a_documentation_only_change_has_no_floor_and_completes() {
    let root = temp_dir("floor-docs-only");
    let data_dir = temp_dir("floor-docs-only-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    let (session, task) = applied_change(&engine, &root, &[("docs/readme.md", "old\n", "new\n")]);
    open_plan(&engine, &session, &task, &[]);
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert_eq!(result.task.status, TaskStatus::Complete, "{result:?}");
    let settled = engine
        .session_store
        .read_completion_report(&session, &result.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert!(settled.eligible, "{settled:?}");
    assert!(settled.reasons.is_empty(), "{settled:?}");
    assert!(settled.waivers.is_empty(), "{settled:?}");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// (e) Declining a floor check that needs approval is the named waiver, and
/// the task can still complete. The user's decision is never represented as a
/// pass.
#[test]
fn declining_a_floor_check_is_a_named_waiver_and_the_task_completes() {
    let root = temp_dir("floor-declined");
    let data_dir = temp_dir("floor-declined-data");
    cargofile(&root);
    // No allowlist: the floor's `cargo test` reaches the ordinary check card.
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &[]);
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    open_plan(&engine, &session, &task, &[]);
    let mut adapter = MockModelAdapter::new("");

    let paused = run_verification(&engine, &root, &session, &mut adapter);
    let batch = paused.check_batch.expect("the floor check needs approval");
    assert_eq!(
        batch.checks.len(),
        1,
        "only the root floor check is offered: {batch:?}"
    );
    assert_eq!(batch.checks[0].command, "cargo test");
    let decisions = batch
        .checks
        .iter()
        .map(|check| (check.id.clone(), false))
        .collect::<Vec<_>>();
    let result = resume_checks(&engine, &batch.id, &decisions, &mut adapter);

    assert_eq!(result.task.status, TaskStatus::Complete, "{result:?}");
    let settled = engine
        .session_store
        .read_completion_report(&session, &result.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert!(settled.eligible, "{settled:?}");
    assert!(settled.reasons.is_empty(), "{settled:?}");
    assert_eq!(settled.waivers.len(), 1, "{settled:?}");
    assert_eq!(
        settled.waivers[0].check,
        CheckId::new("", "cargo test"),
        "the waiver names the declined floor check"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// (e) A floor check the policy can auto-run simply runs; no card, no waiver.
#[test]
fn an_auto_runnable_floor_check_runs_without_a_waiver() {
    let root = temp_dir("floor-auto-run");
    let data_dir = temp_dir("floor-auto-run-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[]);
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    open_plan(&engine, &session, &task, &[]);
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert!(
        result.check_batch.is_none(),
        "an auto-run check needs no card"
    );
    assert_eq!(result.task.status, TaskStatus::Complete, "{result:?}");
    let settled = engine
        .session_store
        .read_completion_report(&session, &result.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert!(settled.eligible, "{settled:?}");
    assert!(settled.waivers.is_empty(), "{settled:?}");

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// (f) In a Cargo workspace, changing a member selects its enclosing workspace
/// root too, so that root's `cargo test` is part of the floor.
#[test]
fn a_member_change_puts_the_workspace_root_check_in_the_floor() {
    let root = temp_dir("floor-workspace-member");
    let data_dir = temp_dir("floor-workspace-member-data");
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/member\"]\n",
    );
    write(
        &root,
        "crates/member/Cargo.toml",
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &[], &["cargo test"]);
    let (session, task) = applied_change(
        &engine,
        &root,
        &[(
            "crates/member/src/lib.rs",
            "pub fn value() -> u8 { 1 }\n",
            "pub fn value() -> u8 { 2 }\n",
        )],
    );
    open_plan(&engine, &session, &task, &[]);
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert_eq!(result.task.status, TaskStatus::Incomplete, "{result:?}");
    let settled = engine
        .session_store
        .read_completion_report(&session, &result.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert!(
        settled
            .reasons
            .iter()
            .any(|reason| reason.explanation.contains("cargo test (repository root)")),
        "the workspace root check is a floor obligation: {settled:?}"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// --- Spec 109 Task 1: reuse a result for an unchanged tree (D6) -------------

fn hashes(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(path, hash)| ((*path).to_string(), (*hash).to_string()))
        .collect()
}

fn reusable(events: &[VerificationEvent], current: &[(&str, &str)], generation: u32) -> bool {
    let folded = state(events);
    let reuse = CheckReuse::from_state(&folded, &hashes(current), generation);
    folded.checks.keys().any(|check| reuse.can_reuse(check))
}

/// A recorded run stands for a new one only when it covered exactly the
/// current tree: the same generation, the same changed files with the same
/// hashes. A failed exit is reusable (as a failure); a run that did not exit,
/// a skip and an environment refusal are not.
#[test]
fn a_recorded_run_is_reusable_only_for_exactly_the_tree_it_covered() {
    let cargo = check("", "cargo test");
    let tree = [("src/a.rs", "h1")];
    let passed = [ran_at_generation(cargo.clone(), Some(0), &tree, 2)];
    assert!(reusable(&passed, &tree, 2), "same tree, same generation");
    assert!(!reusable(&passed, &tree, 3), "a new mutation generation");
    assert!(
        !reusable(&passed, &[("src/a.rs", "h2")], 2),
        "a changed file"
    );
    assert!(
        !reusable(&passed, &[("src/a.rs", "h1"), ("src/b.rs", "h3")], 2),
        "a file the task changed after the run is not covered by it"
    );
    assert!(!reusable(&passed, &[], 2), "a file the run covered is gone");

    let failed = [ran_at_generation(cargo.clone(), Some(101), &tree, 2)];
    assert!(
        reusable(&failed, &tree, 2),
        "a failed exit is reused as failed"
    );

    let signalled = [ran_at_generation(cargo.clone(), None, &tree, 2)];
    assert!(
        !reusable(&signalled, &tree, 2),
        "killed or signalled reruns"
    );

    let cancelled = [VerificationEvent::Check {
        task_id: "task".to_string(),
        check: cargo.clone(),
        outcome: CheckOutcome::Ran {
            exit_code: None,
            marker_id: "marker".to_string(),
            termination: Some(workspace_engine::CheckTermination::Cancelled),
        },
        file_hashes: hashes(&tree),
        generation: 2,
    }];
    assert!(!reusable(&cancelled, &tree, 2), "a cancelled run reruns");

    let declined = [VerificationEvent::Check {
        task_id: "task".to_string(),
        check: cargo.clone(),
        outcome: CheckOutcome::Skipped(SkipReason::DeclinedByUser),
        file_hashes: hashes(&tree),
        generation: 2,
    }];
    assert!(
        !reusable(&declined, &tree, 2),
        "a declined check asks again"
    );

    let environment = [VerificationEvent::Check {
        task_id: "task".to_string(),
        check: cargo.clone(),
        outcome: CheckOutcome::Environment {
            termination: workspace_engine::CheckTermination::MissingExecutable,
        },
        file_hashes: hashes(&tree),
        generation: 2,
    }];
    assert!(
        !reusable(&environment, &tree, 2),
        "an environment refusal reruns"
    );

    let browser = [ran_at_generation(
        CheckId::browser("http://localhost:3000"),
        Some(0),
        &tree,
        2,
    )];
    assert!(
        !reusable(&browser, &tree, 2),
        "a page can change without a file"
    );
}

/// A reused check is removed before anything is proposed, so it is neither
/// stored as a proposal nor run, and the selection names it as reused.
#[test]
fn a_reused_check_is_never_proposed() {
    let repo = Monorepo::new("reuse-selection");
    let engine = repo.engine();
    let map = repo.map(&engine);
    let changed = vec!["crates/engine/src/lib.rs".to_string()];

    let Selection::Proposed(all) = propose_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &changed,
        &mut CheckReuse::none(),
    )
    .unwrap() else {
        panic!("the member has checks");
    };
    let before = repo.pending_proposals();
    let reused_check = all[0].0.clone();

    let recorded = state(&[ran_with_hashes(reused_check.clone(), Some(0), &[])]);
    let mut reuse = CheckReuse::from_state(&recorded, &BTreeMap::new(), 0);
    let selection = propose_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &changed,
        &mut reuse,
    )
    .unwrap();
    let Selection::Proposed(proposed) = selection else {
        panic!("reusing every check is still a selection, got {selection:?}");
    };
    assert!(proposed.iter().all(|(check, _)| check != &reused_check));
    assert_eq!(proposed.len(), all.len() - 1);
    assert_eq!(reuse.reused(), std::slice::from_ref(&reused_check));
    assert_eq!(
        repo.pending_proposals(),
        before + proposed.len(),
        "nothing is stored for the reused check"
    );
}

/// Spec 109 D4 with D6: the plan's scoped check standing in for a root's
/// detected check is reused like any other, so an unchanged tree runs nothing.
#[test]
fn a_scoped_plan_check_is_reused_for_an_unchanged_tree() {
    let repo = Monorepo::new("reuse-scoped");
    let engine = repo.engine();
    let map = repo.map(&engine);
    let changed = vec!["crates/engine/src/lib.rs".to_string()];
    let scoped = CheckId::new("", "cargo nextest run -p engine --test a");
    let before = repo.pending_proposals();

    let recorded = state(&[ran_with_hashes(scoped.clone(), Some(0), &[])]);
    let mut reuse = CheckReuse::from_state(&recorded, &BTreeMap::new(), 0);
    let selection = propose_scoped_checks(
        &engine.validation_orchestrator,
        &repo.root,
        &map,
        &changed,
        &scoped_selection(
            &map,
            &changed,
            std::slice::from_ref(&scoped),
            std::slice::from_ref(&scoped),
        ),
        &|_| false,
        &mut reuse,
    )
    .unwrap();
    assert_eq!(selection, Selection::Proposed(Vec::new()));
    assert_eq!(reuse.reused(), std::slice::from_ref(&scoped));
    assert_eq!(repo.pending_proposals(), before, "nothing is stored");
}

/// The fold and the report: a reused check keeps its recorded run (the gate's
/// evidence is untouched) and the report names it as reused.
#[test]
fn a_reused_check_keeps_its_run_and_the_report_says_reused() {
    let cargo = check("", "cargo test");
    let first = vec![
        due(),
        started(0, std::slice::from_ref(&cargo)),
        ran(cargo.clone(), Some(0)),
        finished(VerificationOutcome::Passed),
    ];
    let before = state(&first);
    let mut events = first.clone();
    events.extend([
        due(),
        started(1, &[]),
        VerificationEvent::Reused {
            task_id: "task".to_string(),
            check: cargo.clone(),
        },
        finished(VerificationOutcome::Passed),
    ]);
    let after = state(&events);
    assert_eq!(after.checks, before.checks, "the recorded run is unchanged");
    assert!(after.reused.contains(&cargo));

    let report = report_for(&events);
    assert_eq!(report.passed, vec![cargo.clone()]);
    assert_eq!(report.reused, vec![cargo.clone()]);
    let text = report.render();
    assert!(text.contains("Checks reused"), "{text}");
    assert!(
        !text.contains("all ran"),
        "a reused check did not run: {text}"
    );

    // A later run of the same check is no longer reused.
    events.extend([
        started(2, std::slice::from_ref(&cargo)),
        ran(cargo.clone(), Some(0)),
    ]);
    assert!(report_for(&events).reused.is_empty());
    assert!(!report_for(&first).render().contains("Checks reused"));

    let json = serde_json::to_value(VerificationEvent::Reused {
        task_id: "task".to_string(),
        check: cargo,
    })
    .unwrap();
    assert_eq!(json["kind"], "verification_reused");
    assert_eq!(json["taskId"], "task");
    assert_eq!(json["check"]["command"], "cargo test");
}

fn check_events(engine: &WorkspaceEngine, session_id: &str) -> usize {
    outcome_events(engine, session_id)
        .iter()
        .filter(|event| matches!(event, VerificationEvent::Check { .. }))
        .count()
}

fn reused_events(engine: &WorkspaceEngine, session_id: &str) -> Vec<CheckId> {
    outcome_events(engine, session_id)
        .into_iter()
        .filter_map(|event| match event {
            VerificationEvent::Reused { check, .. } => Some(check),
            _ => None,
        })
        .collect()
}

/// Make the patch's verification due again with nothing changed, as recovery
/// does for a verification it offers again.
fn due_again(engine: &WorkspaceEngine, session_id: &str) {
    let due = outcome_events(engine, session_id)
        .into_iter()
        .find(|event| matches!(event, VerificationEvent::Due { .. }))
        .expect("the patch recorded a due");
    engine
        .session_store
        .append_verification_event(session_id, &due)
        .expect("the due records");
}

/// Spec 109 requirement 5: a second verification of an unchanged tree runs
/// nothing, records the reuse, and reports it; the completion gate reads the
/// same `CheckRun` it read after the first run.
#[test]
fn a_second_verification_of_an_unchanged_tree_runs_nothing() {
    let root = temp_dir("reuse-unchanged");
    let data_dir = temp_dir("reuse-unchanged-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");
    let first = run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(
        first.verification_report.as_ref().unwrap().outcome,
        Some(VerificationOutcome::Passed)
    );
    assert_eq!(check_events(&engine, &session), 1);
    let gate_evidence = fold_verification(outcome_events(&engine, &session).iter()).checks;

    due_again(&engine, &session);
    let second = run_verification(&engine, &root, &session, &mut adapter);

    assert!(second.check_batch.is_none(), "nothing to approve");
    assert_eq!(check_events(&engine, &session), 1, "nothing ran again");
    let cargo = CheckId::new("", "cargo test");
    assert_eq!(reused_events(&engine, &session), vec![cargo.clone()]);
    let report = second.verification_report.as_ref().expect("a report");
    assert_eq!(report.outcome, Some(VerificationOutcome::Passed));
    assert_eq!(report.passed, vec![cargo.clone()]);
    assert_eq!(report.reused, vec![cargo]);
    assert!(
        second.response.contains("Checks reused"),
        "{}",
        second.response
    );
    assert_eq!(
        fold_verification(outcome_events(&engine, &session).iter()).checks,
        gate_evidence,
        "the gate reads the original run"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// Any change to a file the run covered makes the check run again.
#[test]
fn a_changed_file_makes_the_check_run_again() {
    let root = temp_dir("reuse-changed");
    let data_dir = temp_dir("reuse-changed-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/true", &["cargo test"], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");
    run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(check_events(&engine, &session), 1);

    write(&root, "src/a.rs", "let a = 3;\n");
    due_again(&engine, &session);
    let second = run_verification(&engine, &root, &session, &mut adapter);

    assert_eq!(check_events(&engine, &session), 2, "the check ran again");
    assert!(reused_events(&engine, &session).is_empty());
    assert!(second.verification_report.unwrap().reused.is_empty());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

/// A failed recorded run is reused as that failure — never as a pass.
#[test]
fn a_failed_run_is_reused_as_failed() {
    let root = temp_dir("reuse-failed");
    let data_dir = temp_dir("reuse-failed-data");
    cargofile(&root);
    let engine = verify_engine(&root, &data_dir, "/usr/bin/false", &["cargo test"], &[]);
    let (session, _task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    let mut adapter = MockModelAdapter::new("");
    let first = run_verification(&engine, &root, &session, &mut adapter);
    let first_report = first.verification_report.expect("a report");
    assert!(first_report.passed.is_empty());
    assert_eq!(check_events(&engine, &session), 1);

    due_again(&engine, &session);
    let second = run_verification(&engine, &root, &session, &mut adapter);

    let cargo = CheckId::new("", "cargo test");
    assert_eq!(check_events(&engine, &session), 1, "nothing ran again");
    assert_eq!(reused_events(&engine, &session), vec![cargo.clone()]);
    let report = second.verification_report.expect("a report");
    assert!(report.passed.is_empty(), "{report:?}");
    assert!(
        report.failed.iter().any(|failed| failed.check == cargo),
        "{report:?}"
    );
    assert_eq!(report.reused, vec![cargo]);
    assert_ne!(report.outcome, Some(VerificationOutcome::Passed));
    assert!(!second.response.contains("Verification complete"));

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
}

// Spec 109 D10: a build check after every change.

/// A fake shell for build checks. Every command it is asked to run is logged
/// one per line to `log`; a command naming `cargo check` or `make build` prints
/// `stderr` (when given) and exits 101, and anything else exits 0.
fn build_shell(dir: &std::path::Path, stderr: Option<&str>) -> (PathBuf, PathBuf) {
    let shell = dir.join("build-shell.sh");
    let log = dir.join("commands.log");
    let failure = match stderr {
        Some(text) => {
            let path = dir.join("build-stderr.txt");
            fs::write(&path, text).unwrap();
            format!("cat '{}' >&2; exit 101", path.display())
        }
        None => "exit 0".to_string(),
    };
    let script = format!(
        "#!/bin/sh\n\
         command=\"$2\"\n\
         printf '%s\\n' \"$command\" >> '{}'\n\
         case \"$command\" in\n\
           *\"cargo check\"*|*\"make build\"*) {failure} ;;\n\
         esac\n\
         exit 0\n",
        log.display()
    );
    fs::write(&shell, script).unwrap();
    let mut permissions = fs::metadata(&shell).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&shell, permissions).unwrap();
    (shell, log)
}

fn logged_commands(log: &std::path::Path, needle: &str) -> usize {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(needle))
        .count()
}

fn build_events(engine: &WorkspaceEngine, session_id: &str) -> Vec<VerificationEvent> {
    outcome_events(engine, session_id)
        .into_iter()
        .filter(|event| matches!(event, VerificationEvent::Build { .. }))
        .collect()
}

/// Seven rustc errors: two in the changed `src/a.rs`, one in `src/other.rs`,
/// which the task never changed, then four more in `src/a.rs`.
fn seven_rust_errors() -> String {
    let mut text = String::from("    Checking demo v0.1.0 (/repo)\n");
    for index in 0..7 {
        let path = if index == 1 {
            "src/other.rs"
        } else {
            "src/a.rs"
        };
        text.push_str(&format!(
            "error[E0425]: cannot find function `missing_{index}` in this scope\n --> {path}:1:9\n  |\n1 | let a = missing_{index}();\n  |         ^^^^^^^^^ not found in this scope\n\n"
        ));
    }
    text.push_str("error: could not compile `demo` (lib) due to 7 previous errors\n");
    text
}

/// A plan whose first step is in progress with `build` as its build check,
/// approved, so the verification reads the step's build check.
fn approved_plan_with_build(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
    build: Option<PlannedCheck>,
    check: Option<PlannedCheck>,
) {
    let task = engine
        .session_store
        .read_tasks(session_id)
        .unwrap()
        .into_iter()
        .find(|task| task.id == task_id)
        .expect("the patch's task");
    let mut plan = TaskPlan::new(task_id, 1);
    for (index, title) in ["Change the code", "Document it"].iter().enumerate() {
        plan.steps.push(PlanStep {
            id: format!("step_{}", index + 1),
            title: (*title).to_string(),
            detail: None,
            status: if index == 0 {
                StepStatus::InProgress
            } else {
                StepStatus::Pending
            },
            depends_on: Vec::new(),
            started_at_ms: (index == 0).then_some(1),
            completed_at_ms: None,
            evidence: Vec::new(),
            check: if index == 0 { check.clone() } else { None },
            build: if index == 0 { build.clone() } else { None },
        });
    }
    engine.session_store.create_plan(&task, &plan).unwrap();
    engine.session_store.approve_plan(&task, "tester").unwrap();
}

fn planned(command: &str, directory: &str, required: bool) -> PlannedCheck {
    PlannedCheck {
        version: PLANNED_CHECK_VERSION,
        command: command.to_string(),
        working_directory: directory.to_string(),
        purpose: "test".to_string(),
        required,
    }
}

/// A failing build ends the verification `build_failed` before any check is
/// selected, takes no baseline, and its errors reach the next model request:
/// the first five in full, a count of the rest, and the error in a file the
/// task never changed marked as such. A step with no build check on a Cargo
/// root uses the fast path.
#[test]
fn a_failing_build_reaches_the_next_model_request_and_takes_no_baseline() {
    let root = temp_dir("build-fails");
    let data_dir = temp_dir("build-fails-data");
    let tools = temp_dir("build-fails-tools");
    cargofile(&root);
    write(&root, "src/other.rs", "pub fn other() {}\n");
    let (shell, log) = build_shell(&tools, Some(&seven_rust_errors()));
    let engine = verify_engine(
        &root,
        &data_dir,
        &shell.to_string_lossy(),
        &["cargo test", "cargo check --tests"],
        &[],
    );
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    open_plan(&engine, &session, &task, &["Keep working"]);
    let mut adapter = MockModelAdapter::new("I will fix the build.");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert!(result.check_batch.is_none(), "no card for a build failure");
    let report = result.verification_report.as_ref().expect("a report");
    assert_eq!(report.outcome, Some(VerificationOutcome::BuildFailed));
    let fast_path = CheckId::new("", "cargo check --tests");
    assert_eq!(report.builds.len(), 1, "{report:?}");
    assert_eq!(report.builds[0].check, fast_path);
    assert_eq!(report.builds[0].status, "does_not_build");
    assert_eq!(report.builds[0].label, "does not build: 7 errors");
    let errors = report.builds[0].errors.as_ref().expect("the errors");
    assert_eq!(errors.total, 7);
    assert_eq!(errors.shown.len(), 5);
    assert!(!errors.shown[0].outside_task, "src/a.rs is the task's");
    assert_eq!(errors.shown[1].path.as_deref(), Some("src/other.rs"));
    assert!(errors.shown[1].outside_task, "src/other.rs is not");
    assert!(report.render().contains("Build"), "{}", report.render());

    // Nothing else was selected or run, and no baseline was taken. The
    // fixture's own task-start baseline is written before the verification
    // starts, so only what follows `Started` is the verification's.
    assert_eq!(logged_commands(&log, "cargo check --tests"), 1);
    assert_eq!(logged_commands(&log, "cargo test"), 0, "no check ran");
    let all = outcome_events(&engine, &session);
    let start = all
        .iter()
        .position(|event| matches!(event, VerificationEvent::Started { .. }))
        .expect("the verification started");
    let events = all[start..].to_vec();
    assert!(
        !events.iter().any(|event| matches!(
            event,
            VerificationEvent::Check { .. } | VerificationEvent::Baseline { .. }
        )),
        "{events:?}"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        VerificationEvent::Started { checks, .. } if checks.is_empty()
    )));

    // The continuation request carries the errors.
    let prompt = first_user_message(&adapter);
    assert!(prompt.contains("Your change does not build"), "{prompt}");
    assert!(
        prompt.contains("src/a.rs:1: cannot find function `missing_0`"),
        "{prompt}"
    );
    assert!(
        prompt.contains(
            "src/other.rs:1: cannot find function `missing_1` in this scope (in a file this task has not changed"
        ),
        "{prompt}"
    );
    assert!(prompt.contains("…and 2 more errors not shown."), "{prompt}");
    assert!(
        !prompt.contains("missing_5"),
        "only the first errors in full"
    );
    assert_ne!(result.task.status, TaskStatus::Complete);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
    let _ = fs::remove_dir_all(&tools);
}

/// The step's own build check runs instead of the fast path, and a passing
/// build is never evidence: no `Check` event, no `CheckRun`, no plan step
/// evidence, even when the build command is also the step's required check.
#[test]
fn a_passing_build_is_never_gate_or_complete_step_evidence() {
    let root = temp_dir("build-not-evidence");
    let data_dir = temp_dir("build-not-evidence-data");
    let tools = temp_dir("build-not-evidence-tools");
    cargofile(&root);
    let (shell, log) = build_shell(&tools, None);
    let engine = verify_engine(
        &root,
        &data_dir,
        &shell.to_string_lossy(),
        &["cargo test", "make build"],
        &[],
    );
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    approved_plan_with_build(
        &engine,
        &session,
        &task,
        Some(planned("make build", "", false)),
        Some(planned("make build", "", true)),
    );
    let mut adapter = MockModelAdapter::new("Continuing.");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    let report = result.verification_report.as_ref().expect("a report");
    let build = CheckId::new("", "make build");
    assert_eq!(report.builds.len(), 1, "{report:?}");
    assert_eq!(
        report.builds[0].check, build,
        "the step's build, not the fast path"
    );
    assert_eq!(report.builds[0].label, "builds");
    assert_eq!(logged_commands(&log, "make build"), 1);
    assert_eq!(logged_commands(&log, "cargo check"), 0);
    assert!(
        report
            .render()
            .contains("make build (repository root) — builds")
            || report.render().contains("— builds"),
        "{}",
        report.render()
    );

    let state = fold_verification(outcome_events(&engine, &session).iter());
    assert!(!state.checks.contains_key(&build), "never a CheckRun");
    assert!(!state.selected.contains(&build));
    assert!(state.builds.contains_key(&build));
    assert!(!report.passed.contains(&build));
    for plan_task in engine.session_store.read_tasks(&session).unwrap() {
        if let Some(plan) = engine
            .session_store
            .read_task_plan(&session, &plan_task.id)
            .unwrap()
        {
            for step in &plan.steps {
                assert!(
                    !step
                        .evidence
                        .iter()
                        .any(|evidence| matches!(evidence, Evidence::CommandExit { .. })),
                    "no plan step holds the build's exit: {step:?}"
                );
            }
        }
    }

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
    let _ = fs::remove_dir_all(&tools);
}

/// An unchanged tree builds once: the second verification reuses the build
/// (D6) and says so; a changed file builds again.
#[test]
fn an_unchanged_tree_builds_once() {
    let root = temp_dir("build-once");
    let data_dir = temp_dir("build-once-data");
    let tools = temp_dir("build-once-tools");
    cargofile(&root);
    let (shell, log) = build_shell(&tools, None);
    let engine = verify_engine(
        &root,
        &data_dir,
        &shell.to_string_lossy(),
        &["cargo test", "cargo check --tests"],
        &[],
    );
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
    );
    open_plan(&engine, &session, &task, &["Keep working"]);
    let mut adapter = MockModelAdapter::new("");
    run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(logged_commands(&log, "cargo check --tests"), 1);

    due_again(&engine, &session);
    let second = run_verification(&engine, &root, &session, &mut adapter);

    assert_eq!(
        logged_commands(&log, "cargo check --tests"),
        1,
        "built once"
    );
    let report = second.verification_report.as_ref().expect("a report");
    assert_eq!(report.builds.len(), 1);
    assert!(report.builds[0].reused, "{report:?}");
    assert!(
        build_events(&engine, &session)
            .iter()
            .any(|event| matches!(event, VerificationEvent::Build { reused: true, .. }))
    );

    write(&root, "src/a.rs", "let a = 3;\n");
    due_again(&engine, &session);
    run_verification(&engine, &root, &session, &mut adapter);
    assert_eq!(
        logged_commands(&log, "cargo check --tests"),
        2,
        "a change builds again"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
    let _ = fs::remove_dir_all(&tools);
}

/// A root with no detected Cargo manifest and no build check from its plan
/// runs no build: no language list adds one (D9).
#[test]
fn a_non_cargo_root_without_a_build_check_runs_none() {
    let root = temp_dir("build-none");
    let data_dir = temp_dir("build-none-data");
    let tools = temp_dir("build-none-tools");
    let (shell, log) = build_shell(&tools, None);
    let engine = verify_engine(&root, &data_dir, &shell.to_string_lossy(), &[], &[]);
    let (session, task) = applied_change(
        &engine,
        &root,
        &[("src/a.hs", "main = pure ()\n", "main = print 1\n")],
    );
    open_plan(&engine, &session, &task, &["Keep working"]);
    let mut adapter = MockModelAdapter::new("");

    let result = run_verification(&engine, &root, &session, &mut adapter);

    assert!(build_events(&engine, &session).is_empty());
    assert_eq!(logged_commands(&log, "cargo check"), 0);
    let report = result.verification_report.as_ref().expect("a report");
    assert!(report.builds.is_empty());
    assert!(!report.render().contains("Build "), "{}", report.render());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&data_dir);
    let _ = fs::remove_dir_all(&tools);
}

/// A build check that needs approval parks the verification on a build card
/// before anything else is proposed; approving it runs the build and then the
/// change's checks in the same verification. Declining it skips only the build.
#[test]
fn a_build_that_needs_approval_asks_first_and_the_verification_continues() {
    for approve in [true, false] {
        let root = temp_dir("build-card");
        let data_dir = temp_dir("build-card-data");
        let tools = temp_dir("build-card-tools");
        cargofile(&root);
        let (shell, log) = build_shell(&tools, None);
        let engine = verify_engine(
            &root,
            &data_dir,
            &shell.to_string_lossy(),
            &["cargo test"],
            &[],
        );
        let (session, task) = applied_change(
            &engine,
            &root,
            &[("src/a.rs", "let a = 1;\n", "let a = 2;\n")],
        );
        open_plan(&engine, &session, &task, &["Keep working"]);
        let mut adapter = MockModelAdapter::new("");

        let paused = run_verification(&engine, &root, &session, &mut adapter);
        let card = paused.check_batch.as_ref().expect("a build card");
        assert_eq!(card.title.as_deref(), Some("Build check"));
        assert_eq!(card.checks.len(), 1);
        assert_eq!(card.checks[0].command, "cargo check --tests");
        assert_eq!(
            logged_commands(&log, "cargo"),
            0,
            "nothing ran before the answer"
        );
        assert!(
            !outcome_events(&engine, &session)
                .iter()
                .any(|event| matches!(event, VerificationEvent::Started { .. })),
            "no check was selected before the build was decided"
        );

        let decisions = vec![(card.checks[0].id.clone(), approve)];
        let resumed = resume_checks(&engine, &card.id, &decisions, &mut adapter);

        let report = resumed.verification_report.as_ref().expect("a report");
        assert_eq!(report.builds.len(), 1);
        if approve {
            assert_eq!(report.builds[0].label, "builds");
            assert_eq!(logged_commands(&log, "cargo check --tests"), 1);
        } else {
            assert_eq!(report.builds[0].status, "not_run");
            assert_eq!(logged_commands(&log, "cargo check --tests"), 0);
        }
        assert_eq!(
            report.passed,
            vec![CheckId::new("", "cargo test")],
            "{report:?}"
        );
        assert_eq!(logged_commands(&log, "cargo test"), 1);

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&data_dir);
        let _ = fs::remove_dir_all(&tools);
    }
}

/// The build event's wire shape, the fold, and the report's "Build" row.
#[test]
fn a_build_event_folds_into_the_report_and_never_into_the_checks() {
    let build = CheckId::new("crates/demo", "cargo check --tests");
    let errors = workspace_engine::BuildErrors {
        total: 3,
        shown: vec![workspace_engine::BuildError {
            summary: "mismatched types".to_string(),
            details: None,
            path: Some("crates/demo/src/lib.rs".to_string()),
            line: Some(8),
            outside_task: false,
        }],
    };
    let event = VerificationEvent::Build {
        task_id: "task".to_string(),
        check: build.clone(),
        outcome: CheckOutcome::Ran {
            exit_code: Some(101),
            marker_id: "cmd_1".to_string(),
            termination: None,
        },
        file_hashes: BTreeMap::new(),
        generation: 0,
        errors: Some(errors),
        reused: false,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "verification_build");
    assert_eq!(json["errors"]["total"], 3);
    assert!(json.get("reused").is_none(), "false is omitted");
    let back: VerificationEvent = serde_json::from_value(json).unwrap();
    assert_eq!(back, event);

    let events = [
        started(0, &[]),
        event,
        finished(VerificationOutcome::BuildFailed),
    ];
    let state = state(&events);
    assert!(state.checks.is_empty() && state.selected.is_empty());
    let report = report_for(&events);
    assert_eq!(report.builds[0].label, "does not build: 3 errors");
    assert!(report.failed.is_empty(), "a build is not a failed check");
    assert!(
        report
            .summary(&complete_plan_report())
            .starts_with("The change does not build: cargo check --tests (crates/demo) reported 3 errors. No other check ran."),
        "{}",
        report.summary(&complete_plan_report())
    );
    let rendered = report.render();
    assert!(rendered.contains("Build"), "{rendered}");
    assert!(rendered.contains("does not build: 3 errors"), "{rendered}");
    let prompt = report
        .build_failure_prompt()
        .expect("a build failure prompt");
    assert!(
        prompt.contains("1. crates/demo/src/lib.rs:8: mismatched types"),
        "{prompt}"
    );
    assert!(prompt.contains("…and 2 more errors not shown."), "{prompt}");

    // A later `Started` clears the attempt's builds but keeps them for reuse.
    let mut later = events.to_vec();
    later.push(started(1, &[]));
    let state = fold_verification(later.iter());
    assert!(state.build_results.is_empty());
    assert!(state.builds.contains_key(&build));
}
