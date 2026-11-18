use super::*;
use crate::remember_model_api_key;
use crate::workers::{
    Refusal, SpawnRequest, SpawnedWorker, StopNext, StopOutcome, StopTarget, Supervisor,
    WorkerRequest, WorkerTimeouts, begin_stop, finish_stop, route,
    workers_to_stop_for_deleted_session,
};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use workspace_engine::worker_progress::{ProgressKind, ProgressWriter};
use workspace_engine::{ProcessRegistry, SessionStore};

/// Plain on purpose: the secret scanner leaves it alone, so a leak
/// would show (Task 6 found that an `sk-…` key is redacted on its way
/// to disk, which would hide one). Checked in `Fixture::new`.
const KEY: &str = "plain-worker-handover-key";
static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Records argv, the environment, and the handover line's length and
/// shape with the key masked, so the key itself never reaches disk.
/// Then it holds stdin open as a worker does, and ends at EOF.
const STUB: &str = r#"#!/bin/sh
d=$(dirname "$0")
printf '%s\n' "$@" > "$d/argv.out"
/usr/bin/env -0 > "$d/env.out"
IFS= read -r line
printf '%s' "${#line}" > "$d/len.out"
printf '%s' "$line" | /usr/bin/sed -E 's/"apiKey":"[^"]*"/"apiKey":"<masked>"/' > "$d/handover.out"
: > "$d/ready.out"
exec /bin/cat > /dev/null
"#;

/// Short and not canonicalized: a worker's socket path must fit in
/// 104 bytes, and `/private` would add 8.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dws-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.test"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// A committed one-file repository, canonical as the spawn records it.
fn repository(root: &Path, name: &str) -> PathBuf {
    let repo = root.join(name);
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("README.md"), "# Fixture\n").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init"]);
    fs::canonicalize(&repo).unwrap()
}

fn timeouts() -> WorkerTimeouts {
    WorkerTimeouts {
        ready: Duration::from_secs(10),
        exchange: Duration::from_secs(2),
        stop_to_shutdown: Duration::from_millis(300),
        // Long enough that a stub always ends by its lifeline first:
        // nothing in this module may signal a process.
        shutdown_to_sigterm: Duration::from_secs(10),
        poll: Duration::from_millis(20),
    }
}

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    config: Config,
    supervisor: Supervisor,
}

impl Fixture {
    fn new() -> Self {
        let root = scratch();
        let repo = repository(&root, "r");
        let stub_dir = root.join("stub");
        fs::create_dir_all(&stub_dir).unwrap();
        let stub = stub_dir.join("worker");
        fs::write(&stub, STUB).unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        // The key comes from `resolve_model_api_key`'s in-memory cache,
        // as in the app: an account of this test's own, so no Keychain
        // is ever asked.
        let account = format!(
            "damaian-worker-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        remember_model_api_key(&account, KEY);
        let config = Config {
            data_dir: root.join("d"),
            enable_worker_sessions: true,
            enable_index_watcher: false,
            model_api_key_env: format!("keychain:{account}"),
            ..Config::default()
        };
        let scanner = SecretScanner::new(config.secret_patterns.clone());
        assert_eq!(
            scanner.redact(KEY).text,
            KEY,
            "the test key must survive redaction, or a leak would be hidden"
        );
        Self {
            supervisor: Supervisor::new(stub, timeouts()),
            root,
            repo,
            config,
        }
    }

    fn store(&self) -> SessionStore {
        SessionStore::new(&self.config.data_dir)
    }

    fn spawn_from(&self, repo: &Path, prompt: &str, parent: Option<&str>) -> SpawnedWorker {
        self.supervisor
            .spawn(
                &self.config,
                SpawnRequest {
                    repo: repo.to_path_buf(),
                    prompt: prompt.to_string(),
                    include_uncommitted: false,
                    parent_session_id: parent.map(str::to_string),
                    parent_task_id: None,
                    handed_plan: None,
                },
                KEY,
            )
            .expect("the worker should spawn")
    }

    fn spawn(&self, prompt: &str, parent: Option<&str>) -> SpawnedWorker {
        self.spawn_from(&self.repo, prompt, parent)
    }

    fn parent(&self, title: &str) -> String {
        self.store()
            .create_session(
                &workspace_engine::hash::repository_id_for_root(&self.repo),
                title,
            )
            .unwrap()
            .id
    }

    /// One worker route, with `config` (on or off) and form fields
    /// encoded the way the browser sends them.
    fn route_with(
        &self,
        config: &Config,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        form: &[(&str, &str)],
    ) -> Result<Value, Refusal> {
        let query: HashMap<String, String> = query
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let form = parse_form(&form_body_for_test(form));
        route(
            &self.supervisor,
            config,
            &|config: &Config| resolve_shell_model_api_key(config),
            &WorkerRequest {
                method,
                path,
                query: &query,
                form: &form,
            },
        )
        .map(|body| serde_json::from_str(&body).expect("a JSON answer"))
    }

    fn route(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        form: &[(&str, &str)],
    ) -> Result<Value, Refusal> {
        self.route_with(&self.config, method, path, query, form)
    }

    fn wait_for_exit(&self, session: &str) {
        wait_until("the worker to exit", || !self.supervisor.is_alive(session));
    }
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The stub's record of its start, once it has written all of it.
fn stub_output(fixture: &Fixture, name: &str) -> String {
    let dir = fixture.root.join("stub");
    wait_until("the stub to record its start", || {
        dir.join("ready.out").exists()
    });
    fs::read_to_string(dir.join(name)).unwrap()
}

/// Serves the worker's socket in the test, recording every request and
/// answering with `answer`. Stands in for the worker's protocol.
fn fake_socket(
    data_dir: &Path,
    session: &str,
    answer: fn(&Value) -> Value,
) -> Arc<Mutex<Vec<Value>>> {
    let path = crate::worker_socket_path(data_dir, session).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let Ok(mut writer) = stream.try_clone() else {
                return;
            };
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let request: Value = serde_json::from_str(&line).unwrap();
                let reply = answer(&request);
                seen.lock().unwrap().push(request);
                if writeln!(writer, "{reply}").is_err() {
                    break;
                }
            }
        }
    });
    requests
}

fn echo(request: &Value) -> Value {
    match request["command"].as_str() {
        Some("status") => json!({"ok": true, "state": "idle"}),
        command => json!({"ok": true, "echo": command}),
    }
}

fn audit_events(data_dir: &Path, event_type: &str) -> Vec<Value> {
    fs::read_to_string(data_dir.join("audit").join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["eventType"] == event_type)
        .collect()
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            files.extend(files_under(&path));
        } else if meta.is_file() {
            files.push(path);
        }
    }
    files
}

