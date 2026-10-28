//! Commit preparation end to end, per `docs/specs/35_commit_preparation/`.
//!
//! Every fixture is a throwaway Git repository with its data directory
//! **beside** it, so `git status` never sees Damaian's own state as a candidate
//! (context §2.15). Tests that would run real hooks are here too: the hook is a
//! script the fixture writes, and no user process or checkout is touched.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CancelToken, CheckpointConversation, CheckpointOrigin, CheckpointPath, CheckpointRequest,
    CommitCandidateSource, CommitError, CommitRequest, Config, ProposedChange, Session,
    SessionMode, Task, WorkspaceEngine, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    base: PathBuf,
    repository: PathBuf,
    data_dir: PathBuf,
    engine: WorkspaceEngine,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "damaian-commit-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let repository = base.join("repo");
        let data_dir = base.join("data");
        fs::create_dir_all(&repository).unwrap();
        let mut fixture = Self {
            base,
            repository,
            data_dir,
            engine: WorkspaceEngine::new(Config::default()),
        };
        fixture.engine = WorkspaceEngine::new(Config {
            data_dir: fixture.data_dir.clone(),
            enable_index_watcher: false,
            ..Config::default()
        });
        fixture.git_init();
        fixture
    }

    fn git_init(&self) {
        self.git(&["init", "--quiet", "--initial-branch=main"]);
        self.git(&["config", "user.email", "test@damaian.local"]);
        self.git(&["config", "user.name", "Damaian Test"]);
        self.git(&["config", "commit.gpgsign", "false"]);
        self.git(&["config", "gc.auto", "0"]);
        self.git(&["config", "maintenance.auto", "false"]);
        // The mode tests need Git to report the executable bit. It is the
        // macOS default; setting it explicitly makes the fixture independent
        // of the machine's Git configuration.
        self.git(&["config", "core.fileMode", "true"]);
    }

    fn git(&self, args: &[&str]) -> String {
        self.git_at(&self.repository, args).trim().to_string()
    }

    fn git_raw(&self, args: &[&str]) -> String {
        self.git_at(&self.repository, args)
    }

    fn git_at(&self, cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn write(&self, path: &str, content: &str) {
        let full = self.repository.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.repository.join(path)).unwrap()
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "--quiet", "-m", message]);
    }

    fn session(&self) -> (Session, Task) {
        let repository_id = repository_id_for_root(&self.repository);
        let session = self
            .engine
            .session_store
            .create_session(&repository_id, "Commit test")
            .unwrap();
        self.engine
            .session_store
            .append_message(&session.id, None, "user", "Add a feature")
            .unwrap();
        let task = self
            .engine
            .session_store
            .create_task(&session.id, "Add a feature", "test", "test")
            .unwrap();
        (session, task)
    }

    fn apply(
        &self,
        session: &Session,
        task: &Task,
        files: &[(&str, &str)],
        approved: Option<&[String]>,
        hunks: Option<&HashMap<String, Vec<String>>>,
    ) -> String {
        self.apply_with(session, task, files, approved, hunks, false)
    }

    fn apply_with(
        &self,
        session: &Session,
        task: &Task,
        files: &[(&str, &str)],
        approved: Option<&[String]>,
        hunks: Option<&HashMap<String, Vec<String>>>,
        allow_secrets: bool,
    ) -> String {
        let changes = files
            .iter()
            .map(|(path, content)| ProposedChange {
                path: path.to_string(),
                new_content: content.to_string(),
                status: None,
                allow_restricted: false,
            })
            .collect::<Vec<_>>();
        let mut patch = self
            .engine
            .patch_engine
            .create_patch(&self.repository, &changes, Some(&task.id), "test")
            .unwrap();
        patch.session_id = session.id.clone();
        patch.origin_session_id = session.id.clone();
        self.engine.patch_store.save(&patch).unwrap();
        self.engine
            .edit_orchestrator
            .apply_stored_patch(
                &self.repository,
                &patch.id,
                approved,
                hunks,
                "tester",
                allow_secrets,
            )
            .unwrap();
        patch.id
    }

    fn apply_deleted(&self, session: &Session, task: &Task, path: &str) -> String {
        let mut patch = self
            .engine
            .patch_engine
            .create_patch(
                &self.repository,
                &[workspace_engine::ProposedChange {
                    path: path.to_string(),
                    new_content: String::new(),
                    status: Some("deleted".to_string()),
                    allow_restricted: false,
                }],
                Some(&task.id),
                "delete",
            )
            .unwrap();
        patch.session_id = session.id.clone();
        patch.origin_session_id = session.id.clone();
        self.engine.patch_store.save(&patch).unwrap();
        self.engine
            .edit_orchestrator
            .apply_stored_patch(&self.repository, &patch.id, None, None, "tester", false)
            .unwrap();
        patch.id
    }

    fn prepare(&self, session: &Session) -> workspace_engine::CommitPreparation {
        self.engine
            .commit
            .prepare(&self.repository, &session.id)
            .unwrap()
    }

    fn commit(
        &self,
        session: &Session,
        preparation: &workspace_engine::CommitPreparation,
        paths: &[String],
        message: &str,
        allow_secrets: bool,
    ) -> Result<workspace_engine::CommitOutcome, CommitError> {
        self.engine.commit.commit(
            &self.repository,
            &CommitRequest {
                session_id: session.id.clone(),
                prepare_id: preparation.prepare_id.clone(),
                selected_paths: paths.to_vec(),
                message: message.to_string(),
                allow_generated_secrets: allow_secrets,
                approved_by: "tester".to_string(),
            },
            &CancelToken::new(),
        )
    }

    fn default_paths(&self, preparation: &workspace_engine::CommitPreparation) -> Vec<String> {
        preparation
            .candidates
            .iter()
            .filter(|candidate| candidate.selected_by_default)
            .map(|candidate| candidate.path.clone())
            .collect()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn tree_paths(&self) -> Vec<String> {
        self.git(&["ls-tree", "-r", "--name-only", "HEAD"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn show(&self, path: &str) -> String {
        self.git_raw(&["show", &format!("HEAD:{path}")])
    }

    fn index_bytes(&self) -> Vec<u8> {
        fs::read(self.repository.join(".git/index")).unwrap_or_default()
    }

    fn rebuild(&mut self, change: impl FnOnce(&mut Config)) {
        let mut config = Config {
            data_dir: self.data_dir.clone(),
            enable_index_watcher: false,
            ..Config::default()
        };
        change(&mut config);
        self.engine = WorkspaceEngine::new(config);
    }

    fn write_hook(&self, name: &str, body: &str, executable: bool) {
        let path = self.repository.join(".git/hooks").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if executable { 0o755 } else { 0o644 });
        fs::set_permissions(&path, permissions).unwrap();
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn changed(files: &[&str]) -> Vec<String> {
    files.iter().map(|path| path.to_string()).collect()
}

#[test]
fn a_commit_contains_only_the_accepted_files() {
    let fixture = Fixture::new("accepted");
    for name in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"] {
        fixture.write(name, "original\n");
    }
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    let accepted = ["a.txt", "b.txt", "c.txt"];
    let changes = [
        ("a.txt", "changed a.txt\n"),
        ("b.txt", "changed b.txt\n"),
        ("c.txt", "changed c.txt\n"),
        ("d.txt", "changed d.txt\n"),
        ("e.txt", "changed e.txt\n"),
    ];
    for (name, content) in changes {
        fixture.write(name, content);
    }
    let approved = changed(&accepted);
    fixture.apply(&session, &task, &changes, Some(approved.as_slice()), None);

    let preparation = fixture.prepare(&session);
    assert_eq!(
        fixture.default_paths(&preparation),
        vec!["a.txt", "b.txt", "c.txt"]
    );
    let outcome = fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Add three files",
            false,
        )
        .unwrap();

    assert_eq!(outcome.path_count, 3);
    assert_eq!(
        fixture.tree_paths(),
        vec!["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]
    );
    for name in accepted {
        assert_eq!(fixture.show(name), format!("changed {name}\n"));
    }
    assert_eq!(fixture.show("d.txt"), "original\n");
    assert_eq!(fixture.show("e.txt"), "original\n");
    fixture.cleanup();
}

#[test]
fn a_hunk_level_acceptance_commits_only_the_accepted_hunks() {
    let fixture = Fixture::new("hunks");
    let original = (1..=20)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    fixture.write("long.txt", &original);
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    let mut content = original.clone();
    content = content.replace("line 2\n", "line 2 changed\n");
    content = content.replace("line 18\n", "line 18 changed\n");
    let patch_id = fixture.apply(&session, &task, &[("long.txt", &content)], None, None);
    let patch = fixture.engine.patch_store.load(&patch_id).unwrap();
    assert_eq!(patch.files[0].hunks.len(), 2, "the fixture needs two hunks");
    let accepted = patch.files[0].hunks[0].id.clone();
    let mut selection = HashMap::new();
    selection.insert("long.txt".to_string(), vec![accepted]);
    // Revert, then apply only the first hunk through the real orchestrator.
    fixture.write("long.txt", &original);
    fixture
        .engine
        .edit_orchestrator
        .apply_stored_patch(
            &fixture.repository,
            &patch_id,
            Some(&["long.txt".to_string()]),
            Some(&selection),
            "tester",
            false,
        )
        .unwrap();
    let partially_applied = fixture.read("long.txt");
    assert!(partially_applied.contains("line 2 changed"));
    assert!(partially_applied.contains("line 18\n"));

    let preparation = fixture.prepare(&session);
    assert_eq!(fixture.default_paths(&preparation), vec!["long.txt"]);
    fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Change line 2",
            false,
        )
        .unwrap();
    assert_eq!(fixture.show("long.txt"), partially_applied);
    assert!(!fixture.show("long.txt").contains("line 18 changed"));
    fixture.cleanup();
}

#[test]
fn a_hand_edited_accepted_file_is_shown_labelled_and_unselected() {
    let fixture = Fixture::new("hand-edited");
    fixture.write("clean.txt", "original\n");
    fixture.write("edited.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("clean.txt", "damaian\n");
    fixture.write("edited.txt", "damaian\n");
    fixture.apply(
        &session,
        &task,
        &[("clean.txt", "damaian\n"), ("edited.txt", "damaian\n")],
        None,
        None,
    );
    fixture.write("edited.txt", "damaian\nthen the user typed this\n");

    let preparation = fixture.prepare(&session);
    let edited = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "edited.txt")
        .expect("edited file is a candidate");
    assert_eq!(edited.source, CommitCandidateSource::AcceptedThenEdited);
    assert!(!edited.selected_by_default);
    assert!(edited.label.contains("edited by hand"));
    assert_eq!(
        fixture.default_paths(&preparation),
        vec!["clean.txt".to_string()]
    );

    fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit the clean file",
            false,
        )
        .unwrap();
    assert_eq!(fixture.show("clean.txt"), "damaian\n");
    assert_eq!(fixture.show("edited.txt"), "original\n");
    fixture.cleanup();
}

#[test]
fn a_command_changed_file_is_shown_labelled_and_unselected() {
    let fixture = Fixture::new("command-effect");
    fixture.write("generated.txt", "original\n");
    fixture.write("accepted.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture
        .engine
        .checkpoint_store
        .create_checkpoint(
            &fixture.repository,
            CheckpointRequest {
                session_id: &session.id,
                task_id: Some(&task.id),
                user_message_id: None,
                summary: "Before a command",
                conversation: CheckpointConversation {
                    last_event_seq: 1,
                    task_status: "running_tool".to_string(),
                },
                pending_approvals: Vec::new(),
                paths: vec![CheckpointPath {
                    path: "generated.txt".to_string(),
                    origin: CheckpointOrigin::Command,
                }],
            },
        )
        .unwrap();
    fixture.write("generated.txt", "written by a command\n");
    fixture.write("accepted.txt", "damaian\n");
    fixture.apply(
        &session,
        &task,
        &[("accepted.txt", "damaian\n")],
        None,
        None,
    );

    let preparation = fixture.prepare(&session);
    let generated = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "generated.txt")
        .expect("generated file is a candidate");
    assert_eq!(generated.source, CommitCandidateSource::CommandEffect);
    assert!(!generated.selected_by_default);
    assert!(generated.label.contains("approved command"));
    assert_eq!(
        fixture.default_paths(&preparation),
        vec!["accepted.txt".to_string()]
    );
    fixture.cleanup();
}

#[test]
fn a_commit_updates_the_committed_paths_in_the_users_index() {
    let fixture = Fixture::new("index");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    fixture.write("staged.txt", "the user staged this\n");
    fixture.git(&["add", "staged.txt"]);
    let staged_before = fs::read(fixture.repository.join("staged.txt")).unwrap();
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.write("new.txt", "a new file\n");
    fixture.apply(
        &session,
        &task,
        &[("a.txt", "damaian\n"), ("new.txt", "a new file\n")],
        None,
        None,
    );

    let preparation = fixture.prepare(&session);
    fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit a",
            false,
        )
        .unwrap();

    // The user's own staging is still theirs.
    assert_eq!(
        fs::read(fixture.repository.join("staged.txt")).unwrap(),
        staged_before,
        "the working tree must not change"
    );
    let staged = fixture.git(&["diff", "--cached", "--name-only"]);
    assert_eq!(staged, "staged.txt", "only the user's staging is staged");
    // The committed paths now agree with the new HEAD, so nothing shows as a
    // staged reversal of the commit (the defect that made the next plain
    // `git commit` undo it).
    assert!(
        !staged.contains("a.txt") && !staged.contains("new.txt"),
        "{staged}"
    );
    assert_eq!(
        fixture.git(&["status", "--porcelain"]),
        "A  staged.txt",
        "the index must agree with the new HEAD at every committed path"
    );
    assert!(!fixture.tree_paths().contains(&"staged.txt".to_string()));
    fixture.cleanup();
}

