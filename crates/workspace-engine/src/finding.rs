//! One structured finding type shared by every check source (spec 22).
//!
//! Fields are private so that [`Finding::new`] is the only constructor,
//! and therefore the only place redaction and bounding happen
//! (`docs/specs/22_findings_model_and_panel/context.md` §2).

use crate::command_runner::{CommandExecution, CommandTermination};
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
    /// One command's failure taken whole: nothing was parsed out of it.
    /// Only the generic fallback produces this (`context.md` §8.1).
    Command,
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
            Self::Command => "command",
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

/// Lines kept from each stream for a generic finding's details.
pub const GENERIC_DETAIL_LINES: usize = 40;

/// Turns one tool's output into drafts. Returns drafts, not findings, so that
/// redaction happens in `findings_from_execution` and nowhere else
/// (`context.md` §3). Severity mapping is documented on each parser (§5.2).
pub trait FindingParser {
    /// The source this parser mostly produces. A parser may emit drafts of
    /// another source; `cargo test` emits `Compiler` drafts for a compile
    /// error (`context.md` §5).
    fn source(&self) -> FindingSource;
    /// Whether this parser recognises the output of `command`.
    fn matches(&self, command: &str) -> bool;
    /// Parse already-redacted output. Empty when nothing parsed, and must
    /// never invent a location (§5.3).
    fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft>;
}

/// The shipped parsers, in the order they are tried. Tasks 3–5 register
/// theirs here. The generic fallback is not in this list: it is applied by
/// `findings_from_execution` itself, so no caller can forget it.
pub fn default_parsers() -> Vec<Box<dyn FindingParser>> {
    Vec::new()
}

/// Whether a run reached a verdict, and which. `context.md` §8.2 has the
/// table this encodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Passed,
    Failed,
    /// Cancelled by the user before it finished.
    Unfinished,
}

fn verdict(execution: &CommandExecution) -> Verdict {
    match (execution.termination, execution.exit_code) {
        (CommandTermination::Exited, Some(0)) => Verdict::Passed,
        (CommandTermination::Exited, _) | (CommandTermination::TimedOut, _) => Verdict::Failed,
        (CommandTermination::Cancelled, _) => Verdict::Unfinished,
    }
}

/// The first parser that matches wins. A failed run whose parser found
/// nothing falls through to one generic finding, so a regex that stops
/// matching after a tool upgrade cannot swallow a failure (§5.3).
pub fn findings_from_execution(
    execution: &CommandExecution,
    parsers: &[Box<dyn FindingParser>],
    scanner: &SecretScanner,
) -> Vec<Finding> {
    let mut drafts = parsers
        .iter()
        .find(|parser| parser.matches(&execution.command))
        .map(|parser| parser.parse(execution))
        .unwrap_or_default();
    if drafts.is_empty() && verdict(execution) == Verdict::Failed {
        drafts.push(generic_draft(execution));
    }
    drafts
        .into_iter()
        .map(|draft| Finding::new(draft, scanner))
        .collect()
}

/// One finding for the whole failure, never claiming structure it did not
/// find: no range, no code (`context.md` §8.3).
fn generic_draft(execution: &CommandExecution) -> FindingDraft {
    let command = execution.command.trim();
    let summary = match (execution.termination, execution.exit_code) {
        (CommandTermination::TimedOut, _) => format!("{command} timed out"),
        (_, None) => format!("{command} was killed by a signal"),
        (_, Some(code)) => first_non_empty_line(&execution.stderr)
            .or_else(|| first_non_empty_line(&execution.stdout))
            .map(|line| format!("{command}: {line}"))
            .unwrap_or_else(|| format!("{command} exited with code {code}")),
    };
    FindingDraft {
        source: FindingSource::Command,
        severity: Severity::Error,
        summary,
        details: generic_details(execution),
        range: None,
        code: None,
    }
}

