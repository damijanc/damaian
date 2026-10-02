//! Biome diagnostics: `biome check` / `lint` / `ci` / `format`, and
//! `npm run lint*` (spec 22 Task 5).
//!
//! Severity is Biome's marker on a diagnostic's first message line. `×`
//! (`✖` under forced colour) is `Error`, `!` (`⚠`) is `Warning`, and `i`
//! (`ℹ`) is `Info`. That mapping agrees with Biome's own "Found N errors"
//! count (proposal §5.2). Every Biome finding is `Lint`, `format` and `parse`
//! included. The code is the full category, such as
//! `lint/suspicious/noDebugger`. Every rule here comes from real output
//! recorded in `docs/specs/22_findings_model_and_panel/context.md` §11.

use super::rust_diagnostics::{regex, workspace_range};
use super::{FindingDraft, FindingParser, FindingSource, Severity};
use crate::command_runner::CommandExecution;
use regex::Regex;
use std::sync::LazyLock;

/// Any line ending in a `━━━` rule. A Biome block runs from its header to
/// the next of these, because blocks contain whitespace-only lines.
static ANY_HEADER: LazyLock<Regex> = LazyLock::new(|| regex(r"^\S.*━{3,}\s*$"));
/// `path[:line:col] category [FIXABLE] ━━━`. The path is lazy so it cannot
/// swallow the `:line:col`.
static DIAGNOSTIC: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"^(\S+?)(?::(\d+):(\d+))? ([A-Za-z][A-Za-z0-9/_-]*)(?:\s+FIXABLE)?\s+━{3,}\s*$")
});
static MARKER: LazyLock<Regex> = LazyLock::new(|| regex(r"^  ([×✖!⚠iℹ]) (.+)$"));
static HIDDEN: LazyLock<Regex> = LazyLock::new(|| regex(r"Diagnostics not shown: (\d+)\."));
/// CSI colour codes and OSC 8 hyperlinks, both present under `--colors=force`.
static ESCAPES: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\x1b\[[0-9;]*m|\x1b\]8;;[^\x1b]*\x1b\\"));

pub(super) struct BiomeParser;

impl FindingParser for BiomeParser {
    fn source(&self) -> FindingSource {
        FindingSource::Lint
    }

    /// A `biome` subcommand, or an `npm`/`pnpm`/`yarn run lint[:…]` script.
    /// A lint script may not be Biome. If it is not, `parse` finds nothing
    /// and the run falls through to its generic finding (`context.md` §11).
    fn matches(&self, command: &str) -> bool {
        let tokens: Vec<&str> = command.split_whitespace().collect();
        let biome = tokens.windows(2).any(|pair| {
            (pair[0] == "biome" || pair[0].ends_with("/biome"))
                && matches!(pair[1], "check" | "lint" | "ci" | "format")
        });
        let lint_script = tokens.windows(3).any(|words| {
            matches!(words[0], "npm" | "pnpm" | "yarn")
                && words[1] == "run"
                && (words[2] == "lint" || words[2].starts_with("lint:"))
        });
        biome || lint_script
    }

    fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft> {
        parse_biome(&execution.stderr, &execution.stdout)
    }
}

