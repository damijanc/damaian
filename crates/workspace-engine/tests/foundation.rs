use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    AuditLog, ClientError, CommandPolicy, CommandRisk, Config, ConfigOverlay, IndexCache,
    MockModelAdapter, MockModelTransport, ModelAdapter, ModelMessage, ModelProviderConfig,
    ModelRequest, OpenAICompatibleAdapter, PatchEngine, PatchStore, PathPolicy, ProjectIndexer,
    ProposedChange, SecretScanner, SessionStore, ToolCall, WorkspaceEngine, extract_model_tokens,
    model_request_json, parse_generated_edit,
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
        .resolve_existing(&repo, "linked-secret.txt")
        .expect_err("symlink should be denied");
    assert!(matches!(error, ClientError::AccessDenied(_)));

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

    assert_eq!(policy.classify("git status --short").risk, CommandRisk::Low);
    assert_eq!(policy.classify("git show --stat").risk, CommandRisk::Low);
    assert_eq!(policy.classify("npm test").risk, CommandRisk::Medium);
    assert_eq!(policy.classify("rm -rf .").risk, CommandRisk::Blocked);
    assert_eq!(policy.classify("ls | head").risk, CommandRisk::High);
}

#[test]
fn allowlist_does_not_bypass_shell_control_detection() {
    let policy = CommandPolicy::new(Config {
        data_dir: PathBuf::from("/tmp/damaian-test"),
        command_allowlist: vec!["npm test".to_string()],
        ..Config::default()
    });

    assert_eq!(policy.classify("npm test").risk, CommandRisk::Low);
    for command in ["npm test; rm -rf ~", "npm test\ncat /etc/passwd"] {
        let classification = policy.classify(command);
        assert_eq!(classification.risk, CommandRisk::High);
        assert!(classification.requires_approval);
    }
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
    assert_eq!(messages.len(), 2);
    assert!(messages[1].content.contains("sandbox command completed"));

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
    assert_eq!(config.model_name, "deepseek-chat");
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
        CommandPolicy::new(merged).classify("cargo test").risk,
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
