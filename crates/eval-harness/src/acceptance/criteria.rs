//! Spec 106 requirement 8, judged from durable records (`context.md` D7).
//!
//! Every criterion names the reader it used and the value it saw, so a failed
//! run can be diagnosed from the report without rerunning it. Nothing here
//! reads the model's text.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use workspace_engine::hash::sha256;
use workspace_engine::plan::StepStatus;
use workspace_engine::{
    CheckId, Result, SessionStore, TaskStatus, VerificationEvent, fold_verification,
};

use super::AcceptanceFixture;
use super::driver::{DriveOutcome, FailureReason};

/// The verification loop's own plan steps; the acceptance judges the
/// worker's steps.
const VERIFICATION_STEP_PREFIX: &str = "verify_";

/// One requirement-8 criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Criterion {
    pub name: &'static str,
    pub passed: bool,
    /// The durable reader (or file) the verdict came from.
    pub reader: &'static str,
    /// What the reader returned, briefly.
    pub observed: String,
}

/// The protected files as they were before the run, by content hash.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProtectedSnapshot {
    hashes: BTreeMap<String, String>,
}

impl ProtectedSnapshot {
    /// Hash every file under each protected path.
    pub fn take(repository_root: &Path, protected_paths: &[String]) -> Self {
        let mut hashes = BTreeMap::new();
        for protected in protected_paths {
            let root = repository_root.join(protected.trim_end_matches('/'));
            collect(repository_root, &root, &mut hashes);
        }
        Self { hashes }
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    /// Paths whose content differs, or that were added or removed.
    pub fn changes_since(&self, now: &Self) -> Vec<String> {
        let mut changed: Vec<String> = self
            .hashes
            .iter()
            .filter(|(path, hash)| now.hashes.get(*path) != Some(hash))
            .map(|(path, _)| path.clone())
            .collect();
        changed.extend(
            now.hashes
                .keys()
                .filter(|path| !self.hashes.contains_key(*path))
                .cloned(),
        );
        changed.sort();
        changed
    }
}

fn collect(repository_root: &Path, path: &Path, hashes: &mut BTreeMap<String, String>) {
    if path.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            collect(repository_root, &entry.path(), hashes);
        }
    } else if let Ok(bytes) = fs::read(path) {
        let relative = path
            .strip_prefix(repository_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        hashes.insert(relative, sha256(&bytes));
    }
}

/// What the criteria read.
pub struct CriteriaInput<'a> {
    pub store: &'a SessionStore,
    pub session_id: &'a str,
    pub repository_root: &'a Path,
    pub acceptance: &'a AcceptanceFixture,
    pub outcome: &'a DriveOutcome,
    pub protected_before: &'a ProtectedSnapshot,
}