/// §5.1 steps 7–8: one `worker_spawned` on the worker's log, one on the
/// parent's, one in the audit log, and the worker's and the audit's
/// carry the exact git commands that created the worktree.
#[test]
fn spawn_records_worker_spawned_on_worker_parent_and_audit() {
    let fixture = Fixture::new();
    let parent = fixture.parent("Planner");
    let spawned = fixture.spawn("\n  Fix the parser\n\nRead src/a.rs first.", Some(&parent));

    assert_eq!(spawned.title, "Fix the parser");
    assert_eq!(spawned.branch, "damaian/fix-the-parser");
    assert_eq!(
        spawned.worktree,
        fixture.repo.join(".damaian/worktrees/fix-the-parser")
    );
    let store = fixture.store();
    let record = store.worker_record(&spawned.session_id).unwrap().unwrap();
    assert_eq!(record.parent_session_id.as_deref(), Some(parent.as_str()));
    assert_eq!(record.source_repository, fixture.repo);
    assert_eq!(record.worktree_path, spawned.worktree);
    assert_eq!(record.branch, spawned.branch);
    assert!(!record.included_uncommitted);
    assert!(
        record
            .git_commands
            .iter()
            .any(|command| command.contains("worktree add -b damaian/fix-the-parser")),
        "{:?}",
        record.git_commands
    );
    assert_eq!(
        store.worker_children(&parent).unwrap(),
        vec![spawned.session_id.clone()]
    );
    let session = store.read_session(&spawned.session_id).unwrap().unwrap();
    assert_eq!(session.title, "Fix the parser");
    assert_eq!(
        session.repository_id,
        workspace_engine::hash::repository_id_for_root(&spawned.worktree),
        "the worker session belongs to its worktree"
    );
    assert_eq!(store.session_mode(&spawned.session_id), SessionMode::Code);

    let audited = audit_events(&fixture.config.data_dir, "worker_spawned");
    assert_eq!(audited.len(), 1, "{audited:?}");
    let audited = &audited[0];
    assert_eq!(audited["sessionId"], spawned.session_id);
    assert_eq!(audited["parentSessionId"], parent);
    assert_eq!(audited["branch"], spawned.branch);
    assert_eq!(audited["baseCommit"], record.base_commit);
    assert_eq!(audited["includedUncommitted"], "false");
    let commands: Vec<String> =
        serde_json::from_str(audited["gitCommands"].as_str().unwrap()).unwrap();
    assert_eq!(
        commands, record.git_commands,
        "the exact commands, in order"
    );

    fixture.supervisor.shutdown_all();
}

/// Spec 98: a worker snapshots its parent task's durable protected
/// paths at spawn, and the prompt — which may be model-generated — is
/// never parsed for them. A sidebar worker has no parent task and gets
/// nothing, and a task from a different session is refused.
#[test]
fn a_parent_task_snapshot_crosses_the_handoff_and_prompt_text_cannot_broaden_it() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let parent = fixture.parent("Parent");
    let task = store
        .create_task(&parent, "Change the docs.", "provider", "model")
        .unwrap();
    let engine = workspace_engine::WorkspaceEngine::new(fixture.config.clone());
    engine
        .add_task_protected_paths(&fixture.repo, &parent, &task.id, &["README.md".to_string()])
        .unwrap();

    let spawned = fixture
        .supervisor
        .spawn(
            &fixture.config,
            SpawnRequest {
                repo: fixture.repo.clone(),
                prompt: "Do not touch `src/other.rs`. Do the work.".to_string(),
                include_uncommitted: false,
                parent_session_id: Some(parent.clone()),
                parent_task_id: Some(task.id.clone()),
                handed_plan: None,
            },
            KEY,
        )
        .expect("the worker should spawn");
    let record = store.worker_record(&spawned.session_id).unwrap().unwrap();
    assert_eq!(record.parent_task_id.as_deref(), Some(task.id.as_str()));
    assert_eq!(
        record.inherited_protected_paths,
        vec![workspace_engine::ProtectedPath {
            path: "README.md".to_string(),
            kind: workspace_engine::ProtectedPathKind::File,
        }],
        "only the parent's resolved record is copied"
    );
    assert!(
        !record
            .inherited_protected_paths
            .iter()
            .any(|path| path.path == "src/other.rs"),
        "prompt text must not add a protection"
    );

    let manual = fixture.spawn("A sidebar worker", None);
    assert!(
        store
            .worker_record(&manual.session_id)
            .unwrap()
            .unwrap()
            .inherited_protected_paths
            .is_empty(),
        "a worker with no parent task inherits nothing"
    );

    let other_parent = fixture.parent("Other");
    let refused = fixture
        .supervisor
        .spawn(
            &fixture.config,
            SpawnRequest {
                repo: fixture.repo.clone(),
                prompt: "Do the work.".to_string(),
                include_uncommitted: false,
                parent_session_id: Some(other_parent),
                parent_task_id: Some(task.id),
                handed_plan: None,
            },
            KEY,
        )
        .unwrap_err();
    assert!(refused.contains("Unknown task"), "{refused}");

    fixture.supervisor.shutdown_all();
}

/// Spec 108 D1: with the manual control gone, a worker's card lists the
/// protected paths it was handed, read-only, and a worker handed none
/// lists none. The UI renders these from the board row.
#[test]
fn a_workers_card_lists_the_protected_paths_it_was_handed() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let parent = fixture.parent("Parent");
    let task = store
        .create_task(&parent, "Change the docs.", "provider", "model")
        .unwrap();
    fs::create_dir_all(fixture.repo.join("docs")).unwrap();
    fs::write(fixture.repo.join("docs/guide.md"), "guide\n").unwrap();
    let engine = workspace_engine::WorkspaceEngine::new(fixture.config.clone());
    engine
        .add_task_protected_paths(
            &fixture.repo,
            &parent,
            &task.id,
            &["README.md".to_string(), "docs".to_string()],
        )
        .unwrap();
    let handed = fixture
        .supervisor
        .spawn(
            &fixture.config,
            SpawnRequest {
                repo: fixture.repo.clone(),
                prompt: "Do the work.".to_string(),
                include_uncommitted: false,
                parent_session_id: Some(parent.clone()),
                parent_task_id: Some(task.id.clone()),
                handed_plan: None,
            },
            KEY,
        )
        .expect("the worker should spawn");
    let manual = fixture.spawn("A sidebar worker", None);

    let rows = fixture
        .route(
            "GET",
            "/api/workers",
            &[("repo", fixture.repo.to_str().unwrap())],
            &[],
        )
        .unwrap()["workers"]
        .as_array()
        .unwrap()
        .clone();
    let row_for = |session: &str| {
        rows.iter()
            .find(|row| row["sessionId"] == session)
            .unwrap()
            .clone()
    };
    assert_eq!(
        row_for(&handed.session_id)["protectedPaths"],
        json!([
            {"path": "README.md", "kind": "file"},
            {"path": "docs", "kind": "directory"},
        ])
    );
    assert_eq!(row_for(&manual.session_id)["protectedPaths"], json!([]));

    let app = include_str!("../../static/app.js");
    let card = app
        .split("function renderWorkerBoardRow")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction ").next())
        .expect("the worker board row renderer");
    assert!(card.contains("workerProtectedPathsText(row)"), "{card}");
    let header = app
        .split("function renderWorkerHeaderMeta")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction ").next())
        .expect("the worker header renderer");
    assert!(header.contains("workerProtectedPathsText(row)"), "{header}");

    fixture.supervisor.shutdown_all();
}

/// Spec 108 D3: a worker row says whether its session is archived, so
/// the board can hide it by default and the sidebar can list it under
/// Archived.
#[test]
fn a_worker_row_carries_its_archived_flag() {
    let fixture = Fixture::new();
    let archived = fixture.spawn("An archived worker", None);
    let kept = fixture.spawn("A kept worker", None);
    fixture
        .store()
        .archive_session(&archived.session_id, false)
        .expect("an archive with no running worker");

    let rows = fixture
        .route(
            "GET",
            "/api/workers",
            &[("repo", fixture.repo.to_str().unwrap())],
            &[],
        )
        .unwrap()["workers"]
        .as_array()
        .unwrap()
        .clone();
    let archived_flag = |session: &str| {
        rows.iter()
            .find(|row| row["sessionId"] == session)
            .map(|row| row["archived"].clone())
            .unwrap()
    };
    assert_eq!(archived_flag(&archived.session_id), json!(true));
    assert_eq!(archived_flag(&kept.session_id), json!(false));

    fixture.supervisor.shutdown_all();
}

