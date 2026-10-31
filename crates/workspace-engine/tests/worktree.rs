//! Worker worktrees, per `docs/specs/57_worker_sessions/proposal.md` §5.1.
//!
//! `.damaian/` is repository-controlled (#34), so every refusal here also
//! asserts that nothing changed: no new branch, no exclude edit, and no
//! directory. Each test runs against its own fresh `git init` fixture.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::ClientError;
use workspace_engine::worktree::{
    RemoveOutcome, create_worker_worktree, remove_worker_worktree, source_changes, worker_slug,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    /// Holds the repository and, beside it, a directory outside it.
    root: PathBuf,
    repo: PathBuf,
}

/// A repository with one commit: `README.md`, `a.txt`, and a `.gitignore`
/// that ignores `*.log`.
fn fixture(name: &str) -> Fixture {
    let fixture = bare_dir(name);
    fixture.git(&["init", "-q"]);
    fixture.git(&["config", "user.name", "Damaian Test"]);
    fixture.git(&["config", "user.email", "test@example.invalid"]);
    fixture.git(&["config", "commit.gpgsign", "false"]);
    fixture.write("README.md", "readme\n");
    fixture.write("a.txt", "original\n");
    fixture.write(".gitignore", "*.log\n");
    fixture.git(&["add", "."]);
    fixture.git(&["commit", "-q", "-m", "initial"]);
    fixture
}

/// An empty directory that is not a git repository.
fn bare_dir(name: &str) -> Fixture {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "damaian-worktree-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let repo = root.join("repo");
    fs::create_dir_all(&repo).expect("repository dir should be created");
    fs::create_dir_all(root.join("outside")).expect("outside dir should be created");
    Fixture { root, repo }
}

impl Fixture {
    fn write(&self, relative: &str, content: &str) {
        let path = self.repo.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn outside(&self) -> PathBuf {
        self.root.join("outside")
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.repo, args)
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn exclude(&self) -> String {
        fs::read_to_string(self.repo.join(".git/info/exclude")).unwrap_or_default()
    }

    fn worktree_dir(&self, slug: &str) -> PathBuf {
        canonical(&self.repo).join(".damaian/worktrees").join(slug)
    }

    /// Everything a refusal must leave alone.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            branches: self.git(&["for-each-ref", "--format=%(refname)", "refs/heads"]),
            worktrees: self.git(&["worktree", "list", "--porcelain"]),
            exclude: self.exclude(),
            repo_files: listing(&self.repo),
            outside_files: listing(&self.outside()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    branches: String,
    worktrees: String,
    exclude: String,
    repo_files: BTreeMap<String, String>,
    outside_files: BTreeMap<String, String>,
}

/// `--no-optional-locks` so that reading the fixture cannot itself rewrite
/// its index, which `the_source_tree_is_untouched_by_include` compares.
fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("--no-optional-locks")
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
    String::from_utf8(output.stdout).expect("git output should be UTF-8")
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).expect("path should canonicalize")
}

/// Every entry under `dir` except `.git`, without following symlinks: a file's
/// content, a symlink's target, or `<dir>`.
fn listing(dir: &Path) -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).unwrap().flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .to_string();
            if relative == ".git" {
                continue;
            }
            let file_type = entry.file_type().unwrap();
            let value = if file_type.is_symlink() {
                format!("-> {}", fs::read_link(&path).unwrap().display())
            } else if file_type.is_dir() {
                pending.push(path);
                "<dir>".to_string()
            } else {
                fs::read_to_string(&path).unwrap_or_else(|_| "<binary>".to_string())
            };
            entries.insert(relative, value);
        }
    }
    entries
}

fn branch_exists(fixture: &Fixture, branch: &str) -> bool {
    !fixture
        .git(&["for-each-ref", &format!("refs/heads/{branch}")])
        .trim()
        .is_empty()
}

