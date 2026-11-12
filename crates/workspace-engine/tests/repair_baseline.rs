//! Spec 109 Task 3: failure-time baselines on the task-start tree (D1, D2).
//!
//! A patch applies with no baseline. When a check run after it fails and the
//! task chain has no baseline for it, the engine reruns that check in a
//! temporary tree rebuilt to the task-start state and classifies the failure
//! with spec 102's unchanged `classify`. These tests use real Git fixtures and
//! a fake shell whose "test suite" fails the tests named in `src/lib.rs`, so the
//! same command gives a different result on the task-start tree and on the
//! user's checkout.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::pre_task_tree::temporary_trees;
use workspace_engine::{
    CancelToken, ChatTurnResult, CheckBaseline, CheckId, Config, EditProposalResult,
    FailureProvenance, MockModelAdapter, TurnProgress, TurnSink, VerificationEvent,
    WorkspaceEngine,
};

fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "damaian-failure-baseline-test-{name}-{nanos}-{}",
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

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}: {status:?}");
}

/// A Cargo root whose `src/lib.rs` is committed with `committed` and then left
/// as `uncommitted` in the working tree (the user's state at task start).
fn git_fixture(name: &str, committed: &str, uncommitted: Option<&str>) -> PathBuf {
    let root = temp_dir(name);
    write(&root, "Cargo.toml", "[package]\nname = \"fixture\"\n");
    write(&root, "src/lib.rs", committed);
    git(&root, &["init", "--quiet"]);
    git(&root, &["config", "user.email", "test@damaian.invalid"]);
    git(&root, &["config", "user.name", "Damaian Test"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "--quiet", "-m", "fixture"]);
    if let Some(content) = uncommitted {
        write(&root, "src/lib.rs", content);
    }
    root
}

/// A shell whose every command is a test suite: it fails each test named on a
/// `// fail <name>` line of `src/lib.rs` in the directory it runs in, and
/// appends that directory to `runs.log` so a test can tell where it ran.
fn fake_shell(data_dir: &Path) -> PathBuf {
    let script = data_dir.join("fake-shell.sh");
    let log = data_dir.join("runs.log");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             echo \"$PWD\" >> '{}'\n\
             names=$(sed -n 's|^// fail ||p' src/lib.rs 2>/dev/null)\n\
             if [ -n \"$names\" ]; then\n\
             echo 'failures:'\n\
             for name in $names; do echo \"    $name\"; done\n\
             echo ''\n\
             echo 'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s'\n\
             exit 101\n\
             fi\n\
             echo 'test result: ok. 1 passed; 0 failed'\n\
             exit 0\n",
            log.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    script
}

fn runs(data_dir: &Path) -> Vec<String> {
    fs::read_to_string(data_dir.join("runs.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn engine(data_dir: &Path, shell: &Path, approval: bool) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: data_dir.to_path_buf(),
        enable_index_watcher: false,
        shell: shell.to_string_lossy().to_string(),
        require_approval_for_risky_commands: approval,
        // No repair turn: these tests are about the classification.
        agent_tool_retry_limit: 0,
        ..Config::default()
    })
}

fn edit_response(content: &str) -> String {
    format!(
        "DAMAIAN_EDIT_V1\nSUMMARY: change\nFILE: src/lib.rs\nSTATUS: modified\nCONTENT:\n{content}END_FILE\nEND_PATCH\n"
    )
}

fn propose_and_apply(engine: &WorkspaceEngine, root: &Path, content: &str) -> EditProposalResult {
    let mut adapter = MockModelAdapter::new(edit_response(content));
    let proposal = engine
        .edit_orchestrator
        .propose_edit(root, "change it", &[], None, &mut adapter)
        .expect("the edit should be proposed");
    engine
        .edit_orchestrator
        .apply_stored_patch(root, &proposal.patch.id, None, None, "tester", false)
        .expect("the patch applies with no baseline gate");
    proposal
}

