//! Spec 102 Task 1: durable evidence and provenance data.
//!
//! The fingerprint is a promise about identity: equivalent redacted evidence
//! must hash the same, and a different finding or termination must not. These
//! tests exercise that promise without a model, a command or a repair turn.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use workspace_engine::finding::{Finding, FindingDraft, FindingSource, Severity, SourceRange};
use workspace_engine::{
    CheckBaseline, CheckId, CheckProvenanceRecord, CheckTermination, FailureFingerprint,
    FailureProvenance, FindingEvidence, MAX_REPAIR_STATE_ENTRIES, RepairStateEntry, SecretScanner,
    SessionStore, VerificationEvent, VerificationState, fold_verification,
};

fn finding(summary: &str, path: Option<&str>, line: Option<u32>) -> Finding {
    Finding::new(
        FindingDraft {
            source: FindingSource::Test,
            severity: Severity::Error,
            summary: summary.to_string(),
            details: Some("raw output that must not be fingerprinted".to_string()),
            range: path.map(|path| SourceRange {
                path: path.to_string(),
                start_line: line.unwrap_or(1),
                start_column: Some(7),
                end_line: None,
                end_column: None,
            }),
            code: Some("E0308".to_string()),
        },
        &SecretScanner::default(),
    )
}

fn evidence(summary: &str, path: Option<&str>, line: Option<u32>) -> FindingEvidence {
    FindingEvidence::from_finding(&finding(summary, path, line))
}

fn check() -> CheckId {
    CheckId::new("crates/engine", "cargo test")
}

fn exited(code: i32) -> CheckTermination {
    CheckTermination::Exited { exit_code: code }
}

fn fingerprint(findings: &[FindingEvidence], termination: &CheckTermination) -> FailureFingerprint {
    FailureFingerprint::compute(&check(), findings, termination)
}

#[test]
fn fingerprint_is_order_and_duplicate_insensitive() {
    let a = evidence("mismatched types", Some("src/a.rs"), Some(8));
    let b = evidence("unused variable", Some("src/b.rs"), Some(12));

    let forward = fingerprint(&[a.clone(), b.clone()], &exited(1));
    let reversed = fingerprint(&[b.clone(), a.clone()], &exited(1));
    let repeated = fingerprint(&[a.clone(), b.clone(), a], &exited(1));

    assert_eq!(forward, reversed, "finding order is not identity");
    assert_eq!(forward, repeated, "a duplicate report is not a new failure");
}

#[test]
fn fingerprint_changes_with_material_evidence() {
    let original = evidence("mismatched types", Some("src/a.rs"), Some(8));
    let different_summary = evidence("missing field", Some("src/a.rs"), Some(8));
    let different_location = evidence("mismatched types", Some("src/a.rs"), Some(9));
    let different_path = evidence("mismatched types", Some("src/b.rs"), Some(8));

    let base = fingerprint(std::slice::from_ref(&original), &exited(1));
    assert_ne!(base, fingerprint(&[different_summary], &exited(1)));
    assert_ne!(base, fingerprint(&[different_location], &exited(1)));
    assert_ne!(base, fingerprint(&[different_path], &exited(1)));
}

#[test]
fn fingerprint_changes_with_termination_and_check_identity() {
    let found = evidence("mismatched types", Some("src/a.rs"), Some(8));

    let exit_one = fingerprint(std::slice::from_ref(&found), &exited(1));
    let exit_two = fingerprint(std::slice::from_ref(&found), &exited(2));
    let timed_out = fingerprint(std::slice::from_ref(&found), &CheckTermination::TimedOut);
    let other_check = FailureFingerprint::compute(
        &CheckId::new("", "cargo test"),
        std::slice::from_ref(&found),
        &exited(1),
    );

    assert_ne!(exit_one, exit_two, "a different exit code is observable");
    assert_ne!(exit_one, timed_out, "termination kind is identity");
    assert_ne!(
        exit_one, other_check,
        "the same command in another root differs"
    );
}

#[test]
fn fingerprint_ignores_transient_finding_fields() {
    let first = Finding::new(
        FindingDraft {
            source: FindingSource::Test,
            severity: Severity::Error,
            summary: "mismatched types".to_string(),
            details: Some("raw stdout from the first run".to_string()),
            range: None,
            code: Some("E0308".to_string()),
        },
        &SecretScanner::default(),
    );
    let second = Finding::new(
        FindingDraft {
            source: FindingSource::Test,
            severity: Severity::Error,
            summary: "mismatched types".to_string(),
            details: Some("different raw output from the second run".to_string()),
            range: None,
            code: Some("E0308".to_string()),
        },
        &SecretScanner::default(),
    );

    // `Finding::new` creates a new id and timestamp for every call, and the
    // details are raw output. None of the three is evidence.
    assert_ne!(first.id(), second.id());
    assert_eq!(
        FindingEvidence::from_finding(&first),
        FindingEvidence::from_finding(&second)
    );
}

