//! `cargo build` / `check` / `clippy` diagnostics (spec 22 Task 3).
//!
//! Severity is the header word as printed: `error` is `Error` and `warning`
//! is `Warning`. Under `-D warnings` a lint prints as `error` and is recorded
//! as one (proposal §5.2). A `clippy::` code makes the finding `Lint`, and
//! anything else is `Compiler`. Every rule here comes from real output
//! recorded in `docs/specs/22_findings_model_and_panel/context.md` §9.

use super::{FindingDraft, FindingParser, FindingSource, Severity, SourceRange};
use crate::command_runner::CommandExecution;
use regex::Regex;
use std::path::{Component, Path};
use std::sync::LazyLock;

pub(super) fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid built-in pattern")
}

static HEADER: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^(error|warning)(?:\[([A-Z]\d{4})\])?: (.+)$"));
static LOCATION: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*--> (.+):(\d+):(\d+)$"));
static CHILD_HEADER: LazyLock<Regex> = LazyLock::new(|| regex(r"^(note|help): "));
static CLIPPY_URL: LazyLock<Regex> = LazyLock::new(|| regex(r"rust-clippy/\S*#([a-z0-9_]+)"));
static LINT_ATTRIBUTE: LazyLock<Regex> =
    LazyLock::new(|| regex(r"#\[(?:warn|deny|forbid)\(([a-z0-9_:]+)\)\]"));
