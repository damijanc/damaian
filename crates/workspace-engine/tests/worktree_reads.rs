//! Worktree-aware read tools, per
//! `docs/specs/60_worktree_aware_read_tools/proposal.md`.
//!
//! Task 1 covers §5.1: which worktrees a `worktree` argument may name. Only an
//! entry git itself lists for this repository is reachable, compared by
//! canonical path, so nothing a clone ships under `.claude/worktrees/` can
//! stand in for one. Task 2 covers §5.4 on the main checkout: git reads drop
//! restricted paths, are capped, and run no repository-selected code. Task 3
//! covers §5.2–§5.5: the five read tools take `worktree` and read there under
//! the session's policy. Each test runs against its own fresh `git init`
//! fixture.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::worktree_access::{WorktreeRefusal, classify_worktrees, resolve_worktree};
use workspace_engine::{
    AuditLog, ClientError, Config, DiffOptions, GitReadCaps, GitService, MockModelAdapter,
    ModelProviderConfig, PathPolicy, SecretScanner, ToolCall, WorkspaceEngine, WorktreeListing,
    WorktreeListingEntry,
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

// ---- Task 3: the `worktree` argument on the five read tools (§5.2, §5.3, §5.5) ----

/// The provider block that turns native tools on; without it the engine falls
/// back to the text envelope and no tool call is dispatched.
fn native_tool_provider() -> ModelProviderConfig {
    ModelProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: Vec::new(),
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    }
}

/// What one turn of tool calls left behind.
struct Turn {
    /// The tool results, in call order.
    results: Vec<String>,
    /// Session events and audit records, everything the engine wrote.
    records: String,
    approval_requested: bool,
}

impl Fixture {
    fn run_in(&self, dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_in(&self, worktree: &str, path: &str, contents: &str) {
        let path = self
            .repo
            .join(".claude/worktrees")
            .join(worktree)
            .join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// One turn whose first model message makes `calls` (name, arguments),
    /// answered by the engine with the session's own policy.
    fn tools(&self, calls: &[(&str, &str)]) -> Turn {
        let data = self.root.join("engine-data");
        let mut config = Config {
            data_dir: data.clone(),
            enable_index_watcher: false,
            ..Config::default()
        };
        config.model_providers.push(native_tool_provider());
        let engine = WorkspaceEngine::new(config);
        let calls = calls
            .iter()
            .enumerate()
            .map(|(index, (name, arguments))| ToolCall {
                id: format!("call_{index}"),
                name: name.to_string(),
                arguments_json: arguments.to_string(),
            })
            .collect();
        let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Done.".to_string()],
            vec![calls, Vec::new()],
        );
        let mut on_token = |_token: &str| {};
        let result = engine
            .chat_orchestrator
            .ask(
                &self.repo,
                "Look at the worktrees.",
                &[],
                &mut adapter,
                &mut on_token,
            )
            .unwrap();
        let results = engine
            .session_store
            .read_messages(&result.session.id)
            .unwrap()
            .into_iter()
            .filter(|message| message.role == "tool")
            .map(|message| message.content)
            .collect();
        Turn {
            results,
            records: text_under(&data),
            approval_requested: result.command_proposal.is_some(),
        }
    }
}

/// Every file's text under `dir`, concatenated: the session event logs and
/// the audit log among them.
fn text_under(dir: &Path) -> String {
    let mut text = String::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return text;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            text.push_str(&text_under(&path));
        } else if let Ok(contents) = fs::read_to_string(&path) {
            text.push_str(&contents);
        }
    }
    text
}

const WT_A: &str = "{\"worktree\":\".claude/worktrees/a\"";

fn args(rest: &str) -> String {
    if rest.is_empty() {
        format!("{WT_A}}}")
    } else {
        format!("{WT_A},{rest}}}")
    }
}

