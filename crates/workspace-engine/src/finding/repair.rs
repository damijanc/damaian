//! The scoped repair request (spec 22 Task 9, proposal §5.5).
//!
//! The user selects findings and asks the agent to fix them. Only `Open`
//! findings are kept. Every other selected id is excluded with its reason,
//! never silently, and the agent gets the findings, not the logs behind them
//! (`docs/specs/22_findings_model_and_panel/context.md` §15).

use super::{Finding, FindingStatus, Severity};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// Its file changed after the check ran, so its range may be wrong.
    Stale,
    Dismissed,
    Fixed,
    /// Not a finding in this session, for example after a rewind.
    Unknown,
}

impl ExclusionReason {
    /// The note the rendered request gives for leaving a finding out.
    pub fn explanation(self) -> &'static str {
        match self {
            Self::Stale => {
                "stale: its file changed after the check ran, so its location may be wrong. \
                 Re-run the check for a current finding."
            }
            Self::Dismissed => "dismissed by the user.",
            Self::Fixed => "already marked fixed.",
            Self::Unknown => "not a finding in this session.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exclusion {
    pub finding_id: String,
    pub reason: ExclusionReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairRequest {
    /// Open findings, in session order.
    pub findings: Vec<Finding>,
    pub excluded: Vec<Exclusion>,
}

impl RepairRequest {
    /// Resolves `selected_ids` against `findings`, which should come from
    /// `SessionStore::read_findings` so that staleness is current at the
    /// moment of asking.
    pub fn select(findings: &[Finding], selected_ids: &[String]) -> Self {
        let mut kept = Vec::new();
        let mut excluded = Vec::new();
        for finding in findings
            .iter()
            .filter(|finding| selected_ids.iter().any(|id| id == finding.id()))
        {
            let reason = match finding.status() {
                FindingStatus::Open => {
                    kept.push(finding.clone());
                    continue;
                }
                FindingStatus::Stale => ExclusionReason::Stale,
                FindingStatus::Dismissed => ExclusionReason::Dismissed,
                FindingStatus::Fixed => ExclusionReason::Fixed,
            };
            excluded.push(Exclusion {
                finding_id: finding.id().to_string(),
                reason,
            });
        }
        for id in selected_ids {
            let known = findings.iter().any(|finding| finding.id() == id.as_str());
            let noted = excluded.iter().any(|exclusion| exclusion.finding_id == *id);
            if !known && !noted {
                excluded.push(Exclusion {
                    finding_id: id.clone(),
                    reason: ExclusionReason::Unknown,
                });
            }
        }
        Self {
            findings: kept,
            excluded,
        }
    }

    /// True when nothing is left to repair.
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    /// The model-facing request, or `None` when nothing is left to repair.
    pub fn render(&self) -> Option<String> {
        if self.findings.is_empty() {
            return None;
        }
        let count = self.findings.len();
        let mut lines = vec![
            format!(
                "Fix the {count} finding{} below, which checks reported in this session. \
                 Change only what each one needs, and say which you fixed by its id.",
                if count == 1 { "" } else { "s" }
            ),
            String::new(),
        ];
        for (index, finding) in self.findings.iter().enumerate() {
            lines.push(format!(
                "{}. {} (finding {})",
                index + 1,
                headline(finding),
                finding.id()
            ));
            for line in finding.details().into_iter().flat_map(str::lines) {
                lines.push(if line.is_empty() {
                    String::new()
                } else {
                    format!("   {line}")
                });
            }
        }
        if !self.excluded.is_empty() {
            lines.push(String::new());
            lines.push("Left out of this request:".to_string());
            for exclusion in &self.excluded {
                lines.push(format!(
                    "- {}: {}",
                    exclusion.finding_id,
                    exclusion.reason.explanation()
                ));
            }
        }
        Some(lines.join("\n"))
    }
}

/// `compiler error E0308 at src/a.rs:8:18: mismatched types`, leaving out
/// the parts a finding does not have.
fn headline(finding: &Finding) -> String {
    let severity = match finding.severity() {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    };
    let mut text = format!("{} {severity}", finding.source().as_str());
    if let Some(code) = finding.code() {
        text.push(' ');
        text.push_str(code);
    }
    if let Some(range) = finding.range() {
        text.push_str(&format!(" at {}:{}", range.path, range.start_line));
        if let Some(column) = range.start_column {
            text.push_str(&format!(":{column}"));
        }
    }
    format!("{text}: {}", finding.summary())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::{FindingDraft, FindingSource, SourceRange};
    use crate::secret_scanner::SecretScanner;

    fn finding(
        source: FindingSource,
        summary: &str,
        range: Option<(&str, u32, Option<u32>)>,
        code: Option<&str>,
        details: Option<&str>,
        status: FindingStatus,
    ) -> Finding {
        let mut finding = Finding::new(
            FindingDraft {
                source,
                severity: Severity::Error,
                summary: summary.to_string(),
                details: details.map(str::to_string),
                range: range.map(|(path, line, column)| SourceRange {
                    path: path.to_string(),
                    start_line: line,
                    start_column: column,
                    end_line: None,
                    end_column: None,
                }),
                code: code.map(str::to_string),
            },
            &SecretScanner::default(),
        );
        finding.set_status(status);
        finding
    }

    fn open(summary: &str) -> Finding {
        finding(
            FindingSource::Command,
            summary,
            None,
            None,
            None,
            FindingStatus::Open,
        )
    }

    fn with_status(summary: &str, status: FindingStatus) -> Finding {
        finding(FindingSource::Command, summary, None, None, None, status)
    }

    fn ids(findings: &[&Finding]) -> Vec<String> {
        findings
            .iter()
            .map(|finding| finding.id().to_string())
            .collect()
    }

    /// Session order, not selection order, so a selection always renders
    /// the same text (`context.md` §15).
    #[test]
    fn open_findings_are_kept_in_session_order() {
        let (a, b, c) = (open("a"), open("b"), open("c"));
        let session = [a.clone(), b.clone(), c.clone()];
        let request = RepairRequest::select(&session, &ids(&[&c, &a]));
        let kept: Vec<_> = request.findings.iter().map(Finding::summary).collect();
        assert_eq!(kept, ["a", "c"]);
        assert!(request.excluded.is_empty());
    }

    /// Proposal §5.5: excluded with a note, not silently included.
    #[test]
    fn a_stale_finding_is_excluded_with_its_reason() {
        let stale = with_status("moved", FindingStatus::Stale);
        let fresh = open("fresh");
        let request =
            RepairRequest::select(&[stale.clone(), fresh.clone()], &ids(&[&stale, &fresh]));
        assert_eq!(
            request.findings.iter().map(Finding::id).collect::<Vec<_>>(),
            [fresh.id()]
        );
        assert_eq!(
            request.excluded,
            [Exclusion {
                finding_id: stale.id().to_string(),
                reason: ExclusionReason::Stale
            }]
        );
    }

    #[test]
    fn dismissed_and_fixed_findings_are_excluded_with_their_reasons() {
        let dismissed = with_status("waved away", FindingStatus::Dismissed);
        let fixed = with_status("done", FindingStatus::Fixed);
        let request = RepairRequest::select(
            &[dismissed.clone(), fixed.clone()],
            &ids(&[&dismissed, &fixed]),
        );
        assert!(request.is_empty());
        let reasons: Vec<_> = request.excluded.iter().map(|e| e.reason).collect();
        assert_eq!(
            reasons,
            [ExclusionReason::Dismissed, ExclusionReason::Fixed]
        );
    }

    /// A rewind removes findings, and the panel may hold an old selection.
    #[test]
    fn an_unknown_id_is_excluded_as_unknown_never_dropped() {
        let known = open("known");
        let request = RepairRequest::select(
            std::slice::from_ref(&known),
            &[known.id().to_string(), "finding_gone".to_string()],
        );
        assert_eq!(request.findings.len(), 1);
        assert_eq!(
            request.excluded,
            [Exclusion {
                finding_id: "finding_gone".to_string(),
                reason: ExclusionReason::Unknown,
            }]
        );
    }

    #[test]
    fn a_duplicate_selection_is_counted_once() {
        let known = open("known");
        let gone = "finding_gone".to_string();
        let request = RepairRequest::select(
            std::slice::from_ref(&known),
            &[
                known.id().to_string(),
                known.id().to_string(),
                gone.clone(),
                gone,
            ],
        );
        assert_eq!(request.findings.len(), 1);
        assert_eq!(request.excluded.len(), 1);
    }

    #[test]
    fn nothing_left_to_repair_renders_nothing() {
        let stale = with_status("moved", FindingStatus::Stale);
        let request = RepairRequest::select(std::slice::from_ref(&stale), &ids(&[&stale]));
        assert!(request.is_empty());
        assert_eq!(
            request.render(),
            None,
            "a prompt that fixes nothing is not sent"
        );
        assert_eq!(RepairRequest::select(&[], &[]).render(), None);
    }

    /// The exact text (`context.md` §15): each finding by id, with its
    /// location and code where it has them, and its details indented.
    /// Exclusions are listed with their reason.
    #[test]
    fn the_rendered_request_names_each_finding_and_lists_what_was_left_out() {
        let compiler = finding(
            FindingSource::Compiler,
            "mismatched types",
            Some(("src/a.rs", 8, Some(18))),
            Some("E0308"),
            Some("error[E0308]: mismatched types\n --> src/a.rs:8:18\n\n  |"),
            FindingStatus::Open,
        );
        let command = open("pytest: FAILED test_x");
        let stale = with_status("moved", FindingStatus::Stale);
        let request = RepairRequest::select(
            &[compiler.clone(), command.clone(), stale.clone()],
            &ids(&[&compiler, &command, &stale]),
        );

        let expected = [
            "Fix the 2 findings below, which checks reported in this session. Change only what \
             each one needs, and say which you fixed by its id."
                .to_string(),
            String::new(),
            format!(
                "1. compiler error E0308 at src/a.rs:8:18: mismatched types (finding {})",
                compiler.id()
            ),
            "   error[E0308]: mismatched types".to_string(),
            "    --> src/a.rs:8:18".to_string(),
            String::new(),
            "     |".to_string(),
            format!(
                "2. command error: pytest: FAILED test_x (finding {})",
                command.id()
            ),
            String::new(),
            "Left out of this request:".to_string(),
            format!(
                "- {}: stale: its file changed after the check ran, so its location may be \
                 wrong. Re-run the check for a current finding.",
                stale.id()
            ),
        ]
        .join("\n");
        assert_eq!(request.render().as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn a_line_without_a_column_renders_without_one() {
        let finding = finding(
            FindingSource::Test,
            "tests::adds failed",
            Some(("src/lib.rs", 12, None)),
            None,
            None,
            FindingStatus::Open,
        );
        let text = RepairRequest::select(std::slice::from_ref(&finding), &ids(&[&finding]))
            .render()
            .unwrap();
        assert!(
            text.contains("1. test error at src/lib.rs:12: tests::adds failed"),
            "{text}"
        );
    }

    /// Task 10 serves this shape.
    #[test]
    fn the_request_serialises_for_the_shell() {
        let kept = open("kept");
        let stale = with_status("moved", FindingStatus::Stale);
        let request = RepairRequest::select(&[kept.clone(), stale.clone()], &ids(&[&kept, &stale]));
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["findings"][0]["id"], kept.id());
        assert_eq!(value["excluded"][0]["findingId"], stale.id());
        assert_eq!(value["excluded"][0]["reason"], "stale");
    }
}
