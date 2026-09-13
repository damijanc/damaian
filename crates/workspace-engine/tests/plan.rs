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

use workspace_engine::plan::{
    Evidence, PlanStep, StepOutcome, StepStatus, TaskPhase, TaskPlan, status_from_evidence,
};
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
// The review gate. Proposal §5.5.
// ---------------------------------------------------------------------------

#[test]
fn a_revision_keeps_the_original_plan_in_the_log() {
    // §5.5: both the original and the user's revision are in the log, so the
    // history still describes what was proposed as well as what was run. A
    // revision that overwrote the original would leave no way to see that the
    // user removed a step — which is exactly the fact a reviewer wants.
    let fixture = Fixture::new("revision");
    let task = fixture.task("add retry handling");
    let mut original = TaskPlan::new(&task.id, 0);
    original.steps.push(step("step_1", StepStatus::InProgress));
    original.steps.push(step("step_2", StepStatus::Pending));
    fixture.store.create_plan(&task, &original).unwrap();

    let mut revised = original.clone();
    revised.steps.remove(1);
    fixture.store.revise_plan(&task, &revised).unwrap();

    assert_eq!(
        fixture.plan_event_kinds(),
        vec!["plan_created", "plan_revised"]
    );
    assert_eq!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &task.id)
            .unwrap()
            .unwrap()
            .steps
            .len(),
        1
    );
}

#[test]
fn a_task_whose_plan_was_never_approved_reads_as_unapproved() {
    // The default has to be "not approved" rather than "unknown": the gate
    // reads this to decide whether to pause, and an unknown that fell through
    // to `true` would let the first mutating step run unreviewed.
    let fixture = Fixture::new("unapproved");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    fixture.store.create_plan(&task, &plan).unwrap();

    assert!(
        !fixture
            .store
            .read_plan_approved(&fixture.session_id, &task.id)
            .unwrap()
    );
}

#[test]
fn an_approval_outlives_the_turn_that_recorded_it() {
    // The approval is a fact in the log, not turn-local state, so a restart
    // between the decision and the work does not re-ask.
    let fixture = Fixture::new("approved");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    fixture.store.create_plan(&task, &plan).unwrap();
    fixture.store.approve_plan(&task, "tester").unwrap();

    let reread = SessionStore::new(&fixture.data_dir);
    assert!(
        reread
            .read_plan_approved(&fixture.session_id, &task.id)
            .unwrap()
    );
}

#[test]
fn an_approval_belongs_to_the_task_that_earned_it() {
    // Approving one task's plan must not clear the gate for another task in
    // the same session — the user reviewed those steps, not these.
    let fixture = Fixture::new("approval-scope");
    let approved = fixture.task("add retry handling");
    let other = fixture.task("rewrite the parser");
    fixture.store.approve_plan(&approved, "tester").unwrap();

    assert!(
        fixture
            .store
            .read_plan_approved(&fixture.session_id, &approved.id)
            .unwrap()
    );
    assert!(
        !fixture
            .store
            .read_plan_approved(&fixture.session_id, &other.id)
            .unwrap()
    );
}

#[test]
fn a_rewind_past_an_approval_takes_the_approval_with_it() {
    // Same asymmetry as `a_rewind_past_a_plan_takes_the_plan_with_it`: the
    // approval is part of the conversation. Rewinding to before the user saw
    // the plan and then letting the work proceed unreviewed would mean the
    // rewind quietly widened what the agent may do.
    let fixture = Fixture::new("approval-rewind");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    fixture.store.create_plan(&task, &plan).unwrap();
    let before = fixture
        .store
        .latest_event_seq(&fixture.session_id)
        .expect("a seq");
    fixture.store.approve_plan(&task, "tester").unwrap();

    fixture
        .store
        .rewind_conversation(&fixture.session_id, before)
        .unwrap();

    assert!(
        !fixture
            .store
            .read_plan_approved(&fixture.session_id, &task.id)
            .unwrap(),
        "a rewound approval must not still clear the gate"
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

// ---------------------------------------------------------------------------
// The phase is derived, never set. Requirement 3 and §5.1.
// ---------------------------------------------------------------------------

#[test]
fn the_phase_cannot_contradict_the_steps() {
    // §6: derived from step state. A plan with work still in flight must not
    // report Complete however its steps are titled.
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    plan.steps.push(step("step_2", StepStatus::Pending));
    assert_ne!(plan.phase(false), TaskPhase::Complete);

    for item in plan.steps.iter_mut() {
        item.status = StepStatus::Completed;
    }
    assert_eq!(plan.phase(false), TaskPhase::Complete);
}

#[test]
fn a_blocked_step_keeps_the_plan_out_of_complete() {
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::Completed));
    plan.steps.push(step("step_2", StepStatus::Blocked));
    assert_ne!(plan.phase(false), TaskPhase::Complete);
}

