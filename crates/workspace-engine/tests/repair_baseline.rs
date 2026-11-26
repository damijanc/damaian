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
    FailureProvenance, IsolatedRerunOutcome, MockModelAdapter, TurnProgress, TurnSink,
    VerificationEvent, WorkspaceEngine,
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
/// appends that directory to `runs.log` so a test can tell where it ran. Like
/// libtest, a command ending in `-- --exact <names>` runs only those tests
/// (spec 109 D3); every command is appended to `commands.log`. A test exists
/// in a tree when a `// test <name>` or `// fail <name>` line names it. A
/// narrowed `cargo nextest run` that matches no existing test fails as nextest
/// 0.9.144 does (`error: no tests to run`, exit 4) unless it passes
/// `--no-tests=pass`; a failing nextest run reports on stderr in nextest's
/// shape, exit 100. A test named on a `// flaky <name>` line depends on the
/// order tests run in (spec 114 D3): it fails in a whole run and passes when
/// a command names it with `--exact`.
fn fake_shell(data_dir: &Path) -> PathBuf {
    let script = data_dir.join("fake-shell.sh");
    let log = data_dir.join("runs.log");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             echo \"$PWD\" >> '{}'\n\
             for cmd; do :; done\n\
             echo \"$cmd\" >> '{}'\n\
             if [ -f '{}' ]; then sleep 30; fi\n\
             names=$(sed -n 's|^// fail ||p' src/lib.rs 2>/dev/null)\n\
             flaky=$(sed -n 's|^// flaky ||p' src/lib.rs 2>/dev/null)\n\
             case \"$cmd\" in *' -- --exact '*) ;; *)\n\
             if [ -n \"$flaky\" ]; then names=\"$names $flaky\"; fi;;\n\
             esac\n\
             case \"$cmd\" in *' -- --exact '*)\n\
             only=${{cmd#*' -- --exact '}}; kept=''; matched=''\n\
             existing=$(sed -n 's|^// \\(test\\|fail\\|flaky\\) ||p' src/lib.rs 2>/dev/null)\n\
             for name in $existing; do for one in $only; do\n\
             if [ \"$name\" = \"$one\" ]; then matched=yes; fi\n\
             done; done\n\
             for name in $names; do for one in $only; do\n\
             if [ \"$name\" = \"$one\" ]; then kept=\"$kept $name\"; fi\n\
             done; done\n\
             names=$kept\n\
             case \"$cmd\" in *'nextest run'*)\n\
             case \"$cmd\" in *--no-tests=pass*) ;; *)\n\
             if [ -z \"$matched\" ]; then echo 'error: no tests to run' >&2; exit 4; fi;;\n\
             esac;;\n\
             esac;;\n\
             esac\n\
             case \"$cmd\" in *'nextest run'*)\n\
             if [ -n \"$names\" ]; then\n\
             for name in $names; do echo \"        FAIL [   0.001s] fixture $name\" >&2; done\n\
             echo 'error: test run failed' >&2\n\
             exit 100\n\
             fi\n\
             exit 0;;\n\
             esac\n\
             if [ -n \"$names\" ]; then\n\
             echo 'failures:'\n\
             for name in $names; do echo \"    $name\"; done\n\
             echo ''\n\
             echo 'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s'\n\
             exit 101\n\
             fi\n\
             echo 'test result: ok. 1 passed; 0 failed'\n\
             exit 0\n",
            log.display(),
            data_dir.join("commands.log").display(),
            slow_marker(data_dir).display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    script
}

/// While this file exists the fake shell sleeps before answering, so a test
/// can stop a run that is still in progress instead of racing one that has
/// already finished.
fn slow_marker(data_dir: &Path) -> PathBuf {
    data_dir.join("slow-runs")
}

