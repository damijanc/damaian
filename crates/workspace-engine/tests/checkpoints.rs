//! Session checkpoints, per `docs/specs/16_session_checkpoints_and_rewind.md`.
//!
//! These cover what a checkpoint records and what it deliberately refuses to
//! record: content is stored faithfully so a file can be put back byte for
//! byte, while the manifest carries only paths, hashes, and counts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    AuditLog, CheckpointConversation, CheckpointOrigin, CheckpointPath, CheckpointRequest,
    CheckpointRestoreOptions, CheckpointStore, Config, PathPolicy, SecretScanner, SessionStore,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// A fake credential, so the faithful-capture requirement is asserted against
/// something the secret scanner actually detects.
const SEEDED_SECRET: &str = "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

struct Fixture {
    repository: PathBuf,
    data_dir: PathBuf,
    store: CheckpointStore,
}

fn fixture(name: &str) -> Fixture {
    fixture_with(name, |_| {})
}

fn fixture_with(name: &str, adjust: impl FnOnce(&mut Config)) -> Fixture {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "damaian-checkpoints-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let repository = root.join("repo");
    let data_dir = root.join("data");
    fs::create_dir_all(&repository).expect("repository should be created");
    fs::create_dir_all(&data_dir).expect("data dir should be created");

    let mut config = Config {
        data_dir: data_dir.clone(),
        audit_enabled: true,
        ..Config::default()
    };
    adjust(&mut config);
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let audit_log = AuditLog::new(&config.data_dir, true, scanner.clone());
    let path_policy = PathPolicy::new(&config);
    let store = CheckpointStore::new(config, audit_log, path_policy);
    Fixture {
        repository,
        data_dir,
        store,
    }
}

impl Fixture {
    fn write(&self, relative_path: &str, content: &str) {
        let path = self.repository.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(&self, relative_path: &str) -> String {
        fs::read_to_string(self.repository.join(relative_path)).expect("file should exist")
    }

    fn manifest_json(&self, repository_id: &str, checkpoint_id: &str) -> String {
        fs::read_to_string(
            self.data_dir
                .join("checkpoints")
                .join(repository_id)
                .join("manifests")
                .join(format!("{checkpoint_id}.json")),
        )
        .expect("manifest should be written")
    }
}

fn request<'a>(
    session_id: &'a str,
    summary: &'a str,
    paths: Vec<CheckpointPath>,
) -> CheckpointRequest<'a> {
    CheckpointRequest {
        session_id,
        task_id: Some("task_1"),
        user_message_id: Some("msg_1"),
        summary,
        conversation: CheckpointConversation {
            last_event_seq: 7,
            task_status: "waiting_for_approval".to_string(),
        },
        pending_approvals: Vec::new(),
        paths,
    }
}

fn patch_path(path: &str) -> CheckpointPath {
    CheckpointPath {
        path: path.to_string(),
        origin: CheckpointOrigin::Patch,
    }
}

#[test]
fn records_existence_hash_and_origin_for_every_in_scope_path() {
    let fixture = fixture("scope");
    fixture.write("src/upload.rs", "fn upload() {}\n");

    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: add retry to the upload client",
                vec![
                    patch_path("src/upload.rs"),
                    CheckpointPath {
                        path: "src/generated.rs".to_string(),
                        origin: CheckpointOrigin::Command,
                    },
                ],
            ),
        )
        .expect("checkpoint should be created");

    assert_eq!(manifest.files.len(), 2);
    let existing = manifest
        .files
        .iter()
        .find(|file| file.path == "src/upload.rs")
        .expect("the modified file should be recorded");
    assert!(existing.existed);
    assert!(existing.hash.is_some());
    assert!(existing.oid.is_some());
    assert_eq!(existing.origin, CheckpointOrigin::Patch);

    // A file the agent is about to create has no content to snapshot, and says
    // so explicitly rather than being inferred from absence at restore time.
    let created = manifest
        .files
        .iter()
        .find(|file| file.path == "src/generated.rs")
        .expect("the file to be created should be recorded");
    assert!(!created.existed);
    assert_eq!(created.hash, None);
    assert_eq!(created.oid, None);
    assert_eq!(created.origin, CheckpointOrigin::Command);
    assert_eq!(manifest.conversation.last_event_seq, 7);
    assert!(!manifest.tree_oid.is_empty());
}

#[test]
fn a_restricted_path_is_recorded_as_excluded_rather_than_snapshotted() {
    let fixture = fixture("restricted");
    fixture.write(".env", &format!("{SEEDED_SECRET}\n"));

    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: read config", vec![patch_path(".env")]),
        )
        .unwrap();

    assert!(manifest.files.is_empty());
    assert_eq!(manifest.excluded.len(), 1);
    assert_eq!(manifest.excluded[0].path, ".env");
    assert_eq!(manifest.excluded[0].reason, "restricted_pattern");
}

#[test]
fn a_path_outside_the_repository_is_excluded_rather_than_snapshotted() {
    let fixture = fixture("outside");

    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: touch the neighbour",
                vec![patch_path("../outside.txt")],
            ),
        )
        .unwrap();

    assert!(manifest.files.is_empty());
    assert_eq!(manifest.excluded.len(), 1);
    assert_eq!(manifest.excluded[0].reason, "outside_repository");
}