#[test]
fn worker_slug_is_ascii_words_joined_by_dashes() {
    assert_eq!(worker_slug("Fix the Login Bug"), "fix-the-login-bug");
    assert_eq!(
        worker_slug("Implement Task 4 (worktree creation) from docs/specs/57_x/tasks.md."),
        "implement-task-4-worktree-creation-from"
    );
    // Only the first line names the worker.
    assert_eq!(worker_slug("First line\nSecond line"), "first-line");
    // At most 40 characters, cut at a word boundary and never ending in `-`.
    let long = worker_slug("alpha beta gamma delta epsilon zeta eta theta iota kappa");
    assert!(long.len() <= 40, "{long}");
    assert_eq!(long, "alpha-beta-gamma-delta-epsilon-zeta-eta");
    // A single word longer than the limit is cut.
    assert_eq!(worker_slug(&"x".repeat(60)), "x".repeat(40));
    // Non-ASCII characters are dropped.
    assert_eq!(worker_slug("Ändere die Datei"), "ndere-die-datei");
    assert_eq!(worker_slug("日本語"), "worker");
    assert_eq!(worker_slug(""), "worker");
    assert_eq!(worker_slug("   \n  "), "worker");
    assert_eq!(worker_slug("--- !!! ---"), "worker");
}

#[test]
fn a_worktree_is_created_under_damaian_worktrees_on_a_damaian_branch() {
    let fixture = fixture("created");
    let head = fixture.head();

    let created = create_worker_worktree(&fixture.repo, "Fix the login bug\nDetails.", false)
        .expect("worktree should be created");

    assert_eq!(created.slug, "fix-the-login-bug");
    assert_eq!(created.branch, "damaian/fix-the-login-bug");
    assert_eq!(created.base_commit, head);
    assert_eq!(
        canonical(&created.path),
        fixture.worktree_dir("fix-the-login-bug")
    );
    assert!(created.path.join("README.md").is_file());
    assert!(branch_exists(&fixture, "damaian/fix-the-login-bug"));
    assert!(
        fixture
            .git(&["worktree", "list", "--porcelain"])
            .contains("branch refs/heads/damaian/fix-the-login-bug")
    );

    // Every git command is recorded exactly as it ran, the mutation included.
    assert!(
        created
            .git_commands
            .iter()
            .all(|command| command.starts_with("git --no-optional-locks -C ")),
        "{:#?}",
        created.git_commands
    );
    let add = format!(
        "worktree add -b damaian/fix-the-login-bug {} {head}",
        created.path.display()
    );
    assert!(
        created
            .git_commands
            .iter()
            .any(|command| command.ends_with(&add)),
        "no `{add}` in {:#?}",
        created.git_commands
    );
}

#[test]
fn the_exclude_line_is_added_once() {
    let fixture = fixture("exclude");
    let gitignore_before = fs::read(fixture.repo.join(".gitignore")).unwrap();

    create_worker_worktree(&fixture.repo, "first task", false).unwrap();
    create_worker_worktree(&fixture.repo, "second task", false).unwrap();

    let lines = fixture
        .exclude()
        .lines()
        .filter(|line| line.trim() == ".damaian/worktrees/")
        .count();
    assert_eq!(lines, 1, "{}", fixture.exclude());
    assert_eq!(
        fs::read(fixture.repo.join(".gitignore")).unwrap(),
        gitignore_before
    );
    // The exclude line is what keeps the worktrees out of the source's status.
    assert_eq!(
        fixture.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
        ""
    );
}

#[test]
fn colliding_names_get_matching_suffixes() {
    let fixture = fixture("collide");

    let first = create_worker_worktree(&fixture.repo, "Same task", false).unwrap();
    let second = create_worker_worktree(&fixture.repo, "Same task", false).unwrap();
    assert_eq!(first.slug, "same-task");
    assert_eq!(second.slug, "same-task-2");
    assert_eq!(second.branch, "damaian/same-task-2");
    assert_eq!(canonical(&second.path), fixture.worktree_dir("same-task-2"));

    // Only the branch exists.
    fixture.git(&["branch", "damaian/branch-only"]);
    let branch_only = create_worker_worktree(&fixture.repo, "Branch only", false).unwrap();
    assert_eq!(branch_only.slug, "branch-only-2");
    assert_eq!(branch_only.branch, "damaian/branch-only-2");
    assert_eq!(
        canonical(&branch_only.path),
        fixture.worktree_dir("branch-only-2")
    );
    assert!(!fixture.worktree_dir("branch-only").exists());

    // Only the path exists.
    fs::create_dir_all(fixture.repo.join(".damaian/worktrees/path-only")).unwrap();
    let path_only = create_worker_worktree(&fixture.repo, "Path only", false).unwrap();
    assert_eq!(path_only.slug, "path-only-2");
    assert_eq!(path_only.branch, "damaian/path-only-2");
    assert!(!branch_exists(&fixture, "damaian/path-only"));
}

