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
    AuditLog, BRANCH_SLUG_MAX_CHARS, BranchError, BranchInfo, ClientError, Config, GitMutation,
    GitReadCaps, GitService, PathPolicy, SecretScanner, SessionMode, WorkspaceEngine,
    repository_id_for_root, slugify_branch_title, suggest_branch_name, validate_branch_prefix,
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

#[test]
fn a_slug_is_lowercase_hyphenated_and_bounded() {
    assert_eq!(
        slugify_branch_title("Fix the Retry Loop!"),
        "fix-the-retry-loop"
    );
    assert_eq!(slugify_branch_title("  --Hello__World--  "), "hello-world");
    assert_eq!(slugify_branch_title(""), "change");
    assert_eq!(slugify_branch_title("!!!"), "change");
    assert_eq!(slugify_branch_title("fix 日本 now"), "fix-now");

    let long = slugify_branch_title(&"a".repeat(120));
    assert_eq!(long.len(), BRANCH_SLUG_MAX_CHARS);
    let trailing_separator = slugify_branch_title(&format!("{} broken", "x".repeat(49)));
    assert!(trailing_separator.len() <= BRANCH_SLUG_MAX_CHARS);
    assert!(!trailing_separator.ends_with('-'), "{trailing_separator}");
}

#[test]
fn the_default_branch_prefix_is_damaian() {
    assert_eq!(Config::default().git_branch_prefix, "damaian/");
}

#[test]
fn a_suggestion_uses_the_configured_prefix_and_reports_a_collision() {
    let fixture = fixture("suggestion", true);
    let scanner = SecretScanner::new(Vec::new());
    let suggestion = suggest_branch_name(
        &fixture.git,
        &scanner,
        &fixture.repo,
        "team/",
        "Fix the retry loop",
    )
    .unwrap();
    assert_eq!(suggestion.name, "team/fix-the-retry-loop");
    assert!(suggestion.valid);
    assert!(!suggestion.exists);
    assert_eq!(suggestion.alternative, None);

    fixture.run(&["branch", &suggestion.name]);
    let colliding = suggest_branch_name(
        &fixture.git,
        &scanner,
        &fixture.repo,
        "team/",
        "Fix the retry loop",
    )
    .unwrap();
    assert_eq!(colliding.name, "team/fix-the-retry-loop");
    assert!(colliding.exists);
    assert_eq!(
        colliding.alternative.as_deref(),
        Some("team/fix-the-retry-loop-2")
    );
    // The original name was not silently changed or suffixed.
    assert_eq!(
        fixture.git.local_branches(&fixture.repo).unwrap().len(),
        2,
        "main plus the original suggestion only"
    );

    fixture.run(&["branch", "team/fix-the-retry-loop-2"]);
    let twice = suggest_branch_name(
        &fixture.git,
        &scanner,
        &fixture.repo,
        "team/",
        "Fix the retry loop",
    )
    .unwrap();
    assert_eq!(
        twice.alternative.as_deref(),
        Some("team/fix-the-retry-loop-3")
    );
}

#[test]
fn an_empty_prefix_is_allowed_and_a_previous_branch_expression_is_refused() {
    let fixture = fixture("prefix", true);
    let scanner = SecretScanner::new(Vec::new());
    let suggestion =
        suggest_branch_name(&fixture.git, &scanner, &fixture.repo, "", "Fix the parser").unwrap();
    assert_eq!(suggestion.name, "fix-the-parser");

    let error = suggest_branch_name(
        &fixture.git,
        &scanner,
        &fixture.repo,
        "feature@{1}/",
        "Fix the parser",
    )
    .unwrap_err();
    assert!(
        matches!(error, ClientError::InvalidInput(ref message) if message.contains("not a branch-name prefix")),
        "{error:?}"
    );
    for prefix in [
        "bad prefix/",
        "../escape/",
        "/absolute/",
        "double//slash/",
        ".hidden/",
        "trailing./",
    ] {
        assert!(
            validate_branch_prefix(prefix).is_err(),
            "{prefix} should be refused"
        );
    }
    assert!(validate_branch_prefix("damaian/").is_ok());
    assert!(validate_branch_prefix("team/sub/").is_ok());
    let refs_error = validate_branch_prefix("refs/heads/").unwrap_err();
    assert!(
        matches!(refs_error, ClientError::InvalidInput(ref message) if message.contains("refs/")),
        "{refs_error:?}"
    );
    assert!(
        suggest_branch_name(
            &fixture.git,
            &scanner,
            &fixture.repo,
            "refs/heads/",
            "Fix the parser"
        )
        .is_err(),
        "a reserved refs/ prefix must not compose a branch name"
    );
}

#[test]
fn git_is_authoritative_about_a_branch_name() {
    let fixture = fixture("name-check", true);
    let valid = fixture
        .git
        .check_branch_name(&fixture.repo, "topic/lock")
        .unwrap();
    assert!(valid.valid, "{valid:?}");
    assert_eq!(valid.reason, None);

    let invalid = fixture
        .git
        .check_branch_name(&fixture.repo, "topic.lock")
        .unwrap();
    assert!(!invalid.valid, "{invalid:?}");
    assert!(
        invalid
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("not a valid branch name")),
        "{invalid:?}"
    );
}

#[test]
fn a_secret_is_redacted_before_it_can_become_a_branch_name() {
    let fixture = fixture("secret-name", true);
    let scanner = SecretScanner::new(Vec::new());
    let suggestion = suggest_branch_name(
        &fixture.git,
        &scanner,
        &fixture.repo,
        "damaian/",
        "rotate AKIAIOSFODNN7EXAMPLE before release",
    )
    .unwrap();
    assert!(suggestion.valid, "{suggestion:?}");
    assert!(
        !suggestion.name.to_ascii_lowercase().contains("akia"),
        "the key reached the branch name: {}",
        suggestion.name
    );
    assert!(
        !suggestion.name.contains("iosfodnn7example"),
        "the key reached the branch name: {}",
        suggestion.name
    );
}

// ---------------------------------------------------------------------------
// Task 4: creation and switching through the engine.
// ---------------------------------------------------------------------------

/// A repository with a fresh `WorkspaceEngine` and a Code-mode session. The
/// data directory sits beside the repository, never inside it, so Damaian's own
/// records cannot appear as working-tree changes.
struct EngineFixture {
    base: PathBuf,
    repo: PathBuf,
    data_dir: PathBuf,
    engine: WorkspaceEngine,
    session_id: String,
}

impl EngineFixture {
    fn new(name: &str) -> Self {
        let base = temp_root(&format!("engine-{name}"));
        let repo = base.join("repo");
        let data_dir = base.join("data");
        fs::create_dir_all(&repo).unwrap();
        let engine = WorkspaceEngine::new(Config {
            data_dir: data_dir.clone(),
            enable_index_watcher: false,
            ..Config::default()
        });
        let mut fixture = Self {
            base,
            repo,
            data_dir,
            engine,
            session_id: String::new(),
        };
        fixture.engine = fixture.rebuilt(|_| {});
        fixture.run(&["init", "-q", "--initial-branch=main"]);
        fixture.run(&["config", "user.name", "Damaian Test"]);
        fixture.run(&["config", "user.email", "test@example.invalid"]);
        fixture.run(&["config", "commit.gpgsign", "false"]);
        fixture.write("README.md", "readme\n");
        fixture.run(&["add", "."]);
        fixture.run(&["commit", "-q", "-m", "initial"]);
        let repository_id = repository_id_for_root(&fixture.repo);
        let session = fixture
            .engine
            .session_store
            .create_session(&repository_id, "Branch delivery")
            .unwrap();
        fixture
            .engine
            .session_store
            .set_session_mode(&session.id, SessionMode::Code, "user")
            .unwrap();
        fixture.session_id = session.id;
        fixture
    }