/// Every command the fake shell ran, in order.
fn commands(data_dir: &Path) -> Vec<String> {
    fs::read_to_string(data_dir.join("commands.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
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

/// Every `verification_isolated_rerun` (spec 114 D3): its tests, the ones
/// that passed alone, and its outcome.
fn isolated_reruns(
    engine: &WorkspaceEngine,
    session_id: &str,
) -> Vec<(Vec<String>, Vec<String>, IsolatedRerunOutcome)> {
    engine
        .session_store
        .read_verification_events(session_id, &[])
        .unwrap()
        .into_iter()
        .filter_map(|event| match event {
            VerificationEvent::IsolatedRerun {
                tests,
                passed_alone,
                outcome,
                ..
            } => Some((tests, passed_alone, outcome)),
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
    // The baseline ran once in a temporary tree, never in the checkout. A
    // narrowed baseline that passed is followed by one rerun alone in the
    // checkout (spec 114 D3).
    let runs = runs(&data_dir);
    let reruns = isolated_reruns(&engine, &proposal.session.id).len();
    assert_eq!(
        runs.len(),
        2 + reruns,
        "the live check, its baseline and any rerun alone: {runs:?}"
    );
    assert_eq!(
        runs.iter()
            .filter(|run| run.contains("damaian-baseline-tree_"))
            .count(),
        1,
        "{runs:?}"
    );
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

/// Spec 102's "unchanged" fixture. A whole-check baseline saw both tests fail
/// and read the smaller failure as `unchanged_failure`. Since spec 109 D3 the
/// baseline reruns only the test still failing, which failed at task start
/// too, so it reads `pre_existing`: the dropped test is not rerun. Both
/// classes are the user's to review, never a repair target, and the
/// completion gate treats them alike; `provenance::classify` still gives
/// `unchanged_failure` for a whole-check baseline
/// (`repair_targets::a_failed_baseline_with_no_new_evidence_is_unchanged_failure`).
#[test]
fn fewer_failures_than_at_task_start_is_not_the_tasks() {
    let (class, record) = classify_fixture(
        "unchanged",
        "pub fn a() {}\n// fail tests::a\n// fail tests::b\n",
        None,
        "pub fn a() {}\n// fail tests::a\n",
    );
    assert_eq!(class, FailureProvenance::PreExisting);
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
    // The card shows what runs: only the failing test (spec 109 D3).
    assert_eq!(card.checks[0].command, "cargo test -- --exact tests::a");
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
    // was built. The baseline sleeps until it is stopped, so the stop always
    // lands on a running command (a fast fake run could finish first).
    fs::write(slow_marker(&data_dir), "").unwrap();
    let cancel = CancelToken::new();
    let mut started = false;
    let mut adapter = MockModelAdapter::new("unused");
    let mut on_token = |_token: &str| {};
    let mut on_progress = |event: TurnProgress| {
        if let TurnProgress::Phase(phase) = &event
            && phase.baseline
            && phase.label.starts_with("Checking whether")
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

// --- Spec 109 Task 4 (D3): a Rust test failure reruns only its failing
// tests by name. ---

/// The tests each recorded baseline reran by name (empty: the whole check).
fn baseline_scopes(engine: &WorkspaceEngine, session_id: &str) -> Vec<Vec<String>> {
    engine
        .session_store
        .read_verification_events(session_id, &[])
        .unwrap()
        .into_iter()
        .filter_map(|event| match event {
            VerificationEvent::Baseline { tests, .. } => Some(tests),
            _ => None,
        })
        .collect()
}

#[test]
fn two_failing_tests_rerun_alone_by_name() {
    // At task start `tests::c` already failed. The patch fixes it and breaks
    // `tests::a` and `tests::b`. A whole-check baseline would see `tests::c`
    // fail; the targeted one runs only `a` and `b`, which passed then.
    let root = git_fixture("targeted", "pub fn a() {}\n// fail tests::c\n", None);
    let data_dir = temp_dir("targeted-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, false);
    let proposal = propose_and_apply(
        &engine,
        &root,
        "pub fn a() {}\n// fail tests::b\n// fail tests::a\n",
    );
    verify(&engine, &root, &proposal.session.id);

    assert_eq!(
        commands(&data_dir),
        [
            "cargo test",
            "cargo test -- --exact tests::a tests::b",
            "cargo test -- --exact tests::a tests::b"
        ],
        "the live check, then only its two failing tests in the task-start tree, then \
         the same two alone on the current files (spec 114 D3), where they still fail"
    );
    let recorded = baselines(&engine, &proposal.session.id);
    assert!(
        matches!(recorded.as_slice(), [(check, CheckBaseline::Passed)] if *check == cargo_test()),
        "the baseline belongs to the whole check: {recorded:?}"
    );
    assert_eq!(
        baseline_scopes(&engine, &proposal.session.id),
        [vec!["tests::a".to_string(), "tests::b".to_string()]]
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::NewRegression);
    assert_eq!(record.introduced.len(), 2, "{record:?}");
    cleanup(&root, &data_dir);
}

#[test]
fn a_mixed_targeted_failure_is_worsened_with_only_the_introduced_test() {
    // `tests::a` failed at task start; the patch adds `tests::b`. Rerun by
    // name, `a` still fails and `b` passes, so only `b` is the task's.
    let root = git_fixture("mixed", "pub fn a() {}\n// fail tests::a\n", None);
    let data_dir = temp_dir("mixed-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, false);
    let proposal = propose_and_apply(
        &engine,
        &root,
        "pub fn a() {}\n// fail tests::a\n// fail tests::b\n",
    );
    verify(&engine, &root, &proposal.session.id);

    assert_eq!(
        commands(&data_dir).last().map(String::as_str),
        Some("cargo test -- --exact tests::a tests::b")
    );
    let recorded = baselines(&engine, &proposal.session.id);
    assert!(
        matches!(recorded.as_slice(), [(_, CheckBaseline::Failed { .. })]),
        "{recorded:?}"
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::Worsened);
    assert_eq!(record.introduced.len(), 1, "{record:?}");
    assert!(
        record.introduced[0].summary().starts_with("tests::b"),
        "{record:?}"
    );
    cleanup(&root, &data_dir);
}

#[test]
fn the_whole_check_reruns_when_only_the_narrowed_command_would_need_a_card() {
    // The user allowed `cargo test` itself; the narrowed command is a
    // different command the policy would ask about. Narrowing must never add
    // a card, so the whole check reruns, as it did before spec 109 Task 4.
    let root = git_fixture("allowed", "pub fn a() {}\n", None);
    let data_dir = temp_dir("allowed-data");
    let shell = fake_shell(&data_dir);
    let engine = WorkspaceEngine::new(Config {
        data_dir: data_dir.to_path_buf(),
        enable_index_watcher: false,
        shell: shell.to_string_lossy().to_string(),
        require_approval_for_risky_commands: true,
        command_allowlist: vec!["cargo test".to_string()],
        agent_tool_retry_limit: 0,
        ..Config::default()
    });
    let proposal = propose_and_apply(&engine, &root, "pub fn a() {}\n// fail tests::a\n");
    let result = verify(&engine, &root, &proposal.session.id);

    assert!(
        result.check_batch.is_none(),
        "no card: {:?}",
        result.check_batch
    );
    assert_eq!(commands(&data_dir), ["cargo test", "cargo test"]);
    assert_eq!(
        baseline_scopes(&engine, &proposal.session.id),
        [Vec::<String>::new()]
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::NewRegression);
    cleanup(&root, &data_dir);
}

// --- Task 4 review fix: a baseline whose failing tests were all added by the
// patch. They do not exist in the task-start tree, so the narrowed run
// matches nothing; that must read as a pass, never as a failure. ---

#[test]
fn a_new_failing_test_is_a_new_regression_under_cargo_test() {
    let root = git_fixture("added-cargo", "pub fn a() {}\n// test tests::old\n", None);
    let data_dir = temp_dir("added-cargo-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, false);
    let proposal = propose_and_apply(
        &engine,
        &root,
        "pub fn a() {}\n// test tests::old\n// fail tests::added\n",
    );
    verify(&engine, &root, &proposal.session.id);

    assert_eq!(
        commands(&data_dir),
        [
            "cargo test",
            "cargo test -- --exact tests::added",
            // Spec 114 D3: rerun alone on the current files, where it fails.
            "cargo test -- --exact tests::added"
        ]
    );
    assert_eq!(
        isolated_reruns(&engine, &proposal.session.id),
        [(
            vec!["tests::added".to_string()],
            Vec::new(),
            IsolatedRerunOutcome::Ran
        )]
    );
    let recorded = baselines(&engine, &proposal.session.id);
    assert!(
        matches!(recorded.as_slice(), [(_, CheckBaseline::Passed)]),
        "an empty libtest run passes: {recorded:?}"
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::NewRegression);
    assert_eq!(record.introduced.len(), 1, "{record:?}");
    assert!(record.introduced[0].summary().starts_with("tests::added"));
    cleanup(&root, &data_dir);
}

#[test]
fn a_new_failing_test_is_a_new_regression_under_nextest() {
    use workspace_engine::baseline::FailedCheck;
    use workspace_engine::finding::{Finding, FindingDraft, FindingSource, Severity};
    use workspace_engine::{
        BaselineCoordinator, BaselinePreparation, CheckTermination, FindingEvidence,
        PostPatchResult, SecretScanner, classify,
    };

    let root = git_fixture("added-nextest", "pub fn a() {}\n// test tests::old\n", None);
    let data_dir = temp_dir("added-nextest-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, false);
    let proposal = propose_and_apply(
        &engine,
        &root,
        "pub fn a() {}\n// test tests::old\n// fail tests::added\n",
    );
    // The plan's nextest check failed after the patch on the one test the
    // patch added (the fixture's detected check is `cargo test`, so the
    // failure is handed to the coordinator directly).
    let check = CheckId::new("", "cargo nextest run");
    let evidence = vec![FindingEvidence::from_finding(&Finding::new(
        FindingDraft {
            source: FindingSource::Test,
            severity: Severity::Error,
            summary: "tests::added failed".to_string(),
            details: None,
            range: None,
            code: None,
        },
        &SecretScanner::default(),
    ))];
    let coordinator = BaselineCoordinator::new(
        engine.config.clone(),
        engine.scanner.clone(),
        engine.audit_log.clone(),
        engine.session_store.clone(),
        engine.validation_orchestrator.clone(),
        engine.checkpoint_store.clone(),
    );
    let preparation = coordinator
        .prepare_for_failures(
            &root,
            &proposal.session.id,
            &proposal.task.id,
            &[FailedCheck {
                check: check.clone(),
                findings: evidence.clone(),
            }],
            &CancelToken::new(),
            &mut |_| {},
        )
        .expect("the baseline is prepared");
    // Damaian does not know `cargo nextest`, so the baseline asks first; the
    // card shows exactly what will run.
    let BaselinePreparation::Awaiting(batch) = preparation else {
        panic!("the nextest baseline asks first: {preparation:?}");
    };
    assert_eq!(
        batch.checks[0].command,
        "cargo nextest run --no-tests=pass -- --exact tests::added"
    );
    coordinator
        .resume_streamed(
            &root,
            &batch.id,
            &[(batch.checks[0].proposal_id.clone(), true)],
            &[],
            "tester",
            &CancelToken::new(),
            &mut |_| {},
        )
        .expect("the approved baseline runs");

    assert_eq!(
        commands(&data_dir),
        ["cargo nextest run --no-tests=pass -- --exact tests::added"],
        "only the added test, with an empty run allowed to pass"
    );
    let recorded = baselines(&engine, &proposal.session.id);
    assert!(
        matches!(recorded.as_slice(), [(id, CheckBaseline::Passed)] if *id == check),
        "nothing to run at task start is a pass, not a failure: {recorded:?}"
    );
    let classification = classify(
        &PostPatchResult::new(check, CheckTermination::Exited { exit_code: 100 }, evidence),
        recorded.first().map(|(_, baseline)| baseline),
        None,
    );
    assert_eq!(classification.provenance, FailureProvenance::NewRegression);
    assert_eq!(classification.introduced.len(), 1, "{classification:?}");
    cleanup(&root, &data_dir);
}

// --- Spec 114 Task 2 (D3): failing tests that pass alone. A narrowed
// baseline that passed is followed by one rerun of the same tests alone on
// the current files; the tests that pass there too are not the task's. ---

/// `tests::a`, `tests::b` and `tests::c` all pass at task start.
const THREE_TESTS: &str = "pub fn a() {}\n// test tests::a\n// test tests::b\n// test tests::c\n";

fn alone_engine(data_dir: &Path, shell: &Path, retry_limit: u32) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: data_dir.to_path_buf(),
        enable_index_watcher: false,
        shell: shell.to_string_lossy().to_string(),
        require_approval_for_risky_commands: false,
        agent_tool_retry_limit: retry_limit,
        ..Config::default()
    })
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

#[test]
fn only_the_test_that_fails_alone_is_handed_to_repair() {
    // The 2026-11-16 shape: three tests fail in the whole run, all three pass
    // alone at task start, and on the current files two pass alone.
    let root = git_fixture("alone-mixed", THREE_TESTS, None);
    let data_dir = temp_dir("alone-mixed-data");
    let shell = fake_shell(&data_dir);
    let engine = alone_engine(&data_dir, &shell, 0);
    let proposal = propose_and_apply(
        &engine,
        &root,
        "pub fn a() {}\n// flaky tests::a\n// flaky tests::b\n// fail tests::c\n",
    );
    let result = verify(&engine, &root, &proposal.session.id);

    let narrowed = "cargo test -- --exact tests::a tests::b tests::c";
    assert_eq!(commands(&data_dir), ["cargo test", narrowed, narrowed]);
    let canonical = fs::canonicalize(&root).unwrap();
    let last = runs(&data_dir).last().cloned().unwrap();
    assert!(
        Path::new(&last) == canonical || Path::new(&last) == root,
        "the rerun runs in the checkout, on the current files: {last}"
    );
    assert_eq!(
        isolated_reruns(&engine, &proposal.session.id),
        [(
            names(&["tests::a", "tests::b", "tests::c"]),
            names(&["tests::a", "tests::b"]),
            IsolatedRerunOutcome::Ran
        )]
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::NewRegression);
    assert_eq!(record.findings.len(), 3, "the report keeps every failure");
    assert_eq!(record.introduced.len(), 1, "{record:?}");
    assert!(record.introduced[0].summary().starts_with("tests::c"));
    let report = result.verification_report.expect("a report");
    let request = report.repair_request().expect("tests::c is repaired");
    assert!(request.contains("tests::c"), "{request}");
    assert!(
        !request.contains("tests::a") && !request.contains("tests::b"),
        "a test that passes alone is never a repair target: {request}"
    );
    cleanup(&root, &data_dir);
}

#[test]
fn a_check_whose_failing_tests_all_pass_alone_is_not_repaired_and_is_accepted() {
    let root = git_fixture("alone-all", THREE_TESTS, None);
    let data_dir = temp_dir("alone-all-data");
    let shell = fake_shell(&data_dir);
    // A repair would be allowed, so its absence is the classification's.
    let engine = alone_engine(&data_dir, &shell, 2);
    let proposal = propose_and_apply(
        &engine,
        &root,
        "pub fn a() {}\n// flaky tests::a\n// flaky tests::b\n// flaky tests::c\n",
    );
    let result = verify(&engine, &root, &proposal.session.id);

    let narrowed = "cargo test -- --exact tests::a tests::b tests::c";
    assert_eq!(
        commands(&data_dir),
        ["cargo test", narrowed, narrowed],
        "no repair ran anything"
    );
    let all = names(&["tests::a", "tests::b", "tests::c"]);
    assert_eq!(
        isolated_reruns(&engine, &proposal.session.id),
        [(all.clone(), all, IsolatedRerunOutcome::Ran)]
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::PassesAlone);
    assert!(record.introduced.is_empty(), "{record:?}");

    let report = result.verification_report.expect("a report");
    assert!(report.repair_request().is_none(), "no repair starts");
    assert!(report.repair_targets.is_empty());
    assert_eq!(
        report.outcome,
        Some(workspace_engine::VerificationOutcome::OnlyPreExistingFailures)
    );
    assert_eq!(
        report.verdict.as_ref().map(|verdict| verdict.kind),
        Some(workspace_engine::VerdictKind::NotCausedByTask)
    );
    let partition = &report.provenance[0];
    assert_eq!(partition.provenance, FailureProvenance::PassesAlone);
    assert_eq!(partition.label, "Passes when run alone");
    assert!(!partition.repair_eligible);
    assert_eq!(
        partition.checks[0].baseline,
        "Before this task's changes: the 3 failing tests passed when run alone, and on the current files too"
    );

    let gate = engine
        .session_store
        .read_completion_report(&proposal.session.id, &result.task.id)
        .unwrap()
        .expect("the gate settled the task");
    assert_eq!(gate.accepted_failures.len(), 1, "{gate:?}");
    let accepted = &gate.accepted_failures[0];
    assert_eq!(accepted.check, cargo_test());
    assert_eq!(accepted.provenance, FailureProvenance::PassesAlone);
    for name in ["tests::a", "tests::b", "tests::c"] {
        assert!(accepted.explanation.contains(name), "{accepted:?}");
    }
    cleanup(&root, &data_dir);
}

#[test]
fn a_rerun_alone_that_would_need_approval_is_skipped_without_a_card() {
    let root = git_fixture("alone-approval", "pub fn a() {}\n// test tests::a\n", None);
    let data_dir = temp_dir("alone-approval-data");
    let shell = fake_shell(&data_dir);
    let engine = engine(&data_dir, &shell, true);
    let proposal = propose_and_apply(&engine, &root, "pub fn a() {}\n// flaky tests::a\n");
    let first = verify(&engine, &root, &proposal.session.id);
    let card = first.check_batch.expect("the check needs approval");
    let decisions = card
        .checks
        .iter()
        .map(|check| (check.id.clone(), true))
        .collect::<Vec<_>>();
    let failed = decide(&engine, &card.id, &decisions, &CancelToken::new());
    let card = failed.check_batch.expect("the baseline card");
    let after = decide(
        &engine,
        &card.id,
        &[(card.checks[0].id.clone(), true)],
        &CancelToken::new(),
    );

    assert!(
        after.check_batch.is_none(),
        "the rerun never adds a card: {:?}",
        after.check_batch
    );
    assert_eq!(
        commands(&data_dir),
        ["cargo test", "cargo test -- --exact tests::a"],
        "the rerun did not run"
    );
    assert_eq!(
        isolated_reruns(&engine, &proposal.session.id),
        [(
            names(&["tests::a"]),
            Vec::new(),
            IsolatedRerunOutcome::Skipped
        )]
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(
        record.provenance,
        FailureProvenance::NewRegression,
        "classified as before"
    );
    cleanup(&root, &data_dir);
}

#[test]
fn a_rerun_alone_stopped_while_it_runs_is_cancelled_and_classifies_as_before() {
    let root = git_fixture("alone-stop", "pub fn a() {}\n// test tests::a\n", None);
    let data_dir = temp_dir("alone-stop-data");
    let shell = fake_shell(&data_dir);
    let engine = alone_engine(&data_dir, &shell, 0);
    let proposal = propose_and_apply(&engine, &root, "pub fn a() {}\n// flaky tests::a\n");

    // The baseline answers at once; once it has, every later run sleeps
    // until stopped, so the stop always lands on the running rerun.
    let slow = slow_marker(&data_dir);
    let cancel = CancelToken::new();
    let mut started = false;
    let mut adapter = MockModelAdapter::new("unused");
    let mut on_token = |_token: &str| {};
    let mut on_progress = |event: TurnProgress| {
        let TurnProgress::Phase(phase) = &event else {
            return;
        };
        if phase.baseline && phase.label.starts_with("Before this task's changes") {
            fs::write(&slow, "").unwrap();
        }
        if !phase.baseline && phase.label.starts_with("Checking whether the failing test") {
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
        .verify_due(&root, &proposal.session.id, &mut adapter, &mut sink)
        .expect("the verification finishes");
    assert!(started, "the rerun started");
    assert_eq!(
        isolated_reruns(&engine, &proposal.session.id),
        [(
            names(&["tests::a"]),
            Vec::new(),
            IsolatedRerunOutcome::Cancelled
        )]
    );
    let record = provenance(&engine, &proposal)
        .remove(&cargo_test())
        .expect("the failure is classified");
    assert_eq!(record.provenance, FailureProvenance::NewRegression);
    cleanup(&root, &data_dir);
}
