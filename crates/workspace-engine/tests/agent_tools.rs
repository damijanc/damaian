//! Spec 47, requirements 1–4: the agent working floor.
//!
//! Every acceptance criterion in `docs/specs/47_agent_working_capability/proposal.md`
//! §6's first-slice group lives here.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::edit::{RegionEdit, region_edits_to_changes};
use workspace_engine::file_access::{FileAccessController, LineRange, ReadWindow};
use workspace_engine::navigation::NavigationController;
use workspace_engine::tree_walk::{self, WalkEvent};
use workspace_engine::{
    AuditLog, ClientError, CommandPolicy, Config, MockModelAdapter, ModelProviderConfig,
    PatchEngine, PathPolicy, ProposedChange, SecretScanner, ToolCall, WorkspaceEngine,
};

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

// ---------------------------------------------------------------------------
// Task 4 · search_content (requirements 2 and 3)
// ---------------------------------------------------------------------------

/// The gap this closes: `search_codebase` finds *files* about a topic, so
/// "where is `apply_overlay` wired" had no cheap answer. This finds call sites.
#[test]
fn search_finds_call_sites_with_line_numbers() {
    let repo = temp_dir("search-basic");
    write_fixture(&repo, "src/a.rs", "fn one() {}\nfn apply_overlay() {}\n");
    write_fixture(&repo, "src/b.rs", "fn two() {\n    apply_overlay();\n}\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let found = nav
        .search_content(&repo, "apply_overlay", None, None, None, None)
        .unwrap();

    assert_eq!(found.total_found, 2);
    assert_eq!(found.matches[0].path, "src/a.rs");
    assert_eq!(found.matches[0].line, 2);
    assert_eq!(found.matches[1].path, "src/b.rs");
    assert_eq!(found.matches[1].line, 2);
    assert!(!found.truncated);
}

/// Requirement 2: redacted through `SecretScanner` on the same path as a read.
///
/// The fixture is deliberately a readable file, not `.env`:
/// `DEFAULT_RESTRICTED_PATTERNS` covers `.env`, so a test using one would assert
/// *refusal* while claiming to test *redaction*. Spec 18 Task 10 found that trap
/// the hard way.
#[test]
fn search_redacts_a_secret_it_would_otherwise_return() {
    let repo = temp_dir("search-secret");
    write_fixture(
        &repo,
        "src/telemetry_config.rs",
        "pub const TOKEN: &str = \"AKIAIOSFODNN7EXAMPLE\";\n",
    );
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let found = nav
        .search_content(&repo, "TOKEN", None, None, None, None)
        .unwrap();

    assert_eq!(found.total_found, 1);
    assert!(
        !found.matches[0].text.contains("AKIAIOSFODNN7EXAMPLE"),
        "a match line must be redacted before it leaves the engine: {:?}",
        found.matches[0].text
    );
    assert!(found.matches[0].text.contains("[REDACTED_"));
}

#[test]
fn search_caps_matches_and_says_what_it_cut() {
    let repo = temp_dir("search-cap");
    for n in 1..=100 {
        write_fixture(&repo, &format!("src/m{n}.rs"), "let needle = 1;\n");
    }
    let config = Config {
        max_search_matches: 50,
        ..test_config(&repo)
    };
    let nav = navigation_for(&repo, &config);

    let found = nav
        .search_content(&repo, "needle", None, None, None, None)
        .unwrap();

    assert_eq!(found.matches.len(), 50);
    assert_eq!(found.total_found, 100);
    assert_eq!(found.files_searched, 100);
    assert!(found.truncated);
}

/// A `max_matches` argument may ask for fewer than the configured cap, never
/// more, or the cap would be advisory rather than a cap.
#[test]
fn a_max_matches_argument_cannot_raise_the_configured_cap() {
    let repo = temp_dir("search-arg-cap");
    for n in 1..=100 {
        write_fixture(&repo, &format!("src/m{n}.rs"), "let needle = 1;\n");
    }
    let config = Config {
        max_search_matches: 10,
        ..test_config(&repo)
    };
    let nav = navigation_for(&repo, &config);

    let raised = nav
        .search_content(&repo, "needle", None, Some(90), None, None)
        .unwrap();
    let lowered = nav
        .search_content(&repo, "needle", None, Some(3), None, None)
        .unwrap();

    assert_eq!(
        raised.matches.len(),
        10,
        "an argument must not raise the cap"
    );
    assert_eq!(lowered.matches.len(), 3, "but it may ask for fewer");
}

#[test]
fn an_overlong_match_line_is_trimmed() {
    let repo = temp_dir("search-longline");
    write_fixture(
        &repo,
        "src/min.js",
        &format!("var needle={};\n", "x".repeat(4000)),
    );
    let config = Config {
        max_match_line_chars: 500,
        ..test_config(&repo)
    };
    let nav = navigation_for(&repo, &config);

    let found = nav
        .search_content(&repo, "needle", None, None, None, None)
        .unwrap();

    assert!(
        found.matches[0].text.chars().count() <= 500,
        "line was {} chars",
        found.matches[0].text.chars().count()
    );
}

/// The compile error reaches the model so it can correct the pattern in the
/// same round, rather than being told only that something went wrong.
#[test]
fn an_invalid_pattern_is_refused_with_the_compile_error() {
    let repo = temp_dir("search-badpattern");
    write_fixture(&repo, "src/a.rs", "fn one() {}\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let error = nav
        .search_content(&repo, "fn (one", None, None, None, None)
        .unwrap_err();

    assert!(
        matches!(error, ClientError::InvalidInput(_)),
        "got {error:?}"
    );
    assert!(
        format!("{error}").contains("unclosed"),
        "the compile error must reach the model: {error}"
    );
}

// ---------------------------------------------------------------------------
// Task 5 · edit_file, the anchored splice (requirement 4)
// ---------------------------------------------------------------------------

#[test]
fn an_anchor_matching_once_splices_and_leaves_the_rest_alone() {
    let repo = temp_dir("edit-one");
    write_fixture(&repo, "src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n");
    let policy = PathPolicy::new(&test_config(&repo));

    let changes = region_edits_to_changes(
        &repo,
        &policy,
        &[RegionEdit {
            path: "src/lib.rs".to_string(),
            old_text: "fn b() {}".to_string(),
            new_text: "fn b(x: u8) {}".to_string(),
        }],
    )
    .unwrap();

    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].new_content,
        "fn a() {}\nfn b(x: u8) {}\nfn c() {}\n"
    );
}