#[test]
fn a_plain_commit_after_a_damaian_commit_does_not_revert_it() {
    let fixture = Fixture::new("follow-up-commit");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.write("new.txt", "a new file\n");
    fixture.apply(
        &session,
        &task,
        &[("a.txt", "damaian\n"), ("new.txt", "a new file\n")],
        None,
        None,
    );
    let preparation = fixture.prepare(&session);
    let first = fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Damaian commit",
            false,
        )
        .unwrap();

    fixture.write("user.txt", "the user's own work\n");
    fixture.git(&["add", "user.txt"]);
    assert_eq!(
        fixture.git(&["status", "--porcelain"]),
        "A  user.txt",
        "the commit must leave no staged reversal behind"
    );

    // The user's next plain `git commit` stages whatever was in the index. If
    // the index still described the old HEAD, this commit would revert
    // Damaian's change and delete the file it added.
    fixture.git(&["commit", "--quiet", "-m", "user work"]);
    assert_eq!(fixture.show("a.txt"), "damaian\n");
    assert_eq!(fixture.show("new.txt"), "a new file\n");
    assert!(
        fixture.repository.join("new.txt").exists(),
        "the follow-up commit deleted the file Damaian added"
    );
    assert_eq!(
        fixture.git(&["rev-parse", "HEAD~1"]),
        first.commit_sha,
        "Damaian's commit must still be the parent"
    );
    fixture.cleanup();
}