/// Evaluate requirement 8. The run passes only when every criterion passes.
pub fn evaluate(input: &CriteriaInput<'_>) -> Result<Vec<Criterion>> {
    let tasks = input.store.read_tasks(input.session_id)?;
    let latest = tasks.last();
    let report = match latest {
        Some(task) => input
            .store
            .read_completion_report(input.session_id, &task.id)?,
        None => None,
    };
    let mut plan = None;
    for task in tasks.iter().rev() {
        if let Some(found) = input.store.read_task_plan(input.session_id, &task.id)? {
            plan = Some(found);
            break;
        }
    }
    let events = input
        .store
        .read_verification_events(input.session_id, &[])?;
    let verification = fold_verification(events.iter());

    let mut criteria = Vec::new();

    criteria.push(Criterion {
        name: "final_task_complete",
        passed: latest.is_some_and(|task| task.status == TaskStatus::Complete)
            && report.as_ref().is_some_and(|report| report.eligible),
        reader: "SessionStore::read_tasks + read_completion_report",
        observed: format!(
            "status {}, gate eligible {}",
            latest
                .map(|task| task.status.as_str().to_string())
                .unwrap_or_else(|| "no task".to_string()),
            report.as_ref().is_some_and(|report| report.eligible)
        ),
    });

    let worker_steps: Vec<_> = plan
        .iter()
        .flat_map(|plan| plan.steps.iter())
        .filter(|step| !step.id.starts_with(VERIFICATION_STEP_PREFIX))
        .collect();
    criteria.push(Criterion {
        name: "plan_steps_finished",
        passed: !worker_steps.is_empty()
            && worker_steps
                .iter()
                .all(|step| matches!(step.status, StepStatus::Completed | StepStatus::Skipped)),
        reader: "SessionStore::read_task_plan",
        observed: if worker_steps.is_empty() {
            "no approved plan".to_string()
        } else {
            worker_steps
                .iter()
                .map(|step| format!("{}={:?}", step.id, step.status))
                .collect::<Vec<_>>()
                .join(", ")
        },
    });

    criteria.push(Criterion {
        name: "required_checks_satisfied",
        passed: report
            .as_ref()
            .is_some_and(|report| report.reasons.is_empty()),
        reader: "SessionStore::read_completion_report",
        observed: match &report {
            Some(report) => format!(
                "reasons [{}], waivers {}, accepted failures {}",
                report
                    .reasons
                    .iter()
                    .map(|reason| reason.code.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                report.waivers.len(),
                report.accepted_failures.len()
            ),
            None => "no completion report".to_string(),
        },
    });

    let pre_existing = CheckId::new(
        input
            .acceptance
            .pre_existing_failure
            .working_directory
            .as_str(),
        input.acceptance.pre_existing_failure.command.as_str(),
    );
    let accepted = report.as_ref().is_some_and(|report| {
        report
            .accepted_failures
            .iter()
            .any(|acceptance| acceptance.check == pre_existing)
    });
    criteria.push(Criterion {
        name: "pre_existing_failure_unchanged",
        passed: accepted,
        reader: "SessionStore::read_completion_report (accepted_failures)",
        observed: format!(
            "{} accepted as pre-existing: {accepted}",
            pre_existing.label()
        ),
    });

    let unrepaired: Vec<String> = verification
        .provenance
        .keys()
        .filter(|check| {
            verification
                .current_provenance(check)
                .is_some_and(|record| !record.introduced.is_empty())
        })
        .map(|check| check.label())
        .collect();
    criteria.push(Criterion {
        name: "introduced_failures_repaired",
        passed: unrepaired.is_empty(),
        reader: "SessionStore::read_verification_events (provenance)",
        observed: format!(
            "regression observed: {}; still introduced: [{}]",
            regression_observed(&events),
            unrepaired.join(", ")
        ),
    });

    let mut touched_protected = Vec::new();
    for task in &tasks {
        for decision in input
            .store
            .read_patch_file_decisions(input.session_id, &task.id)?
        {
            for path in &decision.paths {
                if is_protected(input.acceptance, path) {
                    touched_protected.push(path.clone());
                }
            }
        }
    }
    let changed_on_disk = input
        .protected_before
        .changes_since(&ProtectedSnapshot::take(
            input.repository_root,
            &input.acceptance.protected_paths,
        ));
    let proposed_protected = matches!(
        input.outcome.failure,
        Some(FailureReason::ProtectedPathProposed(_))
    );
    criteria.push(Criterion {
        name: "protected_paths_untouched",
        passed: touched_protected.is_empty() && changed_on_disk.is_empty() && !proposed_protected,
        reader: "SessionStore::read_patch_file_decisions + protected-file hashes",
        observed: format!(
            "patch decisions on protected paths [{}], changed on disk [{}], protected proposal {}",
            touched_protected.join(", "),
            changed_on_disk.join(", "),
            proposed_protected
        ),
    });

    let (record_passed, record_observed) = task_record_state(input);
    criteria.push(Criterion {
        name: "task_record_updated",
        passed: record_passed,
        reader: "progress row in the fixture's tasks.md",
        observed: record_observed,
    });

    let pending = match latest {
        Some(task) => input
            .store
            .pending_decision_for(input.session_id, &task.id)?,
        None => None,
    };
    criteria.push(Criterion {
        name: "no_pending_decision",
        passed: pending.is_none(),
        reader: "SessionStore::pending_decision_for",
        observed: format!("{pending:?}"),
    });

    let mut answered = Vec::new();
    for question_id in &input.outcome.clarification_question_ids {
        if let Some(state) = input.store.question_state(input.session_id, question_id)?
            && state.answer.as_ref().is_some_and(|(answer, _)| {
                super::policy::Policy::new(input.acceptance.clone()).is_scripted_answer(answer)
            })
        {
            answered.push(question_id.clone());
        }
    }
    criteria.push(Criterion {
        name: "clarification_asked_and_answered",
        passed: !answered.is_empty(),
        reader: "SessionStore::question_state",
        observed: format!(
            "clarification questions answered with the scripted answer: [{}]",
            answered.join(", ")
        ),
    });

    criteria.push(Criterion {
        name: "no_forbidden_intervention_needed",
        passed: !matches!(
            input.outcome.failure,
            Some(
                FailureReason::NeedsForbiddenIntervention(_)
                    | FailureReason::TooManyDecisions(_)
                    | FailureReason::UnrecognisedDecision(_)
                    | FailureReason::Transport(_)
            )
        ),
        reader: "acceptance driver outcome",
        observed: input
            .outcome
            .failure
            .as_ref()
            .map(FailureReason::describe)
            .unwrap_or_else(|| "none".to_string()),
    });

    Ok(criteria)
}

/// Whether any post-patch classification recorded an introduced failure.
pub fn regression_observed(events: &[VerificationEvent]) -> bool {
    events.iter().any(|event| {
        matches!(event, VerificationEvent::Provenance { record, .. } if !record.introduced.is_empty())
    })
}

fn is_protected(acceptance: &AcceptanceFixture, path: &str) -> bool {
    let path = path.trim().trim_start_matches("./");
    acceptance.protected_paths.iter().any(|protected| {
        let directory = protected.trim_end_matches('/');
        path == directory || path.starts_with(&format!("{directory}/"))
    })
}

fn task_record_state(input: &CriteriaInput<'_>) -> (bool, String) {
    let record = &input.acceptance.progress_record;
    let path = input.repository_root.join(&record.path);
    let Ok(text) = fs::read_to_string(&path) else {
        return (false, format!("{} is missing", record.path));
    };
    let Some(row) = text
        .lines()
        .find(|line| line.starts_with(&record.row_prefix))
    else {
        return (false, format!("no row starting `{}`", record.row_prefix));
    };
    let state = row.split('|').nth(2).map(str::trim).unwrap_or_default();
    (
        state.starts_with(&record.done_state),
        format!("row state `{state}`"),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use workspace_engine::plan::{PlanStep, TaskPlan};
    use workspace_engine::{
        AskUserInput, CompletionAcceptance, CompletionGateReport, CompletionReason,
        CompletionReasonCode, FailureProvenance, PatchFileDecision, PendingQuestionRef, Session,
        Task, repository_id_for_root,
    };

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "damaian-acceptance-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("crates/legacy/src")).unwrap();
        fs::create_dir_all(root.join("docs/specs/01_rounding")).unwrap();
        fs::write(root.join("crates/legacy/src/lib.rs"), "pub fn x() {}\n").unwrap();
        root
    }

    fn acceptance() -> AcceptanceFixture {
        AcceptanceFixture::load("single_worker_spec").unwrap()
    }

    fn write_record(root: &Path, state: &str) {
        fs::write(
            root.join("docs/specs/01_rounding/tasks.md"),
            format!("| Task | State | What landed |\n|---|---|---|\n| 1 · Round amounts | {state} | rounding |\n"),
        )
        .unwrap();
    }

    fn step(id: &str, status: StepStatus) -> PlanStep {
        PlanStep {
            id: id.to_string(),
            title: id.to_string(),
            detail: None,
            status,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
        }
    }

    /// A fabricated finished run: one task, a finished plan, a completion
    /// report accepting the pre-existing failure, an answered clarification.
    struct Run {
        root: PathBuf,
        store: SessionStore,
        session: Session,
        task: Task,
        outcome: DriveOutcome,
        before: ProtectedSnapshot,
    }

    fn finished_run(name: &str) -> Run {
        let acceptance = acceptance();
        let root = temp_root(name);
        let before = ProtectedSnapshot::take(&root, &acceptance.protected_paths);
        let store = SessionStore::new(root.join(".damaian"));
        let session = store
            .create_session(&repository_id_for_root(&root), "acceptance")
            .unwrap();
        let task = store
            .create_task(&session.id, "Implement Task 1", "mock", "mock-model")
            .unwrap();
        let mut plan = TaskPlan::new(&task.id, 0);
        plan.steps = vec![
            step("step_1", StepStatus::Completed),
            step("step_2", StepStatus::Completed),
        ];
        store.create_plan(&task, &plan).unwrap();
        let question = PendingQuestionRef::new();
        let question_id = question.question_id().to_string();
        store
            .record_question_asked(
                &session.id,
                &task.id,
                &question,
                &AskUserInput {
                    question: "How should a tie such as 2.345 round?".to_string(),
                    options: vec!["Round half up".to_string(), "Ask finance".to_string()],
                    allow_free_text: true,
                },
            )
            .unwrap();
        store
            .record_question_answered(
                &session.id,
                &question_id,
                &acceptance.clarification.answer,
                None,
            )
            .unwrap();
        let report = CompletionGateReport {
            task_id: task.id.clone(),
            eligible: true,
            reasons: Vec::new(),
            waivers: Vec::new(),
            accepted_failures: vec![CompletionAcceptance {
                check: CheckId::new("", "cargo test"),
                provenance: FailureProvenance::PreExisting,
                explanation: "known".to_string(),
            }],
        };
        let task = store
            .update_task_status_with_completion(&task, TaskStatus::Complete, &report)
            .unwrap();
        write_record(&root, "Done");
        let outcome = DriveOutcome {
            clarification_question_ids: vec![question_id],
            ended_status: Some("complete".to_string()),
            repository_root: root.clone(),
            ..DriveOutcome::default()
        };
        Run {
            root,
            store,
            session,
            task,
            outcome,
            before,
        }
    }

    fn judge(run: &Run) -> BTreeMap<&'static str, bool> {
        let acceptance = acceptance();
        evaluate(&CriteriaInput {
            store: &run.store,
            session_id: &run.session.id,
            repository_root: &run.root,
            acceptance: &acceptance,
            outcome: &run.outcome,
            protected_before: &run.before,
        })
        .unwrap()
        .into_iter()
        .map(|criterion| (criterion.name, criterion.passed))
        .collect()
    }

    fn failing(verdicts: &BTreeMap<&'static str, bool>) -> Vec<&'static str> {
        verdicts
            .iter()
            .filter(|(_, passed)| !**passed)
            .map(|(name, _)| *name)
            .collect()
    }

    #[test]
    fn a_finished_run_passes_every_criterion() {
        let run = finished_run("pass");
        let verdicts = judge(&run);
        assert_eq!(verdicts.len(), 10);
        assert!(failing(&verdicts).is_empty(), "{verdicts:?}");
        let _ = fs::remove_dir_all(&run.root);
    }

    #[test]
    fn an_open_plan_step_and_an_ineligible_gate_fail_their_criteria() {
        let run = finished_run("open-step");
        let mut plan = TaskPlan::new(&run.task.id, 0);
        plan.steps = vec![
            step("step_1", StepStatus::Completed),
            step("step_2", StepStatus::Blocked),
        ];
        run.store.create_plan(&run.task, &plan).unwrap();
        let report = CompletionGateReport {
            task_id: run.task.id.clone(),
            eligible: false,
            reasons: vec![CompletionReason {
                code: CompletionReasonCode::BlockedPlanSteps,
                explanation: "blocked".to_string(),
            }],
            waivers: Vec::new(),
            accepted_failures: Vec::new(),
        };
        run.store
            .update_task_status_with_completion(&run.task, TaskStatus::Incomplete, &report)
            .unwrap();
        let verdicts = judge(&run);
        assert_eq!(
            failing(&verdicts),
            vec![
                "final_task_complete",
                "plan_steps_finished",
                "pre_existing_failure_unchanged",
                "required_checks_satisfied",
            ]
        );
        let _ = fs::remove_dir_all(&run.root);
    }

    #[test]
    fn a_protected_file_changed_on_disk_fails_even_without_a_patch_record() {
        let run = finished_run("protected-disk");
        fs::write(run.root.join("crates/legacy/src/lib.rs"), "pub fn y() {}\n").unwrap();
        let verdicts = judge(&run);
        assert_eq!(failing(&verdicts), vec!["protected_paths_untouched"]);
        let _ = fs::remove_dir_all(&run.root);
    }

    #[test]
    fn a_protected_patch_decision_fails_the_protected_criterion() {
        let run = finished_run("protected-patch");
        run.store
            .append_patch_file_decision(
                &run.session.id,
                &PatchFileDecision {
                    task_id: run.task.id.clone(),
                    patch_id: "patch_1".to_string(),
                    paths: vec!["crates/legacy/src/lib.rs".to_string()],
                    applied: false,
                    applied_hashes: BTreeMap::new(),
                    accepted_hunks: BTreeMap::new(),
                },
            )
            .unwrap();
        let verdicts = judge(&run);
        assert_eq!(failing(&verdicts), vec!["protected_paths_untouched"]);
        let _ = fs::remove_dir_all(&run.root);
    }

    #[test]
    fn an_untouched_task_record_fails_its_criterion() {
        let run = finished_run("record");
        write_record(&run.root, "Not started");
        let verdicts = judge(&run);
        assert_eq!(failing(&verdicts), vec!["task_record_updated"]);
        let _ = fs::remove_dir_all(&run.root);
    }

    #[test]
    fn a_guessed_tie_rule_without_the_clarification_fails() {
        let mut run = finished_run("no-question");
        run.outcome.clarification_question_ids.clear();
        let verdicts = judge(&run);
        assert_eq!(failing(&verdicts), vec!["clarification_asked_and_answered"]);
        let _ = fs::remove_dir_all(&run.root);
    }

    #[test]
    fn a_run_that_needed_continue_fails_its_criterion() {
        let mut run = finished_run("continue");
        run.outcome.failure = Some(FailureReason::NeedsForbiddenIntervention(
            "the task ended `incomplete`".to_string(),
        ));
        let verdicts = judge(&run);
        assert_eq!(failing(&verdicts), vec!["no_forbidden_intervention_needed"]);
        let _ = fs::remove_dir_all(&run.root);
    }
}
