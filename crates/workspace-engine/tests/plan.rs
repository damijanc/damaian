//! The plan a turn works through, per
//! `docs/specs/21_task_plan_progress_and_budget/proposal.md`.
//!
//! The rule these tests exist to hold is requirement 6: a step is never marked
//! complete only because the model said so. Every assertion about `Evidence`
//! and about `status_from_evidence` is an assertion about that guarantee.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::{Evidence, PlanStep, StepStatus, TaskPlan, status_from_evidence};
use workspace_engine::{SessionStore, Task};

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// A step with nothing but an id and a status, so a test that cares about one
/// field is not obscured by six it does not.
fn step(id: &str, status: StepStatus) -> PlanStep {
    PlanStep {
        id: id.to_string(),
        title: format!("Step {id}"),
        detail: None,
        status,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
    }
}

struct Fixture {
    data_dir: PathBuf,
    store: SessionStore,
    session_id: String,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!(
            "damaian-plan-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&data_dir).expect("temp dir should be created");
        let store = SessionStore::new(&data_dir);
        let session = store
            .create_session("repo_1", "Plan")
            .expect("a session should be created");
        Self {
            data_dir,
            store,
            session_id: session.id,
        }
    }

    fn task(&self, prompt: &str) -> Task {
        self.store
            .create_task(&self.session_id, prompt, "mock", "m")
            .expect("a task should be created")
    }

    fn log_path(&self) -> PathBuf {
        self.data_dir
            .join("sessions")
            .join(format!("{}.jsonl", self.session_id))
    }

    /// Appends a raw line, so a test can write what a crash mid-write leaves
    /// rather than describe it.
    fn append_raw(&self, line: &str) {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(self.log_path())
            .expect("the log should exist");
        writeln!(file, "{line}").expect("append should succeed");
    }

    fn plan_event_kinds(&self) -> Vec<String> {
        let text = fs::read_to_string(self.log_path()).unwrap_or_default();
        text.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|event| {
                event
                    .get("eventType")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .filter(|kind| kind.starts_with("plan_"))
            .collect()
    }
}

#[test]
fn a_plan_round_trips_through_json_unchanged() {
    let plan = TaskPlan {
        task_id: "task_1".to_string(),
        created_at_ms: 1_700_000_000_000,
        steps: vec![PlanStep {
            id: "step_1".to_string(),
            title: "Add the retry helper".to_string(),
            detail: Some("with a bounded backoff".to_string()),
            status: StepStatus::Completed,
            depends_on: Vec::new(),
            started_at_ms: Some(1),
            completed_at_ms: Some(2),
            evidence: vec![Evidence::CommandExit {
                marker_id: "action_1".to_string(),
                exit_code: Some(0),
            }],
        }],
    };

    let text = serde_json::to_string(&plan).expect("a plan serializes");
    assert_eq!(
        serde_json::from_str::<TaskPlan>(&text).expect("and reads back"),
        plan
    );

    // camelCase on the wire: these events go into the session log and out to
    // the shell, which reads them directly rather than through a translation.
    assert!(text.contains("\"exitCode\":0"), "got: {text}");
    assert!(text.contains("\"markerId\":\"action_1\""), "got: {text}");
    assert!(text.contains("\"kind\":\"commandExit\""), "got: {text}");
    assert!(text.contains("\"status\":\"completed\""), "got: {text}");
}

#[test]
fn an_absent_exit_code_survives_the_round_trip_as_absent() {
    // Not as a zero. A killed command reports no code, and a serializer that
    // defaulted it would launder "nothing is known" into "it passed" at the
    // one boundary nobody inspects.
    let evidence = Evidence::CommandExit {
        marker_id: "action_1".to_string(),
        exit_code: None,
    };
    let text = serde_json::to_string(&evidence).expect("evidence serializes");
    assert_eq!(
        serde_json::from_str::<Evidence>(&text).expect("and reads back"),
        evidence
    );
    assert!(!text.contains("\"exitCode\":0"), "got: {text}");
}

#[test]
fn only_one_step_may_be_in_progress() {
    // Requirement 2, as a predicate the panel and the tests can both ask of a
    // replayed plan — which is where a violation would actually show up.
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    assert!(!plan.violates_single_in_progress());

    plan.steps.push(step("step_2", StepStatus::InProgress));
    assert!(plan.violates_single_in_progress());
}

#[test]
fn a_plan_with_no_step_in_progress_is_not_a_violation() {
    // A plan before it starts and a plan after it finishes both have zero, and
    // neither is the bug requirement 2 describes.
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    plan.steps.push(step("step_2", StepStatus::Completed));
    assert!(!plan.violates_single_in_progress());
}

// ---------------------------------------------------------------------------
// Persistence: appended, replayed. Proposal §5.2.
// ---------------------------------------------------------------------------

#[test]
fn the_newest_event_per_step_wins_on_replay() {
    let fixture = Fixture::new("replay");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    plan.steps.push(step("step_2", StepStatus::Pending));
    fixture.store.create_plan(&task, &plan).unwrap();

    let mut first = plan.steps[0].clone();
    first.status = StepStatus::InProgress;
    fixture.store.update_plan_step(&task, &first).unwrap();
    first.status = StepStatus::Completed;
    first.evidence = vec![Evidence::CommandExit {
        marker_id: "action_1".to_string(),
        exit_code: Some(0),
    }];
    fixture.store.update_plan_step(&task, &first).unwrap();

    let replayed = fixture
        .store
        .read_task_plan(&fixture.session_id, &task.id)
        .unwrap()
        .expect("a plan was created");
    assert_eq!(replayed.steps[0].status, StepStatus::Completed);
    assert_eq!(replayed.steps[0].evidence.len(), 1);
    // Untouched, and still in its original position: a fold that rebuilt the
    // list from the updates would lose the order the plan was written in.
    assert_eq!(replayed.steps[1].status, StepStatus::Pending);
    assert_eq!(replayed.steps[1].id, "step_2");
}

#[test]
fn a_plan_from_another_task_in_the_same_session_is_not_returned() {
    let fixture = Fixture::new("other-task");
    let first = fixture.task("one");
    let second = fixture.task("two");
    let mut plan = TaskPlan::new(&first.id, 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    fixture.store.create_plan(&first, &plan).unwrap();

    assert!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &second.id)
            .unwrap()
            .is_none(),
        "a session holds every task's plan; the reader must select one"
    );
}