// §5.2's obligation: only the object store holds bytes. A manifest that carried
// content would put a credential in a file that gets read, listed, and shown.
#[test]
fn the_manifest_carries_paths_and_hashes_but_never_content() {
    let fixture = fixture("no-content");
    fixture.write(
        "config/settings.rs",
        &format!("let key = \"{SEEDED_SECRET}\";\n"),
    );

    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: rotate the key",
                vec![patch_path("config/settings.rs")],
            ),
        )
        .unwrap();

    let raw = fixture.manifest_json(&manifest.repository_id, &manifest.checkpoint_id);
    assert!(raw.contains("config/settings.rs"));
    assert!(!raw.contains(SEEDED_SECRET));
    assert!(!raw.contains("let key"));
}

#[test]
fn checkpoints_are_listed_newest_first_and_read_back_by_id() {
    let fixture = fixture("list");
    fixture.write("a.txt", "one\n");
    let first = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: first", vec![patch_path("a.txt")]),
        )
        .unwrap();
    fixture.write("a.txt", "two\n");
    let second = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: second", vec![patch_path("a.txt")]),
        )
        .unwrap();

    let listed = fixture
        .store
        .list_checkpoints(&first.repository_id)
        .expect("checkpoints should list");

    assert_eq!(
        listed
            .iter()
            .map(|manifest| manifest.checkpoint_id.as_str())
            .collect::<Vec<_>>(),
        vec![second.checkpoint_id.as_str(), first.checkpoint_id.as_str()]
    );
    let read_back = fixture
        .store
        .read_checkpoint(&first.repository_id, &first.checkpoint_id)
        .unwrap()
        .expect("the checkpoint should be readable by id");
    assert_eq!(read_back.summary, "Before: first");
}

// The repository the checkpoint covers is left exactly as it was: creating a
// checkpoint is a read of the working tree, and Damaian never writes to the
// user's own Git directory.
#[test]
fn creating_a_checkpoint_does_not_touch_the_users_repository() {
    let fixture = fixture("untouched");
    fixture.write("a.txt", "unchanged\n");
    let before = snapshot(&fixture.repository);

    fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: nothing", vec![patch_path("a.txt")]),
        )
        .unwrap();

    assert_eq!(snapshot(&fixture.repository), before);
    assert_eq!(fixture.read("a.txt"), "unchanged\n");
}

fn files_only() -> CheckpointRestoreOptions<'static> {
    CheckpointRestoreOptions {
        files: true,
        conversation: false,
        only_path: None,
    }
}

// Requirement 9, the reason the store keeps the user's bytes as they are: a
// redacted snapshot would put a placeholder where the credential was.
#[test]
fn restores_a_file_byte_identically_including_a_seeded_secret() {
    let fixture = fixture("faithful-restore");
    let original = format!("let key = \"{SEEDED_SECRET}\";\n");
    fixture.write("src/config.rs", &original);
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: rotate the key",
                vec![patch_path("src/config.rs")],
            ),
        )
        .unwrap();
    // The turn runs: the agent rewrites the file, and the checkpoint is sealed
    // with what it left behind.
    fixture.write("src/config.rs", "let key = \"\";\n");
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .expect("checkpoint should seal");

    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .expect("restore should succeed");

    assert_eq!(result.restored_files, vec!["src/config.rs".to_string()]);
    assert!(result.conflicted_files.is_empty());
    assert_eq!(fixture.read("src/config.rs"), original);
    assert!(!result.conversation_restored);
}

#[test]
fn deletes_a_file_the_turn_created() {
    let fixture = fixture("delete-created");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: generate a module",
                vec![patch_path("src/generated.rs")],
            ),
        )
        .unwrap();
    fixture.write("src/generated.rs", "// generated\n");
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();

    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    assert_eq!(result.deleted_files, vec!["src/generated.rs".to_string()]);
    assert!(!fixture.repository.join("src/generated.rs").exists());
}

#[test]
fn a_file_the_checkpoint_says_did_not_exist_and_still_does_not_is_skipped() {
    let fixture = fixture("skip-absent");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: a turn that wrote nothing",
                vec![patch_path("src/never_written.rs")],
            ),
        )
        .unwrap();

    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    assert_eq!(
        result.skipped_files,
        vec!["src/never_written.rs".to_string()]
    );
    assert!(result.deleted_files.is_empty());
}

// A partial write with a conflict in the middle leaves a tree that is neither
// the checkpoint nor what the user had, so a conflict writes nothing at all.
#[test]
fn a_file_changed_after_the_checkpoint_conflicts_and_nothing_is_written() {
    let fixture = fixture("conflict");
    fixture.write("src/a.rs", "original a\n");
    fixture.write("src/b.rs", "original b\n");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: touch both",
                vec![patch_path("src/a.rs"), patch_path("src/b.rs")],
            ),
        )
        .unwrap();
    fixture.write("src/a.rs", "the agent changed this too\n");
    fixture.write("src/b.rs", "the agent changed this\n");
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    // ... and then the user edits one of them by hand.
    fixture.write("src/a.rs", "the user edited this by hand\n");

    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .expect("a conflict is a reported outcome, not an error");

    assert_eq!(result.conflicted_files, vec!["src/a.rs".to_string()]);
    assert!(result.restored_files.is_empty());
    assert_eq!(fixture.read("src/a.rs"), "the user edited this by hand\n");
    assert_eq!(fixture.read("src/b.rs"), "the agent changed this\n");
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("src/a.rs")),
        "the conflict should be explained, not just counted: {:?}",
        result.warnings
    );
}

