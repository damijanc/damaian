use serde::Serialize;
use workspace_engine::{ClientError, Result};

use crate::metrics::{MetricSet, MetricValue};
use crate::record::{AssertionOutcome, RunRecord};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub records: Vec<RunRecord>,
    pub metrics: MetricSet,
    /// Without this a baseline is not comparable: §5.7 requires the Damaian
    /// version alongside the numbers.
    pub damaian_version: String,
}

pub fn build(records: Vec<RunRecord>) -> Report {
    let metrics = MetricSet::compute(&records);
    Report {
        records,
        metrics,
        damaian_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

pub fn to_json(report: &Report) -> Result<String> {
    serde_json::to_string_pretty(report)
        .map_err(|error| ClientError::Io(format!("could not serialize the report: {error}")))
}

/// Every assertion that failed, paired with its scenario. Skipped assertions are
/// not failures and do not appear.
pub fn failed_assertions(report: &Report) -> Vec<(String, AssertionOutcome)> {
    let mut failures = Vec::new();
    for record in &report.records {
        for assertion in &record.assertions {
            if !assertion.passed && !assertion.skipped {
                failures.push((record.scenario.clone(), assertion.clone()));
            }
        }
    }
    failures
}

pub fn to_text(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("Damaian evaluation report\n");
    out.push_str(&format!("version: {}\n\n", report.damaian_version));

    out.push_str("Scenarios\n");
    for record in &report.records {
        let state = match &record.not_applicable {
            Some(phase) => format!("skipped ({phase})"),
            None => {
                let failed = record
                    .assertions
                    .iter()
                    .filter(|one| !one.passed && !one.skipped)
                    .count();
                let skipped = record.assertions.iter().filter(|one| one.skipped).count();
                if failed > 0 {
                    format!("FAIL ({failed})")
                } else if skipped > 0 {
                    format!("pass ({skipped} skipped)")
                } else {
                    "pass".to_string()
                }
            }
        };
        out.push_str(&format!(
            "  {:<32} {:<18} {}ms\n",
            record.scenario, state, record.duration_ms
        ));
    }

    out.push_str("\nMetrics\n");
    for metric in &report.metrics.metrics {
        let rendered = match &metric.value {
            MetricValue::Number { value } => format!("{value:.3}"),
            MetricValue::Count { value } => value.to_string(),
            MetricValue::Human {
                value,
                sample_size,
                reason,
            } => match value {
                Some(value) => format!("{value:.3} (human, n={sample_size})"),
                None => format!(
                    "not recorded (human) — {}",
                    reason.as_deref().unwrap_or("no value entered")
                ),
            },
            MetricValue::NotApplicable { phase } => format!("not applicable ({phase})"),
        };
        out.push_str(&format!("  {:<74} {rendered}\n", metric.label));
    }

    let failures = failed_assertions(report);
    if !failures.is_empty() {
        out.push_str("\nFailures\n");
        for (scenario, assertion) in failures {
            out.push_str(&format!(
                "  {scenario} / {}\n    expected: {}\n    actual:   {}\n",
                assertion.name, assertion.expected, assertion.actual
            ));
        }
    }
    out
}
