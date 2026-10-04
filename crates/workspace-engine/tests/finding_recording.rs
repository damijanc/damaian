//! Findings recorded where checks run: spec 22 Task 8, `context.md` §13.4
//! and §14. Every test drives a real turn through the public API.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::finding::{
    Exclusion, ExclusionReason, Finding, FindingDraft, FindingSource, FindingStatus, RepairRequest,
    Severity, SourceRange,
};
use workspace_engine::plan::Evidence;
use workspace_engine::web_diagnostics::{
    WebDiagnosticCall, WebDiagnosticReport, WebDiagnosticsRunner, WebDiagnosticsRunnerHandle,
};
use workspace_engine::{
    CancelToken, ChatTurnResult, Config, MockModelAdapter, ModelAdapter, SecretScanner, ToolCall,
    TurnProgress, TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-finding-recording-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a source file");
    repo
}

fn engine_for(repo: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        // Throwaway repository: a watcher would only cost FSEvents registration.
        enable_index_watcher: false,
        ..Config::default()
    })
}

fn sink_parts() -> (CancelToken, impl FnMut(&str), impl FnMut(TurnProgress)) {
    (
        CancelToken::new(),
        |_token: &str| {},
        |_event: TurnProgress| {},
    )
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let (cancel, mut on_token, mut on_progress) = sink_parts();
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run")
}

/// [`ask`], continuing an existing session.
fn ask_in(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: &str,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let (cancel, mut on_token, mut on_progress) = sink_parts();
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], Some(session_id), adapter, &mut sink)
        .expect("the turn should run")
}

fn approve(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let (cancel, mut on_token, mut on_progress) = sink_parts();
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine
        .chat_orchestrator
        .resume_after_command_decision(proposal_id, true, "tester", adapter, &mut sink)
        .expect("the resumed turn should run")
}

fn call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

/// Scripted tool-call rounds, then a plain answer so the loop ends.
fn scripted(rounds: Vec<Vec<ToolCall>>) -> MockModelAdapter {
    let mut responses: Vec<String> = rounds.iter().map(|_| String::new()).collect();
    let mut calls = rounds;
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

fn findings(engine: &WorkspaceEngine, repo: &Path, session_id: &str) -> Vec<Finding> {
    engine
        .session_store
        .read_findings(session_id, repo)
        .expect("findings should read")
}

/// A failed sandbox command becomes one generic finding (`ls` has no
/// parser). It carries its task and an `origin_ref` under which the full
/// stored output lives (`context.md` §13.4).
#[test]
fn a_failed_sandbox_command_is_recorded_with_its_task_and_reachable_output() {
    let repo = temp_repo("sandbox-failed");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![vec![call(
        "run_command",
        r#"{"command":"ls no-such-directory","reason":"List it"}"#,
    )]]);

    let result = ask(&engine, &repo, "List the folder", &mut adapter);
    let recorded = findings(&engine, &repo, &result.session.id);

    assert_eq!(recorded.len(), 1, "{recorded:?}");
    let finding = &recorded[0];
    assert_eq!(finding.source(), FindingSource::Command);
    assert!(
        finding.summary().starts_with("ls no-such-directory: "),
        "{}",
        finding.summary()
    );
    assert_eq!(finding.task_id(), Some(result.task.id.as_str()));
    let origin = finding.origin_ref().expect("an origin");
    assert!(origin.starts_with("cmd_"), "{origin}");
    let stderr = repo
        .join(".damaian/commands/output")
        .join(origin)
        .join("stderr.log");
    let stored = fs::read_to_string(&stderr).expect("the full output is where origin_ref points");
    assert!(stored.contains("no-such-directory"), "{stored}");
}

#[test]
fn a_passing_sandbox_command_records_nothing() {
    let repo = temp_repo("sandbox-passed");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![vec![call(
        "run_command",
        r#"{"command":"ls src","reason":"List"}"#,
    )]]);

    let result = ask(&engine, &repo, "List the source", &mut adapter);
    assert!(findings(&engine, &repo, &result.session.id).is_empty());
}

/// §14: the step holds the exit and, after it, the findings it produced.
#[test]
fn the_failed_step_carries_findings_evidence_after_its_exit() {
    let repo = temp_repo("evidence");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Check the folder"},{"title":"Report"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"ls no-such-directory","reason":"List it"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Check the folder", &mut adapter);
    let recorded = findings(&engine, &repo, &result.session.id);
    let plan = engine
        .session_store
        .read_task_plan(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the turn proposed a plan");

    match plan.steps[0].evidence.as_slice() {
        [
            Evidence::CommandExit { exit_code, .. },
            Evidence::Findings { refs, failing },
        ] => {
            assert_ne!(*exit_code, Some(0));
            assert_eq!(refs, &vec![recorded[0].id().to_string()]);
            assert_eq!(*failing, 1);
        }
        other => panic!("expected an exit then its findings, got {other:?}"),
    }
}

