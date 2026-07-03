use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    AuditLog, ClientError, CommandPolicy, CommandRisk, Config, PatchEngine, PathPolicy,
    ProjectIndexer, ProposedChange, SecretScanner,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

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
fn marks_restricted_dotenv_files() {
    let policy = PathPolicy::unrestricted();
    assert!(policy.is_restricted(".env", false));
    assert!(!policy.is_restricted("src/app.js", false));
}

#[test]
fn indexes_source_files_while_respecting_gitignore_and_secrets() {
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

    assert_eq!(files, vec!["src/auth.js"]);
    assert!(
        index
            .skipped
            .iter()
            .any(|file| file.path == "src/secret.js" && file.reason == "contains_secret")
    );
    assert_eq!(index.keyword_search("login", 1)[0].path, "src/auth.js");

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn classifies_command_risk() {
    let policy = CommandPolicy::new(Config {
        data_dir: PathBuf::from("/tmp/damaian-test"),
        ..Config::default()
    });

    assert_eq!(policy.classify("git status --short").risk, CommandRisk::Low);
    assert_eq!(policy.classify("npm test").risk, CommandRisk::Medium);
    assert_eq!(policy.classify("rm -rf .").risk, CommandRisk::Blocked);
    assert_eq!(policy.classify("ls | head").risk, CommandRisk::High);
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
        .apply_patch(&repo, &patch, None, "tester", false)
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
        .apply_patch(&repo, &patch, None, "tester", false)
        .unwrap();
    assert_eq!(result.applied_files, vec!["src/features/new-file.js"]);
    assert_eq!(
        fs::read_to_string(repo.join("src/features/new-file.js")).unwrap(),
        "export const ready = true;\n"
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
        .apply_patch(&repo, &patch, None, "tester", false)
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
        .apply_patch(&repo, &patch, None, "tester", false)
        .expect_err("secret should block apply");
    assert!(matches!(error, ClientError::PolicyBlocked(_)));

    fs::remove_dir_all(repo).unwrap();
}
