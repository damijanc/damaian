//! Spec 47, requirements 1–4: the agent working floor.
//!
//! Every acceptance criterion in `docs/specs/47_agent_working_capability/proposal.md`
//! §6's first-slice group lives here.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::file_access::{FileAccessController, LineRange, ReadWindow};
use workspace_engine::navigation::NavigationController;
use workspace_engine::tree_walk::{self, WalkEvent};
use workspace_engine::{AuditLog, ClientError, Config, PathPolicy, SecretScanner};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-agent-tools-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn write_fixture(root: &Path, relative_path: &str, content: &str) {
    let path = root.join(relative_path);
    fs::create_dir_all(path.parent().expect("fixture should have parent")).unwrap();
    fs::write(path, content).unwrap();
}

/// `enable_index_watcher: false` because every test here builds a throwaway
/// repository, and registering an FSEvents watcher costs ten to fifteen seconds
/// for freshness none of them use. `AGENTS.md` states this as a rule.
fn test_config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    }
}

fn test_audit(repo: &Path, scanner: SecretScanner) -> AuditLog {
    AuditLog::new(repo.join(".damaian"), true, scanner)
}

fn file_access_for(repo: &Path, config: &Config) -> FileAccessController {
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    FileAccessController::new(
        config.clone(),
        test_audit(repo, scanner.clone()),
        scanner,
        PathPolicy::new(config),
    )
}

fn navigation_for(repo: &Path, config: &Config) -> NavigationController {
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    NavigationController::new(
        config.clone(),
        test_audit(repo, scanner.clone()),
        scanner,
        PathPolicy::new(config),
    )
}

fn numbered_lines(count: usize) -> String {
    (1..=count).map(|n| format!("line {n}\n")).collect()
}

/// The symlink-escape check is the reason the walk is shared rather than
/// reimplemented: a symlink that canonicalizes outside the root would otherwise
/// let a navigation tool read anything the user can.
#[test]
fn the_walk_rejects_a_symlink_that_escapes_the_root() {
    let repo = temp_dir("walk-symlink");
    write_fixture(&repo, "src/lib.rs", "pub fn a() {}\n");
    let outside = temp_dir("walk-outside");
    write_fixture(&outside, "secret.txt", "s3cret\n");
    std::os::unix::fs::symlink(&outside, repo.join("src/escape")).unwrap();

    let mut seen = Vec::new();
    let mut skipped = Vec::new();
    tree_walk::walk(&repo, &repo, "", &[], &mut |event| {
        match event {
            WalkEvent::File(file) => seen.push(file.relative_path.clone()),
            WalkEvent::Skipped(skip) => skipped.push((skip.path.clone(), skip.reason.clone())),
        }
        Ok(())
    })
    .unwrap();

    assert!(seen.contains(&"src/lib.rs".to_string()), "got {seen:?}");
    assert!(
        !seen.iter().any(|path| path.contains("escape")),
        "a symlink resolving outside the root must not be walked: {seen:?}"
    );
    assert_eq!(
        skipped,
        vec![("src/escape".to_string(), "symlink_outside_root".to_string())]
    );
}

// ---------------------------------------------------------------------------
// Task 2 · Ranged reads (requirement 1)
// ---------------------------------------------------------------------------

/// An unranged read of a large file must not spend a turn's whole context
/// budget: `tests/foundation.rs` is 4420 lines and the default budget is 16k
/// tokens. It returns a bounded window and says what it cut.
#[test]
fn an_unranged_read_caps_lines_and_says_what_it_cut() {
    let repo = temp_dir("read-cap");
    write_fixture(&repo, "big.rs", &numbered_lines(1000));
    let config = Config {
        max_read_lines: 400,
        ..test_config(&repo)
    };
    let access = file_access_for(&repo, &config);

    let read = access
        .read_file(
            &repo,
            "big.rs",
            None,
            None,
            false,
            false,
            ReadWindow::Default,
        )
        .unwrap();

    assert_eq!(read.total_lines, 1000);
    assert_eq!(read.line_range, LineRange { start: 1, end: 400 });
    assert_eq!(read.truncated_by, Some("lines".to_string()));
    assert!(read.content.starts_with("line 1\n"));
    assert!(read.content.trim_end().ends_with("line 400"));
    assert!(!read.content.contains("line 401"));
}

#[test]
fn a_ranged_read_returns_only_that_range() {
    let repo = temp_dir("read-range");
    write_fixture(&repo, "big.rs", &numbered_lines(1000));
    let config = test_config(&repo);
    let access = file_access_for(&repo, &config);

    let read = access
        .read_file(
            &repo,
            "big.rs",
            None,
            None,
            false,
            false,
            ReadWindow::Range(LineRange {
                start: 120,
                end: 180,
            }),
        )
        .unwrap();

    assert_eq!(
        read.line_range,
        LineRange {
            start: 120,
            end: 180
        }
    );
    assert_eq!(read.total_lines, 1000);
    assert_eq!(read.truncated_by, None);
    assert!(read.content.starts_with("line 120\n"));
    assert!(read.content.trim_end().ends_with("line 180"));
}