#[test]
fn a_skipped_step_does_not_block_completion() {
    // Skipped is a terminal answer — the user or the plan decided it was not
    // needed — unlike Blocked, which is work that failed.
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::Completed));
    plan.steps.push(step("step_2", StepStatus::Skipped));
    assert_eq!(plan.phase(false), TaskPhase::Complete);
}

#[test]
fn a_turn_that_never_finished_its_step_is_not_complete() {
    // A turn can end without calling complete_step, which leaves the step open.
    // That is honest — the work was never declared finished — and the phase
    // must not round it up to Complete.
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::Completed));
    plan.steps.push(step("step_2", StepStatus::InProgress));
    assert_ne!(plan.phase(false), TaskPhase::Complete);
}

#[test]
fn the_phase_follows_the_newest_evidence_rather_than_the_oldest() {
    // Chronological, not "any". A step that read a file and then ran a check is
    // validating; treating it as understanding because a read happened at some
    // point would pin the phase to whatever the turn did first.
    let mut plan = TaskPlan::new("task_1", 0);
    let mut first = step("step_1", StepStatus::Completed);
    first.evidence = vec![Evidence::FileRead {
        path: "src/upload.rs".to_string(),
        hash: "abc".to_string(),
    }];
    plan.steps.push(first);
    let mut second = step("step_2", StepStatus::InProgress);
    second.evidence = vec![command_exit(Some(0))];
    plan.steps.push(second);

    assert_eq!(plan.phase(false), TaskPhase::Validating);
}

#[test]
fn a_plan_whose_work_has_not_started_is_planning() {
    let plan = TaskPlan::new("task_1", 0);
    assert_eq!(plan.phase(false), TaskPhase::Planning);
}

#[test]
fn work_with_nothing_observed_yet_is_understanding() {
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    assert_eq!(plan.phase(false), TaskPhase::Understanding);
}

#[test]
fn awaiting_review_is_reviewing_even_with_a_command_behind_it() {
    // `Reviewing` is the one phase the plan cannot see for itself: a patch
    // waiting on a human is a fact about the task, not about the steps. It is
    // passed in rather than guessed, which is why it can outrank the evidence.
    let mut plan = TaskPlan::new("task_1", 0);
    let mut open = step("step_1", StepStatus::InProgress);
    open.evidence = vec![command_exit(Some(0))];
    plan.steps.push(open);

    assert_eq!(plan.phase(false), TaskPhase::Validating);
    assert_eq!(plan.phase(true), TaskPhase::Reviewing);
}

#[test]
fn a_finished_plan_is_complete_even_while_something_awaits_review() {
    // Completion outranks review: every step is terminal, so there is no work
    // left for a review to gate.
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::Completed));
    assert_eq!(plan.phase(true), TaskPhase::Complete);
}

// ---------------------------------------------------------------------------
// Carrying a plan across a task boundary.
//
// `context.md` §3.1: a task is one turn, so "the plan survives" (§5.4) is true
// of the log and false of the user unless something carries it. A resumed turn
// is a *new* task with a new id, and `read_task_plan` is keyed on that id.
// ---------------------------------------------------------------------------

