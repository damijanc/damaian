use super::{TurnEvent, resume_round_budget_decision};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CancelToken, ChatTurnResult, Config, MockModelAdapter, ModelAdapter, TaskStatus, TurnProgress,
    TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "damaian-shell-roundbudget-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.rs"), "fn main() {}\n").unwrap();
    dir
}

fn config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        agent_max_tool_rounds: 8,
        enable_index_watcher: false,
        ..Config::default()
    }
}

fn read_only_round() -> String {
    "DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look again.\nEND_COMMAND\n".to_string()
}

fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// A turn that hit its round budget and paused for the user.
fn paused_turn(repo: &Path) -> ChatTurnResult {
    let engine = WorkspaceEngine::new(config(repo));
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(
            repo,
            "Look at the source",
            &[],
            None,
            &mut MockModelAdapter::new_sequence((0..8).map(|_| read_only_round()).collect()),
            &mut sink,
        )
        .expect("the turn pauses");
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    result
}

#[test]
fn the_round_budget_route_resumes_the_pause() {
    let repo = temp_repo();
    let paused = paused_turn(&repo);
    let proposal_id = paused
        .round_budget_proposal
        .as_ref()
        .expect("the pause surfaces a proposal")
        .id
        .clone();

    let (events, _receiver) = std::sync::mpsc::channel::<TurnEvent>();
    let mut adapters = 0;
    let result = resume_round_budget_decision(
        &form(&[("proposal_id", &proposal_id), ("decision", "continue")]),
        &CancelToken::new(),
        &events,
        &|| Ok(WorkspaceEngine::new(config(&repo))),
        &mut |_config| {
            adapters += 1;
            Ok(Box::new(MockModelAdapter::new("Done.")) as Box<dyn ModelAdapter>)
        },
    );

    let resumed = result.expect("the decision resumes the pause");
    assert_eq!(
        resumed.task.id, paused.task.id,
        "resuming continues the same task, never a new one"
    );
    assert_eq!(resumed.task.status, TaskStatus::Complete);
    assert_eq!(adapters, 1, "one adapter for the resumed turn");
}