/// Proposal §5.2: `max_file_bytes` caps what is *returned*, not what may be
/// *inspected*. Before this spec the same read was refused outright, which is
/// what made an oversized file unreadable rather than merely expensive.
#[test]
fn a_file_over_the_byte_limit_is_readable_by_range() {
    let repo = temp_dir("read-oversize");
    write_fixture(&repo, "huge.rs", &numbered_lines(50_000));
    let config = Config {
        max_file_bytes: 1024,
        ..test_config(&repo)
    };
    let access = file_access_for(&repo, &config);

    let read = access
        .read_file(
            &repo,
            "huge.rs",
            None,
            None,
            false,
            false,
            ReadWindow::Range(LineRange { start: 10, end: 12 }),
        )
        .unwrap();

    assert_eq!(read.total_lines, 50_000);
    assert_eq!(read.content, "line 10\nline 11\nline 12\n");
}

/// The byte cap sits beside the line cap rather than under it: 400 lines of a
/// minified or generated file can exceed any budget a line count implies.
#[test]
fn a_few_enormous_lines_are_cut_by_bytes_and_say_so() {
    let repo = temp_dir("read-bytes");
    let body = (1..=10)
        .map(|_| format!("{}\n", "x".repeat(10_000)))
        .collect::<String>();
    write_fixture(&repo, "minified.js", &body);
    let config = Config {
        max_file_bytes: 5_000,
        max_read_lines: 400,
        ..test_config(&repo)
    };
    let access = file_access_for(&repo, &config);

    let read = access
        .read_file(
            &repo,
            "minified.js",
            None,
            None,
            false,
            false,
            ReadWindow::Default,
        )
        .unwrap();

    assert_eq!(read.truncated_by, Some("bytes".to_string()));
    assert!(
        read.content.len() <= 5_000,
        "returned {} bytes against a 5000-byte cap",
        read.content.len()
    );
    assert_eq!(read.total_lines, 10);
}

// ---------------------------------------------------------------------------
// Task 3 · list_directory (requirements 2 and 3)
// ---------------------------------------------------------------------------

#[test]
fn listing_respects_gitignore_and_caps_with_a_notice() {
    let repo = temp_dir("list-basic");
    for n in 1..=250 {
        write_fixture(&repo, &format!("src/m{n}.rs"), "pub fn a() {}\n");
    }
    // A per-directory `.gitignore`, not a default pattern: this is what proves
    // the listing inherits the walk's ignore handling rather than its own.
    write_fixture(&repo, ".gitignore", "generated/\n");
    write_fixture(&repo, "generated/derived.rs", "pub fn generated() {}\n");
    let config = Config {
        max_list_entries: 200,
        ..test_config(&repo)
    };
    let nav = navigation_for(&repo, &config);

    let listing = nav.list_directory(&repo, None, None, None, None).unwrap();

    assert_eq!(listing.paths.len(), 200);
    assert_eq!(listing.total_found, 250);
    assert!(listing.truncated);
    assert!(
        !listing.paths.iter().any(|p| p.starts_with("generated/")),
        "a .gitignore'd directory must not be listed: {:?}",
        listing.paths
    );
}

/// Requirement 2: resolved through `PathPolicy` on the same path
/// `FileAccessController::read_file` uses. `.env` is in
/// `DEFAULT_RESTRICTED_PATTERNS` and deliberately *not* in
/// `DEFAULT_IGNORE_PATTERNS`, so this tests the restriction rather than the
/// ignore rules — the trap spec 18 Task 10 recorded.
#[test]
fn listing_cannot_reach_a_restricted_path() {
    let repo = temp_dir("list-restricted");
    write_fixture(&repo, "src/lib.rs", "pub fn a() {}\n");
    write_fixture(&repo, ".env", "API_KEY=sk_live_0123456789abcdef\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let listing = nav.list_directory(&repo, None, None, None, None).unwrap();

    assert!(
        listing.paths.iter().any(|p| p == "src/lib.rs"),
        "the readable file should still be listed: {:?}",
        listing.paths
    );
    assert!(
        !listing.paths.iter().any(|p| p == ".env"),
        "a restricted path must not be revealed by a listing: {:?}",
        listing.paths
    );
}

#[test]
fn listing_outside_the_repository_is_denied() {
    let repo = temp_dir("list-escape");
    write_fixture(&repo, "src/lib.rs", "pub fn a() {}\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let error = nav
        .list_directory(&repo, Some("../.."), None, None, None)
        .unwrap_err();

    assert!(
        matches!(error, ClientError::AccessDenied(_)),
        "got {error:?}"
    );
}