/// Spec 108 D7: Run in a worker after Approve names the plan review it
/// approved. The spawn records the engine's snapshot of that review's
/// steps in the worker's log before the worker starts; a review of
/// another task is refused before any worker state exists.
#[test]
fn a_worker_started_after_approve_carries_the_approved_plan() {
    let fixture = Fixture::new();
    let parent = fixture.parent("Parent");
    let engine = workspace_engine::WorkspaceEngine::new(fixture.config.clone());
    let call = |name: &str, arguments: &str| workspace_engine::ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments.to_string(),
    };
    let mut adapter = workspace_engine::MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new()],
        vec![
            vec![call(
                "propose_plan",
                r#"{"steps":[{"title":"Read the README"},{"title":"Fix the typo"}]}"#,
            )],
            vec![call(
                "propose_patch",
                r#"{"summary":"Fix","files":[{"path":"README.md","content":"fixed\n"}]}"#,
            )],
        ],
    );
    let cancel = workspace_engine::CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: workspace_engine::TurnProgress| {};
    let mut sink = super::turn_sink(&cancel, &mut on_token, &mut on_progress);
    let paused = engine
        .chat_orchestrator
        .ask_with_session(
            &fixture.repo,
            "Fix the typo in the README",
            &[],
            Some(&parent),
            &mut adapter,
            &mut sink,
        )
        .expect("the turn pauses on its plan");
    let proposal = paused.plan_proposal.expect("a plan review");
    let repo = fixture.repo.to_str().unwrap();

    let refused = fixture.route(
        "POST",
        "/api/workers",
        &[],
        &[
            ("repo", repo),
            ("prompt", "Fix the typo"),
            ("parent_session_id", &parent),
            ("parent_task_id", "task_other"),
            ("plan_proposal_id", &proposal.id),
        ],
    );
    assert_eq!(refused.err().map(|refusal| refusal.status), Some(400));
    assert!(
        fixture.store().worker_children(&parent).unwrap().is_empty(),
        "refused before any worker state"
    );

    let started = fixture
        .route(
            "POST",
            "/api/workers",
            &[],
            &[
                ("repo", repo),
                ("prompt", "Fix the typo"),
                ("parent_session_id", &parent),
                ("parent_task_id", &paused.task.id),
                ("plan_proposal_id", &proposal.id),
            ],
        )
        .expect("the worker starts");
    let worker = started["sessionId"].as_str().unwrap();
    let handed = fixture
        .store()
        .handed_plan(worker)
        .unwrap()
        .expect("the approved plan is on the worker's log");
    assert_eq!(handed.steps, ["Read the README", "Fix the typo"]);
    assert_eq!(handed.proposal_id, proposal.id);
    assert_eq!(handed.approved_by, "desktop_user");

    fixture.supervisor.shutdown_all();
}

/// A crafted handoff cannot move repository-relative protections from
/// one repository into a worker for another. The check runs before the
/// worktree, branch, session and audit records exist, and applies even
/// when the caller supplies no parent task.
#[test]
fn a_parent_session_from_another_repository_is_refused_before_any_worker_state() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let other_repo = repository(&fixture.root, "o");
    let parent = fixture.parent("Parent");
    let task = store
        .create_task(&parent, "Change the docs.", "provider", "model")
        .unwrap();
    let engine = workspace_engine::WorkspaceEngine::new(fixture.config.clone());
    engine
        .add_task_protected_paths(&fixture.repo, &parent, &task.id, &["README.md".to_string()])
        .unwrap();

    let sessions_before = store.list_sessions(None).unwrap().len();
    let spawn_cross_repo = |parent_task_id: Option<String>| {
        fixture
            .supervisor
            .spawn(
                &fixture.config,
                SpawnRequest {
                    repo: other_repo.clone(),
                    prompt: "Do the work.".to_string(),
                    include_uncommitted: false,
                    parent_session_id: Some(parent.clone()),
                    parent_task_id,
                    handed_plan: None,
                },
                KEY,
            )
            .unwrap_err()
    };
    for parent_task_id in [Some(task.id.clone()), None] {
        let refused = spawn_cross_repo(parent_task_id);
        assert!(
            refused.contains("different repository"),
            "a cross-repository handoff must be refused: {refused}"
        );
    }

    assert_eq!(
        store.list_sessions(None).unwrap().len(),
        sessions_before,
        "no worker session may be created"
    );
    assert!(
        store.worker_children(&parent).unwrap().is_empty(),
        "no child may be recorded"
    );
    assert!(
        !other_repo.join(".damaian").exists(),
        "no worktree or branch metadata may be created"
    );
    assert!(
        audit_events(&fixture.config.data_dir, "worker_spawned").is_empty()
            && audit_events(&fixture.config.data_dir, "worker_spawn_failed").is_empty(),
        "the refusal must precede every audit side effect"
    );

    // The same handoff within the source repository still works.
    let spawned = fixture
        .supervisor
        .spawn(
            &fixture.config,
            SpawnRequest {
                repo: fixture.repo.clone(),
                prompt: "Do the work.".to_string(),
                include_uncommitted: false,
                parent_session_id: Some(parent.clone()),
                parent_task_id: Some(task.id.clone()),
                handed_plan: None,
            },
            KEY,
        )
        .expect("the same-repository worker should spawn");
    let record = store.worker_record(&spawned.session_id).unwrap().unwrap();
    assert_eq!(record.parent_task_id.as_deref(), Some(task.id.as_str()));
    assert_eq!(
        record.inherited_protected_paths,
        vec![workspace_engine::ProtectedPath {
            path: "README.md".to_string(),
            kind: workspace_engine::ProtectedPathKind::File,
        }]
    );

    fixture.supervisor.shutdown_all();
}

/// A data directory whose worker socket path cannot fit `sun_path` is
/// refused before anything is created: no worktree, no branch, no
/// session. Before this check it was refused only when the process
/// started, after all three existed (Task 9 discovery 3).
#[test]
fn a_data_dir_too_long_for_the_socket_is_refused_before_the_worktree() {
    let mut fixture = Fixture::new();
    fixture.config.data_dir = fixture.root.join("d".repeat(120));
    let refused = fixture.supervisor.spawn(
        &fixture.config,
        SpawnRequest {
            repo: fixture.repo.clone(),
            prompt: "Fix the parser".to_string(),
            include_uncommitted: false,
            parent_session_id: None,
            parent_task_id: None,
            handed_plan: None,
        },
        KEY,
    );
    let error = refused.expect_err("the spawn must be refused");
    assert!(error.contains("socket path"), "{error}");
    assert!(
        !fixture.repo.join(".damaian").exists(),
        "a worktree was created before the refusal"
    );
    let branches = Command::new("git")
        .args(["branch", "--list", "damaian/*"])
        .current_dir(&fixture.repo)
        .output()
        .unwrap();
    assert!(
        branches.stdout.is_empty(),
        "a branch was created before the refusal: {}",
        String::from_utf8_lossy(&branches.stdout)
    );
    assert!(
        !fixture.config.data_dir.join("sessions").exists(),
        "a session was created before the refusal"
    );
}