/// Diagnostics from stderr, plus one `Info` draft when stdout says Biome
/// hid some above its display cap (`context.md` §11).
pub(super) fn parse_biome(stderr: &str, stdout: &str) -> Vec<FindingDraft> {
    let clean = ESCAPES.replace_all(stderr, "");
    let lines: Vec<&str> = clean.lines().collect();
    let mut drafts = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(header) = DIAGNOSTIC.captures(line) else {
            continue;
        };
        let end = lines[index + 1..]
            .iter()
            .position(|line| ANY_HEADER.is_match(line))
            .map_or(lines.len(), |offset| index + 1 + offset);
        let block = &lines[index..end];
        let kept = block
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .map_or(1, |last| last + 1);
        let block = &block[..kept];

        let category = &header[4];
        let (severity, summary) = block[1..]
            .iter()
            .find_map(|line| MARKER.captures(line))
            .map(|marker| (severity_of(&marker[1]), marker[2].trim().to_string()))
            .unwrap_or_else(|| (Severity::Error, category.to_string()));
        let range = header.get(2).and_then(|line| {
            workspace_range(&header[1], line.as_str(), header.get(3).map(|c| c.as_str()))
        });
        drafts.push(FindingDraft {
            source: FindingSource::Lint,
            severity,
            summary,
            details: Some(block.join("\n")),
            range,
            code: Some(category.to_string()),
        });
    }

    let stdout = ESCAPES.replace_all(stdout, "");
    if let Some(hidden) = HIDDEN.captures(&stdout) {
        drafts.push(FindingDraft {
            source: FindingSource::Lint,
            severity: Severity::Info,
            summary: format!(
                "Biome did not show {} more diagnostics; rerun with --max-diagnostics to see them",
                &hidden[1]
            ),
            details: None,
            range: None,
            code: None,
        });
    }
    drafts
}