#[test]
fn refuses_a_symlink_anywhere_in_the_path() {
    // `.damaian` itself is a symlink, to a directory inside the repository, so
    // only the per-component check can catch it: the resolved path is inside.
    let first = fixture("symlink-damaian");
    let fixture = &first;
    fs::create_dir_all(fixture.repo.join("shipped/worktrees")).unwrap();
    std::os::unix::fs::symlink("shipped", fixture.repo.join(".damaian")).unwrap();
    fixture.write("a.txt", "edited\n");
    let before = fixture.snapshot();

    let error = create_worker_worktree(&fixture.repo, "Symlinked", true)
        .expect_err("a symlinked .damaian must be refused");
    assert!(matches!(error, ClientError::AccessDenied(_)), "{error:?}");
    assert_eq!(fixture.snapshot(), before);

    // `.damaian/worktrees` is a symlink.
    let second = self::fixture("symlink-worktrees");
    let fixture = &second;
    fs::create_dir_all(fixture.repo.join("shipped")).unwrap();
    fs::create_dir_all(fixture.repo.join(".damaian")).unwrap();
    std::os::unix::fs::symlink("../shipped", fixture.repo.join(".damaian/worktrees")).unwrap();
    let before = fixture.snapshot();

    let error = create_worker_worktree(&fixture.repo, "Symlinked", false)
        .expect_err("a symlinked .damaian/worktrees must be refused");
    assert!(matches!(error, ClientError::AccessDenied(_)), "{error:?}");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn refuses_a_tracked_path_under_worktrees() {
    let fixture = fixture("tracked");
    fixture.write(".damaian/worktrees/fix-it/planted.txt", "planted\n");
    fixture.git(&["add", "."]);
    fixture.git(&["commit", "-q", "-m", "plant a worktree path"]);
    // Tracked but absent from disk, so neither the path nor the branch exists
    // and only `git ls-files` can see it.
    fs::remove_dir_all(fixture.repo.join(".damaian")).unwrap();
    let before = fixture.snapshot();

    let error = create_worker_worktree(&fixture.repo, "Fix it", false)
        .expect_err("a tracked worktree path must be refused");
    assert!(matches!(error, ClientError::AccessDenied(_)), "{error:?}");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn refuses_a_non_git_source() {
    let fixture = bare_dir("non-git");
    fs::write(fixture.repo.join("file.txt"), "content\n").unwrap();
    let before = listing(&fixture.repo);

    let error = create_worker_worktree(&fixture.repo, "Anything", true)
        .expect_err("a non-git source must be refused");
    assert!(matches!(error, ClientError::InvalidInput(_)), "{error:?}");
    assert_eq!(listing(&fixture.repo), before);
    assert!(!fixture.repo.join(".git").exists());
}

#[test]
fn refuses_a_subdirectory_of_a_repository() {
    let fixture = fixture("subdirectory");
    fixture.write("nested/file.txt", "content\n");
    let before = fixture.snapshot();

    let error = create_worker_worktree(&fixture.repo.join("nested"), "Anything", false)
        .expect_err("only a repository's top level hosts worktrees");
    assert!(matches!(error, ClientError::InvalidInput(_)), "{error:?}");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn refuses_a_path_that_resolves_outside_the_repository() {
    let fixture = fixture("outside");
    std::os::unix::fs::symlink(fixture.outside(), fixture.repo.join(".damaian")).unwrap();
    let before = fixture.snapshot();

    let error = create_worker_worktree(&fixture.repo, "Escape", true)
        .expect_err("a path outside the repository must be refused");
    assert!(matches!(error, ClientError::AccessDenied(_)), "{error:?}");
    assert_eq!(fixture.snapshot(), before);
    assert!(listing(&fixture.outside()).is_empty());
}

#[test]
fn include_carries_tracked_edits_and_untracked_files() {
    let fixture = fixture("include");
    let head = fixture.head();
    fixture.write("a.txt", "edited\n");
    fixture.write("docs/new/spec.md", "untracked spec\n");
    fixture.write("debug.log", "ignored\n");

    let created = create_worker_worktree(&fixture.repo, "Carry it over", true).unwrap();

    assert_ne!(
        created.base_commit, head,
        "tracked edits need a stash commit"
    );
    assert_eq!(
        fs::read_to_string(created.path.join("a.txt")).unwrap(),
        "edited\n"
    );
    assert_eq!(
        fs::read_to_string(created.path.join("docs/new/spec.md")).unwrap(),
        "untracked spec\n"
    );
    assert!(
        !created.path.join("debug.log").exists(),
        "ignored files stay"
    );
    assert!(
        created
            .git_commands
            .iter()
            .any(|command| command.ends_with(" stash create")),
        "{:#?}",
        created.git_commands
    );
}

#[test]
fn start_from_last_commit_carries_neither() {
    let fixture = fixture("last-commit");
    let head = fixture.head();
    fixture.write("a.txt", "edited\n");
    fixture.write("docs/new/spec.md", "untracked spec\n");

    let created = create_worker_worktree(&fixture.repo, "Leave it", false).unwrap();

    assert_eq!(created.base_commit, head);
    assert_eq!(
        fs::read_to_string(created.path.join("a.txt")).unwrap(),
        "original\n"
    );
    assert!(!created.path.join("docs/new/spec.md").exists());
    assert!(
        !created
            .git_commands
            .iter()
            .any(|command| command.contains("stash")),
        "{:#?}",
        created.git_commands
    );
}

#[test]
fn include_with_no_tracked_changes_bases_on_head() {
    let fixture = fixture("untracked-only");
    let head = fixture.head();
    fixture.write("notes.md", "only untracked\n");

    let created = create_worker_worktree(&fixture.repo, "Untracked only", true).unwrap();

    // `git stash create` printed nothing.
    assert_eq!(created.base_commit, head);
    assert_eq!(
        fs::read_to_string(created.path.join("notes.md")).unwrap(),
        "only untracked\n"
    );
}

#[test]
fn the_source_tree_is_untouched_by_include() {
    let fixture = fixture("untouched");
    fixture.write("a.txt", "edited\n");
    fixture.write("staged.txt", "staged\n");
    fixture.git(&["add", "staged.txt"]);
    fixture.write("untracked.md", "untracked\n");
    let state = |fixture: &Fixture| {
        (
            fixture.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
            fixture.git(&["ls-files", "--stage"]),
            fixture.git(&["diff"]),
            fixture.git(&["diff", "--cached"]),
            fixture.git(&["stash", "list"]),
            fixture.head(),
        )
    };
    let before = state(&fixture);
    let mut files_before = listing(&fixture.repo);

    create_worker_worktree(&fixture.repo, "Leave the source alone", true).unwrap();

    assert_eq!(state(&fixture), before);
    let mut files_after = listing(&fixture.repo);
    files_after.retain(|path, _| !path.starts_with(".damaian"));
    files_before.retain(|path, _| !path.starts_with(".damaian"));
    assert_eq!(files_after, files_before);
}

#[test]
fn source_changes_lists_files_the_prompt_mentions_first() {
    let fixture = fixture("changes");
    fixture.write("a.txt", "edited\n");
    fixture.write("README.md", "edited\n");
    fixture.write("docs/specs/new/tasks.md", "untracked\n");
    fixture.write("debug.log", "ignored\n");

    let changes = source_changes(
        &fixture.repo,
        "Implement Task 1 from docs/specs/new/tasks.md and touch a.txt.",
    )
    .unwrap();

    assert_eq!(changes.mentioned, vec!["docs/specs/new/tasks.md", "a.txt"]);
    assert_eq!(changes.paths.len(), 3, "{:?}", changes.paths);
    assert_eq!(changes.paths[..2], changes.mentioned[..]);
    assert!(changes.paths.contains(&"README.md".to_string()));
    assert!(!changes.paths.contains(&"debug.log".to_string()));

    // A path is mentioned only as a whole path, not inside a longer one.
    let changes = source_changes(&fixture.repo, "See data.txt and README.mdx.").unwrap();
    assert!(changes.mentioned.is_empty(), "{:?}", changes.mentioned);

    let clean = self::fixture("clean");
    let changes = source_changes(&clean.repo, "anything").unwrap();
    assert!(changes.paths.is_empty() && changes.mentioned.is_empty());
}

#[test]
fn removing_a_clean_worktree_keeps_its_branch() {
    let fixture = fixture("remove-clean");
    let created = create_worker_worktree(&fixture.repo, "Remove me", false).unwrap();

    let outcome = remove_worker_worktree(&fixture.repo, &created.path, false).unwrap();

    assert_eq!(outcome, RemoveOutcome::Removed);
    assert!(!created.path.exists());
    assert!(branch_exists(&fixture, &created.branch));
    assert!(
        !fixture
            .git(&["worktree", "list", "--porcelain"])
            .contains(&created.branch)
    );
}

#[test]
fn removing_a_dirty_worktree_reports_paths_and_does_not_force() {
    let fixture = fixture("remove-dirty");
    let created = create_worker_worktree(&fixture.repo, "Dirty me", false).unwrap();
    fs::write(created.path.join("a.txt"), "worker edit\n").unwrap();
    fs::write(created.path.join("new.txt"), "worker file\n").unwrap();

    let outcome = remove_worker_worktree(&fixture.repo, &created.path, false).unwrap();

    let RemoveOutcome::Dirty { paths } = outcome else {
        panic!("expected Dirty, got {outcome:?}");
    };
    assert_eq!(paths, vec!["a.txt", "new.txt"]);
    assert_eq!(
        fs::read_to_string(created.path.join("new.txt")).unwrap(),
        "worker file\n"
    );

    let outcome = remove_worker_worktree(&fixture.repo, &created.path, true).unwrap();
    assert_eq!(outcome, RemoveOutcome::Removed);
    assert!(!created.path.exists());
    assert!(branch_exists(&fixture, &created.branch));
}

/// Spec 87: a worker worktree removed outside Damaian is already gone, not a
/// refusal, and git's stale record of it is pruned. The branch stays.
#[test]
fn removing_a_worktree_that_is_already_gone_prunes_it() {
    let fixture = fixture("remove-gone");
    let created = create_worker_worktree(&fixture.repo, "Gone already", false).unwrap();
    fs::remove_dir_all(&created.path).unwrap();

    let outcome = remove_worker_worktree(&fixture.repo, &created.path, false).unwrap();

    assert_eq!(outcome, RemoveOutcome::AlreadyGone);
    assert!(
        !fixture
            .git(&["worktree", "list", "--porcelain"])
            .contains(&created.branch),
        "git's record of the worktree is pruned"
    );
    assert!(branch_exists(&fixture, &created.branch));
}

/// A missing path anywhere else is still refused.
#[test]
fn a_missing_path_outside_the_worker_worktrees_is_still_refused() {
    let fixture = fixture("remove-gone-elsewhere");

    let error = remove_worker_worktree(&fixture.repo, &fixture.repo.join("nowhere/x"), false)
        .expect_err("only a worker worktree may be reported gone");
    assert!(matches!(error, ClientError::AccessDenied(_)), "{error:?}");
}

#[test]
fn removal_refuses_a_path_that_is_not_a_worker_worktree() {
    let fixture = fixture("remove-elsewhere");
    let before = fixture.snapshot();

    let error = remove_worker_worktree(&fixture.repo, &fixture.repo, true)
        .expect_err("only a worktree under .damaian/worktrees may be removed");
    assert!(matches!(error, ClientError::AccessDenied(_)), "{error:?}");
    assert_eq!(fixture.snapshot(), before);
}
