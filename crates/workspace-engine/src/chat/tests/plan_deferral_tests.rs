use super::*;
use crate::plan::{PlanStep, PlannedCheck, StepStatus, TaskPlan};

fn plan_with_check(directory: &str) -> TaskPlan {
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(PlanStep {
        id: "test".to_string(),
        title: "Validate the change".to_string(),
        detail: None,
        status: StepStatus::InProgress,
        depends_on: Vec::new(),
        started_at_ms: Some(1),
        completed_at_ms: None,
        evidence: Vec::new(),
        check: Some(PlannedCheck {
            version: PLANNED_CHECK_VERSION,
            command: "cargo test".to_string(),
            working_directory: directory.to_string(),
            purpose: "validates the final tree".to_string(),
            required: true,
        }),
        build: None,
    });
    plan
}

/// Spec 105: the plan's identity and the discovered `CheckId` are compared
/// in the same normalized directory form. A model or card that writes
/// `./crates/foo` must defer the discovered check in `crates/foo`.
#[test]
fn a_planned_check_with_a_dot_prefix_defers_the_normalized_discovered_check() {
    let plan = plan_with_check("./crates/foo");
    assert!(
        plan_defers_check(
            Path::new("/repo"),
            &plan,
            &CheckId::new("crates/foo", "cargo test")
        ),
        "`./crates/foo` and `crates/foo` are the same check directory"
    );
}
