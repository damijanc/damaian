//! Worktree-aware read tools, per
//! `docs/specs/60_worktree_aware_read_tools/proposal.md`.
//!
//! Task 1 covers §5.1: which worktrees a `worktree` argument may name. Only an
//! entry git itself lists for this repository is reachable, compared by
//! canonical path, so nothing a clone ships under `.claude/worktrees/` can
//! stand in for one. Each test runs against its own fresh `git init` fixture.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::worktree_access::{WorktreeRefusal, classify_worktrees, resolve_worktree};
use workspace_engine::{AuditLog, ClientError, Config, GitService, PathPolicy, SecretScanner};

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    /// Holds the repository, a directory outside it, and the audit data.
    root: PathBuf,
    repo: PathBuf,
    git: GitService,
}

/// A repository with one commit and two linked worktrees,
/// `.claude/worktrees/a` on `wt-a` and `.claude/worktrees/b` on `wt-b`.
fn fixture(name: &str) -> Fixture {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "damaian-worktree-reads-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(root.join("outside")).unwrap();
    let scanner = SecretScanner::new(Vec::new());
    let git = GitService::new(
        AuditLog::new(root.join("data"), true, scanner.clone()),
        scanner,
    );
    let fixture = Fixture { root, repo, git };
    fixture.run(&["init", "-q", "--initial-branch=main"]);
    fixture.run(&["config", "user.name", "Damaian Test"]);
    fixture.run(&["config", "user.email", "test@example.invalid"]);
    fixture.run(&["config", "commit.gpgsign", "false"]);
    fs::write(fixture.repo.join("README.md"), "readme\n").unwrap();
    fs::write(fixture.repo.join(".gitignore"), ".claude/\n").unwrap();
    fixture.run(&["add", "."]);
    fixture.run(&["commit", "-q", "-m", "initial"]);
    fixture.run(&["worktree", "add", "-q", ".claude/worktrees/a", "-b", "wt-a"]);
    fixture.run(&["worktree", "add", "-q", ".claude/worktrees/b", "-b", "wt-b"]);
    fixture
}

