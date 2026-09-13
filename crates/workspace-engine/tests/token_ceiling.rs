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

#[test]
fn raising_the_ceiling_and_asking_again_resumes_the_plan() {
    // §5.4: "the plan survives, so the user can raise the ceiling and resume,
    // and the remaining steps are what resumption starts from." A task is one
    // turn (`context.md` §3.1), so the second turn is a *new* task and this is
    // only true if something carries the plan across.
    let repo = temp_repo("resume");
    let engine = engine_with_ceiling(&repo, Some(1_000));
    // Plans first, then keeps working. The mock repeats its last response once
    // the sequence is exhausted, so the command envelope runs until the ceiling
    // stops it — which is the situation §5.4 describes.
    let mut planner = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            "DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look again.\nEND_COMMAND\n".to_string(),
        ],
        vec![
            vec![workspace_engine::ToolCall {
                id: "c1".to_string(),
                name: "propose_plan".to_string(),
                arguments_json: r#"{"steps":[{"title":"Look around"},{"title":"Report back"}]}"#
                    .to_string(),
            }],
            Vec::new(),
        ],
    );
    let (stopped, _) = ask(&engine, &repo, "Look at the source", &mut planner);
    assert_eq!(stopped.task.status, TaskStatus::TokenBudgetExhausted);
    let first_plan = engine
        .session_store
        .read_task_plan(&stopped.session.id, &stopped.task.id)
        .unwrap()
        .expect("the stopped turn had a plan");
    assert_eq!(first_plan.steps.len(), 2);

    // Raise it and ask again in the same session.
    let roomy = engine_with_ceiling(&repo, Some(10_000_000));
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    let resumed = roomy
        .chat_orchestrator
        .ask_with_session(
            &repo,
            "Carry on",
            &[],
            Some(&stopped.session.id),
            &mut MockModelAdapter::new("Carrying on."),
            &mut sink,
        )
        .expect("the resumed turn should run");

    let carried = roomy
        .session_store
        .read_task_plan(&resumed.session.id, &resumed.task.id)
        .unwrap()
        .expect("the plan carried onto the resumed task");
    assert_eq!(carried.steps.len(), 2);
    assert_eq!(carried.steps[0].title, first_plan.steps[0].title);
    assert_ne!(resumed.task.id, stopped.task.id, "resuming is a new task");
}

#[test]
fn a_turn_after_a_completed_task_starts_with_no_plan() {
    // The complement. Carrying a plan into an unrelated next question would put
    // steps on the panel the user never asked for, so the carry is narrow: only
    // a ceiling stop, only with work outstanding.
    let repo = temp_repo("no-carry");
    let engine = engine_with_ceiling(&repo, None);
    let mut planner = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Done.".to_string()],
        vec![
            vec![workspace_engine::ToolCall {
                id: "c1".to_string(),
                name: "propose_plan".to_string(),
                arguments_json: r#"{"steps":[{"title":"One"},{"title":"Two"}]}"#.to_string(),
            }],
            Vec::new(),
        ],
    );
    let (first, _) = ask(&engine, &repo, "Do the thing", &mut planner);
    assert_eq!(first.task.status, TaskStatus::Complete);

    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    let second = engine
        .chat_orchestrator
        .ask_with_session(
            &repo,
            "Unrelated question",
            &[],
            Some(&first.session.id),
            &mut MockModelAdapter::new("An answer."),
            &mut sink,
        )
        .expect("the second turn should run");

    assert!(
        engine
            .session_store
            .read_task_plan(&second.session.id, &second.task.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn completing_a_carried_step_keeps_evidence_the_turn_never_saw() {
    // Task 4b predicted this and it was right: `complete_step` assigning
    // `open.evidence = accrued` silently drops anything recorded outside the
    // current turn. A patch applied *after* the turn that proposed it appends
    // through the log, and a resumed plan arrives carrying it — so the step
    // would be judged on this turn's accrual alone, and a verified step would
    // come out unverified.
    let repo = temp_repo("carried-evidence");
    let engine = engine_with_ceiling(&repo, Some(1_000));
    let mut planner = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            "DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look again.\nEND_COMMAND\n".to_string(),
        ],
        vec![
            vec![workspace_engine::ToolCall {
                id: "c1".to_string(),
                name: "propose_plan".to_string(),
                arguments_json: r#"{"steps":[{"title":"Edit it"},{"title":"Report"}]}"#.to_string(),
            }],
            Vec::new(),
        ],
    );
    let (stopped, _) = ask(&engine, &repo, "Edit the source", &mut planner);
    assert_eq!(stopped.task.status, TaskStatus::TokenBudgetExhausted);

    // What `edit.rs` does when the patch this turn proposed is applied later.
    engine
        .session_store
        .append_step_evidence(
            &stopped.session.id,
            &stopped.task.id,
            workspace_engine::plan::Evidence::PatchApplied {
                marker_id: "action_1".to_string(),
                files: vec![workspace_engine::plan::PatchedFile {
                    path: "src/a.rs".to_string(),
                    applied_hash: "abc".to_string(),
                }],
            },
        )
        .unwrap();

    // Raise the ceiling, resume, and close the step out.
    let roomy = engine_with_ceiling(&repo, Some(10_000_000));
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    let resumed = roomy
        .chat_orchestrator
        .ask_with_session(
            &repo,
            "Carry on",
            &[],
            Some(&stopped.session.id),
            &mut MockModelAdapter::new_sequence_with_tool_calls(
                vec![String::new(), "Done.".to_string()],
                vec![
                    vec![workspace_engine::ToolCall {
                        id: "c2".to_string(),
                        name: "complete_step".to_string(),
                        arguments_json: "{}".to_string(),
                    }],
                    Vec::new(),
                ],
            ),
            &mut sink,
        )
        .expect("the resumed turn should run");

    let plan = roomy
        .session_store
        .read_task_plan(&resumed.session.id, &resumed.task.id)
        .unwrap()
        .expect("the plan carried onto the resumed task");
    assert_eq!(
        plan.steps[0].evidence.len(),
        1,
        "the applied patch must still be behind this step"
    );
    assert!(
        !plan.steps[0].is_unverified(),
        "a step with an applied patch behind it is not unverified"
    );
}