#[test]
fn fingerprint_does_not_depend_on_an_absolute_checkout_path() {
    let alice = evidence(
        "mismatched types",
        Some("/Users/alice/repo/src/a.rs"),
        Some(8),
    );
    let bob = evidence(
        "mismatched types",
        Some("/Users/bob/checkout/src/a.rs"),
        Some(8),
    );

    assert_eq!(alice, bob);
    assert_eq!(alice.path(), Some("a.rs"));
    assert_eq!(
        fingerprint(&[alice], &exited(1)),
        fingerprint(&[bob], &exited(1)),
        "a machine-local checkout path must not change the fingerprint"
    );
}

#[test]
fn fingerprint_normalises_absolute_paths_in_text_and_check_identity() {
    let alice = evidence("failed at /Users/alice/repo/src/a.rs:8:18", None, None);
    let bob = evidence("failed at /Users/bob/checkout/src/a.rs:8:18", None, None);
    assert_eq!(alice, bob);
    assert_eq!(alice.summary(), "failed at a.rs:8:18");
    assert_eq!(
        FailureFingerprint::compute(
            &CheckId::new("/Users/alice/repo/crates/engine", "cargo test"),
            &[],
            &exited(1),
        ),
        FailureFingerprint::compute(
            &CheckId::new("/Users/bob/checkout/crates/engine", "cargo test"),
            &[],
            &exited(1),
        ),
        "an absolute working directory must not become part of the identity"
    );
}

#[test]
fn environment_terminations_are_typed_and_serializable() {
    assert_eq!(
        serde_json::to_value(CheckTermination::SpawnFailed).unwrap(),
        json!({"kind": "spawn_failed"})
    );
    assert_eq!(
        serde_json::to_value(CheckTermination::MissingExecutable).unwrap(),
        json!({"kind": "missing_executable"})
    );
    assert_eq!(
        serde_json::to_value(CheckTermination::PolicyRefused).unwrap(),
        json!({"kind": "policy_refused"})
    );
    assert_eq!(
        serde_json::to_value(CheckTermination::UnavailableInfrastructure).unwrap(),
        json!({"kind": "unavailable_infrastructure"})
    );
    assert_eq!(
        serde_json::to_value(exited(7)).unwrap(),
        json!({"kind": "exited", "exitCode": 7})
    );

    for termination in [
        CheckTermination::SpawnFailed,
        CheckTermination::MissingExecutable,
        CheckTermination::PolicyRefused,
        CheckTermination::UnavailableInfrastructure,
    ] {
        assert!(
            termination.is_environment_failure(),
            "{termination:?} is an environment fact, not a code failure"
        );
    }
    for termination in [
        exited(1),
        CheckTermination::Signalled,
        CheckTermination::TimedOut,
        CheckTermination::Cancelled,
    ] {
        assert!(
            !termination.is_environment_failure(),
            "{termination:?} is not one of requirement 5's environment failures"
        );
    }
}

#[test]
fn baseline_provenance_and_repair_state_round_trip() {
    let found = evidence("mismatched types", Some("src/a.rs"), Some(8));
    let fingerprint = fingerprint(std::slice::from_ref(&found), &exited(1));

    let baseline = CheckBaseline::Failed {
        termination: exited(1),
        fingerprint: fingerprint.clone(),
        findings: vec![found.clone()],
    };
    let encoded = serde_json::to_value(&baseline).unwrap();
    assert_eq!(encoded["result"], "failed");
    assert_eq!(
        serde_json::from_value::<CheckBaseline>(encoded).unwrap(),
        baseline
    );

    let provenance = CheckProvenanceRecord {
        check: check(),
        provenance: FailureProvenance::NewRegression,
        termination: exited(1),
        fingerprint: fingerprint.clone(),
        findings: vec![found.clone()],
        introduced: vec![found],
    };
    let encoded = serde_json::to_value(&provenance).unwrap();
    assert_eq!(encoded["provenance"], "new_regression");
    assert_eq!(
        serde_json::from_value::<CheckProvenanceRecord>(encoded).unwrap(),
        provenance
    );

    let state = RepairStateEntry {
        failure_fingerprint: fingerprint,
        patch_digest: "sha256:patch".to_string(),
        task_diff_digest: "sha256:diff".to_string(),
    };
    let encoded = serde_json::to_value(&state).unwrap();
    assert_eq!(
        serde_json::from_value::<RepairStateEntry>(encoded).unwrap(),
        state
    );
}

#[test]
fn every_provenance_classification_has_its_wire_name() {
    let classes = [
        (FailureProvenance::NewRegression, "new_regression"),
        (FailureProvenance::PreExisting, "pre_existing"),
        (FailureProvenance::Worsened, "worsened"),
        (FailureProvenance::UnchangedFailure, "unchanged_failure"),
        (FailureProvenance::EnvironmentFailure, "environment_failure"),
        (FailureProvenance::UnknownProvenance, "unknown_provenance"),
    ];
    for (classification, name) in classes {
        assert_eq!(classification.as_str(), name);
        assert_eq!(serde_json::to_value(classification).unwrap(), json!(name));
    }
}