#[test]
fn read_file_with_worktree_reads_that_worktree() {
    let fixture = fixture("tool-read-file");
    fixture.write_in("a", "only-in-a.txt", "alpha marker\n");
    let turn = fixture.tools(&[
        ("read_file", &args("\"path\":\"only-in-a.txt\"")),
        ("read_file", "{\"path\":\"only-in-a.txt\"}"),
    ]);
    assert!(
        turn.results[0].contains("alpha marker"),
        "{:?}",
        turn.results
    );
    assert!(
        turn.results[0].contains("in worktree `.claude/worktrees/a`"),
        "{:?}",
        turn.results
    );
    // Without the argument the path resolves against the session's checkout.
    assert!(
        turn.results[1].starts_with("Cannot read"),
        "{:?}",
        turn.results
    );
    assert!(!turn.approval_requested);
    assert!(
        turn.records
            .contains("\"ref\":\".claude/worktrees/a:only-in-a.txt\""),
        "the action marker names the worktree"
    );
}

#[test]
fn list_directory_with_worktree_walks_that_worktree() {
    let fixture = fixture("tool-list");
    fixture.write_in("a", "src/only-in-a.rs", "fn a() {}\n");
    let turn = fixture.tools(&[("list_directory", &args("")), ("list_directory", "{}")]);
    assert!(
        turn.results[0].contains("src/only-in-a.rs"),
        "{:?}",
        turn.results
    );
    assert!(!turn.results[1].contains("only-in-a"), "{:?}", turn.results);
    assert!(turn.records.contains("\"ref\":\".claude/worktrees/a:\""));
}

#[test]
fn search_content_with_worktree_searches_that_worktree() {
    let fixture = fixture("tool-search");
    fixture.write_in("a", "src/lib.rs", "let needle_in_a = 1;\n");
    let turn = fixture.tools(&[
        ("search_content", &args("\"pattern\":\"needle_in_a\"")),
        ("search_content", "{\"pattern\":\"needle_in_a\"}"),
    ]);
    assert!(
        turn.results[0].contains("src/lib.rs:1: let needle_in_a = 1;"),
        "{:?}",
        turn.results
    );
    assert!(
        turn.results[1].starts_with("0 matches"),
        "{:?}",
        turn.results
    );
}

#[test]
fn read_git_status_with_worktree_reads_that_worktree() {
    let fixture = fixture("tool-status");
    fixture.write_in("a", "README.md", "changed in a\n");
    let turn = fixture.tools(&[("read_git_status", &args("")), ("read_git_status", "{}")]);
    assert!(
        turn.results[0].contains(" M README.md"),
        "{:?}",
        turn.results
    );
    assert_eq!(turn.results[1], "Working tree clean.");
}

#[test]
fn read_git_diff_with_worktree_reads_that_worktree() {
    let fixture = fixture("tool-diff");
    fixture.write_in("a", "README.md", "changed in a\n");
    let turn = fixture.tools(&[("read_git_diff", &args("")), ("read_git_diff", "{}")]);
    assert!(
        turn.results[0].contains("+changed in a"),
        "{:?}",
        turn.results
    );
    assert_eq!(turn.results[1], "No differences.");
}

#[test]
fn a_refused_worktree_fails_every_read_tool_and_lists_the_reachable_ones() {
    let fixture = fixture("tool-refused");
    fs::create_dir_all(fixture.repo.join(".claude/worktrees/planted")).unwrap();
    let planted = "{\"worktree\":\".claude/worktrees/planted\"";
    let calls = [
        ("read_file", format!("{planted},\"path\":\"README.md\"}}")),
        ("list_directory", format!("{planted}}}")),
        ("search_content", format!("{planted},\"pattern\":\"x\"}}")),
        ("read_git_status", format!("{planted}}}")),
        ("read_git_diff", format!("{planted}}}")),
    ];
    let calls: Vec<(&str, &str)> = calls.iter().map(|(n, a)| (*n, a.as_str())).collect();
    let turn = fixture.tools(&calls);
    assert_eq!(turn.results.len(), 5);
    for result in &turn.results {
        assert!(
            result.contains("not a registered worktree")
                && result.contains("reachable worktrees: .claude/worktrees/a, .claude/worktrees/b"),
            "{result}"
        );
    }
    assert!(
        !turn.approval_requested,
        "a refusal is not an approval stop"
    );
    assert_eq!(
        turn.records.matches("\"outcome\":\"failed\"").count(),
        5,
        "each refusal is an ordinary failed call"
    );
}