#[test]
fn a_prepare_id_cannot_escape_the_commit_state_directory() {
    let fixture = Fixture::new("prepare-id");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    let preparation = fixture.prepare(&session);

    // A valid record planted one directory above the records directory, so a
    // `..` id would load it, plus a directory the private-index cleanup would
    // delete on the way.
    let record_path = fixture
        .data_dir
        .join("commits")
        .join(format!("{}.json", preparation.prepare_id));
    let mut record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    record["prepareId"] = serde_json::Value::String("../victim".to_string());
    fs::write(
        fixture.data_dir.join("victim.json"),
        serde_json::to_string(&record).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(fixture.data_dir.join("victim")).unwrap();
    fs::write(fixture.data_dir.join("victim/keep.txt"), "keep\n").unwrap();
    let before = fixture.head();

    let outcome = fixture.engine.commit.commit(
        &fixture.repository,
        &CommitRequest {
            session_id: session.id.clone(),
            prepare_id: "../victim".to_string(),
            selected_paths: vec!["a.txt".to_string()],
            message: "Commit a".to_string(),
            allow_generated_secrets: false,
            approved_by: "tester".to_string(),
        },
        &CancelToken::new(),
    );
    match outcome {
        Err(error) => assert_eq!(error.code(), "commit_not_prepared", "{error:?}"),
        Ok(outcome) => panic!("a traversal prepare id committed: {outcome:?}"),
    }
    assert!(
        fixture.data_dir.join("victim/keep.txt").exists(),
        "a traversal prepare id deleted a directory outside commits/"
    );
    assert_eq!(fixture.head(), before);
    assert!(fixture.engine.commit.load_record("../victim").is_err());
    fixture.cleanup();
}

#[test]
fn head_moving_between_prepare_and_commit_fails_with_nothing_changed() {
    let fixture = Fixture::new("head-moved");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    let preparation = fixture.prepare(&session);
    let before = fixture.head();

    fixture.git(&[
        "commit",
        "--allow-empty",
        "--quiet",
        "-m",
        "somebody else committed",
    ]);
    let other = fixture.head();
    assert_ne!(before, other);
    let index_before = fixture.index_bytes();
    let worktree_before = fs::read(fixture.repository.join("a.txt")).unwrap();

    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    assert!(matches!(
        outcome,
        Err(CommitError::Refused {
            code: "commit_head_moved",
            ..
        })
    ));
    assert_eq!(fixture.head(), other, "nothing was committed");
    assert_eq!(fixture.index_bytes(), index_before);
    assert_eq!(
        fs::read(fixture.repository.join("a.txt")).unwrap(),
        worktree_before
    );
    fixture.cleanup();
}

#[test]
fn a_detached_head_and_a_merge_in_progress_are_refused() {
    let fixture = Fixture::new("states");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);

    fixture.git(&["checkout", "--quiet", "--detach", "HEAD"]);
    let preparation = fixture.prepare(&session);
    assert!(
        preparation
            .refusals
            .iter()
            .any(|refusal| refusal.contains("detached")),
        "{:?}",
        preparation.refusals
    );
    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    assert!(matches!(
        outcome,
        Err(CommitError::Refused {
            code: "commit_not_allowed",
            ..
        })
    ));

    fixture.git(&["checkout", "--quiet", "main"]);
    fs::write(fixture.repository.join(".git/MERGE_HEAD"), "deadbeef\n").unwrap();
    let preparation = fixture.prepare(&session);
    assert!(
        preparation
            .refusals
            .iter()
            .any(|refusal| refusal.contains("MERGE_HEAD")),
        "{:?}",
        preparation.refusals
    );
    fixture.cleanup();
}

