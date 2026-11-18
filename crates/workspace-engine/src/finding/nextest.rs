//! `cargo nextest run` failures (spec 109 Task 4).
//!
//! nextest prints everything on stderr: compiler diagnostics first (read by
//! `parse_rust_diagnostics`), then one status line per test, each failing
//! test's captured output indented by four spaces under it, and a closing
//! `Summary` that lists the failures again. Each failing test is one `Test`
//! finding shaped exactly like a `cargo test` one, so a failed-test summary
//! names its test the same way for both runners. Every rule here comes from
//! the real output in `fixtures/nextest_failures_stderr.txt` (nextest 0.9.144,
//! captured without a terminal, as the command runner runs it).

use super::rust_diagnostics::{cargo_subcommand, parse_rust_diagnostics, regex};
use super::rust_test::named_test_draft;
use super::{FindingDraft, FindingParser, FindingSource};
use crate::command_runner::CommandExecution;
use regex::Regex;
use std::collections::BTreeSet;
use std::sync::LazyLock;

/// A test's status line: the status word, its duration, an optional
/// `(n/total)`, the binary id, and the test name. Only the failing words.
static FAILED_STATUS: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"^\s+(?:FAIL|TIMEOUT|ABORT|SIG[A-Z0-9]+) \[[^\]]*\](?: \(\s*\d+/\d+\))? (\S+) (.+)$")
});
/// Any status line, failing or not: where one test's captured output ends.
static ANY_STATUS: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s+[A-Z][A-Z0-9-]* \[[^\]]*\]( |$)"));
static ANSI: LazyLock<Regex> = LazyLock::new(|| regex(r"\x1b\[[0-9;]*m"));

pub(super) struct NextestParser;

impl FindingParser for NextestParser {
    fn source(&self) -> FindingSource {
        FindingSource::Test
    }

    fn matches(&self, command: &str) -> bool {
        is_nextest_run(command)
    }

    fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft> {
        let mut drafts = parse_rust_diagnostics(&execution.stderr);
        drafts.extend(parse_nextest_failures(&execution.stderr));
        drafts
    }
}

/// `cargo nextest run` (or its `r` alias) in a shell command line.
pub(super) fn is_nextest_run(command: &str) -> bool {
    if cargo_subcommand(command) != Some("nextest") {
        return false;
    }
    let mut tokens = command.split_whitespace();
    tokens.find(|token| *token == "nextest");
    tokens
        .find(|token| !token.starts_with('-'))
        .is_some_and(|token| matches!(token, "run" | "r"))
}

/// One draft per failing test, in the order nextest first reported it. A test
/// listed again in the closing summary is the same failure, not a second one.
pub(super) fn parse_nextest_failures(stderr: &str) -> Vec<FindingDraft> {
    let clean = ANSI.replace_all(stderr, "");
    let lines: Vec<&str> = clean.lines().collect();
    let mut seen = BTreeSet::new();
    let mut drafts = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(captures) = FAILED_STATUS.captures(line) else {
            continue;
        };
        let binary = captures[1].to_string();
        let name = captures[2].trim().to_string();
        if !seen.insert((binary, name.clone())) {
            continue;
        }
        let block: Vec<&str> = lines[index + 1..]
            .iter()
            .take_while(|line| !ANY_STATUS.is_match(line) && !line.starts_with('─'))
            .map(|line| line.strip_prefix("    ").unwrap_or(line))
            .collect();
        let details = block.join("\n").trim().to_string();
        drafts.push(named_test_draft(
            &name,
            &block,
            (!details.is_empty()).then_some(details),
        ));
    }
    drafts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_policy::CommandRisk;
    use crate::command_runner::CommandTermination;
    use crate::finding::{SourceRange, default_parsers, failed_test_name, findings_from_execution};
    use crate::secret_scanner::SecretScanner;

    const FAILURES_STDERR: &str = include_str!("fixtures/nextest_failures_stderr.txt");

    fn execution(command: &str, exit_code: i32, stderr: &str) -> CommandExecution {
        CommandExecution {
            id: "cmd_test".to_string(),
            command: command.to_string(),
            working_directory: "/repo".to_string(),
            risk: CommandRisk::Low,
            approved_by: None,
            started_at_ms: 1,
            completed_at_ms: 2,
            exit_code: Some(exit_code),
            termination: CommandTermination::Exited,
            stdout: String::new(),
            stderr: stderr.to_string(),
            purpose: crate::command_purpose::CommandPurpose::Other,
        }
    }

    #[test]
    fn each_failing_test_is_one_test_error_named_like_cargo_test() {
        let drafts = parse_nextest_failures(FAILURES_STDERR);
        let names: Vec<_> = drafts
            .iter()
            .map(|draft| failed_test_name(&draft.summary).unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            [
                "tests::fails_one",
                "tests::fails_one_more",
                "tests::fails_two",
                "integration_fails",
            ]
        );
        let first = &drafts[0];
        assert_eq!(first.source, FindingSource::Test);
        assert_eq!(
            first.summary,
            "tests::fails_one: assertion `left == right` failed: one"
        );
        assert_eq!(
            first.range,
            Some(SourceRange {
                path: "src/lib.rs".to_string(),
                start_line: 5,
                start_column: Some(30),
                end_line: None,
                end_column: None,
            })
        );
        let details = first.details.as_deref().expect("details");
        assert!(details.contains("left: 2"), "{details}");
        assert!(
            !details.contains("prefix sibling"),
            "the next test's output leaked in: {details}"
        );
        assert_eq!(drafts[3].summary, "integration_fails: sum too small");
    }

    /// The closing `error: test run failed` is nextest's summary, not a
    /// compiler error; only the test failures explain the run.
    #[test]
    fn default_parsers_read_a_failed_nextest_run_as_test_findings_only() {
        let findings = findings_from_execution(
            &execution("cargo nextest run --no-fail-fast", 100, FAILURES_STDERR),
            &default_parsers(),
            &SecretScanner::default(),
        );
        let sources: Vec<_> = findings.iter().map(|finding| finding.source()).collect();
        assert_eq!(sources, [FindingSource::Test; 4], "{findings:#?}");
    }

    #[test]
    fn matches_nextest_run_in_its_common_forms_only() {
        for command in [
            "cargo nextest run",
            "cargo nextest r",
            "cargo nextest run -p demo --test a",
            "cargo +nightly nextest run",
        ] {
            assert!(is_nextest_run(command), "{command}");
        }
        for command in [
            "cargo nextest list",
            "cargo test",
            "cargo nextest",
            "nextest run",
        ] {
            assert!(!is_nextest_run(command), "{command}");
        }
    }

    #[test]
    fn a_passing_run_has_no_failures() {
        let stderr = "    Starting 1 test across 1 binary\n        PASS [   0.004s] (1/1) demo tests::passes\n────────────\n     Summary [   0.004s] 1 test run: 1 passed, 0 skipped\n";
        assert!(parse_nextest_failures(stderr).is_empty());
    }
}
