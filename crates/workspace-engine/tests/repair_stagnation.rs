//! Spec 102 Task 4: canonical repair state and stagnation stops.
//!
//! These tests pin the digest inputs, the fixed reason precedence and the
//! crash-safe replay facts without a model or a real repair turn. The repair
//! loop integration lives in `verification_loop.rs`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::patch_engine::ProposedFilePatch;
use workspace_engine::{
    CheckId, CheckTermination, FailureFingerprint, MAX_REPAIR_STATE_ENTRIES, RepairStall,
    RepairStallReason, RepairStateEntry, SecretScanner, SessionStore, TaskPlan, VerificationEvent,
    VerificationOutcome, VerificationReport, detect_repair_stall, fold_verification,
    proposed_patch_digest, task_diff_digest,
};

fn check() -> CheckId {
    CheckId::new("", "cargo test")
}

fn fingerprint(code: i32) -> FailureFingerprint {
    FailureFingerprint::compute(&check(), &[], &CheckTermination::Exited { exit_code: code })
}

fn entry(failure: i32, patch: &str, diff: &str) -> RepairStateEntry {
    RepairStateEntry {
        failure_fingerprint: fingerprint(failure),
        patch_digest: patch.to_string(),
        task_diff_digest: diff.to_string(),
    }
}

fn file(path: &str, content: &str) -> ProposedFilePatch {
    ProposedFilePatch {
        path: path.to_string(),
        status: "modified".to_string(),
        base_hash: Some("sha256:base-one".to_string()),
        new_content: content.to_string(),
        new_hash: "sha256:new-one".to_string(),
        diff: format!("--- a/{path}\n+++ b/{path}\n"),
        hunks: Vec::new(),
    }
}

#[test]
fn patch_digest_is_order_and_transient_field_independent() {
    let scanner = SecretScanner::default();
    let app = file("src/app.rs", "fn app() {}\n");
    let lib = file("src/lib.rs", "pub fn lib() {}\n");

    let forward = proposed_patch_digest(&[app.clone(), lib.clone()], &scanner);
    let reversed = proposed_patch_digest(&[lib.clone(), app.clone()], &scanner);
    assert_eq!(forward, reversed, "file order is not repair identity");

    let mut changed_base = app.clone();
    changed_base.base_hash = Some("sha256:another-base".to_string());
    changed_base.new_hash = "sha256:another-new".to_string();
    changed_base.diff = "a different display diff\n".to_string();
    assert_eq!(
        forward,
        proposed_patch_digest(&[changed_base, lib.clone()], &scanner),
        "base hashes, the display diff and derived hashes are not repair identity"
    );

    let mut different = lib;
    different.new_content = "pub fn lib() { changed(); }\n".to_string();
    assert_ne!(
        forward,
        proposed_patch_digest(&[app, different], &scanner),
        "different effective content is a different repair"
    );
}

#[test]
fn patch_digest_normalises_paths_and_never_contains_a_secret() {
    let scanner = SecretScanner::default();
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let first = file("./src/app.rs", &format!("const key = \"{secret}\";\n"));
    let second = file("src/app.rs", "const key = \"[REDACTED_aws_access_key]\";\n");
    let digest = proposed_patch_digest(&[first], &scanner);

    assert!(
        !digest.contains(secret),
        "the digest must not expose the secret: {digest}"
    );
    assert!(
        digest.starts_with("sha256:"),
        "a digest, not raw content: {digest}"
    );
    assert_ne!(
        digest,
        proposed_patch_digest(&[second], &scanner),
        "a redacted placeholder is not the same effective content"
    );
    assert_eq!(
        digest,
        proposed_patch_digest(
            &[file("src/app.rs", &format!("const key = \"{secret}\";\n"))],
            &scanner
        )
    );
}

#[test]
fn a_path_spelling_does_not_change_the_patch_digest() {
    let scanner = SecretScanner::default();
    assert_eq!(
        proposed_patch_digest(&[file("./src/app.rs", "one\n")], &scanner),
        proposed_patch_digest(&[file("src/app.rs", "one\n")], &scanner)
    );
}