fn repair_state(index: usize) -> VerificationEvent {
    VerificationEvent::RepairState {
        task_id: "task".to_string(),
        entry: RepairStateEntry {
            failure_fingerprint: FailureFingerprint::compute(&check(), &[], &exited(index as i32)),
            patch_digest: format!("sha256:patch-{index}"),
            task_diff_digest: format!("sha256:diff-{index}"),
        },
    }
}

#[test]
fn repair_history_keeps_only_the_bounded_window() {
    let events: Vec<VerificationEvent> = (0..MAX_REPAIR_STATE_ENTRIES + 4)
        .map(repair_state)
        .collect();
    let state = fold_verification(events.iter());

    assert_eq!(state.repair_history.len(), MAX_REPAIR_STATE_ENTRIES);
    assert_eq!(
        state.repair_history.first().unwrap().patch_digest,
        "sha256:patch-4",
        "the oldest entries are dropped, not the newest"
    );
    assert_eq!(
        state.repair_history.last().unwrap().patch_digest,
        format!("sha256:patch-{}", MAX_REPAIR_STATE_ENTRIES + 3)
    );
}

#[test]
fn historical_verification_events_replay_without_inventing_provenance() {
    // The exact shapes written before spec 102. They have no baseline,
    // classification or repair state, and replay must not manufacture any.
    let events: Vec<VerificationEvent> = [
        json!({
            "kind": "verification_due",
            "taskId": "task",
            "patchId": "patch",
        }),
        json!({
            "kind": "verification_started",
            "taskId": "task",
            "attempt": 0,
            "checks": [{"workingDirectory": "", "command": "cargo test"}],
        }),
        json!({
            "kind": "verification_check",
            "taskId": "task",
            "check": {"workingDirectory": "", "command": "cargo test"},
            "outcome": {"outcome": "ran", "exitCode": 1, "markerId": "old-marker"},
            "fileHashes": {},
        }),
        json!({
            "kind": "verification_finished",
            "taskId": "task",
            "outcome": "failed",
        }),
    ]
    .into_iter()
    .map(|value| serde_json::from_value(value).expect("a historical event parses"))
    .collect();

    let state = fold_verification(events.iter());
    assert_eq!(
        state.finished,
        Some(workspace_engine::VerificationOutcome::Failed)
    );
    assert!(state.baselines.is_empty());
    assert!(state.provenance.is_empty());
    assert!(state.repair_history.is_empty());
}

#[test]
fn new_verification_events_round_trip_through_a_fresh_store() {
    let data_dir = temp_dir("provenance-store");
    let found = evidence("mismatched types", Some("src/a.rs"), Some(8));
    let fingerprint = fingerprint(std::slice::from_ref(&found), &exited(1));
    let events = vec![
        VerificationEvent::Baseline {
            task_id: "task".to_string(),
            check: check(),
            baseline: CheckBaseline::Failed {
                termination: exited(1),
                fingerprint: fingerprint.clone(),
                findings: vec![found.clone()],
            },
        },
        VerificationEvent::Provenance {
            task_id: "task".to_string(),
            record: CheckProvenanceRecord {
                check: check(),
                provenance: FailureProvenance::UnchangedFailure,
                termination: exited(1),
                fingerprint: fingerprint.clone(),
                findings: vec![found.clone()],
                introduced: Vec::new(),
            },
        },
        VerificationEvent::RepairState {
            task_id: "task".to_string(),
            entry: RepairStateEntry {
                failure_fingerprint: fingerprint,
                patch_digest: "sha256:patch".to_string(),
                task_diff_digest: "sha256:diff".to_string(),
            },
        },
    ];

    let session = {
        let store = SessionStore::new(&data_dir);
        let session = store.create_session("repo", "title").unwrap();
        for event in &events {
            store
                .append_verification_event(&session.id, event)
                .expect("the event should append");
        }
        session
    };

    let reopened = SessionStore::new(&data_dir);
    let replayed = reopened
        .read_verification_events(&session.id, &["task"])
        .expect("the log should read");
    assert_eq!(replayed, events, "the durable payload is unchanged");

    let state = fold_verification(replayed.iter());
    assert_eq!(state.baselines.len(), 1);
    assert!(matches!(
        state.baselines.get(&check()),
        Some(CheckBaseline::Failed { .. })
    ));
    assert_eq!(
        state.provenance.get(&check()).unwrap().provenance,
        FailureProvenance::UnchangedFailure
    );
    assert_eq!(state.repair_history.len(), 1);

    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn replay_of_an_empty_log_has_no_provenance_state() {
    let state = VerificationState::default();
    assert!(state.baselines.is_empty());
    assert!(state.provenance.is_empty());
    assert!(state.repair_history.is_empty());
}

// --- Fresh-store fixtures --------------------------------------------------

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-provenance-{name}-{now}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}
