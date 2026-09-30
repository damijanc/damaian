//! One structured finding type shared by every check source (spec 22).
//!
//! Fields are private so that [`Finding::new`] is the only constructor,
//! and therefore the only place redaction and bounding happen
//! (`docs/specs/22_findings_model_and_panel/context.md` §2).

use crate::hash::{create_id, now_millis};
use crate::secret_scanner::SecretScanner;
use serde::{Deserialize, Serialize};

pub const MAX_SUMMARY_CHARS: usize = 240;
pub const MAX_DETAILS_BYTES: usize = 4096;
pub const DETAILS_TRUNCATION_MARKER: &str = "\n… (truncated)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSource {
    Compiler,
    Test,
    Lint,
    Security,
    BrowserConsole,
    BrowserNetwork,
    CodeReview,
    /// Phase 3. Declared now so adding it is not a schema change.
    LanguageServer,
}

impl FindingSource {
    /// The serialised form, for text that names a source.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compiler => "compiler",
            Self::Test => "test",
            Self::Lint => "lint",
            Self::Security => "security",
            Self::BrowserConsole => "browser_console",
            Self::BrowserNetwork => "browser_network",
            Self::CodeReview => "code_review",
            Self::LanguageServer => "language_server",
        }
    }
}

/// As the source reported it. Two sources' `Warning`s are not claimed to be
/// equally important (proposal §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    Open,
    Dismissed,
    Fixed,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRange {
    /// Repository-relative.
    pub path: String,
    pub start_line: u32,
    pub start_column: Option<u32>,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
}

/// What a parser extracted, before redaction and bounding. Plain data:
/// parsers build these, and only [`Finding::new`] turns one into a finding
/// (`context.md` §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingDraft {
    pub source: FindingSource,
    pub severity: Severity,
    pub summary: String,
    pub details: Option<String>,
    pub range: Option<SourceRange>,
    pub code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    id: String,
    source: FindingSource,
    severity: Severity,
    /// One line, redacted, at most `MAX_SUMMARY_CHARS`.
    summary: String,
    /// Redacted, at most `MAX_DETAILS_BYTES` plus the marker. Never the
    /// whole tool output, which stays where `origin_ref` points.
    details: Option<String>,
    range: Option<SourceRange>,
    task_id: Option<String>,
    /// The command execution, diagnostic call, or tool call this came from.
    origin_ref: Option<String>,
    status: FindingStatus,
    /// Machine-readable code where the source has one: `E0308`, `no-unused-vars`.
    code: Option<String>,
    /// Hash of `range.path` when recorded. `None` means staleness cannot be
    /// judged, which is not the same as stale (`context.md` §1).
    file_hash: Option<String>,
    created_at_ms: u128,
}

impl Finding {
    /// The only constructor. Redacts, then bounds: the other order can cut
    /// a secret into a fragment the scanner no longer recognises
    /// (`context.md` §4).
    pub fn new(draft: FindingDraft, scanner: &SecretScanner) -> Self {
        let summary = bound_summary(&scanner.redact(&draft.summary).text, draft.source);
        let details = draft
            .details
            .as_deref()
            .and_then(|details| bound_details(&scanner.redact(details).text));
        Self {
            id: create_id("finding"),
            source: draft.source,
            severity: draft.severity,
            summary,
            details,
            range: draft.range,
            task_id: None,
            origin_ref: None,
            status: FindingStatus::Open,
            code: draft.code,
            file_hash: None,
            created_at_ms: now_millis(),
        }
    }

