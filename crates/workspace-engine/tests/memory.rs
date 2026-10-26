//! Spec 28: the memory record, project identity and store.
//!
//! Identity is the part that cannot be proved by reading one file: two clones
//! of one repository must share memory and two unrelated repositories must not,
//! at every resolution tier. These fixtures are real local Git repositories,
//! because the property under test is what Git reports.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    Config, GitReadCaps, GitService, ProjectIdentity, ProjectKeySource, SecretScanner,
    repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-memory-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn test_config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    }
}

fn git_service(config: &Config) -> GitService {
    let scanner = SecretScanner::new(Vec::new());
    let audit = workspace_engine::AuditLog::new(&config.data_dir, false, scanner.clone());
    let path_policy = workspace_engine::PathPolicy::new(config);
    GitService::new(
        audit,
        scanner,
        path_policy,
        GitReadCaps::from_config(config),
    )
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .expect("git should run");
    assert!(
        status.success(),
        "git {args:?} failed in {}",
        root.display()
    );
}

fn init_repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "user.name", "Fixture"]);
}

fn commit(root: &Path, name: &str, contents: &str) {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
    git(root, &["add", name]);
    git(root, &["commit", "-q", "-m", "fixture commit"]);
}

fn repo_with_a_commit(name: &str) -> PathBuf {
    let root = temp_dir(name);
    init_repo(&root);
    // The name goes into the tree so two fixtures never mint the same commit
    // id by sharing content, author and timestamp within one second.
    commit(&root, "README.md", &format!("# Fixture {name}\n"));
    root
}

fn identity(root: &Path) -> ProjectIdentity {
    let config = test_config(root);
    ProjectIdentity::resolve_for_root(root, &git_service(&config)).unwrap()
}

#[test]
fn two_checkouts_of_one_repository_share_a_root_commit_identity() {
    let origin = repo_with_a_commit("identity-origin");
    let clone = temp_dir("identity-clone");
    let status = Command::new("git")
        .args(["clone", "-q"])
        .arg(&origin)
        .arg(&clone)
        .status()
        .unwrap();
    assert!(status.success());

    let first = identity(&origin);
    let second = identity(&clone);

    assert_eq!(first.source, ProjectKeySource::RootCommit);
    assert_eq!(second.source, ProjectKeySource::RootCommit);
    assert_eq!(first.key, second.key);
    assert_eq!(first.key, second.key, "the path must not enter the key");
    assert_ne!(
        first.repository_id, second.repository_id,
        "the path-derived id still differs, which is why memory needs its own key"
    );
}

#[test]
fn a_moved_or_renamed_checkout_keeps_its_identity() {
    let root = repo_with_a_commit("identity-move");
    let before = identity(&root);

    let renamed = root.with_file_name(format!(
        "{}-renamed",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(&root, &renamed).unwrap();
    let after = identity(&renamed);

    assert_eq!(before.key, after.key);
    assert_eq!(before.source, ProjectKeySource::RootCommit);
    assert_ne!(before.repository_id, after.repository_id);
}

#[test]
fn repositories_with_different_root_commits_do_not_share_memory() {
    let first = repo_with_a_commit("identity-first");
    let second = repo_with_a_commit("identity-second");

    let first = identity(&first);
    let second = identity(&second);

    assert_eq!(first.source, ProjectKeySource::RootCommit);
    assert_eq!(second.source, ProjectKeySource::RootCommit);
    assert_ne!(first.key, second.key);
}

#[test]
fn a_fork_shares_its_upstream_root_commit_deliberately() {
    // §5.2: a fork inherits its upstream's root commit, so it shares memory.
    // This is a stated sharing decision, not an accident; the test exists so a
    // change to it is visible.
    let upstream = repo_with_a_commit("identity-fork-upstream");
    let fork = temp_dir("identity-fork");
    assert!(
        Command::new("git")
            .args(["clone", "-q"])
            .arg(&upstream)
            .arg(&fork)
            .status()
            .unwrap()
            .success()
    );

    assert_eq!(identity(&upstream).key, identity(&fork).key);
}

#[test]
fn repositories_without_history_fall_to_the_normalized_remote() {
    let first = temp_dir("identity-remote-first");
    let second = temp_dir("identity-remote-second");
    init_repo(&first);
    init_repo(&second);
    git(
        &first,
        &[
            "remote",
            "add",
            "origin",
            "https://user:secret-token@GitHub.com/Owner/First.git",
        ],
    );
    git(
        &second,
        &["remote", "add", "origin", "git@github.com:owner/second.git"],
    );

    let first = identity(&first);
    let second = identity(&second);

    assert_eq!(first.source, ProjectKeySource::RemoteUrl);
    assert_eq!(second.source, ProjectKeySource::RemoteUrl);
    assert_ne!(first.key, second.key);
    assert!(
        !first.key.contains("secret-token"),
        "no credential in a key"
    );
    assert!(
        first.key.starts_with("remote_") && first.key.len() == "remote_".len() + 16,
        "{}",
        first.key
    );
}

#[test]
fn one_remote_at_two_checkouts_shares_memory_without_history() {
    let first = temp_dir("identity-same-remote-first");
    let second = temp_dir("identity-same-remote-second");
    for root in [&first, &second] {
        init_repo(root);
        git(
            root,
            &["remote", "add", "origin", "git@github.com:owner/shared.git"],
        );
    }

    assert_eq!(identity(&first).key, identity(&second).key);
}

#[test]
fn repositories_with_no_history_and_no_remote_fall_to_the_path_hash() {
    let first = temp_dir("identity-path-first");
    let second = temp_dir("identity-path-second");
    init_repo(&first);
    init_repo(&second);

    let first_identity = identity(&first);
    let second_identity = identity(&second);

    assert_eq!(first_identity.source, ProjectKeySource::RepositoryId);
    assert_eq!(second_identity.source, ProjectKeySource::RepositoryId);
    assert_ne!(first_identity.key, second_identity.key);
    assert_eq!(
        first_identity.key,
        format!("path_{}", repository_id_for_root(&first))
    );
}

#[test]
fn a_non_repository_falls_back_without_failing() {
    let root = temp_dir("identity-not-a-repo");
    let identity = identity(&root);
    assert_eq!(identity.source, ProjectKeySource::RepositoryId);
    assert_eq!(
        ProjectIdentity::source_of_key(&identity.key),
        Some(ProjectKeySource::RepositoryId)
    );
}

#[test]
fn a_project_key_cannot_be_confused_between_tiers() {
    let root = repo_with_a_commit("identity-tier-prefix");
    let identity = identity(&root);
    assert!(identity.key.starts_with("root_"), "{}", identity.key);
    assert_eq!(
        ProjectIdentity::source_of_key(&identity.key),
        Some(ProjectKeySource::RootCommit)
    );
    assert!(!identity.key.contains('/'), "a key is a file name");
}