    fn rebuilt(&self, change: impl FnOnce(&mut Config)) -> WorkspaceEngine {
        let mut config = Config {
            data_dir: self.data_dir.clone(),
            enable_index_watcher: false,
            ..Config::default()
        };
        change(&mut config);
        WorkspaceEngine::new(config)
    }

    fn rebuild(&mut self, change: impl FnOnce(&mut Config)) {
        self.engine = self.rebuilt(change);
    }

    fn write(&self, path: &str, content: &str) {
        let path = self.repo.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.repo.join(path)).unwrap()
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
        String::from_utf8_lossy(&output.stdout).trim().to_string()
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
        self.run(&["rev-parse", "HEAD"])
    }

    fn branch_sha(&self, name: &str) -> String {
        self.run(&["rev-parse", &format!("{name}^{{commit}}")])
    }

    fn current_branch(&self) -> String {
        self.run(&["symbolic-ref", "--short", "HEAD"])
    }

    fn index_bytes(&self) -> Vec<u8> {
        fs::read(self.repo.join(".git/index")).unwrap_or_default()
    }

    fn status(&self) -> String {
        self.run(&["--no-optional-locks", "status", "--porcelain=v2"])
    }

    /// Everything a merge or rebase preparation must leave untouched, plus the
    /// marker a real merge would create. `false` in the last slot is the "no
    /// `.git/MERGE_HEAD` was created" assertion.
    fn read_only_snapshot(&self) -> (Vec<u8>, String, String, bool) {
        (
            self.index_bytes(),
            self.head(),
            self.status(),
            self.repo.join(".git").join("MERGE_HEAD").exists(),
        )
    }

    fn commit(&self, path: &str, content: &str, message: &str) {
        self.write(path, content);
        self.run(&["add", "."]);
        self.run(&["commit", "-q", "-m", message]);
    }

    fn prepare_create(
        &self,
        name: &str,
        start: Option<&str>,
    ) -> workspace_engine::BranchPreparation {
        self.engine
            .branch
            .prepare_create(&self.repo, name, start, &self.session_id)
            .unwrap()
    }

    fn prepare_switch(&self, name: &str) -> workspace_engine::BranchPreparation {
        self.engine
            .branch
            .prepare_switch(&self.repo, name, &self.session_id)
            .unwrap()
    }

    fn prepare_delete(&self, name: &str) -> workspace_engine::BranchPreparation {
        self.engine
            .branch
            .prepare_delete(&self.repo, name, &self.session_id)
            .unwrap()
    }

    fn prepare_merge(&self, base: &str) -> workspace_engine::MergePreparation {
        self.engine
            .branch
            .prepare_merge(&self.repo, base, &self.session_id)
            .unwrap()
    }

    fn prepare_rebase(&self, onto: &str) -> workspace_engine::RebasePreparation {
        self.engine
            .branch
            .prepare_rebase(&self.repo, onto, &self.session_id)
            .unwrap()
    }

    fn session_events(&self) -> Vec<serde_json::Value> {
        let path = self
            .data_dir
            .join("sessions")
            .join(format!("{}.jsonl", self.session_id));
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn audit_events(&self) -> Vec<serde_json::Value> {
        let path = self.data_dir.join("audit").join("events.jsonl");
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn no_force_flag(commands: &[String]) {
    for command in commands {
        assert!(
            !command
                .split_whitespace()
                .any(|token| token == "-f" || token == "--force"),
            "a constructed command carried a force flag: {command}"
        );
    }
}

/// Whether a recorded command line contains `args` as consecutive tokens.
fn has_arguments(command: &str, args: &[&str]) -> bool {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    tokens.windows(args.len()).any(|window| window == args)
}

/// Whether a recorded command line contains the token `flag`.
fn has_token(command: &str, flag: &str) -> bool {
    command.split_whitespace().any(|token| token == flag)
}

#[test]
fn creating_a_branch_leaves_head_and_the_index_unchanged() {
    let fixture = EngineFixture::new("create");
    let head = fixture.head();
    let index = fixture.index_bytes();
    let status = fixture.status();

    let preparation = fixture.prepare_create("damaian/fix-retry", None);
    assert!(preparation.can_apply, "{:?}", preparation.refusal);
    assert_eq!(preparation.start_point.as_deref(), Some("HEAD"));
    assert_eq!(preparation.start_point_sha.as_deref(), Some(head.as_str()));
    assert_eq!(
        preparation.mutation,
        GitMutation::CreateBranch {
            name: "damaian/fix-retry".to_string(),
            start_point: head.clone(),
        }
    );

    let outcome = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .expect("create should apply");

    assert_eq!(outcome.branch, "damaian/fix-retry");
    assert_eq!(
        outcome.created_ref.as_deref(),
        Some("refs/heads/damaian/fix-retry")
    );
    assert_eq!(outcome.head_after.as_deref(), Some(head.as_str()));
    assert_eq!(fixture.head(), head, "creation must not move HEAD");
    assert_eq!(fixture.current_branch(), "main");
    assert_eq!(fixture.branch_sha("damaian/fix-retry"), head);
    assert_eq!(fixture.index_bytes(), index, "the index must be untouched");
    assert_eq!(fixture.status(), status);

    no_force_flag(&outcome.git_commands);
    assert!(
        outcome
            .git_commands
            .iter()
            .any(|command| command.split_whitespace().any(|token| token == "branch")),
        "{:?}",
        outcome.git_commands
    );
    assert!(
        outcome
            .git_commands
            .iter()
            .all(|command| !command.split_whitespace().any(|token| token == "switch")),
        "creation must never switch: {:?}",
        outcome.git_commands
    );
    fixture.cleanup();
}

#[test]
fn create_then_switch_moves_head_and_the_working_tree() {
    let fixture = EngineFixture::new("create-switch");
    fixture.commit("a.txt", "first\n", "first");

    let create = fixture.prepare_create("damaian/feature", None);
    let created = fixture
        .engine
        .branch
        .apply(&create.prepare_id, &create.mutation, "tester", false)
        .unwrap();
    no_force_flag(&created.git_commands);

    // Move main so the new branch no longer shares its tip.
    fixture.commit("a.txt", "second\n", "second");

    let switch = fixture.prepare_switch("damaian/feature");
    assert!(switch.can_apply, "{:?}", switch.refusal);
    assert!(switch.dirty_paths.is_empty());
    let switched = fixture
        .engine
        .branch
        .apply(&switch.prepare_id, &switch.mutation, "tester", false)
        .unwrap();

    assert_eq!(switched.branch, "damaian/feature");
    assert_eq!(fixture.current_branch(), "damaian/feature");
    assert_eq!(fixture.head(), fixture.branch_sha("damaian/feature"));
    assert_eq!(
        fixture.read("a.txt"),
        "first\n",
        "the working tree followed"
    );
    assert_eq!(fixture.status(), "", "the switch left a clean tree");
    no_force_flag(&switched.git_commands);
    assert!(
        switched.git_commands.iter().any(|command| command
            .split_whitespace()
            .any(|token| token == "--no-guess")),
        "{:?}",
        switched.git_commands
    );
    fixture.cleanup();
}

#[test]
fn switching_an_existing_branch_updates_files() {
    let fixture = EngineFixture::new("switch-existing");
    fixture.run(&["switch", "-q", "-c", "topic"]);
    fixture.commit("topic.txt", "topic\n", "topic file");
    fixture.run(&["switch", "-q", "main"]);
    assert!(!fixture.repo.join("topic.txt").exists());

    let preparation = fixture.prepare_switch("topic");
    assert!(preparation.can_apply, "{:?}", preparation.refusal);
    fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .unwrap();

    assert_eq!(fixture.current_branch(), "topic");
    assert_eq!(fixture.read("topic.txt"), "topic\n");
    fixture.cleanup();
}

#[test]
fn mode_and_capability_refusals_name_the_reason_at_prepare_and_apply() {
    let mut fixture = EngineFixture::new("policy");
    let unknown = fixture
        .engine
        .branch
        .prepare_create(&fixture.repo, "damaian/policy", None, "ses_missing")
        .unwrap();
    assert!(!unknown.can_apply);
    assert!(
        unknown
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("Unknown session")),
        "{:?}",
        unknown.refusal
    );

    let elsewhere = fixture.base.join("not-a-repository");
    fs::create_dir_all(&elsewhere).unwrap();
    let not_a_repo = fixture
        .engine
        .branch
        .prepare_create(&elsewhere, "damaian/policy", None, &fixture.session_id)
        .unwrap();
    assert!(!not_a_repo.can_apply);
    assert!(
        not_a_repo
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("not a Git repository")),
        "{:?}",
        not_a_repo.refusal
    );

    for mode in [SessionMode::Ask, SessionMode::Plan, SessionMode::Review] {
        fixture
            .engine
            .session_store
            .set_session_mode(&fixture.session_id, mode, "user")
            .unwrap();
        let preparation = fixture.prepare_create("damaian/policy", None);
        assert!(!preparation.can_apply);
        let refusal = preparation.refusal.expect("a reason");
        assert!(refusal.contains("Code mode"), "{refusal}");
    }

    fixture
        .engine
        .session_store
        .set_session_mode(&fixture.session_id, SessionMode::Code, "user")
        .unwrap();
    let (data_dir, session_id) = (fixture.data_dir.clone(), fixture.session_id.clone());
    fixture.rebuild(|config| config.allow_git_branches = false);
    let denied = fixture.prepare_create("damaian/policy", None);
    assert!(!denied.can_apply);
    assert!(
        denied
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("allow_git_branches")),
        "{:?}",
        denied.refusal
    );
    fixture.engine = WorkspaceEngine::new(Config {
        data_dir,
        enable_index_watcher: false,
        ..Config::default()
    });

    // The same check runs again after the review: a prepared record approved
    // from Ask mode is refused without running git.
    let prepared = fixture.prepare_create("damaian/policy", None);
    assert!(prepared.can_apply);
    fixture
        .engine
        .session_store
        .set_session_mode(&session_id, SessionMode::Ask, "user")
        .unwrap();
    let refused = fixture
        .engine
        .branch
        .apply(&prepared.prepare_id, &prepared.mutation, "tester", false)
        .unwrap_err();
    assert!(
        matches!(refused, BranchError::Refused { .. }),
        "{refused:?}"
    );
    assert_eq!(refused.code(), "branch_not_allowed");
    assert!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/damaian/policy"])
            .status
            .code()
            != Some(0),
        "a refused apply must not create the ref"
    );

    // Narrowing the capability after the review is refused the same way.
    fixture
        .engine
        .session_store
        .set_session_mode(&session_id, SessionMode::Code, "user")
        .unwrap();
    fixture.rebuild(|config| config.allow_git_branches = false);
    let denied_at_apply = fixture
        .engine
        .branch
        .apply(&prepared.prepare_id, &prepared.mutation, "tester", false)
        .unwrap_err();
    assert_eq!(denied_at_apply.code(), "branch_not_allowed");

    // With the capability restored and Code mode, the same record still
    // applies: a policy refusal does not consume it.
    fixture.rebuild(|_| {});
    fixture
        .engine
        .branch
        .apply(&prepared.prepare_id, &prepared.mutation, "tester", false)
        .expect("the record was still prepared");
    assert_eq!(fixture.branch_sha("damaian/policy"), fixture.head());
    fixture.cleanup();
}

