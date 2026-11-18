use super::*;
use workspace_engine::{
    CommitCandidate, CommitCandidateSource, CommitRecord, CommitVerification, MockModelAdapter,
    PlanStep, StepStatus, TaskPlan,
};

struct Fixture {
    engine: WorkspaceEngine,
    session_id: String,
    _base: std::path::PathBuf,
}

fn fixture(name: &str) -> Fixture {
    let base = std::env::temp_dir().join(format!(
        "damaian-commit-message-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let engine = WorkspaceEngine::new(Config {
        data_dir: base.join("data"),
        enable_index_watcher: false,
        ..Config::default()
    });
    let session = engine
        .session_store
        .create_session("repo_test", "Commit message")
        .unwrap();
    engine
        .session_store
        .append_message(&session.id, None, "user", "Add a commit flow")
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "Add a commit flow", "test", "test")
        .unwrap();
    let mut plan = TaskPlan::new(task.id.clone(), 1);
    plan.steps.push(PlanStep {
        id: "step_1".to_string(),
        title: "Add the dialog".to_string(),
        detail: None,
        status: StepStatus::InProgress,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
        check: None,
        build: None,
    });
    engine.session_store.create_plan(&task, &plan).unwrap();
    Fixture {
        engine,
        session_id: session.id,
        _base: base,
    }
}

fn record(fixture: &Fixture) -> CommitRecord {
    CommitRecord {
        prepare_id: "commit_test".to_string(),
        repository_root: "/tmp/repo".to_string(),
        session_id: fixture.session_id.clone(),
        task_id: None,
        selected_paths: vec!["src/app.js".to_string()],
        branch: Some("main".to_string()),
        expected_old: None,
        candidates: vec![CommitCandidate {
            path: "src/app.js".to_string(),
            status: "modified".to_string(),
            source: CommitCandidateSource::AcceptedPatch,
            label: "Accepted change".to_string(),
            selected_by_default: true,
            blob_oid: Some("oid".to_string()),
            content_sha256: None,
            mode: Some("100644".to_string()),
            size: 10,
            unscanned: false,
            secret_warnings: Vec::new(),
            diff: "-old\n+new [REDACTED: aws_access_key]\n".to_string(),
            accepted_hunks: Vec::new(),
            renamed_from: None,
            renamed_to: None,
            copied_from: None,
        }],
        hooks: workspace_engine::GitHookReport::default(),
        message: "chore: Add a commit flow".to_string(),
        verification: CommitVerification::default(),
        prepared_at_ms: 0,
        state: "prepared".to_string(),
        prepared_commit: None,
        head_after: None,
        hook_outcomes: Vec::new(),
        generated_secret_override: false,
        hunk_count: 0,
    }
}

#[test]
fn a_model_draft_is_shown_as_a_subject_and_body() {
    let fixture = fixture("model");
    let record = record(&fixture);
    let mut adapter = MockModelAdapter::new("feat: add the commit flow\n\nBody line.");
    let draft = draft_commit_message_with(
        &fixture.engine,
        &record,
        &["src/app.js".to_string()],
        Some(&mut adapter),
    );
    assert_eq!(draft.source, "model");
    assert_eq!(draft.subject, "feat: add the commit flow");
    assert_eq!(draft.body, "Body line.");
    assert_eq!(draft.message, "feat: add the commit flow\n\nBody line.");
    assert!(draft.notice.is_none());
}

#[test]
fn a_failed_or_empty_model_draft_falls_back_to_the_template() {
    let fixture = fixture("fallback");
    let record = record(&fixture);
    let mut adapter = MockModelAdapter::new("");
    let draft = draft_commit_message_with(
        &fixture.engine,
        &record,
        &["src/app.js".to_string()],
        Some(&mut adapter),
    );
    assert_eq!(draft.source, "fallback");
    assert_eq!(draft.message, "chore: Add a commit flow");
    assert!(draft.notice.is_some());
}

#[test]
fn no_configured_model_falls_back_without_a_call() {
    let fixture = fixture("no-model");
    let record = record(&fixture);
    let draft =
        draft_commit_message_with(&fixture.engine, &record, &["src/app.js".to_string()], None);
    assert_eq!(draft.source, "fallback");
    assert_eq!(draft.message, "chore: Add a commit flow");
}

#[test]
fn the_model_sees_the_redacted_diff_and_the_plan_titles() {
    let fixture = fixture("prompt");
    let record = record(&fixture);
    let mut adapter = MockModelAdapter::new("feat: x");
    let _ = draft_commit_message_with(
        &fixture.engine,
        &record,
        &["src/app.js".to_string()],
        Some(&mut adapter),
    );
    let prompt = &adapter.requests[0].messages[1].content;
    assert!(prompt.contains("Add a commit flow"), "{prompt}");
    assert!(prompt.contains("Add the dialog"), "{prompt}");
    assert!(prompt.contains("[REDACTED: aws_access_key]"), "{prompt}");
    assert!(!prompt.contains("AKIAIOSFODNN7EXAMPLE"), "{prompt}");
    assert!(prompt.contains("src/app.js"), "{prompt}");
}

#[test]
fn a_fenced_or_prefixed_answer_is_still_a_draft() {
    assert_eq!(
        parse_commit_draft("```\nSubject: feat: x\n\nBody\n```"),
        Some(("feat: x".to_string(), "Body".to_string()))
    );
    assert_eq!(parse_commit_draft("   \n\n"), None);
}