#[test]
fn committing_is_refused_outside_code_mode_and_without_the_profile_capability() {
    let fixture = Fixture::new("gate");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);

    fixture
        .engine
        .session_store
        .set_session_mode(&session.id, SessionMode::Ask, "user")
        .unwrap();
    let preparation = fixture.prepare(&session);
    assert!(
        preparation
            .refusals
            .iter()
            .any(|refusal| refusal.contains("Code mode")),
        "{:?}",
        preparation.refusals
    );
    for mode in [SessionMode::Plan, SessionMode::Review] {
        fixture
            .engine
            .session_store
            .set_session_mode(&session.id, mode, "user")
            .unwrap();
        let preparation = fixture.prepare(&session);
        assert!(
            preparation
                .refusals
                .iter()
                .any(|refusal| refusal.contains("Code mode")),
            "{mode:?}: {:?}",
            preparation.refusals
        );
    }
    fixture
        .engine
        .session_store
        .set_session_mode(&session.id, SessionMode::Code, "user")
        .unwrap();

    let mut config = Config {
        data_dir: fixture.data_dir.clone(),
        enable_index_watcher: false,
        ..Config::default()
    };
    config.allow_git_commits = false;
    let narrowed = WorkspaceEngine::new(config);
    let preparation = narrowed
        .commit
        .prepare(&fixture.repository, &session.id)
        .unwrap();
    assert!(
        preparation
            .refusals
            .iter()
            .any(|refusal| refusal.contains("allow_git_commits=false")),
        "{:?}",
        preparation.refusals
    );
    fixture.cleanup();
}

#[test]
fn the_selected_candidate_must_be_part_of_the_preparation() {
    let fixture = Fixture::new("selection");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    let preparation = fixture.prepare(&session);
    let outcome = fixture.commit(
        &session,
        &preparation,
        &["not-a-candidate.txt".to_string()],
        "Commit a",
        false,
    );
    assert!(matches!(
        outcome,
        Err(CommitError::Refused {
            code: "commit_selection_invalid",
            ..
        })
    ));
    fixture.cleanup();
}

#[test]
fn hooks_are_reported_with_path_executable_size_and_interpreter() {
    let fixture = Fixture::new("hooks-report");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    fixture.write_hook("pre-commit", "#!/bin/sh\nexit 0\n", true);
    fixture.write_hook("commit-msg", "#!/bin/sh\nexit 0\n", false);
    let (session, _task) = fixture.session();

    let preparation = fixture.prepare(&session);
    let pre = preparation
        .hooks
        .hooks
        .iter()
        .find(|hook| hook.name == "pre-commit")
        .unwrap();
    assert!(pre.exists);
    assert!(pre.executable);
    assert!(pre.runs);
    assert!(pre.size > 0);
    assert!(pre.content_hash.is_some());
    assert_eq!(pre.interpreter.as_deref(), Some("#!/bin/sh"));
    assert!(pre.path.ends_with(".git/hooks/pre-commit"));
    let commit_msg = preparation
        .hooks
        .hooks
        .iter()
        .find(|hook| hook.name == "commit-msg")
        .unwrap();
    assert!(commit_msg.exists);
    assert!(!commit_msg.executable);
    assert!(!commit_msg.runs);
    assert!(commit_msg.skipped_reason.is_some());

    fixture.git(&["config", "core.hooksPath", ".githooks"]);
    let hook = fixture.repository.join(".githooks/pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = fs::metadata(&hook).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).unwrap();
    let preparation = fixture.prepare(&session);
    assert_eq!(preparation.hooks.source, "core.hooksPath");
    assert!(preparation.hooks.directory.ends_with(".githooks"));
    assert!(
        preparation
            .hooks
            .hooks
            .iter()
            .find(|hook| hook.name == "pre-commit")
            .unwrap()
            .runs
    );
    fixture.cleanup();
}

#[test]
fn a_failing_pre_commit_aborts_and_reports_its_output() {
    let fixture = Fixture::new("pre-commit-fail");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.write_hook(
        "pre-commit",
        "#!/bin/sh\necho 'no commit today'\nexit 1\n",
        true,
    );
    let before = fixture.head();
    let preparation = fixture.prepare(&session);
    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    match outcome {
        Err(CommitError::HookFailed {
            name,
            stdout,
            exit_code,
            ..
        }) => {
            assert_eq!(name, "pre-commit");
            assert_eq!(exit_code, Some(1));
            assert!(stdout.contains("no commit today"), "{stdout}");
        }
        other => panic!("expected a hook failure, got {other:?}"),
    }
    assert_eq!(fixture.head(), before, "no commit was created");
    fixture.cleanup();
}

#[test]
fn a_commit_msg_hook_that_rewrites_the_message_is_shown_before_the_commit() {
    let fixture = Fixture::new("commit-msg-rewrite");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.write_hook(
        "commit-msg",
        "#!/bin/sh\ngrep -q '^Reviewed-by: Test$' \"$1\" || printf '\\nReviewed-by: Test\\n' >> \"$1\"\n",
        true,
    );
    let before = fixture.head();
    let preparation = fixture.prepare(&session);
    let first = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    let rewritten = match first {
        Err(CommitError::MessageRewritten { message }) => message,
        other => panic!("expected a rewritten message, got {other:?}"),
    };
    assert!(rewritten.contains("Reviewed-by: Test"));
    assert_eq!(
        fixture.head(),
        before,
        "nothing was committed on the first try"
    );

    fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            &rewritten,
            false,
        )
        .expect("the second approval commits the shown message");
    assert!(
        fixture
            .git(&["log", "-1", "--pretty=%B"])
            .contains("Reviewed-by: Test")
    );
    fixture.cleanup();
}

#[test]
fn a_commit_msg_hook_that_only_adds_comments_does_not_block_the_commit() {
    let fixture = Fixture::new("commit-msg-comment");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    // Git strips comment lines before writing the message, so a hook that
    // adds one has not changed the message the user approved.
    fixture.write_hook(
        "commit-msg",
        "#!/bin/sh\nprintf '\\n# reviewed by the hook\\n' >> \"$1\"\n",
        true,
    );
    let preparation = fixture.prepare(&session);
    let outcome = fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit a",
            false,
        )
        .expect("a comment line is not a message rewrite");
    assert_eq!(outcome.message, "Commit a");
    let log = fixture.git(&["log", "-1", "--pretty=%B"]);
    assert_eq!(log, "Commit a", "the comment did not reach the commit");
    fixture.cleanup();
}

#[test]
fn a_prepare_commit_msg_hook_is_run_and_its_rewrite_is_shown() {
    let fixture = Fixture::new("prepare-msg-rewrite");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.write_hook(
        "prepare-commit-msg",
        "#!/bin/sh\ngrep -q 'Ticket: 35' \"$1\" || printf '\\nTicket: 35\\n' >> \"$1\"\n",
        true,
    );
    let preparation = fixture.prepare(&session);
    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    match outcome {
        Err(CommitError::MessageRewritten { message }) => {
            assert!(message.contains("Ticket: 35"));
        }
        other => panic!("expected a rewritten message, got {other:?}"),
    }
    fixture.cleanup();
}