#[test]
fn a_torn_final_line_does_not_discard_the_plan_before_it() {
    let fixture = Fixture::new("torn");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    fixture.store.create_plan(&task, &plan).unwrap();

    // What a crash mid-append leaves. The log is append-only and a plan is
    // written during a turn, so this is the expected shape of a bad shutdown,
    // not a corruption to refuse.
    fixture.append_raw("{\"seq\":99,\"eventType\":\"plan_step_upda");

    assert!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &task.id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn an_update_naming_a_step_the_plan_does_not_have_is_ignored() {
    // The step list is set by `plan_created` and `plan_revised`. Letting an
    // update introduce one would let the log grow a plan nobody wrote — and a
    // torn line that happened to parse could then add a step.
    let fixture = Fixture::new("unknown-step");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    fixture.store.create_plan(&task, &plan).unwrap();

    fixture
        .store
        .update_plan_step(&task, &step("step_99", StepStatus::Completed))
        .unwrap();

    let replayed = fixture
        .store
        .read_task_plan(&fixture.session_id, &task.id)
        .unwrap()
        .expect("a plan was created");
    assert_eq!(replayed.steps.len(), 1);
    assert_eq!(replayed.steps[0].id, "step_1");
}

#[test]
fn a_task_with_no_plan_reports_none_rather_than_an_empty_plan() {
    // A trivial turn gets no plan at all (§5.1), which is a different fact
    // from a plan with no steps. The completion report has to tell them apart:
    // one has nothing to say, the other proposed nothing.
    let fixture = Fixture::new("no-plan");
    let task = fixture.task("what does this function do");

    assert!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &task.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_plan_event_records_its_kind_so_the_log_is_readable_by_hand() {
    // `docs/TROUBLESHOOTING.md` tells a user to find plan events in the log.
    let fixture = Fixture::new("kinds");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    fixture.store.create_plan(&task, &plan).unwrap();
    fixture
        .store
        .update_plan_step(&task, &step("step_1", StepStatus::InProgress))
        .unwrap();

    assert_eq!(
        fixture.plan_event_kinds(),
        vec!["plan_created", "plan_step_updated"]
    );
}

#[test]
fn a_rewind_past_a_plan_takes_the_plan_with_it() {
    // `read_task_plan` reads *active* events, unlike `read_task_usage`, and
    // this is the test for that choice rather than only a doc comment about
    // it. A plan is part of the conversation: rewinding to before it was
    // proposed must not leave the panel showing steps the user rewound away.
    // (Usage is the opposite case — what was billed was billed regardless of
    // where the conversation now sits.)
    let fixture = Fixture::new("rewind");
    let task = fixture.task("add retry handling");
    let before = fixture
        .store
        .latest_event_seq(&fixture.session_id)
        .expect("a seq");

    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    fixture.store.create_plan(&task, &plan).unwrap();
    assert!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &task.id)
            .unwrap()
            .is_some(),
        "the plan should be readable before the rewind"
    );

    fixture
        .store
        .rewind_conversation(&fixture.session_id, before)
        .unwrap();

    assert!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &task.id)
            .unwrap()
            .is_none(),
        "a rewound plan must not survive as the task's current plan"
    );
}

