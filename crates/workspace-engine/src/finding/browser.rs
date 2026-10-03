//! Browser findings from a recorded diagnostic (spec 22 Task 6).
//!
//! Not a `FindingParser`: a browser diagnostic is spec 12's typed report,
//! not command output to match. Sources and severities are the browser's
//! own (proposal §5.2; the table is `context.md` §12.2):
//! - a page error is `BrowserConsole`/`Error`;
//! - a console `error` or `assert` is `BrowserConsole`/`Error`, and a
//!   `warning` is `BrowserConsole`/`Warning`;
//! - a failed request is `BrowserNetwork`/`Error`;
//! - a failed scenario step is `BrowserScenario`/`Error`.
//!
//! A runner that failed, with nothing else to explain it, is one `Command`
//! finding (§12.4).

use super::{Finding, FindingDraft, FindingSource, Severity, SourceRange, first_non_empty_line};
use crate::secret_scanner::SecretScanner;
use crate::web_diagnostics::{WebDiagnosticRecord, WebSourceLocation, is_loopback_url};
use std::path::{Component, Path};

/// Findings for one diagnostic run. `repository_files` holds
/// repository-relative paths, against which a served URL is matched
/// (§12.3). Task 8 attaches `task_id` and `origin_ref`.
pub fn findings_from_web_record(
    record: &WebDiagnosticRecord,
    repository_files: &[&str],
    scanner: &SecretScanner,
) -> Vec<Finding> {
    let report = &record.report;
    let mut drafts = Vec::new();
    if let Some(details) = &report.details {
        for error in &details.page_errors {
            drafts.push(draft(
                FindingSource::BrowserConsole,
                Severity::Error,
                error.clone(),
                Some(error.clone()),
            ));
        }
        for entry in details.console.iter().filter(|entry| entry.is_problem()) {
            let level = entry.level.to_ascii_lowercase();
            let severity = if matches!(level.as_str(), "error" | "assert") {
                Severity::Error
            } else {
                Severity::Warning
            };
            let mut console = draft(
                FindingSource::BrowserConsole,
                severity,
                entry.text.clone(),
                Some(entry.model_item()),
            );
            console.range = entry
                .location
                .as_ref()
                .and_then(|location| repository_range(location, repository_files));
            drafts.push(console);
        }
        for request in &details.failed_requests {
            let mut network = draft(
                FindingSource::BrowserNetwork,
                Severity::Error,
                request.model_item(),
                None,
            );
            network.code = request.status.map(|status| status.to_string()).or_else(|| {
                request
                    .failure
                    .clone()
                    .filter(|failure| failure.starts_with("net::"))
            });
            drafts.push(network);
        }
        for step in details.steps.iter().filter(|step| !step.success) {
            drafts.push(draft(
                FindingSource::BrowserScenario,
                Severity::Error,
                step.model_item(),
                None,
            ));
        }
    }

    let explained = drafts.iter().any(|draft| draft.severity == Severity::Error);
    if report.tool_failed() && !explained {
        let message = report
            .details
            .as_ref()
            .and_then(|details| details.tool_error.as_deref())
            .or_else(|| first_non_empty_line(&report.text))
            .unwrap_or("no message");
        drafts.push(draft(
            FindingSource::Command,
            Severity::Error,
            format!("{} {} failed: {message}", record.tool, record.url),
            Some(report.text.clone()),
        ));
    }

    drafts
        .into_iter()
        .map(|draft| Finding::new(draft, scanner))
        .collect()
}

fn draft(
    source: FindingSource,
    severity: Severity,
    summary: String,
    details: Option<String>,
) -> FindingDraft {
    FindingDraft {
        source,
        severity,
        summary,
        details,
        range: None,
        code: None,
    }
}

/// A range only when the served path matches exactly one repository file
/// outside `node_modules`. Two candidates are not a choice to make (§12.3).
fn repository_range(location: &WebSourceLocation, files: &[&str]) -> Option<SourceRange> {
    let line = location.line?;
    let path = served_path(&location.url)?;
    let suffix = format!("/{path}");
    let mut candidates = files.iter().copied().filter(|file| {
        (*file == path || file.ends_with(&suffix))
            && !Path::new(file)
                .components()
                .any(|part| part.as_os_str() == "node_modules")
    });
    let file = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    Some(SourceRange {
        path: file.to_string(),
        start_line: line,
        start_column: location.column,
        end_line: None,
        end_column: None,
    })
}