#[test]
fn a_staged_copy_that_cannot_be_updated_is_a_warning_after_the_commit() {
    let fixture = Fixture::new("index-lock");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    let preparation = fixture.prepare(&session);
    // The same failure a concurrent `git add` produces: the real index cannot
    // be written. The commit is real, so this must not be reported as a failed
    // commit, and its record, event and audit entry must still be written.
    fs::write(fixture.repository.join(".git/index.lock"), "").unwrap();
    let before = fixture.head();
    let outcome = fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit a",
            false,
        )
        .expect("the commit exists even though the index could not be updated");
    assert_ne!(fixture.head(), before);
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("staged copy")),
        "{:?}",
        outcome.warnings
    );
    let record = fixture
        .engine
        .commit
        .load_record(&preparation.prepare_id)
        .unwrap();
    assert_eq!(record.state, "created");
    let audit = fs::read_to_string(fixture.data_dir.join("audit/events.jsonl")).unwrap();
    assert!(audit.contains("commit_created"), "{audit}");
    fs::remove_file(fixture.repository.join(".git/index.lock")).unwrap();
    fixture.cleanup();
}

#[test]
fn a_failing_post_commit_hook_is_a_warning_and_the_commit_stays() {
    let fixture = Fixture::new("post-commit-fail");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.write_hook("post-commit", "#!/bin/sh\nexit 3\n", true);
    let before = fixture.head();
    let preparation = fixture.prepare(&session);
    let outcome = fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit a",
            false,
        )
        .unwrap();
    assert_ne!(fixture.head(), before);
    assert!(!outcome.warnings.is_empty());
    assert!(outcome.warnings[0].contains("post-commit"));
    fixture.cleanup();
}

#[test]
fn hooks_run_with_the_private_index() {
    let fixture = Fixture::new("hook-index");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    fixture.write("staged.txt", "the user staged this\n");
    fixture.git(&["add", "staged.txt"]);
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.write_hook(
        "pre-commit",
        "#!/bin/sh\ngit diff --cached --name-only > hook-index.txt\n",
        true,
    );
    let preparation = fixture.prepare(&session);
    fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit a",
            false,
        )
        .unwrap();
    let seen = fs::read_to_string(fixture.repository.join("hook-index.txt")).unwrap();
    assert!(seen.contains("a.txt"), "{seen}");
    assert!(
        !seen.contains("staged.txt"),
        "the hook saw the user's index: {seen}"
    );
    fixture.cleanup();
}

#[test]
fn a_hanging_hook_times_out() {
    let mut fixture = Fixture::new("hook-timeout");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.rebuild(|config| config.command_timeout_secs = 1);
    fixture.write_hook("pre-commit", "#!/bin/sh\nsleep 5\nexit 0\n", true);
    let preparation = fixture.prepare(&session);
    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    match outcome {
        Err(CommitError::HookFailed { termination, .. }) => {
            assert_eq!(termination, "timed_out");
        }
        other => panic!("expected a timeout, got {other:?}"),
    }
    fixture.cleanup();
}

#[test]
fn hook_output_is_truncated_and_redacted() {
    let mut fixture = Fixture::new("hook-output");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.rebuild(|config| config.max_command_output_bytes = 200);
    fixture.write_hook(
        "pre-commit",
        "#!/bin/sh\nprintf '%0.sx' $(seq 1 100)\nprintf 'AKIAIOSFODNN7EXAMPLE'\nprintf '%0.sy' $(seq 1 100)\nexit 1\n",
        true,
    );
    let preparation = fixture.prepare(&session);
    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    match outcome {
        Err(CommitError::HookFailed { stdout, stderr, .. }) => {
            assert!(!stdout.contains("AKIAIOSFODNN7EXAMPLE"), "{stdout}");
            assert!(!stderr.contains("AKIAIOSFODNN7EXAMPLE"), "{stderr}");
            // Redaction can replace a short match with a longer marker, so the
            // bound is on the raw output plus that expansion, not exactly 200.
            assert!(stdout.len() <= 400, "{} bytes", stdout.len());
        }
        other => panic!("expected a hook failure, got {other:?}"),
    }
    fixture.cleanup();
}

#[test]
fn there_is_no_hook_bypass() {
    let commit = include_str!("../src/commit.rs");
    let runner = include_str!("../src/command_runner.rs");
    for source in [commit, runner] {
        for line in source.lines() {
            let code = line.trim();
            if code.starts_with("//") {
                continue;
            }
            assert!(
                !code.contains("--no-verify"),
                "a code path passes --no-verify: {code}"
            );
        }
    }
}

#[test]
fn a_running_hook_is_registered_with_the_process_registry() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    let fixture = Fixture::new("hook-pid");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.write_hook(
        "pre-commit",
        "#!/bin/sh\nprintf '%s %s\\n' \"$$\" \"$PPID\" > hook-pids.txt\nsleep 2\nexit 0\n",
        true,
    );
    let preparation = fixture.prepare(&session);
    let seen = Arc::new(AtomicBool::new(false));
    let watcher = {
        let seen = Arc::clone(&seen);
        let data_dir = fixture.data_dir.clone();
        let repository = fixture.repository.clone();
        std::thread::spawn(move || {
            for _ in 0..80 {
                let pids = fs::read_to_string(repository.join("hook-pids.txt")).unwrap_or_default();
                let pids = pids
                    .split_whitespace()
                    .filter_map(|pid| pid.parse::<u32>().ok())
                    .collect::<Vec<_>>();
                if !pids.is_empty()
                    && let Ok(registry) = workspace_engine::ProcessRegistry::open(&data_dir)
                        .and_then(|r| {
                            r.entries().map(|entries| {
                                entries
                                    .into_iter()
                                    .filter_map(|(_, entry)| entry)
                                    .map(|entry| entry.pid)
                                    .collect::<Vec<_>>()
                            })
                        })
                    && pids.iter().any(|pid| registry.contains(pid))
                {
                    seen.store(true, AtomicOrdering::SeqCst);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        })
    };
    fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit a",
            false,
        )
        .unwrap();
    watcher.join().unwrap();
    assert!(
        seen.load(AtomicOrdering::SeqCst),
        "the running hook's pid was never registered"
    );
    fixture.cleanup();
}

const FAKE_AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