#[test]
fn restoring_a_single_file_leaves_every_other_file_untouched() {
    let fixture = fixture("single-file");
    fixture.write("src/a.rs", "original a\n");
    fixture.write("src/b.rs", "original b\n");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: touch both",
                vec![patch_path("src/a.rs"), patch_path("src/b.rs")],
            ),
        )
        .unwrap();
    fixture.write("src/a.rs", "changed a\n");
    fixture.write("src/b.rs", "changed b\n");
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();

    let result = fixture
        .store
        .restore(
            &fixture.repository,
            &manifest,
            CheckpointRestoreOptions {
                files: true,
                conversation: false,
                only_path: Some("src/a.rs"),
            },
            "user",
        )
        .unwrap();

    assert_eq!(result.restored_files, vec!["src/a.rs".to_string()]);
    assert_eq!(fixture.read("src/a.rs"), "original a\n");
    assert_eq!(fixture.read("src/b.rs"), "changed b\n");
}

#[test]
fn restoring_conversation_only_moves_the_conversation_and_leaves_files_alone() {
    let fixture = fixture("conversation-only");
    let sessions = SessionStore::new(&fixture.data_dir);
    let session = sessions.create_session("repo_1", "Rewind me").unwrap();
    sessions
        .append_message(&session.id, None, "user", "keep me")
        .unwrap();
    let position = sessions.latest_event_seq(&session.id).unwrap();
    fixture.write("src/a.rs", "original a\n");
    let mut checkpoint_request = request(
        &session.id,
        "Before: the wrong direction",
        vec![patch_path("src/a.rs")],
    );
    checkpoint_request.conversation = CheckpointConversation {
        last_event_seq: position,
        task_status: "complete".to_string(),
    };
    let manifest = fixture
        .store
        .create_checkpoint(&fixture.repository, checkpoint_request)
        .unwrap();
    sessions
        .append_message(&session.id, None, "assistant", "wrong direction")
        .unwrap();
    fixture.write("src/a.rs", "changed a\n");

    let result = fixture
        .store
        .restore(
            &fixture.repository,
            &manifest,
            CheckpointRestoreOptions {
                files: false,
                conversation: true,
                only_path: None,
            },
            "user",
        )
        .unwrap();

    assert!(result.conversation_restored);
    assert!(result.restored_files.is_empty());
    // Files are the other switch: a conversation rewind leaves the tree alone.
    assert_eq!(fixture.read("src/a.rs"), "changed a\n");
    let messages = sessions.read_messages(&session.id).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "keep me");
}

#[test]
fn an_executable_file_keeps_its_executable_bit_when_restored() {
    let fixture = fixture("executable");
    fixture.write("scripts/run.sh", "#!/bin/sh\necho original\n");
    let script = fixture.repository.join("scripts/run.sh");
    set_executable(&script);
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: rewrite the script",
                vec![patch_path("scripts/run.sh")],
            ),
        )
        .unwrap();
    fixture.write("scripts/run.sh", "#!/bin/sh\necho changed\n");
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();

    fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    assert_eq!(fixture.read("scripts/run.sh"), "#!/bin/sh\necho original\n");
    assert!(
        is_executable(&script),
        "a script restored without its executable bit no longer runs"
    );
}

#[test]
fn restoring_records_that_the_checkpoint_was_restored_from() {
    let fixture = fixture("restored-at");
    fixture.write("a.txt", "one\n");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: one", vec![patch_path("a.txt")]),
        )
        .unwrap();

    fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    let stored = fixture
        .store
        .read_checkpoint(&manifest.repository_id, &manifest.checkpoint_id)
        .unwrap()
        .unwrap();
    assert!(stored.restored_at_ms.is_some());
}

// Requirement 13, with the §5.2 obligation attached: the audit log records what
// happened to which path, and never what the file contained.
#[test]
fn create_restore_and_conflict_events_reach_the_audit_log_without_content() {
    let fixture = fixture("audit");
    fixture.write("src/a.rs", &format!("let key = \"{SEEDED_SECRET}\";\n"));
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: audited", vec![patch_path("src/a.rs")]),
        )
        .unwrap();
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();
    fixture.write("src/a.rs", "edited by hand\n");
    fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    let audit = fs::read_to_string(fixture.data_dir.join("audit").join("events.jsonl"))
        .expect("audit log should exist");
    assert!(audit.contains("checkpoint_created"));
    assert!(audit.contains("checkpoint_restored"));
    assert!(audit.contains("checkpoint_conflicted"));
    assert!(!audit.contains(SEEDED_SECRET));
}