fn severity_of(marker: &str) -> Severity {
    match marker {
        "×" | "✖" => Severity::Error,
        "!" | "⚠" => Severity::Warning,
        _ => Severity::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_policy::CommandRisk;
    use crate::command_runner::{CommandExecution, CommandTermination};
    use crate::finding::{SourceRange, default_parsers, findings_from_execution};
    use crate::secret_scanner::SecretScanner;

    const CHECK_STDERR: &str = include_str!("fixtures/biome_check_stderr.txt");
    const CHECK_STDOUT: &str = include_str!("fixtures/biome_check_stdout.txt");
    const INFO_STDERR: &str = include_str!("fixtures/biome_info_stderr.txt");

    fn range(path: &str, line: u32, column: u32) -> Option<SourceRange> {
        Some(SourceRange {
            path: path.to_string(),
            start_line: line,
            start_column: Some(column),
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

    fn check() -> Vec<FindingDraft> {
        parse_biome(CHECK_STDERR, CHECK_STDOUT)
    }

    #[test]
    fn a_lint_error_keeps_its_rule_location_and_message() {
        let drafts = check();
        let debugger = &drafts[2];
        assert_eq!(debugger.source, FindingSource::Lint);
        assert_eq!(debugger.severity, Severity::Error);
        assert_eq!(debugger.code.as_deref(), Some("lint/suspicious/noDebugger"));
        assert_eq!(
            debugger.summary,
            "This is an unexpected use of the debugger statement."
        );
        assert_eq!(
            debugger.range,
            range("crates/desktop-shell/static/app.js", 6, 3)
        );
    }

    #[test]
    fn a_warning_marker_is_a_warning() {
        let unused = &check()[0];
        assert_eq!(unused.severity, Severity::Warning);
        assert_eq!(
            unused.code.as_deref(),
            Some("lint/correctness/noUnusedVariables")
        );
        assert_eq!(unused.summary, "This variable unused is unused.");
        assert_eq!(
            unused.range,
            range("crates/desktop-shell/static/app.js", 2, 9)
        );
    }

    /// This repository's own output at planning time (`context.md` §11).
    #[test]
    fn an_info_marker_is_info() {
        let drafts = parse_biome(INFO_STDERR, "");
        assert_eq!(drafts.len(), 1, "{drafts:?}");
        assert_eq!(drafts[0].severity, Severity::Info);
        assert_eq!(drafts[0].code.as_deref(), Some("lint/style/useTemplate"));
        assert_eq!(
            drafts[0].range,
            range("scripts/check-spec-status.mjs", 136, 7)
        );
    }

    /// Biome's own stdout says "Found 6 errors. Found 2 warnings." The
    /// marker mapping must agree with it (`context.md` §11).
    #[test]
    fn one_draft_per_diagnostic_agreeing_with_biomes_own_count() {
        let drafts = check();
        let errors = drafts
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let warnings = drafts
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        assert_eq!((drafts.len(), errors, warnings), (8, 6, 2), "{drafts:#?}");
        assert!(
            CHECK_STDOUT.contains("Found 6 errors.") && CHECK_STDOUT.contains("Found 2 warnings.")
        );
    }

    #[test]
    fn a_format_diagnostic_has_no_range() {
        let drafts = check();
        let styles = &drafts[4];
        assert_eq!(styles.code.as_deref(), Some("format"));
        assert_eq!(styles.severity, Severity::Error);
        assert_eq!(
            styles.summary,
            "Formatter would have printed the following content:"
        );
        assert_eq!(
            styles.range, None,
            "a location was invented for a whole-file diff"
        );
    }

    #[test]
    fn a_parse_error_keeps_its_location() {
        let parse = &check()[6];
        assert_eq!(parse.code.as_deref(), Some("parse"));
        assert_eq!(parse.summary, "expected `)` but instead found `;`");
        assert_eq!(parse.range, range("scripts/parse.mjs", 1, 17));
    }

    #[test]
    fn the_closing_check_block_is_not_a_finding() {
        assert!(
            check()
                .iter()
                .all(|d| !d.summary.contains("Some errors were emitted")),
            "the run summary became a finding"
        );
    }

    /// Biome blocks contain whitespace-only lines, so a block must run to
    /// the next `━━━` header (`context.md` §11).
    #[test]
    fn details_run_past_whitespace_only_lines_to_the_next_header() {
        let drafts = check();
        let details = drafts[0].details.as_deref().expect("details");
        assert!(
            details.starts_with("crates/desktop-shell/static/app.js:2:9 "),
            "{details}"
        );
        assert!(
            details.contains("const·_unused·=·1;"),
            "the fix was cut off: {details}"
        );
        assert!(
            !details.contains("scripts/parse.mjs"),
            "the next block leaked in: {details}"
        );
        assert!(
            !details.ends_with(' ') && !details.ends_with('\n'),
            "{details:?}"
        );
    }

    #[test]
    fn hidden_diagnostics_become_one_info_draft() {
        let stdout = "The number of diagnostics exceeds the limit allowed. Use --max-diagnostics to increase it.\nDiagnostics not shown: 9.\nChecked 27 files in 4ms. No fixes applied.\nFound 28 errors.\n";
        let drafts = parse_biome(INFO_STDERR, stdout);
        assert_eq!(drafts.len(), 2, "{drafts:?}");
        let hidden = &drafts[1];
        assert_eq!(hidden.severity, Severity::Info);
        assert!(hidden.summary.contains('9'), "{}", hidden.summary);
        assert_eq!(hidden.range, None);
    }

    /// Captured with `--colors=force` (`context.md` §11): CSI colours, an
    /// OSC 8 hyperlink around the category, and `⚠` for the warning marker.
    #[test]
    fn forced_colour_output_parses_the_same() {
        let stderr = "\x1b[0mcrates/desktop-shell/static/app.js\x1b[0m\x1b[0m:\x1b[0m\x1b[0m2\x1b[0m\x1b[0m:\x1b[0m\x1b[0m9\x1b[0m\x1b[0m \x1b[0m\x1b[0m\x1b]8;;https://biomejs.dev/linter/rules/no-unused-variables\x1b\\lint/correctness/noUnusedVariables\x1b]8;;\x1b\\\x1b[0m\x1b[0m \x1b[0m\x1b[0m\x1b[30m\x1b[47m FIXABLE \x1b[0m\x1b[0m \x1b[0m\x1b[0m━━━━━━━━━━━━━━━━\x1b[0m\x1b[0m\n\n\x1b[0m\x1b[0m  \x1b[0m\x1b[0m\x1b[1m\x1b[33m⚠\x1b[0m\x1b[0m \x1b[0m\x1b[0m\x1b[33mThis variable \x1b[0m\x1b[0m\x1b[1m\x1b[33munused\x1b[0m\x1b[0m\x1b[33m is unused.\x1b[0m\x1b[0m\n";
        let drafts = parse_biome(stderr, "");
        assert_eq!(drafts.len(), 1, "{drafts:?}");
        assert_eq!(drafts[0].severity, Severity::Warning);
        assert_eq!(drafts[0].summary, "This variable unused is unused.");
        assert_eq!(
            drafts[0].code.as_deref(),
            Some("lint/correctness/noUnusedVariables")
        );
        assert_eq!(
            drafts[0].range,
            range("crates/desktop-shell/static/app.js", 2, 9)
        );
    }

    #[test]
    fn colour_markers_map_like_plain_ones() {
        for (marker, severity) in [
            ("✖", Severity::Error),
            ("⚠", Severity::Warning),
            ("ℹ", Severity::Info),
        ] {
            let stderr = format!("a.js:1:1 lint/x/y ━━━━━━\n\n  {marker} message\n");
            assert_eq!(parse_biome(&stderr, "")[0].severity, severity, "{marker}");
        }
    }

    #[test]
    fn an_absolute_path_has_no_range() {
        let stderr =
            "/tmp/elsewhere/app.js:1:1 lint/suspicious/noDebugger ━━━━━━\n\n  × debugger\n";
        let drafts = parse_biome(stderr, "");
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].range, None);
    }

    #[test]
    fn a_block_without_a_marker_is_kept_as_an_error_named_by_its_category() {
        let stderr = "a.js:3:1 lint/x/y ━━━━━━\n\n    3 │ code\n";
        let drafts = parse_biome(stderr, "");
        assert_eq!(drafts.len(), 1, "a printed diagnostic was dropped");
        assert_eq!(drafts[0].severity, Severity::Error);
        assert_eq!(drafts[0].summary, "lint/x/y");
    }

    #[test]
    fn matches_biome_invocations_and_npm_lint_scripts() {
        let parser = BiomeParser;
        for command in [
            "biome check",
            "npx biome check --write",
            "./node_modules/.bin/biome lint scripts",
            "biome ci",
            "biome format .",
            "npm run lint:web",
            "npm run lint",
            "pnpm run lint:js",
            "yarn run lint",
        ] {
            assert!(parser.matches(command), "{command}");
        }
    }

    #[test]
    fn does_not_match_other_commands() {
        let parser = BiomeParser;
        for command in [
            "npm test",
            "npm run build",
            "npm run lint-staged",
            "biome --version",
            "cargo clippy",
            "eslint .",
        ] {
            assert!(!parser.matches(command), "{command}");
        }
    }

    /// Registration, end to end through the dispatcher.
    #[test]
    fn default_parsers_turn_a_failed_lint_run_into_parsed_findings() {
        let findings = findings_from_execution(
            &execution("npm run lint:web", 1, CHECK_STDOUT, CHECK_STDERR),
            &default_parsers(),
            &SecretScanner::default(),
        );
        assert_eq!(findings.len(), 8, "{findings:?}");
        assert!(findings.iter().all(|f| f.source() == FindingSource::Lint));
    }

    /// `context.md` §11: `npm run lint` may not be Biome at all. Output this
    /// parser cannot read must fall through, exactly as with no parser.
    #[test]
    fn a_failed_lint_script_that_is_not_biome_falls_through_to_generic() {
        let findings = findings_from_execution(
            &execution("npm run lint", 1, "", "sh: eslint: command not found\n"),
            &default_parsers(),
            &SecretScanner::default(),
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].source(), FindingSource::Command);
    }

    #[test]
    fn a_passing_run_keeps_its_info() {
        let findings = findings_from_execution(
            &execution("npm run lint:web", 0, "", INFO_STDERR),
            &default_parsers(),
            &SecretScanner::default(),
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].severity(), Severity::Info);
    }
}
