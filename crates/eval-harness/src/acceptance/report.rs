//! The acceptance report (spec 106 proposal §5, `context.md` D8 and D9).
//!
//! One run produces `report.json`, a short `report.md`, and a summary shaped
//! for #100's worker return so the deviations reach the main session that
//! started the work, not only a file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use workspace_engine::{ClientError, Result, SessionStore, VerificationEvent};

use super::criteria::{Criterion, regression_observed};
use super::driver::{DecisionKind, DriveOutcome};

/// Which run this was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunMetadata {
    pub run_id: String,
    /// `deterministic` or `live`.
    pub tier: String,
    pub fixture: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub duration_ms: u128,
}

/// Counts read from the durable session (§5).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Metrics {
    pub tasks: usize,
    pub segments_started: usize,
    pub within_turn_compactions: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `true` when any contributing count was an estimate, not provider-reported.
    pub tokens_estimated: bool,
    pub questions_asked: usize,
    /// Verification attempts after the first (each is one repair).
    pub repair_attempts: usize,
    pub decisions_by_kind: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedDecision {
    pub sequence: usize,
    pub kind: String,
    pub subject: String,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedDeviation {
    pub sequence: usize,
    pub kind: String,
    pub detail: String,
}

/// The whole report for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcceptanceReport {
    pub metadata: RunMetadata,
    /// Every criterion passed.
    pub passed: bool,
    pub ended_status: Option<String>,
    pub failure: Option<String>,
    pub criteria: Vec<Criterion>,
    pub deviations: Vec<ReportedDeviation>,
    pub decisions: Vec<ReportedDecision>,
    pub metrics: Metrics,
    pub regression_observed: bool,
    pub changed_paths: Vec<String>,
}