/// A repository script named `./cargo` prints rustc-shaped errors, so the
/// real Rust diagnostics parser reads it (`context.md` §14).
const FAKE_CARGO: &str = "#!/bin/sh\ncat >&2 <<'EOF'\nerror[E0308]: mismatched types\n --> src/a.rs:1:1\n  |\n\nerror[E0425]: cannot find value `x` in this scope\n --> src/gone.rs:2:3\n  |\n\nerror: could not compile `demo` (lib) due to 2 previous errors\nEOF\nexit 101\n";

/// The approval path records too. A range is kept only when it names a file
/// in the repository, and is hashed then (`context.md` §13.4). Changing that
/// file then makes the finding stale (Task 7).
#[test]
fn an_approved_command_records_findings_with_checked_and_hashed_ranges() {
    let repo = temp_repo("approved");
    fs::write(repo.join("cargo"), FAKE_CARGO).unwrap();
    fs::set_permissions(repo.join("cargo"), fs::Permissions::from_mode(0o755)).unwrap();
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![vec![call(
        "run_command",
        r#"{"command":"./cargo check","reason":"Check it"}"#,
    )]]);

    let stopped = ask(&engine, &repo, "Check the crate", &mut adapter);
    let proposal = stopped
        .command_proposal
        .expect("an unknown executable needs approval");
    assert!(
        proposal.requires_approval && !proposal.blocked,
        "{proposal:?}"
    );
    approve(&engine, &proposal.id, &mut scripted(vec![]));

    let recorded = findings(&engine, &repo, &stopped.session.id);
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    let (kept, dropped) = (&recorded[0], &recorded[1]);
    assert_eq!(kept.code(), Some("E0308"));
    assert_eq!(
        kept.range().map(|range| range.path.as_str()),
        Some("src/a.rs")
    );
    assert_eq!(
        kept.file_hash(),
        Some(
            workspace_engine::hash::file_hash(repo.join("src/a.rs"))
                .unwrap()
                .as_str()
        )
    );
    assert_eq!(dropped.code(), Some("E0425"));
    assert_eq!(
        dropped.range(),
        None,
        "src/gone.rs is not in the repository"
    );
    assert_eq!(dropped.file_hash(), None);
    for finding in &recorded {
        assert_eq!(finding.task_id(), Some(stopped.task.id.as_str()));
        assert!(
            finding
                .origin_ref()
                .is_some_and(|origin| origin.starts_with("cmd_"))
        );
    }

    fs::write(repo.join("src/a.rs"), "fn main() { changed() }\n").unwrap();
    let after = findings(&engine, &repo, &stopped.session.id);
    assert_eq!(after[0].status(), FindingStatus::Stale);
    assert_eq!(
        after[1].status(),
        FindingStatus::Open,
        "no range, so no staleness"
    );
}

const CONSOLE_REPORT: &str = r#"{"final_url": "http://localhost:5001/", "console": [
  {"type": "error", "text": "Uncaught TypeError: game is undefined",
   "location": {"url": "http://localhost:5001/js/app.js", "lineNumber": 41, "columnNumber": 7}}]}"#;

struct ConsoleErrorRunner;

impl WebDiagnosticsRunner for ConsoleErrorRunner {
    fn inspect(&self, _call: &WebDiagnosticCall) -> workspace_engine::Result<WebDiagnosticReport> {
        Ok(WebDiagnosticReport::from_text(CONSOLE_REPORT, false))
    }

    fn run_scenario(
        &self,
        call: &WebDiagnosticCall,
    ) -> workspace_engine::Result<WebDiagnosticReport> {
        self.inspect(call)
    }
}

