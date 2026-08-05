use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    AuditLog, ClientError, CommandPolicy, CommandRisk, Config, ConfigOverlay,
    DEFAULT_CONTEXT_TOKEN_BUDGET, IndexCache, McpClient,
    McpServerConfig, McpTransport, MockModelAdapter, MockModelTransport, ModelAdapter, ModelMessage,
    ModelProviderConfig, ModelRequest, OpenAICompatibleAdapter, PatchEngine, PatchStore, PathPolicy,
    ProjectIndexer, ProposedChange, SecretScanner, SessionStore, ToolCall, WorkspaceEngine,
    extract_model_tokens, model_request_json, parse_generated_edit,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);
const AWS_ACCESS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-rust-{name}-{now}-{}",
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

fn run_git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git should run");
    assert!(status.success(), "git {:?} failed", args);
}

fn test_config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        ..Config::default()
    }
}

fn test_audit(repo: &Path, scanner: SecretScanner) -> AuditLog {
    AuditLog::new(repo.join(".damaian"), true, scanner)
}

#[test]
fn redacts_credential_assignments() {
    let scanner = SecretScanner::default();
    let result = scanner.redact("api_key = \"sk_test_12345678901234567890\"");

    assert!(result.text.contains("api_key = \""));
    assert!(result.text.contains("[REDACTED_"));
    assert_eq!(result.findings.len(), 1);
}

#[test]
fn detects_private_keys() {
    let scanner = SecretScanner::default();
    let secret = "-----BEGIN PRIVATE KEY-----\nabc123\n-----END PRIVATE KEY-----";
    let findings = scanner.scan(secret);

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, "private_key");
}

#[test]
fn redacts_jwts() {
    let scanner = SecretScanner::default();
    let jwt = concat!(
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.",
        "eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkRhbWFpYW4iLCJpYXQiOjE1MTYyMzkwMjJ9.",
        "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
    );
    let result = scanner.redact(&format!("jwt={jwt}"));

    assert!(result.text.contains("[REDACTED_JWT_"));
    assert!(!result.text.contains(jwt));
    assert_eq!(result.findings[0].category, "jwt");
}

#[test]
fn redacts_gcp_api_keys() {
    let scanner = SecretScanner::default();
    let key = "AIza12345678901234567890123456789012345";
    let result = scanner.redact(&format!("gcp={key}"));

    assert!(result.text.contains("[REDACTED_GCP_API_KEY_"));
    assert!(!result.text.contains(key));
    assert_eq!(result.findings[0].category, "gcp_api_key");
}

#[test]
fn redacts_azure_account_keys() {
    let scanner = SecretScanner::default();
    let key = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+/abcd==";
    let result = scanner.redact(&format!(
        "DefaultEndpointsProtocol=https;AccountName=test;AccountKey={key};EndpointSuffix=core.windows.net"
    ));

    assert!(result.text.contains("[REDACTED_AZURE_ACCOUNT_KEY_"));
    assert!(!result.text.contains(key));
    assert_eq!(result.findings[0].category, "azure_account_key");
}

#[test]
fn redacts_slack_xoxa_tokens() {
    let scanner = SecretScanner::default();
    let token = "xoxa-2-123456789012-123456789012-abcdefghijklmnopqrstuvwx";
    let result = scanner.redact(&format!("slack={token}"));

    assert!(result.text.contains("[REDACTED_GENERIC_API_KEY_"));
    assert!(!result.text.contains(token));
}

#[test]
fn redacts_slack_xoxr_tokens() {
    let scanner = SecretScanner::default();
    let token = "xoxr-2-123456789012-123456789012-abcdefghijklmnopqrstuvwx";
    let result = scanner.redact(&format!("slack={token}"));

    assert!(result.text.contains("[REDACTED_GENERIC_API_KEY_"));
    assert!(!result.text.contains(token));
}

#[test]
fn redacts_slack_xoxs_tokens() {
    let scanner = SecretScanner::default();
    let token = "xoxs-2-123456789012-123456789012-abcdefghijklmnopqrstuvwx";
    let result = scanner.redact(&format!("slack={token}"));

    assert!(result.text.contains("[REDACTED_GENERIC_API_KEY_"));
    assert!(!result.text.contains(token));
}

#[test]
fn scans_non_ascii_text_without_panicking() {
    let scanner = SecretScanner::default();
    let result = scanner.redact("AI Coding Assistant Client — Must-Have Features");

    assert_eq!(result.findings.len(), 0);
}

