//! Spec 102 Task 2: pre-apply baseline decisions.
//!
//! These tests run against a real temporary repository so they pin the
//! ordering guarantee: a check is offered or run while the patch's base hashes
//! still name the reviewed bytes, and no patch file is written until every
//! newly selected check has a terminal baseline.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::patch_engine::ProposedChange;
use workspace_engine::{
    BaselinePreparation, CheckBaseline, CheckId, CheckTermination, ClientError, Config,
    EditProposalResult, MockModelAdapter, PatchApplyOutcome, Session, Task, WorkerRecord,
    WorkspaceEngine,
};

fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "damaian-baseline-{name}-{nanos}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn write(repo: &Path, relative_path: &str, content: &str) {
    let path = repo.join(relative_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn test_engine(root: &Path, data_dir: &Path, shell: &str) -> WorkspaceEngine {
    fs::create_dir_all(root).unwrap();
    WorkspaceEngine::new(Config {
        data_dir: data_dir.to_path_buf(),
        enable_index_watcher: false,
        shell: shell.to_string(),
        require_approval_for_risky_commands: false,
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

fn propose(engine: &WorkspaceEngine, root: &Path, response: &str) -> EditProposalResult {
    let mut adapter = MockModelAdapter::new(response);
    engine
        .edit_orchestrator
        .propose_edit(root, "change it", &[], None, &mut adapter)
        .expect("the edit should be proposed")
}

fn baseline_events(
    engine: &WorkspaceEngine,
    session: &Session,
) -> Vec<(String, CheckId, CheckBaseline)> {
    engine
        .session_store
        .read_verification_events(&session.id, &[])
        .unwrap()
        .into_iter()
        .filter_map(|event| match event {
            workspace_engine::VerificationEvent::Baseline {
                task_id,
                check,
                baseline,
            } => Some((task_id, check, baseline)),
            _ => None,
        })
        .collect()
}

fn apply(
    engine: &WorkspaceEngine,
    root: &Path,
    patch_id: &str,
    paths: Option<&[String]>,
) -> workspace_engine::Result<PatchApplyOutcome> {
    engine
        .edit_orchestrator
        .apply_stored_patch_with_baseline(root, patch_id, paths, None, "tester", false)
}

fn pending_batches(data_dir: &Path) -> Vec<PathBuf> {
    let directory = data_dir.join("baselines").join("pending");
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect()
}

fn cleanup(root: &Path, data_dir: &Path) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(data_dir);
}

#[test]
fn a_safe_check_runs_as_a_passing_baseline_before_the_patch_writes() {
    let root = temp_dir("auto-run");
    let data_dir = temp_dir("auto-run-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = test_engine(&root, &data_dir, "/usr/bin/true");
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );

    let outcome = apply(&engine, &root, &proposal.patch.id, None).unwrap();
    assert!(matches!(outcome, PatchApplyOutcome::Applied(_)));
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 2;\n"
    );

    let recorded = baseline_events(&engine, &proposal.session);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    let (task_id, check, baseline) = &recorded[0];
    assert_eq!(
        task_id, &proposal.task.id,
        "the baseline belongs to the patch task"
    );
    assert_eq!(check, &CheckId::new("", "cargo test"));
    assert_eq!(baseline, &CheckBaseline::Passed);

    cleanup(&root, &data_dir);
}

#[test]
fn a_sessionless_patch_with_a_task_id_applies_without_recording_a_baseline() {
    let root = temp_dir("sessionless");
    let data_dir = temp_dir("sessionless-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    // The script would leave `ran` behind if the patch's detected check were
    // baselined. A sessionless patch has nowhere to record that baseline, so
    // its apply must stay exactly as it was before this spec.
    let script = data_dir.join("fake-shell.sh");
    executable(&script, "#!/bin/sh\nprintf ran > \"$PWD/ran\"\nexit 0\n");
    let engine = test_engine(&root, &data_dir, &script.to_string_lossy());

    let patch = engine
        .patch_engine
        .create_patch(
            &root,
            &[ProposedChange {
                path: "src/a.rs".to_string(),
                new_content: "let a = 2;\n".to_string(),
                status: Some("modified".to_string()),
                allow_restricted: false,
            }],
            Some("task_without_a_session"),
            "legacy patch",
        )
        .unwrap();
    assert!(patch.session_id.is_empty());
    assert!(
        patch.task_id.is_some(),
        "the task id alone is not a session"
    );
    engine.patch_store.save(&patch).unwrap();

    assert!(matches!(
        apply(&engine, &root, &patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 2;\n"
    );
    assert!(
        !root.join("ran").exists(),
        "a baseline command ran for a patch with no session"
    );
    assert!(pending_batches(&data_dir).is_empty());
    assert!(
        engine.session_store.list_sessions(None).unwrap().is_empty(),
        "a sessionless patch recorded into a session"
    );
    let audit = fs::read_to_string(data_dir.join("audit").join("events.jsonl")).unwrap_or_default();
    assert!(
        !audit.contains("verification_baseline"),
        "a sessionless patch wrote a baseline event: {audit}"
    );

    cleanup(&root, &data_dir);
}

#[test]
fn a_declined_baseline_does_not_decline_the_patch() {
    let root = temp_dir("decline");
    let data_dir = temp_dir("decline-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        require_approval_for_risky_commands: true,
        ..Config::default()
    });
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );

    let outcome = apply(&engine, &root, &proposal.patch.id, None).unwrap();
    let PatchApplyOutcome::BaselineRequired(batch) = outcome else {
        panic!("the check needs its own approval");
    };
    assert_eq!(batch.checks.len(), 1);
    assert_eq!(batch.task_id, proposal.task.id);
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 1;\n",
        "no patch file is written while a baseline is undecided"
    );
    assert!(
        baseline_events(&engine, &proposal.session).is_empty(),
        "an offer is not a terminal baseline"
    );

    let decisions = vec![(batch.checks[0].proposal_id.clone(), false)];
    let resumed = engine
        .edit_orchestrator
        .resume_stored_patch_baseline(&root, &batch.id, &decisions, &[], "tester")
        .unwrap();
    assert!(matches!(
        resumed,
        BaselinePreparation::Complete { ref recorded } if recorded.len() == 1
    ));

    let applied = apply(&engine, &root, &proposal.patch.id, None).unwrap();
    assert!(matches!(applied, PatchApplyOutcome::Applied(_)));
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 2;\n"
    );
    let recorded = baseline_events(&engine, &proposal.session);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].2, CheckBaseline::Declined);

    cleanup(&root, &data_dir);
}

