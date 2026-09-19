use std::path::PathBuf;

use workspace_engine::{
    Evidence, ExportFormat, PlanStep, SearchOptions, SecretScanner, SessionStore, StepStatus,
    TaskPlan, TokenUsage, UsageSource,
};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "damaian-search-export-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn scanner() -> SecretScanner {
    SecretScanner::new(Vec::new())
}

fn default_options(max_results: usize) -> SearchOptions {
    SearchOptions {
        whole_word: false,
        literal_phrase: true,
        max_results,
    }
}

#[test]
fn search_finds_a_phrase_and_anchors_on_seq() {
    let data_dir = scratch("find");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Earlier").unwrap();
    store
        .append_message(&session.id, None, "user", "the keychain fix is here")
        .unwrap();
    store
        .append_message(&session.id, None, "assistant", "I looked at the keychain")
        .unwrap();

    let result = store
        .search_sessions(Some("repo_1"), "keychain", default_options(10), &scanner())
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert!(result.hits.iter().all(|hit| hit.session_id == session.id));
    assert!(result.hits.iter().all(|hit| hit.seq > 0));
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.snippet.contains("keychain"))
    );
    assert!(!result.capped);
}

#[test]
fn search_defaults_to_the_current_repository() {
    let data_dir = scratch("scope");
    let store = SessionStore::new(&data_dir);
    let in_repo = store.create_session("repo_a", "In A").unwrap();
    store
        .append_message(&in_repo.id, None, "user", "the word needle appears here")
        .unwrap();
    let other = store.create_session("repo_b", "In B").unwrap();
    store
        .append_message(&other.id, None, "user", "the word needle appears here too")
        .unwrap();

    let scoped = store
        .search_sessions(Some("repo_a"), "needle", default_options(10), &scanner())
        .unwrap();
    assert_eq!(scoped.hits.len(), 1);
    assert_eq!(scoped.hits[0].session_id, in_repo.id);

    let all = store
        .search_sessions(None, "needle", default_options(10), &scanner())
        .unwrap();
    assert_eq!(all.hits.len(), 2);
}

#[test]
fn search_tolerates_a_torn_line_and_reports_it() {
    let data_dir = scratch("torn");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Torn").unwrap();
    store
        .append_message(&session.id, None, "user", "a needle before the tear")
        .unwrap();
    // A truncated final line, exactly what a crash mid-append leaves behind.
    let log = data_dir
        .join("sessions")
        .join(format!("{}.jsonl", session.id));
    let mut content = std::fs::read_to_string(&log).unwrap();
    content.push_str(
        "{\"eventId\":\"e\",\"seq\":9,\"eventType\":\"message_appended\",\"payload\":{\"conte",
    );
    std::fs::write(&log, content).unwrap();

    let result = store
        .search_sessions(Some("repo_1"), "needle", default_options(10), &scanner())
        .unwrap();
    assert_eq!(result.hits.len(), 1, "the readable match survives");
    assert!(result.unreadable_lines >= 1);
}

#[test]
fn search_reports_the_cap_rather_than_silently_truncating() {
    let data_dir = scratch("cap");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Many").unwrap();
    for i in 0..5 {
        store
            .append_message(
                &session.id,
                None,
                "user",
                &format!("needle match number {i}"),
            )
            .unwrap();
    }

    let result = store
        .search_sessions(Some("repo_1"), "needle", default_options(2), &scanner())
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert!(result.capped);
}

#[test]
fn two_identical_searches_return_identical_order() {
    let data_dir = scratch("stable");
    let store = SessionStore::new(&data_dir);
    let first = store.create_session("repo_1", "Alpha").unwrap();
    let second = store.create_session("repo_1", "Beta").unwrap();
    store
        .append_message(&first.id, None, "user", "needle once")
        .unwrap();
    store
        .append_message(&second.id, None, "user", "needle twice and needle again")
        .unwrap();

    let run = |store: &SessionStore| {
        store
            .search_sessions(Some("repo_1"), "needle", default_options(10), &scanner())
            .unwrap()
            .hits
            .iter()
            .map(|hit| (hit.session_id.clone(), hit.seq))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(&store), run(&store));
}

#[test]
fn search_redacts_secrets_in_snippets() {
    let data_dir = scratch("snippet-redact");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Secret").unwrap();
    store
        .append_message(
            &session.id,
            None,
            "user",
            "the deploy token is sk_test_12345678901234567890 in the env",
        )
        .unwrap();

    let result = store
        .search_sessions(Some("repo_1"), "deploy", default_options(10), &scanner())
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert!(
        result.hits[0].snippet.contains("[REDACTED_"),
        "snippet should be redacted: {}",
        result.hits[0].snippet
    );
    assert!(
        !result.hits[0]
            .snippet
            .contains("sk_test_12345678901234567890")
    );
}

#[test]
fn export_markdown_redacts_with_a_count_and_includes_tasks_and_plans() {
    let data_dir = scratch("export-md");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Export me").unwrap();
    let task = store
        .create_task(
            &session.id,
            "rotate the key sk_test_12345678901234567890",
            "p",
            "m",
        )
        .unwrap();
    store
        .append_message(&session.id, Some(&task.id), "assistant", "done rotating")
        .unwrap();
    store
        .record_task_usage(
            &task,
            "run_1",
            None,
            TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                cached_input_tokens: None,
                source: UsageSource::Measured,
            },
            Some(0.001),
            None,
        )
        .unwrap();
    store
        .create_plan(
            &task,
            &TaskPlan {
                task_id: task.id.clone(),
                created_at_ms: 1,
                steps: vec![PlanStep {
                    id: "step_1".to_string(),
                    title: "verify".to_string(),
                    detail: None,
                    status: StepStatus::Completed,
                    depends_on: Vec::new(),
                    started_at_ms: None,
                    completed_at_ms: None,
                    evidence: vec![Evidence::FileRead {
                        path: "a.txt".to_string(),
                        hash: "abc".to_string(),
                    }],
                }],
            },
        )
        .unwrap();

    let markdown = store
        .export_session(&session.id, ExportFormat::Markdown, &scanner())
        .unwrap();
    assert!(markdown.contains("# Export me"));
    assert!(markdown.contains("secret(s) removed"), "states the count");
    assert!(
        !markdown.contains("sk_test_12345678901234567890"),
        "the secret is not in the export"
    );
    assert!(markdown.contains("[REDACTED_"));
    assert!(markdown.contains("verify"), "plan step title present");
    assert!(markdown.contains("20"), "output tokens present");
}

#[test]
fn export_json_preserves_seq_and_redacts() {
    let data_dir = scratch("export-json");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "JSON").unwrap();
    store
        .append_message(
            &session.id,
            None,
            "user",
            "the key sk_test_12345678901234567890 is live",
        )
        .unwrap();

    let json = store
        .export_session(&session.id, ExportFormat::Json, &scanner())
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["redacted"], true);
    assert!(value["redactionCount"].as_u64().unwrap() >= 1);
    let messages = value["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0]["seq"].as_u64().unwrap() > 0);
    assert!(
        !messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("sk_test_12345678901234567890")
    );
    assert!(
        messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("[REDACTED_")
    );
}