impl AcceptanceReport {
    /// Assemble the report from the driver's account, the criteria and the
    /// durable session.
    pub fn build(
        metadata: RunMetadata,
        outcome: &DriveOutcome,
        criteria: Vec<Criterion>,
        store: &SessionStore,
        session_id: &str,
    ) -> Result<Self> {
        let tasks = store.read_tasks(session_id)?;
        let events = store.read_verification_events(session_id, &[])?;
        let mut metrics = Metrics {
            tasks: tasks.len(),
            within_turn_compactions: store.read_within_turn_compaction_attempts(session_id)?.len(),
            repair_attempts: events
                .iter()
                .filter(|event| {
                    matches!(event, VerificationEvent::Started { attempt, .. } if *attempt > 0)
                })
                .count(),
            ..Metrics::default()
        };
        let mut changed_paths = Vec::new();
        for task in &tasks {
            metrics.segments_started += store
                .read_task_segments(session_id, &task.id)?
                .iter()
                .filter(|segment| segment.is_started())
                .count();
            metrics.questions_asked += store.questions_asked_in_task(session_id, &task.id)?;
            for decision in store.read_patch_file_decisions(session_id, &task.id)? {
                if decision.applied {
                    changed_paths.extend(decision.paths);
                }
            }
        }
        changed_paths.sort();
        changed_paths.dedup();
        for usage in store.read_task_usage(session_id)?.values() {
            metrics.input_tokens += usage.input_tokens;
            metrics.output_tokens += usage.output_tokens;
            metrics.tokens_estimated |= usage.source == workspace_engine::UsageSource::Estimated;
        }
        for kind in [
            DecisionKind::PlanReview,
            DecisionKind::Command,
            DecisionKind::Checks,
            DecisionKind::Baselines,
            DecisionKind::Question,
            DecisionKind::Patch,
        ] {
            metrics
                .decisions_by_kind
                .insert(kind.as_str().to_string(), outcome.count(kind));
        }
        let passed = !criteria.is_empty() && criteria.iter().all(|criterion| criterion.passed);
        Ok(Self {
            metadata,
            passed,
            ended_status: outcome.ended_status.clone(),
            failure: outcome.failure.as_ref().map(|failure| failure.describe()),
            criteria,
            deviations: outcome
                .deviations
                .iter()
                .map(|deviation| ReportedDeviation {
                    sequence: deviation.sequence,
                    kind: deviation.kind.as_str().to_string(),
                    detail: deviation.detail.clone(),
                })
                .collect(),
            decisions: outcome
                .decisions
                .iter()
                .map(|decision| ReportedDecision {
                    sequence: decision.sequence,
                    kind: decision.kind.as_str().to_string(),
                    subject: decision.subject.clone(),
                    outcome: decision.outcome.clone(),
                })
                .collect(),
            metrics,
            regression_observed: regression_observed(&events),
            changed_paths,
        })
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| ClientError::InvalidInput(format!("report serialization: {error}")))
    }

    /// The short summary for the main session (D9): the verdict, every failed
    /// criterion, and every deviation.
    pub fn return_summary(&self) -> String {
        let mut lines = vec![format!(
            "Acceptance run {} ({} tier, {}): {}.",
            self.metadata.run_id,
            self.metadata.tier,
            self.metadata.fixture,
            if self.passed { "PASSED" } else { "FAILED" }
        )];
        if let Some(failure) = &self.failure {
            lines.push(format!("Stopped: {failure}."));
        }
        for criterion in self.criteria.iter().filter(|criterion| !criterion.passed) {
            lines.push(format!(
                "Failed criterion {}: {}",
                criterion.name, criterion.observed
            ));
        }
        if self.deviations.is_empty() {
            lines.push("Deviations: none.".to_string());
        } else {
            lines.push(format!("Deviations ({}):", self.deviations.len()));
            for deviation in &self.deviations {
                lines.push(format!(
                    "- [{} #{}] {}",
                    deviation.kind, deviation.sequence, deviation.detail
                ));
            }
        }
        lines.join("\n")
    }

    /// The Markdown summary written beside `report.json`.
    pub fn to_markdown(&self) -> String {
        let mut text = format!(
            "# Acceptance run {}\n\n- Tier: {}\n- Fixture: {}\n- Provider/model: {}\n- Duration: {} ms\n- Result: **{}**\n- Task ended: {}\n",
            self.metadata.run_id,
            self.metadata.tier,
            self.metadata.fixture,
            match (&self.metadata.provider, &self.metadata.model) {
                (Some(provider), Some(model)) => format!("{provider} / {model}"),
                _ => "scripted (no provider)".to_string(),
            },
            self.metadata.duration_ms,
            if self.passed { "passed" } else { "failed" },
            self.ended_status.as_deref().unwrap_or("did not end"),
        );
        if let Some(failure) = &self.failure {
            text.push_str(&format!("- Stopped: {failure}\n"));
        }
        text.push_str(
            "\n## Criteria\n\n| Criterion | Result | Read from | Observed |\n|---|---|---|---|\n",
        );
        for criterion in &self.criteria {
            text.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                criterion.name,
                if criterion.passed { "pass" } else { "FAIL" },
                criterion.reader,
                criterion.observed.replace('|', "\\|")
            ));
        }
        text.push_str("\n## Deviations\n\n");
        if self.deviations.is_empty() {
            text.push_str("None.\n");
        } else {
            for deviation in &self.deviations {
                text.push_str(&format!(
                    "- [{} #{}] {}\n",
                    deviation.kind, deviation.sequence, deviation.detail
                ));
            }
        }
        let metrics = &self.metrics;
        text.push_str(&format!(
            "\n## Metrics\n\n- Tasks: {}\n- Segments started: {}\n- Within-turn compactions: {}\n- Tokens: {} in / {} out{}\n- Questions asked: {}\n- Repair attempts: {}\n- Regression observed: {}\n- Decisions: {}\n- Changed paths: {}\n",
            metrics.tasks,
            metrics.segments_started,
            metrics.within_turn_compactions,
            metrics.input_tokens,
            metrics.output_tokens,
            if metrics.tokens_estimated { " (estimated)" } else { "" },
            metrics.questions_asked,
            metrics.repair_attempts,
            self.regression_observed,
            metrics
                .decisions_by_kind
                .iter()
                .map(|(kind, count)| format!("{kind} {count}"))
                .collect::<Vec<_>>()
                .join(", "),
            if self.changed_paths.is_empty() {
                "none".to_string()
            } else {
                self.changed_paths.join(", ")
            }
        ));
        text
    }

    /// Write `report.json` and `report.md` under
    /// `<data_dir>/acceptance/<run-id>/`. Returns that directory.
    pub fn write(&self, data_dir: &Path) -> Result<PathBuf> {
        let dir = data_dir.join("acceptance").join(&self.metadata.run_id);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("report.json"), self.to_json()?)?;
        fs::write(dir.join("report.md"), self.to_markdown())?;
        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acceptance::driver::{DecisionRecord, Deviation, FailureReason};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use workspace_engine::repository_id_for_root;

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "damaian-report-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn metadata() -> RunMetadata {
        RunMetadata {
            run_id: "run-1".to_string(),
            tier: "deterministic".to_string(),
            fixture: "single_worker_spec".to_string(),
            provider: None,
            model: None,
            duration_ms: 1234,
        }
    }

    fn criterion(name: &'static str, passed: bool) -> Criterion {
        Criterion {
            name,
            passed,
            reader: "test",
            observed: format!("{name} observed"),
        }
    }

    fn outcome_with_deviation() -> DriveOutcome {
        DriveOutcome {
            decisions: vec![DecisionRecord {
                sequence: 1,
                kind: DecisionKind::Question,
                subject: "Should I add a changelog entry?".to_string(),
                outcome: "No further preference; follow the spec and AGENTS.md.".to_string(),
            }],
            deviations: vec![Deviation {
                sequence: 1,
                kind: DecisionKind::Question,
                detail: "unexpected question \"Should I add a changelog entry?\"".to_string(),
            }],
            ended_status: Some("complete".to_string()),
            ..DriveOutcome::default()
        }
    }

    fn build(outcome: &DriveOutcome, criteria: Vec<Criterion>) -> AcceptanceReport {
        let root = temp_dir("store");
        let store = SessionStore::new(root.join(".damaian"));
        let session = store
            .create_session(&repository_id_for_root(&root), "report")
            .unwrap();
        AcceptanceReport::build(metadata(), outcome, criteria, &store, &session.id).unwrap()
    }

    #[test]
    fn a_run_passes_only_when_every_criterion_passes() {
        let outcome = outcome_with_deviation();
        assert!(build(&outcome, vec![criterion("a", true), criterion("b", true)]).passed);
        assert!(!build(&outcome, vec![criterion("a", true), criterion("b", false)]).passed);
        assert!(
            !build(&outcome, Vec::new()).passed,
            "no criteria is not a pass"
        );
    }

    #[test]
    fn the_main_session_summary_carries_the_verdict_failures_and_deviations() {
        let mut outcome = outcome_with_deviation();
        outcome.failure = Some(FailureReason::NeedsForbiddenIntervention(
            "the task ended `incomplete`".to_string(),
        ));
        let report = build(&outcome, vec![criterion("final_task_complete", false)]);
        let summary = report.return_summary();
        assert!(summary.contains("FAILED"), "{summary}");
        assert!(
            summary.contains("would need a forbidden intervention"),
            "{summary}"
        );
        assert!(
            summary.contains("Failed criterion final_task_complete"),
            "{summary}"
        );
        assert!(summary.contains("Deviations (1):"), "{summary}");
        assert!(summary.contains("changelog"), "{summary}");
    }

    #[test]
    fn the_report_writes_json_and_markdown_under_the_run_id() {
        let report = build(&outcome_with_deviation(), vec![criterion("a", true)]);
        let data_dir = temp_dir("write");
        let dir = report.write(&data_dir).unwrap();
        assert_eq!(dir, data_dir.join("acceptance").join("run-1"));
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
        assert_eq!(json["passed"], true);
        assert_eq!(json["deviations"][0]["kind"], "question");
        assert!(json["metrics"]["decisionsByKind"].is_null());
        assert_eq!(json["metrics"]["decisions_by_kind"]["question"], 1);
        let markdown = fs::read_to_string(dir.join("report.md")).unwrap();
        assert!(markdown.contains("## Deviations"), "{markdown}");
        assert!(markdown.contains("## Criteria"), "{markdown}");
        assert!(markdown.contains("scripted (no provider)"), "{markdown}");
        let _ = fs::remove_dir_all(&data_dir);
    }
}
