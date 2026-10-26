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
    memory_store_with(root, user_scope_enabled, |_| {})
}

fn memory_store_with(
    root: &Path,
    user_scope_enabled: bool,
    tweak: impl FnOnce(&mut Config),
) -> MemoryStore {
    let mut config = test_config(root);
    tweak(&mut config);
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let audit = AuditLog::new(&config.data_dir, true, scanner.clone());
    MemoryStore::new(
        MemoryStoreConfig {
            data_dir: config.data_dir.clone(),
            user_scope_enabled,
            ignore_patterns: config.ignore_patterns.clone(),
        },
        audit,
        scanner,
        workspace_engine::PathPolicy::new(&config),
        workspace_engine::SessionStore::new(&config.data_dir),
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
    let audit = AuditLog::new(&data_dir, true, scanner.clone());
    let store = MemoryStore::new(
        MemoryStoreConfig {
            data_dir: data_dir.clone(),
            user_scope_enabled: false,
            ignore_patterns: Vec::new(),
        },
        audit,
        scanner,
        workspace_engine::PathPolicy::unrestricted(),
        workspace_engine::SessionStore::new(&data_dir),
    );
    store.ensure_schema().unwrap();
    assert!(data_dir.join("memory").join("schema.conf").exists());
}

// ---------------------------------------------------------------------------
// Refusal path: secrets, evidence and scopes (Task 5).
// ---------------------------------------------------------------------------

use workspace_engine::{EvidenceRefusal, MemoryRefusal, SessionStore};

fn project_identity(key: &str) -> ProjectIdentity {
    ProjectIdentity {
        key: key.to_string(),
        source: ProjectKeySource::RootCommit,
        repository_id: format!("repo_{key}"),
    }
}

fn project_entry(key: &str, statement: &str) -> MemoryEntry {
    MemoryEntry::new(
        MemoryScope::Project,
        MemoryCategory::Convention,
        statement,
        MemoryProvenance::UserRequest {
            session_id: "session_fixture".to_string(),
            message_id: "message_fixture".to_string(),
        },
        Verification::UserConfirmed,
    )
    .in_project(key)
}

fn user_entry(statement: &str) -> MemoryEntry {
    MemoryEntry::new(
        MemoryScope::User,
        MemoryCategory::Preference,
        statement,
        MemoryProvenance::UserRequest {
            session_id: "session_fixture".to_string(),
            message_id: "message_fixture".to_string(),
        },
        Verification::UserConfirmed,
    )
}

fn session_entry(session_id: &str, statement: &str) -> MemoryEntry {
    MemoryEntry::new(
        MemoryScope::Session,
        MemoryCategory::Fact,
        statement,
        MemoryProvenance::UserRequest {
            session_id: session_id.to_string(),
            message_id: "message_fixture".to_string(),
        },
        Verification::UserConfirmed,
    )
    .in_session(session_id)
}

const SEEDED_SECRET: &str = "AKIAIOSFODNN7EXAMPLE";

#[test]
fn an_entry_written_through_insert_round_trips_with_every_field() {
    let root = temp_dir("insert-round-trip");
    let store = memory_store(&root, false);
    let identity = project_identity("root_insert_round_trip");
    let mut entry = project_entry(&identity.key, "Builds write into /tmp/damaian-build.");
    entry.category = MemoryCategory::Environment;
    entry.verification = Verification::EvidenceCurrent;
    entry.provenance = MemoryProvenance::RepositoryEvidence {
        paths: vec![EvidenceRef::new("README.md", "sha256:abc")],
    };
    entry.expiry = Some(MemoryExpiry {
        expires_at_ms: Some(1_700_000_000_000),
        revalidate_on_evidence_change: true,
    });
    entry.supersedes = Some("mem_previous".to_string());

    let stored = store.insert(&root, Some(&identity), entry).unwrap();
    assert_eq!(stored.sensitivity, Sensitivity::Environmental);
    assert_eq!(store.project_entries(&identity).unwrap(), vec![stored]);
}

#[test]
fn a_detected_secret_is_refused_rather_than_stored_redacted() {
    let root = temp_dir("refuse-secret");
    let store = memory_store(&root, false);
    let identity = project_identity("root_refuse_secret");
    let entry = project_entry(
        &identity.key,
        &format!("The deploy key {SEEDED_SECRET} is set on the runner."),
    );

    let error = store.insert(&root, Some(&identity), entry).unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::Secret { .. })
    ));
    assert!(error.to_string().contains("will not store it"));
    assert!(!error.to_string().contains(SEEDED_SECRET));
    assert!(
        store.project_entries(&identity).unwrap().is_empty(),
        "nothing was written"
    );
    let stored_text = fs::read_to_string(store.project_path(&identity.key)).unwrap_or_default();
    assert!(!stored_text.contains(SEEDED_SECRET));
    assert!(!stored_text.contains("[REDACTED"), "refused, not redacted");
}