#[test]
fn a_mid_merge_or_detached_head_refuses_creation_with_a_reason() {
    let fixture = EngineFixture::new("states");
    fixture.commit("conflict.txt", "a\nb\nc\n", "base");
    fixture.run(&["switch", "-q", "-c", "other"]);
    fixture.commit("conflict.txt", "a\nX\nc\n", "other");
    fixture.run(&["switch", "-q", "main"]);
    fixture.commit("conflict.txt", "a\nY\nc\n", "main");
    fixture.run_failing(&["merge", "other"]);

    let mid_merge = fixture.prepare_create("damaian/mid-merge", None);
    assert!(!mid_merge.can_apply);
    assert!(
        mid_merge
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("merge")),
        "{:?}",
        mid_merge.refusal
    );
    fixture.run(&["merge", "--abort"]);

    fixture.run(&["checkout", "-q", "--detach"]);
    let detached = fixture.prepare_create("damaian/from-head", None);
    assert!(!detached.can_apply);
    assert!(
        detached
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("detached")),
        "{:?}",
        detached.refusal
    );

    // An explicit start point is the documented way out of a detached HEAD.
    let explicit = fixture.prepare_create("damaian/from-detached", Some("HEAD"));
    assert!(explicit.can_apply, "{:?}", explicit.refusal);
    let head = fixture.head();
    fixture
        .engine
        .branch
        .apply(&explicit.prepare_id, &explicit.mutation, "tester", false)
        .unwrap();
    assert_eq!(fixture.branch_sha("damaian/from-detached"), head);
    assert_eq!(fixture.head(), head, "HEAD stays detached after creation");
    assert!(
        fixture
            .try_run(&["symbolic-ref", "-q", "HEAD"])
            .status
            .code()
            != Some(0)
    );
    fixture.cleanup();
}

#[test]
fn a_collision_an_invalid_name_and_an_option_shaped_start_are_refused() {
    let fixture = EngineFixture::new("refusals");

    let collision = fixture.prepare_create("main", None);
    assert!(!collision.can_apply);
    let collision_reason = collision.refusal.clone().unwrap();
    assert!(
        collision_reason.contains("already exists"),
        "{collision_reason}"
    );
    assert_eq!(
        collision
            .suggestion
            .as_ref()
            .and_then(|s| s.alternative.as_deref()),
        Some("main-2")
    );

    let invalid = fixture.prepare_create("topic.lock", None);
    assert!(!invalid.can_apply);
    assert!(
        invalid
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("not a valid branch name")),
        "{:?}",
        invalid.refusal
    );

    let option_shaped_name = fixture.prepare_create("-dangerous", None);
    assert!(!option_shaped_name.can_apply);
    assert!(
        option_shaped_name
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("not a branch name Git would accept")),
        "{:?}",
        option_shaped_name.refusal
    );

    let option_shaped_start = fixture.prepare_create("damaian/safe", Some("--output=/tmp/x"));
    assert!(!option_shaped_start.can_apply);
    assert!(
        option_shaped_start
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("start with `-`")),
        "{:?}",
        option_shaped_start.refusal
    );

    let unresolved = fixture.prepare_create("damaian/safe", Some("no-such-ref"));
    assert!(!unresolved.can_apply);
    assert!(
        unresolved
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("did not resolve")),
        "{:?}",
        unresolved.refusal
    );

    for refused in [
        collision,
        invalid,
        option_shaped_name,
        option_shaped_start,
        unresolved,
    ] {
        assert!(
            fixture
                .engine
                .branch
                .load_record(&refused.prepare_id)
                .is_err(),
            "a refused preparation must not leave a record"
        );
    }
    fixture.cleanup();
}

#[test]
fn a_dirty_switch_names_the_paths_it_would_overwrite() {
    let fixture = EngineFixture::new("dirty-switch");
    fixture.commit("shared.txt", "base\n", "base");
    fixture.run(&["switch", "-q", "-c", "other"]);
    fixture.commit("shared.txt", "other\n", "other");
    fixture.run(&["switch", "-q", "main"]);
    fixture.write("shared.txt", "uncommitted local edit\n");

    let preparation = fixture.prepare_switch("other");
    assert!(!preparation.can_apply);
    assert_eq!(preparation.dirty_paths, vec!["shared.txt".to_string()]);
    let refusal = preparation.refusal.expect("a reason");
    assert!(refusal.contains("shared.txt"), "{refusal}");
    assert!(refusal.contains("uncommitted changes"), "{refusal}");

    fixture.run(&["checkout", "--", "shared.txt"]);
    let clean = fixture.prepare_switch("other");
    assert!(clean.can_apply, "{:?}", clean.refusal);

    // A record prepared against a clean tree is checked again at apply.
    fixture.write("shared.txt", "uncommitted after review\n");
    let refused = fixture
        .engine
        .branch
        .apply(&clean.prepare_id, &clean.mutation, "tester", false)
        .unwrap_err();
    assert_eq!(refused.code(), "branch_dirty_switch");
    assert!(refused.message().contains("shared.txt"), "{refused:?}");
    assert_eq!(fixture.current_branch(), "main");
    fixture.cleanup();
}

