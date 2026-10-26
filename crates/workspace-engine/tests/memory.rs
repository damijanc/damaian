//! Spec 28: the memory record, project identity and store.
//!
//! Identity is the part that cannot be proved by reading one file: two clones
//! of one repository must share memory and two unrelated repositories must not,
//! at every resolution tier. These fixtures are real local Git repositories,
//! because the property under test is what Git reports.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    Config, GitReadCaps, GitService, ProjectIdentity, ProjectKeySource, SecretScanner,
    repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-memory-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn test_config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    }
}

fn git_service(config: &Config) -> GitService {
    let scanner = SecretScanner::new(Vec::new());
    let audit = workspace_engine::AuditLog::new(&config.data_dir, false, scanner.clone());
    let path_policy = workspace_engine::PathPolicy::new(config);
    GitService::new(
        audit,
        scanner,
        path_policy,
        GitReadCaps::from_config(config),
    )
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .expect("git should run");
    assert!(
        status.success(),
        "git {args:?} failed in {}",
        root.display()
    );
}

fn init_repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "user.name", "Fixture"]);
}

fn commit(root: &Path, name: &str, contents: &str) {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
    git(root, &["add", name]);
    git(root, &["commit", "-q", "-m", "fixture commit"]);
}

fn repo_with_a_commit(name: &str) -> PathBuf {
    let root = temp_dir(name);
    init_repo(&root);
    // The name goes into the tree so two fixtures never mint the same commit
    // id by sharing content, author and timestamp within one second.
    commit(&root, "README.md", &format!("# Fixture {name}\n"));
    root
}

fn identity(root: &Path) -> ProjectIdentity {
    let config = test_config(root);
    ProjectIdentity::resolve_for_root(root, &git_service(&config)).unwrap()
}

#[test]
fn two_checkouts_of_one_repository_share_a_root_commit_identity() {
    let origin = repo_with_a_commit("identity-origin");
    let clone = temp_dir("identity-clone");
    let status = Command::new("git")
        .args(["clone", "-q"])
        .arg(&origin)
        .arg(&clone)
        .status()
        .unwrap();
    assert!(status.success());

    let first = identity(&origin);
    let second = identity(&clone);

    assert_eq!(first.source, ProjectKeySource::RootCommit);
    assert_eq!(second.source, ProjectKeySource::RootCommit);
    assert_eq!(first.key, second.key);
    assert_eq!(first.key, second.key, "the path must not enter the key");
    assert_ne!(
        first.repository_id, second.repository_id,
        "the path-derived id still differs, which is why memory needs its own key"
    );
}