static DENY_FLAG: LazyLock<Regex> = LazyLock::new(|| regex(r"`-D ([a-z0-9_:-]+)` implied by"));
static GENERATED: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^`[^`]+` \([^)]*\) generated \d+ warnings?"));
static EMITTED: LazyLock<Regex> = LazyLock::new(|| regex(r"^\d+ warnings? emitted"));
static TARGETS_FAILED: LazyLock<Regex> = LazyLock::new(|| regex(r"^\d+ targets? failed"));
static ANSI: LazyLock<Regex> = LazyLock::new(|| regex(r"\x1b\[[0-9;]*m"));

pub(super) struct RustDiagnosticsParser;

impl FindingParser for RustDiagnosticsParser {
    fn source(&self) -> FindingSource {
        FindingSource::Compiler
    }

    fn matches(&self, command: &str) -> bool {
        matches!(
            cargo_subcommand(command),
            Some("build" | "b" | "check" | "c" | "clippy")
        )
    }

    fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft> {
        parse_rust_diagnostics(&execution.stderr)
    }
}

/// The subcommand of the first `cargo` in a shell command line, skipping a
/// `+toolchain` and flags. Environment assignments and `cd … &&` before it
/// are just tokens that are passed over.
pub(super) fn cargo_subcommand(command: &str) -> Option<&str> {
    let mut tokens = command.split_whitespace();
    tokens.find(|token| *token == "cargo" || token.ends_with("/cargo"))?;
    tokens.find(|token| !token.starts_with('+') && !token.starts_with('-'))
}

/// rustc and clippy diagnostics in any text. A block runs from a column-0
/// `error`/`warning` header to the next blank line or header (`context.md` §9).
pub(super) fn parse_rust_diagnostics(output: &str) -> Vec<FindingDraft> {
    let clean = ANSI.replace_all(output, "");
    let lines: Vec<&str> = clean.lines().collect();
    let mut drafts = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let Some(header) = HEADER.captures(lines[index]) else {
            index += 1;
            continue;
        };
        let end = lines[index + 1..]
            .iter()
            .position(|line| line.trim().is_empty() || HEADER.is_match(line))
            .map_or(lines.len(), |offset| index + 1 + offset);
        let block = &lines[index..end];
        index = end;

        let message = header[3].trim();
        if is_cargo_summary(message) {
            continue;
        }
        let code = header
            .get(2)
            .map(|code| code.as_str().to_string())
            .or_else(|| lint_code(block));
        let source = if code
            .as_deref()
            .is_some_and(|code| code.starts_with("clippy::"))
        {
            FindingSource::Lint
        } else {
            FindingSource::Compiler
        };
        drafts.push(FindingDraft {
            source,
            severity: if &header[1] == "error" {
                Severity::Error
            } else {
                Severity::Warning
            },
            summary: message.to_string(),
            details: Some(block.join("\n")),
            range: primary_location(block),
            code,
        });
    }
    drafts
}

fn is_cargo_summary(message: &str) -> bool {
    message.starts_with("could not compile ")
        || message.starts_with("aborting due to ")
        || message.starts_with("build failed")
        || message.starts_with("test failed, to rerun pass ")
        || message.starts_with("doctest failed, to rerun pass ")
        || TARGETS_FAILED.is_match(message)
        || GENERATED.is_match(message)
        || EMITTED.is_match(message)
}

/// The first `-->` after the header and before any child `note:`/`help:`,
/// whose own `-->` points somewhere else (`context.md` §9).
fn primary_location(block: &[&str]) -> Option<SourceRange> {
    let captures = block[1..]
        .iter()
        .take_while(|line| !CHILD_HEADER.is_match(line))
        .find_map(|line| LOCATION.captures(line))?;
    workspace_range(&captures[1], &captures[2], Some(&captures[3]))
}

/// A printed `path:line[:column]` as a range, or `None` when the path is
/// absolute or climbs out with `..`: registry sources, the standard library,
/// a doctest's temporary file (`context.md` §9, §10).
pub(super) fn workspace_range(path: &str, line: &str, column: Option<&str>) -> Option<SourceRange> {
    let outside = Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|part| part == Component::ParentDir);
    if outside {
        return None;
    }
    Some(SourceRange {
        path: path.to_string(),
        start_line: line.parse().ok()?,
        start_column: column.and_then(|column| column.parse().ok()),
        end_line: None,
        end_column: None,
    })
}

/// Bracket codes are read from the header. This handles lint codes, in the
/// order `context.md` §9 sets, reading only `= note:`/`= help:` annotations
/// so a snippet of the user's own `#[warn(…)]` source is never read as one.
fn lint_code(block: &[&str]) -> Option<String> {
    let annotations = || {
        block
            .iter()
            .map(|line| line.trim_start())
            .filter(|line| line.starts_with("= "))
    };
    annotations()
        .find_map(|line| CLIPPY_URL.captures(line))
        .map(|captures| format!("clippy::{}", &captures[1]))
        .or_else(|| {
            annotations()
                .find_map(|line| LINT_ATTRIBUTE.captures(line))
                .map(|captures| captures[1].to_string())
        })
        .or_else(|| {
            annotations()
                .find_map(|line| DENY_FLAG.captures(line))
                .map(|captures| captures[1].replace('-', "_"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_policy::CommandRisk;
    use crate::command_runner::{CommandExecution, CommandTermination};
    use crate::finding::{default_parsers, findings_from_execution};
    use crate::secret_scanner::SecretScanner;

    const BUILD_ERRORS: &str = include_str!("fixtures/rust_build_errors.txt");
    const CLIPPY_WARNINGS: &str = include_str!("fixtures/rust_clippy_warnings.txt");
    const CLIPPY_DENY: &str = include_str!("fixtures/rust_clippy_deny.txt");

    fn range(path: &str, line: u32, column: u32) -> Option<SourceRange> {
        Some(SourceRange {
            path: path.to_string(),
            start_line: line,
            start_column: Some(column),
            end_line: None,
            end_column: None,
        })
    }

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
        }
    }

    #[test]
    fn a_type_error_keeps_its_code_message_and_location() {
        let drafts = parse_rust_diagnostics(BUILD_ERRORS);
        let e0308 = &drafts[0];
        assert_eq!(e0308.source, FindingSource::Compiler);
        assert_eq!(e0308.severity, Severity::Error);
        assert_eq!(e0308.code.as_deref(), Some("E0308"));
        assert_eq!(e0308.summary, "mismatched types");
        assert_eq!(e0308.range, range("crates/demo/src/lib.rs", 8, 18));
    }

    /// `context.md` §9: E0061's block also carries
    /// `note: function defined here --> …:13:4`, which is the definition,
    /// not the error.
    #[test]
    fn the_location_is_the_primary_one_not_a_child_notes() {
        let drafts = parse_rust_diagnostics(BUILD_ERRORS);
        let e0061 = &drafts[1];
        assert_eq!(e0061.code.as_deref(), Some("E0061"));
        assert_eq!(e0061.range, range("crates/demo/src/lib.rs", 9, 5));
    }

    /// The second rustc-lint occurrence carries no note, so it has no code.
    /// That is honest, not missing (`context.md` §9).
    #[test]
    fn compiler_warnings_keep_their_severity_and_only_a_code_they_printed() {
        let drafts = parse_rust_diagnostics(BUILD_ERRORS);
        let (unused, other) = (&drafts[2], &drafts[3]);
        assert_eq!(unused.severity, Severity::Warning);
        assert_eq!(unused.source, FindingSource::Compiler);
        assert_eq!(unused.summary, "unused variable: `unused`");
        assert_eq!(unused.code.as_deref(), Some("unused_variables"));
        assert_eq!(unused.range, range("crates/demo/src/lib.rs", 2, 9));
        assert_eq!(other.summary, "unused variable: `other`");
        assert_eq!(other.code, None);
    }

    #[test]
    fn cargo_summary_lines_are_not_findings() {
        for (fixture, expected) in [(BUILD_ERRORS, 4), (CLIPPY_WARNINGS, 4), (CLIPPY_DENY, 4)] {
            let drafts = parse_rust_diagnostics(fixture);
            let summaries: Vec<_> = drafts.iter().map(|d| d.summary.as_str()).collect();
            assert_eq!(drafts.len(), expected, "{summaries:?}");
            assert!(
                summaries
                    .iter()
                    .all(|s| !s.starts_with("could not compile") && !s.contains(" generated ")),
                "{summaries:?}"
            );
        }
    }

    /// The `#[warn(clippy::…)]` note is on the first occurrence only. The
    /// help URL is on every one (`context.md` §9).
    #[test]
    fn every_clippy_occurrence_gets_its_code_from_the_help_url() {
        let drafts = parse_rust_diagnostics(CLIPPY_WARNINGS);
        let codes: Vec<_> = drafts.iter().map(|d| d.code.as_deref()).collect();
        assert_eq!(
            codes,
            [
                Some("unused_variables"),
                Some("clippy::needless_return"),
                Some("clippy::needless_return"),
                Some("clippy::ptr_arg"),
            ]
        );
        let sources: Vec<_> = drafts.iter().map(|d| d.source).collect();
        assert_eq!(
            sources,
            [
                FindingSource::Compiler,
                FindingSource::Lint,
                FindingSource::Lint,
                FindingSource::Lint,
            ]
        );
        assert!(drafts.iter().all(|d| d.severity == Severity::Warning));
        assert_eq!(drafts[1].range, range("crates/demo/src/lib.rs", 3, 5));
        assert_eq!(drafts[2].range, range("crates/demo/src/lib.rs", 7, 5));
    }

    /// Severity is recorded as printed (§5.2): `-D warnings` prints `error`.
    #[test]
    fn deny_warnings_records_lints_as_errors_and_reads_the_dash_flag() {
        let drafts = parse_rust_diagnostics(CLIPPY_DENY);
        assert!(drafts.iter().all(|d| d.severity == Severity::Error));
        assert_eq!(drafts[0].code.as_deref(), Some("unused_variables"));
        assert_eq!(drafts[0].source, FindingSource::Compiler);
        assert_eq!(drafts[3].code.as_deref(), Some("clippy::ptr_arg"));
    }

    #[test]
    fn details_are_the_diagnostic_block_and_nothing_after_it() {
        let drafts = parse_rust_diagnostics(BUILD_ERRORS);
        let details = drafts[0].details.as_deref().expect("details");
        assert!(
            details.starts_with("error[E0308]: mismatched types\n"),
            "{details}"
        );
        assert!(details.contains("expected `u32`, found `&str`"));
        assert!(
            !details.contains("E0061"),
            "the next block leaked in: {details}"
        );
        let e0061 = drafts[1].details.as_deref().expect("details");
        assert!(
            e0061.contains("help: remove the extra argument"),
            "child lost: {e0061}"
        );
    }

    #[test]
    fn a_diagnostic_without_a_location_has_no_range() {
        let output = "error: linker `cc` not found\n  |\n  = note: No such file or directory (os error 2)\n\nerror: could not compile `demo` (bin \"demo\") due to 1 previous error\n";
        let drafts = parse_rust_diagnostics(output);
        assert_eq!(drafts.len(), 1, "{drafts:?}");
        assert_eq!(drafts[0].summary, "linker `cc` not found");
        assert_eq!(drafts[0].range, None);
    }

    /// An error with no location of its own must not borrow its child
    /// note's, which points at something else.
    #[test]
    fn a_child_notes_location_is_not_borrowed_when_the_error_has_none() {
        let output = "error: cannot find macro `foo` in this scope\n  |\nnote: a macro with a similar name exists\n --> crates/demo/src/lib.rs:1:1\n  |\n";
        let drafts = parse_rust_diagnostics(output);
        assert_eq!(drafts.len(), 1, "{drafts:?}");
        assert_eq!(drafts[0].range, None);
    }

    #[test]
    fn a_location_outside_the_workspace_has_no_range() {
        for path in [
            "/Users/me/.cargo/registry/src/index.crates.io-1/serde-1.0.0/src/lib.rs",
            "/rustc/0123abcd/library/core/src/option.rs",
            "../sibling/src/lib.rs",
        ] {
            let output = format!("error[E0599]: no method\n --> {path}:1:1\n  |\n");
            let drafts = parse_rust_diagnostics(&output);
            assert_eq!(drafts.len(), 1);
            assert_eq!(drafts[0].range, None, "{path} kept as a range");
        }
    }

    /// Captured with `CARGO_TERM_COLOR=always` (`context.md` §9).
    #[test]
    fn ansi_colour_codes_are_ignored() {
        let output = "\x1b[1m\x1b[91merror[E0308]\x1b[0m\x1b[1m: mismatched types\x1b[0m\n \x1b[1m\x1b[94m--> \x1b[0mcrates/demo/src/lib.rs:2:18\n  \x1b[1m\x1b[94m|\x1b[0m\n";
        let drafts = parse_rust_diagnostics(output);
        assert_eq!(drafts.len(), 1, "{drafts:?}");
        assert_eq!(drafts[0].code.as_deref(), Some("E0308"));
        assert_eq!(drafts[0].summary, "mismatched types");
        assert_eq!(drafts[0].range, range("crates/demo/src/lib.rs", 2, 18));
    }

    #[test]
    fn clean_output_yields_nothing() {
        let output = "   Compiling demo v0.1.0 (/repo/crates/demo)\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.33s\n";
        assert!(parse_rust_diagnostics(output).is_empty());
    }

    #[test]
    fn matches_build_check_and_clippy_in_their_common_forms() {
        let parser = RustDiagnosticsParser;
        for command in [
            "cargo build",
            "cargo b --release",
            "cargo check --workspace",
            "cargo c",
            "cargo clippy --workspace --all-targets --locked -- -D warnings",
            "cargo +nightly check",
            "RUSTFLAGS=-Dwarnings cargo check",
            "cd crates/demo && cargo check",
            "/Users/me/.cargo/bin/cargo build",
        ] {
            assert!(parser.matches(command), "{command}");
        }
    }

    #[test]
    fn does_not_match_other_commands() {
        let parser = RustDiagnosticsParser;
        for command in [
            "cargo test",
            "cargo run",
            "cargo fmt --check",
            "cargo",
            "npm run build",
            "make check",
        ] {
            assert!(!parser.matches(command), "{command}");
        }
    }

    #[test]
    fn cargo_subcommand_skips_toolchains_and_flags() {
        assert_eq!(
            cargo_subcommand("cargo +nightly -q test --lib"),
            Some("test")
        );
        assert_eq!(
            cargo_subcommand("cd x && cargo nextest run"),
            Some("nextest")
        );
        assert_eq!(cargo_subcommand("npm test"), None);
        assert_eq!(cargo_subcommand("cargo"), None);
    }

    /// Registration, end to end through the Task 2 dispatcher.
    #[test]
    fn default_parsers_turn_a_failed_build_into_parsed_findings() {
        let findings = findings_from_execution(
            &execution("cargo build", 101, BUILD_ERRORS),
            &default_parsers(),
            &SecretScanner::default(),
        );
        assert_eq!(findings.len(), 4, "{findings:?}");
        assert!(
            findings
                .iter()
                .all(|f| f.source() != FindingSource::Command)
        );
        assert_eq!(findings[0].code(), Some("E0308"));
    }

    /// §5.3's fall-through, against this parser: a build that failed with
    /// output this parser cannot read still yields one finding.
    #[test]
    fn a_failed_build_this_parser_cannot_read_falls_through_to_generic() {
        let findings = findings_from_execution(
            &execution("cargo build", 101, "Killed: 9\n"),
            &default_parsers(),
            &SecretScanner::default(),
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].source(), FindingSource::Command);
        assert_eq!(findings[0].summary(), "cargo build: Killed: 9");
    }
}
