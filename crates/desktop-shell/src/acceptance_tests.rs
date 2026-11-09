//! Spec 106 Task 4: the deterministic tier of the single-worker acceptance
//! gate (`context.md` D3).
//!
//! A scripted model drives a real worker host in this process, against a fresh
//! copy of the `single_worker_spec` fixture, through the shared acceptance
//! driver. Decisions go through the worker's own control-socket parser, patches
//! are applied the way the shell applies them, and success is judged from
//! durable records by the shared criteria. This proves the state machine end to
//! end; it is not evidence that a model chooses the right tools.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eval_harness::acceptance::AcceptanceFixture;
use eval_harness::acceptance::criteria::{CriteriaInput, ProtectedSnapshot, evaluate};
use eval_harness::acceptance::driver::{DEFAULT_MAX_DECISIONS, DriveOutcome, drive};
use eval_harness::acceptance::policy::Policy;
use eval_harness::acceptance::report::{AcceptanceReport, RunMetadata};
use eval_harness::acceptance::worker::{WorkerControl, WorkerTiming, WorkerTransport};
use eval_harness::fixture;
use serde_json::{Value, json};
use workspace_engine::{
    CancelToken, Config, MockModelAdapter, ModelAdapter, ModelRequest, ModelRun, SessionStore,
    ToolCall, WorkerRecord,
};

use crate::worker::{WorkerHost, dispatch_line, host_in_process};

const FIXTURE: &str = "single_worker_spec";
const TOKEN: &str = "acceptance-deterministic";

/// Hands each control line to the in-process host through the socket parser.
struct InProcess {
    host: Arc<WorkerHost>,
}

impl WorkerControl for InProcess {
    fn request(&mut self, mut request: Value) -> workspace_engine::Result<Value> {
        request["token"] = json!(TOKEN);
        Ok(dispatch_line(&self.host, &request.to_string(), TOKEN))
    }
}

/// The worker builds an adapter per job; one shared script makes a turn and its
/// resumes read one sequence.
#[derive(Clone)]
struct Scripted(Arc<Mutex<MockModelAdapter>>);

impl ModelAdapter for Scripted {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        self.0
            .lock()
            .unwrap()
            .stream_response(request, cancel, on_token, on_wait)
    }
}

fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments.to_string(),
    }
}

fn patch(summary: &str, path: &str, content: &str) -> ToolCall {
    call(
        "propose_patch",
        json!({"summary": summary, "files": [{"path": path, "content": content}]}),
    )
}

const PRICING_ROUNDED: &str = "//! Money helpers.
//!
//! Amounts are integer mills: thousandths of the currency unit, so `2349` is
//! 2.349. Integers keep every conversion exact.

/// Converts an amount in mills to whole cents, rounding to the nearest cent.
///
/// An amount exactly halfway between two cents rounds to the even cent, as
/// finance confirmed: 2345 mills (2.345) becomes 234 cents (2.34).
pub fn to_cents(mills: i64) -> i64 {
    let whole = mills / 10;
    let rest = mills % 10;
    if rest > 5 || (rest == 5 && whole % 2 == 1) {
        whole + 1
    } else {
        whole
    }
}

#[cfg(test)]
mod tests {
    use super::to_cents;

    #[test]
    fn whole_cents_are_unchanged() {
        assert_eq!(to_cents(2340), 234);
    }

    #[test]
    fn an_amount_above_halfway_rounds_up() {
        assert_eq!(to_cents(2349), 235);
    }

    #[test]
    fn an_amount_below_halfway_rounds_down() {
        assert_eq!(to_cents(2341), 234);
    }

    #[test]
    fn a_tie_rounds_to_the_even_cent() {
        assert_eq!(to_cents(2345), 234);
        assert_eq!(to_cents(2355), 236);
    }
}
";

const INVOICE_REPAIRED: &str = "//! Invoice lines.
//!
//! A line total is the quantity times the unit price in mills, converted to
//! cents by `pricing::to_cents`, so invoices always follow the pricing rule.

/// The total of one invoice line, in cents.
pub fn line_total(quantity: i64, unit_price_mills: i64) -> i64 {
    pricing::to_cents(quantity * unit_price_mills)
}

/// The total of several lines, in cents. Each line is converted on its own,
/// then the cents are added.
pub fn invoice_total(lines: &[(i64, i64)]) -> i64 {
    lines
        .iter()
        .map(|(quantity, unit_price_mills)| line_total(*quantity, *unit_price_mills))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::{invoice_total, line_total};

    #[test]
    fn a_line_of_three_items_in_cents() {
        // 3 x 1.236 = 3.708, which rounds to 3.71.
        assert_eq!(line_total(3, 1236), 371);
    }

    #[test]
    fn an_invoice_adds_its_lines() {
        assert_eq!(invoice_total(&[(1, 1000), (2, 2500)]), 600);
    }
}
";