/// Requirement 9: the key reaches the worker on stdin only, from
/// `resolve_model_api_key`'s cache. Its argv carries neither key nor
/// token, its environment is the shell's own, and the key is in no
/// file the spawn wrote and not in the route's answer.
#[test]
fn spawn_hands_over_the_cached_key_on_stdin_only() {
    let fixture = Fixture::new();
    let answer = fixture
        .route(
            "POST",
            "/api/workers",
            &[],
            &[
                ("repo", fixture.repo.to_str().unwrap()),
                ("prompt", "Write the docs"),
            ],
        )
        .expect("the spawn route answers");
    let session = answer["sessionId"].as_str().unwrap().to_string();

    let argv: Vec<String> = stub_output(&fixture, "argv.out")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        argv,
        vec![
            "--worker".to_string(),
            "--session".to_string(),
            session.clone(),
            "--data-dir".to_string(),
            fixture.config.data_dir.to_string_lossy().to_string(),
        ]
    );

    let handover: Value = serde_json::from_str(&stub_output(&fixture, "handover.out")).unwrap();
    let token = handover["token"].as_str().unwrap().to_string();
    assert_eq!(handover["apiKey"], "<masked>");
    assert_eq!(handover.as_object().unwrap().len(), 2, "{handover}");
    assert!(
        token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "a 256-bit token in hex"
    );
    let expected = json!({"apiKey": KEY, "token": token}).to_string();
    assert_eq!(
        stub_output(&fixture, "len.out"),
        expected.len().to_string(),
        "the line on stdin is the handover with this key"
    );
    assert!(!argv.join(" ").contains(KEY) && !argv.join(" ").contains(&token));

    let child_env: HashMap<String, String> = stub_output(&fixture, "env.out")
        .split('\0')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    // What the stub's `/bin/sh` changes on its own: it manages these
    // four, and macOS's System Integrity Protection strips `DYLD_*`
    // when it runs a protected binary. The real worker is neither.
    let set_by_sh =
        |name: &str| matches!(name, "PWD" | "OLDPWD" | "SHLVL" | "_") || name.starts_with("DYLD_");
    for (name, value) in std::env::vars().filter(|(name, _)| !set_by_sh(name)) {
        assert_eq!(
            child_env.get(&name),
            Some(&value),
            "the worker gets the shell's environment unchanged: {name}"
        );
    }
    let added: Vec<&String> = child_env
        .keys()
        .filter(|name| std::env::var(name.as_str()).is_err() && !set_by_sh(name))
        .collect();
    assert!(added.is_empty(), "added to the environment: {added:?}");
    assert!(child_env.values().all(|value| !value.contains(KEY)));

    assert!(!answer.to_string().contains(KEY));
    // Everything the spawn wrote: the data directory (session logs,
    // audit, registry) and the repository with its new worktree. The
    // stub's own files are left out; they hold the token by design.
    let written = files_under(&fixture.config.data_dir)
        .into_iter()
        .chain(files_under(&fixture.repo));
    for file in written {
        let bytes = fs::read(&file).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(KEY), "the key reached {}", file.display());
        assert!(
            !text.contains(&token),
            "the token reached {}",
            file.display()
        );
    }

    fixture.supervisor.shutdown_all();
}

/// C6: the shell registers the worker as `worker`, under the worker's
/// session id, with the shell as owner, so the launch sweep and the
/// shell's own `sweep_own` both reach it.
#[test]
fn the_shell_registers_the_worker_as_process_kind_worker() {
    let fixture = Fixture::new();
    let spawned = fixture.spawn("Register me", None);
    let pid = fixture.supervisor.pid(&spawned.session_id).unwrap();

    let registry = ProcessRegistry::open(&fixture.config.data_dir).unwrap();
    let entries: Vec<_> = registry
        .entries()
        .unwrap()
        .into_iter()
        .filter_map(|(_, entry)| entry)
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    let entry = &entries[0];
    assert_eq!(entry.kind, "worker");
    assert_eq!(entry.session_id, spawned.session_id);
    assert_eq!(entry.pid, pid);
    assert_eq!(entry.pgid, pid, "in its own process group");
    assert_eq!(entry.owner_pid, std::process::id(), "owned by the shell");

    fixture.supervisor.shutdown_all();
    assert!(
        registry.entries().unwrap().is_empty(),
        "the entry goes with the worker"
    );
}

/// §5.3 step 5: deleting a parent sends `shutdown` to its workers, and
/// the workers' worktrees and session logs stay.
#[test]
fn deleting_a_parent_sends_shutdown_to_its_workers_and_keeps_their_worktrees() {
    let fixture = Fixture::new();
    let parent = fixture.parent("Planner");
    let other = fixture.parent("Someone else");
    let spawned = fixture.spawn("Child task", Some(&parent));
    let unrelated = fixture.spawn("Unrelated task", Some(&other));
    let requests = fake_socket(&fixture.config.data_dir, &spawned.session_id, echo);
    let store = fixture.store();

    let stopping = workers_to_stop_for_deleted_session(&store, &parent);
    assert_eq!(stopping, vec![spawned.session_id.clone()]);
    for worker in &stopping {
        assert!(fixture.supervisor.shutdown(worker));
    }
    store.delete_session(&parent).unwrap();

    fixture.wait_for_exit(&spawned.session_id);
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["command"] == "shutdown"),
        "{:?}",
        requests.lock().unwrap()
    );
    assert!(fixture.supervisor.is_alive(&unrelated.session_id));
    assert!(spawned.worktree.is_dir(), "the worktree is kept");
    assert!(
        store.read_session(&spawned.session_id).unwrap().is_some(),
        "the worker's session is kept"
    );
    // Deleting a worker's own session stops that worker too.
    assert_eq!(
        workers_to_stop_for_deleted_session(&store, &unrelated.session_id),
        vec![unrelated.session_id.clone()]
    );

    fixture.supervisor.shutdown_all();
}

/// §5.1 "One worker per worktree": *Restart worker* is refused while a
/// live worker holds the session's worktree, and works once it is gone.
#[test]
fn restart_is_refused_while_a_live_worker_holds_the_worktree() {
    let fixture = Fixture::new();
    let spawned = fixture.spawn("Restartable", None);
    let first = fixture.supervisor.pid(&spawned.session_id).unwrap();
    let restart = |fixture: &Fixture| {
        fixture.route(
            "POST",
            "/api/worker-restart",
            &[],
            &[("session_id", spawned.session_id.as_str())],
        )
    };

    let refused = restart(&fixture).unwrap_err();
    assert_eq!(refused.status, 409, "{refused:?}");
    assert_eq!(fixture.supervisor.pid(&spawned.session_id), Some(first));

    assert!(fixture.supervisor.shutdown(&spawned.session_id));
    fixture.wait_for_exit(&spawned.session_id);
    assert_eq!(
        restart(&fixture).unwrap(),
        json!({"status": "started"}),
        "restarted once the worktree is free"
    );
    let second = fixture.supervisor.pid(&spawned.session_id).unwrap();
    assert_ne!(first, second, "a new process for the same session");

    // An ordinary session has no worktree to restart in.
    let ordinary = fixture.parent("Not a worker");
    assert_eq!(
        fixture
            .route(
                "POST",
                "/api/worker-restart",
                &[],
                &[("session_id", ordinary.as_str())],
            )
            .unwrap_err()
            .status,
        400
    );

    fixture.supervisor.shutdown_all();
}