    pub fn with_task_id(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    pub fn with_origin_ref(mut self, origin_ref: impl Into<String>) -> Self {
        self.origin_ref = Some(origin_ref.into());
        self
    }

    pub fn with_file_hash(mut self, file_hash: impl Into<String>) -> Self {
        self.file_hash = Some(file_hash.into());
        self
    }

    pub fn set_status(&mut self, status: FindingStatus) {
        self.status = status;
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn source(&self) -> FindingSource {
        self.source
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn details(&self) -> Option<&str> {
        self.details.as_deref()
    }

    pub fn range(&self) -> Option<&SourceRange> {
        self.range.as_ref()
    }

    pub fn task_id(&self) -> Option<&str> {
        self.task_id.as_deref()
    }

    pub fn origin_ref(&self) -> Option<&str> {
        self.origin_ref.as_deref()
    }

    pub fn status(&self) -> FindingStatus {
        self.status
    }

    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn file_hash(&self) -> Option<&str> {
        self.file_hash.as_deref()
    }

    pub fn created_at_ms(&self) -> u128 {
        self.created_at_ms
    }
}

fn bound_summary(text: &str, source: FindingSource) -> String {
    let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return format!("{} finding with no message", source.as_str());
    };
    if line.chars().count() <= MAX_SUMMARY_CHARS {
        return line.to_string();
    }
    let mut cut: String = line.chars().take(MAX_SUMMARY_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// Keeps the head: parsed details put the useful lines first, and the
/// generic parser picks its own tail before building the draft.
fn bound_details(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    if text.len() <= MAX_DETAILS_BYTES {
        return Some(text.to_string());
    }
    let mut end = MAX_DETAILS_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}{DETAILS_TRUNCATION_MARKER}", &text[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_scanner::SecretScanner;

    const FAKE_AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

    fn draft(summary: &str, details: Option<&str>) -> FindingDraft {
        FindingDraft {
            source: FindingSource::Test,
            severity: Severity::Error,
            summary: summary.to_string(),
            details: details.map(str::to_string),
            range: None,
            code: None,
        }
    }

    #[test]
    fn a_new_finding_is_open_with_a_finding_id_and_no_references() {
        let finding = Finding::new(draft("it broke", None), &SecretScanner::default());
        assert!(finding.id().starts_with("finding_"), "{}", finding.id());
        assert_eq!(finding.status(), FindingStatus::Open);
        assert_eq!(finding.source(), FindingSource::Test);
        assert_eq!(finding.severity(), Severity::Error);
        assert_eq!(finding.task_id(), None);
        assert_eq!(finding.origin_ref(), None);
        assert_eq!(finding.file_hash(), None);
        assert!(finding.created_at_ms() > 0);
    }

    #[test]
    fn two_findings_from_the_same_draft_have_different_ids() {
        let scanner = SecretScanner::default();
        let a = Finding::new(draft("same", None), &scanner);
        let b = Finding::new(draft("same", None), &scanner);
        assert_ne!(a.id(), b.id());
    }

    /// Requirement 4, at construction (proposal §5.6).
    #[test]
    fn new_redacts_a_secret_in_summary_and_details() {
        let finding = Finding::new(
            draft(
                &format!("login failed with key {FAKE_AWS_KEY}"),
                Some(&format!("request\nAuthorization key: {FAKE_AWS_KEY}\nend")),
            ),
            &SecretScanner::default(),
        );
        assert!(
            !finding.summary().contains(FAKE_AWS_KEY),
            "{}",
            finding.summary()
        );
        assert!(
            finding.summary().contains("[REDACTED_"),
            "{}",
            finding.summary()
        );
        let details = finding.details().expect("details kept");
        assert!(!details.contains(FAKE_AWS_KEY), "{details}");
        assert!(details.contains("[REDACTED_"), "{details}");
    }

    /// `context.md` §4: redact, *then* bound. The key starts six bytes
    /// before the bound. Bounding first would keep `AKIAIO`, too short for
    /// the AWS rule to match, and leave it in plain text. The filler is
    /// spaces so no other scanner rule can swallow the key and hide the
    /// ordering bug.
    #[test]
    fn a_secret_straddling_the_details_bound_is_redacted_not_cut() {
        let details = format!("{}{FAKE_AWS_KEY}", " ".repeat(MAX_DETAILS_BYTES - 6));
        let finding = Finding::new(draft("s", Some(&details)), &SecretScanner::default());
        let kept = finding.details().expect("details kept");
        assert!(
            !kept.contains("AKIA"),
            "a fragment of the key survived: {:?}",
            &kept[kept.len().saturating_sub(40)..]
        );
        assert!(kept.ends_with(DETAILS_TRUNCATION_MARKER));
    }

    #[test]
    fn summary_is_the_first_non_empty_line_trimmed() {
        let finding = Finding::new(
            draft("\n   \n  error: first real line  \nsecond line", None),
            &SecretScanner::default(),
        );
        assert_eq!(finding.summary(), "error: first real line");
    }

    #[test]
    fn a_long_summary_is_cut_to_the_bound_with_an_ellipsis() {
        let long = "é".repeat(MAX_SUMMARY_CHARS + 50);
        let finding = Finding::new(draft(&long, None), &SecretScanner::default());
        assert_eq!(finding.summary().chars().count(), MAX_SUMMARY_CHARS);
        assert!(finding.summary().ends_with('…'));
    }

    #[test]
    fn a_summary_at_exactly_the_bound_is_not_cut() {
        let exact = "a".repeat(MAX_SUMMARY_CHARS);
        let finding = Finding::new(draft(&exact, None), &SecretScanner::default());
        assert_eq!(finding.summary(), exact);
    }

    /// A finding is never blank, even when the source printed nothing.
    #[test]
    fn an_empty_summary_becomes_a_message_naming_the_source() {
        let mut blank = draft("  \n\t\n", None);
        blank.source = FindingSource::BrowserConsole;
        let finding = Finding::new(blank, &SecretScanner::default());
        assert_eq!(finding.summary(), "browser_console finding with no message");
    }

    #[test]
    fn details_are_bounded_on_a_char_boundary() {
        // 3-byte chars, so MAX_DETAILS_BYTES is unlikely to land on a boundary.
        let details = "€".repeat(MAX_DETAILS_BYTES);
        let finding = Finding::new(draft("s", Some(&details)), &SecretScanner::default());
        let kept = finding.details().expect("details kept");
        assert!(kept.len() <= MAX_DETAILS_BYTES + DETAILS_TRUNCATION_MARKER.len());
        let body = kept
            .strip_suffix(DETAILS_TRUNCATION_MARKER)
            .expect("marked as cut");
        assert!(body.chars().all(|c| c == '€'), "the cut split a character");
    }

    #[test]
    fn details_within_the_bound_are_kept_verbatim() {
        let details = "line one\n  line two\n";
        let finding = Finding::new(draft("s", Some(details)), &SecretScanner::default());
        assert_eq!(finding.details(), Some(details));
    }

    #[test]
    fn blank_details_become_none() {
        let finding = Finding::new(draft("s", Some("  \n  ")), &SecretScanner::default());
        assert_eq!(finding.details(), None);
    }

    #[test]
    fn builders_attach_task_origin_and_file_hash() {
        let finding = Finding::new(draft("s", None), &SecretScanner::default())
            .with_task_id("task_1")
            .with_origin_ref("cmd_1")
            .with_file_hash("sha256:abc");
        assert_eq!(finding.task_id(), Some("task_1"));
        assert_eq!(finding.origin_ref(), Some("cmd_1"));
        assert_eq!(finding.file_hash(), Some("sha256:abc"));
    }

    #[test]
    fn set_status_changes_only_the_status() {
        let mut finding = Finding::new(draft("s", None), &SecretScanner::default());
        let before = finding.clone();
        finding.set_status(FindingStatus::Dismissed);
        assert_eq!(finding.status(), FindingStatus::Dismissed);
        assert_eq!(finding.id(), before.id());
        assert_eq!(finding.summary(), before.summary());
    }

    /// The persisted and served shape (Tasks 7 and 9 depend on it).
    #[test]
    fn json_uses_camel_case_keys_and_snake_case_enum_values() {
        let finding = Finding::new(
            FindingDraft {
                source: FindingSource::BrowserConsole,
                severity: Severity::Warning,
                summary: "s".to_string(),
                details: None,
                range: Some(SourceRange {
                    path: "src/app.js".to_string(),
                    start_line: 3,
                    start_column: Some(7),
                    end_line: None,
                    end_column: None,
                }),
                code: Some("no-unused-vars".to_string()),
            },
            &SecretScanner::default(),
        )
        .with_task_id("task_1")
        .with_origin_ref("cmd_1")
        .with_file_hash("sha256:abc");

        let value = serde_json::to_value(&finding).unwrap();
        assert_eq!(value["source"], "browser_console");
        assert_eq!(value["severity"], "warning");
        assert_eq!(value["status"], "open");
        assert_eq!(value["taskId"], "task_1");
        assert_eq!(value["originRef"], "cmd_1");
        assert_eq!(value["fileHash"], "sha256:abc");
        assert_eq!(value["code"], "no-unused-vars");
        assert_eq!(value["range"]["path"], "src/app.js");
        assert_eq!(value["range"]["startLine"], 3);
        assert_eq!(value["range"]["startColumn"], 7);
        assert!(value["createdAtMs"].is_u64());

        let back: Finding = serde_json::from_value(value).unwrap();
        assert_eq!(back, finding);
    }

    #[test]
    fn every_source_serialises_as_its_as_str() {
        use FindingSource::*;
        for source in [
            Compiler,
            Test,
            Lint,
            Security,
            BrowserConsole,
            BrowserNetwork,
            CodeReview,
            LanguageServer,
        ] {
            assert_eq!(
                serde_json::to_value(source).unwrap(),
                serde_json::Value::String(source.as_str().to_string()),
            );
        }
    }

    #[test]
    fn severity_sorts_error_before_warning_before_info() {
        let mut severities = vec![Severity::Info, Severity::Error, Severity::Warning];
        severities.sort();
        assert_eq!(
            severities,
            [Severity::Error, Severity::Warning, Severity::Info]
        );
    }
}