#[test]
fn switching_never_overwrites_an_ignored_local_file() {
    let fixture = EngineFixture::new("ignored-switch");
    // The target branch tracks a file that the current branch ignores. A
    // working-tree copy is therefore invisible to `git status` and to the
    // dirty-path check, but Git would replace it on a plain switch.
    fixture.write(".gitignore", "private.settings\n");
    fixture.write("private.settings", "committed defaults\n");
    fixture.run(&["add", ".gitignore"]);
    fixture.run(&["add", "-f", "private.settings"]);
    fixture.run(&["commit", "-q", "-m", "track an ignored settings file"]);
    fixture.run(&["branch", "damaian/tracks-ignored"]);
    fixture.run(&["rm", "-q", "--cached", "private.settings"]);
    fixture.run(&["commit", "-q", "-m", "stop tracking the settings file"]);
    fixture.write("private.settings", "user local secret\n");
    assert_eq!(
        fixture.status(),
        "",
        "the ignored file must not appear as a working-tree change"
    );

    let preparation = fixture.prepare_switch("damaian/tracks-ignored");
    assert_eq!(preparation.dirty_paths, Vec::<String>::new());
    let refused = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .unwrap_err();
    assert!(
        refused.message().contains("would be overwritten"),
        "{refused:?}"
    );
    assert_eq!(
        fixture.read("private.settings"),
        "user local secret\n",
        "an ignored local file must never be silently replaced"
    );
    assert_eq!(fixture.current_branch(), "main");

    let record = fixture
        .engine
        .branch
        .load_record(&preparation.prepare_id)
        .unwrap();
    assert_eq!(record.state, "failed", "the switch did not run");
    assert!(
        record
            .git_commands
            .iter()
            .any(|command| has_token(command, "--no-overwrite-ignore")),
        "{:?}",
        record.git_commands
    );
    no_force_flag(&record.git_commands);
    fixture.cleanup();
}

#[test]
fn an_approval_cannot_be_retargeted_and_is_consumed_once() {
    let fixture = EngineFixture::new("consumed");
    let preparation = fixture.prepare_create("damaian/once", None);

    let mismatched = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &GitMutation::SwitchBranch {
                name: "damaian/once".to_string(),
            },
            "tester",
            false,
        )
        .unwrap_err();
    assert_eq!(mismatched.code(), "branch_mutation_mismatch");
    assert!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/damaian/once"])
            .status
            .code()
            != Some(0),
        "a mismatched approval must not run git"
    );

    fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .expect("the correct mutation applies");

    let second = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .unwrap_err();
    assert_eq!(second.code(), "branch_not_prepared");
    assert_eq!(
        fixture
            .run(&[
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/damaian/once"
            ])
            .lines()
            .count(),
        1
    );
    fixture.cleanup();
}

#[test]
fn a_record_that_is_applying_cannot_be_applied_again() {
    // `apply` writes `applying` before Git runs; a second request therefore
    // observes a state that is not `prepared` even while the first is still
    // running. Injecting the state proves the guard without a Git command that
    // can be paused.
    let fixture = EngineFixture::new("applying");
    let preparation = fixture.prepare_create("damaian/applying", None);
    let path = fixture
        .data_dir
        .join("branches")
        .join(format!("{}.json", preparation.prepare_id));
    let mut record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    record["state"] = serde_json::json!("applying");
    fs::write(&path, serde_json::to_string_pretty(&record).unwrap()).unwrap();

    let refused = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .unwrap_err();
    assert_eq!(refused.code(), "branch_not_prepared");
    assert!(
        refused.message().contains("no longer current"),
        "{refused:?}"
    );
    assert!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/damaian/applying"])
            .status
            .code()
            != Some(0),
        "an applying record must not run git"
    );
    fixture.cleanup();
}

#[test]
fn inspect_reports_a_dangling_record_truthfully() {
    let fixture = EngineFixture::new("inspect");
    fixture.commit("later.txt", "later\n", "later");
    let prepared_at = fixture.head();
    let preparation = fixture.prepare_create("damaian/inspect", None);

    let prepared = fixture
        .engine
        .branch
        .inspect(&preparation.prepare_id)
        .unwrap();
    assert_eq!(prepared.outcome, "not_landed");
    assert_eq!(prepared.state, "prepared");
    assert_eq!(prepared.actual_oid, None);

    // The crash case: the ref exists at the start commit the record resolved
    // but the record never reached `applied`. That is `landed`, not unknown.
    fixture.run(&["branch", "damaian/inspect", &prepared_at]);
    let landed_before_save = fixture
        .engine
        .branch
        .inspect(&preparation.prepare_id)
        .unwrap();
    assert_eq!(landed_before_save.outcome, "landed");
    assert_eq!(landed_before_save.state, "prepared");

    // Someone else created the same name at a different commit: report the
    // ref's real state rather than claiming success.
    fixture.run(&["branch", "-D", "damaian/inspect"]);
    let elsewhere = fixture.run(&["rev-parse", "HEAD^"]);
    fixture.run(&["branch", "damaian/inspect", &elsewhere]);
    let other = fixture
        .engine
        .branch
        .inspect(&preparation.prepare_id)
        .unwrap();
    assert_eq!(other.outcome, "other_ref_state");
    assert_eq!(other.actual_oid.as_deref(), Some(elsewhere.as_str()));

    fixture.run(&["branch", "-D", "damaian/inspect"]);
    fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .expect("the record was still prepared");
    let landed = fixture
        .engine
        .branch
        .inspect(&preparation.prepare_id)
        .unwrap();
    assert_eq!(landed.outcome, "landed");
    assert_eq!(landed.state, "applied");
    assert_eq!(landed.actual_oid.as_deref(), Some(prepared_at.as_str()));
    fixture.cleanup();
}

#[test]
fn a_branch_apply_is_bracketed_by_a_marker_pair_and_audited() {
    let fixture = EngineFixture::new("markers");
    let preparation = fixture.prepare_create("damaian/audited", None);
    fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .unwrap();

    let events = fixture.session_events();
    let started = events
        .iter()
        .filter(|event| {
            event["eventType"] == "action_started"
                && event["payload"]["action"] == "git_branch_create"
        })
        .collect::<Vec<_>>();
    let finished = events
        .iter()
        .filter(|event| {
            event["eventType"] == "action_finished"
                && event["payload"]["action"] == "git_branch_create"
        })
        .collect::<Vec<_>>();
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert_eq!(
        started[0]["payload"]["markerId"],
        finished[0]["payload"]["markerId"]
    );
    assert_eq!(started[0]["payload"]["sideEffecting"], true);
    assert_eq!(
        started[0]["payload"]["ref"],
        serde_json::json!(preparation.prepare_id)
    );

    let audit = fixture.audit_events();
    let created = audit
        .iter()
        .filter(|event| event["eventType"] == "git_branch_created")
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 1, "{created:?}");
    assert_eq!(created[0]["approvedBy"], "tester");
    assert_eq!(created[0]["branch"], "damaian/audited");
    assert_eq!(created[0]["outcome"], "ok");
    assert_eq!(
        created[0]["reference"],
        serde_json::json!("refs/heads/damaian/audited")
    );
    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Task 5: deletion, with the separate unmerged confirmation.
// ---------------------------------------------------------------------------