#[test]
fn a_moved_or_renamed_checkout_keeps_its_identity() {
    let root = repo_with_a_commit("identity-move");
    let before = identity(&root);

    let renamed = root.with_file_name(format!(
        "{}-renamed",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(&root, &renamed).unwrap();
    let after = identity(&renamed);

    assert_eq!(before.key, after.key);
    assert_eq!(before.source, ProjectKeySource::RootCommit);
    assert_ne!(before.repository_id, after.repository_id);
}

#[test]
fn repositories_with_different_root_commits_do_not_share_memory() {
    let first = repo_with_a_commit("identity-first");
    let second = repo_with_a_commit("identity-second");

    let first = identity(&first);
    let second = identity(&second);

    assert_eq!(first.source, ProjectKeySource::RootCommit);
    assert_eq!(second.source, ProjectKeySource::RootCommit);
    assert_ne!(first.key, second.key);
}

#[test]
fn a_fork_shares_its_upstream_root_commit_deliberately() {
    // §5.2: a fork inherits its upstream's root commit, so it shares memory.
    // This is a stated sharing decision, not an accident; the test exists so a
    // change to it is visible.
    let upstream = repo_with_a_commit("identity-fork-upstream");
    let fork = temp_dir("identity-fork");
    assert!(
        Command::new("git")
            .args(["clone", "-q"])
            .arg(&upstream)
            .arg(&fork)
            .status()
            .unwrap()
            .success()
    );

    assert_eq!(identity(&upstream).key, identity(&fork).key);
}

#[test]
fn repositories_without_history_fall_to_the_normalized_remote() {
    let first = temp_dir("identity-remote-first");
    let second = temp_dir("identity-remote-second");
    init_repo(&first);
    init_repo(&second);
    git(
        &first,
        &[
            "remote",
            "add",
            "origin",
            "https://user:secret-token@GitHub.com/Owner/First.git",
        ],
    );
    git(
        &second,
        &["remote", "add", "origin", "git@github.com:owner/second.git"],
    );

    let first = identity(&first);
    let second = identity(&second);

    assert_eq!(first.source, ProjectKeySource::RemoteUrl);
    assert_eq!(second.source, ProjectKeySource::RemoteUrl);
    assert_ne!(first.key, second.key);
    assert!(
        !first.key.contains("secret-token"),
        "no credential in a key"
    );
    assert!(
        first.key.starts_with("remote_") && first.key.len() == "remote_".len() + 16,
        "{}",
        first.key
    );
}

#[test]
fn one_remote_at_two_checkouts_shares_memory_without_history() {
    let first = temp_dir("identity-same-remote-first");
    let second = temp_dir("identity-same-remote-second");
    for root in [&first, &second] {
        init_repo(root);
        git(
            root,
            &["remote", "add", "origin", "git@github.com:owner/shared.git"],
        );
    }

    assert_eq!(identity(&first).key, identity(&second).key);
}

#[test]
fn repositories_with_no_history_and_no_remote_fall_to_the_path_hash() {
    let first = temp_dir("identity-path-first");
    let second = temp_dir("identity-path-second");
    init_repo(&first);
    init_repo(&second);

    let first_identity = identity(&first);
    let second_identity = identity(&second);

    assert_eq!(first_identity.source, ProjectKeySource::RepositoryId);
    assert_eq!(second_identity.source, ProjectKeySource::RepositoryId);
    assert_ne!(first_identity.key, second_identity.key);
    assert_eq!(
        first_identity.key,
        format!("path_{}", repository_id_for_root(&first))
    );
}

#[test]
fn a_non_repository_falls_back_without_failing() {
    let root = temp_dir("identity-not-a-repo");
    let identity = identity(&root);
    assert_eq!(identity.source, ProjectKeySource::RepositoryId);
    assert_eq!(
        ProjectIdentity::source_of_key(&identity.key),
        Some(ProjectKeySource::RepositoryId)
    );
}

#[test]
fn a_project_key_cannot_be_confused_between_tiers() {
    let root = repo_with_a_commit("identity-tier-prefix");
    let identity = identity(&root);
    assert!(identity.key.starts_with("root_"), "{}", identity.key);
    assert_eq!(
        ProjectIdentity::source_of_key(&identity.key),
        Some(ProjectKeySource::RootCommit)
    );
    assert!(!identity.key.contains('/'), "a key is a file name");
}

// ---------------------------------------------------------------------------
// Store layout, schema version and migration (Task 4).
// ---------------------------------------------------------------------------

use workspace_engine::{
    AuditLog, EvidenceRef, MEMORY_SCHEMA_VERSION, MemoryCategory, MemoryEntry, MemoryError,
    MemoryExpiry, MemoryProvenance, MemorySchemaOutcome, MemoryScope, MemoryState, MemoryStore,
    MemoryStoreConfig, Sensitivity, Verification,
};

fn memory_store(root: &Path, user_scope_enabled: bool) -> MemoryStore {
    let config = test_config(root);
    let scanner = SecretScanner::new(Vec::new());
    let audit = AuditLog::new(&config.data_dir, true, scanner);
    MemoryStore::new(
        MemoryStoreConfig {
            data_dir: config.data_dir,
            user_scope_enabled,
            ignore_patterns: Vec::new(),
        },
        audit,
    )
}

fn full_entry(project_key: &str) -> MemoryEntry {
    let mut entry = MemoryEntry::new(
        MemoryScope::Project,
        MemoryCategory::Environment,
        "The integration suite needs Docker running.",
        MemoryProvenance::RepositoryEvidence {
            paths: vec![EvidenceRef::new("docker-compose.yml", "sha256:abc")],
        },
        Verification::EvidenceCurrent,
    )
    .in_project(project_key);
    entry.sensitivity = Sensitivity::Environmental;
    entry.expiry = Some(MemoryExpiry {
        expires_at_ms: Some(1_700_000_000_000),
        revalidate_on_evidence_change: true,
    });
    entry.supersedes = Some("mem_previous".to_string());
    entry.updated_at_ms = entry.created_at_ms + 5;
    entry
}

fn write_lines(path: &Path, lines: &[String]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut content = String::new();
    for line in lines {
        content.push_str(line);
        content.push('\n');
    }
    fs::write(path, content).unwrap();
}

#[test]
fn memory_is_stored_under_the_given_data_directory() {
    let root = temp_dir("store-layout");
    let store = memory_store(&root, false);
    let expected = root.join(".damaian").join("memory");
    assert_eq!(store.memory_dir(), expected);
    assert_eq!(
        store.project_path("root_abc"),
        expected.join("project").join("root_abc.jsonl")
    );
    assert_eq!(store.user_path(), expected.join("user.jsonl"));

    let outcome = store.ensure_schema().unwrap();
    assert_eq!(outcome, MemorySchemaOutcome::Initialized);
    let marker = fs::read_to_string(expected.join("schema.conf")).unwrap();
    assert_eq!(marker, format!("schema_version={MEMORY_SCHEMA_VERSION}\n"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&expected).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "memory directory is private");
    }
}

#[test]
fn a_complete_entry_round_trips_through_the_file() {
    let root = temp_dir("store-round-trip");
    let store = memory_store(&root, false);
    let identity = ProjectIdentity {
        key: "root_roundtrip".to_string(),
        source: ProjectKeySource::RootCommit,
        repository_id: "repo_fixture".to_string(),
    };
    store.ensure_schema().unwrap();

    let entry = full_entry(&identity.key);
    let line = serde_json::to_string(&entry).unwrap();
    write_lines(&store.project_path(&identity.key), &[line]);

    let loaded = store.project_entries(&identity).unwrap();
    assert_eq!(loaded, vec![entry]);
}

#[test]
fn the_newest_record_for_an_id_wins_without_rewriting_the_file() {
    let root = temp_dir("store-newest");
    let store = memory_store(&root, false);
    let identity = ProjectIdentity {
        key: "root_newest".to_string(),
        source: ProjectKeySource::RootCommit,
        repository_id: "repo_fixture".to_string(),
    };
    store.ensure_schema().unwrap();

    let mut entry = full_entry(&identity.key);
    let first = serde_json::to_string(&entry).unwrap();
    entry.state = MemoryState::Deleted;
    let second = serde_json::to_string(&entry).unwrap();
    let path = store.project_path(&identity.key);
    write_lines(&path, &[first, second]);

    let loaded = store.project_entries(&identity).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].state, MemoryState::Deleted);
    assert_eq!(
        fs::read_to_string(&path).unwrap().lines().count(),
        2,
        "appends keep history; nothing was truncated"
    );
}