// Acceptance: an untracked generated file with no recorded agent action is not
// removed by rewind. An unsealed checkpoint cannot attribute what appeared at
// the path, so it refuses rather than deleting the user's file.
#[test]
fn a_file_that_appeared_without_a_recorded_agent_action_is_not_deleted() {
    let fixture = fixture("unattributed");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: a turn that never finished",
                vec![patch_path("src/generated.rs")],
            ),
        )
        .unwrap();
    // Nothing sealed this checkpoint, and a file is now there.
    fixture.write("src/generated.rs", "written by someone else\n");

    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    assert_eq!(
        result.conflicted_files,
        vec!["src/generated.rs".to_string()]
    );
    assert!(result.deleted_files.is_empty());
    assert_eq!(
        fixture.read("src/generated.rs"),
        "written by someone else\n"
    );
}

#[test]
fn a_created_file_the_user_edited_after_the_turn_is_not_deleted() {
    let fixture = fixture("edited-created");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: generate a module",
                vec![patch_path("src/generated.rs")],
            ),
        )
        .unwrap();
    fixture.write("src/generated.rs", "// generated\n");
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    fixture.write(
        "src/generated.rs",
        "// generated, then edited by the user\n",
    );

    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    assert_eq!(
        result.conflicted_files,
        vec!["src/generated.rs".to_string()]
    );
    assert!(fixture.repository.join("src/generated.rs").exists());
}

/// Backdates a manifest the way an old checkpoint would look, so retention can
/// be tested without waiting a day.
fn backdate(fixture: &Fixture, manifest: &workspace_engine::CheckpointManifest, days: u128) {
    let path = fixture
        .data_dir
        .join("checkpoints")
        .join(&manifest.repository_id)
        .join("manifests")
        .join(format!("{}.json", manifest.checkpoint_id));
    let raw = fs::read_to_string(&path).unwrap();
    let created = manifest.created_at_ms - days * 24 * 60 * 60 * 1000;
    let updated = raw.replace(
        &format!("\"createdAtMs\": {}", manifest.created_at_ms),
        &format!("\"createdAtMs\": {created}"),
    );
    assert_ne!(raw, updated, "the manifest timestamp should be rewritten");
    fs::write(path, updated).unwrap();
}

#[test]
fn cleanup_drops_checkpoints_past_the_retention_window_and_keeps_the_rest() {
    let fixture = fixture_with("retention", |config| {
        config.checkpoint_retention_days = 7;
    });
    fixture.write("a.txt", "old\n");
    let old = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: old", vec![patch_path("a.txt")]),
        )
        .unwrap();
    backdate(&fixture, &old, 30);
    fixture.write("a.txt", "recent\n");
    let recent = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_2", "Before: recent", vec![patch_path("a.txt")]),
        )
        .unwrap();

    let removed = fixture
        .store
        .cleanup(&old.repository_id, None)
        .expect("cleanup should run");

    assert_eq!(removed, vec![old.checkpoint_id.clone()]);
    let remaining = fixture.store.list_checkpoints(&old.repository_id).unwrap();
    assert_eq!(
        remaining
            .iter()
            .map(|manifest| manifest.checkpoint_id.as_str())
            .collect::<Vec<_>>(),
        vec![recent.checkpoint_id.as_str()]
    );
}

// The checkpoint a user is most likely to want is the newest one of the session
// they are in, which is exactly what a time-based rule would take during a long
// session.
#[test]
fn cleanup_keeps_the_newest_checkpoint_of_the_active_session_regardless_of_age() {
    let fixture = fixture_with("retention-newest", |config| {
        config.checkpoint_retention_days = 1;
    });
    fixture.write("a.txt", "one\n");
    let first = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: one", vec![patch_path("a.txt")]),
        )
        .unwrap();
    fixture.write("a.txt", "two\n");
    let second = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: two", vec![patch_path("a.txt")]),
        )
        .unwrap();
    backdate(&fixture, &first, 30);
    backdate(&fixture, &second, 30);

    let removed = fixture
        .store
        .cleanup(&first.repository_id, Some("session_1"))
        .unwrap();

    assert_eq!(removed, vec![first.checkpoint_id.clone()]);
    let remaining = fixture
        .store
        .list_checkpoints(&first.repository_id)
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].checkpoint_id, second.checkpoint_id);
}

// The point of the ref-per-checkpoint design: what cleanup drops is collectable,
// and what it keeps still restores.
#[test]
fn cleanup_collects_dropped_content_and_leaves_live_checkpoints_restorable() {
    let fixture = fixture_with("retention-gc", |config| {
        config.checkpoint_retention_days = 7;
    });
    fixture.write("a.txt", "old\n");
    let old = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: old", vec![patch_path("a.txt")]),
        )
        .unwrap();
    backdate(&fixture, &old, 30);
    fixture.write("a.txt", "live\n");
    let live = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_2", "Before: live", vec![patch_path("a.txt")]),
        )
        .unwrap();
    fixture.write("a.txt", "changed by the agent\n");
    let live = fixture
        .store
        .seal_checkpoint(&fixture.repository, &live)
        .unwrap();

    fixture.store.cleanup(&old.repository_id, None).unwrap();

    let result = fixture
        .store
        .restore(&fixture.repository, &live, files_only(), "user")
        .expect("a live checkpoint should still restore after cleanup");
    assert_eq!(result.restored_files, vec!["a.txt".to_string()]);
    assert_eq!(fixture.read("a.txt"), "live\n");
}