#[test]
fn an_approved_baseline_runs_and_the_patch_applies_afterwards() {
    let root = temp_dir("approve");
    let data_dir = temp_dir("approve-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        require_approval_for_risky_commands: true,
        ..Config::default()
    });
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );

    let PatchApplyOutcome::BaselineRequired(batch) =
        apply(&engine, &root, &proposal.patch.id, None).unwrap()
    else {
        panic!("the check needs its own approval");
    };
    let decisions = vec![(batch.checks[0].proposal_id.clone(), true)];
    engine
        .edit_orchestrator
        .resume_stored_patch_baseline(&root, &batch.id, &decisions, &[], "tester")
        .unwrap();
    assert_eq!(
        baseline_events(&engine, &proposal.session)[0].2,
        CheckBaseline::Passed
    );

    let applied = apply(&engine, &root, &proposal.patch.id, None).unwrap();
    assert!(matches!(applied, PatchApplyOutcome::Applied(_)));
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 2;\n"
    );

    cleanup(&root, &data_dir);
}

#[test]
fn a_partial_selection_baselines_only_the_roots_it_selects() {
    let root = temp_dir("partial");
    let data_dir = temp_dir("partial-data");
    write(&root, "crates/a/Cargo.toml", "[package]\nname = \"a\"\n");
    write(&root, "crates/a/src/lib.rs", "pub fn a() -> u8 { 1 }\n");
    write(&root, "crates/b/Cargo.toml", "[package]\nname = \"b\"\n");
    write(&root, "crates/b/src/lib.rs", "pub fn b() -> u8 { 1 }\n");
    let engine = test_engine(&root, &data_dir, "/usr/bin/true");
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[
            ("crates/a/src/lib.rs", "pub fn a() -> u8 { 2 }\n"),
            ("crates/b/src/lib.rs", "pub fn b() -> u8 { 2 }\n"),
        ]),
    );

    let first = vec!["crates/a/src/lib.rs".to_string()];
    assert!(matches!(
        apply(&engine, &root, &proposal.patch.id, Some(&first)).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    let recorded = baseline_events(&engine, &proposal.session);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].1, CheckId::new("crates/a", "cargo test"));

    let second = vec!["crates/b/src/lib.rs".to_string()];
    assert!(matches!(
        apply(&engine, &root, &proposal.patch.id, Some(&second)).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    let recorded = baseline_events(&engine, &proposal.session);
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    assert!(
        recorded
            .iter()
            .any(|(_, check, _)| check == &CheckId::new("crates/b", "cargo test"))
    );

    cleanup(&root, &data_dir);
}

#[test]
fn a_check_selected_by_a_later_patch_is_baselined_before_it_applies() {
    let root = temp_dir("later");
    let data_dir = temp_dir("later-data");
    write(&root, "crates/a/Cargo.toml", "[package]\nname = \"a\"\n");
    write(&root, "crates/a/src/lib.rs", "pub fn a() -> u8 { 1 }\n");
    write(&root, "crates/b/Cargo.toml", "[package]\nname = \"b\"\n");
    write(&root, "crates/b/src/lib.rs", "pub fn b() -> u8 { 1 }\n");
    let engine = test_engine(&root, &data_dir, "/usr/bin/true");

    let first = propose(
        &engine,
        &root,
        &edit_response(&[("crates/a/src/lib.rs", "pub fn a() -> u8 { 2 }\n")]),
    );
    assert!(matches!(
        apply(&engine, &root, &first.patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    assert_eq!(baseline_events(&engine, &first.session).len(), 1);

    let second = propose(
        &engine,
        &root,
        &edit_response(&[("crates/b/src/lib.rs", "pub fn b() -> u8 { 2 }\n")]),
    );
    assert!(matches!(
        apply(&engine, &root, &second.patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    let recorded = baseline_events(&engine, &second.session);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].1, CheckId::new("crates/b", "cargo test"));

    cleanup(&root, &data_dir);
}

#[test]
fn a_baseline_that_changes_reviewed_bytes_makes_the_patch_stale() {
    let root = temp_dir("stale");
    let data_dir = temp_dir("stale-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let script = data_dir.join("fake-shell.sh");
    executable(
        &script,
        "#!/bin/sh\nprintf 'baseline wrote this\\n' > \"$PWD/src/a.rs\"\nexit 0\n",
    );
    let engine = test_engine(&root, &data_dir, &script.to_string_lossy());
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );

    let error = apply(&engine, &root, &proposal.patch.id, None).unwrap_err();
    assert!(
        matches!(error, workspace_engine::ClientError::PatchConflict(_)),
        "{error:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "baseline wrote this\n",
        "the reviewed patch bytes were not written over the changed file"
    );
    assert!(matches!(
        baseline_events(&engine, &proposal.session)[0].2,
        CheckBaseline::Passed
    ));

    cleanup(&root, &data_dir);
}

#[test]
fn a_fresh_engine_reuses_the_durable_baseline_without_running_it_again() {
    let root = temp_dir("reload");
    let data_dir = temp_dir("reload-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let first = test_engine(&root, &data_dir, "/usr/bin/true");
    let proposal = propose(
        &first,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );

    let prepared = first
        .edit_orchestrator
        .prepare_stored_patch_baseline(&root, &proposal.patch.id, None)
        .unwrap();
    assert!(matches!(
        prepared,
        BaselinePreparation::Complete { ref recorded } if recorded.len() == 1
    ));
    assert_eq!(baseline_events(&first, &proposal.session).len(), 1);

    drop(first);
    let reopened = test_engine(&root, &data_dir, "/usr/bin/true");
    let prepared = reopened
        .edit_orchestrator
        .prepare_stored_patch_baseline(&root, &proposal.patch.id, None)
        .unwrap();
    assert!(
        matches!(prepared, BaselinePreparation::Complete { ref recorded } if recorded.is_empty()),
        "a current baseline is reused, not run again: {prepared:?}"
    );
    assert_eq!(baseline_events(&reopened, &proposal.session).len(), 1);

    assert!(matches!(
        apply(&reopened, &root, &proposal.patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));

    cleanup(&root, &data_dir);
}

#[test]
fn a_worker_session_baseline_replays_after_the_worker_restarts() {
    let root = temp_dir("worker");
    let data_dir = temp_dir("worker-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    write(&root, "src/b.rs", "let b = 1;\n");
    let first = test_engine(&root, &data_dir, "/usr/bin/true");
    let session = first
        .session_store
        .create_session("repo", "worker task")
        .unwrap();
    first
        .session_store
        .record_worker_spawned(
            &session.id,
            &WorkerRecord {
                parent_session_id: None,
                parent_task_id: None,
                inherited_protected_paths: Vec::new(),
                source_repository: root.clone(),
                worktree_path: root.clone(),
                branch: "worker".to_string(),
                base_commit: "base".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .unwrap();
    let task: Task = first
        .session_store
        .create_task(&session.id, "worker task", "provider", "model")
        .unwrap();
    let patch_for = |path: &str, before: &str, after: &str| {
        let mut patch = first
            .patch_engine
            .create_patch(
                &root,
                &[ProposedChange {
                    path: path.to_string(),
                    new_content: after.to_string(),
                    status: Some("modified".to_string()),
                    allow_restricted: false,
                }],
                Some(&task.id),
                "worker patch",
            )
            .unwrap();
        assert_eq!(patch.files[0].new_content, after);
        patch.session_id = session.id.clone();
        first.patch_store.save(&patch).unwrap();
        assert!(before != after);
        patch
    };
    let first_patch = patch_for("src/a.rs", "let a = 1;\n", "let a = 2;\n");
    let second_patch = patch_for("src/b.rs", "let b = 1;\n", "let b = 2;\n");

    assert!(matches!(
        apply(&first, &root, &first_patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    assert_eq!(baseline_events(&first, &session).len(), 1);

    drop(first);
    let reopened = test_engine(&root, &data_dir, "/usr/bin/true");
    let prepared = reopened
        .edit_orchestrator
        .prepare_stored_patch_baseline(&root, &second_patch.id, None)
        .unwrap();
    assert!(
        matches!(prepared, BaselinePreparation::Complete { ref recorded } if recorded.is_empty()),
        "the restarted worker reuses its durable baseline"
    );
    assert!(matches!(
        apply(&reopened, &root, &second_patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    assert_eq!(baseline_events(&reopened, &session).len(), 1);

    cleanup(&root, &data_dir);
}

#[test]
fn policy_refusal_is_an_unavailable_baseline_and_the_patch_still_applies() {
    let root = temp_dir("policy");
    let data_dir = temp_dir("policy-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        command_blocklist: vec!["cargo test".to_string()],
        ..Config::default()
    });
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );

    assert!(matches!(
        apply(&engine, &root, &proposal.patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    let recorded = baseline_events(&engine, &proposal.session);
    assert!(matches!(
        &recorded[0].2,
        CheckBaseline::Unavailable {
            termination: CheckTermination::PolicyRefused,
            ..
        }
    ));

    cleanup(&root, &data_dir);
}

#[test]
fn a_spawn_failure_and_a_missing_executable_are_unavailable_baselines() {
    let root = temp_dir("spawn");
    let data_dir = temp_dir("spawn-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let missing_shell = data_dir.join("does-not-exist");
    let engine = test_engine(&root, &data_dir, &missing_shell.to_string_lossy());
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );

    assert!(matches!(
        apply(&engine, &root, &proposal.patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    assert!(matches!(
        &baseline_events(&engine, &proposal.session)[0].2,
        CheckBaseline::Unavailable {
            termination: CheckTermination::SpawnFailed,
            ..
        }
    ));

    let root2 = temp_dir("missing");
    let data_dir2 = temp_dir("missing-data");
    write(&root2, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root2, "src/a.rs", "let a = 1;\n");
    let script = data_dir2.join("fake-shell.sh");
    executable(&script, "#!/bin/sh\nexit 127\n");
    let engine2 = test_engine(&root2, &data_dir2, &script.to_string_lossy());
    let proposal2 = propose(
        &engine2,
        &root2,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    assert!(matches!(
        apply(&engine2, &root2, &proposal2.patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    assert!(matches!(
        &baseline_events(&engine2, &proposal2.session)[0].2,
        CheckBaseline::Unavailable {
            termination: CheckTermination::MissingExecutable,
            ..
        }
    ));

    cleanup(&root, &data_dir);
    cleanup(&root2, &data_dir2);
}

/// A malformed retry is validated as a whole before any command runs or any
/// baseline is recorded, so it can be corrected and retried. The script would
/// leave `ran` behind if the engine executed even one approved check.
#[test]
fn malformed_resume_decisions_have_no_side_effects() {
    let root = temp_dir("atomic");
    let data_dir = temp_dir("atomic-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let script = data_dir.join("fake-shell.sh");
    executable(&script, "#!/bin/sh\nprintf ran > \"$PWD/ran\"\nexit 0\n");
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: script.to_string_lossy().to_string(),
        require_approval_for_risky_commands: true,
        ..Config::default()
    });
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    let PatchApplyOutcome::BaselineRequired(batch) =
        apply(&engine, &root, &proposal.patch.id, None).unwrap()
    else {
        panic!("the check needs its own approval");
    };
    let proposal_id = batch.checks[0].proposal_id.clone();
    assert_eq!(pending_batches(&data_dir).len(), 1);

    for decisions in [
        Vec::new(),
        vec![(proposal_id.clone(), true), (proposal_id.clone(), true)],
        vec![
            (proposal_id.clone(), true),
            ("baseline_unknown".to_string(), true),
        ],
    ] {
        let error = engine
            .edit_orchestrator
            .resume_stored_patch_baseline(&root, &batch.id, &decisions, &[], "tester")
            .unwrap_err();
        assert!(matches!(error, ClientError::InvalidInput(_)), "{error:?}");
        assert!(
            baseline_events(&engine, &proposal.session).is_empty(),
            "a malformed retry recorded a baseline"
        );
        assert!(
            !root.join("ran").exists(),
            "a malformed retry ran a baseline command"
        );
        assert_eq!(
            pending_batches(&data_dir).len(),
            1,
            "a malformed retry discarded the batch"
        );
    }

    let decisions = vec![(proposal_id, false)];
    assert!(matches!(
        engine
            .edit_orchestrator
            .resume_stored_patch_baseline(&root, &batch.id, &decisions, &[], "tester")
            .unwrap(),
        BaselinePreparation::Complete { .. }
    ));
    assert!(pending_batches(&data_dir).is_empty());
    assert!(!root.join("ran").exists());

    cleanup(&root, &data_dir);
}

#[test]
fn always_grants_must_belong_to_the_pending_batch_and_be_approved_and_eligible() {
    let root = temp_dir("always");
    let data_dir = temp_dir("always-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let script = data_dir.join("fake-shell.sh");
    executable(&script, "#!/bin/sh\nprintf ran > \"$PWD/ran\"\nexit 0\n");
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: script.to_string_lossy().to_string(),
        require_approval_for_risky_commands: true,
        ..Config::default()
    });
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    let PatchApplyOutcome::BaselineRequired(batch) =
        apply(&engine, &root, &proposal.patch.id, None).unwrap()
    else {
        panic!("the check needs its own approval");
    };
    let proposal_id = batch.checks[0].proposal_id.clone();
    let config_path = engine.config.user_config_path();

    let invalid = [
        // A proposal that is not on this batch.
        (
            vec![(proposal_id.clone(), true)],
            vec!["cmd_unrelated".to_string()],
        ),
        // The check itself, but the user declined to run it.
        (
            vec![(proposal_id.clone(), false)],
            vec![proposal_id.clone()],
        ),
        // The same eligible id twice.
        (
            vec![(proposal_id.clone(), true)],
            vec![proposal_id.clone(), proposal_id.clone()],
        ),
    ];
    for (decisions, always) in invalid {
        let error = engine
            .edit_orchestrator
            .resume_stored_patch_baseline(&root, &batch.id, &decisions, &always, "tester")
            .unwrap_err();
        assert!(matches!(error, ClientError::InvalidInput(_)), "{error:?}");
        assert!(
            baseline_events(&engine, &proposal.session).is_empty(),
            "an invalid always decision recorded a baseline"
        );
        assert!(
            !root.join("ran").exists(),
            "an invalid always decision ran a baseline command"
        );
        assert!(
            !fs::read_to_string(&config_path)
                .unwrap_or_default()
                .contains("cargo test"),
            "an invalid always decision wrote an allowlist grant"
        );
        assert_eq!(pending_batches(&data_dir).len(), 1);
    }

    assert!(matches!(
        engine
            .edit_orchestrator
            .resume_stored_patch_baseline(
                &root,
                &batch.id,
                &[(proposal_id, true)],
                &[batch.checks[0].proposal_id.clone()],
                "tester",
            )
            .unwrap(),
        BaselinePreparation::Complete { .. }
    ));
    assert!(root.join("ran").exists(), "the approved baseline ran");
    assert!(
        fs::read_to_string(&config_path)
            .unwrap_or_default()
            .contains("cargo test"),
        "the approved always grant was written"
    );

    cleanup(&root, &data_dir);
}

#[test]
fn a_batch_id_cannot_escape_the_baseline_directory() {
    let root = temp_dir("escape");
    let data_dir = temp_dir("escape-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    let engine = test_engine(&root, &data_dir, "/usr/bin/true");

    for batch_id in [
        "../escape",
        "nested/escape",
        "baseline_../../escape",
        "baseline_ok/../../escape",
        "",
    ] {
        let error = engine
            .edit_orchestrator
            .resume_stored_patch_baseline(&root, batch_id, &[], &[], "tester")
            .unwrap_err();
        assert!(
            matches!(error, ClientError::InvalidInput(_)),
            "{batch_id}: {error:?}"
        );
    }
    assert!(!data_dir.join("escape.json").exists());
    assert!(
        !data_dir
            .join("baselines")
            .join("pending")
            .join("escape.json")
            .exists()
    );
    assert!(
        !data_dir
            .parent()
            .expect("the data dir has a parent")
            .join("escape.json")
            .exists()
    );

    cleanup(&root, &data_dir);
}

#[test]
fn a_batch_can_only_be_resumed_by_the_repository_that_created_it() {
    let root = temp_dir("bound");
    let other = temp_dir("bound-other");
    let data_dir = temp_dir("bound-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        require_approval_for_risky_commands: true,
        ..Config::default()
    });
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );
    let PatchApplyOutcome::BaselineRequired(batch) =
        apply(&engine, &root, &proposal.patch.id, None).unwrap()
    else {
        panic!("the check needs its own approval");
    };
    let decisions = vec![(batch.checks[0].proposal_id.clone(), false)];

    let error = engine
        .edit_orchestrator
        .resume_stored_patch_baseline(&other, &batch.id, &decisions, &[], "tester")
        .unwrap_err();
    assert!(matches!(error, ClientError::InvalidInput(_)), "{error:?}");
    assert!(baseline_events(&engine, &proposal.session).is_empty());
    assert_eq!(pending_batches(&data_dir).len(), 1);
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "let a = 1;\n"
    );

    assert!(matches!(
        engine
            .edit_orchestrator
            .resume_stored_patch_baseline(&root, &batch.id, &decisions, &[], "tester")
            .unwrap(),
        BaselinePreparation::Complete { .. }
    ));

    cleanup(&root, &data_dir);
    let _ = fs::remove_dir_all(&other);
}

/// A real failed check keeps its exit code and only redacted, normalized
/// evidence. The AWS-shaped value in the panic message must not survive into
/// the finding or the fingerprint input.
#[test]
fn a_failed_baseline_records_redacted_normalized_findings_and_a_fingerprint() {
    let root = temp_dir("failed");
    let data_dir = temp_dir("failed-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let script = data_dir.join("fake-shell.sh");
    executable(
        &script,
        "#!/bin/sh\ncat <<'OUT'\n---- tests::fails stdout ----\nthread 'tests::fails' panicked at src/a.rs:2:5:\nleaked AKIAIOSFODNN7EXAMPLE\nfailures:\n    tests::fails\nOUT\nexit 1\n",
    );
    let engine = test_engine(&root, &data_dir, &script.to_string_lossy());
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );

    assert!(matches!(
        apply(&engine, &root, &proposal.patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));
    let CheckBaseline::Failed {
        termination,
        fingerprint,
        findings,
    } = &baseline_events(&engine, &proposal.session)[0].2
    else {
        panic!("the baseline failed");
    };
    assert_eq!(
        termination,
        &CheckTermination::Exited { exit_code: 1 },
        "the real exit code is fingerprint material"
    );
    assert!(!fingerprint.as_str().is_empty());
    assert_eq!(findings.len(), 1, "{findings:?}");
    let finding = &findings[0];
    assert_eq!(finding.path(), Some("src/a.rs"));
    assert_eq!(finding.start_line(), Some(2));
    assert_eq!(finding.start_column(), Some(5));
    assert!(
        finding.summary().contains("[REDACTED_"),
        "the secret is redacted, not merely trimmed: {}",
        finding.summary()
    );
    assert!(!finding.summary().contains("AKIAIOSFODNN7EXAMPLE"));
    // The stored value is exactly the canonical computation over the stored
    // evidence, so Task 3 can recompute and compare it.
    assert_eq!(
        fingerprint,
        &workspace_engine::FailureFingerprint::compute(
            &CheckId::new("", "cargo test"),
            findings,
            termination
        )
    );

    cleanup(&root, &data_dir);
}

#[test]
fn repeated_preparation_reoffers_one_pending_batch_for_the_same_patch() {
    let root = temp_dir("dedupe");
    let data_dir = temp_dir("dedupe-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "src/a.rs", "let a = 1;\n");
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        require_approval_for_risky_commands: true,
        ..Config::default()
    });
    let proposal = propose(
        &engine,
        &root,
        &edit_response(&[("src/a.rs", "let a = 2;\n")]),
    );

    let first = engine
        .edit_orchestrator
        .prepare_stored_patch_baseline(&root, &proposal.patch.id, None)
        .unwrap();
    let second = engine
        .edit_orchestrator
        .prepare_stored_patch_baseline(&root, &proposal.patch.id, None)
        .unwrap();
    let (BaselinePreparation::Awaiting(first), BaselinePreparation::Awaiting(second)) =
        (first, second)
    else {
        panic!("both preparations need the same approval");
    };
    assert_eq!(first.id, second.id);
    assert_eq!(pending_batches(&data_dir).len(), 1);

    let decisions = vec![(second.checks[0].proposal_id.clone(), false)];
    engine
        .edit_orchestrator
        .resume_stored_patch_baseline(&root, &second.id, &decisions, &[], "tester")
        .unwrap();
    assert!(pending_batches(&data_dir).is_empty());
    assert!(matches!(
        apply(&engine, &root, &proposal.patch.id, None).unwrap(),
        PatchApplyOutcome::Applied(_)
    ));

    cleanup(&root, &data_dir);
}

#[test]
fn a_partial_hunk_apply_still_passes_the_baseline_gate() {
    let root = temp_dir("hunks");
    let data_dir = temp_dir("hunks-data");
    write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
    let before = (1..=24)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    let after = before
        .replace("line 2\n", "line two\n")
        .replace("line 22\n", "line twenty-two\n");
    write(&root, "src/a.rs", &before);
    let engine = test_engine(&root, &data_dir, "/usr/bin/true");
    let proposal = propose(&engine, &root, &edit_response(&[("src/a.rs", &after)]));
    assert!(
        proposal.patch.files[0].hunks.len() >= 2,
        "the fixture needs two hunks"
    );

    let mut selection = std::collections::HashMap::new();
    selection.insert(
        "src/a.rs".to_string(),
        vec![proposal.patch.files[0].hunks[0].id.clone()],
    );
    let outcome = engine
        .edit_orchestrator
        .apply_stored_patch_with_baseline(
            &root,
            &proposal.patch.id,
            Some(&["src/a.rs".to_string()]),
            Some(&selection),
            "tester",
            false,
        )
        .unwrap();
    assert!(matches!(outcome, PatchApplyOutcome::Applied(_)));
    let written = fs::read_to_string(root.join("src/a.rs")).unwrap();
    assert!(written.contains("line two\n"), "{written}");
    assert!(written.contains("line 22\n"), "{written}");
    assert!(!written.contains("line twenty-two\n"), "{written}");

    cleanup(&root, &data_dir);
}