#[test]
fn a_torn_final_line_is_discarded_and_the_rest_replays() {
    let root = temp_dir("store-torn");
    let store = memory_store(&root, false);
    let identity = ProjectIdentity {
        key: "root_torn".to_string(),
        source: ProjectKeySource::RootCommit,
        repository_id: "repo_fixture".to_string(),
    };
    store.ensure_schema().unwrap();

    let entry = full_entry(&identity.key);
    let complete = serde_json::to_string(&entry).unwrap();
    let path = store.project_path(&identity.key);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("{complete}\n{{\"id\":\"mem_torn\"")).unwrap();

    let loaded = store.project_entries(&identity).unwrap();
    assert_eq!(loaded, vec![entry]);
}

#[test]
fn an_unparsable_middle_line_is_skipped_without_losing_the_others() {
    let root = temp_dir("store-middle");
    let store = memory_store(&root, false);
    let identity = ProjectIdentity {
        key: "root_middle".to_string(),
        source: ProjectKeySource::RootCommit,
        repository_id: "repo_fixture".to_string(),
    };
    store.ensure_schema().unwrap();

    let entry = full_entry(&identity.key);
    write_lines(
        &store.project_path(&identity.key),
        &[
            "not json at all".to_string(),
            serde_json::to_string(&entry).unwrap(),
        ],
    );

    assert_eq!(store.project_entries(&identity).unwrap(), vec![entry]);
}