fn sink_run<T>(
    cancel: &CancelToken,
    progress: &mut Vec<TurnProgress>,
    run: impl FnOnce(&mut TurnSink<'_>) -> T,
) -> T {
    let mut on_token = |_token: &str| {};
    let mut on_progress = |event: TurnProgress| progress.push(event);
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    run(&mut sink)
}

fn verify(engine: &WorkspaceEngine, root: &Path, session_id: &str) -> ChatTurnResult {
    let mut adapter = MockModelAdapter::new("unused");
    let mut progress = Vec::new();
    sink_run(&CancelToken::new(), &mut progress, |sink| {
        engine
            .chat_orchestrator
            .verify_due(root, session_id, &mut adapter, sink)
    })
    .expect("the verification runs")
}

fn decide(
    engine: &WorkspaceEngine,
    batch_id: &str,
    decisions: &[(String, bool)],
    cancel: &CancelToken,
) -> ChatTurnResult {
    let mut adapter = MockModelAdapter::new("unused");
    let mut progress = Vec::new();
    sink_run(cancel, &mut progress, |sink| {
        engine.chat_orchestrator.resume_after_check_decision(
            batch_id,
            decisions,
            "tester",
            &mut adapter,
            sink,
        )
    })
    .expect("the decision resumes the verification")
}

fn baselines(engine: &WorkspaceEngine, session_id: &str) -> Vec<(CheckId, CheckBaseline)> {
    engine
        .session_store
        .read_verification_events(session_id, &[])
        .unwrap()
        .into_iter()
        .filter_map(|event| match event {
            VerificationEvent::Baseline {
                check, baseline, ..
            } => Some((check, baseline)),
            _ => None,
        })
        .collect()
}

fn provenance(
    engine: &WorkspaceEngine,
    proposal: &EditProposalResult,
) -> BTreeMap<CheckId, workspace_engine::CheckProvenanceRecord> {
    engine
        .session_store
        .task_verification_provenance(&proposal.session.id, &proposal.task.id)
        .unwrap()
}

fn cargo_test() -> CheckId {
    CheckId::new("", "cargo test")
}

/// Every file under `root`, `.git` included, with its bytes.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

fn cleanup(root: &Path, data_dir: &Path) {
    assert!(
        temporary_trees(data_dir, root).is_empty(),
        "every temporary task-start tree is removed"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(data_dir);
}

/// Runs one fixture end to end: the task-start state, the patch, and the
/// classification the failure-time baseline produced.
fn classify_fixture(
    name: &str,
    committed: &str,
    uncommitted: Option<&str>,
    patched: &str,
) -> (FailureProvenance, workspace_engine::CheckProvenanceRecord) {
    let root = git_fixture(name, committed, uncommitted);
    let data_dir = temp_dir(&format!("{name}-data"));
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, false);
    let proposal = propose_and_apply(&engine, &root, patched);
    verify(&engine, &root, &proposal.session.id);

    let recorded = baselines(&engine, &proposal.session.id);
    assert_eq!(
        recorded.len(),
        1,
        "one baseline, for the failed check: {recorded:?}"
    );
    assert_eq!(recorded[0].0, cargo_test());
    // The baseline ran once in a temporary tree, never in the checkout.
    let runs = runs(&data_dir);
    assert_eq!(runs.len(), 2, "the live check and its baseline: {runs:?}");
    let canonical = fs::canonicalize(&root).unwrap();
    assert!(
        runs.iter()
            .any(|run| Path::new(run) == canonical || Path::new(run) == root)
    );
    assert!(
        runs.iter()
            .any(|run| run.contains("damaian-baseline-tree_")),
        "the baseline ran in the task-start tree: {runs:?}"
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    cleanup(&root, &data_dir);
    (record.provenance, record)
}

// --- Spec 102's provenance fixtures, now classified from a failure-time
// baseline. Each one gives the same class the pre-apply baseline gave. ---

#[test]
fn a_check_that_passed_at_task_start_is_a_new_regression() {
    let (class, record) = classify_fixture(
        "introduced",
        "pub fn a() {}\n",
        None,
        "pub fn a() {}\n// fail tests::a\n",
    );
    assert_eq!(class, FailureProvenance::NewRegression);
    assert_eq!(record.introduced.len(), 1, "{record:?}");
}

#[test]
fn the_same_failure_as_at_task_start_is_pre_existing() {
    // The failure is in the user's uncommitted state at task start, so the
    // tree must carry that state, not `HEAD`.
    let (class, _) = classify_fixture(
        "pre-existing",
        "pub fn a() {}\n",
        Some("pub fn a() {}\n// fail tests::a\n"),
        "pub fn a() { let _ = 1; }\n// fail tests::a\n",
    );
    assert_eq!(class, FailureProvenance::PreExisting);
}

#[test]
fn a_new_failure_beside_a_pre_existing_one_is_worsened_with_only_the_new_one_introduced() {
    let (class, record) = classify_fixture(
        "worsened",
        "pub fn a() {}\n// fail tests::a\n",
        None,
        "pub fn a() {}\n// fail tests::a\n// fail tests::b\n",
    );
    assert_eq!(class, FailureProvenance::Worsened);
    assert_eq!(record.introduced.len(), 1, "{record:?}");
    assert!(
        format!("{:?}", record.introduced[0]).contains("tests::b"),
        "{record:?}"
    );
}

#[test]
fn fewer_failures_than_at_task_start_is_unchanged() {
    let (class, record) = classify_fixture(
        "unchanged",
        "pub fn a() {}\n// fail tests::a\n// fail tests::b\n",
        None,
        "pub fn a() {}\n// fail tests::a\n",
    );
    assert_eq!(class, FailureProvenance::UnchangedFailure);
    assert!(record.introduced.is_empty());
}

#[test]
fn a_passing_check_never_takes_a_baseline() {
    let root = git_fixture("passing", "pub fn a() {}\n", None);
    let data_dir = temp_dir("passing-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, false);
    let proposal = propose_and_apply(&engine, &root, "pub fn a() { let _ = 2; }\n");
    assert!(
        baselines(&engine, &proposal.session.id).is_empty(),
        "applying a patch takes no baseline"
    );
    let result = verify(&engine, &root, &proposal.session.id);
    assert!(result.check_batch.is_none());
    assert!(baselines(&engine, &proposal.session.id).is_empty());
    assert_eq!(runs(&data_dir).len(), 1, "only the live check ran");
    cleanup(&root, &data_dir);
}

#[test]
fn a_reconstruction_mismatch_is_unknown_provenance_and_runs_nothing() {
    let committed = "pub fn a() {}\n";
    let root = git_fixture("mismatch", committed, None);
    let data_dir = temp_dir("mismatch-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, false);
    let proposal = propose_and_apply(&engine, &root, "pub fn a() {}\n// fail tests::a\n");
    // Corrupt every record of the task-start content: the patch's rollback
    // snapshot and the checkpoint no longer match the bytes they name.
    let real = workspace_engine::hash::sha256(committed);
    for directory in ["rollback", "checkpoints"] {
        let mut stack = vec![data_dir.join(directory)];
        while let Some(directory) = stack.pop() {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(text) = fs::read_to_string(&path)
                    && text.contains(&real)
                {
                    fs::write(&path, text.replace(&real, "sha256:0000")).unwrap();
                }
            }
        }
    }
    verify(&engine, &root, &proposal.session.id);

    let recorded = baselines(&engine, &proposal.session.id);
    assert!(
        matches!(
            recorded.as_slice(),
            [(_, CheckBaseline::Unavailable { .. })]
        ),
        "{recorded:?}"
    );
    assert_eq!(
        runs(&data_dir).len(),
        1,
        "no baseline ran on a guessed tree"
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::UnknownProvenance);
    cleanup(&root, &data_dir);
}

#[test]
fn a_baseline_leaves_the_users_checkout_byte_identical() {
    let root = git_fixture(
        "untouched",
        "pub fn a() {}\n",
        Some("pub fn a() {}\n// a local edit\n"),
    );
    write(&root, "notes/untracked.txt", "kept\n");
    let data_dir = temp_dir("untouched-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, false);
    let proposal = propose_and_apply(&engine, &root, "pub fn a() {}\n// fail tests::a\n");
    let before = snapshot(&root);
    verify(&engine, &root, &proposal.session.id);
    assert_eq!(
        baselines(&engine, &proposal.session.id).len(),
        1,
        "a baseline ran"
    );
    let after = snapshot(&root);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "no file appeared in or vanished from the checkout or its .git"
    );
    assert!(before == after, "every byte of the checkout is unchanged");
    cleanup(&root, &data_dir);
}

#[test]
fn a_baseline_that_needs_approval_asks_once_with_why_and_then_classifies() {
    let root = git_fixture("approval", "pub fn a() {}\n", None);
    let data_dir = temp_dir("approval-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, true);
    let proposal = propose_and_apply(&engine, &root, "pub fn a() {}\n// fail tests::a\n");

    // The check itself needs approval first, as it always did.
    let first = verify(&engine, &root, &proposal.session.id);
    let card = first.check_batch.expect("the check needs approval");
    assert!(card.title.is_none(), "the ordinary check card");
    let decisions = card
        .checks
        .iter()
        .map(|check| (check.id.clone(), true))
        .collect::<Vec<_>>();
    let failed = decide(&engine, &card.id, &decisions, &CancelToken::new());

    // It failed and has no baseline: one card that says why.
    let card = failed
        .check_batch
        .expect("the failure's baseline needs approval");
    assert_eq!(card.checks.len(), 1);
    assert_eq!(card.checks[0].command, "cargo test");
    assert!(
        card.note
            .as_deref()
            .is_some_and(|note| note.contains("before this task's changes")),
        "{card:?}"
    );
    assert!(card.title.is_some());
    assert!(baselines(&engine, &proposal.session.id).is_empty());
    assert_eq!(runs(&data_dir).len(), 1, "nothing ran before the answer");

    let decisions = vec![(card.checks[0].id.clone(), true)];
    decide(&engine, &card.id, &decisions, &CancelToken::new());
    let recorded = baselines(&engine, &proposal.session.id);
    assert!(
        matches!(recorded.as_slice(), [(_, CheckBaseline::Passed)]),
        "{recorded:?}"
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::NewRegression);
    cleanup(&root, &data_dir);
}

#[test]
fn a_cancelled_baseline_is_declined_and_its_tree_is_removed() {
    let root = git_fixture("cancel", "pub fn a() {}\n", None);
    let data_dir = temp_dir("cancel-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, true);
    let proposal = propose_and_apply(&engine, &root, "pub fn a() {}\n// fail tests::a\n");
    let first = verify(&engine, &root, &proposal.session.id);
    let card = first.check_batch.expect("the check needs approval");
    let decisions = card
        .checks
        .iter()
        .map(|check| (check.id.clone(), true))
        .collect::<Vec<_>>();
    let failed = decide(&engine, &card.id, &decisions, &CancelToken::new());
    let card = failed.check_batch.expect("the baseline card");

    // The user stops the turn: the approved baseline never starts.
    let cancel = CancelToken::new();
    cancel.cancel();
    let decisions = vec![(card.checks[0].id.clone(), true)];
    decide(&engine, &card.id, &decisions, &cancel);
    let recorded = baselines(&engine, &proposal.session.id);
    assert!(
        matches!(recorded.as_slice(), [(_, CheckBaseline::Declined)]),
        "{recorded:?}"
    );
    assert_eq!(runs(&data_dir).len(), 1, "the baseline did not run");
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::UnknownProvenance);
    cleanup(&root, &data_dir);
}

#[test]
fn a_repository_without_git_is_an_unavailable_baseline() {
    let root = temp_dir("no-git");
    write(&root, "Cargo.toml", "[package]\nname = \"fixture\"\n");
    write(&root, "src/lib.rs", "pub fn a() {}\n");
    let data_dir = temp_dir("no-git-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, false);
    let proposal = propose_and_apply(&engine, &root, "pub fn a() {}\n// fail tests::a\n");
    verify(&engine, &root, &proposal.session.id);
    let recorded = baselines(&engine, &proposal.session.id);
    assert!(
        matches!(
            recorded.as_slice(),
            [(_, CheckBaseline::Unavailable { .. })]
        ),
        "{recorded:?}"
    );
    cleanup(&root, &data_dir);
}

#[test]
fn a_baseline_cancelled_while_it_runs_is_declined_and_its_tree_is_removed() {
    let root = git_fixture("cancel-running", "pub fn a() {}\n", None);
    let data_dir = temp_dir("cancel-running-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, true);
    let proposal = propose_and_apply(&engine, &root, "pub fn a() {}\n// fail tests::a\n");
    let first = verify(&engine, &root, &proposal.session.id);
    let card = first.check_batch.expect("the check needs approval");
    let decisions = card
        .checks
        .iter()
        .map(|check| (check.id.clone(), true))
        .collect::<Vec<_>>();
    let failed = decide(&engine, &card.id, &decisions, &CancelToken::new());
    let card = failed.check_batch.expect("the baseline card");

    // The user stops the turn as soon as the baseline starts, after its tree
    // was built.
    let cancel = CancelToken::new();
    let mut started = false;
    let mut adapter = MockModelAdapter::new("unused");
    let mut on_token = |_token: &str| {};
    let mut on_progress = |event: TurnProgress| {
        if let TurnProgress::Phase(phase) = &event
            && phase.label.starts_with("Running baseline")
        {
            started = true;
            cancel.cancel();
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_after_check_decision(
            &card.id,
            &[(card.checks[0].id.clone(), true)],
            "tester",
            &mut adapter,
            &mut sink,
        )
        .expect("the decision resumes the verification");
    assert!(started, "the baseline started");
    let recorded = baselines(&engine, &proposal.session.id);
    assert!(
        matches!(recorded.as_slice(), [(_, CheckBaseline::Declined)]),
        "{recorded:?}"
    );
    cleanup(&root, &data_dir);
}