/// The path of a loopback `http(s)` URL, without its leading `/`, query or
/// fragment. `None` for any other URL, a directory, or a `..` path.
fn served_path(url: &str) -> Option<&str> {
    if !is_loopback_url(url) {
        return None;
    }
    let (_, rest) = url.trim().split_once("://")?;
    let (_, path) = rest.split_once('/')?;
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let unusable = path.is_empty()
        || path.ends_with('/')
        || Path::new(path)
            .components()
            .any(|part| part == Component::ParentDir);
    (!unusable).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_diagnostics::WebDiagnosticReport;

    /// spec 12's `COMPANION_REPORT` (`web_diagnostics.rs` tests), which is
    /// trimmed from a real companion `inspect_page` result. One console
    /// warning from another origin is added (`context.md` §12).
    const INSPECT_REPORT: &str = r#"{
      "success": false, "diagnostic_ok": false, "run_id": "20260925-1",
      "url": "http://localhost:5001/", "final_url": "http://localhost:5001/",
      "title": "Snake Game", "status": 200,
      "page_errors": ["ReferenceError: Cannot access 'game' before initialization"],
      "console": [
        {"type": "error", "text": "Failed to load resource: 404",
         "location": {"url": "http://localhost:5001/js/app.js", "lineNumber": 41, "columnNumber": 7}},
        {"type": "log", "text": "booting"},
        {"type": "warning", "text": "Deprecated API used",
         "location": {"url": "https://cdn.example.com/js/app.js", "lineNumber": 0, "columnNumber": 0}}
      ],
      "failed_requests": [
        {"kind": "response", "url": "http://localhost:5001/api/me", "method": "GET",
         "resource_type": "fetch", "status": 404, "status_text": "Not Found"},
        {"kind": "requestfailed", "url": "http://localhost:5001/ws", "method": "GET",
         "resource_type": "websocket", "failure": "net::ERR_CONNECTION_REFUSED"}
      ],
      "dom_summary": {"forms": 1, "buttons": ["Log in", "Register"], "fields": [],
        "status_text": "", "visible_text_excerpt": "Snake Log in Register Score: 0"},
      "artifacts": [],
      "text_report": "ignored by Damaian"
    }"#;

    /// The repository's file list (`context.md` §12.3). The `node_modules`
    /// copy would make `js/app.js` ambiguous if it were not excluded.
    const FILES: &[&str] = &[
        "static/js/app.js",
        "static/index.html",
        "server.py",
        "node_modules/lib/js/app.js",
    ];

    fn record(tool: &str, text: &str, is_error: bool) -> WebDiagnosticRecord {
        WebDiagnosticRecord {
            id: "webdiagrec_1".to_string(),
            task_id: "task_1".to_string(),
            tool: tool.to_string(),
            url: "http://localhost:5001/".to_string(),
            recorded_at_ms: 1,
            report: WebDiagnosticReport::from_text(text, is_error),
        }
    }

    fn findings_in(text: &str, is_error: bool, files: &[&str]) -> Vec<Finding> {
        findings_from_web_record(
            &record("inspect_web_page", text, is_error),
            files,
            &SecretScanner::default(),
        )
    }

    fn findings(text: &str, is_error: bool) -> Vec<Finding> {
        findings_in(text, is_error, FILES)
    }

    fn console_error_at(url: &str, line: Option<u32>) -> String {
        let line = line.map_or(String::new(), |line| {
            format!(r#", "lineNumber": {}"#, line - 1)
        });
        format!(
            r#"{{"final_url": "http://localhost:3000/", "console": [
              {{"type": "error", "text": "boom", "location": {{"url": "{url}"{line}}}}}]}}"#
        )
    }

    #[test]
    fn an_inspection_yields_one_finding_per_problem_in_report_order() {
        let found = findings(INSPECT_REPORT, false);
        let shape: Vec<_> = found.iter().map(|f| (f.source(), f.severity())).collect();
        assert_eq!(
            shape,
            [
                (FindingSource::BrowserConsole, Severity::Error),
                (FindingSource::BrowserConsole, Severity::Error),
                (FindingSource::BrowserConsole, Severity::Warning),
                (FindingSource::BrowserNetwork, Severity::Error),
                (FindingSource::BrowserNetwork, Severity::Error),
            ],
            "{found:#?}"
        );
        assert!(
            found
                .iter()
                .all(|f| f.task_id().is_none() && f.origin_ref().is_none())
        );
    }

    #[test]
    fn a_page_error_is_a_console_error_without_a_location() {
        let page_error = &findings(INSPECT_REPORT, false)[0];
        assert_eq!(
            page_error.summary(),
            "ReferenceError: Cannot access 'game' before initialization"
        );
        assert_eq!(page_error.range(), None);
    }

    /// The served `/js/app.js` is `static/js/app.js`. The `node_modules`
    /// copy is excluded, so there is exactly one match. Spec 12 stored the
    /// line and column 1-based (41/7 → 42/8).
    #[test]
    fn a_console_error_at_a_served_url_maps_to_the_one_repository_file() {
        let console_error = &findings(INSPECT_REPORT, false)[1];
        assert_eq!(console_error.summary(), "Failed to load resource: 404");
        assert_eq!(
            console_error.range(),
            Some(&SourceRange {
                path: "static/js/app.js".to_string(),
                start_line: 42,
                start_column: Some(8),
                end_line: None,
                end_column: None,
            })
        );
        assert_eq!(
            console_error.details(),
            Some(
                "console error: Failed to load resource: 404 (http://localhost:5001/js/app.js:42:8)"
            )
        );
    }

    #[test]
    fn a_console_location_on_another_origin_has_no_range() {
        let warning = &findings(INSPECT_REPORT, false)[2];
        assert_eq!(warning.summary(), "Deprecated API used");
        assert_eq!(
            warning.range(),
            None,
            "a CDN script was mapped to a repository file"
        );
    }

    #[test]
    fn a_log_line_is_not_a_finding() {
        assert!(
            findings(INSPECT_REPORT, false)
                .iter()
                .all(|f| f.summary() != "booting")
        );
    }

    #[test]
    fn failed_requests_carry_their_status_or_network_error_as_code() {
        let found = findings(INSPECT_REPORT, false);
        assert_eq!(
            found[3].summary(),
            "failed request: GET http://localhost:5001/api/me → 404 Not Found"
        );
        assert_eq!(found[3].code(), Some("404"));
        assert_eq!(found[4].code(), Some("net::ERR_CONNECTION_REFUSED"));
        assert_eq!(found[3].range(), None);
    }

    /// `context.md` §12.1: the model is told about failed steps, so the
    /// panel must show them.
    #[test]
    fn a_failed_scenario_step_is_a_browser_scenario_error() {
        let scenario = r##"{"final_url": "http://localhost:5001/", "results": [
          {"step": 1, "action": "goto", "success": true},
          {"step": 2, "action": "click", "selector": "#start", "success": false,
           "error": "Timeout 5000ms exceeded"}]}"##;
        let found = findings_from_web_record(
            &record("run_web_scenario", scenario, false),
            FILES,
            &SecretScanner::default(),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].source(), FindingSource::BrowserScenario);
        assert_eq!(found[0].severity(), Severity::Error);
        assert_eq!(
            found[0].summary(),
            "step 2 click failed: Timeout 5000ms exceeded"
        );
    }

    /// spec 12's own companion-error shape (`web_diagnostics.rs` tests).
    #[test]
    fn a_runner_that_failed_yields_one_generic_finding() {
        let failed = r#"{"error": true, "success": false, "message": "Timeout 30000ms exceeded",
            "final_url": "http://localhost:5001/"}"#;
        let found = findings(failed, false);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].source(), FindingSource::Command);
        assert_eq!(found[0].severity(), Severity::Error);
        assert_eq!(
            found[0].summary(),
            "inspect_web_page http://localhost:5001/ failed: Timeout 30000ms exceeded"
        );
        assert_eq!(found[0].range(), None);
    }

    #[test]
    fn a_non_companion_runner_failure_yields_one_generic_finding_from_its_text() {
        let found = findings("\nBrowser could not start\nstack…\n", true);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].source(), FindingSource::Command);
        assert_eq!(
            found[0].summary(),
            "inspect_web_page http://localhost:5001/ failed: Browser could not start"
        );
    }

    /// §12.4: prose from another runner is not parsed.
    #[test]
    fn a_non_companion_runner_that_succeeded_yields_nothing() {
        assert!(findings("The page loaded and looked fine.", false).is_empty());
    }

    #[test]
    fn a_clean_inspection_yields_nothing() {
        let clean = r#"{"final_url": "http://localhost:5001/", "page_errors": [],
            "console": [{"type": "log", "text": "ready"}], "failed_requests": []}"#;
        assert!(findings(clean, false).is_empty());
    }

    /// §10.3's rule: the generic finding is added only when nothing else
    /// explains the failure.
    #[test]
    fn a_tool_failure_is_not_doubled_when_the_report_already_explains_it() {
        let found = findings(INSPECT_REPORT, true);
        assert_eq!(found.len(), 5, "{found:?}");
        assert!(found.iter().all(|f| f.source() != FindingSource::Command));
    }

    #[test]
    fn an_ambiguous_served_path_has_no_range() {
        let found = findings_in(
            &console_error_at("http://localhost:3000/app.js", Some(3)),
            false,
            &["dist/app.js", "src/app.js"],
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].range(), None, "one of two candidates was guessed");
    }

    #[test]
    fn an_exact_path_match_counts() {
        let found = findings_in(
            &console_error_at("http://127.0.0.1:3000/app.js", Some(3)),
            false,
            &["app.js", "README.md"],
        );
        assert_eq!(
            found[0].range().map(|r| (r.path.as_str(), r.start_line)),
            Some(("app.js", 3))
        );
    }

    #[test]
    fn a_bundled_url_with_no_repository_file_has_no_range() {
        let found = findings(
            &console_error_at("http://localhost:5001/assets/index-3f9a.js", Some(1)),
            false,
        );
        assert_eq!(found[0].range(), None);
    }

    #[test]
    fn a_location_without_a_line_has_no_range() {
        let found = findings(
            &console_error_at("http://localhost:5001/js/app.js", None),
            false,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].range(), None);
    }

    #[test]
    fn served_path_accepts_only_loopback_http_file_paths() {
        assert_eq!(
            served_path("http://localhost:5001/js/app.js?v=3#top"),
            Some("js/app.js")
        );
        assert_eq!(served_path("http://127.0.0.1/app.js"), Some("app.js"));
        assert_eq!(served_path("http://[::1]:8080/a/b.js"), Some("a/b.js"));
        for url in [
            "https://cdn.example.com/js/app.js",
            "file:///repo/static/js/app.js",
            "webpack:///./src/app.js",
            "http://localhost:5001/",
            "http://localhost:5001",
            "http://localhost:5001/static/",
            "http://localhost:5001/../etc/passwd",
        ] {
            assert_eq!(served_path(url), None, "{url}");
        }
    }

    /// Acceptance criterion: no unredacted secret from "browser output".
    /// The record is built **without** `redacted()`, so only `Finding::new`
    /// stands between the key and the finding.
    #[test]
    fn a_secret_in_browser_output_is_redacted() {
        let key = "AKIAIOSFODNN7EXAMPLE";
        let report = format!(
            r#"{{"final_url": "http://localhost:5001/", "page_errors": ["bad key {key}"],
                "console": [{{"type": "error", "text": "token {key}"}}],
                "failed_requests": [{{"url": "http://localhost:5001/api?key={key}", "method": "GET", "status": 401}}]}}"#
        );
        let found = findings(&report, false);
        assert_eq!(found.len(), 3, "{found:?}");
        for finding in &found {
            assert!(!finding.summary().contains(key), "{}", finding.summary());
            assert!(!finding.details().unwrap_or_default().contains(key));
        }
    }
}