#[test]
fn cleanup_is_recorded_in_the_audit_log() {
    let fixture = fixture_with("retention-audit", |config| {
        config.checkpoint_retention_days = 7;
    });
    fixture.write("a.txt", "old\n");
    let old = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: old", vec![patch_path("a.txt")]),
        )
        .unwrap();
    backdate(&fixture, &old, 30);
    fixture.write("a.txt", "recent\n");
    fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_2", "Before: recent", vec![patch_path("a.txt")]),
        )
        .unwrap();

    fixture.store.cleanup(&old.repository_id, None).unwrap();

    let audit = fs::read_to_string(fixture.data_dir.join("audit").join("events.jsonl")).unwrap();
    assert!(audit.contains("checkpoint_cleanup"));
}

impl Fixture {
    /// A real repository, because the census asks git what changed.
    fn git_init(&self) {
        self.git(&["init", "--quiet"]);
        self.git(&["config", "user.email", "test@damaian.local"]);
        self.git(&["config", "user.name", "Damaian Test"]);
        // Git's own background maintenance leaves a transient
        // `objects/maintenance.lock` behind a commit, which would show up as a
        // change to `.git` that Damaian did not make.
        self.git(&["config", "gc.auto", "0"]);
        self.git(&["config", "maintenance.auto", "false"]);
    }

    fn git_commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "--quiet", "-m", message]);
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repository)
            .args(args)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn command_checkpoint(fixture: &Fixture, session_id: &str) -> workspace_engine::CheckpointManifest {
    fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(session_id, "Before: run a command", Vec::new()),
        )
        .unwrap()
}

// The `cargo fmt` case: a file that was committed and clean, rewritten by an
// approved command. Its pre-command content exists only in the user's own Git
// objects, which the census reads (never writes, never locks) to cover it.
#[test]
fn a_command_rewriting_a_clean_file_is_covered_and_restorable() {
    let fixture = fixture("census-clean");
    fixture.git_init();
    fixture.write("src/main.rs", "fn main ( ) { }\n");
    fixture.git_commit_all("initial");
    let manifest = command_checkpoint(&fixture, "session_1");
    let before = fixture
        .store
        .begin_command_census(&fixture.repository)
        .expect("census should run");

    // The approved command runs.
    fixture.write("src/main.rs", "fn main() {}\n");

    let manifest = fixture
        .store
        .record_command_effects(&fixture.repository, &manifest, &before)
        .expect("command effects should be recorded");

    let recorded = manifest
        .files
        .iter()
        .find(|file| file.path == "src/main.rs")
        .expect("the rewritten file should be covered");
    assert_eq!(recorded.origin, CheckpointOrigin::Command);
    assert!(recorded.existed);
    assert!(manifest.command_effects_covered);

    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();
    assert_eq!(fixture.read("src/main.rs"), "fn main ( ) { }\n");
}

// A file the user had already modified before the command: the checkpoint must
// put back what was there before the command, not what was committed.
#[test]
fn a_command_changing_an_already_dirty_file_restores_the_pre_command_content() {
    let fixture = fixture("census-dirty");
    fixture.git_init();
    fixture.write("src/main.rs", "committed\n");
    fixture.git_commit_all("initial");
    fixture.write("src/main.rs", "the user's own uncommitted work\n");
    let manifest = command_checkpoint(&fixture, "session_1");
    let before = fixture
        .store
        .begin_command_census(&fixture.repository)
        .unwrap();

    fixture.write("src/main.rs", "rewritten by the command\n");

    let manifest = fixture
        .store
        .record_command_effects(&fixture.repository, &manifest, &before)
        .unwrap();
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    assert_eq!(
        fixture.read("src/main.rs"),
        "the user's own uncommitted work\n"
    );
}

#[test]
fn a_file_a_command_created_is_deleted_by_restore() {
    let fixture = fixture("census-created");
    fixture.git_init();
    fixture.write("src/main.rs", "committed\n");
    fixture.git_commit_all("initial");
    let manifest = command_checkpoint(&fixture, "session_1");
    let before = fixture
        .store
        .begin_command_census(&fixture.repository)
        .unwrap();

    fixture.write("src/generated.rs", "// generated by the command\n");

    let manifest = fixture
        .store
        .record_command_effects(&fixture.repository, &manifest, &before)
        .unwrap();
    let recorded = manifest
        .files
        .iter()
        .find(|file| file.path == "src/generated.rs")
        .expect("the created file should be covered");
    assert!(!recorded.existed);

    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();
    assert_eq!(result.deleted_files, vec!["src/generated.rs".to_string()]);
}