#[test]
fn a_restricted_file_in_a_worktree_is_refused_or_absent() {
    let fixture = fixture("tool-restricted");
    let a = fixture.worktree("a");
    fixture.write_in("a", ".env", "TOKEN=one\n");
    fixture.run_in(&a, &["add", "-f", ".env"]);
    fixture.run_in(&a, &["commit", "-q", "-m", "secret"]);
    fixture.write_in("a", ".env", "TOKEN=two\n");
    fixture.write_in("a", "app.txt", "TOKEN_FREE\n");
    let turn = fixture.tools(&[
        ("read_file", &args("\"path\":\".env\"")),
        ("list_directory", &args("")),
        ("search_content", &args("\"pattern\":\"TOKEN\"")),
        ("read_git_status", &args("")),
        ("read_git_diff", &args("")),
    ]);
    assert!(
        turn.results[0].contains("restricted by policy"),
        "{:?}",
        turn.results
    );
    for result in &turn.results[1..] {
        assert!(!result.contains(".env"), "{result}");
        assert!(!result.contains("TOKEN="), "{result}");
    }
    assert!(turn.results[1].contains("app.txt"), "{:?}", turn.results);
}

#[test]
fn the_audit_records_name_the_worktree() {
    let fixture = fixture("tool-audit");
    fixture.write_in("a", "README.md", "changed in a\n");
    let turn = fixture.tools(&[
        ("read_file", &args("\"path\":\"README.md\"")),
        ("list_directory", &args("")),
        ("search_content", &args("\"pattern\":\"changed\"")),
        ("read_git_status", &args("")),
        ("read_git_diff", &args("")),
    ]);
    for event in [
        "file_read",
        "directory_listed",
        "content_searched",
        "git_status_read",
        "git_diff_read",
    ] {
        assert!(
            turn.records.lines().any(|line| line.contains(event)
                && line.contains("\"worktree\":\".claude/worktrees/a\"")),
            "{event} must name the worktree"
        );
    }
}

/// Worktree `b` with one commit ahead of `main` and an uncommitted change.
fn ahead_fixture(name: &str) -> (Fixture, PathBuf) {
    let fixture = fixture(name);
    let b = fixture.worktree("b");
    fixture.write_in("b", "committed.txt", "on wt-b\n");
    fixture.write_in("b", "other.txt", "other\n");
    fixture.run_in(&b, &["add", "-A"]);
    fixture.run_in(&b, &["commit", "-q", "-m", "ahead"]);
    fixture.write_in("b", "README.md", "uncommitted in b\n");
    (fixture, b)
}

#[test]
fn a_diff_against_a_base_includes_the_commits_on_the_branch() {
    let (fixture, b) = ahead_fixture("diff-base");
    let options = DiffOptions {
        base: Some("main".to_string()),
        ..DiffOptions::default()
    };
    let diff = fixture
        .git
        .diff_in(&b, Some(".claude/worktrees/b"), &options)
        .unwrap();
    assert!(diff.contains("+on wt-b"), "{diff}");
    assert!(diff.contains("+uncommitted in b"), "{diff}");
    let plain = fixture.git.diff(&b, false).unwrap();
    assert!(!plain.contains("on wt-b"), "{plain}");
}

#[test]
fn a_diff_limited_to_paths_shows_only_those() {
    let (fixture, b) = ahead_fixture("diff-paths");
    let options = DiffOptions {
        base: Some("main".to_string()),
        paths: vec!["committed.txt".to_string()],
        ..DiffOptions::default()
    };
    let diff = fixture.git.diff_in(&b, None, &options).unwrap();
    assert!(diff.contains("committed.txt"), "{diff}");
    assert!(
        !diff.contains("README.md") && !diff.contains("other.txt"),
        "{diff}"
    );
}

