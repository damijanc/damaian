//! Spec 98 Task 4: `damaian protect-paths` lists, adds and removes a task's
//! explicit protected paths through the workspace engine.
//!
//! These are end-to-end through the real binary, like the other CLI tests: the
//! session and task are seeded with the engine, then every command runs with
//! `DAMAIAN_DATA_DIR` pointing at the same durable store.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{Config, WorkspaceEngine, repository_id_for_root};

struct Fixture {
    repo: PathBuf,
    data: PathBuf,
    session: String,
    task: String,
}

fn temp_dir(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("damaian-cli-protected-{name}-{stamp}"));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn engine(data: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: data.to_path_buf(),
        enable_index_watcher: false,
        ..Config::default()
    })
}

fn fixture(name: &str) -> Fixture {
    let repo = temp_dir(&format!("{name}-repo"));
    let data = temp_dir(&format!("{name}-data"));
    fs::create_dir_all(repo.join("src/nested")).expect("source dirs");
    fs::write(repo.join("src/a.rs"), "pub fn a() {}\n").expect("source file");
    fs::write(repo.join("src/nested/b.rs"), "pub fn b() {}\n").expect("nested file");
    let engine = engine(&data);
    let session = engine
        .session_store
        .create_session(&repository_id_for_root(&repo), "protected paths")
        .expect("session");
    let task = engine
        .session_store
        .create_task(&session.id, "Do the work.", "provider", "model")
        .expect("task");
    Fixture {
        repo,
        data,
        session: session.id,
        task: task.id,
    }
}

fn run(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_damaian"))
        .args(args)
        .env("DAMAIAN_DATA_DIR", data)
        .env_remove("DAMAIAN_ADMIN_CONFIG")
        .output()
        .expect("cli should run")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command should succeed: {}",
        stderr(output)
    );
}

#[test]
fn list_add_and_remove_show_canonical_paths_and_reload_from_the_store() {
    let fixture = fixture("roundtrip");
    let repo = fixture.repo.to_string_lossy().to_string();

    let listed = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "list",
        ],
    );
    assert_success(&listed);
    assert!(
        stdout(&listed).contains("protected paths for task") && stdout(&listed).contains("none"),
        "{}",
        stdout(&listed)
    );

    let added = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "add",
            "src/a.rs",
            "src/nested",
        ],
    );
    assert_success(&added);
    let added_text = stdout(&added);
    assert!(
        added_text.contains(
            "Protected src/a.rs (file), src/nested (directory (and everything under it))."
        ),
        "{added_text}"
    );
    assert!(
        added_text.contains("  src/a.rs  file")
            && added_text.contains("  src/nested  directory (and everything under it)"),
        "the refreshed active set must be printed: {added_text}"
    );

    let reloaded = engine(&fixture.data)
        .task_protected_paths(&fixture.session, &fixture.task)
        .expect("a fresh engine reads the durable state");
    assert_eq!(
        reloaded.path_list(),
        vec!["src/a.rs".to_string(), "src/nested".to_string()]
    );

    let removed = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "remove",
            "src/a.rs",
        ],
    );
    assert_success(&removed);
    let removed_text = stdout(&removed);
    assert!(
        removed_text.contains("Removed protection for src/a.rs (file)."),
        "{removed_text}"
    );
    assert!(
        !removed_text.contains("  src/a.rs  file"),
        "the refreshed active set must not list the removed path: {removed_text}"
    );

    let reloaded = engine(&fixture.data)
        .task_protected_paths(&fixture.session, &fixture.task)
        .expect("a fresh engine reads the removal");
    assert_eq!(reloaded.path_list(), vec!["src/nested".to_string()]);

    let listed = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "list",
        ],
    );
    assert_success(&listed);
    let listed_text = stdout(&listed);
    assert!(
        listed_text.contains("src/nested  directory"),
        "{listed_text}"
    );
    assert!(!listed_text.contains("src/a.rs"), "{listed_text}");

    fs::remove_dir_all(&fixture.repo).ok();
    fs::remove_dir_all(&fixture.data).ok();
}

#[test]
fn an_invalid_path_is_refused_with_its_reason_and_keeps_the_accepted_half() {
    let fixture = fixture("invalid");
    let repo = fixture.repo.to_string_lossy().to_string();

    let added = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "add",
            "src/nested",
            "src/missing.rs",
        ],
    );
    assert_success(&added);
    let text = stdout(&added);
    assert!(
        text.contains("Protected src/nested (directory (and everything under it))."),
        "{text}"
    );
    assert!(
        text.contains("Refused:")
            && text.contains("src/missing.rs")
            && text.contains("does not exist"),
        "{text}"
    );
    assert!(
        text.contains("  src/nested  directory (and everything under it)"),
        "{text}"
    );

    let active = engine(&fixture.data)
        .task_protected_paths(&fixture.session, &fixture.task)
        .expect("the accepted path is durable");
    assert_eq!(active.path_list(), vec!["src/nested".to_string()]);

    let removed = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "remove",
            "src/missing.rs",
        ],
    );
    assert_success(&removed);
    let removed_text = stdout(&removed);
    assert!(
        removed_text.contains("Refused:")
            && removed_text.contains("not currently protected")
            && removed_text.contains("src/missing.rs"),
        "{removed_text}"
    );
    assert!(
        removed_text.contains("  src/nested  directory (and everything under it)"),
        "{removed_text}"
    );

    fs::remove_dir_all(&fixture.repo).ok();
    fs::remove_dir_all(&fixture.data).ok();
}