#[test]
fn a_file_a_command_deleted_is_recreated_by_restore() {
    let fixture = fixture("census-deleted");
    fixture.git_init();
    fixture.write("src/doomed.rs", "delete me\n");
    fixture.git_commit_all("initial");
    let manifest = command_checkpoint(&fixture, "session_1");
    let before = fixture
        .store
        .begin_command_census(&fixture.repository)
        .unwrap();

    fs::remove_file(fixture.repository.join("src/doomed.rs")).unwrap();

    let manifest = fixture
        .store
        .record_command_effects(&fixture.repository, &manifest, &before)
        .unwrap();
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    assert_eq!(result.restored_files, vec!["src/doomed.rs".to_string()]);
    assert_eq!(fixture.read("src/doomed.rs"), "delete me\n");
}

// A checkpoint that claims coverage it does not have is worse than one that
// admits the gap.
#[test]
fn a_repository_over_the_census_ceiling_reports_that_command_effects_are_not_covered() {
    let fixture = fixture_with("census-ceiling", |config| {
        config.checkpoint_census_max_paths = 1;
    });
    fixture.git_init();
    fixture.write("a.txt", "one\n");
    fixture.write("b.txt", "two\n");
    let manifest = command_checkpoint(&fixture, "session_1");
    let before = fixture
        .store
        .begin_command_census(&fixture.repository)
        .unwrap();

    fixture.write("a.txt", "changed by the command\n");

    let manifest = fixture
        .store
        .record_command_effects(&fixture.repository, &manifest, &before)
        .unwrap();

    assert!(!manifest.command_effects_covered);
    assert!(manifest.files.is_empty());
}

#[test]
fn a_restricted_path_a_command_changed_is_not_censused() {
    let fixture = fixture("census-restricted");
    fixture.git_init();
    fixture.write("a.txt", "one\n");
    fixture.git_commit_all("initial");
    let manifest = command_checkpoint(&fixture, "session_1");
    let before = fixture
        .store
        .begin_command_census(&fixture.repository)
        .unwrap();

    fixture.write(".env", &format!("{SEEDED_SECRET}\n"));

    let manifest = fixture
        .store
        .record_command_effects(&fixture.repository, &manifest, &before)
        .unwrap();

    assert!(manifest.files.iter().all(|file| file.path != ".env"));
}

// §5.1's boundary: the census reads the user's repository and never writes to
// it. Its `.git` must come back byte for byte as it was.
#[test]
fn the_census_does_not_write_to_the_users_git_directory() {
    let fixture = fixture("census-readonly");
    fixture.git_init();
    fixture.write("a.txt", "one\n");
    fixture.git_commit_all("initial");
    fixture.write("a.txt", "dirty\n");
    let before_git = git_snapshot(&fixture.repository);

    let census = fixture
        .store
        .begin_command_census(&fixture.repository)
        .unwrap();
    let manifest = command_checkpoint(&fixture, "session_1");
    fixture
        .store
        .record_command_effects(&fixture.repository, &manifest, &census)
        .unwrap();

    assert_eq!(git_snapshot(&fixture.repository), before_git);
}

// A checkpoint is created before the turn, when nothing yet knows which files
// the turn will touch. Paths are added as the turn accepts a patch or runs a
// command, and each is snapshotted as it is at that moment.
#[test]
fn paths_added_during_the_turn_are_snapshotted_and_restorable() {
    let fixture = fixture("add-paths");
    fixture.write("src/a.rs", "original a\n");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: a patch", Vec::new()),
        )
        .unwrap();
    assert!(manifest.files.is_empty());

    let manifest = fixture
        .store
        .add_paths(&fixture.repository, &manifest, vec![patch_path("src/a.rs")])
        .expect("paths should be added");
    fixture.write("src/a.rs", "changed by the patch\n");
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    assert_eq!(result.restored_files, vec!["src/a.rs".to_string()]);
    assert_eq!(fixture.read("src/a.rs"), "original a\n");
}

// The earlier snapshot is the useful one: a path a patch captured before the
// turn must not be re-captured with the content a later action left there.
#[test]
fn adding_a_path_twice_keeps_the_first_snapshot() {
    let fixture = fixture("add-paths-twice");
    fixture.write("src/a.rs", "original a\n");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: two actions",
                vec![patch_path("src/a.rs")],
            ),
        )
        .unwrap();
    fixture.write("src/a.rs", "changed once\n");

    let manifest = fixture
        .store
        .add_paths(&fixture.repository, &manifest, vec![patch_path("src/a.rs")])
        .unwrap();

    assert_eq!(manifest.files.len(), 1);
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();
    assert_eq!(fixture.read("src/a.rs"), "original a\n");
}