#[test]
fn a_stat_diff_summarises_instead_of_showing_hunks() {
    let (fixture, b) = ahead_fixture("diff-stat");
    let options = DiffOptions {
        base: Some("main".to_string()),
        stat: true,
        ..DiffOptions::default()
    };
    let diff = fixture.git.diff_in(&b, None, &options).unwrap();
    assert!(diff.contains("3 files changed"), "{diff}");
    assert!(!diff.contains("@@"), "{diff}");
}

#[test]
fn a_base_that_is_an_option_or_a_shell_fragment_is_refused_before_git_runs() {
    let (fixture, b) = ahead_fixture("diff-bad-base");
    let output = fixture.outside().join("x");
    for base in [
        format!("--output={}", output.display()),
        "main..; rm".to_string(),
        "-p".to_string(),
        "main:README.md".to_string(),
    ] {
        let options = DiffOptions {
            base: Some(base.clone()),
            ..DiffOptions::default()
        };
        let result = fixture.git.diff_in(&b, None, &options);
        assert!(
            matches!(result, Err(ClientError::InvalidInput(_))),
            "{base}: {result:?}"
        );
    }
    assert!(!output.exists(), "git ran with the option");
    let audit = text_under(&fixture.root.join("data"));
    assert!(!audit.contains("git_diff_read"), "git ran: {audit}");
}

#[test]
fn a_diff_path_escaping_the_worktree_is_refused() {
    let (fixture, b) = ahead_fixture("diff-escape");
    for path in ["../a/README.md", "../../../README.md", "/etc/hosts"] {
        let options = DiffOptions {
            paths: vec![path.to_string()],
            ..DiffOptions::default()
        };
        let result = fixture.git.diff_in(&b, None, &options);
        assert!(
            matches!(result, Err(ClientError::AccessDenied(_))),
            "{path}: {result:?}"
        );
    }
}

#[test]
fn read_git_diff_passes_paths_stat_and_base_from_the_tool_call() {
    let (fixture, _) = ahead_fixture("tool-diff-options");
    let wt = "{\"worktree\":\".claude/worktrees/b\"";
    let stat = format!("{wt},\"base\":\"main\",\"stat\":true}}");
    let paths = format!("{wt},\"base\":\"main\",\"paths\":[\"committed.txt\"]}}");
    let bad = format!("{wt},\"base\":\"--output=x\"}}");
    let turn = fixture.tools(&[
        ("read_git_diff", &stat),
        ("read_git_diff", &paths),
        ("read_git_diff", &bad),
    ]);
    assert!(
        turn.results[0].contains("3 files changed"),
        "{:?}",
        turn.results
    );
    assert!(
        turn.results[1].contains("+on wt-b") && !turn.results[1].contains("README.md"),
        "{:?}",
        turn.results
    );
    assert!(
        turn.results[2].starts_with("Cannot read git diff"),
        "{:?}",
        turn.results
    );
}

// ---- Task 4: `list_worktrees` and `read_git_log` (§5.2, §5.5, §5.6) ----

impl Fixture {
    /// `git -C <dir>`'s stdout, for asserting a tool output matches git's.
    fn git_out(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
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

    fn short_head(&self, dir: &Path) -> String {
        self.git_out(dir, &["rev-parse", "--short", "--verify", "-q", "HEAD"])
            .trim()
            .to_string()
    }
}

fn entry<'a>(listing: &'a WorktreeListing, path: &str) -> &'a WorktreeListingEntry {
    listing
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("no entry for {path} in {listing:?}"))
}