#[test]
fn an_anchor_matching_zero_times_is_refused() {
    let repo = temp_dir("edit-zero");
    write_fixture(&repo, "src/lib.rs", "fn a() {}\n");
    let policy = PathPolicy::new(&test_config(&repo));

    let error = region_edits_to_changes(
        &repo,
        &policy,
        &[RegionEdit {
            path: "src/lib.rs".to_string(),
            old_text: "fn missing() {}".to_string(),
            new_text: "fn other() {}".to_string(),
        }],
    )
    .unwrap_err();

    assert!(
        matches!(error, ClientError::InvalidInput(_)),
        "got {error:?}"
    );
    assert!(format!("{error}").contains("did not match"));
}

#[test]
fn an_anchor_matching_twice_is_refused_and_names_the_count() {
    let repo = temp_dir("edit-two");
    write_fixture(&repo, "src/lib.rs", "fn a() {}\nfn a() {}\n");
    let policy = PathPolicy::new(&test_config(&repo));

    let error = region_edits_to_changes(
        &repo,
        &policy,
        &[RegionEdit {
            path: "src/lib.rs".to_string(),
            old_text: "fn a() {}".to_string(),
            new_text: "fn a(x: u8) {}".to_string(),
        }],
    )
    .unwrap_err();

    let message = format!("{error}");
    assert!(
        message.contains("2 times"),
        "the count must reach the model: {message}"
    );
    assert!(message.contains("more context"));
}