impl Fixture {
    fn run(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn outside(&self) -> PathBuf {
        self.root.join("outside")
    }

    fn worktree(&self, name: &str) -> PathBuf {
        fs::canonicalize(self.repo.join(".claude/worktrees").join(name)).unwrap()
    }

    /// The display paths of the reachable set, as `list_worktrees` will show
    /// them, from `session_root`.
    fn reachable(&self, policy: &PathPolicy, session_root: &Path) -> Vec<String> {
        let entries = self.git.worktrees(session_root).unwrap();
        classify_worktrees(policy, session_root, entries)
            .unwrap()
            .into_iter()
            .filter(|worktree| worktree.refusal.is_none())
            .map(|worktree| worktree.display)
            .collect()
    }

    fn resolve(&self, policy: &PathPolicy, requested: &str) -> workspace_engine::Result<PathBuf> {
        resolve_worktree(&self.git, policy, &self.repo, requested).map(|worktree| {
            worktree
                .canonical
                .expect("a resolved worktree has a canonical path")
        })
    }
}

fn policy() -> PathPolicy {
    PathPolicy::new(&Config::default())
}

fn policy_allowing(root: &Path) -> PathPolicy {
    PathPolicy::new(&Config {
        allowed_roots: vec![root.to_path_buf()],
        ..Config::default()
    })
}

fn refusal_message(result: workspace_engine::Result<PathBuf>) -> String {
    match result {
        Err(ClientError::AccessDenied(message)) => message,
        other => panic!("expected an access-denied refusal, got {other:?}"),
    }
}

#[test]
fn registered_worktrees_inside_the_root_are_reachable() {
    let fixture = fixture("reachable");
    assert_eq!(
        fixture.reachable(&policy(), &fixture.repo),
        vec![".claude/worktrees/a", ".claude/worktrees/b"]
    );
}

#[test]
fn a_worktree_argument_resolves_relative_or_absolute_to_the_same_root() {
    let fixture = fixture("relative-absolute");
    let expected = fixture.worktree("a");
    assert_eq!(
        fixture.resolve(&policy(), ".claude/worktrees/a").unwrap(),
        expected
    );
    assert_eq!(
        fixture.resolve(&policy(), ".claude/worktrees/a/").unwrap(),
        expected
    );
    let absolute = fixture.repo.join(".claude/worktrees/a");
    assert_eq!(
        fixture
            .resolve(&policy(), absolute.to_str().unwrap())
            .unwrap(),
        expected
    );
}

#[test]
fn a_symlink_under_claude_worktrees_pointing_outside_is_refused() {
    let fixture = fixture("symlink-outside");
    std::os::unix::fs::symlink(
        fixture.outside(),
        fixture.repo.join(".claude/worktrees/evil"),
    )
    .unwrap();
    let message = refusal_message(fixture.resolve(&policy(), ".claude/worktrees/evil"));
    assert!(
        message.contains("not a registered worktree"),
        "the refusal should say why: {message}"
    );
}

#[test]
fn a_symlink_to_a_registered_worktree_resolves_to_that_worktree() {
    let fixture = fixture("symlink-registered");
    std::os::unix::fs::symlink(
        fixture.worktree("a"),
        fixture.repo.join(".claude/worktrees/alias"),
    )
    .unwrap();
    assert_eq!(
        fixture
            .resolve(&policy(), ".claude/worktrees/alias")
            .unwrap(),
        fixture.worktree("a")
    );
}

#[test]
fn a_plain_directory_that_is_not_registered_is_refused() {
    let fixture = fixture("plain-dir");
    let planted = fixture.repo.join(".claude/worktrees/planted");
    fs::create_dir_all(&planted).unwrap();
    // A `.git` file that looks like a worktree's is still not registration.
    fs::write(
        planted.join(".git"),
        format!("gitdir: {}\n", fixture.repo.join(".git").display()),
    )
    .unwrap();
    let message = refusal_message(fixture.resolve(&policy(), ".claude/worktrees/planted"));
    assert!(message.contains("not a registered worktree"), "{message}");
}

#[test]
fn a_refusal_lists_the_reachable_worktrees() {
    let fixture = fixture("lists");
    let message = refusal_message(fixture.resolve(&policy(), "README.md"));
    assert!(message.contains(".claude/worktrees/a"), "{message}");
    assert!(message.contains(".claude/worktrees/b"), "{message}");
}

#[test]
fn a_prunable_worktree_is_refused_with_the_reason() {
    let fixture = fixture("prunable");
    // Git marks a worktree prunable when its `.git` file is gone, even though
    // the directory is still there.
    fs::remove_file(fixture.repo.join(".claude/worktrees/b/.git")).unwrap();
    assert_eq!(
        fixture.reachable(&policy(), &fixture.repo),
        vec![".claude/worktrees/a"]
    );
    let message = refusal_message(fixture.resolve(&policy(), ".claude/worktrees/b"));
    assert!(message.contains("prunable"), "{message}");
    assert!(message.contains(".claude/worktrees/a"), "{message}");
}

#[test]
fn the_sessions_own_checkout_is_not_a_worktree_argument() {
    let fixture = fixture("own-checkout");
    let message = refusal_message(fixture.resolve(&policy(), "."));
    assert!(message.contains("own checkout"), "{message}");
}

#[test]
fn an_outside_worktree_is_refused_with_empty_allowed_roots() {
    let fixture = fixture("outside-refused");
    let outside = fixture.outside().join("wt");
    fixture.run(&[
        "worktree",
        "add",
        "-q",
        outside.to_str().unwrap(),
        "-b",
        "wt-out",
    ]);
    assert_eq!(
        fixture.reachable(&policy(), &fixture.repo),
        vec![".claude/worktrees/a", ".claude/worktrees/b"]
    );
    let message = refusal_message(fixture.resolve(&policy(), outside.to_str().unwrap()));
    assert!(message.contains("allowed_roots"), "{message}");
}

#[test]
fn an_outside_worktree_is_reachable_inside_an_allowed_root() {
    let fixture = fixture("outside-allowed");
    let outside = fixture.outside().join("wt");
    fixture.run(&[
        "worktree",
        "add",
        "-q",
        outside.to_str().unwrap(),
        "-b",
        "wt-out",
    ]);
    let policy = policy_allowing(&fixture.root);
    let canonical = fs::canonicalize(&outside).unwrap();
    // Outside the repository root, it is shown by its absolute path.
    assert!(
        fixture
            .reachable(&policy, &fixture.repo)
            .contains(&canonical.to_string_lossy().to_string())
    );
    assert_eq!(
        fixture.resolve(&policy, outside.to_str().unwrap()).unwrap(),
        canonical
    );
}

#[test]
fn an_allowed_roots_entry_that_holds_only_the_repository_keeps_outside_worktrees_out() {
    let fixture = fixture("outside-not-covered");
    let outside = fixture.outside().join("wt");
    fixture.run(&[
        "worktree",
        "add",
        "-q",
        outside.to_str().unwrap(),
        "-b",
        "wt-out",
    ]);
    let policy = policy_allowing(&fixture.repo);
    let message = refusal_message(fixture.resolve(&policy, outside.to_str().unwrap()));
    assert!(message.contains("allowed_roots"), "{message}");
    assert_eq!(
        fixture.reachable(&policy, &fixture.repo),
        vec![".claude/worktrees/a", ".claude/worktrees/b"]
    );
}

#[test]
fn from_a_linked_worktree_the_main_checkout_is_listed_but_outside() {
    let fixture = fixture("from-linked");
    let session_root = fixture.worktree("a");
    let entries = fixture.git.worktrees(&session_root).unwrap();
    let classified = classify_worktrees(&policy(), &session_root, entries).unwrap();
    let refusal_of = |path: &Path| {
        classified
            .iter()
            .find(|worktree| worktree.canonical.as_deref() == Some(path))
            .map(|worktree| worktree.refusal)
            .expect("every worktree is listed")
    };
    let main = fs::canonicalize(&fixture.repo).unwrap();
    assert_eq!(
        refusal_of(&main),
        Some(WorktreeRefusal::OutsideAllowedRoots)
    );
    assert_eq!(
        refusal_of(&session_root),
        Some(WorktreeRefusal::OwnCheckout)
    );
    assert_eq!(
        refusal_of(&fixture.worktree("b")),
        Some(WorktreeRefusal::OutsideAllowedRoots)
    );

    // With the repository allowed, the main checkout and the sibling become
    // reachable from the worker's root.
    let policy = policy_allowing(&fixture.repo);
    let reachable = fixture.reachable(&policy, &session_root);
    assert_eq!(
        reachable,
        vec![
            main.to_string_lossy().to_string(),
            fixture.worktree("b").to_string_lossy().to_string(),
        ]
    );
}

#[test]
fn a_nonexistent_argument_is_refused_with_the_list() {
    let fixture = fixture("nonexistent");
    let message = refusal_message(fixture.resolve(&policy(), ".claude/worktrees/missing"));
    assert!(message.contains("does not exist"), "{message}");
    assert!(message.contains(".claude/worktrees/a"), "{message}");
}

#[test]
fn a_folder_that_is_not_a_repository_resolves_no_worktree() {
    let fixture = fixture("not-a-repo");
    let folder = fixture.outside();
    let result = resolve_worktree(&fixture.git, &policy(), &folder, ".");
    assert!(result.is_err(), "{result:?}");
}