#[test]
fn a_merged_branch_deletes_with_d_and_no_force() {
    let fixture = EngineFixture::new("delete-merged");
    let head = fixture.head();
    fixture.run(&["branch", "damaian/done"]);

    let preparation = fixture.prepare_delete("damaian/done");
    assert!(preparation.can_apply, "{:?}", preparation.refusal);
    assert_eq!(
        preparation.mutation,
        GitMutation::DeleteBranch {
            name: "damaian/done".to_string(),
            unmerged_commits: Some(0),
        }
    );
    assert_eq!(preparation.unmerged_commits, Some(0));
    assert!(preparation.unmerged_subjects.is_empty());
    assert_eq!(preparation.start_point_sha.as_deref(), Some(head.as_str()));

    let outcome = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .expect("a merged branch deletes with a plain approval");

    assert_eq!(outcome.branch, "damaian/done");
    assert_eq!(outcome.created_ref, None);
    assert_eq!(outcome.head_after.as_deref(), Some(head.as_str()));
    assert_eq!(fixture.head(), head, "deleting a branch must not move HEAD");
    assert_eq!(fixture.current_branch(), "main");
    assert_ne!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/damaian/done"])
            .status
            .code(),
        Some(0),
        "the deleted ref must be absent"
    );

    no_force_flag(&outcome.git_commands);
    assert!(
        outcome
            .git_commands
            .iter()
            .any(|command| has_arguments(command, &["branch", "-d", "damaian/done"])),
        "the merged case must use -d: {:?}",
        outcome.git_commands
    );
    assert!(
        outcome
            .git_commands
            .iter()
            .all(|command| !has_token(command, "-D")),
        "the merged case must never use -D: {:?}",
        outcome.git_commands
    );

    let audit = fixture.audit_events();
    let deleted = audit
        .iter()
        .filter(|event| event["eventType"] == "git_branch_deleted")
        .collect::<Vec<_>>();
    assert_eq!(deleted.len(), 1, "{deleted:?}");
    assert_eq!(deleted[0]["approvedBy"], "tester");
    assert_eq!(deleted[0]["branch"], "damaian/done");
    assert_eq!(deleted[0]["reference"], "refs/heads/damaian/done");
    assert_eq!(deleted[0]["unmergedCommits"], "0");
    assert_eq!(deleted[0]["confirmedUnmerged"], "false");
    assert_eq!(deleted[0]["outcome"], "ok");

    let inspection = fixture
        .engine
        .branch
        .inspect(&preparation.prepare_id)
        .unwrap();
    assert_eq!(inspection.outcome, "landed");
    assert_eq!(inspection.state, "applied");
    assert_eq!(inspection.actual_oid, None);
    assert_eq!(inspection.expected_oid.as_deref(), Some(head.as_str()));
    fixture.cleanup();
}

#[test]
fn an_unmerged_branch_needs_a_distinct_confirmation() {
    let fixture = EngineFixture::new("delete-unmerged");
    fixture.run(&["switch", "-q", "-c", "damaian/lost"]);
    fixture.commit("lost.txt", "lost\n", "only on the lost branch");
    fixture.write("lost.txt", "lost again\n");
    fixture.run(&["add", "."]);
    fixture.run(&[
        "commit",
        "-q",
        "-m",
        "second lost subject",
        "-m",
        "body-only-text-that-must-not-be-audited",
    ]);
    let lost_sha = fixture.branch_sha("damaian/lost");
    fixture.run(&["switch", "-q", "main"]);

    let preparation = fixture.prepare_delete("damaian/lost");
    assert!(preparation.can_apply, "{:?}", preparation.refusal);
    assert_eq!(
        preparation.mutation,
        GitMutation::DeleteBranch {
            name: "damaian/lost".to_string(),
            unmerged_commits: Some(2),
        }
    );
    assert_eq!(preparation.unmerged_commits, Some(2));
    assert!(
        preparation
            .unmerged_subjects
            .contains(&"second lost subject".to_string()),
        "{:?}",
        preparation.unmerged_subjects
    );
    assert!(
        preparation
            .unmerged_subjects
            .contains(&"only on the lost branch".to_string()),
        "{:?}",
        preparation.unmerged_subjects
    );

    let refused = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .unwrap_err();
    assert_eq!(refused.code(), "branch_unmerged_confirmation_required");
    let message = refused.message();
    assert!(message.contains("2 commits"), "{message}");
    assert!(message.contains("second lost subject"), "{message}");
    assert!(message.contains("only on the lost branch"), "{message}");
    assert!(
        message.contains("plain approval is not enough"),
        "{message}"
    );
    assert_eq!(
        fixture.branch_sha("damaian/lost"),
        lost_sha,
        "a refused approval must not delete the branch"
    );
    let still_prepared = fixture
        .engine
        .branch
        .load_record(&preparation.prepare_id)
        .unwrap();
    assert_eq!(still_prepared.state, "prepared");
    assert!(still_prepared.git_commands.is_empty());
    assert!(
        fixture.session_events().iter().all(|event| {
            event["eventType"] != "action_started"
                || event["payload"]["action"] != "git_branch_delete"
        }),
        "a refused plain approval must not start the deletion action"
    );
    let audit_before = fixture.audit_events();
    assert!(
        audit_before
            .iter()
            .all(|event| event["eventType"] != "git_branch_deleted"),
        "nothing was deleted yet"
    );

    let outcome = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            true,
        )
        .expect("the separate confirmation deletes the branch");
    assert_eq!(outcome.branch, "damaian/lost");
    assert_ne!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/damaian/lost"])
            .status
            .code(),
        Some(0)
    );
    no_force_flag(&outcome.git_commands);
    assert!(
        outcome
            .git_commands
            .iter()
            .any(|command| has_arguments(command, &["branch", "-D", "damaian/lost"])),
        "the confirmed unmerged case must use -D: {:?}",
        outcome.git_commands
    );

    let audit = fixture.audit_events();
    let deleted = audit
        .iter()
        .filter(|event| event["eventType"] == "git_branch_deleted")
        .collect::<Vec<_>>();
    assert_eq!(deleted.len(), 1, "{deleted:?}");
    assert_eq!(deleted[0]["unmergedCommits"], "2");
    assert_eq!(deleted[0]["confirmedUnmerged"], "true");
    assert_eq!(deleted[0]["outcome"], "ok");
    let audit_text =
        fs::read_to_string(fixture.data_dir.join("audit").join("events.jsonl")).unwrap();
    assert!(
        !audit_text.contains("body-only-text-that-must-not-be-audited"),
        "the audit must never contain a commit body"
    );

    let inspection = fixture
        .engine
        .branch
        .inspect(&preparation.prepare_id)
        .unwrap();
    assert_eq!(inspection.outcome, "landed");
    assert_eq!(inspection.actual_oid, None);

    let consumed = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            true,
        )
        .unwrap_err();
    assert_eq!(consumed.code(), "branch_not_prepared");
    fixture.cleanup();
}