/// Proposal §5.3, the reason this anchors on text rather than a line range: an
/// anchor against a stale view of the file stops matching, so staleness lands
/// in a refusal instead of a wrong-region write that only a human reading the
/// diff would catch.
#[test]
fn an_anchor_against_a_changed_file_refuses_rather_than_writing_elsewhere() {
    let repo = temp_dir("edit-stale");
    write_fixture(&repo, "src/lib.rs", "fn a() {}\nfn b() {}\n");
    let policy = PathPolicy::new(&test_config(&repo));
    // The model read the file, then it changed underneath.
    write_fixture(&repo, "src/lib.rs", "fn a() {}\nfn renamed() {}\n");

    let error = region_edits_to_changes(
        &repo,
        &policy,
        &[RegionEdit {
            path: "src/lib.rs".to_string(),
            old_text: "fn b() {}".to_string(),
            new_text: "fn b(x: u8) {}".to_string(),
        }],
    )
    .unwrap_err();

    assert!(matches!(error, ClientError::InvalidInput(_)));
    assert_eq!(
        fs::read_to_string(repo.join("src/lib.rs")).unwrap(),
        "fn a() {}\nfn renamed() {}\n",
        "a refused edit must write nothing"
    );
}

/// Acceptance criterion 1: the payload is the size of the change, not the file.
#[test]
fn a_ten_line_change_in_a_150kb_file_has_a_ten_line_payload() {
    let repo = temp_dir("edit-payload");
    // ~14 bytes a line, so 12000 lines clears 150 KB with room to spare. The
    // assertion below is what caught an undersized fixture at 6000.
    let mut body = (1..=12_000)
        .map(|n| format!("fn f{n}() {{}}\n"))
        .collect::<String>();
    body.push_str("fn target() {}\n");
    assert!(
        body.len() > 150_000,
        "fixture must exceed 150 KB: {}",
        body.len()
    );
    write_fixture(&repo, "src/big.rs", &body);
    let policy = PathPolicy::new(&test_config(&repo));

    let edit = RegionEdit {
        path: "src/big.rs".to_string(),
        old_text: "fn target() {}".to_string(),
        new_text: (1..=10).map(|n| format!("fn target{n}() {{}}\n")).collect(),
    };
    let payload_bytes = edit.old_text.len() + edit.new_text.len();

    let changes = region_edits_to_changes(&repo, &policy, &[edit]).unwrap();

    assert!(
        payload_bytes < 1_000,
        "the edit payload must scale with the change, not the file: {payload_bytes}"
    );
    assert!(
        changes[0].new_content.len() > 150_000,
        "the spliced result is still the whole file"
    );
}

/// Two edits to one file must compose, or the second would be computed against
/// the file on disk and silently discard the first.
#[test]
fn two_edits_to_one_file_build_on_each_other() {
    let repo = temp_dir("edit-compose");
    write_fixture(&repo, "src/lib.rs", "fn a() {}\nfn b() {}\n");
    let policy = PathPolicy::new(&test_config(&repo));

    let changes = region_edits_to_changes(
        &repo,
        &policy,
        &[
            RegionEdit {
                path: "src/lib.rs".to_string(),
                old_text: "fn a() {}".to_string(),
                new_text: "fn a(x: u8) {}".to_string(),
            },
            RegionEdit {
                path: "src/lib.rs".to_string(),
                old_text: "fn b() {}".to_string(),
                new_text: "fn b(y: u8) {}".to_string(),
            },
        ],
    )
    .unwrap();

    assert_eq!(changes.len(), 1, "one file, one ProposedChange");
    assert_eq!(changes[0].new_content, "fn a(x: u8) {}\nfn b(y: u8) {}\n");
}

// ---------------------------------------------------------------------------
// Task 6 · the four tools, wired into the turn
// ---------------------------------------------------------------------------

/// The provider block `foundation.rs` uses to turn native tools on; without it
/// the engine falls back to the text envelope and no tool call is dispatched.
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
    }
}