#[test]
fn a_session_from_another_repository_is_refused_before_any_event() {
    let fixture = fixture("cross-repo");
    let other_repo = temp_dir("cross-repo-other");
    fs::write(other_repo.join("notes.txt"), "notes\n").expect("other repo file");
    let other_repo_arg = other_repo.to_string_lossy().to_string();

    let refused = run(
        &fixture.data,
        &[
            "protect-paths",
            &other_repo_arg,
            &fixture.session,
            &fixture.task,
            "list",
        ],
    );
    assert!(!refused.status.success(), "{}", stdout(&refused));
    assert!(
        stderr(&refused).contains("does not belong to repository"),
        "{}",
        stderr(&refused)
    );

    let written = run(
        &fixture.data,
        &[
            "protect-paths",
            &other_repo_arg,
            &fixture.session,
            &fixture.task,
            "add",
            "src/a.rs",
        ],
    );
    assert!(!written.status.success(), "{}", stdout(&written));

    let fresh = engine(&fixture.data);
    assert!(
        fresh
            .session_store
            .read_task_protected_path_events(&fixture.session)
            .expect("read events")
            .is_empty(),
        "a cross-repository request must not append an event"
    );

    fs::remove_dir_all(&fixture.repo).ok();
    fs::remove_dir_all(&other_repo).ok();
    fs::remove_dir_all(&fixture.data).ok();
}

#[test]
fn list_add_and_remove_keep_operating_on_a_named_older_task() {
    let fixture = fixture("older-task");
    let repo = fixture.repo.to_string_lossy().to_string();
    let seeded = engine(&fixture.data);
    let newer = seeded
        .session_store
        .create_task(&fixture.session, "Newer work.", "provider", "model")
        .expect("second task");

    let listed = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "list",
        ],
    );
    assert_success(&listed);
    assert!(
        stdout(&listed).contains(&format!("protected paths for task {}:", fixture.task)),
        "{}",
        stdout(&listed)
    );
    assert!(!stdout(&listed).contains(&newer.id));

    let added = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "add",
            "src/a.rs",
        ],
    );
    assert_success(&added);
    let added_text = stdout(&added);
    assert!(
        added_text.contains(&format!("protected paths for task {}:", fixture.task)),
        "{added_text}"
    );
    assert!(added_text.contains("  src/a.rs  file"), "{added_text}");

    let newer_control = engine(&fixture.data)
        .task_protected_paths_control_for_task(&fixture.repo, &fixture.session, &newer.id)
        .expect("the newer task's own control");
    assert!(newer_control.rows.is_empty());

    let removed = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "remove",
            "src/a.rs",
        ],
    );
    assert_success(&removed);
    assert!(
        stdout(&removed).contains("Removed protection for src/a.rs (file)."),
        "{}",
        stdout(&removed)
    );
    assert!(
        engine(&fixture.data)
            .task_protected_paths(&fixture.session, &fixture.task)
            .expect("durable state")
            .is_empty()
    );

    fs::remove_dir_all(&fixture.repo).ok();
    fs::remove_dir_all(&fixture.data).ok();
}

#[test]
fn the_cli_prints_the_engine_result_instead_of_rebuilding_it() {
    let source = include_str!("../src/main.rs");
    assert!(!source.contains("ProtectedPathKind"), "{source}");
    assert!(
        !source.contains("fn protected_path_update_text"),
        "{source}"
    );
    assert!(!source.contains("fn protected_path_kind_label"), "{source}");
    assert!(source.contains("task_protected_paths_control"), "{source}");
    assert!(source.contains("update.outcome"), "{source}");
    assert!(
        source.contains("protected_paths_text(&update.control)"),
        "{source}"
    );
    assert!(
        !source.contains("accepted {}  {}"),
        "the CLI must not compose acceptance text itself"
    );
}

#[test]
fn help_and_missing_arguments_name_the_required_shape() {
    let fixture = fixture("usage");
    let repo = fixture.repo.to_string_lossy().to_string();

    let help = run(&fixture.data, &["--help"]);
    assert_success(&help);
    assert!(
        stdout(&help).contains("damaian protect-paths <repo> <session> <task> list"),
        "{}",
        stdout(&help)
    );
    assert!(
        stdout(&help).contains("add <path...>") && stdout(&help).contains("remove <path...>"),
        "{}",
        stdout(&help)
    );

    let missing_action = run(
        &fixture.data,
        &["protect-paths", &repo, &fixture.session, &fixture.task],
    );
    assert!(!missing_action.status.success());
    assert!(
        stderr(&missing_action).contains("protect-paths needs list, add, or remove"),
        "{}",
        stderr(&missing_action)
    );

    let missing_paths = run(
        &fixture.data,
        &[
            "protect-paths",
            &repo,
            &fixture.session,
            &fixture.task,
            "add",
        ],
    );
    assert!(!missing_paths.status.success());
    assert!(
        stderr(&missing_paths).contains("protect-paths add needs at least one path"),
        "{}",
        stderr(&missing_paths)
    );

    fs::remove_dir_all(&fixture.repo).ok();
    fs::remove_dir_all(&fixture.data).ok();
}