#[test]
fn a_branch_merged_into_main_but_not_into_head_needs_the_confirmation() {
    let fixture = EngineFixture::new("delete-not-merged-into-head");
    fixture.run(&["switch", "-q", "-c", "other"]);
    fixture.run(&["switch", "-q", "main"]);
    fixture.run(&["switch", "-q", "-c", "topic"]);
    fixture.commit("topic.txt", "topic\n", "topic work");
    fixture.run(&["switch", "-q", "main"]);
    fixture.run(&["merge", "-q", "--no-ff", "-m", "merge topic", "topic"]);
    fixture.run(&["switch", "-q", "other"]);

    // Damaian's default-branch count calls this merged: the only commit is on
    // main. Git's `branch -d`, however, checks the checked-out branch because
    // there is no upstream, and `other` does not contain it.
    assert_eq!(
        fixture
            .run(&[
                "rev-list",
                "--count",
                "refs/heads/topic",
                "--not",
                "--remotes",
                "refs/heads/main",
            ])
            .trim(),
        "0"
    );
    let git_refusal = fixture.run_failing(&["branch", "-d", "topic"]);
    assert!(git_refusal.contains("not fully merged"), "{git_refusal}");
    let topic_sha = fixture.branch_sha("topic");

    let preparation = fixture.prepare_delete("topic");
    assert!(preparation.can_apply, "{:?}", preparation.refusal);
    assert_eq!(preparation.unmerged_commits, Some(1));
    assert_eq!(
        preparation.unmerged_subjects,
        vec!["topic work".to_string()]
    );
    let reason = preparation.unmerged_reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("checked-out branch"),
        "the dialog must name what Git checks: {reason}"
    );

    let refused = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .unwrap_err();
    assert_eq!(refused.code(), "branch_unmerged_confirmation_required");
    let message = refused.message();
    assert!(message.contains("checked-out branch"), "{message}");
    assert!(message.contains("topic work"), "{message}");
    assert_eq!(
        fixture.branch_sha("topic"),
        topic_sha,
        "a refused approval must not delete the branch"
    );
    let record = fixture
        .engine
        .branch
        .load_record(&preparation.prepare_id)
        .unwrap();
    assert_eq!(record.state, "prepared");
    assert!(record.git_commands.is_empty());

    let outcome = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            true,
        )
        .expect("the separate confirmation deletes the branch");
    no_force_flag(&outcome.git_commands);
    assert!(
        outcome
            .git_commands
            .iter()
            .any(|command| has_arguments(command, &["branch", "-D", "topic"])),
        "{:?}",
        outcome.git_commands
    );
    assert_ne!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/topic"])
            .status
            .code(),
        Some(0)
    );

    let audit = fixture.audit_events();
    let deleted = audit
        .iter()
        .find(|event| event["eventType"] == "git_branch_deleted")
        .expect("a deletion audit event");
    assert_eq!(deleted["unmergedCommits"], "1");
    assert_eq!(deleted["confirmedUnmerged"], "true");
    fixture.cleanup();
}

#[test]
fn an_unknown_default_branch_still_requires_the_unmerged_confirmation() {
    let fixture = EngineFixture::new("delete-unknown-default");
    fixture.run(&["branch", "-m", "trunk"]);
    fixture.run(&["switch", "--orphan", "topic"]);
    let _ = fixture.try_run(&["rm", "-rf", "."]);
    fixture.write("orphan.txt", "orphan\n");
    fixture.run(&["add", "."]);
    fixture.run(&["commit", "-q", "-m", "orphan work"]);
    fixture.run(&["switch", "-q", "trunk"]);

    let preparation = fixture.prepare_delete("topic");
    assert!(preparation.can_apply, "{:?}", preparation.refusal);
    assert_eq!(
        preparation.mutation,
        GitMutation::DeleteBranch {
            name: "topic".to_string(),
            unmerged_commits: None,
        }
    );
    assert_eq!(preparation.unmerged_commits, None);
    assert!(
        preparation
            .unmerged_subjects
            .contains(&"orphan work".to_string()),
        "{:?}",
        preparation.unmerged_subjects
    );

    let refused = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .unwrap_err();
    assert_eq!(refused.code(), "branch_unmerged_confirmation_required");
    let message = refused.message();
    assert!(
        message.contains("could not determine the default branch"),
        "{message}"
    );
    assert!(message.contains("orphan work"), "{message}");
    assert_eq!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/topic"])
            .status
            .code(),
        Some(0)
    );

    let outcome = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            true,
        )
        .unwrap();
    no_force_flag(&outcome.git_commands);
    assert!(
        outcome
            .git_commands
            .iter()
            .any(|command| has_arguments(command, &["branch", "-D", "topic"])),
        "{:?}",
        outcome.git_commands
    );
    assert_ne!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/topic"])
            .status
            .code(),
        Some(0)
    );
    let audit = fixture.audit_events();
    let deleted = audit
        .iter()
        .find(|event| event["eventType"] == "git_branch_deleted")
        .expect("a deletion audit event");
    assert_eq!(deleted["unmergedCommits"], "unknown");
    assert_eq!(deleted["confirmedUnmerged"], "true");
    fixture.cleanup();
}

#[test]
fn the_current_branch_and_a_checked_out_branch_cannot_be_deleted() {
    let fixture = EngineFixture::new("delete-checkouts");
    fixture.run(&["branch", "damaian/linked"]);

    let current = fixture.prepare_delete("main");
    assert!(!current.can_apply);
    assert!(
        current
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("current branch")),
        "{:?}",
        current.refusal
    );
    assert!(
        fixture
            .engine
            .branch
            .load_record(&current.prepare_id)
            .is_err()
    );

    let prepared = fixture.prepare_delete("damaian/linked");
    assert!(prepared.can_apply, "{:?}", prepared.refusal);

    let worktree = fixture.base.join("linked-worktree");
    let worktree_text = worktree.to_str().unwrap();
    fixture.run(&["worktree", "add", worktree_text, "damaian/linked"]);

    let at_apply = fixture
        .engine
        .branch
        .apply(&prepared.prepare_id, &prepared.mutation, "tester", false)
        .unwrap_err();
    assert_eq!(at_apply.code(), "branch_delete_checked_out");
    assert!(at_apply.message().contains(worktree_text), "{at_apply:?}");
    assert_eq!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/damaian/linked"])
            .status
            .code(),
        Some(0),
        "a checked-out branch must survive"
    );

    let at_prepare = fixture.prepare_delete("damaian/linked");
    assert!(!at_prepare.can_apply);
    assert!(
        at_prepare
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("checked out in the worktree")),
        "{:?}",
        at_prepare.refusal
    );

    fixture.run(&["worktree", "remove", worktree_text]);
    fixture
        .engine
        .branch
        .apply(&prepared.prepare_id, &prepared.mutation, "tester", false)
        .expect("the refusal did not consume the record and the branch is free");
    assert_ne!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/damaian/linked"])
            .status
            .code(),
        Some(0)
    );
    fixture.cleanup();
}

#[test]
fn a_missing_branch_and_a_policy_refusal_are_reported_before_git() {
    let mut fixture = EngineFixture::new("delete-refusals");
    fixture.run(&["branch", "damaian/policy"]);

    let missing = fixture.prepare_delete("damaian/missing");
    assert!(!missing.can_apply);
    assert!(
        missing
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("no local branch")),
        "{:?}",
        missing.refusal
    );
    assert!(
        fixture
            .engine
            .branch
            .load_record(&missing.prepare_id)
            .is_err()
    );

    fixture.commit("conflict.txt", "a\nb\nc\n", "base");
    fixture.run(&["switch", "-q", "-c", "other"]);
    fixture.commit("conflict.txt", "a\nX\nc\n", "other");
    fixture.run(&["switch", "-q", "main"]);
    fixture.commit("conflict.txt", "a\nY\nc\n", "main");
    fixture.run_failing(&["merge", "other"]);
    let mid_merge = fixture.prepare_delete("other");
    assert!(!mid_merge.can_apply);
    assert!(
        mid_merge
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("in progress")),
        "{:?}",
        mid_merge.refusal
    );
    fixture.run(&["merge", "--abort"]);

    let session_id = fixture.session_id.clone();
    for mode in [SessionMode::Ask, SessionMode::Plan, SessionMode::Review] {
        fixture
            .engine
            .session_store
            .set_session_mode(&session_id, mode, "user")
            .unwrap();
        let refused = fixture.prepare_delete("damaian/policy");
        assert!(!refused.can_apply);
        assert!(
            refused
                .refusal
                .as_deref()
                .is_some_and(|refusal| refusal.contains("Code mode")),
            "{:?}",
            refused.refusal
        );
    }
    fixture
        .engine
        .session_store
        .set_session_mode(&session_id, SessionMode::Code, "user")
        .unwrap();
    fixture.rebuild(|config| config.allow_git_branches = false);
    let denied = fixture.prepare_delete("damaian/policy");
    assert!(!denied.can_apply);
    assert!(
        denied
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("allow_git_branches")),
        "{:?}",
        denied.refusal
    );
    fixture.rebuild(|_| {});

    let prepared = fixture.prepare_delete("damaian/policy");
    assert!(prepared.can_apply, "{:?}", prepared.refusal);
    fixture
        .engine
        .session_store
        .set_session_mode(&session_id, SessionMode::Ask, "user")
        .unwrap();
    let at_apply = fixture
        .engine
        .branch
        .apply(&prepared.prepare_id, &prepared.mutation, "tester", false)
        .unwrap_err();
    assert_eq!(at_apply.code(), "branch_not_allowed");
    assert_eq!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/damaian/policy"])
            .status
            .code(),
        Some(0)
    );

    fixture
        .engine
        .session_store
        .set_session_mode(&session_id, SessionMode::Code, "user")
        .unwrap();
    fixture
        .engine
        .branch
        .apply(&prepared.prepare_id, &prepared.mutation, "tester", false)
        .expect("a policy refusal does not consume the prepared record");
    assert_ne!(
        fixture
            .try_run(&["show-ref", "--verify", "refs/heads/damaian/policy"])
            .status
            .code(),
        Some(0)
    );
    fixture.cleanup();
}