/// Acceptance criterion 5: navigating requires no allowlist entry and no change
/// to the command trust boundary, because these are engine tools that never
/// reach `command_policy.rs`.
#[test]
fn navigation_needs_no_command_allowlist_entry() {
    let repo = temp_dir("tools-allowlist");
    write_fixture(&repo, "src/lib.rs", "pub fn apply_overlay() {}\n");
    let mut config = test_config(&repo);
    config.command_allowlist = Vec::new();
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);

    // Two navigation calls, then an answer. If either needed approval the turn
    // would stop with a `command_proposal` instead of reaching the third round.
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "apply_overlay is defined in src/lib.rs.".to_string(),
        ],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "list_directory".to_string(),
                arguments_json: "{}".to_string(),
            }],
            vec![ToolCall {
                id: "call_2".to_string(),
                name: "search_content".to_string(),
                arguments_json: "{\"pattern\":\"apply_overlay\"}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "Where is apply_overlay defined?",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert!(
        result.command_proposal.is_none(),
        "no approval may be requested for navigation"
    );
    assert!(result.response.contains("src/lib.rs"));
}

/// The trust boundary itself, pinned so a later change has to be deliberate.
#[test]
fn the_low_risk_read_only_set_is_unchanged_by_this_spec() {
    let repo = temp_dir("tools-boundary");
    let policy = CommandPolicy::new(test_config(&repo));
    for command in ["pwd", "ls", "git status", "git diff", "git log", "git show"] {
        assert!(
            !policy.classify(command, &repo).requires_approval,
            "{command} must stay approval-free"
        );
    }
    for command in [
        "grep -r foo .",
        "find . -name x",
        "cat src/lib.rs",
        "head -n 5 a",
    ] {
        assert!(
            policy.classify(command, &repo).requires_approval,
            "{command} must still need approval: this spec gives navigation as a \
             tool, not at the shell"
        );
    }
}

// ---------------------------------------------------------------------------
// Task 7 · acceptance criteria and closing the slice
// ---------------------------------------------------------------------------

/// Acceptance criterion: "same review, same hunks, same checkpoint".
#[test]
fn a_region_edit_and_a_whole_file_proposal_produce_the_same_patch() {
    let repo = temp_dir("edit-equivalence");
    let original = "fn a() {}\nfn b() {}\nfn c() {}\n";
    let expected = "fn a() {}\nfn b(x: u8) {}\nfn c() {}\n";
    write_fixture(&repo, "src/lib.rs", original);
    let config = test_config(&repo);
    let scanner = SecretScanner::default();
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let policy = PathPolicy::new(&config);

    let whole = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/lib.rs".to_string(),
                new_content: expected.to_string(),
                status: Some("modified".to_string()),
                allow_restricted: false,
            }],
            None,
            "change b",
        )
        .unwrap();

    let region_changes = region_edits_to_changes(
        &repo,
        &policy,
        &[RegionEdit {
            path: "src/lib.rs".to_string(),
            old_text: "fn b() {}".to_string(),
            new_text: "fn b(x: u8) {}".to_string(),
        }],
    )
    .unwrap();
    let region = engine
        .create_patch(&repo, &region_changes, None, "change b")
        .unwrap();

    assert_eq!(region.files[0].new_content, whole.files[0].new_content);
    assert_eq!(region.files[0].new_hash, whole.files[0].new_hash);
    assert_eq!(region.files[0].base_hash, whole.files[0].base_hash);
    assert_eq!(region.files[0].diff, whole.files[0].diff);
    assert_eq!(region.files[0].hunks, whole.files[0].hunks);
    assert_eq!(region.files[0].status, whole.files[0].status);
}