// ---------------------------------------------------------------------------
// A step's status is a function of its evidence. Proposal §5.3.
//
// The model does not appear in any of these inputs, which is the point.
// ---------------------------------------------------------------------------

fn command_exit(exit_code: Option<i32>) -> Evidence {
    Evidence::CommandExit {
        marker_id: "action_1".to_string(),
        exit_code,
    }
}

fn completed_step() -> PlanStep {
    let mut step = step("step_1", StepStatus::Completed);
    step.completed_at_ms = Some(2);
    step
}

#[test]
fn a_clean_exit_completes_the_step() {
    assert_eq!(
        status_from_evidence(&[command_exit(Some(0))]),
        StepStatus::Completed
    );
}

#[test]
fn a_non_zero_exit_blocks_the_step() {
    assert_eq!(
        status_from_evidence(&[command_exit(Some(1))]),
        StepStatus::Blocked
    );
}

#[test]
fn an_absent_exit_code_blocks_the_step() {
    // The `unwrap_or(0)` failure mode, asserted directly per §6. A killed or
    // signalled command reported nothing, and nothing is not success.
    assert_eq!(
        status_from_evidence(&[command_exit(None)]),
        StepStatus::Blocked
    );
}

#[test]
fn one_failure_among_successes_still_blocks() {
    // A step is not done because most of its checks passed.
    assert_eq!(
        status_from_evidence(&[command_exit(Some(0)), command_exit(Some(1))]),
        StepStatus::Blocked
    );
    assert_eq!(
        status_from_evidence(&[command_exit(Some(1)), command_exit(Some(0))]),
        StepStatus::Blocked
    );
}

#[test]
fn no_evidence_completes_the_step_but_marks_it_unverified() {
    // Requirement 6's second sentence. A step like "understand the existing
    // retry logic" genuinely has nothing observable behind it, and inventing a
    // fake observation would be worse than admitting the gap.
    assert_eq!(status_from_evidence(&[]), StepStatus::Completed);
    assert!(completed_step().is_unverified());
}

#[test]
fn a_step_with_evidence_is_not_unverified() {
    let mut step = completed_step();
    step.evidence = vec![command_exit(Some(0))];
    assert!(!step.is_unverified());
}

#[test]
fn a_blocked_step_is_not_reported_as_unverified() {
    // "Unverified" means completed with nothing behind it. A blocked step is
    // not completed at all, and reporting it as completed-unverified would
    // turn a failure into a soft pass — the exact laundering §5.3 forbids.
    let mut step = step("step_1", StepStatus::Blocked);
    step.evidence = Vec::new();
    assert!(!step.is_unverified());
}

#[test]
fn a_patch_applied_completes_the_step() {
    // Applying a patch is something Damaian observed itself, and there is no
    // failure mode to encode: a patch that did not apply returns an error and
    // produces no evidence at all.
    let evidence = Evidence::PatchApplied {
        marker_id: "action_1".to_string(),
        files: vec![workspace_engine::plan::PatchedFile {
            path: "src/upload.rs".to_string(),
            applied_hash: "abc".to_string(),
        }],
    };
    assert_eq!(status_from_evidence(&[evidence]), StepStatus::Completed);
}