#[test]
fn list_worktrees_reports_path_branch_head_changed_count_and_ahead_behind() {
    let (fixture, _) = ahead_fixture("list-fields");
    let listing = fixture.git.list_worktrees(&fixture.repo, None).unwrap();
    assert_eq!(listing.base.as_deref(), Some("main"));
    assert_eq!(listing.entries.len(), 2);
    assert_eq!(listing.omitted, 0);

    let a = entry(&listing, ".claude/worktrees/a");
    assert_eq!(a.branch.as_deref(), Some("wt-a"));
    assert_eq!(
        a.head.as_deref(),
        Some(fixture.short_head(&fixture.worktree("a")).as_str())
    );
    assert_eq!(a.changed_files, 0);
    assert_eq!((a.ahead, a.behind), (Some(0), Some(0)));

    // Worktree b is one commit ahead of `main` and has an uncommitted change.
    let b = entry(&listing, ".claude/worktrees/b");
    assert_eq!(b.branch.as_deref(), Some("wt-b"));
    assert_eq!(
        b.head.as_deref(),
        Some(fixture.short_head(&fixture.worktree("b")).as_str())
    );
    assert_eq!(b.changed_files, 1);
    assert_eq!((b.ahead, b.behind), (Some(1), Some(0)));
}

#[test]
fn from_a_linked_worktree_list_worktrees_reads_none_but_names_them_without_allowed_roots() {
    // Spec 60 §5.1 / C1: the containment root is the session's own checkout,
    // not the repository's main worktree. A session in a linked worktree
    // therefore reads none of the others, but names each one and why, so the
    // listing does not read as "no worktrees". Its own checkout is in neither.
    let fixture = fixture("list-from-linked");
    let session_root = fixture.worktree("a");
    let listing = fixture.git.list_worktrees(&session_root, None).unwrap();
    assert_eq!(listing.base.as_deref(), Some("main"));
    assert_eq!(listing.entries.len(), 0, "{listing:?}");
    assert_eq!(listing.omitted, 0, "{listing:?}");
    let main = fs::canonicalize(&fixture.repo).unwrap();
    let named: Vec<(String, WorktreeRefusal)> = listing
        .unreachable
        .iter()
        .map(|worktree| (worktree.path.clone(), worktree.reason))
        .collect();
    assert_eq!(
        named,
        vec![
            (
                main.to_string_lossy().to_string(),
                WorktreeRefusal::OutsideAllowedRoots
            ),
            (
                fixture.worktree("b").to_string_lossy().to_string(),
                WorktreeRefusal::OutsideAllowedRoots
            ),
        ],
        "{listing:?}"
    );

    // The documented remedy: allow the main repository, and the main checkout
    // and the sibling become reachable from the linked session. The session's
    // own checkout stays excluded.
    let scanner = SecretScanner::new(Vec::new());
    let allowed_git = GitService::new(
        AuditLog::new(fixture.root.join("data-allowed"), true, scanner.clone()),
        scanner,
        policy_allowing(&fixture.repo),
        GitReadCaps::from_config(&Config::default()),
    );
    let listing = allowed_git.list_worktrees(&session_root, None).unwrap();
    assert_eq!(listing.entries.len(), 2, "{listing:?}");
    assert!(listing.unreachable.is_empty(), "{listing:?}");
    let main = fs::canonicalize(&fixture.repo).unwrap();
    let paths: Vec<String> = listing
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    assert!(
        paths.contains(&main.to_string_lossy().to_string()),
        "{listing:?}"
    );
    assert!(
        paths.contains(&fixture.worktree("b").to_string_lossy().to_string()),
        "{listing:?}"
    );
}

