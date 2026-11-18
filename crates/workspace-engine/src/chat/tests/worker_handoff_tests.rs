use super::*;
use crate::mode::SessionMode;
use crate::model::MockModelAdapter;
use crate::session::WorkerRecord;
use crate::workspace_engine::WorkspaceEngine;
use std::sync::atomic::{AtomicU64, Ordering};

const ALL_MODES: [SessionMode; 4] = [
    SessionMode::Ask,
    SessionMode::Plan,
    SessionMode::Code,
    SessionMode::Review,
];

#[test]
fn the_handoff_paragraph_is_appended_when_offered() {
    // Verbatim from proposal §5.0, re-typed here so an edit to the
    // constant has to be an edit to the spec too.
    let from_the_spec = "When work should continue in a separate session — because it is a self-contained task, or one of several that can run in parallel — write each task as a prompt in its own fenced block whose opening line is exactly ```worker-prompt, with the tag on the fence line itself and not inside the block. The user can start a worker from that block. A worker starts with a fresh context in its own git worktree and sees only the repository and that prompt, so say what to read, what to change, which checks to run, and when to stop. Do not use the `worker-prompt` tag for anything else.";
    assert_eq!(WORKER_HANDOFF_PARAGRAPH, from_the_spec);
    // Verbatim from spec 76 §4 as amended by spec 100 §4, for the same
    // reason.
    let worktree_task = "Your edits and commands always apply to this session's checkout. The `worktree` argument of the read tools only reads. When the user asks for work on a new branch or in a separate git worktree, do not create a worktree yourself: recommend a worker in a ```worker-prompt block, which runs it in a worktree of its own. If the user explicitly chooses to work in this session instead, for example with *Work here instead*, do the work in the current checkout; that choice does not create or select another worktree.";
    assert_eq!(WORKTREE_TASK_PARAGRAPH, worktree_task);
    // Spec 100: the paragraph recommends isolation but must keep the
    // explicit inline choice and the rule against silently creating a
    // worktree.
    assert!(WORKTREE_TASK_PARAGRAPH.contains("Work here instead"));
    assert!(WORKTREE_TASK_PARAGRAPH.contains("do not create a worktree yourself"));

    for mode in ALL_MODES {
        for user_present in [true, false] {
            let options = ChatTurnOptions {
                synchronous_user_present: user_present,
                question_route_available: user_present,
                ..ChatTurnOptions::default()
            };
            assert_eq!(
                system_prompt_for_session(mode, &options, true, "/bin/zsh"),
                format!(
                    "{}\n\n{worktree_task}\n\n{from_the_spec}",
                    system_prompt(mode, user_present, "/bin/zsh")
                ),
                "{mode:?}"
            );
        }
    }
}

#[test]
fn no_mode_changes_when_handoffs_are_not_offered() {
    for mode in ALL_MODES {
        for user_present in [true, false] {
            let options = ChatTurnOptions {
                synchronous_user_present: user_present,
                question_route_available: user_present,
                ..ChatTurnOptions::default()
            };
            assert_eq!(
                system_prompt_for_session(mode, &options, false, "/bin/zsh"),
                system_prompt(mode, user_present, "/bin/zsh"),
                "{mode:?}"
            );
        }
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-worker-handoff-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), "# Worker handoff test\n").unwrap();
    dir
}

fn engine_with_workers_on(repo: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        enable_worker_sessions: true,
        shell: "/bin/zsh".to_string(),
        ..Config::default()
    })
}

fn system_message(adapter: &MockModelAdapter) -> &str {
    &adapter.requests[0].messages[0].content
}

/// Runs one turn in a new session and returns the session id.
fn first_turn(engine: &WorkspaceEngine, repo: &Path, adapter: &mut MockModelAdapter) -> String {
    let mut on_token = |_token: &str| {};
    engine
        .chat_orchestrator
        .ask(repo, "Plan the work.", &[], adapter, &mut on_token)
        .unwrap()
        .session
        .id
}

#[test]
fn a_non_worker_session_gets_the_paragraph_when_enabled() {
    let repo = temp_repo("enabled");
    let engine = engine_with_workers_on(&repo);

    let mut adapter = MockModelAdapter::new("Here is the split.");
    first_turn(&engine, &repo, &mut adapter);

    assert!(
        system_message(&adapter).ends_with(WORKER_HANDOFF_PARAGRAPH),
        "{}",
        system_message(&adapter)
    );
    // Spec 76: a worktree request goes to a worker, said just before the
    // handoff paragraph and only when handoffs are offered.
    assert!(
        system_message(&adapter).ends_with(&format!(
            "{WORKTREE_TASK_PARAGRAPH}\n\n{WORKER_HANDOFF_PARAGRAPH}"
        )),
        "{}",
        system_message(&adapter)
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_worker_session_never_gets_the_paragraph() {
    let repo = temp_repo("worker");
    let engine = engine_with_workers_on(&repo);
    let mut warm = MockModelAdapter::new("Ready.");
    let session_id = first_turn(&engine, &repo, &mut warm);
    engine
        .session_store
        .record_worker_spawned(
            &session_id,
            &WorkerRecord {
                parent_session_id: None,
                parent_task_id: None,
                inherited_protected_paths: Vec::new(),
                source_repository: repo.clone(),
                worktree_path: repo.join(".damaian/worktrees/worker"),
                branch: "damaian/worker".to_string(),
                base_commit: "0123456".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .unwrap();

    let mut adapter = MockModelAdapter::new("Working.");
    let mut on_token = |_token: &str| {};
    let cancel = CancelToken::new();
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(
            &repo,
            "Do the task.",
            &[],
            Some(&session_id),
            &mut adapter,
            &mut sink,
        )
        .unwrap();

    assert!(
        !system_message(&adapter).contains("worker-prompt"),
        "a worker must not be invited to hand off: {}",
        system_message(&adapter)
    );
    assert_eq!(
        system_message(&adapter),
        system_prompt(SessionMode::Code, false, "/bin/zsh")
    );

    fs::remove_dir_all(repo).unwrap();
}
