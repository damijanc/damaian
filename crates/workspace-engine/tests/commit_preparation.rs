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
                false,
            )
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
fn the_users_index_and_working_tree_are_unchanged_by_a_commit() {
    let fixture = Fixture::new("index");
    fixture.write("a.txt", "original\n");
    fixture.commit_all("initial");
    fixture.write("staged.txt", "the user staged this\n");
    fixture.git(&["add", "staged.txt"]);
    let index_before = fixture.index_bytes();
    let staged_before = fs::read(fixture.repository.join("staged.txt")).unwrap();
    let (session, task) = fixture.session();
    fixture.write("a.txt", "damaian\n");
    fixture.apply(&session, &task, &[("a.txt", "damaian\n")], None, None);

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

    assert_eq!(
        fixture.index_bytes(),
        index_before,
        "the index must not change"
    );
    assert_eq!(
        fs::read(fixture.repository.join("staged.txt")).unwrap(),
        staged_before,
        "the working tree must not change"
    );
    assert!(
        fixture
            .git(&["diff", "--cached", "--name-only"])
            .contains("staged.txt")
    );
    assert!(!fixture.tree_paths().contains(&"staged.txt".to_string()));
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