// The documented development setup puts the data directory inside the
// repository (`DAMAIAN_DATA_DIR=.damaian`). Snapshotting it would put
// Damaian's own audit log, sessions, and checkpoint store into a checkpoint,
// and a rewind would then roll back Damaian's own state.
#[test]
fn damaians_own_data_directory_is_never_snapshotted() {
    let root = temp_data_dir_root("inside");
    let repository = root.join("repo");
    fs::create_dir_all(&repository).unwrap();
    let data_dir = repository.join(".damaian");
    fs::create_dir_all(&data_dir).unwrap();
    let config = Config {
        data_dir: data_dir.clone(),
        audit_enabled: true,
        ..Config::default()
    };
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let audit_log = AuditLog::new(&config.data_dir, true, scanner.clone());
    let path_policy = PathPolicy::new(&config);
    let store = CheckpointStore::new(config, audit_log, path_policy);
    fs::write(data_dir.join("notes.txt"), "damaian's own state\n").unwrap();

    let manifest = store
        .create_checkpoint(
            &repository,
            request(
                "session_1",
                "Before: a turn",
                vec![patch_path(".damaian/notes.txt")],
            ),
        )
        .unwrap();

    assert!(manifest.files.is_empty());
    assert_eq!(manifest.excluded.len(), 1);
    assert_eq!(manifest.excluded[0].reason, "damaian_data_directory");
}

fn temp_data_dir_root(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "damaian-checkpoints-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

// Requirement 15's bound. Content addressing is what makes a long session
// affordable: each turn stores only the blobs whose bytes actually changed and
// shares everything else, so the store grows with edits rather than with turns.
// The assertion is deliberately loose — it is a regression guard on the order
// of magnitude, not a benchmark.
#[test]
fn a_hundred_turn_session_stays_within_its_documented_storage_bound() {
    let fixture = fixture("hundred-turns");
    let filler = "// a line of source that stands in for real code\n".repeat(200);
    for name in ["src/a.rs", "src/b.rs", "src/c.rs"] {
        fixture.write(name, &filler);
    }
    let mut repository_id = String::new();

    for turn in 0..100 {
        let manifest = fixture
            .store
            .create_checkpoint(
                &fixture.repository,
                request(
                    "session_1",
                    "Before: one of a hundred turns",
                    vec![
                        patch_path("src/a.rs"),
                        patch_path("src/b.rs"),
                        patch_path("src/c.rs"),
                    ],
                ),
            )
            .unwrap();
        repository_id = manifest.repository_id.clone();
        // Each turn changes one line in each file, the way a real turn does.
        for name in ["src/a.rs", "src/b.rs", "src/c.rs"] {
            fixture.write(name, &format!("// turn {turn}\n{filler}"));
        }
        fixture
            .store
            .seal_checkpoint(&fixture.repository, &manifest)
            .unwrap();
    }

    let bytes = directory_bytes(&fixture.data_dir.join("checkpoints").join(&repository_id));
    // 100 turns x 3 files x ~9 KB of changed content is ~2.7 MB uncompressed;
    // zlib-deflated near-identical sources land far below that.
    assert!(
        bytes < 8 * 1024 * 1024,
        "checkpoint store for a 100-turn session grew to {bytes} bytes"
    );
    println!("100-turn checkpoint store: {bytes} bytes");
}

fn directory_bytes(root: &Path) -> u64 {
    snapshot(root).iter().map(|(_, size)| size).sum()
}

// §6: a symlink pointing outside the repository is not followed. `.env` and
// friends are refused by pattern; this is the other escape — a path inside the
// repository whose real target is not.
#[cfg(unix)]
#[test]
fn a_symlink_pointing_outside_the_repository_is_not_snapshotted() {
    let fixture = fixture("symlink-snapshot");
    let outside = fixture.repository.parent().unwrap().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let secret = outside.join("secret.txt");
    fs::write(&secret, format!("{SEEDED_SECRET}\n")).unwrap();
    std::os::unix::fs::symlink(&secret, fixture.repository.join("linked.txt")).unwrap();

    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: follow the link",
                vec![patch_path("linked.txt")],
            ),
        )
        .unwrap();

    assert!(manifest.files.is_empty());
    assert_eq!(manifest.excluded.len(), 1);
    assert_eq!(manifest.excluded[0].reason, "outside_repository");
}

// The same escape in the other direction: the checkpoint covers a real file,
// and by restore time the path is a symlink out of the repository. Writing the
// content back would write through the link.
#[cfg(unix)]
#[test]
fn restore_does_not_write_through_a_symlink_out_of_the_repository() {
    let fixture = fixture("symlink-restore");
    fixture.write("app.js", "const a = 1;\n");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: a normal file",
                vec![patch_path("app.js")],
            ),
        )
        .unwrap();
    fixture.write("app.js", "const a = 2;\n");
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();

    // Between the turn and the rewind, the path becomes a link out of the tree.
    let outside = fixture.repository.parent().unwrap().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let target = outside.join("target.txt");
    fs::write(&target, "someone else's file\n").unwrap();
    fs::remove_file(fixture.repository.join("app.js")).unwrap();
    std::os::unix::fs::symlink(&target, fixture.repository.join("app.js")).unwrap();

    let result = fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .expect("restore should report the refusal rather than failing");

    assert!(result.restored_files.is_empty());
    assert_eq!(result.skipped_files, vec!["app.js".to_string()]);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "someone else's file\n"
    );
}