#[test]
fn the_approved_content_is_the_committed_content() {
    let fixture = Fixture::new("content-identity");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    let preparation = fixture.prepare(&session);
    fixture.write("a.txt", "changed after the preview\n");
    let before = fixture.head();
    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    assert!(matches!(
        outcome,
        Err(CommitError::Refused {
            code: "commit_content_changed",
            ..
        })
    ));
    assert_eq!(fixture.head(), before);

    fixture.write("a.txt", "damaian\n");
    fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit a",
            false,
        )
        .expect("the reviewed bytes are committed once they are back");
    assert_eq!(fixture.show("a.txt"), "damaian\n");
    fixture.cleanup();
}

#[test]
fn the_secret_scan_reads_the_blob_that_will_be_committed() {
    let fixture = Fixture::new("filtered-secret");
    fixture.write("filtered.txt", "original\n");
    fixture.commit_all("initial");
    // A clean filter that injects a credential: the working-tree bytes are
    // clean, the blob Git will store is not.
    fixture.git(&[
        "config",
        "filter.inject.clean",
        &format!("sed s/PLACEHOLDER/{FAKE_AWS_KEY}/"),
    ]);
    fixture.write(".gitattributes", "filtered.txt filter=inject\n");
    let (session, task) = fixture.session();
    let content = "PLACEHOLDER\n";
    fixture.write("filtered.txt", content);
    fixture.apply(&session, &task, &[("filtered.txt", content)], None, None);
    let preparation = fixture.prepare(&session);
    let selected = fixture.default_paths(&preparation);
    assert_eq!(selected, vec!["filtered.txt".to_string()]);

    // Scanning the working tree would see no secret and commit one.
    let refused = fixture.commit(&session, &preparation, &selected, "Commit", false);
    assert!(
        matches!(
            refused,
            Err(CommitError::Refused {
                code: "commit_secrets_require_override",
                ..
            })
        ),
        "{refused:?}"
    );
    // With the override, the committed bytes are the bytes that were scanned.
    let outcome = fixture
        .commit(&session, &preparation, &selected, "Commit", true)
        .expect("the override commits");
    assert!(outcome.generated_secret_override);
    assert_eq!(fixture.show("filtered.txt"), format!("{FAKE_AWS_KEY}\n"));
    fixture.cleanup();
}

#[test]
fn a_seeded_secret_warns_with_categories_and_counts_and_no_text() {
    let fixture = Fixture::new("secret-warning");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    let content = format!("token = {FAKE_AWS_KEY}\n");
    fixture.write("a.txt", &content);
    fixture.apply_with(&session, &task, &[("a.txt", &content)], None, None, true);
    let preparation = fixture.prepare(&session);
    let candidate = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "a.txt")
        .unwrap();
    let warning = candidate
        .secret_warnings
        .first()
        .expect("the seeded key is flagged");
    assert!(warning.categories.contains(&"aws_access_key".to_string()));
    assert_eq!(warning.count, 1);
    assert_eq!(warning.path, "a.txt");

    let serialized = serde_json::to_string(&preparation).unwrap();
    assert!(!serialized.contains(FAKE_AWS_KEY), "{serialized}");
    assert!(serialized.contains("aws_access_key"));
    fixture.cleanup();
}

#[test]
fn a_secret_needs_a_separate_per_commit_override() {
    let fixture = Fixture::new("secret-override");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    let content = format!("token = {FAKE_AWS_KEY}\n");
    fixture.write("a.txt", &content);
    fixture.apply_with(&session, &task, &[("a.txt", &content)], None, None, true);
    let preparation = fixture.prepare(&session);
    let selected = fixture.default_paths(&preparation);
    let before = fixture.head();

    let first = fixture.commit(&session, &preparation, &selected, "Commit a", false);
    assert!(matches!(
        first,
        Err(CommitError::Refused {
            code: "commit_secrets_require_override",
            ..
        })
    ));
    assert_eq!(fixture.head(), before);

    let outcome = fixture
        .commit(&session, &preparation, &selected, "Commit a", true)
        .expect("the override commits");
    assert!(outcome.generated_secret_override);
    assert_ne!(fixture.head(), before);

    let audit = fs::read_to_string(fixture.data_dir.join("audit/events.jsonl")).unwrap();
    assert!(audit.contains("commit_secret_override"), "{audit}");
    assert!(audit.contains("aws_access_key"), "{audit}");
    assert!(audit.contains("generatedSecretOverride"));
    assert!(!audit.contains(FAKE_AWS_KEY), "the audit leaked the secret");
    fixture.cleanup();
}

#[test]
fn an_unscannable_file_is_treated_as_a_secret_warning() {
    let mut fixture = Fixture::new("unscanned");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    let content = "this file is far too long to scan\n";
    fixture.write("big.txt", content);
    fixture.apply(&session, &task, &[("big.txt", content)], None, None);
    fixture.rebuild(|config| config.max_file_bytes = 8);
    let preparation = fixture.prepare(&session);
    let candidate = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "big.txt")
        .unwrap();
    assert!(candidate.unscanned);
    assert!(
        candidate
            .secret_warnings
            .iter()
            .any(|warning| warning.categories == vec!["unscanned".to_string()])
    );
    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit big",
        false,
    );
    assert!(
        matches!(
            outcome,
            Err(CommitError::Refused {
                code: "commit_secrets_require_override",
                ..
            })
        ),
        "{outcome:?}"
    );
    fixture.cleanup();
}

#[test]
fn the_recorded_diff_is_redacted_and_the_record_holds_no_secret() {
    let fixture = Fixture::new("record-redacted");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    let content = format!("token = {FAKE_AWS_KEY}\n");
    fixture.write("a.txt", &content);
    fixture.apply_with(&session, &task, &[("a.txt", &content)], None, None, true);
    let preparation = fixture.prepare(&session);
    let candidate = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "a.txt")
        .unwrap();
    assert!(candidate.diff.contains("[REDACTED"), "{}", candidate.diff);
    let record = fs::read_to_string(
        fixture
            .data_dir
            .join("commits")
            .join(format!("{}.json", preparation.prepare_id)),
    )
    .unwrap();
    assert!(
        !record.contains(FAKE_AWS_KEY),
        "the record leaked the secret"
    );
    fixture.cleanup();
}