#[test]
fn a_version_zero_store_is_migrated_by_appending_then_read() {
    let root = temp_dir("store-migrate-zero");
    let store = memory_store(&root, false);
    let identity = ProjectIdentity {
        key: "root_legacy".to_string(),
        source: ProjectKeySource::RootCommit,
        repository_id: "repo_fixture".to_string(),
    };

    let mut legacy = serde_json::to_value(full_entry(&identity.key)).unwrap();
    let object = legacy.as_object_mut().unwrap();
    object.remove("sensitivity");
    object.remove("state");
    // The old record's statement names a path, so the derived value must be
    // Environmental even though no sensitivity was recorded.
    object.insert(
        "statement".to_string(),
        serde_json::Value::String("Builds write into /tmp/damaian-build.".to_string()),
    );
    let legacy_line = serde_json::to_string(&legacy).unwrap();
    let path = store.project_path(&identity.key);
    write_lines(&path, &[legacy_line]);

    let loaded = store.project_entries(&identity).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].state, MemoryState::Active);
    assert_eq!(
        loaded[0].sensitivity,
        Sensitivity::Environmental,
        "migration derives sensitivity rather than trusting the old record"
    );
    assert_eq!(
        fs::read_to_string(store.memory_dir().join("schema.conf"))
            .unwrap()
            .trim(),
        format!("schema_version={MEMORY_SCHEMA_VERSION}")
    );
    let lines: Vec<String> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines.len(), 2, "the pre-migration line is retained");
    assert!(lines[0].contains("\"id\""));
    assert!(!lines[0].contains("sensitivity"));
}

#[test]
fn a_newer_schema_refuses_to_load_and_changes_nothing() {
    let root = temp_dir("store-newer");
    let store = memory_store(&root, false);
    let identity = ProjectIdentity {
        key: "root_newer".to_string(),
        source: ProjectKeySource::RootCommit,
        repository_id: "repo_fixture".to_string(),
    };
    store.ensure_schema().unwrap();
    let path = store.project_path(&identity.key);
    write_lines(
        &path,
        &[serde_json::to_string(&full_entry(&identity.key)).unwrap()],
    );
    let before = fs::read(&path).unwrap();
    let marker = store.memory_dir().join("schema.conf");
    fs::write(&marker, "schema_version=2\n").unwrap();

    let error = store.project_entries(&identity).unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Schema { found, supported, .. }
            if found == "2" && supported == MEMORY_SCHEMA_VERSION
    ));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_to_string(&marker).unwrap(), "schema_version=2\n");
}

#[test]
fn an_unparsable_schema_refuses_and_reports_what_it_found() {
    let root = temp_dir("store-garbled");
    let store = memory_store(&root, false);
    store.ensure_schema().unwrap();
    let marker = store.memory_dir().join("schema.conf");
    fs::write(&marker, "schema_version=banana\n").unwrap();

    let error = store.ensure_schema().unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Schema { ref found, .. } if found == "banana"
    ));
    assert!(error.to_string().contains("Nothing in it was changed"));
}

#[test]
fn a_project_store_claiming_another_project_is_refused() {
    let root = temp_dir("store-cross-project");
    let store = memory_store(&root, false);
    let identity = ProjectIdentity {
        key: "root_mine".to_string(),
        source: ProjectKeySource::RootCommit,
        repository_id: "repo_fixture".to_string(),
    };
    store.ensure_schema().unwrap();
    let entry = full_entry("root_someone_else");
    write_lines(
        &store.project_path(&identity.key),
        &[serde_json::to_string(&entry).unwrap()],
    );

    let error = store.project_entries(&identity).unwrap_err();
    assert!(matches!(error, MemoryError::Invalid(_)));
    assert!(error.to_string().contains("cross a project boundary"));
}

#[test]
fn the_store_follows_config_default_data_dir() {
    let root = temp_dir("store-data-dir-env");
    let previous = std::env::var_os("DAMAIAN_DATA_DIR");
    // Nextest runs each test in its own process, so this cannot race another
    // test; the guard restores the value for a plain threaded `cargo test`.
    unsafe { std::env::set_var("DAMAIAN_DATA_DIR", root.join("redirected")) };
    let data_dir = Config::default_data_dir();
    match previous {
        Some(value) => unsafe { std::env::set_var("DAMAIAN_DATA_DIR", value) },
        None => unsafe { std::env::remove_var("DAMAIAN_DATA_DIR") },
    }

    assert_eq!(data_dir, root.join("redirected"));
    let scanner = SecretScanner::new(Vec::new());
    let audit = AuditLog::new(&data_dir, true, scanner);
    let store = MemoryStore::new(
        MemoryStoreConfig {
            data_dir: data_dir.clone(),
            user_scope_enabled: false,
            ignore_patterns: Vec::new(),
        },
        audit,
    );
    store.ensure_schema().unwrap();
    assert!(data_dir.join("memory").join("schema.conf").exists());
}
