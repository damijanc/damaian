//! `cargo test` failures (spec 22 Task 4).
//!
//! Compiler diagnostics on stderr come first, through
//! `parse_rust_diagnostics` and its own severity mapping (`context.md` §5).
//! Then each name in a `failures:` list on stdout is one `Test` finding of
//! severity `Error`: libtest reports no other level, and no code. Every rule
//! here comes from real output recorded in
//! `docs/specs/22_findings_model_and_panel/context.md` §10.

use super::rust_diagnostics::{cargo_subcommand, parse_rust_diagnostics, regex, workspace_range};
use super::{FindingDraft, FindingParser, FindingSource, Severity, SourceRange};
use crate::command_runner::CommandExecution;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

static SECTION: LazyLock<Regex> = LazyLock::new(|| regex(r"^---- (.+) stdout ----$"));
static PANICKED: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^thread '[^']*'(?: \(\d+\))? panicked at (.+):(\d+):(\d+):$"));
static DID_NOT_PANIC: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^note: test did not panic as expected at (.+):(\d+):(\d+)$"));
static DOCTEST_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\S+) - .+ \(line (\d+)\)$"));

pub(super) struct RustTestParser;

impl FindingParser for RustTestParser {
    fn source(&self) -> FindingSource {
        FindingSource::Test
    }

    fn matches(&self, command: &str) -> bool {
        matches!(cargo_subcommand(command), Some("test" | "t"))
    }

    /// Both, not either: a build that printed a warning can still fail its
    /// tests (`context.md` §10).
    fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft> {
        let mut drafts = parse_rust_diagnostics(&execution.stderr);
        drafts.extend(parse_test_failures(&execution.stdout));
        drafts
    }
}

/// One draft per name in each `failures:` list. A list reads only the
/// `---- name stdout ----` sections printed since the previous list, because
/// `--no-fail-fast` prints one block per test binary and names can repeat.
pub(super) fn parse_test_failures(stdout: &str) -> Vec<FindingDraft> {
    let lines: Vec<&str> = stdout.lines().collect();
    let mut sections: HashMap<&str, &[&str]> = HashMap::new();
    let mut drafts = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if let Some(header) = SECTION.captures(lines[index]) {
            let end = lines[index + 1..]
                .iter()
                .position(|line| SECTION.is_match(line) || *line == "failures:")
                .map_or(lines.len(), |offset| index + 1 + offset);
            if let Some(name) = header.get(1) {
                sections.insert(name.as_str(), &lines[index..end]);
            }
            index = end;
        } else if lines[index] == "failures:" {
            let names: Vec<&str> = lines[index + 1..]
                .iter()
                .take_while(|line| line.starts_with("    ") && !line.trim().is_empty())
                .map(|line| line.trim())
                .collect();
            index += 1 + names.len();
            if !names.is_empty() {
                for name in names {
                    drafts.push(test_draft(name, sections.get(name).copied()));
                }
                sections.clear();
            }
        } else {
            index += 1;
        }
    }
    drafts
}

fn test_draft(name: &str, section: Option<&[&str]>) -> FindingDraft {
    let body = section.map_or(&[][..], |lines| &lines[1..]);
    FindingDraft {
        source: FindingSource::Test,
        severity: Severity::Error,
        summary: match failure_message(body) {
            Some(message) => format!("{name}: {message}"),
            None => format!("{name} failed"),
        },
        details: section.map(|lines| lines.join("\n").trim_end().to_string()),
        range: failure_location(body).or_else(|| doctest_location(name)),
        code: None,
    }
}

