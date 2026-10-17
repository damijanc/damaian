//! Worktree-aware read tools, per
//! `docs/specs/60_worktree_aware_read_tools/proposal.md`.
//!
//! Task 1 covers §5.1: which worktrees a `worktree` argument may name. Only an
//! entry git itself lists for this repository is reachable, compared by
//! canonical path, so nothing a clone ships under `.claude/worktrees/` can
//! stand in for one. Task 2 covers §5.4 on the main checkout: git reads drop
//! restricted paths, are capped, and run no repository-selected code. Each
//! test runs against its own fresh `git init` fixture.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::worktree_access::{WorktreeRefusal, classify_worktrees, resolve_worktree};
use workspace_engine::{
    AuditLog, ClientError, Config, GitReadCaps, GitService, PathPolicy, SecretScanner,
};

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
    let git = git_service(&root, GitReadCaps::from_config(&Config::default()));
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

/// A service with an empty secret scanner, so the tests below see exactly
/// what the restricted-path filter and the caps leave, not redaction.
fn git_service(root: &Path, caps: GitReadCaps) -> GitService {
    let scanner = SecretScanner::new(Vec::new());
    GitService::new(
        AuditLog::new(root.join("data"), true, scanner.clone()),
        scanner,
        policy(),
        caps,
    )
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

// ---- Task 2: git read hardening on the main checkout (§5.4) ----

impl Fixture {
    fn write(&self, path: &str, contents: &str) {
        let path = self.repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn commit_all(&self, message: &str) {
        self.run(&["add", "-A"]);
        self.run(&["commit", "-q", "-m", message]);
    }

    /// An executable script outside the repository that touches `marker` and
    /// then runs `tail`, so a test can tell whether git ran it.
    fn marker_script(&self, name: &str, marker: &Path, tail: &str) -> PathBuf {
        let script = self.outside().join(name);
        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n{tail}\n", marker.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn status_paths(&self, git: &GitService) -> (Vec<String>, String) {
        let status = git.status(&self.repo).unwrap();
        (
            status.files.into_iter().map(|file| file.path).collect(),
            status.raw,
        )
    }
}

#[test]
fn a_restricted_tracked_file_is_absent_from_status_and_diff() {
    let fixture = fixture("restricted");
    fixture.write(".env", "TOKEN=one\n");
    fixture.write("config/server.pem", "PEM_ONE\n");
    fixture.write("app.txt", "app one\n");
    fixture.commit_all("tracked secrets");
    fixture.write(".env", "TOKEN=two\n");
    fixture.write("config/server.pem", "PEM_TWO\n");
    fixture.write("app.txt", "app two\n");
    // Untracked and non-ASCII, so porcelain quotes it as `".env.\303\251"`,
    // which no pattern matches until the quoting is undone.
    fixture.write(".env.é", "TOKEN=three\n");

    for staged in [false, true] {
        if staged {
            fixture.run(&["add", "-A"]);
        }
        let (paths, raw) = fixture.status_paths(&fixture.git);
        assert_eq!(paths, vec!["app.txt"], "staged={staged}");
        assert!(!raw.contains(".env"), "staged={staged}: {raw}");
        assert!(!raw.contains("server.pem"), "staged={staged}: {raw}");
        assert!(raw.contains("app.txt"), "staged={staged}: {raw}");

        let diff = fixture.git.diff(&fixture.repo, staged).unwrap();
        assert!(diff.contains("+app two"), "staged={staged}: {diff}");
        for leaked in [".env", "TOKEN=", "server.pem", "PEM_"] {
            assert!(
                !diff.contains(leaked),
                "staged={staged}: {leaked} in {diff}"
            );
        }
    }
}

#[test]
fn a_file_inside_a_restricted_directory_is_absent_from_status_and_diff() {
    let fixture = fixture("restricted-dir");
    // `config/private` (no trailing `/`) matches that path exactly, not what
    // is below it, so only judging each parent directory catches the file;
    // `list_directory` never descends into it, and a git read follows that.
    fixture.write("config/private/notes.txt", "one\n");
    fixture.write("app.txt", "app one\n");
    fixture.commit_all("private notes");
    fixture.write("config/private/notes.txt", "two\n");
    fixture.write("app.txt", "app two\n");
    let scanner = SecretScanner::new(Vec::new());
    let git = GitService::new(
        AuditLog::new(fixture.root.join("data"), true, scanner.clone()),
        scanner,
        PathPolicy::new(&Config {
            restricted_patterns: vec!["config/private".to_string()],
            ..Config::default()
        }),
        GitReadCaps::from_config(&Config::default()),
    );

    let (paths, raw) = fixture.status_paths(&git);
    assert_eq!(paths, vec!["app.txt"], "{raw}");
    let diff = git.diff(&fixture.repo, false).unwrap();
    assert!(!diff.contains("private"), "{diff}");
    assert!(diff.contains("+app two"), "{diff}");
}

#[test]
fn a_status_with_only_restricted_changes_is_clean() {
    let fixture = fixture("restricted-only");
    fixture.write(".env", "TOKEN=one\n");
    fixture.commit_all("tracked secret");
    fixture.write(".env", "TOKEN=two\n");
    fixture.write("id_rsa", "untracked key\n");

    let status = fixture.git.status(&fixture.repo).unwrap();
    assert!(status.clean, "{status:?}");
    assert!(
        status.files.is_empty() && status.raw.is_empty(),
        "{status:?}"
    );
    assert_eq!(fixture.git.diff(&fixture.repo, false).unwrap(), "");
}

#[test]
fn a_rename_onto_a_restricted_path_is_excluded() {
    let fixture = fixture("rename");
    fixture.write("notes.txt", "moved line\n");
    fixture.write(".env.prod", "TOKEN=prod\n");
    fixture.write("kept.txt", "kept one\n");
    fixture.commit_all("notes");
    fixture.run(&["mv", "notes.txt", ".env.local"]);
    // And off one: the old name is as telling as the new one.
    fixture.run(&["mv", ".env.prod", "plain.txt"]);
    fixture.write("kept.txt", "kept two\n");
    fixture.run(&["add", "-A"]);

    let (paths, raw) = fixture.status_paths(&fixture.git);
    assert_eq!(paths, vec!["kept.txt"], "{raw}");
    assert!(!raw.contains(".env"), "{raw}");

    let diff = fixture.git.diff(&fixture.repo, true).unwrap();
    assert!(!diff.contains(".env"), "{diff}");
    assert!(!diff.contains("rename"), "{diff}");
    assert!(diff.contains("+kept two"), "{diff}");
}

#[test]
fn a_large_diff_is_truncated_and_says_what_it_cut() {
    let fixture = fixture("large-diff");
    for name in ["a.txt", "b.txt", "c.txt"] {
        fixture.write(name, &"old\n".repeat(50));
    }
    fixture.commit_all("three files");
    for name in ["a.txt", "b.txt", "c.txt"] {
        fixture.write(name, &"new\n".repeat(50));
    }
    let full = fixture.run(&["diff"]);
    let total_lines = full.lines().count();

    // Cut by lines: 20 lines is inside the first file's hunk.
    let git = git_service(
        &fixture.root,
        GitReadCaps {
            max_lines: 20,
            max_bytes: 1024 * 1024,
        },
    );
    let diff = git.diff(&fixture.repo, false).unwrap();
    let lines: Vec<&str> = diff.lines().collect();
    assert_eq!(lines.len(), 21, "{diff}");
    assert_eq!(
        lines[..20].join("\n"),
        full.lines().take(20).collect::<Vec<_>>().join("\n")
    );
    assert_eq!(
        lines[20],
        format!(
            "[Diff truncated by lines: 20 of {total_lines} lines shown; 2 of 3 files not shown.]"
        )
    );

    // Cut by bytes, with the line cap far away.
    let git = git_service(
        &fixture.root,
        GitReadCaps {
            max_lines: 10_000,
            max_bytes: 200,
        },
    );
    let diff = git.diff(&fixture.repo, false).unwrap();
    let footer = diff.lines().last().unwrap();
    assert!(
        footer.starts_with("[Diff truncated by bytes: ")
            && footer.ends_with(&format!(
                " of {total_lines} lines shown; 2 of 3 files not shown.]"
            )),
        "{diff}"
    );
    assert!(diff.len() - footer.len() - 1 <= 200, "{diff}");
}

#[test]
fn a_large_status_is_truncated_and_says_what_it_cut() {
    let fixture = fixture("large-status");
    for index in 0..5 {
        fixture.write(&format!("new-{index}.txt"), "x\n");
    }
    let git = git_service(
        &fixture.root,
        GitReadCaps {
            max_lines: 3,
            max_bytes: 1024 * 1024,
        },
    );
    let status = git.status(&fixture.repo).unwrap();
    assert!(!status.clean);
    assert_eq!(status.files.len(), 3, "{status:?}");
    assert_eq!(status.raw.lines().count(), 3, "{status:?}");
    assert_eq!(status.omitted, 2, "{status:?}");
}

#[test]
fn a_gitattributes_diff_driver_is_not_invoked() {
    let fixture = fixture("ext-diff");
    let marker = fixture.outside().join("ext-diff-ran");
    let driver = fixture.marker_script("driver.sh", &marker, "exit 0");
    fixture.run(&["config", "diff.evil.command", driver.to_str().unwrap()]);
    fixture.write(".gitattributes", "*.txt diff=evil\n");
    fixture.write("a.txt", "one\n");
    fixture.commit_all("attributes");
    fixture.write("a.txt", "two\n");

    let diff = fixture.git.diff(&fixture.repo, false).unwrap();
    assert!(diff.contains("+two"), "{diff}");
    assert!(!marker.exists(), "the external diff driver ran");

    // The fixture is live: plain git does run the driver.
    fixture.run(&["diff"]);
    assert!(
        marker.exists(),
        "the fixture's driver should run under plain git"
    );
}

#[test]
fn a_gitattributes_textconv_driver_is_not_invoked() {
    let fixture = fixture("textconv");
    let marker = fixture.outside().join("textconv-ran");
    let driver = fixture.marker_script("conv.sh", &marker, "cat \"$1\"");
    fixture.run(&["config", "diff.conv.textconv", driver.to_str().unwrap()]);
    fixture.write(".gitattributes", "*.txt diff=conv\n");
    fixture.write("a.txt", "one\n");
    fixture.commit_all("attributes");
    fixture.write("a.txt", "two\n");

    for staged in [false, true] {
        if staged {
            fixture.run(&["add", "-A"]);
        }
        let diff = fixture.git.diff(&fixture.repo, staged).unwrap();
        assert!(diff.contains("+two"), "staged={staged}: {diff}");
        assert!(!marker.exists(), "staged={staged}: the textconv driver ran");
    }

    fixture.run(&["diff", "--staged"]);
    assert!(
        marker.exists(),
        "the fixture's textconv should run under plain git"
    );
}

#[test]
fn fsmonitor_is_not_run() {
    let fixture = fixture("fsmonitor");
    let marker = fixture.outside().join("fsmonitor-ran");
    let hook = fixture.marker_script("fsmonitor.sh", &marker, "exit 1");
    fixture.run(&["config", "core.fsmonitor", hook.to_str().unwrap()]);
    fixture.write("README.md", "changed\n");

    fixture.git.status(&fixture.repo).unwrap();
    fixture.git.diff(&fixture.repo, false).unwrap();
    fixture.git.summary(&fixture.repo).unwrap();
    assert!(!marker.exists(), "the fsmonitor hook ran");

    fixture.run(&["status", "--porcelain"]);
    assert!(
        marker.exists(),
        "the fixture's fsmonitor hook should run under plain git"
    );
}

#[test]
fn status_does_not_take_the_index_lock() {
    let fixture = fixture("index-lock");
    let index = fixture.repo.join(".git/index");
    // Same content, new mtime: plain `git status` would refresh the stat data
    // and rewrite the index under `index.lock`.
    let readme = fixture.repo.join("README.md");
    let status = Command::new("touch")
        .args(["-t", "202001010000"])
        .arg(&readme)
        .status()
        .unwrap();
    assert!(status.success());
    let before = fs::read(&index).unwrap();

    let status = fixture.git.status(&fixture.repo).unwrap();
    assert!(status.clean, "{status:?}");
    fixture.git.diff(&fixture.repo, false).unwrap();
    assert_eq!(
        fs::read(&index).unwrap(),
        before,
        "a read rewrote the index"
    );

    // Another process holding the lock does not make the read fail.
    fs::write(fixture.repo.join(".git/index.lock"), "").unwrap();
    let status = fixture.git.status(&fixture.repo).unwrap();
    assert_eq!(status.exit_code, 0, "{status:?}");
    fs::remove_file(fixture.repo.join(".git/index.lock")).unwrap();

    fixture.run(&["status", "--porcelain"]);
    assert_ne!(
        fs::read(&index).unwrap(),
        before,
        "the fixture should make plain git rewrite the index"
    );
}

#[test]
fn a_diff_with_no_changes_is_unchanged() {
    let fixture = fixture("unchanged");
    fixture.write("notes.txt", "one\ntwo\nthree\nfour\n");
    fixture.write("space name.txt", "alpha\n");
    fixture.write("b.txt", "beta\n");
    fixture.commit_all("ordinary");
    fixture.run(&["mv", "notes.txt", "renamed.txt"]);
    fixture.write("b.txt", "beta two\n");
    fixture.run(&["add", "-A"]);
    fixture.write("space name.txt", "alpha two\n");
    fixture.write("untracked.txt", "new\n");

    assert_eq!(
        fixture.git.diff(&fixture.repo, false).unwrap(),
        fixture.run(&["diff"])
    );
    assert_eq!(
        fixture.git.diff(&fixture.repo, true).unwrap(),
        fixture.run(&["diff", "--staged"])
    );
    let status = fixture.git.status(&fixture.repo).unwrap();
    assert_eq!(status.raw, fixture.run(&["status", "--porcelain=v1"]));
    assert_eq!(status.omitted, 0);
}