#[test]
fn a_resumed_turn_recovers_the_plan_of_the_task_it_resumes() {
    let fixture = Fixture::new("resume");
    let first = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&first.id, 0);
    let mut done = step("step_1", StepStatus::Completed);
    done.evidence = vec![command_exit(Some(0))];
    done.completed_at_ms = Some(5);
    plan.steps.push(done);
    plan.steps.push(step("step_2", StepStatus::Pending));
    fixture.store.create_plan(&first, &plan).unwrap();

    let second = fixture.task("add retry handling");
    fixture.store.resume_plan(&second, &first.id).unwrap();

    let carried = fixture
        .store
        .read_task_plan(&fixture.session_id, &second.id)
        .unwrap()
        .expect("the plan carried over");
    assert_eq!(carried.steps.len(), 2);
    assert_eq!(carried.steps[0].status, StepStatus::Completed);
    assert_eq!(carried.steps[1].status, StepStatus::Pending);
    // The evidence came with it. Dropping it would make the resumed plan
    // re-run work it can already show was done, and would turn a verified step
    // into an unverified one on the way through.
    assert_eq!(carried.steps[0].evidence, plan.steps[0].evidence);
    assert!(!carried.steps[0].is_unverified());
}

#[test]
fn resuming_leaves_the_original_plan_readable_under_its_own_task() {
    // Append-only: the carried copy is a new event, not a rewrite. The first
    // task's history still says what it did.
    let fixture = Fixture::new("resume-original");
    let first = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&first.id, 0);
    plan.steps.push(step("step_1", StepStatus::Completed));
    plan.steps.push(step("step_2", StepStatus::Pending));
    fixture.store.create_plan(&first, &plan).unwrap();

    let second = fixture.task("add retry handling");
    fixture.store.resume_plan(&second, &first.id).unwrap();

    assert!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &first.id)
            .unwrap()
            .is_some(),
        "the original task must keep its plan"
    );
    assert_eq!(
        fixture.plan_event_kinds(),
        vec!["plan_created", "plan_resumed"],
        "the carry must be visible in the log as its own kind"
    );
}

#[test]
fn resuming_a_task_that_never_had_a_plan_carries_nothing() {
    let fixture = Fixture::new("resume-none");
    let first = fixture.task("what does this do");
    let second = fixture.task("and this");

    fixture.store.resume_plan(&second, &first.id).unwrap();

    assert!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &second.id)
            .unwrap()
            .is_none(),
        "there was no plan to carry, and inventing an empty one would be worse"
    );
}

#[test]
fn evidence_appended_to_a_step_outside_its_turn_survives() {
    // A patch is *proposed* in one turn and *applied* later, after that turn
    // has ended, so the apply path appends evidence to a step whose turn is
    // over. This is the path `PatchApplyResult::applied` exists for.
    let fixture = Fixture::new("append-evidence");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    fixture.store.create_plan(&task, &plan).unwrap();

    fixture
        .store
        .append_step_evidence(
            &fixture.session_id,
            &task.id,
            Evidence::PatchApplied {
                marker_id: "action_1".to_string(),
                files: vec![workspace_engine::plan::PatchedFile {
                    path: "src/upload.rs".to_string(),
                    applied_hash: "abc".to_string(),
                }],
            },
        )
        .unwrap();

    let updated = fixture
        .store
        .read_task_plan(&fixture.session_id, &task.id)
        .unwrap()
        .expect("the plan is still there");
    assert_eq!(updated.steps[0].evidence.len(), 1);
    // Still open: appending evidence records what happened, it does not decide
    // the step is finished. Only `complete_step` does that, from the evidence.
    assert_eq!(updated.steps[0].status, StepStatus::InProgress);
}

#[test]
fn appending_evidence_with_no_open_step_changes_nothing() {
    let fixture = Fixture::new("append-closed");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::Completed));
    fixture.store.create_plan(&task, &plan).unwrap();

    fixture
        .store
        .append_step_evidence(&fixture.session_id, &task.id, command_exit(Some(0)))
        .unwrap();

    let updated = fixture
        .store
        .read_task_plan(&fixture.session_id, &task.id)
        .unwrap()
        .expect("the plan is still there");
    assert!(
        updated.steps[0].evidence.is_empty(),
        "evidence must not be attached to a step that already closed on its own evidence"
    );
}