const TASKS_DONE: &str = "# Feature 01 tasks: rounding

| Task | State | What landed |
|---|---|---|
| 1 · Round amounts to the nearest cent | Done | `pricing::to_cents` rounds to the nearest cent, ties to the even cent; pricing and invoice tests updated. Checks: `cargo test -p pricing -p invoice` passed. |

## Task 1 · Round amounts to the nearest cent

- [x] Change `pricing::to_cents` to round to the nearest cent (requirement 1),
      with the halfway rule confirmed (requirement 4).
- [x] Add the `pricing` tests the acceptance section lists.
- [x] Update the `invoice` tests to the new rounding (requirement 2).
- [x] Run the scoped checks from `AGENTS.md` and update this task's row.
";

/// The scripted worker. It explores, asks the material clarification, plans
/// three steps, changes `pricing` (which breaks `invoice` without noticing),
/// records the task, runs its planned check, answers, and then repairs the
/// regression the completion floor finds.
fn script() -> MockModelAdapter {
    let rounds: Vec<(String, Vec<ToolCall>)> = vec![
        (
            String::new(),
            vec![call("read_file", json!({"path": "AGENTS.md"}))],
        ),
        (
            String::new(),
            vec![call(
                "read_file",
                json!({"path": "docs/specs/01_rounding/proposal.md"}),
            )],
        ),
        (
            String::new(),
            vec![call(
                "read_file",
                json!({"path": "crates/pricing/src/lib.rs"}),
            )],
        ),
        (
            String::new(),
            vec![call(
                "ask_user",
                json!({
                    "question": "Requirement 4 leaves ties open. How should an amount exactly halfway between two cents, such as 2.345, round?",
                    "options": ["Round half up (2.35)", "Ask finance first"],
                    "allowFreeText": true
                }),
            )],
        ),
        (
            String::new(),
            vec![call(
                "propose_plan",
                json!({"steps": [
                    {"title": "Round to the nearest cent in pricing"},
                    {"title": "Record the task as done"},
                    {"title": "Run the pricing checks", "check": {
                        "command": "cargo test -p pricing",
                        "workingDirectory": "",
                        "purpose": "validates the rounding change",
                        "required": true
                    }}
                ]}),
            )],
        ),
        // The first patch is held by the plan review. Approval does not
        // replay it: the engine asks the model again, which proposes it anew.
        (
            String::new(),
            vec![patch(
                "Round to the nearest cent, ties to even",
                "crates/pricing/src/lib.rs",
                PRICING_ROUNDED,
            )],
        ),
        (
            String::new(),
            vec![patch(
                "Round to the nearest cent, ties to even",
                "crates/pricing/src/lib.rs",
                PRICING_ROUNDED,
            )],
        ),
        (String::new(), vec![call("complete_step", json!({}))]),
        (
            String::new(),
            vec![patch(
                "Record task 1 as done",
                "docs/specs/01_rounding/tasks.md",
                TASKS_DONE,
            )],
        ),
        (String::new(), vec![call("complete_step", json!({}))]),
        (
            String::new(),
            vec![call(
                "run_command",
                json!({"command": "cargo test -p pricing", "reason": "Run the planned check"}),
            )],
        ),
        (String::new(), vec![call("complete_step", json!({}))]),
        ("Task 1 is done.".to_string(), Vec::new()),
        (
            String::new(),
            vec![patch(
                "Update the invoice test to the new rounding",
                "crates/invoice/src/lib.rs",
                INVOICE_REPAIRED,
            )],
        ),
    ];
    let mut texts: Vec<String> = rounds.iter().map(|(text, _)| text.clone()).collect();
    let mut calls: Vec<Vec<ToolCall>> = rounds.into_iter().map(|(_, calls)| calls).collect();
    for _ in 0..8 {
        texts.push("Task 1 is done.".to_string());
        calls.push(Vec::new());
    }
    MockModelAdapter::new_sequence_with_tool_calls(texts, calls)
}

/// Small segments, so the run crosses an internal segment boundary (spec 103)
/// without the user.
const WORKER_CONFIG: &str = "agent_max_tool_rounds=4
agent_max_exploration_rounds=24
worker_task_budget_action_rounds=24
worker_task_budget_exploration_rounds=24
";