#[test]
fn an_outside_worktree_is_named_with_branch_head_and_ahead_behind_but_not_read() {
    // The motivating case (spec 60 §7, 2026-10-29): a tool's worktrees live
    // outside the repository root, and a listing that left them out made a
    // model report "nothing in progress" with a branch six commits ahead.
    let fixture = fixture("list-outside-named");
    let outside = fixture.outside().join("wt");
    fixture.run(&[
        "worktree",
        "add",
        "-q",
        outside.to_str().unwrap(),
        "-b",
        "wt-out",
    ]);
    fs::write(outside.join("committed.txt"), "out\n").unwrap();
    fixture.run_in(&outside, &["add", "-A"]);
    fixture.run_in(&outside, &["commit", "-q", "-m", "ahead outside"]);

    let listing = fixture.git.list_worktrees(&fixture.repo, None).unwrap();
    // The reachable ones are unchanged.
    assert_eq!(listing.entries.len(), 2, "{listing:?}");
    assert_eq!(listing.unreachable.len(), 1, "{listing:?}");
    let named = &listing.unreachable[0];
    assert_eq!(
        named.path,
        fs::canonicalize(&outside).unwrap().to_string_lossy()
    );
    assert_eq!(named.branch.as_deref(), Some("wt-out"));
    assert_eq!(
        named.head.as_deref(),
        Some(fixture.short_head(&outside).as_str())
    );
    assert_eq!((named.ahead, named.behind), (Some(1), Some(0)));
    assert_eq!(named.reason, WorktreeRefusal::OutsideAllowedRoots);
}

#[test]
fn a_removed_worktree_directory_is_named_as_prunable() {
    let fixture = fixture("list-prunable");
    fs::remove_dir_all(fixture.worktree("b")).unwrap();
    let listing = fixture.git.list_worktrees(&fixture.repo, None).unwrap();
    assert_eq!(listing.entries.len(), 1, "{listing:?}");
    assert_eq!(listing.unreachable.len(), 1, "{listing:?}");
    assert_eq!(listing.unreachable[0].branch.as_deref(), Some("wt-b"));
    assert_eq!(listing.unreachable[0].reason, WorktreeRefusal::Prunable);
}

#[test]
fn the_list_worktrees_tool_names_unreadable_worktrees_and_why() {
    let fixture = fixture("tools-listing-unreadable");
    let outside = fixture.outside().join("wt");
    fixture.run(&[
        "worktree",
        "add",
        "-q",
        outside.to_str().unwrap(),
        "-b",
        "wt-out",
    ]);
    let turn = fixture.tools(&[("list_worktrees", "{}")]);
    assert!(!turn.approval_requested);
    let text = &turn.results[0];
    assert!(
        text.contains("Not readable from this session (1)"),
        "{text}"
    );
    let line = text
        .lines()
        .find(|line| line.contains("wt-out"))
        .unwrap_or_else(|| panic!("no line for wt-out in {text}"));
    assert!(
        line.contains("0 ahead, 0 behind") && line.contains("allowed_roots"),
        "{line}"
    );
    assert!(
        turn.records
            .lines()
            .any(|line| line.contains("git_worktrees_listed")
                && line.contains("\"unreachable\":\"1\"")),
        "{}",
        turn.records
    );
}

#[test]
fn list_worktrees_omits_ahead_behind_when_the_main_worktree_is_detached() {
    let fixture = fixture("list-detached");
    fixture.run(&["checkout", "--quiet", "--detach"]);
    let listing = fixture.git.list_worktrees(&fixture.repo, None).unwrap();
    assert_eq!(listing.base, None);
    for entry in &listing.entries {
        assert_eq!(entry.ahead, None, "{entry:?}");
        assert_eq!(entry.behind, None, "{entry:?}");
    }
}

#[test]
fn list_worktrees_does_not_count_restricted_changes() {
    let fixture = fixture("list-restricted");
    let b = fixture.worktree("b");
    fixture.write_in("b", ".env", "TOKEN=one\n");
    fixture.run_in(&b, &["add", "-f", ".env"]);
    fixture.run_in(&b, &["commit", "-q", "-m", "secret"]);
    fixture.write_in("b", ".env", "TOKEN=two\n");
    fixture.write_in("b", "app.txt", "free\n");

    let listing = fixture.git.list_worktrees(&fixture.repo, None).unwrap();
    assert_eq!(entry(&listing, ".claude/worktrees/b").changed_files, 1);
}

