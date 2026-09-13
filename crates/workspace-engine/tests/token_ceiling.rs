//! The enforced per-task token ceiling, per
//! `docs/specs/21_task_plan_progress_and_budget/proposal.md` §5.4.
//!
//! `token_accounting.rs` covers what a run costs; this covers what happens when
//! the total reaches a bound. The rule these tests exist to hold is the one
//! `context.md` §3.3 corrects the proposal on: the check runs **before** the
//! request is built, not after the response arrives. The round budget's
//! `force_final` does the opposite — it spends one more model call on crossing
//! — and because context grows monotonically across rounds that call is the
//! most expensive of the turn. A ceiling whose enforcement action is to spend
//! more than the ceiling is not a ceiling, so these tests count calls rather
//! than only reading the final status.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnResult, Config, MockModelAdapter, TaskStatus, TurnProgress, TurnSink,
    WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-ceiling-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn engine_with_ceiling(repo: &Path, ceiling: Option<u64>) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        agent_max_task_tokens: ceiling,
        ..Config::default()
    })
}

/// Runs one turn and hands back both the result and how many times the model
/// was actually called — the number the ceiling is supposed to bound.
fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut MockModelAdapter,
) -> (ChatTurnResult, usize) {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run");
    let calls = adapter.requests.len();
    (result, calls)
}

/// A model that keeps asking for a cheap command, so the turn would run to the
/// round limit if nothing else stopped it. The text envelope is used rather
/// than native tool calls so this does not depend on tool wiring.
fn keeps_working() -> MockModelAdapter {
    MockModelAdapter::new("DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look again.\nEND_COMMAND\n")
}

fn usage_of(engine: &WorkspaceEngine, result: &ChatTurnResult) -> u64 {
    engine
        .session_store
        .read_task_usage(&result.session.id)
        .expect("usage should read")
        .get(&result.task.id)
        .map(|usage| usage.input_tokens + usage.output_tokens)
        .unwrap_or(0)
}

#[test]
fn an_unset_ceiling_imposes_no_limit() {
    // The upgrade guarantee: a configuration written before this existed must
    // behave exactly as it did. Establishes the unbounded call count the
    // ceiling tests below are measured against.
    let repo = temp_repo("unset");
    let engine = engine_with_ceiling(&repo, None);
    let (result, calls) = ask(&engine, &repo, "Look at the source", &mut keeps_working());

    assert_ne!(result.task.status, TaskStatus::TokenBudgetExhausted);
    assert!(
        calls > 2,
        "the turn should have run on unbounded, got {calls} calls"
    );
}

#[test]
fn a_turn_stops_before_the_call_that_would_cross_the_ceiling() {
    // The correction in `context.md` §3.3, and the assertion has to be an
    // **exact** count rather than "fewer than unbounded".
    //
    // That looser version was written first and it was worthless: a turn that
    // enforced the ceiling the round budget's way — one more model call with
    // tools dropped, then stop — also runs fewer calls than an unbounded turn,
    // so it passed. Moving the check produced no failure at all. The whole
    // point of this ceiling is that it does *not* spend a call to discover it
    // has run out, and only the exact number can say so.
    //
    // 3, measured rather than assumed: two calls record enough estimated usage
    // to reach 1000 tokens, and the third iteration stops before its request is
    // built. The `force_final` shape would make it 4. If this number moves,
    // check *why* before updating it — a change here is a change in what the
    // ceiling costs.
    let repo = temp_repo("stops");
    let engine = engine_with_ceiling(&repo, Some(1_000));
    let (result, calls) = ask(&engine, &repo, "Look at the source", &mut keeps_working());

    assert_eq!(result.task.status, TaskStatus::TokenBudgetExhausted);
    assert_eq!(
        calls, 3,
        "the ceiling must not spend a model call discovering it was reached"
    );
}

#[test]
fn an_estimated_total_is_enforced_against() {
    // §5.4: declining to enforce on an estimate would make the ceiling
    // inoperative for every provider that does not report usage — which is
    // most of them. `MockModelAdapter` reports no usage, so everything it
    // records is `Estimated`, and this turn is that case by construction.
    let repo = temp_repo("estimated");
    let engine = engine_with_ceiling(&repo, Some(1_000));
    let (result, _) = ask(&engine, &repo, "Look at the source", &mut keeps_working());

    let source = engine
        .session_store
        .read_task_usage(&result.session.id)
        .expect("usage should read")
        .get(&result.task.id)
        .map(|usage| usage.source)
        .expect("the turn recorded usage");
    assert_eq!(
        source,
        workspace_engine::UsageSource::Estimated,
        "this test is only meaningful while the mock's usage is estimated"
    );
    assert_eq!(result.task.status, TaskStatus::TokenBudgetExhausted);
}

#[test]
fn the_stop_reports_the_ceiling_and_what_was_actually_spent() {
    // §5.4: the report names the usage consumed. A message that said only
    // "budget exhausted" would leave the user unable to tell a ceiling set too
    // low from a turn that genuinely ran away.
    let repo = temp_repo("reports");
    let engine = engine_with_ceiling(&repo, Some(1_000));
    let (result, _) = ask(&engine, &repo, "Look at the source", &mut keeps_working());

    let spent = usage_of(&engine, &result);
    assert!(spent > 0, "the turn should have recorded what it spent");
    assert!(
        result.response.contains("1000"),
        "the ceiling should be named: {}",
        result.response
    );
    assert!(
        result.response.contains(&spent.to_string()),
        "the spend ({spent}) should be named: {}",
        result.response
    );
}

#[test]
fn a_ceiling_the_turn_never_approaches_changes_nothing() {
    // The complement of the stop tests. Without this, a ceiling that fired
    // unconditionally would pass every assertion above.
    let repo = temp_repo("roomy");
    let engine = engine_with_ceiling(&repo, Some(10_000_000));
    let (result, calls) = ask(&engine, &repo, "Look at the source", &mut keeps_working());

    assert_ne!(result.task.status, TaskStatus::TokenBudgetExhausted);
    assert!(calls > 2, "got {calls} calls");
}