#[test]
fn a_delete_that_already_landed_is_inspected_not_retried() {
    let fixture = EngineFixture::new("delete-inspect");
    fixture.run(&["branch", "damaian/done"]);
    let preparation = fixture.prepare_delete("damaian/done");

    let prepared = fixture
        .engine
        .branch
        .inspect(&preparation.prepare_id)
        .unwrap();
    assert_eq!(prepared.state, "prepared");
    assert_eq!(prepared.outcome, "not_landed");
    assert!(
        prepared.message.contains("still at"),
        "{}",
        prepared.message
    );

    // The crash-after-delete case: the ref is gone but the record never reached
    // `applied`. That is `landed`, and a later apply must not retry it.
    fixture.run(&["branch", "-d", "damaian/done"]);
    let landed = fixture
        .engine
        .branch
        .inspect(&preparation.prepare_id)
        .unwrap();
    assert_eq!(landed.state, "prepared");
    assert_eq!(landed.outcome, "landed");
    assert_eq!(landed.actual_oid, None);
    assert!(
        landed.message.contains("deletion landed"),
        "{}",
        landed.message
    );

    let retried = fixture
        .engine
        .branch
        .apply(
            &preparation.prepare_id,
            &preparation.mutation,
            "tester",
            false,
        )
        .unwrap_err();
    assert_eq!(retried.code(), "branch_state_changed");
    assert!(
        retried.message().contains("no longer exists"),
        "{retried:?}"
    );
    let record = fixture
        .engine
        .branch
        .load_record(&preparation.prepare_id)
        .unwrap();
    assert_eq!(record.state, "prepared");
    assert!(record.git_commands.is_empty(), "nothing may run again");
    let audit = fixture.audit_events();
    assert!(
        audit
            .iter()
            .all(|event| event["eventType"] != "git_branch_deleted"),
        "a delete that already landed must not be retried"
    );
    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Task 6: merge and rebase preparation, which must execute neither.
// ---------------------------------------------------------------------------

#[test]
fn a_clean_merge_is_planned_without_merging() {
    let fixture = EngineFixture::new("merge-clean");
    fixture.run(&["switch", "-q", "-c", "feature"]);
    fixture.write("feature.txt", "feature\n");
    fixture.run(&["add", "."]);
    fixture.run(&[
        "commit",
        "-q",
        "-m",
        "feature change",
        "-m",
        "body-only-text-that-must-not-be-audited",
    ]);
    fixture.run(&["switch", "-q", "main"]);
    fixture.commit("main.txt", "main\n", "main change");
    let fork = fixture.run(&["merge-base", "main", "feature"]);
    let before = fixture.read_only_snapshot();

    let plan = fixture.prepare_merge("feature");
    assert_eq!(plan.problem, None);
    assert!(
        plan.conflict_prediction_available,
        "{:?}",
        plan.conflict_prediction_unavailable_reason
    );
    assert_eq!(plan.merge_tree_exit_status, Some(0));
    assert_eq!(plan.merge_base.as_deref(), Some(fork.as_str()));
    assert_eq!(plan.head_oid.as_deref(), Some(fixture.head().as_str()));
    assert!(!plan.fast_forward, "main and feature diverged");
    assert!(
        plan.conflicted_paths.is_empty(),
        "{:?}",
        plan.conflicted_paths
    );
    assert_eq!(plan.changed_files, vec!["feature.txt".to_string()]);
    assert_eq!(
        plan.incoming_commits.len(),
        1,
        "{:?}",
        plan.incoming_commits
    );
    assert_eq!((plan.ahead, plan.behind), (1, 1), "{plan:?}");
    assert!(
        plan.incoming_commits[0].ends_with("feature change"),
        "{:?}",
        plan.incoming_commits
    );

    assert_eq!(fixture.read_only_snapshot(), before);
    assert!(!fixture.repo.join(".git").join("MERGE_HEAD").exists());

    let audit = fixture.audit_events();
    let prepared = audit
        .iter()
        .find(|event| event["eventType"] == "git_merge_prepared")
        .expect("a merge preparation audit event");
    assert_eq!(prepared["base"], "feature");
    assert_eq!(prepared["mergeBase"], fork);
    assert_eq!(prepared["incomingCommits"], "1");
    assert_eq!(prepared["behind"], "1");
    assert_eq!(prepared["changedFiles"], "1");
    assert_eq!(prepared["conflictedPaths"], "0");
    assert_eq!(prepared["conflictPredictionAvailable"], "true");
    assert_eq!(prepared["outcome"], "prepared");
    let audit_text =
        fs::read_to_string(fixture.data_dir.join("audit").join("events.jsonl")).unwrap();
    assert!(
        !audit_text.contains("body-only-text-that-must-not-be-audited"),
        "the audit must never contain a commit body"
    );
    fixture.cleanup();
}

#[test]
fn an_overlapping_merge_is_reported_as_a_conflict_without_merging() {
    let fixture = EngineFixture::new("merge-conflict");
    fixture.commit("conflict.txt", "a\nb\nc\n", "base");
    fixture.run(&["switch", "-q", "-c", "feature"]);
    fixture.commit("conflict.txt", "a\nX\nc\n", "feature side");
    fixture.run(&["switch", "-q", "main"]);
    fixture.commit("conflict.txt", "a\nY\nc\n", "main side");
    let before = fixture.read_only_snapshot();

    let plan = fixture.prepare_merge("feature");
    assert_eq!(plan.problem, None);
    assert!(plan.conflict_prediction_available, "{plan:?}");
    assert_eq!(plan.merge_tree_exit_status, Some(1));
    assert_eq!(plan.conflicted_paths, vec!["conflict.txt".to_string()]);
    assert!(!plan.fast_forward);
    assert_eq!((plan.ahead, plan.behind), (1, 1), "{plan:?}");
    assert!(
        plan.changed_files.contains(&"conflict.txt".to_string()),
        "{:?}",
        plan.changed_files
    );

    assert_eq!(fixture.read_only_snapshot(), before);
    assert!(
        !fixture.repo.join(".git").join("MERGE_HEAD").exists(),
        "a real merge would have created MERGE_HEAD"
    );

    let audit = fixture.audit_events();
    let prepared = audit
        .iter()
        .find(|event| event["eventType"] == "git_merge_prepared")
        .expect("a merge preparation audit event");
    assert_eq!(prepared["conflictedPaths"], "1");
    assert_eq!(prepared["conflictPredictionAvailable"], "true");
    fixture.cleanup();
}

#[test]
fn a_fast_forward_merge_is_reported_as_fast_forward() {
    let fixture = EngineFixture::new("merge-fast-forward");
    fixture.run(&["switch", "-q", "-c", "feature"]);
    fixture.commit("feature.txt", "feature\n", "feature change");
    fixture.run(&["switch", "-q", "main"]);
    let before = fixture.read_only_snapshot();

    let plan = fixture.prepare_merge("feature");
    assert_eq!(plan.problem, None);
    assert!(plan.conflict_prediction_available, "{plan:?}");
    assert!(plan.fast_forward, "{plan:?}");
    assert_eq!(plan.merge_tree_exit_status, Some(0));
    assert_eq!(plan.merge_base.as_deref(), Some(fixture.head().as_str()));
    assert_eq!(plan.changed_files, vec!["feature.txt".to_string()]);
    assert_eq!((plan.ahead, plan.behind), (0, 1), "{plan:?}");

    assert_eq!(fixture.read_only_snapshot(), before);
    assert!(!fixture.repo.join(".git").join("MERGE_HEAD").exists());
    fixture.cleanup();
}

#[test]
fn an_unrelated_or_unresolvable_base_reports_prediction_unavailable() {
    let fixture = EngineFixture::new("merge-unavailable");
    fixture.run(&["switch", "-q", "--orphan", "unrelated"]);
    let _ = fixture.try_run(&["rm", "-rf", "."]);
    fixture.write("unrelated.txt", "unrelated\n");
    fixture.run(&["add", "."]);
    fixture.run(&["commit", "-q", "-m", "unrelated root"]);
    fixture.run(&["switch", "-q", "main"]);
    let before = fixture.read_only_snapshot();

    for base in ["unrelated", "damaian/does-not-exist"] {
        let plan = fixture.prepare_merge(base);
        assert!(
            !plan.conflict_prediction_available,
            "{base}: prediction must be unavailable"
        );
        assert!(plan.problem.is_some(), "{base}: {:?}", plan.problem);
        assert_eq!(plan.merge_base, None, "{base}");
        assert!(
            plan.merge_tree_exit_status.is_none(),
            "{base}: merge-tree must not run without a merge base"
        );
        assert_eq!((plan.ahead, plan.behind), (0, 0), "{base}");
        assert!(plan.conflicted_paths.is_empty(), "{base}");
        assert!(plan.changed_files.is_empty(), "{base}");
        assert_eq!(fixture.read_only_snapshot(), before, "{base}");
        assert!(!fixture.repo.join(".git").join("MERGE_HEAD").exists());
    }

    // An option-shaped base is refused before Git sees it, exactly as the
    // merge-base read does.
    let refused = fixture
        .engine
        .branch
        .prepare_merge(&fixture.repo, "--output=/tmp/x", &fixture.session_id)
        .unwrap_err();
    assert!(
        matches!(refused, ClientError::InvalidInput(ref message) if message.contains("not a branch name")),
        "{refused:?}"
    );

    let unavailable = fixture
        .audit_events()
        .into_iter()
        .filter(|event| event["eventType"] == "git_merge_prepared")
        .collect::<Vec<_>>();
    assert_eq!(unavailable.len(), 2, "{unavailable:?}");
    assert!(
        unavailable
            .iter()
            .all(|event| event["conflictPredictionAvailable"] == "false"),
        "{unavailable:?}"
    );
    fixture.cleanup();
}

#[test]
fn a_rebase_plan_lists_the_replayed_commits_and_says_prediction_is_unavailable() {
    let fixture = EngineFixture::new("rebase-plan");
    let fork = fixture.head();
    fixture.run(&["switch", "-q", "-c", "topic"]);
    fixture.commit("one.txt", "one\n", "topic one");
    fixture.commit("two.txt", "two\n", "topic two");
    let head = fixture.head();
    let before = fixture.read_only_snapshot();

    let plan = fixture.prepare_rebase("main");
    assert_eq!(plan.problem, None);
    assert_eq!(plan.onto, "main");
    assert_eq!(plan.onto_oid.as_deref(), Some(fork.as_str()));
    assert_eq!(plan.merge_base.as_deref(), Some(fork.as_str()));
    assert_eq!(plan.head_oid.as_deref(), Some(head.as_str()));
    assert_eq!(
        plan.commits_to_replay.len(),
        2,
        "{:?}",
        plan.commits_to_replay
    );
    assert!(
        plan.commits_to_replay
            .iter()
            .any(|commit| commit.ends_with("topic one")),
        "{:?}",
        plan.commits_to_replay
    );
    assert!(
        plan.commits_to_replay
            .iter()
            .any(|commit| commit.ends_with("topic two")),
        "{:?}",
        plan.commits_to_replay
    );
    assert!(!plan.conflict_prediction_available);
    assert!(
        plan.conflict_prediction_unavailable_reason
            .to_lowercase()
            .contains("replaying"),
        "{}",
        plan.conflict_prediction_unavailable_reason
    );
    assert!(
        plan.conflict_prediction_unavailable_reason
            .contains("Nothing was replayed"),
        "{}",
        plan.conflict_prediction_unavailable_reason
    );

    assert_eq!(fixture.read_only_snapshot(), before);
    assert!(!fixture.repo.join(".git").join("MERGE_HEAD").exists());
    assert!(!fixture.repo.join(".git").join("rebase-merge").exists());

    let audit = fixture.audit_events();
    let prepared = audit
        .iter()
        .find(|event| event["eventType"] == "git_rebase_prepared")
        .expect("a rebase preparation audit event");
    assert_eq!(prepared["onto"], "main");
    assert_eq!(prepared["commitsToReplay"], "2");
    assert_eq!(prepared["conflictPredictionAvailable"], "false");
    assert_eq!(prepared["outcome"], "prepared");
    let audit_text =
        fs::read_to_string(fixture.data_dir.join("audit").join("events.jsonl")).unwrap();
    assert!(
        !audit_text.contains("topic one") && !audit_text.contains("topic two"),
        "the audit must not repeat commit subjects"
    );
    fixture.cleanup();
}

#[test]
fn commit_subjects_are_redacted_before_the_dialog_sees_them() {
    let fixture = EngineFixture::new("subject-redaction");
    fixture.run(&["switch", "-q", "-c", "leaky"]);
    fixture.commit(
        "leaky.txt",
        "leaky\n",
        "rotate AKIAIOSFODNN7EXAMPLE before release",
    );
    fixture.run(&["switch", "-q", "main"]);

    let plan = fixture.prepare_merge("leaky");
    assert_eq!(plan.incoming_commits.len(), 1, "{plan:?}");
    assert!(
        !plan.incoming_commits[0].contains("AKIAIOSFODNN7EXAMPLE"),
        "{:?}",
        plan.incoming_commits
    );
    assert!(
        plan.incoming_commits[0].contains("[REDACTED_AWS_ACCESS_KEY_"),
        "{:?}",
        plan.incoming_commits
    );

    let preparation = fixture.prepare_delete("leaky");
    assert_eq!(preparation.unmerged_commits, Some(1));
    assert!(
        !preparation
            .unmerged_subjects
            .iter()
            .any(|subject| subject.contains("AKIAIOSFODNN7EXAMPLE")),
        "{:?}",
        preparation.unmerged_subjects
    );
    assert!(
        preparation
            .unmerged_subjects
            .iter()
            .any(|subject| subject.contains("[REDACTED_AWS_ACCESS_KEY_")),
        "{:?}",
        preparation.unmerged_subjects
    );

    fixture.run(&["switch", "-q", "leaky"]);
    let rebase = fixture.prepare_rebase("main");
    assert!(
        !rebase
            .commits_to_replay
            .iter()
            .any(|commit| commit.contains("AKIAIOSFODNN7EXAMPLE")),
        "{:?}",
        rebase.commits_to_replay
    );
    assert_eq!(rebase.commits_to_replay.len(), 1, "{rebase:?}");

    let audit_text =
        fs::read_to_string(fixture.data_dir.join("audit").join("events.jsonl")).unwrap();
    assert!(
        !audit_text.contains("AKIAIOSFODNN7EXAMPLE"),
        "the audit must never carry the unredacted subject"
    );
    fixture.cleanup();
}