/// libtest's `#[should_panic]` notes say why the test failed better than the
/// panic message does. Then the panic message, which is the line after
/// `panicked at`, then a returned `Err`.
fn failure_message<'a>(body: &[&'a str]) -> Option<&'a str> {
    let should_panic_note = body.iter().find_map(|line| {
        line.strip_prefix("note: ").filter(|note| {
            note.starts_with("test did not panic as expected")
                || note.starts_with("panic did not contain expected string")
        })
    });
    let panic_message = || {
        let at = body.iter().position(|line| PANICKED.is_match(line))?;
        body.get(at + 1)
            .copied()
            .filter(|line| !line.trim().is_empty())
    };
    should_panic_note
        .or_else(panic_message)
        .or_else(|| {
            body.iter()
                .copied()
                .find(|line| line.starts_with("Error: "))
        })
        .map(str::trim)
}

/// The first `panicked at` or `did not panic … at`, when it is inside the
/// workspace. A doctest panics in a temporary file, which is not.
fn failure_location(body: &[&str]) -> Option<SourceRange> {
    let captures = body.iter().find_map(|line| {
        PANICKED
            .captures(line)
            .or_else(|| DID_NOT_PANIC.captures(line))
    })?;
    workspace_range(&captures[1], &captures[2], Some(&captures[3]))
}