// The UI always restores from a manifest it read back from disk, never from the
// one create_checkpoint returned, so the sealed state has to survive the
// round-trip through JSON.
#[test]
fn a_manifest_read_back_from_disk_restores_the_same_way() {
    let fixture = fixture("roundtrip");
    fixture.write("app.js", "const a = 1;\n");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: bump", vec![patch_path("app.js")]),
        )
        .unwrap();
    fixture.write("app.js", "const a = 2;\n");
    fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();

    let reloaded = fixture
        .store
        .read_checkpoint(&manifest.repository_id, &manifest.checkpoint_id)
        .unwrap()
        .expect("the manifest should read back");
    let result = fixture
        .store
        .restore(&fixture.repository, &reloaded, files_only(), "user")
        .unwrap();

    assert_eq!(result.conflicted_files, Vec::<String>::new());
    assert_eq!(result.restored_files, vec!["app.js".to_string()]);
    assert_eq!(fixture.read("app.js"), "const a = 1;\n");
}

// Rewinding one file and then the whole turn is an ordinary thing to do, and
// the second rewind must not report the file the first one restored as a
// conflict: after a restore, what Damaian last left at that path *is* the
// checkpoint's content.
#[test]
fn rewinding_the_same_checkpoint_twice_does_not_conflict_with_itself() {
    let fixture = fixture("twice");
    fixture.write("src/a.rs", "original a\n");
    fixture.write("src/b.rs", "original b\n");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request(
                "session_1",
                "Before: touch both",
                vec![patch_path("src/a.rs"), patch_path("src/b.rs")],
            ),
        )
        .unwrap();
    fixture.write("src/a.rs", "changed a\n");
    fixture.write("src/b.rs", "changed b\n");
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();

    let first = fixture
        .store
        .restore(
            &fixture.repository,
            &manifest,
            CheckpointRestoreOptions {
                files: true,
                conversation: false,
                only_path: Some("src/a.rs"),
            },
            "user",
        )
        .unwrap();
    assert_eq!(first.restored_files, vec!["src/a.rs".to_string()]);

    // The manifest the UI reaches for next is the one on disk.
    let reloaded = fixture
        .store
        .read_checkpoint(&manifest.repository_id, &manifest.checkpoint_id)
        .unwrap()
        .unwrap();
    let second = fixture
        .store
        .restore(&fixture.repository, &reloaded, files_only(), "user")
        .unwrap();

    assert_eq!(second.conflicted_files, Vec::<String>::new());
    assert_eq!(fixture.read("src/a.rs"), "original a\n");
    assert_eq!(fixture.read("src/b.rs"), "original b\n");
}

/// Measures what a working-tree census costs on a real repository, which is the
/// number `checkpoint_census_max_paths` has to be chosen against.
///
/// `#[ignore]`d because it reads the developer's own checkout — the one
/// repository a test has no business assuming anything about. Run it by hand:
///
/// ```sh
/// cargo test -p workspace-engine --test checkpoints -- --ignored --nocapture census_cost
/// ```
#[test]
#[ignore]
fn measures_census_cost_on_this_repository() {
    let fixture = fixture("census-cost");
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();

    let started = SystemTime::now();
    let census = fixture
        .store
        .begin_command_census(&repository)
        .expect("census should run on this repository");
    let elapsed = started.elapsed().expect("clock should work");

    println!(
        "census of {}: {} paths in {} ms (truncated: {})",
        repository.display(),
        census.path_count(),
        elapsed.as_millis(),
        census.truncated()
    );
}

// The census writes its blobs in one batched git call, and git can apply
// `.gitattributes` clean filters when it is given paths rather than bytes.
// A file with CRLF line endings and a NUL byte is where that would show: the
// store promises the user's bytes, not git's idea of them.
#[test]
fn a_census_stores_bytes_faithfully_including_crlf_and_binary() {
    let fixture = fixture("census-bytes");
    fixture.git_init();
    let original = "first\r\nsecond\r\n\u{0}\u{1}\u{2}binary\r\n";
    fixture.write(".gitattributes", "* text=auto\n");
    fixture.write("data.bin", original);
    fixture.git_commit_all("initial");
    let manifest = fixture
        .store
        .create_checkpoint(
            &fixture.repository,
            request("session_1", "Before: rewrite it", Vec::new()),
        )
        .unwrap();
    let before = fixture
        .store
        .begin_command_census(&fixture.repository)
        .unwrap();

    fixture.write("data.bin", "replaced by the command\n");

    let manifest = fixture
        .store
        .record_command_effects(&fixture.repository, &manifest, &before)
        .unwrap();
    let manifest = fixture
        .store
        .seal_checkpoint(&fixture.repository, &manifest)
        .unwrap();
    fixture
        .store
        .restore(&fixture.repository, &manifest, files_only(), "user")
        .unwrap();

    assert_eq!(fixture.read("data.bin"), original);
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o111 != 0
}

/// The user's `.git`, ignoring lock files: those are git's own transient
/// bookkeeping, and the claim under test is that Damaian writes nothing.
fn git_snapshot(repository: &Path) -> Vec<(PathBuf, u64)> {
    snapshot(&repository.join(".git"))
        .into_iter()
        .filter(|(path, _)| path.extension().and_then(|value| value.to_str()) != Some("lock"))
        .collect()
}

fn snapshot(root: &Path) -> Vec<(PathBuf, u64)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                entries.push((entry.path(), metadata.len()));
            }
        }
    }
    entries.sort();
    entries
}