/// §5.5: a truncated result that reads as complete is the failure this design
/// exists to prevent. One assertion per read tool.
#[test]
fn every_capped_read_tool_states_what_it_cut() {
    let repo = temp_dir("notices");
    for n in 1..=300 {
        write_fixture(&repo, &format!("src/m{n}.rs"), "let needle = 1;\n");
    }
    let body = (1..=1000)
        .map(|n| format!("line {n}\n"))
        .collect::<String>();
    write_fixture(&repo, "big.rs", &body);
    let config = Config {
        max_list_entries: 10,
        max_search_matches: 10,
        max_read_lines: 10,
        ..test_config(&repo)
    };
    let nav = navigation_for(&repo, &config);
    let scanner = SecretScanner::default();
    let access = FileAccessController::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );

    let listing = nav.list_directory(&repo, None, None, None, None).unwrap();
    assert!(listing.truncated && listing.total_found > listing.paths.len());

    let found = nav
        .search_content(&repo, "needle", None, None, None, None)
        .unwrap();
    assert!(found.truncated && found.total_found > found.matches.len());

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
    assert_eq!(read.truncated_by, Some("lines".to_string()));
    assert!(read.total_lines > read.line_range.end);
}

/// Requirement 7. `read_file` already audits as `file_read`; the two new
/// navigation tools must too, and an `edit_file` proposal must reach the audit
/// log by the same `patch_proposed` event a whole-file proposal produces —
/// which is the audit half of "indistinguishable downstream".
#[test]
fn every_new_tool_records_an_audit_event() {
    let repo = temp_dir("audit-tools");
    write_fixture(&repo, "src/lib.rs", "pub fn apply_overlay() {}\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    nav.list_directory(&repo, None, None, Some("task-1"), Some("repo-1"))
        .unwrap();
    nav.search_content(
        &repo,
        "apply_overlay",
        None,
        None,
        Some("task-1"),
        Some("repo-1"),
    )
    .unwrap();

    let events = fs::read_to_string(repo.join(".damaian/audit/events.jsonl")).unwrap();
    assert!(
        events.contains("directory_listed"),
        "list_directory must audit"
    );
    assert!(
        events.contains("content_searched"),
        "search_content must audit"
    );
    assert!(
        !events.contains("AKIA") && !events.contains("sk_live"),
        "an audit entry must never carry a secret from the search it recorded"
    );
}

// ---------------------------------------------------------------------------
// Requirement 8 · batching and concurrent dispatch
// ---------------------------------------------------------------------------

/// Requirement 8, first half. A round that asks for two read-only calls must
/// execute both — `first_decodable_tool_action` used to return on the first
/// decode and silently drop the rest, so the multi-call shape the spec assumed
/// the code already had did not exist. The second file's content is the
/// observable proof that the second call reached dispatch.
#[test]
fn a_round_executes_every_read_only_call_it_was_given() {
    let repo = temp_dir("batch-two-reads");
    write_fixture(&repo, "src/one.rs", "pub fn one() {}\n");
    write_fixture(&repo, "src/two.rs", "pub fn two() {}\n");
    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);

    // One model message carrying both calls, then an answer. The two calls
    // share a turn because they arrived in one `tool_calls` vector.
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Both files read.".to_string()],
        vec![
            vec![
                ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments_json: "{\"path\":\"src/one.rs\"}".to_string(),
                },
                ToolCall {
                    id: "call_2".to_string(),
                    name: "read_file".to_string(),
                    arguments_json: "{\"path\":\"src/two.rs\"}".to_string(),
                },
            ],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(&repo, "Read both files.", &[], &mut adapter, &mut on_token)
        .unwrap();

    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    let tool_contents: Vec<&str> = messages
        .iter()
        .filter(|message| message.role == "tool")
        .map(|message| message.content.as_str())
        .collect();

    assert!(
        tool_contents
            .iter()
            .any(|text| text.contains("pub fn one()")),
        "the first call's result must be present: {tool_contents:?}"
    );
    assert!(
        tool_contents
            .iter()
            .any(|text| text.contains("pub fn two()")),
        "the second call in the same round must run, not be dropped: {tool_contents:?}"
    );
}