#[test]
fn a_commit_is_bracketed_by_a_side_effecting_marker() {
    let fixture = Fixture::new("marker");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    let preparation = fixture.prepare(&session);
    let outcome = fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit a",
            false,
        )
        .unwrap();
    let log = fs::read_to_string(
        fixture
            .data_dir
            .join("sessions")
            .join(format!("{}.jsonl", session.id)),
    )
    .unwrap();
    assert!(
        log.contains(r#""action":"commit""#) && log.contains(r#""sideEffecting":true"#),
        "{log}"
    );
    assert!(log.contains(r#""action_finished""#), "{log}");
    assert!(log.contains(r#""outcome":"ok""#), "{log}");
    assert!(log.contains(r#""commit_created""#), "{log}");
    assert!(log.contains(&format!(r#""commitSha":"{}""#, outcome.commit_sha)));
    assert!(log.contains(r#""paths":["a.txt"]"#), "{log}");
    assert!(
        log.contains(r#""status":"complete""#),
        "the synthetic task is completed: {log}"
    );
    let record = fixture
        .engine
        .commit
        .load_record(&preparation.prepare_id)
        .unwrap();
    assert_eq!(record.task_id, Some(record_task_id(&log)));
    assert_eq!(record.selected_paths, vec!["a.txt".to_string()]);
    assert_eq!(
        record.prepared_commit.as_deref(),
        Some(outcome.commit_sha.as_str())
    );
    assert_eq!(
        record.head_after.as_deref(),
        Some(outcome.commit_sha.as_str())
    );
    fixture.cleanup();
}

fn record_task_id(log: &str) -> String {
    // The marker's task id is the first `taskId` beside `"action":"commit"`.
    let start = log.find(r#""action":"commit""#).expect("a commit marker");
    let prefix = &log[..start];
    let at = prefix.rfind(r#""taskId":""#).expect("a task id");
    let rest = &prefix[at + r#""taskId":""#.len()..];
    rest.split('"').next().unwrap().to_string()
}

#[test]
fn a_crash_between_the_markers_is_an_unknown_outcome() {
    let fixture = Fixture::new("crash");
    let (session, task) = fixture.session();
    let task_id = task.id.clone();
    let task = fixture
        .engine
        .session_store
        .update_task_status(&task, workspace_engine::TaskStatus::RunningTool, None)
        .unwrap();
    let _marker = fixture
        .engine
        .session_store
        .start_action(&task, "commit", "commit_crashed", true)
        .unwrap();
    let recovered = workspace_engine::classify_session(
        &fixture.engine.session_store,
        &fixture.engine.audit_log,
        &session.id,
    )
    .unwrap();
    let commit = recovered
        .iter()
        .find(|recovered| recovered.task_id == task_id)
        .expect("the commit task is recovered");
    assert_eq!(
        commit.classification,
        workspace_engine::TaskStatus::UnknownExternalOutcome
    );
    assert!(!workspace_engine::resume_allowed(commit));
    fixture.cleanup();
}

#[test]
fn the_commit_is_audited_with_counts_and_no_secret_text() {
    let fixture = Fixture::new("audit");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    let content = format!("token = {FAKE_AWS_KEY}\n");
    fixture.write("a.txt", &content);
    fixture.apply_with(&session, &task, &[("a.txt", &content)], None, None, true);
    let preparation = fixture.prepare(&session);
    let outcome = fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            &format!("Commit a\n\n{FAKE_AWS_KEY}"),
            true,
        )
        .unwrap();
    assert_eq!(
        fixture.git(&["log", "-1", "--pretty=%B"]),
        format!("Commit a\n\n{FAKE_AWS_KEY}"),
        "the user's edited message is what was committed"
    );
    let audit = fs::read_to_string(fixture.data_dir.join("audit/events.jsonl")).unwrap();
    assert!(audit.contains(r#""eventType":"commit_created""#), "{audit}");
    assert!(audit.contains(r#""pathCount":"1""#), "{audit}");
    assert!(audit.contains(r#""messageLength":"#), "{audit}");
    assert!(audit.contains(&format!(r#""commitSha":"{}""#, outcome.commit_sha)));
    assert!(
        audit.contains(r#""generatedSecretOverride":"true""#),
        "{audit}"
    );
    assert!(audit.contains(r#""hooks":""#), "{audit}");
    assert!(
        !audit.contains(FAKE_AWS_KEY),
        "the audit leaked the message's key"
    );
    fixture.cleanup();
}

#[test]
fn a_restricted_path_cannot_be_committed_even_if_the_record_names_it() {
    let fixture = Fixture::new("restricted");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    let preparation = fixture.prepare(&session);
    let record_path = fixture
        .data_dir
        .join("commits")
        .join(format!("{}.json", preparation.prepare_id));
    let mut record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    let candidates = record["candidates"].as_array_mut().unwrap();
    let mut forged = candidates[0].clone();
    forged["path"] = serde_json::Value::String(".env".to_string());
    forged["selectedByDefault"] = serde_json::Value::Bool(false);
    candidates.push(forged);
    fs::write(&record_path, serde_json::to_string_pretty(&record).unwrap()).unwrap();
    let outcome = fixture.commit(
        &session,
        &preparation,
        &[".env".to_string()],
        "Commit the env file",
        false,
    );
    assert!(
        matches!(
            outcome,
            Err(CommitError::Refused {
                code: "commit_restricted_path",
                ..
            })
        ),
        "{outcome:?}"
    );
    fixture.cleanup();
}

#[test]
fn the_commit_module_has_no_history_rewriting_operation() {
    let source = include_str!("../src/commit.rs");
    for forbidden in [
        "\"amend\"",
        "\"reset\"",
        "\"rebase\"",
        "\"cherry-pick\"",
        "\"filter-branch\"",
        "\"commit --amend\"",
    ] {
        assert!(!source.contains(forbidden), "commit.rs reaches {forbidden}");
    }
}

#[test]
fn an_accepted_deletion_is_selected_and_commits_the_file_away() {
    let fixture = Fixture::new("delete");
    fixture.write("a.txt", "original\n");
    fixture.write("keep.txt", "keep\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.apply_deleted(&session, &task, "a.txt");
    let preparation = fixture.prepare(&session);
    let candidate = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "a.txt")
        .expect("the deletion is a candidate");
    assert_eq!(candidate.status, "deleted");
    assert_eq!(candidate.source, CommitCandidateSource::AcceptedPatch);
    assert!(candidate.selected_by_default);
    fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Remove a",
            false,
        )
        .unwrap();
    assert!(!fixture.tree_paths().contains(&"a.txt".to_string()));
    assert!(fixture.tree_paths().contains(&"keep.txt".to_string()));
    fixture.cleanup();
}

#[test]
fn a_nested_repository_is_shown_and_can_never_be_selected() {
    let fixture = Fixture::new("nested");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let nested = fixture.repository.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fixture.git_at(&nested, &["init", "--quiet"]);
    fs::write(nested.join("inner.txt"), "inner\n").unwrap();
    let (session, _task) = fixture.session();
    let preparation = fixture.prepare(&session);
    let candidate = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path.starts_with("nested"))
        .expect("the nested repository is a candidate");
    assert_eq!(candidate.source, CommitCandidateSource::Unsupported);
    assert!(!candidate.selected_by_default);
    let outcome = fixture.commit(
        &session,
        &preparation,
        std::slice::from_ref(&candidate.path),
        "Commit the nested repo",
        false,
    );
    assert!(
        matches!(
            outcome,
            Err(CommitError::Refused {
                code: "commit_not_supported",
                ..
            })
        ),
        "{outcome:?}"
    );
    fixture.cleanup();
}

#[test]
fn a_same_size_hook_edit_after_the_report_is_refused() {
    let fixture = Fixture::new("hook-edited");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.write_hook("pre-commit", "#!/bin/sh\nexit 0\n", true);
    let preparation = fixture.prepare(&session);
    // Same length, different behaviour: a size-only report would miss it.
    fixture.write_hook("pre-commit", "#!/bin/sh\nexit 1\n", true);
    let before = fixture.head();
    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    assert!(
        matches!(
            outcome,
            Err(CommitError::Refused {
                code: "commit_hooks_changed",
                ..
            })
        ),
        "{outcome:?}"
    );
    assert_eq!(fixture.head(), before);
    fixture.cleanup();
}

#[test]
fn a_staged_rename_is_prepared_as_a_pair_and_committed_as_one() {
    let fixture = Fixture::new("rename");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    fixture.git(&["mv", "a.txt", "b.txt"]);
    let (session, _task) = fixture.session();

    let preparation = fixture.prepare(&session);
    let target = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "b.txt")
        .expect("the rename target is a candidate");
    assert_eq!(target.renamed_from.as_deref(), Some("a.txt"));
    assert_eq!(target.copied_from, None);
    let source = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "a.txt")
        .expect("the rename source is a deletion candidate");
    assert_eq!(source.status, "deleted");
    assert_eq!(source.renamed_to.as_deref(), Some("b.txt"));
    assert!(!source.selected_by_default);

    let before = fixture.head();
    let half = fixture.commit(
        &session,
        &preparation,
        &["b.txt".to_string()],
        "Rename a to b",
        false,
    );
    assert!(
        matches!(
            half,
            Err(CommitError::Refused {
                code: "commit_rename_incomplete",
                ..
            })
        ),
        "{half:?}"
    );
    assert_eq!(fixture.head(), before);

    fixture
        .commit(
            &session,
            &preparation,
            &["a.txt".to_string(), "b.txt".to_string()],
            "Rename a to b",
            false,
        )
        .unwrap();
    assert_eq!(fixture.tree_paths(), vec!["b.txt".to_string()]);
    assert_eq!(fixture.show("b.txt"), "original\n");
    fixture.cleanup();
}

#[test]
fn a_hook_that_stages_a_fix_keeps_it_out_of_the_commit_and_says_so() {
    let fixture = Fixture::new("hook-stages");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    fixture.write_hook(
        "pre-commit",
        "#!/bin/sh\nprintf 'hook edit\\n' >> a.txt\ngit add a.txt\nexit 0\n",
        true,
    );
    let preparation = fixture.prepare(&session);
    let outcome = fixture
        .commit(
            &session,
            &preparation,
            &fixture.default_paths(&preparation),
            "Commit a",
            false,
        )
        .unwrap();
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("pre-commit")
                && warning.contains("remain uncommitted")),
        "{:?}",
        outcome.warnings
    );
    assert_eq!(fixture.show("a.txt"), "damaian\n");
    assert_eq!(fixture.read("a.txt"), "damaian\nhook edit\n");
    fixture.cleanup();
}

#[test]
fn a_chmod_only_change_is_a_candidate_and_commits_the_mode() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("chmod");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let full = fixture.repository.join("a.txt");
    let mut permissions = fs::metadata(&full).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&full, permissions).unwrap();
    let (session, _task) = fixture.session();
    let preparation = fixture.prepare(&session);
    let candidate = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "a.txt")
        .expect("a chmod-only change is a candidate");
    assert_eq!(candidate.mode.as_deref(), Some("100755"));
    fixture
        .commit(
            &session,
            &preparation,
            &["a.txt".to_string()],
            "Make a executable",
            false,
        )
        .unwrap();
    let entry = fixture.git(&["ls-tree", "HEAD", "a.txt"]);
    assert!(entry.starts_with("100755 blob "), "{entry}");
    assert_eq!(fixture.show("a.txt"), "original\n");
    fixture.cleanup();
}

#[test]
fn replacing_a_file_with_a_symlink_commits_mode_120000() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new("symlink");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    fs::remove_file(fixture.repository.join("a.txt")).unwrap();
    symlink("target", fixture.repository.join("a.txt")).unwrap();
    let (session, _task) = fixture.session();
    let preparation = fixture.prepare(&session);
    let candidate = preparation
        .candidates
        .iter()
        .find(|candidate| candidate.path == "a.txt")
        .expect("the type change is a candidate");
    assert_eq!(candidate.mode.as_deref(), Some("120000"));
    fixture
        .commit(
            &session,
            &preparation,
            &["a.txt".to_string()],
            "Replace a with a link",
            false,
        )
        .unwrap();
    let entry = fixture.git(&["ls-tree", "HEAD", "a.txt"]);
    assert!(entry.starts_with("120000 blob "), "{entry}");
    assert_eq!(fixture.show("a.txt"), "target");
    fixture.cleanup();
}

#[test]
fn a_chmod_after_preparation_is_refused_as_content_changed() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("chmod-after");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);
    let preparation = fixture.prepare(&session);
    let full = fixture.repository.join("a.txt");
    let mut permissions = fs::metadata(&full).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&full, permissions).unwrap();
    let before = fixture.head();
    let outcome = fixture.commit(
        &session,
        &preparation,
        &fixture.default_paths(&preparation),
        "Commit a",
        false,
    );
    assert!(
        matches!(
            outcome,
            Err(CommitError::Refused {
                code: "commit_content_changed",
                ..
            })
        ),
        "{outcome:?}"
    );
    assert_eq!(fixture.head(), before);
    fixture.cleanup();
}