#[test]
fn list_worktrees_is_capped_at_twenty_with_the_rest_counted() {
    let fixture = fixture("list-capped");
    for index in 0..20 {
        let name = format!("extra-{index}");
        fixture.run(&[
            "worktree",
            "add",
            "-q",
            &format!(".claude/worktrees/{name}"),
            "-b",
            &format!("wt-{name}"),
        ]);
    }
    // 22 reachable worktrees (a, b, plus 20 extras): 20 listed, 2 counted.
    let listing = fixture.git.list_worktrees(&fixture.repo, None).unwrap();
    assert_eq!(listing.entries.len(), 20, "{listing:?}");
    assert_eq!(listing.omitted, 2, "{listing:?}");
}

#[test]
fn read_git_log_returns_short_sha_subject_lines_newest_first() {
    let fixture = fixture("log-newest");
    let b = fixture.worktree("b");
    for (index, subject) in ["first", "second", "third"].iter().enumerate() {
        fixture.write_in("b", &format!("f{index}.txt"), "x\n");
        fixture.run_in(&b, &["add", "-A"]);
        fixture.run_in(&b, &["commit", "-q", "-m", subject]);
    }
    let log = fixture
        .git
        .log_in(&b, Some(".claude/worktrees/b"), Some("main"), None)
        .unwrap();
    let expected = fixture.git_out(&b, &["log", "--oneline", "main..HEAD"]);
    assert_eq!(log, expected);
    let subjects: Vec<&str> = log
        .lines()
        .map(|line| line.split(' ').nth(1).unwrap())
        .collect();
    assert_eq!(subjects, vec!["third", "second", "first"]);
}

#[test]
fn read_git_log_is_capped_by_max_count() {
    let fixture = fixture("log-max-count");
    let b = fixture.worktree("b");
    for index in 0..4 {
        fixture.write_in("b", &format!("f{index}.txt"), "x\n");
        fixture.run_in(&b, &["add", "-A"]);
        fixture.run_in(&b, &["commit", "-q", "-m", &format!("commit-{index}")]);
    }
    let log = fixture.git.log_in(&b, None, Some("main"), Some(2)).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert_eq!(lines[0].split(' ').nth(1).unwrap(), "commit-3");
    assert_eq!(lines[1].split(' ').nth(1).unwrap(), "commit-2");
}

#[test]
fn read_git_log_is_capped_by_the_configured_cap() {
    let fixture = fixture("log-configured-cap");
    let b = fixture.worktree("b");
    for index in 0..5 {
        fixture.write_in("b", &format!("f{index}.txt"), "x\n");
        fixture.run_in(&b, &["add", "-A"]);
        fixture.run_in(&b, &["commit", "-q", "-m", &format!("commit {index}")]);
    }
    let git = git_service(
        &fixture.root,
        GitReadCaps {
            max_lines: 3,
            max_bytes: 1024 * 1024,
        },
    );
    let log = git.log_in(&b, None, Some("main"), None).unwrap();
    assert_eq!(log.lines().count(), 3, "{log}");
}

#[test]
fn list_worktrees_and_read_git_log_batch_read_only_and_audit() {
    let (fixture, _) = ahead_fixture("tools-listing-log");
    let turn = fixture.tools(&[
        ("list_worktrees", "{}"),
        (
            "read_git_log",
            "{\"worktree\":\".claude/worktrees/b\",\"base\":\"main\"}",
        ),
    ]);
    assert!(!turn.approval_requested);
    assert_eq!(turn.results.len(), 2);
    assert!(
        turn.results[0].contains(".claude/worktrees/a")
            && turn.results[0].contains(".claude/worktrees/b")
            && turn.results[0].contains("ahead/behind vs `main`"),
        "{:?}",
        turn.results
    );
    assert!(turn.results[1].contains(" ahead"), "{:?}", turn.results);
    assert!(
        turn.records
            .lines()
            .any(|line| line.contains("git_worktrees_listed")),
        "list_worktrees must audit"
    );
    assert!(
        turn.records
            .lines()
            .any(|line| line.contains("git_log_read")),
        "read_git_log must audit"
    );
}
