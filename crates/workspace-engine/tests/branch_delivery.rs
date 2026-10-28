//! Branch and worktree delivery, per
//! `docs/specs/36_branch_and_worktree_delivery/proposal.md`.
//!
//! Task 1 covers §5.1: the read-only `BranchInfo` and a public merge-base.
//! Every test runs against its own fresh `git init` repository; no test touches
//! the user's checkout, a network remote, or a real provider.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    AuditLog, BranchInfo, ClientError, Config, GitReadCaps, GitService, PathPolicy, SecretScanner,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    /// Holds the repository and the audit data, beside it.
    root: PathBuf,
    repo: PathBuf,
    git: GitService,
}

fn temp_root(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "damaian-branch-delivery-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A repository on `main` with one commit, unless `initial_commit` is false.
fn fixture(name: &str, initial_commit: bool) -> Fixture {
    let root = temp_root(name);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    let scanner = SecretScanner::new(Vec::new());
    let git = GitService::new(
        AuditLog::new(root.join("data"), true, scanner.clone()),
        scanner,
        PathPolicy::new(&Config::default()),
        GitReadCaps::from_config(&Config::default()),
    );
    let fixture = Fixture { root, repo, git };
    fixture.run(&["init", "-q", "--initial-branch=main"]);
    fixture.run(&["config", "user.name", "Damaian Test"]);
    fixture.run(&["config", "user.email", "test@example.invalid"]);
    fixture.run(&["config", "commit.gpgsign", "false"]);
    if initial_commit {
        fixture.write("README.md", "readme\n");
        fixture.run(&["add", "."]);
        fixture.run(&["commit", "-q", "-m", "initial"]);
    }
    fixture
}

impl Fixture {
    fn write(&self, path: &str, content: &str) {
        let path = self.repo.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn try_run(&self, args: &[&str]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .expect("git should run")
    }

    fn run(&self, args: &[&str]) -> String {
        let output = self.try_run(args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn run_failing(&self, args: &[&str]) -> String {
        let output = self.try_run(args);
        assert!(
            !output.status.success(),
            "git {args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        String::from_utf8_lossy(&output.stderr).to_string()
    }

    fn head(&self) -> String {
        self.run(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn info(&self) -> BranchInfo {
        self.git.branch_info(&self.repo).expect("branch info")
    }

    fn commit(&self, path: &str, content: &str, message: &str) {
        self.write(path, content);
        self.run(&["add", "."]);
        self.run(&["commit", "-q", "-m", message]);
    }
}

#[test]
fn branch_info_reports_the_upstream_and_both_counts() {
    let fixture = fixture("upstream", true);
    let origin = fixture.root.join("origin.git");
    let output = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&origin)
        .output()
        .expect("git should run");
    assert!(output.status.success(), "{output:?}");
    fixture.run(&["remote", "add", "origin", origin.to_str().unwrap()]);
    fixture.run(&["push", "-q", "-u", "origin", "main"]);

    let info = fixture.info();
    assert_eq!(info.current.as_deref(), Some("main"));
    assert_eq!(info.upstream.as_deref(), Some("origin/main"));
    assert_eq!(info.head_oid.as_deref(), Some(fixture.head().as_str()));
    assert!(!info.detached);
    assert_eq!((info.ahead, info.behind), (0, 0));

    let first = fixture.head();
    fixture.commit("ahead.txt", "ahead\n", "local ahead");
    let info = fixture.info();
    assert_eq!((info.ahead, info.behind), (1, 0), "{info:?}");

    // Move the remote to the local commit, then put local main back: the same
    // commit is now only on the remote, which is one behind.
    fixture.run(&["push", "-q", "origin", "main"]);
    fixture.run(&["reset", "--hard", &first]);
    let info = fixture.info();
    assert_eq!((info.ahead, info.behind), (0, 1), "{info:?}");

    fixture.commit("diverged.txt", "diverged\n", "local diverged");
    let info = fixture.info();
    assert_eq!((info.ahead, info.behind), (1, 1), "{info:?}");
}

#[test]
fn a_detached_head_is_reported_as_detached() {
    let fixture = fixture("detached", true);
    fixture.run(&["checkout", "-q", "--detach"]);
    let info = fixture.info();
    assert_eq!(info.current, None);
    assert_eq!(info.head_oid.as_deref(), Some(fixture.head().as_str()));
    assert!(info.detached);
    assert_eq!(info.upstream, None);
    assert_eq!((info.ahead, info.behind), (0, 0));
}

#[test]
fn an_unborn_branch_has_a_name_and_no_head() {
    let fixture = fixture("unborn", false);
    let info = fixture.info();
    assert!(info.current.is_some(), "{info:?}");
    assert_eq!(info.head_oid, None);
    assert!(!info.detached);
    assert_eq!(info.upstream, None);
    assert_eq!((info.ahead, info.behind), (0, 0));
    assert_eq!(info.in_progress_operation, None);
    assert!(info.conflicted_paths.is_empty());
}

#[test]
fn a_conflicted_merge_is_reported_with_its_operation_and_paths() {
    let fixture = fixture("merge", true);
    fixture.commit("conflict.txt", "a\nb\nc\n", "base");
    fixture.run(&["checkout", "-q", "-b", "other"]);
    fixture.commit("conflict.txt", "a\nX\nc\n", "other");
    fixture.run(&["checkout", "-q", "main"]);
    fixture.commit("conflict.txt", "a\nY\nc\n", "main");
    fixture.run_failing(&["merge", "other"]);

    let info = fixture.info();
    assert_eq!(info.current.as_deref(), Some("main"));
    assert_eq!(info.in_progress_operation.as_deref(), Some("merge"));
    assert_eq!(info.conflicted_paths, vec!["conflict.txt".to_string()]);
}

#[test]
fn a_clean_repository_reports_no_operation_or_conflicts() {
    let fixture = fixture("clean", true);
    let info = fixture.info();
    assert_eq!(info.in_progress_operation, None);
    assert!(info.conflicted_paths.is_empty());
    assert_eq!((info.ahead, info.behind), (0, 0));
}

#[test]
fn merge_base_is_the_fork_point_and_the_base_is_validated() {
    let fixture = fixture("merge-base", true);
    fixture.commit("main.txt", "main\n", "main change");
    let fork = fixture.head();
    fixture.run(&["checkout", "-q", "-b", "other"]);
    fixture.commit("other.txt", "other\n", "other change");
    fixture.run(&["checkout", "-q", "main"]);

    assert_eq!(
        fixture.git.merge_base(&fixture.repo, "other").unwrap(),
        fork
    );
    let refusal = fixture
        .git
        .merge_base(&fixture.repo, "--output=/tmp/x")
        .unwrap_err();
    assert!(
        matches!(refusal, ClientError::InvalidInput(ref message) if message.contains("not a branch name")),
        "{refusal:?}"
    );
}

#[test]
fn branch_reads_leave_the_index_head_and_status_byte_identical() {
    let fixture = fixture("read-only", true);
    fixture.commit("changed.txt", "one\n", "change");
    fixture.write("changed.txt", "two\n");
    fixture.write("untracked.txt", "untracked\n");
    fixture.run(&["branch", "other"]);

    let index = fs::read(fixture.repo.join(".git/index")).unwrap();
    let head = fixture.head();
    let status = fixture.run(&["--no-optional-locks", "status", "--porcelain=v2"]);

    let info = fixture.info();
    assert_eq!(info.head_oid.as_deref(), Some(head.as_str()));
    let _ = fixture.git.merge_base(&fixture.repo, "other").unwrap();

    assert_eq!(fs::read(fixture.repo.join(".git/index")).unwrap(), index);
    assert_eq!(fixture.head(), head);
    assert_eq!(
        fixture.run(&["--no-optional-locks", "status", "--porcelain=v2"]),
        status
    );
}

#[test]
fn a_restricted_path_in_conflict_is_omitted_from_the_list() {
    let fixture = fixture("restricted", true);
    fixture.write(".env", "a\nb\n");
    fixture.run(&["add", "-f", ".env"]);
    fixture.run(&["commit", "-q", "-m", "base env"]);
    fixture.run(&["checkout", "-q", "-b", "other"]);
    fixture.write(".env", "a\nX\n");
    fixture.run(&["commit", "-q", "-am", "other env"]);
    fixture.run(&["checkout", "-q", "main"]);
    fixture.write(".env", "a\nY\n");
    fixture.run(&["commit", "-q", "-am", "main env"]);
    fixture.run_failing(&["merge", "other"]);

    let info = fixture.info();
    assert_eq!(info.in_progress_operation.as_deref(), Some("merge"));
    assert!(
        info.conflicted_paths.is_empty(),
        "a restricted path must not be listed: {info:?}"
    );
}

#[test]
fn a_folder_that_is_not_a_repository_is_refused() {
    let root = temp_root("not-a-repo");
    fs::create_dir_all(&root).unwrap();
    let scanner = SecretScanner::new(Vec::new());
    let git = GitService::new(
        AuditLog::new(root.join("data"), true, scanner.clone()),
        scanner,
        PathPolicy::new(&Config::default()),
        GitReadCaps::from_config(&Config::default()),
    );
    let error = git.branch_info(&root).unwrap_err();
    assert!(
        matches!(error, ClientError::InvalidInput(ref message) if message.contains("not a Git work tree")),
        "{error:?}"
    );
}

#[test]
fn branch_info_is_serialisable_for_the_shell() {
    let fixture = fixture("serialise", true);
    let json = serde_json::to_value(fixture.info()).unwrap();
    assert!(json.get("headOid").is_some(), "{json}");
    assert!(json.get("inProgressOperation").is_some(), "{json}");
    assert!(json.get("conflictedPaths").is_some(), "{json}");
}