#[test]
fn denies_symlink_traversal_outside_selected_repository() {
    let root = temp_dir("path-policy");
    let repo = root.join("repo");
    let outside = root.join("outside");
    write_fixture(&repo, "src/app.js", "console.log(\"ok\");");
    write_fixture(&outside, "secret.txt", "password=supersecret");
    std::os::unix::fs::symlink(outside.join("secret.txt"), repo.join("linked-secret.txt")).unwrap();

    let config = Config {
        allowed_roots: vec![repo.clone()],
        data_dir: repo.join(".damaian"),
        ..Config::default()
    };
    let policy = PathPolicy::new(&config);
    let error = policy
        .resolve_existing(&repo, "linked-secret.txt", false)
        .expect_err("symlink should be denied");
    assert!(matches!(error, ClientError::AccessDenied(_)));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn allows_path_outside_repository_when_explicitly_permitted() {
    let root = temp_dir("path-policy-permitted");
    let repo = root.join("repo");
    let outside = root.join("outside");
    write_fixture(&repo, "src/app.js", "console.log(\"ok\");");
    write_fixture(&outside, "notes.txt", "pinned by the user");

    let config = Config {
        allowed_roots: vec![repo.clone()],
        data_dir: repo.join(".damaian"),
        ..Config::default()
    };
    let policy = PathPolicy::new(&config);
    let resolved = policy
        .resolve_existing(&repo, outside.join("notes.txt"), true)
        .expect("explicitly permitted path outside the repo should resolve");
    assert_eq!(
        resolved.relative_path,
        fs::canonicalize(outside.join("notes.txt"))
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_patch_creation_for_symlink_write_target_outside_repository() {
    let root = temp_dir("patch-symlink");
    let repo = root.join("repo");
    let outside = root.join("outside");
    write_fixture(&repo, "src/app.js", "console.log(\"ok\");");
    write_fixture(&outside, "secret.txt", "password=supersecret");
    std::os::unix::fs::symlink(outside.join("secret.txt"), repo.join("linked-secret.txt")).unwrap();

    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let path_policy = PathPolicy::new(&config);
    let error = path_policy
        .resolve_for_write(&repo, "linked-secret.txt")
        .expect_err("symlink write target should be denied");
    assert!(matches!(error, ClientError::AccessDenied(_)));

    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        path_policy,
    );
    let error = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "linked-secret.txt".to_string(),
                new_content: "safe replacement\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "replace linked file",
        )
        .expect_err("patch creation should not read through symlink");
    assert!(matches!(error, ClientError::AccessDenied(_)));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn marks_restricted_dotenv_files() {
    let policy = PathPolicy::unrestricted();
    assert!(policy.is_restricted(".env", false));
    assert!(!policy.is_restricted("src/app.js", false));
}

#[test]
fn indexes_source_files_while_respecting_gitignore_and_redacting_secrets() {
    let repo = temp_dir("indexer");
    write_fixture(&repo, ".gitignore", "dist/\nignored.js\n");
    write_fixture(
        &repo,
        "src/auth.js",
        "export function login() { return true; }\n",
    );
    write_fixture(&repo, "dist/bundle.js", "generated");
    write_fixture(&repo, "ignored.js", "ignored");
    write_fixture(
        &repo,
        "src/secret.js",
        "const api_key = \"sk_test_12345678901234567890\";\n",
    );

    let scanner = SecretScanner::default();
    let indexer = ProjectIndexer::new(
        test_config(&repo),
        scanner.clone(),
        test_audit(&repo, scanner),
    );
    let index = indexer.index_repository(&repo).unwrap();
    let files = index
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(files, vec!["src/auth.js", "src/secret.js"]);
    assert!(
        index
            .skipped
            .iter()
            .any(|file| file.path == "dist" || file.path == "dist/bundle.js")
    );
    let secret_file = index
        .files
        .iter()
        .find(|file| file.path == "src/secret.js")
        .expect("secret-bearing file should be indexed with redaction");
    assert!(
        secret_file
            .chunks
            .iter()
            .all(|chunk| !chunk.text.contains("sk_test_12345678901234567890"))
    );
    assert!(
        secret_file
            .chunks
            .iter()
            .any(|chunk| chunk.text.contains("[REDACTED_"))
    );
    assert_eq!(index.keyword_search("login", 1)[0].path, "src/auth.js");

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn index_cache_picks_up_file_changes_via_watcher_without_full_rescan() {
    let repo = temp_dir("index-cache-watcher");
    write_fixture(&repo, "src/app.js", "export const value = \"original\";\n");

    let scanner = SecretScanner::default();
    let indexer = ProjectIndexer::new(
        test_config(&repo),
        scanner.clone(),
        test_audit(&repo, scanner),
    );

    let first = IndexCache::get_or_build(&indexer, &repo).unwrap();
    assert_eq!(first.keyword_search("original", 1).len(), 1);

    // Modify the file on disk after the index was built; the background
    // watcher (not the 5-minute periodic rescan) is responsible for picking
    // this up, so poll with a short bounded timeout rather than sleeping for
    // the rescan interval.
    fs::write(repo.join("src/app.js"), "export const value = \"updated\";\n").unwrap();

    let mut picked_up = false;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let refreshed = IndexCache::get_or_build(&indexer, &repo).unwrap();
        if refreshed.keyword_search("updated", 1).len() == 1
            && refreshed.keyword_search("original", 1).is_empty()
        {
            picked_up = true;
            break;
        }
    }
    assert!(
        picked_up,
        "expected the watcher to reindex the changed file within 5 seconds"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn classifies_command_risk() {
    let policy = CommandPolicy::new(Config {
        data_dir: PathBuf::from("/tmp/damaian-test"),
        ..Config::default()
    });

    let root = Path::new("/tmp/damaian-test");
    assert_eq!(
        policy.classify("git status --short", root).risk,
        CommandRisk::Low
    );
    assert_eq!(
        policy.classify("git show --stat", root).risk,
        CommandRisk::Low
    );
    assert_eq!(policy.classify("npm test", root).risk, CommandRisk::Medium);
    assert_eq!(policy.classify("rm -rf .", root).risk, CommandRisk::Blocked);
    assert_eq!(policy.classify("ls | head", root).risk, CommandRisk::High);
}

#[test]
fn allowlist_does_not_bypass_shell_control_detection() {
    let policy = CommandPolicy::new(Config {
        data_dir: PathBuf::from("/tmp/damaian-test"),
        command_allowlist: vec!["npm test".to_string()],
        ..Config::default()
    });

    let root = Path::new("/tmp/damaian-test");
    assert_eq!(policy.classify("npm test", root).risk, CommandRisk::Low);
    for command in ["npm test; rm -rf ~", "npm test\ncat /etc/passwd"] {
        let classification = policy.classify(command, root);
        assert_eq!(classification.risk, CommandRisk::High);
        assert!(classification.requires_approval);
    }
}

#[test]
fn escalates_approval_for_commands_referencing_paths_outside_repo() {
    let policy = CommandPolicy::new(Config {
        data_dir: PathBuf::from("/tmp/damaian-test"),
        ..Config::default()
    });
    let root = Path::new("/Users/example/project");

    let classification = policy.classify("ls ../outside", root);
    assert_eq!(classification.risk, CommandRisk::Medium);
    assert!(classification.requires_approval);
    assert!(
        classification
            .reasons
            .iter()
            .any(|reason| reason.contains("outside the selected repository"))
    );

    let inside = policy.classify("ls src", root);
    assert_eq!(inside.risk, CommandRisk::Low);
    assert!(!inside.requires_approval);
    assert!(
        !inside
            .reasons
            .iter()
            .any(|reason| reason.contains("outside the selected repository"))
    );
}

#[test]
fn creates_diff_and_applies_approved_changes_safely() {
    let repo = temp_dir("patch");
    write_fixture(&repo, "src/app.js", "export const value = 1;\n");
    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/app.js".to_string(),
                new_content: "export const value = 2;\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            Some("task_1"),
            "Update value",
        )
        .unwrap();

    assert!(patch.files[0].diff.contains("-export const value = 1;"));
    assert!(patch.files[0].diff.contains("+export const value = 2;"));

    let result = engine
        .apply_patch(&repo, &patch, None, None, "tester", false)
        .unwrap();
    assert_eq!(result.applied_files, vec!["src/app.js"]);
    assert_eq!(
        fs::read_to_string(repo.join("src/app.js")).unwrap(),
        "export const value = 2;\n"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn supports_adding_files_in_new_directories() {
    let repo = temp_dir("patch-new-file");
    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/features/new-file.js".to_string(),
                new_content: "export const ready = true;\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            Some("task_2"),
            "Add feature file",
        )
        .unwrap();

    let result = engine
        .apply_patch(&repo, &patch, None, None, "tester", false)
        .unwrap();
    assert_eq!(result.applied_files, vec!["src/features/new-file.js"]);
    assert_eq!(
        fs::read_to_string(repo.join("src/features/new-file.js")).unwrap(),
        "export const ready = true;\n"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn applies_only_selected_hunk_and_allows_rollback_afterward() {
    let repo = temp_dir("patch-partial-hunk");
    let old_content: String = (1..=30).map(|n| format!("line{n}\n")).collect();
    write_fixture(&repo, "src/app.js", &old_content);
    let mut new_lines: Vec<String> = (1..=30).map(|n| format!("line{n}\n")).collect();
    new_lines[1] = "CHANGED_2\n".to_string();
    new_lines[27] = "CHANGED_28\n".to_string();
    let new_content = new_lines.concat();

    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/app.js".to_string(),
                new_content: new_content.clone(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "two separate changes",
        )
        .unwrap();
    assert_eq!(patch.files[0].hunks.len(), 2);

    // Accept only the second hunk.
    let accepted_hunk_id = patch.files[0].hunks[1].id.clone();
    let mut hunk_selection = std::collections::HashMap::new();
    hunk_selection.insert("src/app.js".to_string(), vec![accepted_hunk_id]);

    let result = engine
        .apply_patch(&repo, &patch, None, Some(&hunk_selection), "tester", false)
        .unwrap();
    assert_eq!(result.applied_files, vec!["src/app.js"]);

    let mut expected_lines: Vec<String> = (1..=30).map(|n| format!("line{n}\n")).collect();
    expected_lines[27] = "CHANGED_28\n".to_string();
    let expected_content = expected_lines.concat();
    assert_eq!(
        fs::read_to_string(repo.join("src/app.js")).unwrap(),
        expected_content
    );
    assert_ne!(expected_content, new_content);

    // Rollback should still work: the conflict check must compare against
    // what was actually written (the partial-accept content), not the
    // patch's full `new_hash`.
    let rollback = engine.rollback_patch(&repo, &patch, None, "tester").unwrap();
    assert_eq!(rollback.restored_files, vec!["src/app.js"]);
    assert_eq!(
        fs::read_to_string(repo.join("src/app.js")).unwrap(),
        old_content
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn partial_hunk_apply_records_excluded_hunks_in_audit_log() {
    let repo = temp_dir("patch-hunk-audit");
    let old_content: String = (1..=30).map(|n| format!("line{n}\n")).collect();
    write_fixture(&repo, "src/app.js", &old_content);
    let mut new_lines: Vec<String> = (1..=30).map(|n| format!("line{n}\n")).collect();
    new_lines[1] = "CHANGED_2\n".to_string();
    new_lines[27] = "CHANGED_28\n".to_string();
    let new_content = new_lines.concat();

    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/app.js".to_string(),
                new_content,
                status: None,
                allow_restricted: false,
            }],
            None,
            "two separate changes",
        )
        .unwrap();
    assert_eq!(patch.files[0].hunks.len(), 2);

    let excluded_hunk_id = patch.files[0].hunks[0].id.clone();
    let accepted_hunk_id = patch.files[0].hunks[1].id.clone();
    let mut hunk_selection = std::collections::HashMap::new();
    hunk_selection.insert("src/app.js".to_string(), vec![accepted_hunk_id]);

    engine
        .apply_patch(&repo, &patch, None, Some(&hunk_selection), "tester", false)
        .unwrap();

    let audit_log =
        fs::read_to_string(repo.join(".damaian").join("audit").join("events.jsonl")).unwrap();
    let rejection_line = audit_log
        .lines()
        .find(|line| line.contains("patch_hunks_rejected"))
        .expect("expected a patch_hunks_rejected audit event");
    assert!(rejection_line.contains(&excluded_hunk_id));
    assert!(rejection_line.contains("src/app.js"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn full_hunk_apply_records_no_excluded_hunks_audit_event() {
    let repo = temp_dir("patch-hunk-audit-full");
    let old_content: String = (1..=30).map(|n| format!("line{n}\n")).collect();
    write_fixture(&repo, "src/app.js", &old_content);
    let mut new_lines: Vec<String> = (1..=30).map(|n| format!("line{n}\n")).collect();
    new_lines[1] = "CHANGED_2\n".to_string();
    new_lines[27] = "CHANGED_28\n".to_string();
    let new_content = new_lines.concat();

    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/app.js".to_string(),
                new_content,
                status: None,
                allow_restricted: false,
            }],
            None,
            "two separate changes",
        )
        .unwrap();

    // Accept every hunk: no exclusions, so no rejection event should fire.
    let all_hunk_ids: Vec<String> = patch.files[0].hunks.iter().map(|h| h.id.clone()).collect();
    let mut hunk_selection = std::collections::HashMap::new();
    hunk_selection.insert("src/app.js".to_string(), all_hunk_ids);

    engine
        .apply_patch(&repo, &patch, None, Some(&hunk_selection), "tester", false)
        .unwrap();

    let audit_log =
        fs::read_to_string(repo.join(".damaian").join("audit").join("events.jsonl")).unwrap();
    assert!(!audit_log.contains("patch_hunks_rejected"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn parses_shared_hunk_selection_json() {
    let selection =
        workspace_engine::parse_hunk_selection("{\"src/app.js\":[\"hunk_0\",\"hunk_1\"]}").unwrap();
    assert_eq!(
        selection.get("src/app.js"),
        Some(&vec!["hunk_0".to_string(), "hunk_1".to_string()])
    );

    assert!(workspace_engine::parse_hunk_selection("not json").is_err());
}

#[test]
fn blocks_apply_when_target_changes_after_patch_creation() {
    let repo = temp_dir("patch-conflict");
    write_fixture(&repo, "src/app.js", "one\n");
    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/app.js".to_string(),
                new_content: "two\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "change",
        )
        .unwrap();
    fs::write(repo.join("src/app.js"), "user edit\n").unwrap();

    let error = engine
        .apply_patch(&repo, &patch, None, None, "tester", false)
        .expect_err("conflict should block apply");
    assert!(matches!(error, ClientError::PatchConflict(_)));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn blocks_generated_hardcoded_secrets_by_default() {
    let repo = temp_dir("patch-secret");
    write_fixture(&repo, "src/config.js", "export const token = \"\";\n");
    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/config.js".to_string(),
                new_content: "export const api_key = \"sk_test_12345678901234567890\";\n"
                    .to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "secret",
        )
        .unwrap();

    let error = engine
        .apply_patch(&repo, &patch, None, None, "tester", false)
        .expect_err("secret should block apply");
    assert!(matches!(error, ClientError::PolicyBlocked(_)));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn redacts_secrets_from_patch_diffs_before_storage() {
    let repo = temp_dir("patch-diff-redaction");
    write_fixture(
        &repo,
        "src/config.js",
        &format!("export const awsKey = \"{AWS_ACCESS_KEY}\";\n"),
    );
    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );

    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/config.js".to_string(),
                new_content: "export const awsKey = \"\";\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "remove secret",
        )
        .unwrap();

    assert!(patch.files[0].diff.contains("[REDACTED_AWS_ACCESS_KEY_"));
    assert!(!patch.files[0].diff.contains(AWS_ACCESS_KEY));

    let store = PatchStore::new(&config.data_dir);
    let patch_path = store.save(&patch).unwrap();
    let stored_patch = fs::read_to_string(patch_path).unwrap();
    assert!(stored_patch.contains("[REDACTED_AWS_ACCESS_KEY_"));
    assert!(!stored_patch.contains(AWS_ACCESS_KEY));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn redacts_secrets_from_rollback_snapshots() {
    let repo = temp_dir("rollback-redaction");
    write_fixture(
        &repo,
        "src/config.js",
        &format!("export const awsKey = \"{AWS_ACCESS_KEY}\";\n"),
    );
    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/config.js".to_string(),
                new_content: "export const awsKey = \"\";\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "remove secret",
        )
        .unwrap();

    engine
        .apply_patch(&repo, &patch, None, None, "tester", false)
        .unwrap();

    let rollback_path = config
        .data_dir
        .join("rollback")
        .join(&patch.id)
        .join("src__config.js");
    let rollback_snapshot = fs::read_to_string(rollback_path).unwrap();
    assert!(rollback_snapshot.contains("[REDACTED_AWS_ACCESS_KEY_"));
    assert!(!rollback_snapshot.contains(AWS_ACCESS_KEY));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn rollback_restores_modified_file_and_warns_about_lost_secret() {
    let repo = temp_dir("rollback-restore-modified");
    write_fixture(
        &repo,
        "src/config.js",
        &format!("export const awsKey = \"{AWS_ACCESS_KEY}\";\n"),
    );
    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/config.js".to_string(),
                new_content: "export const awsKey = \"\";\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "remove secret",
        )
        .unwrap();
    engine
        .apply_patch(&repo, &patch, None, None, "tester", false)
        .unwrap();
    assert_eq!(
        fs::read_to_string(repo.join("src/config.js")).unwrap(),
        "export const awsKey = \"\";\n"
    );

    let result = engine.rollback_patch(&repo, &patch, None, "tester").unwrap();

    assert_eq!(result.restored_files, vec!["src/config.js"]);
    assert!(result.deleted_files.is_empty());
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("src/config.js"));
    let restored = fs::read_to_string(repo.join("src/config.js")).unwrap();
    assert!(restored.contains("[REDACTED_AWS_ACCESS_KEY_"));
    assert!(!restored.contains(AWS_ACCESS_KEY));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn rollback_deletes_file_that_patch_added() {
    let repo = temp_dir("rollback-delete-added");
    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/new-file.js".to_string(),
                new_content: "export const ready = true;\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "add file",
        )
        .unwrap();
    engine
        .apply_patch(&repo, &patch, None, None, "tester", false)
        .unwrap();
    assert!(repo.join("src/new-file.js").exists());

    let result = engine.rollback_patch(&repo, &patch, None, "tester").unwrap();

    assert_eq!(result.deleted_files, vec!["src/new-file.js"]);
    assert!(result.restored_files.is_empty());
    assert!(result.warnings.is_empty());
    assert!(!repo.join("src/new-file.js").exists());

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn rollback_refuses_when_file_changed_after_apply() {
    let repo = temp_dir("rollback-conflict");
    write_fixture(&repo, "src/app.js", "one\n");
    let scanner = SecretScanner::default();
    let config = test_config(&repo);
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let patch = engine
        .create_patch(
            &repo,
            &[ProposedChange {
                path: "src/app.js".to_string(),
                new_content: "two\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "change",
        )
        .unwrap();
    engine
        .apply_patch(&repo, &patch, None, None, "tester", false)
        .unwrap();
    fs::write(repo.join("src/app.js"), "independent edit\n").unwrap();

    let error = engine
        .rollback_patch(&repo, &patch, None, "tester")
        .expect_err("conflict should block rollback");
    assert!(matches!(error, ClientError::PatchConflict(_)));
    assert_eq!(
        fs::read_to_string(repo.join("src/app.js")).unwrap(),
        "independent edit\n"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn redacts_secrets_from_git_diff_output() {
    let repo = temp_dir("git-diff-redaction");
    write_fixture(
        &repo,
        "src/config.js",
        &format!("export const awsKey = \"{AWS_ACCESS_KEY}\";\n"),
    );
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["add", "src/config.js"]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=Damaian Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "baseline",
        ],
    );
    fs::write(repo.join("src/config.js"), "export const awsKey = \"\";\n").unwrap();
    let engine = WorkspaceEngine::new(test_config(&repo));

    let diff = engine.git.diff(&repo, false).unwrap();

    assert!(diff.contains("[REDACTED_AWS_ACCESS_KEY_"));
    assert!(!diff.contains(AWS_ACCESS_KEY));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn persists_session_tasks_and_messages() {
    let repo = temp_dir("session-store");
    let store = SessionStore::new(repo.join(".damaian"));
    let session = store.create_session("repo_1", "Explain auth flow").unwrap();
    let task = store
        .create_task(&session.id, "Explain auth", "mock", "mock-model")
        .unwrap();
    store
        .append_message(&session.id, Some(&task.id), "user", "Explain auth")
        .unwrap();
    store
        .append_message(
            &session.id,
            Some(&task.id),
            "assistant",
            "Auth uses tokens — safely.",
        )
        .unwrap();

    let messages = store.read_messages(&session.id).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].content, "Auth uses tokens — safely.");

    let sessions = store.list_sessions(Some("repo_1")).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "Explain auth flow");

    let renamed = store.rename_session(&session.id, "Auth notes").unwrap();
    assert_eq!(renamed.title, "Auth notes");
    assert_eq!(
        store.read_session(&session.id).unwrap().unwrap().title,
        "Auth notes"
    );

    store.delete_session(&session.id).unwrap();
    assert!(store.read_session(&session.id).unwrap().is_none());

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn orchestrates_chat_with_indexed_context_and_mock_model() {
    let repo = temp_dir("chat");
    write_fixture(&repo, "README.md", "# Chat test\n");
    write_fixture(
        &repo,
        "src/auth.js",
        "export function refreshToken() { return 'ok'; }\n",
    );
    let config = test_config(&repo);
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new("Refresh token is implemented in src/auth.js.");
    let mut streamed = String::new();
    let mut on_token = |token: &str| streamed.push_str(token);

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "How does refresh token work?",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert_eq!(streamed, "Refresh token is implemented in src/auth.js.");
    assert!(result.context_files.contains(&"src/auth.js".to_string()));
    assert!(result.response.contains("src/auth.js"));
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    assert_eq!(messages.len(), 2);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_runs_sandbox_command_requested_by_model() {
    let repo = temp_dir("chat-sandbox-command");
    write_fixture(&repo, "README.md", "# Chat command test\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new_sequence(vec![
        "I need to inspect the working directory first.\n\nDAMAIAN_COMMAND_V1\nCOMMAND: pwd\nREASON: Inspect current working directory.\nEND_COMMAND\n"
            .to_string(),
        "The sandbox command completed and the repository path was inspected.".to_string(),
    ]);
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "What directory is this project using?",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert!(result.command_proposal.is_none());
    assert!(result.response.contains("sandbox command completed"));
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    // The tool call and its sandboxed result are now persisted alongside the
    // user prompt and final answer, so a follow-up turn in this session can
    // still see that a command ran.
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert!(messages[1].content.contains("pwd"));
    assert_eq!(messages[2].role, "tool");
    assert!(messages[2].content.contains("Command: pwd"));
    assert_eq!(messages[3].role, "assistant");
    assert!(messages[3].content.contains("sandbox command completed"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_dispatches_native_tool_call_when_provider_supports_it() {
    let repo = temp_dir("chat-native-tool-call");
    write_fixture(&repo, "README.md", "# Chat native tool call test\n");
    let mut config = test_config(&repo);
    config.model_providers.push(ModelProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: Vec::new(),
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
    });
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            "The sandbox command completed via a native tool call.".to_string(),
        ],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "run_command".to_string(),
                arguments_json:
                    "{\"command\":\"pwd\",\"reason\":\"Inspect working directory\"}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "What directory is this project using?",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert!(result.command_proposal.is_none());
    assert!(result.response.contains("native tool call"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_chains_multiple_native_tool_calls_within_one_turn() {
    let repo = temp_dir("chat-multi-tool-call");
    write_fixture(&repo, "README.md", "# Chat multi tool call test\n");
    let mut config = test_config(&repo);
    config.model_providers.push(ModelProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: Vec::new(),
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
    });
    let engine = WorkspaceEngine::new(config);
    // The model asks to run `pwd`, then—after seeing that result—asks to
    // run `ls`, before finally answering. This only works if the client
    // keeps tools available across rounds instead of stopping after one.
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "Both commands ran; the repo root contains a README.".to_string(),
        ],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "run_command".to_string(),
                arguments_json:
                    "{\"command\":\"pwd\",\"reason\":\"Inspect working directory\"}".to_string(),
            }],
            vec![ToolCall {
                id: "call_2".to_string(),
                name: "run_command".to_string(),
                arguments_json:
                    "{\"command\":\"ls\",\"reason\":\"List repository contents\"}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "What does this project contain?",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert!(result.command_proposal.is_none());
    assert!(result.response.contains("Both commands ran"));

    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    // user, (assistant summary + tool result) for `pwd`, (assistant summary
    // + tool result) for `ls`, final assistant answer.
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[0].role, "user");
    assert!(messages[1].content.contains("pwd"));
    assert_eq!(messages[2].role, "tool");
    assert!(messages[2].content.contains("Command: pwd"));
    assert!(messages[3].content.contains("ls"));
    assert_eq!(messages[4].role, "tool");
    assert!(messages[4].content.contains("Command: ls"));
    assert_eq!(messages[5].role, "assistant");
    assert!(messages[5].content.contains("Both commands ran"));

    fs::remove_dir_all(repo).unwrap();
}

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
    }
}

#[test]
fn chat_dispatches_propose_patch_tool_call_and_returns_reviewable_patch() {
    let repo = temp_dir("chat-propose-patch");
    write_fixture(&repo, "README.md", "# Chat propose patch test\n");
    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "I've prepared the patch for review.".to_string()],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "propose_patch".to_string(),
                arguments_json: "{\"summary\":\"Add greeting helper\",\"files\":[{\"path\":\"src/greeting.rs\",\"content\":\"pub fn greet() -> &'static str { \\\"hi\\\" }\\n\"}]}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "Add a greeting helper function",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    let proposal = result.patch_proposal.expect("expected a patch proposal");
    assert_eq!(proposal.summary, "Add greeting helper");
    assert_eq!(proposal.files.len(), 1);
    assert_eq!(proposal.files[0].path, "src/greeting.rs");
    assert_eq!(result.task.status.as_str(), "waiting_for_approval");
    assert!(result.command_proposal.is_none());

    // The tool-call path must produce a patch that applies exactly like one
    // from the text-envelope `propose_edit` flow.
    let apply_result = engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &proposal.patch_id, None, None, "test_user")
        .unwrap();
    assert_eq!(
        apply_result.applied_files,
        vec!["src/greeting.rs".to_string()]
    );
    let written = fs::read_to_string(repo.join("src/greeting.rs")).unwrap();
    assert!(written.contains("pub fn greet"));

    fs::remove_dir_all(repo).unwrap();
}

// When the provider stops at its output-token ceiling (finish_reason
// "length"), a `propose_patch` call arrives with its `arguments` JSON cut off
// mid-string and cannot be decoded. That must not end the turn silently — the
// old behavior marked the task complete with only the model's lead-in prose
// ("Let me create all the necessary files:") and no patch. The failure is fed
// back so the model can retry with a smaller call in a later round.
#[test]
fn chat_truncated_propose_patch_tool_call_is_fed_back_for_retry() {
    let repo = temp_dir("chat-propose-patch-truncated");
    write_fixture(&repo, "README.md", "# Chat propose patch truncated test\n");
    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            "Let me create all the necessary files:".to_string(),
            "I've prepared a smaller patch for review.".to_string(),
        ],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "propose_patch".to_string(),
                // Cut off mid-`content`, exactly as a `length` finish produces.
                arguments_json:
                    "{\"summary\":\"Scaffold Vite app\",\"files\":[{\"path\":\"package.json\",\"content\":\"{\\n  \\\"name\\\": \\\"snake"
                        .to_string(),
            }],
            vec![ToolCall {
                id: "call_2".to_string(),
                name: "propose_patch".to_string(),
                arguments_json:
                    "{\"summary\":\"Add package.json\",\"files\":[{\"path\":\"package.json\",\"content\":\"{}\\n\"}]}"
                        .to_string(),
            }],
        ],
    )
    .with_truncated(vec![true, false]);
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "Scaffold a TypeScript + Vite project",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    // The retry's patch survives instead of the turn dying on the truncation.
    let proposal = result.patch_proposal.expect("expected a patch proposal");
    assert_eq!(proposal.summary, "Add package.json");
    assert_eq!(proposal.files.len(), 1);
    assert_eq!(proposal.files[0].path, "package.json");
    assert_eq!(result.task.status.as_str(), "waiting_for_approval");

    // The dropped call is recorded rather than vanishing, and the feedback
    // names truncation as the cause so the model shrinks the retry.
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    let note = messages
        .iter()
        .find(|message| message.role == "tool" && message.content.contains("propose_patch"))
        .expect("expected the undecodable call to be recorded");
    assert!(note.content.contains("maximum output length"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_propose_patch_tool_call_with_restricted_path_feeds_error_back_for_retry() {
    let repo = temp_dir("chat-propose-patch-restricted");
    write_fixture(&repo, "README.md", "# Chat propose patch restricted test\n");
    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "Wrote the change to an allowed file instead.".to_string(),
        ],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "propose_patch".to_string(),
                arguments_json: "{\"summary\":\"Store a secret\",\"files\":[{\"path\":\".env\",\"content\":\"SECRET=1\\n\"}]}".to_string(),
            }],
            vec![ToolCall {
                id: "call_2".to_string(),
                name: "propose_patch".to_string(),
                arguments_json: "{\"summary\":\"Add config helper\",\"files\":[{\"path\":\"src/config.rs\",\"content\":\"pub const NAME: &str = \\\"demo\\\";\\n\"}]}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "Add a config helper",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    let proposal = result
        .patch_proposal
        .expect("expected a patch proposal from the corrected retry");
    assert_eq!(proposal.files[0].path, "src/config.rs");

    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    assert!(messages.iter().any(|message| {
        message.role == "tool" && message.content.contains("Cannot propose that patch")
    }));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_dispatches_read_file_tool_call_and_feeds_content_back() {
    let repo = temp_dir("chat-read-file");
    write_fixture(&repo, "docs/notes.md", "The answer is 42.\n");
    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "The notes say the answer is 42.".to_string()],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments_json: "{\"path\":\"docs/notes.md\"}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "What do the notes say?",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert!(result.response.contains("42"));
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    assert!(messages.iter().any(|message| {
        message.role == "tool" && message.content.contains("The answer is 42")
    }));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_read_file_tool_call_on_restricted_path_feeds_error_back() {
    let repo = temp_dir("chat-read-file-restricted");
    write_fixture(&repo, ".env", "SECRET=1\n");
    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "I can't access that file.".to_string()],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments_json: "{\"path\":\".env\"}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "What's in the .env file?",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.role == "tool" && message.content.contains("Cannot read"))
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_dispatches_search_codebase_tool_call() {
    let repo = temp_dir("chat-search-codebase");
    write_fixture(&repo, "src/auth.rs", "fn login() {}\n");
    write_fixture(&repo, "src/other.rs", "fn unrelated() {}\n");
    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Login logic is in src/auth.rs.".to_string()],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "search_codebase".to_string(),
                arguments_json: "{\"query\":\"login\"}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "Where is login handled?",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert!(result.response.contains("src/auth.rs"));
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.role == "tool" && message.content.contains("auth.rs"))
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_dispatches_git_status_and_git_diff_tool_calls() {
    let repo = temp_dir("chat-git-tools");
    write_fixture(&repo, "README.md", "hello\n");
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["add", "README.md"]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=Damaian Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "baseline",
        ],
    );
    fs::write(repo.join("README.md"), "hello world\n").unwrap();

    let mut config = test_config(&repo);
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "README.md has uncommitted changes.".to_string(),
        ],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "read_git_status".to_string(),
                arguments_json: "{}".to_string(),
            }],
            vec![ToolCall {
                id: "call_2".to_string(),
                name: "read_git_diff".to_string(),
                arguments_json: "{\"staged\":false}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "What changed in the working tree?",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert!(result.response.contains("README.md"));
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.role == "tool" && message.content.contains("README.md"))
    );
    assert!(messages.iter().any(|message| {
        message.role == "tool" && message.content.contains("+hello world")
    }));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_returns_command_approval_when_command_exits_sandbox() {
    let repo = temp_dir("chat-command-approval");
    write_fixture(&repo, "README.md", "# Chat command approval\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new(
        "DAMAIAN_COMMAND_V1\nCOMMAND: npm test\nREASON: Run project tests.\nEND_COMMAND\n",
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(&repo, "Run the tests.", &[], &mut adapter, &mut on_token)
        .unwrap();

    let proposal = result
        .command_proposal
        .expect("approval-required command should create proposal metadata");
    assert_eq!(proposal.command, "npm test");
    assert!(proposal.requires_approval);
    assert!(result.response.contains("approval"));
    // The task is paused awaiting a human decision, not finished — a prior
    // bug marked it Complete even though the model never got to answer.
    assert_eq!(result.task.status.as_str(), "waiting_for_approval");
    assert!(
        engine
            .chat_orchestrator
            .has_pending_chat_command(&proposal.id)
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_resumes_after_command_approval_and_answers_using_the_result() {
    let repo = temp_dir("chat-resume-approve");
    write_fixture(&repo, "README.md", "# Chat resume test\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    // A generic, unclassified command lands in the policy's catch-all
    // bucket (`requires_approval: true` regardless of config flags) while
    // still being something every test machine can actually execute.
    let mut adapter = MockModelAdapter::new_sequence(vec![
        "DAMAIAN_COMMAND_V1\nCOMMAND: echo hello-from-sandbox\nREASON: Verify sandbox execution.\nEND_COMMAND\n"
            .to_string(),
        "The sandbox printed the expected marker.".to_string(),
    ]);
    let mut on_token = |_token: &str| {};

    let first = engine
        .chat_orchestrator
        .ask(&repo, "Print a marker.", &[], &mut adapter, &mut on_token)
        .unwrap();
    let proposal = first
        .command_proposal
        .expect("unclassified command should require approval");
    assert!(proposal.requires_approval);

    let resumed = engine
        .chat_orchestrator
        .resume_after_command_decision(&proposal.id, true, "tester", &mut adapter, &mut on_token)
        .unwrap();

    assert!(resumed.command_proposal.is_none());
    assert!(resumed.response.contains("expected marker"));
    assert_eq!(resumed.task.status.as_str(), "complete");
    assert!(
        !engine
            .chat_orchestrator
            .has_pending_chat_command(&proposal.id)
    );

    let messages = engine
        .session_store
        .read_messages(&resumed.session.id)
        .unwrap();
    // user, "needs approval" notice (from the first turn), tool-call
    // summary, tool result, final assistant answer (from the resumed turn).
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].role, "user");
    assert!(messages[1].content.contains("approval"));
    assert_eq!(messages[3].role, "tool");
    assert!(messages[3].content.contains("hello-from-sandbox"));
    assert_eq!(messages[4].content, resumed.response);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn chat_resumes_after_command_rejection_and_answers_without_it() {
    let repo = temp_dir("chat-resume-reject");
    write_fixture(&repo, "README.md", "# Chat resume reject test\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new_sequence(vec![
        "DAMAIAN_COMMAND_V1\nCOMMAND: echo hello-from-sandbox\nREASON: Verify sandbox execution.\nEND_COMMAND\n"
            .to_string(),
        "Understood, I'll answer without running that command.".to_string(),
    ]);
    let mut on_token = |_token: &str| {};

    let first = engine
        .chat_orchestrator
        .ask(&repo, "Print a marker.", &[], &mut adapter, &mut on_token)
        .unwrap();
    let proposal = first
        .command_proposal
        .expect("unclassified command should require approval");

    let resumed = engine
        .chat_orchestrator
        .resume_after_command_decision(&proposal.id, false, "tester", &mut adapter, &mut on_token)
        .unwrap();

    assert!(resumed.command_proposal.is_none());
    assert!(resumed.response.contains("answer without running"));
    assert_eq!(resumed.task.status.as_str(), "complete");
    assert!(
        !engine
            .chat_orchestrator
            .has_pending_chat_command(&proposal.id)
    );

    let messages = engine
        .session_store
        .read_messages(&resumed.session.id)
        .unwrap();
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[3].role, "tool");
    assert!(messages[3].content.contains("declined"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn attaches_unique_file_mentions_to_chat_context() {
    let repo = temp_dir("chat-file-mentions");
    write_fixture(&repo, "README.md", "# Chat test\n");
    write_fixture(
        &repo,
        "docs/USER_GUIDE.md",
        "# User guide\n\nDesktop setup and runtime notes.\n",
    );
    let config = test_config(&repo);
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new("The guide is available in docs/USER_GUIDE.md.");
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(
            &repo,
            "Check USER_GUIDE.md for correctness against current implementation.",
            &[],
            &mut adapter,
            &mut on_token,
        )
        .unwrap();

    assert!(
        result
            .context_files
            .contains(&"docs/USER_GUIDE.md".to_string())
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn builds_openai_request_json_and_extracts_stream_tokens() {
    let request = ModelRequest {
        provider: "openai".to_string(),
        model: "test-model".to_string(),
        messages: vec![ModelMessage::user("hello \"repo\"")],
        temperature: Some("0".to_string()),
        reasoning_level: Some("high".to_string()),
        stream: true,
        tools: None,
        max_tokens: None,
    };
    let body = model_request_json(&request);
    assert!(body.contains("\"model\":\"test-model\""));
    assert!(body.contains("hello \\\"repo\\\""));
    assert!(body.contains("\"reasoning_effort\":\"high\""));

    let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" repo — ok\"}}]}\n\ndata: [DONE]\n\n";
    assert_eq!(extract_model_tokens(raw), vec!["Hello", " repo — ok"]);
}

#[test]
fn reports_openai_compatible_error_payloads() {
    let request = ModelRequest {
        provider: "deepseek".to_string(),
        model: "test-model".to_string(),
        messages: vec![ModelMessage::user("hello")],
        temperature: Some("0".to_string()),
        reasoning_level: Some("high".to_string()),
        stream: true,
        tools: None,
        max_tokens: None,
    };
    let body = model_request_json(&request);
    assert!(!body.contains("reasoning_effort"));
    let transport = MockModelTransport::new("{\"error\":{\"message\":\"Rate limit exceeded\"}}\n");
    let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
    let error = adapter
        .stream_response(&request, &mut |_token| {})
        .unwrap_err();
    assert!(error.to_string().contains("Rate limit exceeded"));
}

#[test]
fn parses_generated_edit_envelope() {
    let raw = "DAMAIAN_EDIT_V1\nSUMMARY: Update greeting\nFILE: src/app.js\nSTATUS: modified\nCONTENT:\nexport const greeting = 'hi';\nEND_FILE\nEND_PATCH\n";
    let edit = parse_generated_edit(raw).unwrap();

    assert_eq!(edit.summary, "Update greeting");
    assert_eq!(edit.changes.len(), 1);
    assert_eq!(edit.changes[0].path, "src/app.js");
    assert_eq!(
        edit.changes[0].new_content,
        "export const greeting = 'hi';\n"
    );
}

#[test]
fn proposes_edit_stores_patch_and_applies_selected_files() {
    let repo = temp_dir("edit-apply");
    write_fixture(&repo, "src/a.js", "export const a = 1;\n");
    write_fixture(&repo, "src/b.js", "export const b = 1;\n");
    let config = test_config(&repo);
    let engine = WorkspaceEngine::new(config);
    let response = "DAMAIAN_EDIT_V1\nSUMMARY: Update constants\nFILE: src/a.js\nSTATUS: modified\nCONTENT:\nexport const a = 2;\nEND_FILE\nFILE: src/b.js\nSTATUS: modified\nCONTENT:\nexport const b = 2;\nEND_FILE\nEND_PATCH\n";
    let mut adapter = MockModelAdapter::new(response);

    let proposal = engine
        .edit_orchestrator
        .propose_edit(&repo, "Update constants", &[], &mut adapter)
        .unwrap();

    assert_eq!(proposal.patch.files.len(), 2);
    assert!(
        proposal.patch.files[0]
            .diff
            .contains("-export const a = 1;")
    );
    assert!(
        proposal.patch.files[0]
            .diff
            .contains("+export const a = 2;")
    );

    let approved = vec!["src/a.js".to_string()];
    let result = engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &proposal.patch.id, Some(&approved), None, "tester")
        .unwrap();

    assert_eq!(result.applied_files, vec!["src/a.js"]);
    assert_eq!(
        fs::read_to_string(repo.join("src/a.js")).unwrap(),
        "export const a = 2;\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("src/b.js")).unwrap(),
        "export const b = 1;\n"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn rejects_selected_patch_files_without_modifying_workspace() {
    let repo = temp_dir("edit-reject-selected");
    write_fixture(&repo, "src/a.js", "export const a = 1;\n");
    write_fixture(&repo, "src/b.js", "export const b = 1;\n");
    let config = test_config(&repo);
    let engine = WorkspaceEngine::new(config);
    let response = "DAMAIAN_EDIT_V1\nSUMMARY: Update constants\nFILE: src/a.js\nSTATUS: modified\nCONTENT:\nexport const a = 2;\nEND_FILE\nFILE: src/b.js\nSTATUS: modified\nCONTENT:\nexport const b = 2;\nEND_FILE\nEND_PATCH\n";
    let mut adapter = MockModelAdapter::new(response);
    let proposal = engine
        .edit_orchestrator
        .propose_edit(&repo, "Update constants", &[], &mut adapter)
        .unwrap();

    let rejected = vec!["src/b.js".to_string()];
    let rejected_path = engine
        .edit_orchestrator
        .reject_stored_patch_files(&proposal.patch.id, &rejected, "tester")
        .unwrap();
    let rejection_record = fs::read_to_string(rejected_path).unwrap();
    assert!(rejection_record.contains("REJECTED_PATH"));
    assert!(rejection_record.contains("src/b.js"));
    assert_eq!(
        fs::read_to_string(repo.join("src/a.js")).unwrap(),
        "export const a = 1;\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("src/b.js")).unwrap(),
        "export const b = 1;\n"
    );

    let approved = vec!["src/a.js".to_string()];
    let result = engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &proposal.patch.id, Some(&approved), None, "tester")
        .unwrap();
    assert_eq!(result.applied_files, vec!["src/a.js"]);
    assert_eq!(
        fs::read_to_string(repo.join("src/a.js")).unwrap(),
        "export const a = 2;\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("src/b.js")).unwrap(),
        "export const b = 1;\n"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn rejects_unknown_selected_patch_file() {
    let repo = temp_dir("edit-unknown-selected");
    write_fixture(&repo, "src/app.js", "export const value = 1;\n");
    let config = test_config(&repo);
    let engine = WorkspaceEngine::new(config);
    let response = "DAMAIAN_EDIT_V1\nSUMMARY: Update value\nFILE: src/app.js\nSTATUS: modified\nCONTENT:\nexport const value = 2;\nEND_FILE\nEND_PATCH\n";
    let mut adapter = MockModelAdapter::new(response);
    let proposal = engine
        .edit_orchestrator
        .propose_edit(&repo, "Update value", &[], &mut adapter)
        .unwrap();
    let approved = vec!["src/app.js".to_string(), "src/missing.js".to_string()];

    let error = engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &proposal.patch.id, Some(&approved), None, "tester")
        .unwrap_err();
    assert!(matches!(error, ClientError::InvalidInput(_)));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn rejects_stored_patch_without_modifying_workspace() {
    let repo = temp_dir("edit-reject");
    write_fixture(&repo, "src/app.js", "export const value = 1;\n");
    let config = test_config(&repo);
    let engine = WorkspaceEngine::new(config);
    let response = "DAMAIAN_EDIT_V1\nSUMMARY: Update value\nFILE: src/app.js\nSTATUS: modified\nCONTENT:\nexport const value = 2;\nEND_FILE\nEND_PATCH\n";
    let mut adapter = MockModelAdapter::new(response);
    let proposal = engine
        .edit_orchestrator
        .propose_edit(&repo, "Update value", &[], &mut adapter)
        .unwrap();

    let rejected_path = engine
        .edit_orchestrator
        .reject_stored_patch(&proposal.patch.id, "tester")
        .unwrap();

    assert!(rejected_path.exists());
    assert_eq!(
        fs::read_to_string(repo.join("src/app.js")).unwrap(),
        "export const value = 1;\n"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn proposes_command_and_requires_approval_for_risky_execution() {
    let repo = temp_dir("command-approval");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "npm test", "Run project tests")
        .unwrap();

    assert_eq!(proposal.risk, CommandRisk::Medium);
    assert!(proposal.requires_approval);

    let error = engine
        .validation_orchestrator
        .run_proposal(&proposal.id, false, "tester")
        .expect_err("approval should be required");
    assert!(matches!(error, ClientError::ApprovalRequired(_)));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn executes_stored_command_and_persists_redacted_output() {
    let repo = temp_dir("command-run");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "printf token=supersecretvalue", "Capture output")
        .unwrap();

    let record = engine
        .validation_orchestrator
        .run_proposal(&proposal.id, true, "tester")
        .unwrap();

    assert_eq!(record.execution.exit_code, Some(0));
    assert!(record.stdout_ref.exists());
    let stdout = fs::read_to_string(record.stdout_ref).unwrap();
    assert!(stdout.contains("[REDACTED_"));
    assert!(!stdout.contains("supersecretvalue"));
    assert!(record.summary_ref.exists());

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn proposes_detected_validation_commands() {
    let repo = temp_dir("validation-plan");
    write_fixture(
        &repo,
        "package.json",
        "{\"scripts\":{\"test\":\"node --test\",\"lint\":\"eslint .\"}}\n",
    );
    let engine = WorkspaceEngine::new(test_config(&repo));
    let proposals = engine
        .validation_orchestrator
        .propose_detected_validations(&repo)
        .unwrap();

    assert!(
        proposals
            .iter()
            .any(|proposal| proposal.command == "npm test")
    );
    assert!(
        proposals
            .iter()
            .any(|proposal| proposal.command == "npm run lint")
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn rejects_stored_command_without_execution() {
    let repo = temp_dir("command-reject");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "pwd", "Inspect cwd")
        .unwrap();

    let rejected_path = engine
        .validation_orchestrator
        .reject_proposal(&proposal.id, "tester")
        .unwrap();

    assert!(rejected_path.exists());

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn config_overlay_round_trips_policy_values() {
    let root = temp_dir("config-overlay");
    let path = root.join("user.conf");
    let mut overlay = ConfigOverlay::default();
    overlay
        .set("command_allowlist", "npm test|cargo test")
        .unwrap();
    overlay.set("secret_patterns", "INTERNAL_TOKEN").unwrap();
    overlay.set("audit_retention_days", "7").unwrap();
    overlay.save(&path).unwrap();

    let loaded = ConfigOverlay::load(&path).unwrap();
    assert_eq!(
        loaded.command_allowlist,
        Some(vec!["npm test".to_string(), "cargo test".to_string()])
    );
    assert_eq!(
        loaded.secret_patterns,
        Some(vec!["INTERNAL_TOKEN".to_string()])
    );
    assert_eq!(loaded.audit_retention_days, Some(7));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn config_overlay_accepts_model_api_key_references() {
    let env_overlay = ConfigOverlay::parse("model_api_key_env=DEEPSEEK_API_KEY\n").unwrap();
    assert_eq!(
        env_overlay.model_api_key_env,
        Some("DEEPSEEK_API_KEY".to_string())
    );

    let keychain_overlay =
        ConfigOverlay::parse("model_api_key_env=keychain: model-api-key\n").unwrap();
    assert_eq!(
        keychain_overlay.model_api_key_env,
        Some("keychain:model-api-key".to_string())
    );
}

#[test]
fn config_overlay_applies_provider_defaults_and_reasoning_level() {
    let overlay =
        ConfigOverlay::parse("model_provider=deedseek\nmodel_reasoning_level=High\n").unwrap();
    assert_eq!(overlay.model_provider, Some("deepseek".to_string()));
    assert_eq!(overlay.model_reasoning_level, Some("high".to_string()));

    let mut config = Config::default();
    config.apply_overlay(overlay);

    assert_eq!(config.model_provider, "deepseek");
    assert_eq!(config.model_base_url, "https://api.deepseek.com");
    assert_eq!(config.model_api_key_env, "DEEPSEEK_API_KEY");
    // The retired `deepseek-chat` alias is no longer the default; selecting
    // the provider picks a current model, which also carries a real ceiling.
    assert_eq!(config.model_name, "deepseek-v4-flash");
    assert_eq!(config.max_output_tokens(), Some(65_536));
    assert_eq!(config.model_reasoning_level, "high");
}

#[test]
fn default_config_has_no_configured_model_providers() {
    let config = Config::default();

    assert!(config.model_providers.is_empty());
    assert!(!config.to_policy_text().contains("model_provider.openai."));
    assert!(!config.to_policy_text().contains("model_provider.deepseek."));
}

#[test]
fn provider_defaults_preserve_keychain_references() {
    let overlay =
        ConfigOverlay::parse("model_api_key_env=keychain:model-api-key\nmodel_provider=deepseek\n")
            .unwrap();
    let mut config = Config::default();
    config.apply_overlay(overlay);

    assert_eq!(config.model_api_key_env, "keychain:model-api-key");
}

#[test]
fn config_overlay_supports_custom_model_providers() {
    let overlay = ConfigOverlay::parse(
        "model_provider.acme.label=Acme AI\n\
         model_provider.acme.base_url=https://api.acme.test\n\
         model_provider.acme.api_key_env=keychain:acme-ai-key\n\
         model_provider.acme.models=acme-large|acme-fast\n\
         model_provider=acme\n",
    )
    .unwrap();

    let mut config = Config::default();
    config.apply_overlay(overlay.clone());

    assert_eq!(overlay.model_providers.len(), 1);
    assert_eq!(config.model_provider, "acme");
    assert_eq!(config.model_base_url, "https://api.acme.test");
    assert_eq!(config.model_api_key_env, "keychain:acme-ai-key");
    assert_eq!(config.model_name, "acme-large");
    assert_eq!(
        config.model_provider_config("acme").unwrap().models.clone(),
        vec!["acme-large".to_string(), "acme-fast".to_string()]
    );
    assert!(
        config
            .to_policy_text()
            .contains("model_provider.acme.base_url=https://api.acme.test")
    );
}

// DeepSeek's own default is 4096 output tokens, which silently truncates a
// multi-file `propose_patch` call mid-arguments, so the built-in provider
// pins the documented 8192 ceiling instead of leaving the field off.
#[test]
fn deepseek_output_token_ceiling_follows_the_selected_model() {
    let ceiling_for = |model: &str| {
        let mut config = Config::default();
        config.apply_overlay(
            ConfigOverlay::parse(&format!("model_provider=deepseek\nmodel_name={model}\n"))
                .unwrap(),
        );
        config.max_output_tokens()
    };

    // V4 models accept far more than the retired aliases they replaced, so
    // pinning every DeepSeek request to the legacy 8192 would needlessly
    // force multi-file patches into extra round trips.
    assert_eq!(ceiling_for("deepseek-v4-flash"), Some(65_536));
    assert_eq!(ceiling_for("deepseek-v4-pro"), Some(65_536));

    // The legacy aliases keep their real ceiling: too large is a hard API
    // error, which is worse than the extra round trip too small costs.
    assert_eq!(ceiling_for("deepseek-chat"), Some(8_192));
    assert_eq!(ceiling_for("deepseek-reasoner"), Some(8_192));

    // An unrecognized DeepSeek model falls back to the conservative ceiling
    // rather than to no ceiling at all.
    assert_eq!(ceiling_for("deepseek-something-new"), Some(8_192));

    // Providers whose defaults are generous stay unpinned.
    let mut openai = Config::default();
    openai.apply_overlay(ConfigOverlay::parse("model_provider=openai\n").unwrap());
    assert_eq!(openai.max_output_tokens(), None);
}

// The repository-context budget was a hardcoded 16_000 at both `build_context`
// call sites. It now resolves per model, with the old literal as the fallback
// so a config that says nothing behaves exactly as it did before.
#[test]
fn context_token_budget_follows_the_selected_model() {
    let budget_for = |provider: &str, model: &str| {
        let mut config = Config::default();
        config.apply_overlay(
            ConfigOverlay::parse(&format!("model_provider={provider}\nmodel_name={model}\n"))
                .unwrap(),
        );
        config.context_token_budget()
    };

    // A 1M-token context window earns more than the legacy 16k.
    assert_eq!(budget_for("deepseek", "deepseek-v4-flash"), 64_000);
    assert_eq!(budget_for("deepseek", "deepseek-v4-pro"), 64_000);

    // Everything else keeps the previous hardcoded behavior.
    assert_eq!(budget_for("deepseek", "deepseek-chat"), 16_000);
    assert_eq!(budget_for("openai", "gpt-4.1"), 16_000);
    assert_eq!(
        Config::default().context_token_budget(),
        DEFAULT_CONTEXT_TOKEN_BUDGET as usize
    );
}

#[test]
fn configured_context_token_budget_overrides_the_model_default() {
    let mut config = Config::default();
    config.apply_overlay(
        ConfigOverlay::parse(
            "model_provider=deepseek\n\
             model_name=deepseek-v4-flash\n\
             model_provider.deepseek.context_token_budget=200000\n",
        )
        .unwrap(),
    );
    assert_eq!(config.context_token_budget(), 200_000);
    assert!(
        config
            .to_policy_text()
            .contains("model_provider.deepseek.context_token_budget=200000")
    );

    // The two budgets are independent: pinning input must not disturb output.
    assert_eq!(config.max_output_tokens(), Some(65_536));

    assert!(ConfigOverlay::parse("model_provider.deepseek.context_token_budget=0\n").is_err());
    assert!(ConfigOverlay::parse("model_provider.deepseek.context_token_budget=big\n").is_err());
}

// An explicit per-install ceiling outranks the built-in model table, so a user
// on V4 can opt into the full 384000 the model actually allows.
#[test]
fn configured_output_token_ceiling_overrides_the_model_default() {
    let mut config = Config::default();
    config.apply_overlay(
        ConfigOverlay::parse(
            "model_provider=deepseek\n\
             model_name=deepseek-v4-flash\n\
             model_provider.deepseek.max_output_tokens=384000\n",
        )
        .unwrap(),
    );
    assert_eq!(config.max_output_tokens(), Some(384_000));
}

// A user overlay that customizes a built-in provider without mentioning
// max_output_tokens (the common shape: label + models + supports_native_tools)
// must not shadow the built-in ceiling — doing so would silently reinstate
// DeepSeek's 4096-token default and the truncated-patch failure with it.
#[test]
fn partial_provider_overlay_keeps_the_builtin_output_token_ceiling() {
    let mut config = Config::default();
    config.apply_overlay(
        ConfigOverlay::parse(
            "model_provider=deepseek\n\
             model_provider.deepseek.label=DeepSeek\n\
             model_provider.deepseek.models=deepseek-chat|deepseek-reasoner\n\
             model_provider.deepseek.supports_native_tools=true\n",
        )
        .unwrap(),
    );

    assert!(config.supports_native_tools());
    assert_eq!(config.max_output_tokens(), Some(8192));
}

#[test]
fn model_provider_output_token_ceiling_round_trips_through_overlay() {
    let mut config = Config::default();
    config.apply_overlay(
        ConfigOverlay::parse(
            "model_provider.deepseek.max_output_tokens=4096\n\
             model_provider=deepseek\n",
        )
        .unwrap(),
    );

    assert_eq!(config.max_output_tokens(), Some(4096));
    assert!(
        config
            .to_policy_text()
            .contains("model_provider.deepseek.max_output_tokens=4096")
    );

    assert!(
        ConfigOverlay::parse("model_provider.deepseek.max_output_tokens=0\n").is_err(),
        "zero is not a usable ceiling"
    );
    assert!(ConfigOverlay::parse("model_provider.deepseek.max_output_tokens=lots\n").is_err());
}

#[test]
fn mcp_server_config_round_trips_through_overlay() {
    let overlay = ConfigOverlay::parse(
        "mcp_server.filesystem.transport=stdio\n\
         mcp_server.filesystem.command=npx\n\
         mcp_server.filesystem.args=-y|@modelcontextprotocol/server-filesystem|/tmp\n\
         mcp_server.filesystem.env=LOG_LEVEL=warn|NODE_ENV=production\n\
         mcp_server.filesystem.enabled=true\n\
         mcp_server.sentry.transport=http\n\
         mcp_server.sentry.url=https://mcp.sentry.example/mcp\n\
         mcp_server.sentry.auth_token_env=keychain:mcp-sentry-token\n\
         mcp_server.sentry.enabled=false\n",
    )
    .unwrap();

    let mut config = Config::default();
    config.apply_overlay(overlay);

    let filesystem = config.mcp_server_config("filesystem").unwrap();
    assert_eq!(filesystem.transport, McpTransport::Stdio);
    assert_eq!(filesystem.command, "npx");
    assert_eq!(
        filesystem.args,
        vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-filesystem".to_string(),
            "/tmp".to_string()
        ]
    );
    assert_eq!(
        filesystem.env,
        vec![
            ("LOG_LEVEL".to_string(), "warn".to_string()),
            ("NODE_ENV".to_string(), "production".to_string())
        ]
    );
    assert!(filesystem.enabled);
    // Defaults: approval required unless explicitly disabled.
    assert!(filesystem.require_approval);

    let sentry = config.mcp_server_config("sentry").unwrap();
    assert_eq!(sentry.transport, McpTransport::Http);
    assert_eq!(sentry.url, "https://mcp.sentry.example/mcp");
    assert_eq!(sentry.auth_token_env, "keychain:mcp-sentry-token");
    assert!(!sentry.enabled);

    // Only the enabled server is offered.
    let active: Vec<&str> = config
        .active_mcp_servers()
        .iter()
        .map(|server| server.id.as_str())
        .collect();
    assert_eq!(active, vec!["filesystem"]);

    // Serialization is loadable again and preserves the transport-specific keys.
    let policy = config.to_policy_text();
    assert!(policy.contains("mcp_server.filesystem.command=npx"));
    assert!(policy.contains("mcp_server.sentry.url=https://mcp.sentry.example/mcp"));
    let reparsed = ConfigOverlay::parse(&policy).unwrap();
    let mut reloaded = Config::default();
    reloaded.apply_overlay(reparsed);
    assert_eq!(
        reloaded.mcp_server_config("filesystem"),
        config.mcp_server_config("filesystem")
    );
}

#[test]
fn mcp_stdio_client_handshakes_lists_and_calls_tools() {
    // A tiny stdio MCP server: reads newline-delimited JSON-RPC requests and
    // replies with canned results, echoing back the request id.
    let root = temp_dir("mcp-stdio");
    fs::create_dir_all(&root).unwrap();
    let script = root.join("server.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fake","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echoes text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echoed: hi"}]}}\n' "$id" ;;
  esac
done
"#,
    )
    .unwrap();

    let config = McpServerConfig {
        id: "fake".to_string(),
        label: "Fake".to_string(),
        transport: McpTransport::Stdio,
        command: "sh".to_string(),
        args: vec![script.to_string_lossy().to_string()],
        env: Vec::new(),
        url: String::new(),
        auth_token_env: String::new(),
        enabled: true,
        require_approval: true,
    };

    let mut client = McpClient::connect(&config, None).expect("connect + initialize");
    let tools = client.list_tools().expect("tools/list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].to_tool_definition("fake").name, "mcp__fake__echo");

    let result = client
        .call_tool("echo", "{\"text\":\"hi\"}")
        .expect("tools/call");
    assert!(!result.is_error);
    assert_eq!(result.text, "echoed: hi");
}

#[test]
fn mcp_kill_switch_and_allowlist_gate_active_servers() {
    let base = "mcp_server.a.command=a-cmd\n\
                mcp_server.a.enabled=true\n\
                mcp_server.b.command=b-cmd\n\
                mcp_server.b.enabled=true\n";

    // Admin allowlist narrows the active set to listed ids only.
    let mut allowlisted = Config::default();
    allowlisted.apply_overlay(
        ConfigOverlay::parse(&format!("{base}mcp_server_allowlist=a\n")).unwrap(),
    );
    let active: Vec<&str> = allowlisted
        .active_mcp_servers()
        .iter()
        .map(|server| server.id.as_str())
        .collect();
    assert_eq!(active, vec!["a"]);

    // The global kill-switch disables everything regardless of per-server state.
    let mut disabled = Config::default();
    disabled.apply_overlay(ConfigOverlay::parse(&format!("{base}mcp_enabled=false\n")).unwrap());
    assert!(disabled.active_mcp_servers().is_empty());
}

#[test]
fn config_overlay_rejects_literal_model_api_keys() {
    let error = ConfigOverlay::parse("model_api_key_env=sk-test-secret\n").unwrap_err();

    assert!(error.to_string().contains("do not paste the API key"));
}

#[test]
fn config_precedence_is_user_then_repo_then_admin() {
    let root = temp_dir("config-precedence");
    let user = root.join("user.conf");
    let repo = root.join("repo.conf");
    let admin = root.join("admin.conf");
    fs::write(
        &user,
        "model_name=user-model\ncommand_allowlist=npm test\naudit_retention_days=30\n",
    )
    .unwrap();
    fs::write(
        &repo,
        "model_name=repo-model\ncommand_allowlist=cargo test\nsecret_patterns=REPO_SECRET\n",
    )
    .unwrap();
    fs::write(
        &admin,
        "model_name=admin-model\ncommand_blocklist=cargo test\naudit_retention_days=3\n",
    )
    .unwrap();
    let base = Config {
        data_dir: root.join("data"),
        ..Config::default()
    };

    let merged =
        Config::load_with_policy_paths(base, Some(&user), Some(&repo), Some(&admin)).unwrap();

    assert_eq!(merged.model_name, "admin-model");
    assert_eq!(merged.command_allowlist, vec!["cargo test"]);
    assert_eq!(merged.command_blocklist, vec!["cargo test"]);
    assert_eq!(merged.secret_patterns, vec!["REPO_SECRET"]);
    assert_eq!(merged.audit_retention_days, 3);
    assert_eq!(
        CommandPolicy::new(merged)
            .classify("cargo test", &root)
            .risk,
        CommandRisk::Blocked
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn engine_uses_custom_secret_patterns_from_config() {
    let repo = temp_dir("custom-secret-pattern");
    let engine = WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        secret_patterns: vec!["INTERNAL_TOKEN_123".to_string()],
        ..Config::default()
    });
    let redaction = engine.scanner.redact("value=INTERNAL_TOKEN_123");

    assert_eq!(redaction.findings.len(), 1);
    assert_eq!(redaction.findings[0].category, "custom_secret");
    assert!(redaction.text.contains("[REDACTED_CUSTOM_SECRET_"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn audit_can_be_disabled_by_policy() {
    let repo = temp_dir("audit-disabled");
    let engine = WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        audit_enabled: false,
        ..Config::default()
    });
    engine
        .audit_log
        .record(
            "test_event",
            &[("token", "secret=supersecretvalue".to_string())],
        )
        .unwrap();

    assert!(!repo.join(".damaian").join("audit").exists());

    fs::remove_dir_all(repo).unwrap();
}