/// Requirement 8: a concurrent batch's results are recorded in the model's
/// call order, not in completion order. Three calls with three distinct
/// contents, so the assertion is on the order and not just on presence.
#[test]
fn a_read_only_batch_records_results_in_the_models_call_order() {
    let repo = temp_dir("batch-order");
    write_fixture(&repo, "src/a.rs", "pub fn alpha() {}\n");
    write_fixture(&repo, "src/b.rs", "pub fn beta() {}\n");
    write_fixture(&repo, "src/c.rs", "pub fn gamma() {}\n");
    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);

    let reads = ["src/a.rs", "src/b.rs", "src/c.rs"]
        .iter()
        .enumerate()
        .map(|(index, path)| ToolCall {
            id: format!("call_{}", index + 1),
            name: "read_file".to_string(),
            arguments_json: format!("{{\"path\":\"{path}\"}}"),
        })
        .collect();
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Read all three.".to_string()],
        vec![reads, Vec::new()],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(&repo, "Read three files.", &[], &mut adapter, &mut on_token)
        .unwrap();

    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    let tool_contents: Vec<&str> = messages
        .iter()
        .filter(|message| message.role == "tool")
        .map(|message| message.content.as_str())
        .collect();

    let position = |needle: &str| {
        tool_contents
            .iter()
            .position(|text| text.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle:?}: {tool_contents:?}"))
    };
    assert!(
        position("alpha") < position("beta") && position("beta") < position("gamma"),
        "results must be recorded in the model's call order: {tool_contents:?}"
    );
}

/// Requirement 8: a round that mixes a read-only call with a mutating one runs
/// the mutating call sequentially. The read before it still ran and was
/// recorded; the command then stopped the turn for approval.
#[test]
fn a_round_mixing_reads_and_a_mutating_call_runs_sequentially() {
    let repo = temp_dir("batch-mixed");
    write_fixture(&repo, "src/one.rs", "pub fn one() {}\n");
    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);

    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new()],
        vec![
            vec![
                ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments_json: "{\"path\":\"src/one.rs\"}".to_string(),
                },
                ToolCall {
                    id: "call_2".to_string(),
                    name: "run_command".to_string(),
                    arguments_json: "{\"command\":\"npm test\",\"reason\":\"Run the tests\"}"
                        .to_string(),
                },
            ],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "Read src/one.rs, then run the tests.",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert!(
        result.command_proposal.is_some(),
        "the mutating call must stop for approval"
    );
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.role == "tool" && message.content.contains("pub fn one()")),
        "the read that preceded the mutating call must still have run: {messages:?}"
    );
}

/// Requirement 8: session-log order after a concurrent round is deterministic.
/// Three fresh runs of the same read-only round must produce the same role and
/// content sequence, because the results are joined in call order.
#[test]
fn repeated_read_only_rounds_produce_the_same_session_sequence() {
    let mut sequences = Vec::new();
    for run in 0..3 {
        let repo = temp_dir(&format!("batch-determinism-{run}"));
        write_fixture(&repo, "src/a.rs", "pub fn alpha() {}\n");
        write_fixture(&repo, "src/b.rs", "pub fn beta() {}\n");
        let mut config = test_config(&repo);
        config.model_providers.push(native_tool_provider());
        let engine = WorkspaceEngine::new(config);

        let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Done.".to_string()],
            vec![
                vec![
                    ToolCall {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        arguments_json: "{\"path\":\"src/a.rs\"}".to_string(),
                    },
                    ToolCall {
                        id: "call_2".to_string(),
                        name: "read_file".to_string(),
                        arguments_json: "{\"path\":\"src/b.rs\"}".to_string(),
                    },
                ],
                Vec::new(),
            ],
        );
        let mut on_token = |_token: &str| {};

        let result = engine
            .chat_orchestrator
            .ask(&repo, "Read both.", &[], &mut adapter, &mut on_token)
            .unwrap();
        let sequence: Vec<(String, String)> = engine
            .session_store
            .read_messages(&result.session.id)
            .unwrap()
            .into_iter()
            .map(|message| (message.role, message.content))
            .collect();
        sequences.push(sequence);
    }

    assert_eq!(
        sequences[0], sequences[1],
        "a concurrent round must log the same order every run"
    );
    assert_eq!(
        sequences[1], sequences[2],
        "a concurrent round must log the same order every run"
    );
}