/// A browser console error is recorded with its served URL mapped to the
/// one repository file (`context.md` §12.3), hashed, and tied to the
/// diagnostic record that produced it.
#[test]
fn a_browser_console_error_is_recorded_with_its_mapped_range_and_record() {
    let repo = temp_repo("browser");
    fs::create_dir_all(repo.join("static/js")).unwrap();
    fs::write(repo.join("static/js/app.js"), "let game;\n").unwrap();
    let mut engine = engine_for(&repo);
    engine
        .chat_orchestrator
        .set_web_diagnostics_runner(WebDiagnosticsRunnerHandle::new(ConsoleErrorRunner));
    let mut adapter = scripted(vec![vec![call(
        "inspect_web_page",
        r#"{"url":"http://localhost:5001/"}"#,
    )]]);

    let result = ask(&engine, &repo, "Why is the game broken?", &mut adapter);
    let recorded = findings(&engine, &repo, &result.session.id);

    assert_eq!(recorded.len(), 1, "{recorded:?}");
    let finding = &recorded[0];
    assert_eq!(finding.source(), FindingSource::BrowserConsole);
    let range = finding.range().expect("the served URL maps to one file");
    assert_eq!(
        (range.path.as_str(), range.start_line),
        ("static/js/app.js", 42)
    );
    assert_eq!(
        finding.file_hash(),
        Some(
            workspace_engine::hash::file_hash(repo.join("static/js/app.js"))
                .unwrap()
                .as_str()
        )
    );
    assert_eq!(finding.task_id(), Some(result.task.id.as_str()));
    assert!(
        finding
            .origin_ref()
            .is_some_and(|origin| origin.starts_with("webdiagrec_"))
    );
}

/// Acceptance criterion and proposal §5.5: dismissal is not suppression. The
/// same failure from a later check is a new, open finding.
#[test]
fn dismissing_a_finding_does_not_suppress_the_same_problem_from_a_later_check() {
    let repo = temp_repo("dismiss");
    let engine = engine_for(&repo);
    let failing = || {
        scripted(vec![vec![call(
            "run_command",
            r#"{"command":"ls no-such-directory","reason":"List it"}"#,
        )]])
    };

    let first = ask(&engine, &repo, "List the folder", &mut failing());
    let dismissed = findings(&engine, &repo, &first.session.id)[0].clone();
    engine
        .session_store
        .set_finding_status(&first.session.id, dismissed.id(), FindingStatus::Dismissed)
        .unwrap();
    ask_in(
        &engine,
        &repo,
        &first.session.id,
        "List it again",
        &mut failing(),
    );

    let all = findings(&engine, &repo, &first.session.id);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[0].status(), FindingStatus::Dismissed);
    assert_eq!(
        all[1].status(),
        FindingStatus::Open,
        "the later check is not suppressed"
    );
    assert_eq!(
        all[1].summary(),
        dismissed.summary(),
        "it is the same problem"
    );
    assert_ne!(all[1].id(), dismissed.id());

    let request =
        RepairRequest::select(&all, &[dismissed.id().to_string(), all[1].id().to_string()]);
    assert_eq!(
        request.findings.iter().map(Finding::id).collect::<Vec<_>>(),
        [all[1].id()]
    );
    assert_eq!(
        request.excluded,
        [Exclusion {
            finding_id: dismissed.id().to_string(),
            reason: ExclusionReason::Dismissed
        }]
    );
}

/// "Current ranges" (§5.5): a request built after an edit excludes the
/// finding the edit made stale, whatever the panel last showed.
#[test]
fn a_finding_made_stale_by_an_edit_is_excluded_when_the_request_is_built() {
    let repo = temp_repo("stale-request");
    let engine = engine_for(&repo);
    let session = engine
        .session_store
        .create_session("repo_1", "Stale")
        .unwrap();
    let recorded = Finding::new(
        FindingDraft {
            source: FindingSource::Compiler,
            severity: Severity::Error,
            summary: "mismatched types".to_string(),
            details: None,
            range: Some(SourceRange {
                path: "src/a.rs".to_string(),
                start_line: 1,
                start_column: Some(1),
                end_line: None,
                end_column: None,
            }),
            code: Some("E0308".to_string()),
        },
        &SecretScanner::default(),
    )
    .with_file_hash(workspace_engine::hash::file_hash(repo.join("src/a.rs")).unwrap());
    engine
        .session_store
        .record_finding(&session.id, &recorded)
        .unwrap();

    fs::write(repo.join("src/a.rs"), "fn main() { edited() }\n").unwrap();
    let all = findings(&engine, &repo, &session.id);
    let request = RepairRequest::select(&all, &[recorded.id().to_string()]);

    assert!(request.is_empty());
    assert_eq!(
        request.excluded,
        [Exclusion {
            finding_id: recorded.id().to_string(),
            reason: ExclusionReason::Stale
        }]
    );
    assert_eq!(request.render(), None);
}