/// #36 via §5.1: a dirty worktree is removed only after the second
/// confirmation, the branch is never deleted, and a live worker's
/// worktree is not removed at all.
#[test]
fn remove_worktree_requires_the_second_confirmation_when_dirty() {
    let fixture = Fixture::new();
    let spawned = fixture.spawn("Leave a mess", None);
    let remove = |confirmed: Option<&str>| {
        let mut form = vec![("session_id", spawned.session_id.as_str())];
        if let Some(confirmed) = confirmed {
            form.push(("confirmed_dirty", confirmed));
        }
        fixture.route("POST", "/api/worker-remove-worktree", &[], &form)
    };

    assert_eq!(
        remove(Some("true")).unwrap_err().status,
        409,
        "not while the worker runs"
    );
    assert!(fixture.supervisor.shutdown(&spawned.session_id));
    fixture.wait_for_exit(&spawned.session_id);
    fs::write(spawned.worktree.join("scratch.txt"), "unsaved\n").unwrap();

    assert_eq!(
        remove(None).unwrap(),
        json!({"status": "dirty", "paths": ["scratch.txt"]})
    );
    assert!(spawned.worktree.join("scratch.txt").exists());
    assert_eq!(
        remove(Some("false")).unwrap()["status"],
        "dirty",
        "only `true` confirms"
    );

    assert_eq!(remove(Some("true")).unwrap(), json!({"status": "removed"}));
    assert!(!spawned.worktree.exists());
    assert!(
        git(&fixture.repo, &["branch", "--list", &spawned.branch]).contains(&spawned.branch),
        "the branch is never deleted"
    );
    assert_eq!(
        audit_events(&fixture.config.data_dir, "worker_worktree_removed").len(),
        1
    );
}