#[test]
fn a_session_entry_with_a_secret_is_refused_and_never_reaches_the_log() {
    let root = temp_dir("refuse-session-secret");
    let store = memory_store(&root, false);
    let sessions = SessionStore::new(root.join(".damaian"));
    let session = sessions
        .create_session("repo_fixture", "fixture session")
        .unwrap();

    let entry = session_entry(&session.id, &format!("My token is {SEEDED_SECRET}."));
    let error = store.insert(&root, None, entry).unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::Secret { .. })
    ));

    let log = fs::read_to_string(
        root.join(".damaian")
            .join("sessions")
            .join(format!("{}.jsonl", session.id)),
    )
    .unwrap();
    assert!(!log.contains(SEEDED_SECRET));
    assert!(
        !log.contains("memory_recorded"),
        "the store refused before appending"
    );
    assert!(store.session_entries(&session.id).unwrap().is_empty());
}

#[test]
fn session_memory_is_private_to_its_session_and_dies_with_it() {
    let root = temp_dir("session-scope");
    let store = memory_store(&root, false);
    let sessions = SessionStore::new(root.join(".damaian"));
    let first = sessions.create_session("repo_fixture", "first").unwrap();
    let second = sessions.create_session("repo_fixture", "second").unwrap();

    let entry = session_entry(&first.id, "This checkout is the review copy.");
    let stored = store.insert(&root, None, entry).unwrap();
    assert_eq!(store.session_entries(&first.id).unwrap(), vec![stored]);
    assert!(store.session_entries(&second.id).unwrap().is_empty());

    sessions.delete_session(&first.id).unwrap();
    assert!(store.session_entries(&first.id).unwrap().is_empty());
}

#[test]
fn a_session_entry_must_name_a_session_that_exists() {
    let root = temp_dir("session-unknown");
    let store = memory_store(&root, false);
    let entry = session_entry("session_missing", "A fact.");
    let error = store.insert(&root, None, entry).unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::InvalidShape(_))
    ));
}

#[test]
fn user_scope_is_refused_on_write_and_invisible_on_read_while_disabled() {
    let root = temp_dir("user-scope-disabled");
    let disabled = memory_store(&root, false);
    let error = disabled
        .insert(&root, None, user_entry("Prefer short commit subjects."))
        .unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::UserScopeDisabled)
    ));

    // A store written while the scope was enabled must stop influencing
    // sessions once it is switched off.
    let enabled = memory_store(&root, true);
    let stored = enabled
        .insert(&root, None, user_entry("Prefer short commit subjects."))
        .unwrap();
    assert_eq!(enabled.user_entries().unwrap(), vec![stored]);
    assert!(
        disabled.user_entries().unwrap().is_empty(),
        "a disabled scope must not read the file"
    );
}

#[test]
fn evidence_citing_a_restricted_path_is_refused_by_the_store() {
    let root = temp_dir("evidence-restricted");
    let store = memory_store_with(&root, false, |config| {
        config.restricted_patterns = vec!["secrets/**".to_string()];
    });
    let identity = project_identity("root_evidence_restricted");
    let mut entry = project_entry(&identity.key, "The deploy reads one protected file.");
    entry.provenance = MemoryProvenance::RepositoryEvidence {
        paths: vec![EvidenceRef::new("secrets/prod.env", "sha256:abc")],
    };

    let error = store.insert(&root, Some(&identity), entry).unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::IneligibleEvidence {
            reason: EvidenceRefusal::Restricted
        })
    ));
    assert!(store.project_entries(&identity).unwrap().is_empty());
}