/// A doctest's name is `path - item (line N)`: the doc block's location as
/// rustdoc printed it.
fn doctest_location(name: &str) -> Option<SourceRange> {
    let captures = DOCTEST_NAME.captures(name)?;
    workspace_range(&captures[1], &captures[2], None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_policy::CommandRisk;
    use crate::command_runner::{CommandExecution, CommandTermination};
    use crate::finding::{default_parsers, findings_from_execution};
    use crate::secret_scanner::SecretScanner;

    const COMPILE_ERROR_STDERR: &str = include_str!("fixtures/rust_test_compile_error_stderr.txt");
    const FAILURES_STDOUT: &str = include_str!("fixtures/rust_test_failures_stdout.txt");
    const FAILURES_STDERR: &str = include_str!("fixtures/rust_test_failures_stderr.txt");
    const CRASH_STDERR: &str = include_str!("fixtures/rust_test_crash_stderr.txt");
    /// The crash capture's whole stdout: the binary died before reporting.
    const CRASH_STDOUT: &str = "\nrunning 1 test\n";

    fn range(path: &str, line: u32, column: Option<u32>) -> Option<SourceRange> {
        Some(SourceRange {
            path: path.to_string(),
            start_line: line,
            start_column: column,
            end_line: None,
            end_column: None,
        })
    }

    fn execution(command: &str, exit_code: i32, stdout: &str, stderr: &str) -> CommandExecution {
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
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn failure<'a>(drafts: &'a [FindingDraft], name: &str) -> &'a FindingDraft {
        drafts
            .iter()
            .find(|draft| draft.summary.starts_with(&format!("{name}: ")))
            .unwrap_or_else(|| panic!("no finding for {name}: {drafts:#?}"))
    }

    /// `context.md` §5: a compile error has no `failures:` block, so the
    /// diagnostics parser is what reads it.
    #[test]
    fn a_compile_error_under_cargo_test_is_a_compiler_finding_with_its_location() {
        let drafts = RustTestParser.parse(&execution("cargo test", 101, "", COMPILE_ERROR_STDERR));
        assert_eq!(drafts.len(), 1, "{drafts:#?}");
        assert_eq!(drafts[0].source, FindingSource::Compiler);
        assert_eq!(drafts[0].severity, Severity::Error);
        assert_eq!(drafts[0].code.as_deref(), Some("E0308"));
        assert_eq!(
            drafts[0].range,
            range("crates/demo/src/lib.rs", 11, Some(22))
        );
    }

    /// libtest sorts the `failures:` list by name, so the order is stable
    /// even though the sections above it come in thread-finishing order.
    #[test]
    fn each_failed_test_is_one_test_error_in_failures_list_order() {
        let drafts = parse_test_failures(FAILURES_STDOUT);
        let names: Vec<_> = drafts
            .iter()
            .map(|draft| draft.summary.split(": ").next().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            [
                "tests::assert_eq_fails",
                "tests::panicked_with_the_wrong_message",
                "tests::panics_with_message",
                "tests::returns_err",
                "tests::should_have_panicked",
                "tests::unwrap_in_library_code",
                "integration_fails",
                "crates/demo/src/lib.rs - add (line 3)",
            ]
        );
        assert!(drafts.iter().all(|d| d.source == FindingSource::Test));
        assert!(drafts.iter().all(|d| d.severity == Severity::Error));
        assert!(drafts.iter().all(|d| d.code.is_none()));
    }

    /// The test printed a line before panicking, so the message is the line
    /// after `panicked at`, not the section's first line.
    #[test]
    fn a_panic_takes_its_location_and_message() {
        let drafts = parse_test_failures(FAILURES_STDOUT);
        let panic = failure(&drafts, "tests::panics_with_message");
        assert_eq!(
            panic.summary,
            "tests::panics_with_message: the widget was not ready"
        );
        assert_eq!(panic.range, range("crates/demo/src/lib.rs", 27, Some(9)));
    }

    #[test]
    fn an_assert_eq_failure_keeps_its_values_in_the_details() {
        let drafts = parse_test_failures(FAILURES_STDOUT);
        let assert = failure(&drafts, "tests::assert_eq_fails");
        assert_eq!(
            assert.summary,
            "tests::assert_eq_fails: assertion `left == right` failed: two plus two"
        );
        assert_eq!(assert.range, range("crates/demo/src/lib.rs", 32, Some(9)));
        let details = assert.details.as_deref().expect("details");
        assert!(
            details.starts_with("---- tests::assert_eq_fails stdout ----"),
            "{details}"
        );
        assert!(details.contains("\n  left: 4\n right: 5"), "{details}");
    }

    /// Its section runs straight into the next one with no blank line
    /// (`context.md` §10).
    #[test]
    fn a_should_panic_that_did_not_panic_points_at_the_test() {
        let drafts = parse_test_failures(FAILURES_STDOUT);
        let missed = failure(&drafts, "tests::should_have_panicked");
        assert_eq!(
            missed.summary,
            "tests::should_have_panicked: test did not panic as expected at crates/demo/src/lib.rs:42:8"
        );
        assert_eq!(missed.range, range("crates/demo/src/lib.rs", 42, Some(8)));
        let details = missed.details.as_deref().expect("details");
        assert!(
            !details.contains("underflow"),
            "the next section leaked in: {details}"
        );
    }

    /// The panic message, `underflow`, is not why the test failed.
    #[test]
    fn a_wrong_panic_message_is_summarised_by_the_note_not_the_panic() {
        let drafts = parse_test_failures(FAILURES_STDOUT);
        let wrong = failure(&drafts, "tests::panicked_with_the_wrong_message");
        assert_eq!(
            wrong.summary,
            "tests::panicked_with_the_wrong_message: panic did not contain expected string"
        );
        assert_eq!(wrong.range, range("crates/demo/src/lib.rs", 49, Some(9)));
    }

    /// A test returning `Err` prints no location. It must not get one.
    #[test]
    fn a_test_returning_err_has_no_range() {
        let drafts = parse_test_failures(FAILURES_STDOUT);
        let err = failure(&drafts, "tests::returns_err");
        assert_eq!(err.summary, "tests::returns_err: Error: \"config missing\"");
        assert_eq!(err.range, None);
    }

    /// `#[track_caller]` puts the location in the library function the test
    /// called. That is where it panicked, so that is the range.
    #[test]
    fn a_panic_in_library_code_points_where_it_panicked() {
        let drafts = parse_test_failures(FAILURES_STDOUT);
        let unwrap = failure(&drafts, "tests::unwrap_in_library_code");
        assert_eq!(unwrap.range, range("crates/demo/src/lib.rs", 11, Some(15)));
    }

    #[test]
    fn a_failure_in_an_integration_test_binary_is_found_too() {
        let drafts = parse_test_failures(FAILURES_STDOUT);
        let integration = failure(&drafts, "integration_fails");
        assert_eq!(integration.summary, "integration_fails: sum too small");
        assert_eq!(
            integration.range,
            range("crates/demo/tests/integration.rs", 8, Some(5))
        );
    }

    /// A doctest panics in a temporary file, which is not the user's code.
    /// Its name carries the path and line of the doc block instead.
    #[test]
    fn a_doctest_points_at_its_doc_block_not_the_temporary_file() {
        let drafts = parse_test_failures(FAILURES_STDOUT);
        let doctest = failure(&drafts, "crates/demo/src/lib.rs - add (line 3)");
        assert_eq!(
            doctest.summary,
            "crates/demo/src/lib.rs - add (line 3): assertion `left == right` failed"
        );
        assert_eq!(doctest.range, range("crates/demo/src/lib.rs", 3, None));
    }

    /// `context.md` §6: tail truncation can remove a section and keep the
    /// list. The name is still a finding. The location is not guessed.
    #[test]
    fn a_failure_whose_section_was_cut_off_keeps_its_name_and_no_range() {
        let stdout = "failures:\n    tests::lost\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        let drafts = parse_test_failures(stdout);
        assert_eq!(drafts.len(), 1, "{drafts:#?}");
        assert_eq!(drafts[0].summary, "tests::lost failed");
        assert_eq!(drafts[0].range, None);
        assert_eq!(drafts[0].details, None);
    }

    /// Each binary's list reads only that binary's sections. The second
    /// binary's `tests::same` printed no section, so it must not borrow the
    /// first binary's location.
    #[test]
    fn a_name_repeated_in_a_later_binary_does_not_borrow_an_earlier_section() {
        let stdout = "\
failures:

---- tests::same stdout ----

thread 'tests::same' (1) panicked at crates/a/src/lib.rs:4:9:
first


failures:
    tests::same

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 1 test
test tests::same ... FAILED

failures:
    tests::same

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";
        let drafts = parse_test_failures(stdout);
        assert_eq!(drafts.len(), 2, "{drafts:#?}");
        assert_eq!(drafts[0].range, range("crates/a/src/lib.rs", 4, Some(9)));
        assert_eq!(drafts[1].range, None);
        assert_eq!(drafts[1].summary, "tests::same failed");
    }

    /// cargo 1.98.0 prints a thread id in brackets. Earlier toolchains print
    /// the same line without it.
    #[test]
    fn a_panic_line_without_a_thread_id_still_has_its_location() {
        let stdout = "failures:\n\n---- tests::old stdout ----\n\nthread 'tests::old' panicked at src/lib.rs:5:9:\nboom\n\n\nfailures:\n    tests::old\n";
        let drafts = parse_test_failures(stdout);
        assert_eq!(drafts.len(), 1, "{drafts:#?}");
        assert_eq!(drafts[0].summary, "tests::old: boom");
        assert_eq!(drafts[0].range, range("src/lib.rs", 5, Some(9)));
    }

    #[test]
    fn a_passing_run_has_no_failures() {
        let stdout = "\nrunning 1 test\ntest tests::adds ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        assert!(parse_test_failures(stdout).is_empty());
    }

    /// `context.md` §10: cargo's per-target lines on `cargo test`'s stderr use
    /// the diagnostic header shape, and are summaries, not diagnostics.
    #[test]
    fn cargo_test_target_lines_on_stderr_are_not_findings() {
        let drafts = parse_rust_diagnostics(FAILURES_STDERR);
        let summaries: Vec<_> = drafts.iter().map(|d| d.summary.as_str()).collect();
        assert_eq!(summaries, ["unused variable: `unused`"]);
    }

    /// Registration, end to end. The compiler warning on stderr must not
    /// stand in for the test failures on stdout (`context.md` §10 corrects
    /// §5's "uses its drafts when it returns any").
    #[test]
    fn default_parsers_turn_a_failed_cargo_test_into_compiler_and_test_findings() {
        let findings = findings_from_execution(
            &execution(
                "cargo test --no-fail-fast",
                101,
                FAILURES_STDOUT,
                FAILURES_STDERR,
            ),
            &default_parsers(),
            &SecretScanner::default(),
        );
        let sources: Vec<_> = findings.iter().map(|f| f.source()).collect();
        let mut expected = vec![FindingSource::Compiler];
        expected.extend([FindingSource::Test; 8]);
        assert_eq!(sources, expected, "{findings:#?}");
        assert_eq!(findings[0].severity(), Severity::Warning);
    }

    /// The crash capture: a warning, then a test binary killed by SIGABRT
    /// before it printed a `failures:` list. The warning alone must not
    /// swallow the failure (`context.md` §10).
    #[test]
    fn a_crashed_test_binary_with_a_warning_still_gets_the_generic_finding() {
        let findings = findings_from_execution(
            &execution("cargo test", 101, CRASH_STDOUT, CRASH_STDERR),
            &default_parsers(),
            &SecretScanner::default(),
        );
        assert_eq!(findings.len(), 2, "{findings:#?}");
        assert_eq!(findings[0].code(), Some("unused_variables"));
        assert_eq!(findings[0].severity(), Severity::Warning);
        assert_eq!(findings[1].source(), FindingSource::Command);
        assert!(findings[1].details().unwrap().contains("SIGABRT"));
    }

    /// §5.3's fall-through, against this parser.
    #[test]
    fn a_failed_cargo_test_this_parser_cannot_read_falls_through_to_generic() {
        let findings = findings_from_execution(
            &execution("cargo test", 101, "\nrunning 3 tests\n", "Killed: 9\n"),
            &default_parsers(),
            &SecretScanner::default(),
        );
        assert_eq!(findings.len(), 1, "{findings:#?}");
        assert_eq!(findings[0].source(), FindingSource::Command);
        assert_eq!(findings[0].summary(), "cargo test: Killed: 9");
    }

    #[test]
    fn a_passing_cargo_test_yields_nothing() {
        let stdout = "\nrunning 1 test\ntest tests::adds ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        let stderr = "   Compiling demo v0.1.0 (/repo/crates/demo)\n     Running unittests src/lib.rs (target/debug/deps/demo-379d40ceacbd0d01)\n   Doc-tests demo\n";
        let findings = findings_from_execution(
            &execution("cargo test", 0, stdout, stderr),
            &default_parsers(),
            &SecretScanner::default(),
        );
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn matches_cargo_test_in_its_common_forms() {
        for command in [
            "cargo test",
            "cargo t",
            "cargo test --workspace --no-fail-fast",
            "cargo +nightly test -p demo",
            "RUST_BACKTRACE=1 cargo test",
            "cd crates/demo && cargo test",
        ] {
            assert!(RustTestParser.matches(command), "{command}");
        }
    }

    /// `cargo nextest run` is out of scope (`context.md` §10): no parser
    /// matches it, so a failure reaches the generic fallback.
    #[test]
    fn does_not_match_nextest_or_other_commands() {
        for command in [
            "cargo nextest run",
            "cargo build",
            "cargo bench",
            "npm test",
            "cargo",
        ] {
            assert!(!RustTestParser.matches(command), "{command}");
        }
        let findings = findings_from_execution(
            &execution(
                "cargo nextest run",
                100,
                "",
                "        FAIL [   0.013s] (1/8) demo tests::panics_with_message\nerror: test run failed\n",
            ),
            &default_parsers(),
            &SecretScanner::default(),
        );
        assert_eq!(findings.len(), 1, "{findings:#?}");
        assert_eq!(findings[0].source(), FindingSource::Command);
    }
}
