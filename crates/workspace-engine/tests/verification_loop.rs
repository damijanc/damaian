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
    CancelToken, CheckId, CheckOutcome, CommandPurpose, Config, Evidence, HookConfig, HookEvent,
    HookMode, HookSource, MockModelAdapter, PlanReport, PlanStep, Selection, SkipReason, TaskPlan,
    TaskStatus, TurnProgress, TurnSink, VerificationEvent, VerificationOutcome, VerificationReport,
    VerificationState, WorkspaceEngine, fold_verification, is_documentation, propose_checks,
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

#[test]
fn a_member_change_selects_its_own_root_and_every_enclosing_root() {
    let repo = Monorepo::new("member");
    let engine = repo.engine();
    let map = repo.map(&engine);
    let changed = vec!["crates/engine/src/lib.rs".to_string()];

    assert_eq!(
        select_roots(&map, &changed),
        vec!["".to_string(), "crates/engine".to_string()]
    );

    let selection =
        propose_checks(&engine.validation_orchestrator, &repo.root, &map, &changed).unwrap();
    let Selection::Proposed(checks) = selection else {
        panic!("expected proposed checks, got {selection:?}");
    };
    let directories: Vec<&str> = checks
        .iter()
        .map(|(check, _)| check.working_directory.as_str())
        .collect();
    assert!(directories.contains(&""));
    assert!(directories.contains(&"crates/engine"));
    assert!(!directories.contains(&"crates/cli"));
    assert!(!directories.contains(&"packages/web"));
    for (check, proposal) in &checks {
        if check.working_directory == "crates/engine" {
            assert!(proposal.working_directory.ends_with("crates/engine"));
        }
    }
}

#[test]
fn a_web_change_does_not_duplicate_the_test_script() {
    let repo = Monorepo::new("web");
    let engine = repo.engine();
    let map = repo.map(&engine);
    let changed = vec!["packages/web/src/index.ts".to_string()];

    assert_eq!(
        select_roots(&map, &changed),
        vec!["".to_string(), "packages/web".to_string()]
    );

    let selection =
        propose_checks(&engine.validation_orchestrator, &repo.root, &map, &changed).unwrap();
    let Selection::Proposed(checks) = selection else {
        panic!("expected proposed checks, got {selection:?}");
    };
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
            &["docs/guide.md".to_string()]
        )
        .unwrap(),
        Selection::OnlyDocumentation
    );
    assert!(select_roots(&map, &["docs/guide.md".to_string()]).is_empty());

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
    let selection =
        propose_checks(&engine.validation_orchestrator, &repo.root, &map, &changed).unwrap();
    let Selection::Proposed(checks) = selection else {
        panic!("expected proposed checks, got {selection:?}");
    };
    assert_eq!(
        checks.len(),
        2,
        "the repository root and crates/engine each propose cargo test"
    );
    assert_eq!(
        repo.pending_proposals() - before,
        2,
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
    fs::create_dir_all(root).unwrap();
    WorkspaceEngine::new(Config {
        data_dir: data_dir.to_path_buf(),
        enable_index_watcher: false,
        shell: shell.to_string(),
        command_allowlist: allow.iter().map(|entry| (*entry).to_string()).collect(),
        command_blocklist: block.iter().map(|entry| (*entry).to_string()).collect(),
        ..Config::default()
    })
}

fn cargofile(root: &std::path::Path) {
    write(root, "Cargo.toml", "[workspace]\nmembers = []\n");
}

fn run_verification(
    engine: &WorkspaceEngine,
    root: &std::path::Path,
    session_id: &str,
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
        .verify_due(root, session_id, adapter, &mut sink)
        .expect("the verification turn should run")
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
    for (path, before, _) in files {
        write(root, path, before);
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
    assert_eq!(resumed.task.status, TaskStatus::Complete);

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
    assert_eq!(report.outcome, Some(VerificationOutcome::Failed));
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
        Some(VerificationOutcome::Failed)
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