fn worker_session(store: &SessionStore, repo: &Path) -> String {
    let session = store
        .create_session(
            &workspace_engine::hash::repository_id_for_root(repo),
            "acceptance",
        )
        .unwrap();
    store
        .record_worker_spawned(
            &session.id,
            &WorkerRecord {
                parent_session_id: None,
                parent_task_id: None,
                inherited_protected_paths: Vec::new(),
                source_repository: repo.to_path_buf(),
                worktree_path: repo.to_path_buf(),
                branch: "damaian/acceptance".to_string(),
                base_commit: "HEAD".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .unwrap();
    session.id
}

struct Run {
    outcome: DriveOutcome,
    report: AcceptanceReport,
    /// Where the run's evidence is.
    evidence: String,
    directories: Vec<std::path::PathBuf>,
}

fn run(script: MockModelAdapter, prompt: &str) -> Run {
    let acceptance = AcceptanceFixture::load(FIXTURE).unwrap();
    let materialized = fixture::materialize(FIXTURE).unwrap();
    let repo = fs::canonicalize(&materialized.root).unwrap();
    let data_dir = materialized.data_dir.clone();
    fs::create_dir_all(data_dir.join("config")).unwrap();
    fs::write(
        Config {
            data_dir: data_dir.clone(),
            ..Config::default()
        }
        .user_config_path(),
        WORKER_CONFIG,
    )
    .unwrap();
    let store = SessionStore::new(&data_dir);
    let session = worker_session(&store, &repo);
    let protected_before = ProtectedSnapshot::take(&repo, &acceptance.protected_paths);

    let adapter = Scripted(Arc::new(Mutex::new(script)));
    let host = host_in_process(
        &session,
        &data_dir,
        Box::new(move |_config: &Config| Ok(Box::new(adapter.clone()) as Box<dyn ModelAdapter>)),
    )
    .unwrap();
    let mut control = InProcess {
        host: Arc::clone(&host),
    };
    // The task itself, as a spawned worker receives it. Not an intervention.
    let started = control
        .request(json!({"command": "message", "text": prompt}))
        .unwrap();
    assert_ne!(started["ok"], json!(false), "{started}");

    let clock = Instant::now();
    let mut transport = WorkerTransport::new(
        control,
        &data_dir,
        &session,
        &repo,
        WorkerTiming::deterministic(),
    )
    .unwrap();
    let policy = Policy::new(acceptance.clone());
    let outcome = drive(&mut transport, &policy, DEFAULT_MAX_DECISIONS);
    let criteria = evaluate(&CriteriaInput {
        store: &store,
        session_id: &session,
        repository_root: &repo,
        acceptance: &acceptance,
        outcome: &outcome,
        protected_before: &protected_before,
    })
    .unwrap();
    let report = AcceptanceReport::build(
        RunMetadata {
            run_id: format!("deterministic-{session}"),
            tier: "deterministic".to_string(),
            fixture: FIXTURE.to_string(),
            provider: None,
            model: None,
            duration_ms: clock.elapsed().as_millis(),
        },
        &outcome,
        criteria,
        &store,
        &session,
    )
    .unwrap();
    report.write(&data_dir).unwrap();
    let _ = transport
        .control_mut()
        .request(json!({"command": "shutdown"}));
    Run {
        outcome,
        report,
        evidence: format!("repository {} data {}", repo.display(), data_dir.display()),
        directories: vec![materialized.root.clone(), data_dir],
    }
}

impl Run {
    /// Remove the run's repository and data directory. The scripted success
    /// run keeps them when it fails, as evidence; guard runs always clean up.
    fn clean_up(&self) {
        for directory in &self.directories {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

/// Requirement 5: the full state machine, end to end, through the worker host.
#[test]
fn the_scripted_worker_finishes_the_fixture_spec_through_the_worker_host() {
    let acceptance = AcceptanceFixture::load(FIXTURE).unwrap();
    let run = run(script(), &acceptance.prompt);
    let report = &run.report;
    if report.passed {
        run.clean_up();
    }
    assert!(
        report.passed,
        "the scripted run must pass every criterion ({}):\n{}\n{:#?}",
        run.evidence,
        report.return_summary(),
        run.outcome
    );
    assert!(
        report.regression_observed,
        "the completion floor must have found the invoice regression: {:?}",
        report.metrics
    );
    assert!(
        report.metrics.repair_attempts >= 1,
        "the regression is repaired by a repair turn: {:?}",
        report.metrics
    );
    assert!(
        report.metrics.segments_started >= 2,
        "the run crosses an internal segment boundary: {:?}",
        report.metrics
    );
    assert!(report.deviations.is_empty(), "{:?}", report.deviations);
    assert!(report.metrics.decisions_by_kind["plan_review"] == 1);
    assert!(report.metrics.decisions_by_kind["question"] == 1);
    assert!(report.metrics.decisions_by_kind["patch"] >= 3);
}

/// The worker explores, asks and plans, as in [`script`], then holds and
/// re-proposes `second_patch` after the plan review.
fn script_after_plan(
    second_patch: ToolCall,
    tail: Vec<(String, Vec<ToolCall>)>,
) -> MockModelAdapter {
    let mut rounds: Vec<(String, Vec<ToolCall>)> = vec![
        (
            String::new(),
            vec![call("read_file", json!({"path": "AGENTS.md"}))],
        ),
        (
            String::new(),
            vec![call(
                "ask_user",
                json!({
                    "question": "How should an amount exactly halfway between two cents, such as 2.345, round?",
                    "options": ["Round half up (2.35)", "Ask finance first"],
                    "allowFreeText": true
                }),
            )],
        ),
        (
            String::new(),
            vec![call(
                "propose_plan",
                json!({"steps": [
                    {"title": "Round to the nearest cent in pricing"},
                    {"title": "Record the task as done"}
                ]}),
            )],
        ),
        (String::new(), vec![second_patch.clone()]),
        (String::new(), vec![second_patch]),
    ];
    rounds.extend(tail);
    let mut texts: Vec<String> = rounds.iter().map(|(text, _)| text.clone()).collect();
    let mut calls: Vec<Vec<ToolCall>> = rounds.into_iter().map(|(_, calls)| calls).collect();
    for _ in 0..8 {
        texts.push("Task 1 is done.".to_string());
        calls.push(Vec::new());
    }
    MockModelAdapter::new_sequence_with_tool_calls(texts, calls)
}

/// Requirement 3: a proposal touching the protected path fails the run, and
/// the protected file is never written.
#[test]
fn a_protected_path_proposal_fails_the_acceptance_run() {
    let acceptance = AcceptanceFixture::load(FIXTURE).unwrap();
    let run = run(
        script_after_plan(
            patch(
                "Make the legacy test pass",
                "crates/legacy/src/lib.rs",
                "pub fn parse_code(code: &str) -> Option<u32> {\n    code.trim_start_matches(\"LG-\").parse().ok()\n}\n",
            ),
            Vec::new(),
        ),
        &acceptance.prompt,
    );
    run.clean_up();
    let report = &run.report;
    assert!(!report.passed, "{}", report.return_summary());
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("protected path")),
        "{:?}",
        report.failure
    );
    let protected = report
        .criteria
        .iter()
        .find(|criterion| criterion.name == "protected_paths_untouched")
        .unwrap();
    assert!(!protected.passed, "{protected:?}");
    assert!(
        protected.observed.contains("changed on disk []"),
        "the rejected proposal never reached the file: {protected:?}"
    );
}

/// Requirement 7: a worker that stops with plan work open would need Continue
/// task, which the driver cannot send; the run fails and says why.
#[test]
fn a_worker_that_stops_with_work_open_fails_instead_of_being_continued() {
    let acceptance = AcceptanceFixture::load(FIXTURE).unwrap();
    // After the plan review the model only answers, never re-proposing its
    // held patch, so every plan step stays open.
    let stalled = {
        let rounds: Vec<(String, Vec<ToolCall>)> = vec![
            (
                String::new(),
                vec![call("read_file", json!({"path": "AGENTS.md"}))],
            ),
            (
                String::new(),
                vec![call(
                    "propose_plan",
                    json!({"steps": [
                        {"title": "Round to the nearest cent in pricing"},
                        {"title": "Record the task as done"}
                    ]}),
                )],
            ),
            (
                String::new(),
                vec![patch(
                    "Round to the nearest cent, ties to even",
                    "crates/pricing/src/lib.rs",
                    PRICING_ROUNDED,
                )],
            ),
        ];
        let mut texts: Vec<String> = rounds.iter().map(|(text, _)| text.clone()).collect();
        let mut calls: Vec<Vec<ToolCall>> = rounds.into_iter().map(|(_, calls)| calls).collect();
        for _ in 0..8 {
            texts.push("I will finish this later.".to_string());
            calls.push(Vec::new());
        }
        MockModelAdapter::new_sequence_with_tool_calls(texts, calls)
    };
    let run = run(stalled, &acceptance.prompt);
    run.clean_up();
    let report = &run.report;
    assert!(!report.passed, "{}", report.return_summary());
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("forbidden intervention")),
        "{:?}",
        report.failure
    );
    assert!(
        report.return_summary().contains("Continue task"),
        "the main session is told what would have been needed: {}",
        report.return_summary()
    );
}