#[test]
fn evidence_excluded_by_a_per_directory_gitignore_is_refused() {
    let root = temp_dir("evidence-ignored");
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("nested/.gitignore"), "*.tmp\n").unwrap();
    fs::write(root.join("nested/scratch.tmp"), "scratch\n").unwrap();
    fs::write(root.join("nested/kept.txt"), "kept\n").unwrap();
    let store = memory_store(&root, false);
    let identity = project_identity("root_evidence_ignored");

    let mut entry = project_entry(&identity.key, "Scratch files are disposable.");
    entry.provenance = MemoryProvenance::RepositoryEvidence {
        paths: vec![EvidenceRef::new("nested/scratch.tmp", "sha256:abc")],
    };
    let error = store.insert(&root, Some(&identity), entry).unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::IneligibleEvidence {
            reason: EvidenceRefusal::Ignored
        })
    ));

    let mut allowed = project_entry(&identity.key, "Kept files are committed.");
    allowed.provenance = MemoryProvenance::RepositoryEvidence {
        paths: vec![EvidenceRef::new("nested/kept.txt", "sha256:abc")],
    };
    assert!(store.insert(&root, Some(&identity), allowed).is_ok());
}

#[test]
fn evidence_must_be_repository_relative() {
    let root = temp_dir("evidence-relative");
    let store = memory_store(&root, false);
    let identity = project_identity("root_evidence_relative");

    for path in ["/etc/passwd", "../outside.txt", "./README.md", ""] {
        let mut entry = project_entry(&identity.key, "A statement.");
        entry.provenance = MemoryProvenance::RepositoryEvidence {
            paths: vec![EvidenceRef::new(path, "sha256:abc")],
        };
        let error = store.insert(&root, Some(&identity), entry).unwrap_err();
        assert!(
            matches!(
                error,
                MemoryError::Refused(MemoryRefusal::IneligibleEvidence {
                    reason: EvidenceRefusal::NotRelative
                })
            ),
            "{path:?} was not refused as non-relative"
        );
    }
}

#[test]
fn a_secret_in_an_evidence_path_is_refused() {
    let root = temp_dir("evidence-secret-path");
    let store = memory_store(&root, false);
    let identity = project_identity("root_evidence_secret_path");
    let mut entry = project_entry(&identity.key, "The runner uses a generated key.");
    entry.provenance = MemoryProvenance::RepositoryEvidence {
        paths: vec![EvidenceRef::new(
            format!("config/{SEEDED_SECRET}.txt"),
            "sha256:abc",
        )],
    };
    let error = store.insert(&root, Some(&identity), entry).unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::Secret { .. })
    ));
    assert!(!error.to_string().contains(SEEDED_SECRET));
}

#[test]
fn sensitivity_is_raised_to_the_derived_value_but_never_lowered() {
    let root = temp_dir("sensitivity-write");
    let store = memory_store(&root, false);
    let identity = project_identity("root_sensitivity_write");

    let mut lowered = project_entry(&identity.key, "Logs live under /var/log/damaian.");
    lowered.sensitivity = Sensitivity::Ordinary;
    let stored = store.insert(&root, Some(&identity), lowered).unwrap();
    assert_eq!(stored.sensitivity, Sensitivity::Environmental);

    let mut raised = project_entry(&identity.key, "A plain convention.");
    raised.sensitivity = Sensitivity::Personal;
    let stored = store.insert(&root, Some(&identity), raised).unwrap();
    assert_eq!(stored.sensitivity, Sensitivity::Personal);
}

#[test]
fn a_statement_over_the_ceiling_is_refused_not_truncated() {
    let root = temp_dir("statement-ceiling");
    let store = memory_store(&root, false);
    let identity = project_identity("root_statement_ceiling");
    let statement = "a".repeat(workspace_engine::STATEMENT_MAX_CHARS + 1);
    let error = store
        .insert(
            &root,
            Some(&identity),
            project_entry(&identity.key, &statement),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::InvalidShape(_))
    ));
    assert!(error.to_string().contains("not truncated"));
}