#[test]
fn task_diff_digest_is_order_independent_and_content_safe() {
    let root = temp_dir("diff-digest");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/a.rs"),
        "const key = \"AKIAIOSFODNN7EXAMPLE\";\n",
    )
    .unwrap();
    fs::write(root.join("src/b.rs"), "fn b() {}\n").unwrap();
    let a = "src/a.rs".to_string();
    let b = "src/b.rs".to_string();

    let forward = task_diff_digest(&root, &[a.clone(), b.clone()]);
    let reversed = task_diff_digest(&root, &[b.clone(), a.clone()]);
    assert_eq!(forward, reversed, "path order is not task identity");
    assert!(!forward.contains("AKIA"), "{forward}");
    assert_eq!(
        forward,
        task_diff_digest(&root, &["./src/a.rs".to_string(), b.clone()]),
        "the same repository path spelled differently is the same change"
    );

    fs::write(root.join("src/a.rs"), "changed\n").unwrap();
    assert_ne!(
        forward,
        task_diff_digest(&root, &[a, b]),
        "a changed file changes the net diff"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_first_progressing_attempt_never_stalls() {
    assert_eq!(detect_repair_stall(&[], &entry(1, "p1", "d1")), None);

    let history = vec![entry(1, "p1", "d1")];
    assert_eq!(
        detect_repair_stall(&history, &entry(2, "p2", "d2")),
        None,
        "a different failure, patch and diff is progress"
    );
    assert_eq!(
        detect_repair_stall(&history, &entry(1, "p2", "d2")),
        Some(RepairStallReason::RepeatedFailure),
        "the same failure alone is not progress"
    );
}

#[test]
fn each_stop_cause_is_detected_independently() {
    let previous = entry(1, "p1", "d1");

    assert_eq!(
        detect_repair_stall(std::slice::from_ref(&previous), &previous.clone()),
        Some(RepairStallReason::IdenticalState)
    );
    assert_eq!(
        detect_repair_stall(std::slice::from_ref(&previous), &entry(1, "p2", "d2")),
        Some(RepairStallReason::RepeatedFailure)
    );
    assert_eq!(
        detect_repair_stall(std::slice::from_ref(&previous), &entry(2, "p1", "d2")),
        Some(RepairStallReason::EquivalentPatch)
    );
    assert_eq!(
        detect_repair_stall(std::slice::from_ref(&previous), &entry(2, "p2", "d1")),
        Some(RepairStallReason::UnchangedTaskDiff)
    );
}

#[test]
fn an_a_to_b_to_a_state_oscillates() {
    let a = entry(1, "p-a", "d-a");
    let b = entry(2, "p-b", "d-b");
    let back_to_a = entry(1, "p-a2", "d-a");

    let history = vec![a.clone(), b];
    assert_eq!(
        detect_repair_stall(&history, &back_to_a),
        Some(RepairStallReason::Oscillation),
        "the fingerprint and diff returned to an earlier state"
    );

    // The same fingerprint with a different diff is not the pinned pair.
    assert_eq!(
        detect_repair_stall(&history, &entry(1, "p-a3", "d-c")),
        None
    );
}

#[test]
fn reason_precedence_is_fixed() {
    assert_eq!(
        RepairStallReason::PRECEDENCE,
        [
            RepairStallReason::IdenticalState,
            RepairStallReason::RepeatedFailure,
            RepairStallReason::EquivalentPatch,
            RepairStallReason::UnchangedTaskDiff,
            RepairStallReason::Oscillation,
        ]
    );

    // All five conditions hold: identical state wins.
    let previous = entry(1, "p1", "d1");
    assert_eq!(
        detect_repair_stall(std::slice::from_ref(&previous), &previous),
        Some(RepairStallReason::IdenticalState)
    );

    // Repeated failure and equivalent patch both hold; repeated failure wins.
    let previous = entry(1, "p1", "d1");
    assert_eq!(
        detect_repair_stall(std::slice::from_ref(&previous), &entry(1, "p1", "d2")),
        Some(RepairStallReason::RepeatedFailure)
    );

    // Equivalent patch and unchanged diff both hold; equivalent patch wins.
    let previous = entry(1, "p1", "d1");
    assert_eq!(
        detect_repair_stall(std::slice::from_ref(&previous), &entry(2, "p1", "d1")),
        Some(RepairStallReason::EquivalentPatch)
    );
}

#[test]
fn a_stall_has_engine_owned_presentation_and_no_continue_action() {
    for reason in RepairStallReason::PRECEDENCE {
        let stall = RepairStall::new(reason);
        assert_eq!(stall.reason, reason);
        assert!(!stall.label.is_empty());
        assert!(!stall.explanation.is_empty());
        assert!(!stall.action.label.is_empty());
        assert!(!stall.action.detail.is_empty());
        assert!(
            !stall.action.label.to_lowercase().contains("continue"),
            "a stall does not offer continue repair: {stall:?}"
        );
        assert_eq!(
            serde_json::to_value(&stall).unwrap()["reason"],
            serde_json::to_value(reason).unwrap()
        );
    }
    assert_eq!(RepairStallReason::Oscillation.as_str(), "oscillation");
}

/// The engine's explanation already ends with a sentence period. The summary
/// must print it verbatim and separate it from the plan note with one space,
/// not append a second period.
#[test]
fn a_stall_summary_does_not_double_the_explanation_period() {
    let stall = RepairStall::new(RepairStallReason::RepeatedFailure);
    let events = [
        VerificationEvent::Started {
            task_id: "task".to_string(),
            attempt: 1,
            checks: vec![check()],
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check: check(),
            outcome: workspace_engine::CheckOutcome::Ran {
                exit_code: Some(1),
                marker_id: "marker".to_string(),
                termination: Some(CheckTermination::Exited { exit_code: 1 }),
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::RepairStalled {
            task_id: "task".to_string(),
            entry: entry(1, "p1", "d1"),
            stall: stall.clone(),
        },
    ];
    let plan = TaskPlan::new("task", 0);
    let report = VerificationReport::build(
        &fold_verification(events.iter()),
        &plan,
        &[],
        &BTreeMap::new(),
    );

    let summary = report.summary(&plan.report());
    assert!(
        summary.contains(&format!("{}: {}", stall.label, stall.explanation)),
        "{summary}"
    );
    assert!(!summary.contains(".."), "double punctuation: {summary}");
    assert!(
        summary.ends_with("0 of 0 plan steps finished."),
        "{summary}"
    );
}

fn repair_state(task_id: &str, value: usize) -> VerificationEvent {
    VerificationEvent::RepairState {
        task_id: task_id.to_string(),
        entry: RepairStateEntry {
            failure_fingerprint: fingerprint(value as i32),
            patch_digest: format!("sha256:patch-{value}"),
            task_diff_digest: format!("sha256:diff-{value}"),
        },
    }
}

#[test]
fn repair_history_survives_a_fresh_store_and_repair_continuations() {
    let data_dir = temp_dir("history-store");
    let session = {
        let store = SessionStore::new(&data_dir);
        let session = store.create_session("repo", "history").unwrap();
        let first = store
            .create_task(&session.id, "first", "provider", "model")
            .unwrap();
        let verification = store
            .create_task(&session.id, "verification", "provider", "model")
            .unwrap();
        // `verification_due` names the parent and `verification_started` names
        // the child, which is the repair continuation edge.
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Due {
                    task_id: first.id.clone(),
                    patch_id: "patch".to_string(),
                    paths: Vec::new(),
                    checks: Vec::new(),
                },
            )
            .unwrap();
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Started {
                    task_id: verification.id.clone(),
                    attempt: 0,
                    checks: Vec::new(),
                },
            )
            .unwrap();
        store
            .append_verification_event(&session.id, &repair_state(&first.id, 1))
            .unwrap();
        store
            .append_verification_event(&session.id, &repair_state(&verification.id, 2))
            .unwrap();
        (session, first, verification)
    };
    let (session, first, verification) = session;

    let reopened = SessionStore::new(&data_dir);
    let history = reopened
        .task_verification_repair_history(&session.id, &verification.id)
        .expect("the chain history should read");
    assert_eq!(
        history
            .iter()
            .map(|entry| entry.patch_digest.as_str())
            .collect::<Vec<_>>(),
        vec!["sha256:patch-1", "sha256:patch-2"],
        "a repair continuation keeps its own earlier attempts"
    );

    // A new user task has no continuation edge, so none of the earlier work
    // leaks into it. It starts with an empty window even in the same session.
    let new_task = reopened
        .create_task(&session.id, "a new request", "provider", "model")
        .unwrap();
    assert!(
        reopened
            .task_verification_repair_history(&session.id, &new_task.id)
            .unwrap()
            .is_empty(),
        "history must not leak into a new user task"
    );
    reopened
        .append_verification_event(&session.id, &repair_state(&new_task.id, 9))
        .unwrap();
    let fresh = SessionStore::new(&data_dir);
    assert_eq!(
        fresh
            .task_verification_repair_history(&session.id, &new_task.id)
            .unwrap()
            .iter()
            .map(|entry| entry.patch_digest.as_str())
            .collect::<Vec<_>>(),
        vec!["sha256:patch-9"],
        "the new task's own first attempt is its whole window"
    );
    assert!(
        fresh
            .task_verification_repair_history(&session.id, &first.id)
            .unwrap()
            .iter()
            .all(|entry| entry.patch_digest != "sha256:patch-9"),
        "the new task's state does not leak back either"
    );

    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn repair_stall_is_one_terminal_event_that_replays_as_a_stop() {
    let data_dir = temp_dir("stall-replay");
    let entry = entry(1, "p1", "d1");
    let stall = RepairStall::new(RepairStallReason::IdenticalState);
    let events = [
        VerificationEvent::Due {
            task_id: "task".to_string(),
            patch_id: "patch".to_string(),
            paths: Vec::new(),
            checks: Vec::new(),
        },
        VerificationEvent::Started {
            task_id: "task".to_string(),
            attempt: 1,
            checks: vec![check()],
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check: check(),
            outcome: workspace_engine::CheckOutcome::Ran {
                exit_code: Some(1),
                marker_id: "marker".to_string(),
                termination: Some(CheckTermination::Exited { exit_code: 1 }),
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::RepairState {
            task_id: "task".to_string(),
            entry: entry.clone(),
        },
        VerificationEvent::RepairStalled {
            task_id: "task".to_string(),
            entry: entry.clone(),
            stall: stall.clone(),
        },
    ];

    let state = fold_verification(events.iter());
    assert_eq!(state.finished, Some(VerificationOutcome::RepairStalled));
    assert_eq!(state.repair_stall, Some(stall.clone()));
    assert!(state.due.is_none(), "the stop clears the due run");
    assert!(!state.in_progress, "the stop clears the in-progress run");
    assert_eq!(state.repair_history, vec![entry.clone(), entry]);

    let session = {
        let store = SessionStore::new(&data_dir);
        let session = store.create_session("repo", "stall").unwrap();
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
        .expect("the durable log should read");
    assert_eq!(replayed, events, "the stall event round-trips unchanged");
    let state = fold_verification(replayed.iter());
    assert_eq!(state.finished, Some(VerificationOutcome::RepairStalled));
    assert_eq!(state.repair_stall, Some(stall));

    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn a_new_started_run_clears_a_previous_stall_but_keeps_the_window() {
    let mut state = fold_verification(
        [
            VerificationEvent::RepairStalled {
                task_id: "task".to_string(),
                entry: entry(1, "p1", "d1"),
                stall: RepairStall::new(RepairStallReason::RepeatedFailure),
            },
            VerificationEvent::Started {
                task_id: "task".to_string(),
                attempt: 2,
                checks: vec![check()],
            },
        ]
        .iter(),
    );
    assert!(state.repair_stall.is_none());
    assert_eq!(state.repair_history.len(), 1);
    state.apply(&VerificationEvent::RepairState {
        task_id: "task".to_string(),
        entry: entry(2, "p2", "d2"),
    });
    assert_eq!(state.repair_history.len(), 2);
    assert!(state.repair_history.len() <= MAX_REPAIR_STATE_ENTRIES);
}

// --- Fresh-store fixtures --------------------------------------------------

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-stagnation-{name}-{now}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}
