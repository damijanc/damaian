//! The plan a turn works through, per
//! `docs/specs/21_task_plan_progress_and_budget/proposal.md`.
//!
//! The rule these tests exist to hold is requirement 6: a step is never marked
//! complete only because the model said so. Every assertion about `Evidence`
//! and about `status_from_evidence` is an assertion about that guarantee.

use workspace_engine::plan::{Evidence, PlanStep, StepStatus, TaskPlan};

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