#[test]
fn a_step_whose_patch_landed_reports_the_editing_phase() {
    // Task 6 could not reach `TaskPhase::Editing` because nothing produced
    // `Evidence::PatchApplied` — a patch is applied after the turn that
    // proposed it has ended. `append_step_evidence` is that producer, so this
    // closes the gap Task 6 recorded rather than leaving a variant nothing can
    // ever return.
    let fixture = Fixture::new("editing-phase");
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    fixture.store.create_plan(&task, &plan).unwrap();

    fixture
        .store
        .append_step_evidence(
            &fixture.session_id,
            &task.id,
            Evidence::PatchApplied {
                marker_id: "action_1".to_string(),
                files: vec![workspace_engine::plan::PatchedFile {
                    path: "src/upload.rs".to_string(),
                    applied_hash: "abc".to_string(),
                }],
            },
        )
        .unwrap();

    let updated = fixture
        .store
        .read_task_plan(&fixture.session_id, &task.id)
        .unwrap()
        .expect("the plan is still there");
    assert_eq!(updated.phase(false), TaskPhase::Editing);
}

// ---------------------------------------------------------------------------
// The completion report. Proposal §5.6.
//
// The rule these hold is that the summary never overstates. A blocked step and
// an unverified one are different kinds of "not confirmed", and collapsing
// either into "complete" is the failure requirement 6 exists to prevent.
// ---------------------------------------------------------------------------

fn reported(plan: &TaskPlan) -> Vec<StepOutcome> {
    plan.report()
        .steps
        .iter()
        .map(|step| step.outcome)
        .collect()
}

#[test]
fn each_step_reports_one_of_the_four_outcomes() {
    let mut plan = TaskPlan::new("task_1", 0);
    let mut verified = step("step_1", StepStatus::Completed);
    verified.evidence.push(command_exit(Some(0)));
    plan.steps.push(verified);
    plan.steps.push(step("step_2", StepStatus::Completed));
    plan.steps.push(step("step_3", StepStatus::Blocked));
    plan.steps.push(step("step_4", StepStatus::Skipped));

    assert_eq!(
        reported(&plan),
        vec![
            StepOutcome::Verified,
            StepOutcome::Unverified,
            StepOutcome::Blocked,
            StepOutcome::Skipped,
        ]
    );
}

#[test]
fn a_step_still_running_is_not_reported_as_an_outcome() {
    // A turn can end with a step open — a token stop, a stop button, a plan
    // held for review. Reporting it beside the finished ones would say the
    // work reached an outcome it has not reached.
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::Completed));
    plan.steps.push(step("step_2", StepStatus::InProgress));
    plan.steps.push(step("step_3", StepStatus::Pending));

    let report = plan.report();
    assert_eq!(report.steps.len(), 3);
    assert_eq!(report.steps[1].outcome, StepOutcome::Outstanding);
    assert_eq!(report.outstanding, 2);
    assert!(
        !report.is_complete,
        "a plan with work still open is not complete"
    );
    assert!(
        !report.summary().to_lowercase().contains("complete"),
        "the summary said complete while two steps were still open: {}",
        report.summary()
    );
}

#[test]
fn a_plan_with_a_blocked_step_is_never_summarised_as_complete() {
    // The line the spec names by hand. Every other step passing does not make
    // the plan complete, and a summary that said so would be the single most
    // misleading thing the panel could print.
    let mut plan = TaskPlan::new("task_1", 0);
    let mut verified = step("step_1", StepStatus::Completed);
    verified.evidence.push(command_exit(Some(0)));
    plan.steps.push(verified);
    plan.steps.push(step("step_2", StepStatus::Blocked));

    let report = plan.report();
    assert!(!report.is_complete);
    assert_eq!(report.blocked, 1);
    assert!(
        !report.summary().to_lowercase().contains("complete"),
        "the summary said complete for a plan with a blocked step: {}",
        report.summary()
    );
}