#[test]
fn a_project_key_that_is_not_this_repository_is_refused() {
    let root = temp_dir("project-key-mismatch");
    let store = memory_store(&root, false);
    let identity = project_identity("root_mine");
    let entry = project_entry("root_someone_else", "A fact.");
    let error = store.insert(&root, Some(&identity), entry).unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::InvalidShape(_))
    ));
}

#[test]
fn a_project_write_without_an_identity_is_refused() {
    let root = temp_dir("project-key-missing-identity");
    let store = memory_store(&root, false);
    let entry = project_entry("root_any", "A fact.");
    let error = store.insert(&root, None, entry).unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::InvalidShape(_))
    ));
}

// ---------------------------------------------------------------------------
// Lifecycle transitions and audit (Task 6).
// ---------------------------------------------------------------------------

use workspace_engine::MemoryTarget;

fn audit_events(root: &Path) -> Vec<serde_json::Value> {
    let path = root.join(".damaian").join("audit").join("events.jsonl");
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn audit_event_types(root: &Path) -> Vec<String> {
    audit_events(root)
        .iter()
        .filter_map(|event| event.get("eventType").and_then(|value| value.as_str()))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_lifecycle_transition_records_one_audit_event() {
    let root = temp_dir("audit-lifecycle");
    let store = memory_store(&root, false);
    let identity = project_identity("root_audit_lifecycle");
    let target = MemoryTarget::Project(&identity);

    let created = store
        .insert(
            &root,
            Some(&identity),
            project_entry(&identity.key, "Builds use Cargo."),
        )
        .unwrap();
    let edited = store
        .edit_statement(&root, target, &created.id, "Builds use Cargo and nextest.")
        .unwrap();
    let replacement = store
        .supersede(
            &root,
            target,
            &edited.id,
            project_entry(&identity.key, "Builds use Cargo and the nextest runner."),
        )
        .unwrap();
    store
        .mark_state(target, &replacement.id, MemoryState::Expired)
        .unwrap();
    store
        .mark_state(target, &replacement.id, MemoryState::Deleted)
        .unwrap();
    store
        .record_recall(std::slice::from_ref(&replacement))
        .unwrap();

    let events = audit_event_types(&root);
    for expected in [
        "memory_created",
        "memory_edited",
        "memory_superseded",
        "memory_expired",
        "memory_deleted",
        "memory_recalled",
    ] {
        assert!(
            events.iter().any(|event| event == expected),
            "{expected} missing from {events:?}"
        );
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| *event == "memory_created")
            .count(),
        2,
        "the replacement is a creation too"
    );

    let states: Vec<_> = store.project_entries(&identity).unwrap();
    let replacement_record = states
        .iter()
        .find(|entry| entry.id == replacement.id)
        .unwrap();
    assert_eq!(replacement_record.state, MemoryState::Deleted);
    let superseded = states.iter().find(|entry| entry.id == edited.id).unwrap();
    assert_eq!(superseded.state, MemoryState::Superseded);
}

#[test]
fn confirming_a_suggestion_is_audited_as_a_confirmation() {
    let root = temp_dir("audit-confirmation");
    let store = memory_store(&root, false);
    let identity = project_identity("root_audit_confirmation");
    let mut entry = project_entry(&identity.key, "Prefer nextest over cargo test.");
    entry.provenance = MemoryProvenance::SuggestedAndConfirmed {
        session_id: "session_fixture".to_string(),
        task_id: "task_fixture".to_string(),
    };
    store.insert(&root, Some(&identity), entry).unwrap();

    let events = audit_event_types(&root);
    assert!(events.contains(&"memory_created".to_string()));
    assert!(events.contains(&"memory_confirmed".to_string()));
}

#[test]
fn personal_and_environmental_statements_never_reach_the_audit_log() {
    let root = temp_dir("audit-sensitivity");
    let store = memory_store(&root, true);
    let identity = project_identity("root_audit_sensitivity");
    let environmental = "Logs live under /var/log/damaian and rotate weekly.";
    let personal = "Prefer very short commit subjects.";
    let ordinary = "Builds use Cargo.";

    let environmental_entry = store
        .insert(
            &root,
            Some(&identity),
            project_entry(&identity.key, environmental),
        )
        .unwrap();
    assert_eq!(environmental_entry.sensitivity, Sensitivity::Environmental);
    let personal_entry = store.insert(&root, None, user_entry(personal)).unwrap();
    assert_eq!(personal_entry.sensitivity, Sensitivity::Personal);
    store
        .insert(
            &root,
            Some(&identity),
            project_entry(&identity.key, ordinary),
        )
        .unwrap();

    store
        .mark_state(
            MemoryTarget::Project(&identity),
            &environmental_entry.id,
            MemoryState::Deleted,
        )
        .unwrap();
    store
        .record_recall(&[environmental_entry, personal_entry])
        .unwrap();

    let content =
        fs::read_to_string(root.join(".damaian").join("audit").join("events.jsonl")).unwrap();
    assert!(
        !content.contains(environmental),
        "environmental statement leaked"
    );
    assert!(!content.contains(personal), "personal statement leaked");
    assert!(
        content.contains(ordinary),
        "an ordinary statement is useful in the log"
    );
}

#[test]
fn a_disabled_audit_log_writes_no_memory_events() {
    let root = temp_dir("audit-disabled");
    let config = test_config(&root);
    let scanner = SecretScanner::new(Vec::new());
    let audit = AuditLog::new(&config.data_dir, false, scanner.clone());
    let store = MemoryStore::new(
        MemoryStoreConfig {
            data_dir: config.data_dir.clone(),
            user_scope_enabled: false,
            ignore_patterns: config.ignore_patterns.clone(),
        },
        audit,
        scanner,
        workspace_engine::PathPolicy::new(&config),
        SessionStore::new(&config.data_dir),
    );
    let identity = project_identity("root_audit_disabled");
    store
        .insert(
            &root,
            Some(&identity),
            project_entry(&identity.key, "Builds use Cargo."),
        )
        .unwrap();
    assert!(
        !root
            .join(".damaian")
            .join("audit")
            .join("events.jsonl")
            .exists()
    );
}

#[test]
fn editing_a_refused_statement_leaves_the_old_one_active() {
    let root = temp_dir("audit-refused-edit");
    let store = memory_store(&root, false);
    let identity = project_identity("root_refused_edit");
    let target = MemoryTarget::Project(&identity);
    let entry = store
        .insert(
            &root,
            Some(&identity),
            project_entry(&identity.key, "Builds use Cargo."),
        )
        .unwrap();

    let error = store
        .edit_statement(
            &root,
            target,
            &entry.id,
            &format!("The token is {SEEDED_SECRET}."),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        MemoryError::Refused(MemoryRefusal::Secret { .. })
    ));
    let stored = store.project_entries(&identity).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].statement, "Builds use Cargo.");
    assert_eq!(stored[0].state, MemoryState::Active);
}