/// C7: `/api/worker-control` takes form fields and relays exactly the
/// socket's names to the worker; the first message a spawn sends is the
/// prompt.
#[test]
fn worker_control_takes_form_fields() {
    let fixture = Fixture::new();
    let spawned = fixture.spawn("First prompt\nwith two lines", None);
    let requests = fake_socket(&fixture.config.data_dir, &spawned.session_id, echo);
    let control = |form: &[(&str, &str)]| {
        let mut fields = vec![("session_id", spawned.session_id.as_str())];
        fields.extend_from_slice(form);
        fixture.route("POST", "/api/worker-control", &[], &fields)
    };
    wait_until("the first message", || !requests.lock().unwrap().is_empty());

    assert_eq!(
        control(&[("command", "note"), ("text", "check the tests & docs")]).unwrap(),
        json!({"ok": true, "echo": "note"}),
        "the socket's own answer"
    );
    control(&[("command", "approve"), ("proposal_id", "prop_1")]).unwrap();
    // Spec 103 Task 2: the plan card posts the exact typed budget as a
    // form value; this route must relay it to the socket's `budget`
    // field byte-for-byte, on approve and on reject.
    let budget = r#"{"actionRounds":96,"explorationRounds":144,"inputTokens":8000000,"outputTokens":512000}"#;
    control(&[
        ("command", "approve"),
        ("proposal_id", "prop_budget_approve"),
        ("budget", budget),
    ])
    .unwrap();
    control(&[
        ("command", "reject"),
        ("proposal_id", "prop_budget_reject"),
        ("budget", budget),
    ])
    .unwrap();
    control(&[("command", "resume"), ("note", "carry on")]).unwrap();
    control(&[("command", "resume")]).unwrap();
    control(&[("command", "pause")]).unwrap();
    control(&[("command", "continue-repair")]).unwrap();
    // Spec 104: a question answer has its own command name and field
    // names; the route must not turn it into an approval.
    control(&[
        ("command", "answer_question"),
        ("question_id", "question_1"),
        ("answer", "The CLI"),
        ("selected_option", "0"),
    ])
    .unwrap();

    let seen = requests.lock().unwrap().clone();
    let token = seen[0]["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 64);
    let without_token: Vec<Value> = seen
        .iter()
        .map(|request| {
            assert_eq!(request["token"], token.as_str(), "one token throughout");
            let mut request = request.clone();
            request.as_object_mut().unwrap().remove("token");
            request
        })
        .collect();
    assert_eq!(
        without_token,
        vec![
            json!({"command": "message", "text": "First prompt\nwith two lines"}),
            json!({"command": "note", "text": "check the tests & docs"}),
            json!({"command": "approve", "proposalId": "prop_1"}),
            json!({
                "command": "approve",
                "proposalId": "prop_budget_approve",
                "budget": budget,
            }),
            json!({
                "command": "reject",
                "proposalId": "prop_budget_reject",
                "budget": budget,
            }),
            json!({"command": "resume", "note": "carry on"}),
            json!({"command": "resume"}),
            json!({"command": "pause"}),
            json!({"command": "continue-repair"}),
            json!({
                "command": "answer_question",
                "questionId": "question_1",
                "answer": "The CLI",
                "selectedOption": "0",
            }),
        ]
    );

    let unknown = control(&[("command", "self_destruct")]).unwrap_err();
    assert_eq!(unknown.status, 400, "{unknown:?}");
    // A JSON body, as §5.7 first described it, is not a form.
    let json_body = route(
        &fixture.supervisor,
        &fixture.config,
        &|_: &Config| Ok(KEY.to_string()),
        &WorkerRequest {
            method: "POST",
            path: "/api/worker-control",
            query: &HashMap::new(),
            form: &parse_form(&format!(
                r#"{{"sessionId":"{}","command":"status"}}"#,
                spawned.session_id
            )),
        },
    )
    .unwrap_err();
    assert_eq!(json_body.status, 400, "{json_body:?}");
    assert_eq!(
        requests.lock().unwrap().len(),
        seen.len(),
        "neither reached it"
    );

    fixture.supervisor.shutdown_all();
}

/// §5.7: a project's board lists the workers whose source is that
/// repository, in Needs you / Running / Finished, newest first, and
/// only finished rows carry files changed and commits ahead.
#[test]
fn workers_list_groups_by_source_repository() {
    let fixture = Fixture::new();
    let other_repo = repository(&fixture.root, "o");
    let parent = fixture.parent("Planner");
    let waiting = fixture.spawn("Waiting one", Some(&parent));
    let patch_waiting = fixture.spawn("Patch waiting", None);
    let running = fixture.spawn("Running one", None);
    let done = fixture.spawn("Done one", None);
    let gone = fixture.spawn("Gone one", None);
    let elsewhere = fixture.spawn_from(&other_repo, "Elsewhere", None);
    // A progress status is a report about a durable task; the board's
    // engine control reads the task itself.
    let done_task = fixture
        .store()
        .create_task(&done.session_id, "Done one", "mock", "mock")
        .unwrap();
    fixture
        .store()
        .update_task_status(&done_task, TaskStatus::Complete, None)
        .unwrap();
    let progress = |session: &str, lines: &[(ProgressKind, Value)]| {
        let mut writer = ProgressWriter::create(
            &fixture.config.data_dir,
            session,
            SecretScanner::new(Vec::new()),
        )
        .unwrap();
        for (kind, payload) in lines {
            writer.append(*kind, payload.clone()).unwrap();
        }
    };
    progress(
        &waiting.session_id,
        &[(
            ProgressKind::Status,
            json!({"state": "waiting_for_approval"}),
        )],
    );
    progress(
        &patch_waiting.session_id,
        &[(
            ProgressKind::Status,
            json!({"state": "idle", "taskStatus": "waiting_for_approval"}),
        )],
    );
    progress(
        &running.session_id,
        &[
            (ProgressKind::Status, json!({"state": "running"})),
            (
                ProgressKind::Phase,
                json!({
                    "phase": "tool",
                    "label": "Running cargo test",
                    "round": 2,
                    "maxRounds": 8,
                    "explorationRound": 3,
                    "maxExplorationRounds": 24,
                }),
            ),
        ],
    );
    progress(
        &done.session_id,
        &[(
            ProgressKind::Status,
            json!({"state": "idle", "taskStatus": "complete"}),
        )],
    );
    progress(
        &gone.session_id,
        &[(ProgressKind::Status, json!({"state": "running"}))],
    );
    assert!(fixture.supervisor.shutdown(&gone.session_id));
    fixture.wait_for_exit(&gone.session_id);

    let list = || {
        fixture
            .route(
                "GET",
                "/api/workers",
                &[("repo", fixture.repo.to_str().unwrap())],
                &[],
            )
            .unwrap()["workers"]
            .as_array()
            .unwrap()
            .clone()
    };
    let rows = list();
    let summary: Vec<(String, String, String)> = rows
        .iter()
        .map(|row| {
            (
                row["title"].as_str().unwrap().to_string(),
                row["group"].as_str().unwrap().to_string(),
                row["state"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let row = |title: &str, group: &str, state: &str| {
        (title.to_string(), group.to_string(), state.to_string())
    };
    assert_eq!(
        summary,
        vec![
            row("Patch waiting", "needs_you", "idle"),
            row("Waiting one", "needs_you", "waiting_for_approval"),
            row("Running one", "running", "running"),
            row("Gone one", "finished", "exited"),
            row("Done one", "finished", "idle"),
        ],
        "grouped, newest first in each group, and only this repository's"
    );
    assert!(
        rows.iter()
            .all(|row| row["sessionId"] != elsewhere.session_id)
    );

    let by_title = |rows: &[Value], title: &str| {
        rows.iter()
            .find(|row| row["title"] == title)
            .unwrap()
            .clone()
    };
    let waiting_row = by_title(&rows, "Waiting one");
    assert_eq!(waiting_row["parentSessionId"], parent);
    assert_eq!(waiting_row["parentTitle"], "Planner");
    assert_eq!(waiting_row["alive"], true);
    assert_eq!(waiting_row["filesChanged"], Value::Null);
    let patch_waiting_row = by_title(&rows, "Patch waiting");
    assert_eq!(patch_waiting_row["doing"], "Needs your decision");
    assert_eq!(patch_waiting_row["control"]["needsDecision"], true);
    assert_eq!(
        patch_waiting_row["control"]["primaryAction"]["command"],
        "review_decision"
    );
    let running_row = by_title(&rows, "Running one");
    assert_eq!(running_row["parentTitle"], Value::Null);
    assert_eq!(running_row["round"], 2);
    assert_eq!(running_row["maxRounds"], 8);
    assert_eq!(
        running_row["explorationRound"], 3,
        "the board row mirrors the phase's read count additively"
    );
    assert_eq!(running_row["maxExplorationRounds"], 24);
    assert_eq!(running_row["doing"], "Running cargo test");
    assert_eq!(running_row["branch"], running.branch);
    let gone_row = by_title(&rows, "Gone one");
    assert_eq!(gone_row["alive"], false);
    assert_eq!(gone_row["filesChanged"], 0);
    assert_eq!(gone_row["commitsAhead"], 0);
    let done_row = by_title(&rows, "Done one");
    assert_eq!(done_row["taskStatus"], "complete");
    assert_eq!(done_row["control"]["outcome"], "complete");
    assert_eq!(
        done_row["control"]["primaryAction"]["command"],
        "send_follow_up"
    );

    fs::write(gone.worktree.join("README.md"), "# Changed\n").unwrap();
    git(&gone.worktree, &["commit", "-qam", "one ahead"]);
    fs::write(gone.worktree.join("new.txt"), "uncommitted\n").unwrap();
    git(&gone.worktree, &["add", "new.txt"]);
    let gone_row = by_title(&list(), "Gone one");
    assert_eq!(gone_row["commitsAhead"], 1);
    assert_eq!(
        gone_row["filesChanged"], 2,
        "committed and staged, against the base"
    );

    fixture.supervisor.shutdown_all();
}

/// Spec 95: without `repo`, the list is the one Workers panel's, with
/// every project's workers, each naming the project it belongs to.
#[test]
fn the_workers_list_covers_every_project_without_a_repo() {
    let fixture = Fixture::new();
    let other_repo = repository(&fixture.root, "o");
    let here = fixture.spawn("Here", None);
    let elsewhere = fixture.spawn_from(&other_repo, "Elsewhere", None);

    let rows = fixture.route("GET", "/api/workers", &[], &[]).unwrap()["workers"]
        .as_array()
        .unwrap()
        .clone();
    let source_of = |session: &str| {
        rows.iter()
            .find(|row| row["sessionId"] == session)
            .map(|row| row["sourceRepository"].as_str().unwrap().to_string())
            .unwrap_or_else(|| panic!("{session} is not listed: {rows:?}"))
    };
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        source_of(&here.session_id),
        fs::canonicalize(&fixture.repo)
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(
        source_of(&elsewhere.session_id),
        fs::canonicalize(&other_repo).unwrap().display().to_string()
    );

    fixture.supervisor.shutdown_all();
}

/// Spec 101 Task 3: an incomplete worker's row and progress answer use
/// the engine's outcome and action, preserve the literal status, and
/// suppress the composer when a decision is pending.
#[test]
fn worker_outcomes_use_engine_control_and_preserve_incomplete() {
    let fixture = Fixture::new();
    let spawned = fixture.spawn("Incomplete task", None);
    let store = fixture.store();
    let task = store
        .create_task(&spawned.session_id, "Finish it", "mock", "mock")
        .unwrap();
    let mut plan = TaskPlan::new(task.id.clone(), 0);
    plan.steps.push(PlanStep {
        id: "finish".to_string(),
        title: "Finish the work".to_string(),
        detail: None,
        status: StepStatus::InProgress,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
        check: None,
        build: None,
    });
    store.create_plan(&task, &plan).unwrap();
    let report = workspace_engine::CompletionGateReport {
        task_id: task.id.clone(),
        eligible: false,
        reasons: vec![workspace_engine::CompletionReason {
            code: workspace_engine::CompletionReasonCode::OpenPlanSteps,
            explanation: "1 planned step is still open: \"Finish the work\".".to_string(),
        }],
        waivers: Vec::new(),
        accepted_failures: Vec::new(),
    };
    store
        .update_task_status_with_completion(&task, TaskStatus::Incomplete, &report)
        .unwrap();
    let mut writer = ProgressWriter::create(
        &fixture.config.data_dir,
        &spawned.session_id,
        SecretScanner::new(Vec::new()),
    )
    .unwrap();
    writer
        .append(
            ProgressKind::Status,
            json!({"state": "idle", "taskStatus": "incomplete"}),
        )
        .unwrap();

    let rows = fixture
        .route(
            "GET",
            "/api/workers",
            &[("repo", fixture.repo.to_str().unwrap())],
            &[],
        )
        .unwrap()["workers"]
        .as_array()
        .unwrap()
        .clone();
    let row = rows
        .iter()
        .find(|row| row["sessionId"] == spawned.session_id.as_str())
        .unwrap();
    assert_eq!(row["taskStatus"], "incomplete");
    assert_eq!(row["control"]["status"], "incomplete");
    assert_eq!(row["control"]["outcome"], "incomplete");
    assert_eq!(row["control"]["headline"], "Task incomplete");
    assert_eq!(row["control"]["primaryAction"]["command"], "continue_task");
    assert_eq!(row["control"]["primaryAction"]["input"], "optional_note");
    assert!(
        row["control"]["detail"]
            .as_str()
            .unwrap()
            .contains("Finish the work"),
        "{row}"
    );

    let page = fixture
        .route(
            "GET",
            "/api/worker-progress",
            &[("session", spawned.session_id.as_str()), ("after", "0")],
            &[],
        )
        .unwrap();
    assert_eq!(page["control"]["status"], "incomplete");
    assert_eq!(page["control"]["outcome"], "incomplete");

    writer
        .append(
            ProgressKind::Status,
            json!({"state": "waiting_for_approval", "taskStatus": "incomplete"}),
        )
        .unwrap();
    let page = fixture
        .route(
            "GET",
            "/api/worker-progress",
            &[("session", spawned.session_id.as_str()), ("after", "0")],
            &[],
        )
        .unwrap();
    assert_eq!(page["control"]["needsDecision"], true);
    assert_eq!(page["control"]["composer"], Value::Null);
    assert_eq!(
        page["control"]["primaryAction"]["command"],
        "review_decision"
    );

    fixture.supervisor.shutdown_all();
}

/// Spec 104: a waiting question is projected as **Needs your answer**,
/// grouped under *Needs you* with the review action, and never as an
/// approval.
#[test]
fn a_worker_question_is_needs_your_answer_on_the_board_and_progress() {
    let fixture = Fixture::new();
    let spawned = fixture.spawn("Waiting on a question", None);
    let task = fixture
        .store()
        .create_task(&spawned.session_id, "Ask first", "mock", "mock")
        .unwrap();
    fixture
        .store()
        .update_task_status(&task, TaskStatus::WaitingForApproval, None)
        .unwrap();
    let mut writer = ProgressWriter::create(
        &fixture.config.data_dir,
        &spawned.session_id,
        SecretScanner::new(Vec::new()),
    )
    .unwrap();
    writer
        .append(
            ProgressKind::Question,
            json!({"questionId": "question_1", "question": "Which one?"}),
        )
        .unwrap();
    writer
        .append(
            ProgressKind::Status,
            json!({
                "state": "waiting_for_approval",
                "taskStatus": "waiting_for_approval",
                "questionId": "question_1",
            }),
        )
        .unwrap();

    let rows = fixture
        .route(
            "GET",
            "/api/workers",
            &[("repo", fixture.repo.to_str().unwrap())],
            &[],
        )
        .unwrap()["workers"]
        .as_array()
        .unwrap()
        .clone();
    let row = rows
        .iter()
        .find(|row| row["sessionId"] == spawned.session_id.as_str())
        .unwrap();
    assert_eq!(row["group"], "needs_you");
    assert_eq!(row["doing"], "Needs your answer");
    assert_eq!(row["control"]["outcome"], "needs_answer");
    assert_eq!(row["control"]["headline"], "Needs your answer");
    assert_eq!(row["control"]["needsDecision"], true);
    assert_eq!(
        row["control"]["primaryAction"]["command"],
        "review_decision"
    );
    assert_eq!(row["control"]["composer"], Value::Null);

    let page = fixture
        .route(
            "GET",
            "/api/worker-progress",
            &[("session", spawned.session_id.as_str()), ("after", "0")],
            &[],
        )
        .unwrap();
    assert_eq!(page["control"]["outcome"], "needs_answer");
    assert!(
        !page["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "approval"),
        "a question is not an approval: {page}"
    );

    fixture.supervisor.shutdown_all();
}

/// `after=` pages exactly, `alive` says whether this shell still holds
/// the worker, and a session id that is not one is refused before any
/// path is built from it.
#[test]
fn worker_progress_pages_with_after() {
    let fixture = Fixture::new();
    let spawned = fixture.spawn("Make progress", None);
    let mut writer = ProgressWriter::create(
        &fixture.config.data_dir,
        &spawned.session_id,
        SecretScanner::new(Vec::new()),
    )
    .unwrap();
    for text in ["one", "two", "three"] {
        writer
            .append(ProgressKind::Text, json!({"text": text}))
            .unwrap();
    }
    let page = |after: &str| {
        fixture.route(
            "GET",
            "/api/worker-progress",
            &[("session", spawned.session_id.as_str()), ("after", after)],
            &[],
        )
    };

    let answer = page("1").unwrap();
    let seqs: Vec<u64> = answer["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, vec![2, 3]);
    assert_eq!(answer["events"][1]["payload"]["text"], "three");
    assert_eq!(answer["alive"], true);
    assert_eq!(answer["control"]["headline"], "Starting worker");
    assert_eq!(page("3").unwrap()["events"], json!([]));
    assert_eq!(page("0").unwrap()["events"].as_array().unwrap().len(), 3);

    assert!(fixture.supervisor.shutdown(&spawned.session_id));
    fixture.wait_for_exit(&spawned.session_id);
    assert_eq!(page("0").unwrap()["alive"], false);

    let traversal = fixture
        .route(
            "GET",
            "/api/worker-progress",
            &[("session", "../../etc/x"), ("after", "0")],
            &[],
        )
        .unwrap_err();
    assert_eq!(traversal.status, 400);
    assert_eq!(page("not-a-number").unwrap_err().status, 400);
}

/// Requirement 1 and §5.0: with the setting off, every worker route
/// refuses, except the list (a running worker stays reachable) and
/// the setting's own read.
#[test]
fn every_worker_route_refuses_when_the_setting_is_off() {
    let fixture = Fixture::new();
    let spawned = fixture.spawn("Started while on", None);
    let off = Config {
        enable_worker_sessions: false,
        ..fixture.config.clone()
    };
    let repo = fixture.repo.to_str().unwrap();
    let session = spawned.session_id.as_str();
    type Case<'a> = (
        &'a str,
        &'a str,
        Vec<(&'a str, &'a str)>,
        Vec<(&'a str, &'a str)>,
    );
    let refused: Vec<Case> = vec![
        (
            "GET",
            "/api/worker-source-status",
            vec![("repo", repo), ("prompt", "x")],
            vec![],
        ),
        (
            "POST",
            "/api/workers",
            vec![],
            vec![("repo", repo), ("prompt", "x")],
        ),
        (
            "GET",
            "/api/worker-progress",
            vec![("session", session), ("after", "0")],
            vec![],
        ),
        (
            "POST",
            "/api/worker-control",
            vec![],
            vec![("session_id", session), ("command", "status")],
        ),
        (
            "POST",
            "/api/worker-restart",
            vec![],
            vec![("session_id", session)],
        ),
        (
            "POST",
            "/api/worker-remove-worktree",
            vec![],
            vec![("session_id", session)],
        ),
    ];
    for (method, path, query, form) in refused {
        let refusal = fixture
            .route_with(&off, method, path, &query, &form)
            .expect_err(path);
        assert_eq!(refusal.status, 403, "{path}: {refusal:?}");
    }
    assert_eq!(
        fixture
            .store()
            .worker_sessions_for_source(&fixture.repo)
            .unwrap()
            .len(),
        1,
        "the refused spawn created nothing"
    );

    let listed = fixture
        .route_with(&off, "GET", "/api/workers", &[("repo", repo)], &[])
        .unwrap();
    assert_eq!(listed["workers"][0]["sessionId"], session);
    assert_eq!(listed["workers"][0]["alive"], true);
    assert_eq!(
        fixture
            .route_with(&off, "GET", "/api/workers-enabled", &[], &[])
            .unwrap(),
        json!({"enabled": false})
    );
    assert_eq!(
        fixture
            .route("GET", "/api/workers-enabled", &[], &[])
            .unwrap(),
        json!({"enabled": true})
    );

    fixture.supervisor.shutdown_all();
}

/// The routes are served by `handle_connection`, through the global
/// supervisor and the shell's own config, which is off by default.
#[test]
fn the_shell_serves_the_worker_routes_and_they_refuse_while_off() {
    isolated_data_dir();
    let repo = scratch();
    let (port, token) = serve_for_test();

    let control = post_fields_for_test(
        port,
        &token,
        "/api/worker-control",
        &[("session_id", "session_x"), ("command", "status")],
    );
    assert!(control.starts_with("HTTP/1.1 403"), "{control}");
    let enabled = get_for_test(port, &token, "/api/workers-enabled");
    assert!(enabled.starts_with("HTTP/1.1 200"), "{enabled}");
    assert_eq!(json_of(&enabled), json!({"enabled": false}));
    let listed = get_for_test(
        port,
        &token,
        &format!("/api/workers?repo={}", repo.display()),
    );
    assert_eq!(json_of(&listed), json!({"workers": []}), "{listed}");
}

/// Every stub here ends by its lifeline: `shutdown_all` lets go of
/// each worker's stdin and waits for it, and no signal is needed.
#[test]
fn shutdown_all_ends_every_worker_and_waits_for_it() {
    let fixture = Fixture::new();
    let first = fixture.spawn("First", None);
    let second = fixture.spawn("Second", None);
    let pids = [
        fixture.supervisor.pid(&first.session_id).unwrap(),
        fixture.supervisor.pid(&second.session_id).unwrap(),
    ];

    fixture.supervisor.shutdown_all();

    for pid in pids {
        assert!(
            workspace_engine::ProcessIdentity::of(pid).is_none(),
            "{pid} is still running after shutdown_all returned"
        );
    }
    assert!(!fixture.supervisor.is_alive(&first.session_id));
    assert!(!fixture.supervisor.is_alive(&second.session_id));
    assert!(
        ProcessRegistry::open(&fixture.config.data_dir)
            .unwrap()
            .entries()
            .unwrap()
            .is_empty(),
        "every registration went with its worker"
    );
    assert!(
        audit_events(&fixture.config.data_dir, "worker_terminated").is_empty(),
        "the lifeline ended them; no signal was needed"
    );
}

/// A scripted worker for the stop escalation: what it answers, whether
/// a stop ends its turn, and whether `shutdown` makes it exit.
struct FakeTarget {
    state: &'static str,
    answers: bool,
    stop_ends_turn: bool,
    exits_on_shutdown: bool,
    exited: bool,
    terminated: bool,
    sent: Vec<String>,
}

impl FakeTarget {
    fn new(state: &'static str) -> Self {
        Self {
            state,
            answers: true,
            stop_ends_turn: false,
            exits_on_shutdown: false,
            exited: false,
            terminated: false,
            sent: Vec::new(),
        }
    }
}

impl StopTarget for FakeTarget {
    fn send(&mut self, command: &str) -> Option<Value> {
        self.sent.push(command.to_string());
        if !self.answers || self.exited {
            return None;
        }
        Some(match command {
            "status" => json!({"ok": true, "state": self.state}),
            "stop" => match self.state {
                "running" | "paused" => {
                    if self.stop_ends_turn {
                        self.state = "idle";
                    }
                    json!({"ok": true})
                }
                "waiting_for_approval" => {
                    json!({"ok": false, "error": "approve or reject it"})
                }
                _ => json!({"ok": false, "error": "no turn is running"}),
            },
            "shutdown" => {
                if self.exits_on_shutdown {
                    self.exited = true;
                }
                json!({"ok": true})
            }
            _ => json!({"ok": false, "error": "unexpected"}),
        })
    }

    fn has_exited(&mut self) -> bool {
        self.exited
    }

    fn terminate(&mut self) {
        self.terminated = true;
        self.exited = true;
    }
}

fn short() -> WorkerTimeouts {
    WorkerTimeouts {
        stop_to_shutdown: Duration::from_millis(150),
        shutdown_to_sigterm: Duration::from_millis(150),
        poll: Duration::from_millis(10),
        ..WorkerTimeouts::default()
    }
}

/// §5.3: `stop`; the turn has not ended in time, so `shutdown`; the
/// process has not exited in time, so `SIGTERM` by registry identity.
/// The real-process half is `tests/workers.rs`'s
/// `stop_escalates_to_shutdown_then_sigterm_by_registry_pid`.
#[test]
fn the_stop_escalation_sends_shutdown_then_sigterm_after_its_timeouts() {
    let mut target = FakeTarget::new("running");
    let (answer, next) = begin_stop(&mut target);
    assert_eq!(answer, json!({"ok": true}), "the stop's own answer");
    assert_eq!(next, StopNext::AwaitTurnEnd);

    let started = Instant::now();
    assert_eq!(
        finish_stop(&mut target, next, &short()),
        StopOutcome::Terminated
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(5),
        "waited both timeouts and no longer: {elapsed:?}"
    );
    assert!(target.terminated);
    assert_eq!(&target.sent[..2], ["status", "stop"]);
    assert_eq!(
        target
            .sent
            .iter()
            .filter(|sent| *sent == "shutdown")
            .count(),
        1,
        "{:?}",
        target.sent
    );
}

#[test]
fn a_stop_that_ends_the_turn_goes_no_further() {
    let mut target = FakeTarget::new("paused");
    target.stop_ends_turn = true;
    let (_, next) = begin_stop(&mut target);
    assert_eq!(
        finish_stop(&mut target, next, &short()),
        StopOutcome::TurnEnded
    );
    assert!(!target.sent.contains(&"shutdown".to_string()));
    assert!(!target.terminated);
}

#[test]
fn a_worker_that_exits_on_shutdown_is_not_signalled() {
    let mut target = FakeTarget::new("running");
    target.exits_on_shutdown = true;
    let (_, next) = begin_stop(&mut target);
    assert_eq!(
        finish_stop(&mut target, next, &short()),
        StopOutcome::ExitedAfterShutdown
    );
    assert!(!target.terminated);
}

/// Task 6 (d): `stop` is refused while waiting for approval, so the
/// escalation does not send it and goes straight to `shutdown`.
#[test]
fn a_stop_while_waiting_for_approval_goes_straight_to_shutdown() {
    let mut target = FakeTarget::new("waiting_for_approval");
    target.exits_on_shutdown = true;
    let (answer, next) = begin_stop(&mut target);
    assert_eq!(next, StopNext::Shutdown);
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(target.sent, ["status"]);
    assert_eq!(
        finish_stop(&mut target, next, &short()),
        StopOutcome::ExitedAfterShutdown
    );
    assert_eq!(target.sent, ["status", "shutdown"]);
}

/// An idle worker has no turn to stop and is left running.
#[test]
fn an_idle_worker_is_not_escalated() {
    let mut target = FakeTarget::new("idle");
    let (answer, next) = begin_stop(&mut target);
    assert_eq!(next, StopNext::Done);
    assert_eq!(answer["ok"], false, "the worker's own refusal: {answer}");
    assert_eq!(finish_stop(&mut target, next, &short()), StopOutcome::Done);
    assert!(!target.sent.contains(&"shutdown".to_string()));
}

/// A worker that does not answer at all is shut down, then signalled.
#[test]
fn a_silent_worker_is_shut_down_then_signalled() {
    let mut target = FakeTarget::new("running");
    target.answers = false;
    let (answer, next) = begin_stop(&mut target);
    assert_eq!(next, StopNext::Shutdown);
    assert_eq!(answer["ok"], false);
    assert_eq!(
        finish_stop(&mut target, next, &short()),
        StopOutcome::Terminated
    );
}