fn generic_details(execution: &CommandExecution) -> Option<String> {
    let sections: Vec<String> = [("stderr", &execution.stderr), ("stdout", &execution.stdout)]
        .into_iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(label, text)| format!("{label}:\n{}", last_lines(text, GENERIC_DETAIL_LINES)))
        .collect();
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn bound_summary(text: &str, source: FindingSource) -> String {
    let Some(line) = first_non_empty_line(text) else {
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
            Command,
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

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::command_policy::CommandRisk;
    use crate::command_runner::{CommandExecution, CommandTermination};
    use crate::secret_scanner::SecretScanner;

    fn execution(
        command: &str,
        termination: CommandTermination,
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
    ) -> CommandExecution {
        CommandExecution {
            id: "cmd_test".to_string(),
            command: command.to_string(),
            working_directory: "/repo".to_string(),
            risk: CommandRisk::Low,
            approved_by: None,
            started_at_ms: 1,
            completed_at_ms: 2,
            exit_code,
            termination,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn failed(command: &str, stdout: &str, stderr: &str) -> CommandExecution {
        execution(command, CommandTermination::Exited, Some(1), stdout, stderr)
    }

    fn passed(command: &str) -> CommandExecution {
        execution(command, CommandTermination::Exited, Some(0), "ok\n", "")
    }

    fn draft(source: FindingSource, severity: Severity, summary: &str) -> FindingDraft {
        FindingDraft {
            source,
            severity,
            summary: summary.to_string(),
            details: None,
            range: None,
            code: None,
        }
    }

    /// Matches commands starting with `prefix` and returns fixed drafts.
    struct Stub {
        prefix: &'static str,
        drafts: Vec<FindingDraft>,
    }

    impl FindingParser for Stub {
        fn source(&self) -> FindingSource {
            FindingSource::Test
        }
        fn matches(&self, command: &str) -> bool {
            command.starts_with(self.prefix)
        }
        fn parse(&self, _execution: &CommandExecution) -> Vec<FindingDraft> {
            self.drafts.clone()
        }
    }

    /// Never matches, and fails the test if asked to parse.
    struct NeverMatches;

    impl FindingParser for NeverMatches {
        fn source(&self) -> FindingSource {
            FindingSource::Lint
        }
        fn matches(&self, _command: &str) -> bool {
            false
        }
        fn parse(&self, _execution: &CommandExecution) -> Vec<FindingDraft> {
            panic!("a parser that does not match was asked to parse")
        }
    }

    fn stub(prefix: &'static str, drafts: Vec<FindingDraft>) -> Box<dyn FindingParser> {
        Box::new(Stub { prefix, drafts })
    }

    fn run(execution: &CommandExecution, parsers: &[Box<dyn FindingParser>]) -> Vec<Finding> {
        findings_from_execution(execution, parsers, &SecretScanner::default())
    }

    #[test]
    fn a_failure_no_parser_matches_yields_exactly_one_generic_finding() {
        let findings = run(
            &failed("pytest", "", "\n  first line  \nsecond line\n"),
            &[],
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        let finding = &findings[0];
        assert_eq!(finding.source(), FindingSource::Command);
        assert_eq!(finding.severity(), Severity::Error);
        assert_eq!(finding.summary(), "pytest: first line");
        assert_eq!(
            finding.range(),
            None,
            "the generic parser invented a location"
        );
        assert_eq!(finding.code(), None);
    }

    /// The rule proposal §5.3 calls "most likely to be omitted".
    #[test]
    fn a_matching_parser_that_extracts_nothing_falls_through_to_generic() {
        let parsers = [stub("cargo test", vec![])];
        let findings = run(&failed("cargo test", "", "something new\n"), &parsers);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].source(), FindingSource::Command);
        assert_eq!(findings[0].summary(), "cargo test: something new");
    }

    #[test]
    fn parsed_drafts_replace_the_generic_finding() {
        let parsers = [stub(
            "cargo test",
            vec![
                draft(FindingSource::Test, Severity::Error, "a failed"),
                draft(FindingSource::Compiler, Severity::Error, "b failed"),
            ],
        )];
        let findings = run(&failed("cargo test", "", "boom\n"), &parsers);
        let summaries: Vec<_> = findings.iter().map(Finding::summary).collect();
        assert_eq!(summaries, ["a failed", "b failed"]);
        assert!(
            findings
                .iter()
                .all(|f| f.source() != FindingSource::Command)
        );
    }

    #[test]
    fn the_first_matching_parser_wins() {
        let parsers = [
            stub(
                "cargo",
                vec![draft(FindingSource::Compiler, Severity::Error, "first")],
            ),
            stub(
                "cargo test",
                vec![draft(FindingSource::Test, Severity::Error, "second")],
            ),
        ];
        let findings = run(&failed("cargo test", "", "x\n"), &parsers);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].summary(), "first");
    }

    #[test]
    fn a_parser_that_does_not_match_is_never_asked() {
        let parsers: [Box<dyn FindingParser>; 2] = [
            Box::new(NeverMatches),
            stub(
                "npm",
                vec![draft(FindingSource::Lint, Severity::Error, "lint")],
            ),
        ];
        let findings = run(&failed("npm run lint", "", "x\n"), &parsers);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].summary(), "lint");
    }

    #[test]
    fn a_passing_execution_with_no_parser_yields_nothing() {
        assert!(run(&passed("pytest"), &[]).is_empty());
    }

    #[test]
    fn a_passing_execution_keeps_its_parsers_warnings() {
        let parsers = [stub(
            "cargo clippy",
            vec![draft(
                FindingSource::Compiler,
                Severity::Warning,
                "unused import",
            )],
        )];
        let findings = run(&passed("cargo clippy"), &parsers);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity(), Severity::Warning);
    }

    #[test]
    fn a_passing_execution_whose_parser_finds_nothing_yields_nothing() {
        let parsers = [stub("cargo test", vec![])];
        assert!(run(&passed("cargo test"), &parsers).is_empty());
    }

    #[test]
    fn a_timeout_is_a_failure_even_without_an_exit_code() {
        let timed_out = execution(
            "cargo test",
            CommandTermination::TimedOut,
            None,
            "running 3 tests\n",
            "",
        );
        let findings = run(&timed_out, &[]);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].summary(), "cargo test timed out");
        assert!(findings[0].details().unwrap().contains("running 3 tests"));
    }

    /// `Exited` with no code is a signal. It is not a zero, and it must never
    /// be read as one (the same rule as `Evidence::CommandExit`).
    #[test]
    fn a_signal_kill_is_a_failure() {
        let killed = execution("make check", CommandTermination::Exited, None, "", "");
        let findings = run(&killed, &[]);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].summary(), "make check was killed by a signal");
    }

    /// `context.md` §8.2: a check the user stopped has no verdict to repair.
    #[test]
    fn a_cancelled_execution_gets_no_generic_finding() {
        let cancelled = execution(
            "cargo test",
            CommandTermination::Cancelled,
            None,
            "",
            "Compiling x\n",
        );
        assert!(run(&cancelled, &[]).is_empty());
        assert!(run(&cancelled, &[stub("cargo test", vec![])]).is_empty());
    }

    #[test]
    fn a_cancelled_execution_keeps_what_its_parser_found() {
        let cancelled = execution("cargo test", CommandTermination::Cancelled, None, "", "");
        let parsers = [stub(
            "cargo test",
            vec![draft(FindingSource::Compiler, Severity::Error, "E0308")],
        )];
        let findings = run(&cancelled, &parsers);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].summary(), "E0308");
    }

    #[test]
    fn the_generic_summary_falls_back_to_stdout_then_to_the_exit_code() {
        let from_stdout = run(&failed("pytest", "\nFAILED test_x\n", "  \n"), &[]);
        assert_eq!(from_stdout[0].summary(), "pytest: FAILED test_x");

        let silent = execution("pytest", CommandTermination::Exited, Some(2), "", "");
        let from_code = run(&silent, &[]);
        assert_eq!(from_code[0].summary(), "pytest exited with code 2");
        assert_eq!(from_code[0].details(), None);
    }

    #[test]
    fn the_generic_details_keep_the_last_lines_of_each_stream() {
        let stderr: String = (0..100).map(|n| format!("err {n:03}\n")).collect();
        let findings = run(&failed("pytest", "out a\nout b\n", &stderr), &[]);
        let details = findings[0].details().expect("details");
        assert!(details.starts_with("stderr:\n"), "{details}");
        assert!(details.contains("err 060"), "the tail was not kept");
        assert!(details.contains("err 099"));
        assert!(
            !details.contains("err 059"),
            "more than {GENERIC_DETAIL_LINES} lines kept"
        );
        assert!(details.contains("\n\nstdout:\nout a\nout b"), "{details}");
    }

    #[test]
    fn an_empty_stream_is_left_out_of_the_generic_details() {
        let findings = run(&failed("pytest", "only stdout\n", "   \n"), &[]);
        assert_eq!(findings[0].details(), Some("stdout:\nonly stdout"));
    }

    /// Acceptance criterion: no unredacted secret in "a generic fallback".
    /// The runner already redacts command output (`context.md` §6). This
    /// test proves the dispatcher goes through `Finding::new` anyway.
    #[test]
    fn a_secret_in_failed_output_is_redacted_in_the_generic_finding() {
        let key = "AKIAIOSFODNN7EXAMPLE";
        let findings = run(&failed("deploy", "", &format!("bad key {key}\n")), &[]);
        assert!(
            !findings[0].summary().contains(key),
            "{}",
            findings[0].summary()
        );
        assert!(!findings[0].details().unwrap().contains(key));
    }
}