#[test]
fn a_superseded_entry_keeps_the_link_to_its_replacement() {
    let root = temp_dir("audit-supersede-link");
    let store = memory_store(&root, false);
    let identity = project_identity("root_supersede_link");
    let target = MemoryTarget::Project(&identity);
    let first = store
        .insert(
            &root,
            Some(&identity),
            project_entry(&identity.key, "Tests need Docker."),
        )
        .unwrap();
    let second = store
        .supersede(
            &root,
            target,
            &first.id,
            project_entry(&identity.key, "Tests need Docker Compose v2."),
        )
        .unwrap();
    assert_eq!(second.supersedes.as_deref(), Some(first.id.as_str()));
}

#[test]
fn the_engine_exposes_a_memory_store_that_follows_config() {
    let root = temp_dir("engine-memory-store");
    let config = test_config(&root);
    let engine = workspace_engine::WorkspaceEngine::new(config.clone());
    assert_eq!(
        engine.memory_store.memory_dir(),
        config.data_dir.join("memory")
    );

    let mut enabled = test_config(&root);
    enabled.memory_user_scope_enabled = true;
    let engine = workspace_engine::WorkspaceEngine::new(enabled);
    engine
        .memory_store
        .insert(&root, None, user_entry("Prefer short commit subjects."))
        .unwrap();
    assert_eq!(engine.memory_store.user_entries().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// End-to-end isolation at each identity tier (acceptance criteria).
// ---------------------------------------------------------------------------

fn shared_identity(repo: &Path, data_root: &Path) -> ProjectIdentity {
    let config = Config {
        data_dir: data_root.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    };
    ProjectIdentity::resolve_for_root(repo, &git_service(&config)).unwrap()
}

#[test]
fn project_memory_written_in_one_repository_is_invisible_in_another() {
    let data_root = temp_dir("isolation-root-commits");
    let first = repo_with_a_commit("isolation-first");
    let second = repo_with_a_commit("isolation-second");
    let first_identity = shared_identity(&first, &data_root);
    let second_identity = shared_identity(&second, &data_root);
    let store = memory_store(&data_root, false);

    let stored = store
        .insert(
            &first,
            Some(&first_identity),
            project_entry(&first_identity.key, "Integration tests need Docker."),
        )
        .unwrap();

    assert_eq!(
        store.project_entries(&first_identity).unwrap(),
        vec![stored]
    );
    assert!(store.project_entries(&second_identity).unwrap().is_empty());
}

#[test]
fn project_memory_is_isolated_at_the_remote_and_path_tiers_too() {
    let data_root = temp_dir("isolation-weak-tiers");
    let store = memory_store(&data_root, false);

    // Remote tier: no history, different remotes.
    let remote_first = temp_dir("isolation-remote-first");
    let remote_second = temp_dir("isolation-remote-second");
    for (root, remote) in [
        (&remote_first, "git@github.com:owner/first.git"),
        (&remote_second, "git@github.com:owner/second.git"),
    ] {
        init_repo(root);
        git(root, &["remote", "add", "origin", remote]);
    }
    let first = shared_identity(&remote_first, &data_root);
    let second = shared_identity(&remote_second, &data_root);
    assert_eq!(first.source, ProjectKeySource::RemoteUrl);
    store
        .insert(
            &remote_first,
            Some(&first),
            project_entry(&first.key, "Remote-tier fact."),
        )
        .unwrap();
    assert!(store.project_entries(&second).unwrap().is_empty());

    // Path tier: neither history nor a remote.
    let path_first = temp_dir("isolation-path-first");
    let path_second = temp_dir("isolation-path-second");
    init_repo(&path_first);
    init_repo(&path_second);
    let first = shared_identity(&path_first, &data_root);
    let second = shared_identity(&path_second, &data_root);
    assert_eq!(first.source, ProjectKeySource::RepositoryId);
    store
        .insert(
            &path_first,
            Some(&first),
            project_entry(&first.key, "Path-tier fact."),
        )
        .unwrap();
    assert!(store.project_entries(&second).unwrap().is_empty());
}

#[test]
fn two_checkouts_share_project_memory_through_the_store() {
    let data_root = temp_dir("isolation-shared-clone");
    let origin = repo_with_a_commit("isolation-origin");
    let clone = temp_dir("isolation-clone-target");
    assert!(
        Command::new("git")
            .args(["clone", "-q"])
            .arg(&origin)
            .arg(&clone)
            .status()
            .unwrap()
            .success()
    );
    let store = memory_store(&data_root, false);
    let origin_identity = shared_identity(&origin, &data_root);
    let clone_identity = shared_identity(&clone, &data_root);

    let stored = store
        .insert(
            &origin,
            Some(&origin_identity),
            project_entry(&origin_identity.key, "Integration tests need Docker."),
        )
        .unwrap();

    assert_eq!(clone_identity.key, origin_identity.key);
    assert_eq!(
        store.project_entries(&clone_identity).unwrap(),
        vec![stored],
        "the clone reads the same project memory"
    );
}

#[test]
fn a_renamed_checkout_retains_its_project_memory() {
    let data_root = temp_dir("isolation-renamed");
    let repo = repo_with_a_commit("isolation-renamed-repo");
    let store = memory_store(&data_root, false);
    let before = shared_identity(&repo, &data_root);
    let stored = store
        .insert(
            &repo,
            Some(&before),
            project_entry(&before.key, "A fact that survives a rename."),
        )
        .unwrap();

    let renamed = repo.with_file_name(format!(
        "{}-moved",
        repo.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(&repo, &renamed).unwrap();
    let after = shared_identity(&renamed, &data_root);

    assert_eq!(after.key, before.key);
    assert_eq!(store.project_entries(&after).unwrap(), vec![stored]);
}