#[test]
fn a_plan_whose_steps_all_carry_evidence_is_complete() {
    let mut plan = TaskPlan::new("task_1", 0);
    for id in ["step_1", "step_2"] {
        let mut done = step(id, StepStatus::Completed);
        done.evidence.push(command_exit(Some(0)));
        plan.steps.push(done);
    }

    let report = plan.report();
    assert!(report.is_complete);
    assert_eq!(report.verified, 2);
    assert_eq!(report.unverified, 0);
    assert!(report.summary().contains("2 steps complete"));
}

#[test]
fn an_unverified_step_is_still_complete_but_the_summary_says_so() {
    // §5.3's last row, carried into the report: a step with nothing observable
    // behind it did finish, and the honest thing is to say it finished without
    // confirmation rather than either hiding the gap or calling it a failure.
    let mut plan = TaskPlan::new("task_1", 0);
    let mut verified = step("step_1", StepStatus::Completed);
    verified.evidence.push(command_exit(Some(0)));
    plan.steps.push(verified);
    plan.steps.push(step("step_2", StepStatus::Completed));

    let report = plan.report();
    assert!(report.is_complete);
    assert_eq!(report.unverified, 1);
    assert!(
        report.summary().contains("1 unverified"),
        "an unverified step must be visible in the summary: {}",
        report.summary()
    );
}

#[test]
fn a_plan_with_no_steps_reports_nothing_rather_than_success() {
    // A zero-step plan and a finished one are different facts. Counting "all
    // zero steps complete" as success is the vacuous-truth bug that would make
    // an empty plan the best-looking outcome in the report.
    let plan = TaskPlan::new("task_1", 0);
    let report = plan.report();

    assert!(!report.is_complete);
    assert_eq!(report.summary(), "No steps planned.");
}

#[test]
fn the_session_reader_agrees_with_the_per_task_reader() {
    // `read_session_plans` exists only to avoid re-reading the log once per
    // task. The moment it disagrees with `read_task_plan` about any plan it
    // has stopped being an optimisation and become a second implementation.
    let fixture = Fixture::new("session-plans");
    let first = fixture.task("add retry handling");
    let second = fixture.task("rewrite the parser");

    let mut plan_one = TaskPlan::new(&first.id, 0);
    plan_one.steps.push(step("step_1", StepStatus::InProgress));
    plan_one.steps.push(step("step_2", StepStatus::Pending));
    fixture.store.create_plan(&first, &plan_one).unwrap();

    let mut plan_two = TaskPlan::new(&second.id, 0);
    plan_two.steps.push(step("step_1", StepStatus::Pending));
    fixture.store.create_plan(&second, &plan_two).unwrap();

    // An update to the first plan, and one naming a step nothing has.
    let mut closed = step("step_1", StepStatus::Completed);
    closed.evidence.push(command_exit(Some(0)));
    fixture.store.update_plan_step(&first, &closed).unwrap();
    fixture
        .store
        .update_plan_step(&first, &step("step_99", StepStatus::Completed))
        .unwrap();

    let all = fixture
        .store
        .read_session_plans(&fixture.session_id)
        .unwrap();
    assert_eq!(all.len(), 2);
    for task in [&first, &second] {
        assert_eq!(
            all.get(&task.id),
            fixture
                .store
                .read_task_plan(&fixture.session_id, &task.id)
                .unwrap()
                .as_ref(),
            "the two readers disagree about {}",
            task.id
        );
    }
    assert_eq!(all[&first.id].steps[0].status, StepStatus::Completed);
    assert_eq!(
        all[&first.id].steps.len(),
        2,
        "the unknown step was ignored"
    );
}

#[test]
fn a_session_with_no_plans_reads_as_empty_rather_than_failing() {
    let fixture = Fixture::new("session-plans-empty");
    fixture.task("what does this do?");
    assert!(
        fixture
            .store
            .read_session_plans(&fixture.session_id)
            .unwrap()
            .is_empty()
    );
}
