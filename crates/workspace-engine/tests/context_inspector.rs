//! Spec 27: the context inspector's data and controls.
//!
//! These tests pin what the panel renders — manifest rows, exclusions, pins and
//! the session restriction — and the removal API's refusal of `Required`. The
//! browser surface is walked manually in the close-out task.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::compaction::ConversationSummary;
use workspace_engine::context_manager::ExclusionReason;
use workspace_engine::{
    CancelToken, ChatTurnResult, Config, ContextCategory, ContextDecision, ContextKind, ContextPin,
    ContextRequest, LineRange, MockModelAdapter, ModelAdapter, Provenance, Symbol,
    SymbolIndexStore, TaskStatus, ToolCall, TurnProgress, TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-context-inspector-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn write_fixture(root: &Path, relative_path: &str, content: &str) {
    let path = root.join(relative_path);
    fs::create_dir_all(path.parent().expect("fixture should have parent")).unwrap();
    fs::write(path, content).unwrap();
}

fn test_config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    }
}

fn fixture_repo(name: &str) -> PathBuf {
    let repo = temp_dir(name);
    write_fixture(&repo, "AGENTS.md", "Follow the repository rules.\n");
    write_fixture(&repo, "README.md", "# Fixture\n\nA small repository.\n");
    write_fixture(
        &repo,
        "src/upload.rs",
        &format!(
            "{}pub fn upload_retry() {{}}\n\npub fn upload_once() {{}}\n",
            "// filler\n".repeat(41)
        ),
    );
    write_fixture(
        &repo,
        "src/other.rs",
        "pub fn other_marker_zzq() -> usize { 7 }\n",
    );
    write_fixture(&repo, "secrets/api_token.txt", "not a real token\n");
    repo
}

fn summary() -> ConversationSummary {
    ConversationSummary {
        id: "summary_1".to_string(),
        through_seq: 12,
        format_version: 1,
        objective: "Ship the upload retry.".to_string(),
        files_changed: vec!["src/upload.rs".to_string()],
        plan_steps: Vec::new(),
        approvals: Vec::new(),
        validation: Vec::new(),
        constraints: vec!["Keep the retry bounded.".to_string()],
        decisions: Vec::new(),
        failed_approaches: Vec::new(),
        uncertainty: Vec::new(),
        model_run_id: None,
        message_count: 4,
    }
}

/// Assemble once through the public manager with the controls under test.
fn assemble(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    pinned: &[ContextPin],
    restriction: Option<&[String]>,
) -> workspace_engine::ContextPlan {
    let index = engine
        .indexer
        .index_repository(repo)
        .expect("the fixture should index");
    engine
        .context_manager
        .build_context(ContextRequest {
            repository_root: repo,
            repository_id: &index.repository_id,
            task_id: "task_inspector",
            prompt,
            index: Some(&index),
            explicit_paths: &[],
            limit: 16_000,
            hook_context: &[],
            plan: None,
            pinned,
            restriction,
            conversation_summary: None,
        })
        .expect("the assembly should run")
}

fn excluded_reason(plan: &workspace_engine::ContextPlan, path: &str) -> Option<ExclusionReason> {
    plan.manifest
        .excluded
        .iter()
        .find(|record| record.path.as_deref() == Some(path))
        .map(|record| record.reason)
}

fn included_paths(plan: &workspace_engine::ContextPlan) -> Vec<&str> {
    plan.manifest
        .included
        .iter()
        .filter_map(|included| included.path.as_deref())
        .collect()
}

#[test]
fn the_manifest_has_a_summary_row_with_its_message_count() {
    let repo = fixture_repo("summary-row");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let summary = summary();

    let plan = engine
        .context_manager
        .build_context(ContextRequest {
            repository_root: &repo,
            repository_id: &index.repository_id,
            task_id: "task_summary",
            prompt: "What next?",
            index: Some(&index),
            explicit_paths: &[],
            limit: 16_000,
            hook_context: &[],
            plan: None,
            pinned: &[],
            restriction: None,
            conversation_summary: Some(&summary),
        })
        .expect("the assembly should run");

    let usage = plan
        .manifest
        .conversation_summary
        .expect("the summary row is present");
    assert_eq!(usage.messages, 4);
    assert_eq!(usage.through_seq, 12);
    assert!(!usage.facts_only);
    assert!(usage.tokens > 0, "{usage:?}");
    // The manifest is safe to show: the summary's text is not in it.
    let json = serde_json::to_string(&plan.manifest).unwrap();
    assert!(!json.contains("Keep the retry bounded"), "{json}");
}

#[test]
fn the_symbols_row_counts_language_server_and_heuristic_symbols() {
    let repo = fixture_repo("symbol-counts");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let (stored, _) = engine.symbol_index(&repo).unwrap();
    let retry = stored
        .symbols
        .iter()
        .find(|symbol| symbol.name == "upload_retry")
        .expect("the heuristic index has the symbol")
        .clone();
    let hash = index
        .files
        .iter()
        .find(|file| file.path == "src/upload.rs")
        .expect("the fixture is indexed")
        .content_hash
        .clone();
    let store = SymbolIndexStore::new(&engine.config.data_dir, engine.audit_log.clone());
    assert!(
        store
            .merge_language_server(
                &index.repository_id,
                "src/upload.rs",
                &hash,
                vec![Symbol {
                    provenance: Provenance::LanguageServer,
                    ..retry
                }],
            )
            .unwrap(),
        "the language-server symbol was not merged"
    );

    let plan = assemble(
        &engine,
        &repo,
        "Compare upload_retry and upload_once",
        &[],
        None,
    );
    let counts = plan
        .manifest
        .symbol_provenance
        .expect("the symbols block was included");
    assert!(counts.total >= 2, "{counts:?}");
    assert_eq!(
        counts.total,
        counts.language_server + counts.heuristic,
        "{counts:?}"
    );
    assert!(counts.language_server >= 1, "{counts:?}");
    assert!(counts.heuristic >= 1, "{counts:?}");
}

#[test]
fn a_pinned_file_enters_the_pinned_category() {
    let repo = fixture_repo("pin-file");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let pin = ContextPin {
        path: "src/other.rs".to_string(),
        range: None,
    };

    let plan = assemble(&engine, &repo, "What is this?", &[pin], None);

    let pinned = plan
        .manifest
        .included
        .iter()
        .find(|included| included.category == ContextCategory::Pinned)
        .expect("a pinned item is present");
    assert_eq!(pinned.path.as_deref(), Some("src/other.rs"));
    assert_eq!(pinned.range, None);
    assert!(plan.files.iter().any(|path| path == "src/other.rs"));
}

#[test]
fn a_pinned_range_carries_its_line_range() {
    let repo = fixture_repo("pin-range");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let pin = ContextPin {
        path: "src/upload.rs".to_string(),
        range: Some(LineRange { start: 42, end: 44 }),
    };

    let plan = assemble(&engine, &repo, "What is this?", &[pin], None);

    let pinned = plan
        .manifest
        .included
        .iter()
        .find(|included| included.category == ContextCategory::Pinned)
        .expect("a pinned item is present");
    assert_eq!(pinned.range, Some(LineRange { start: 42, end: 44 }));
    let item = plan
        .items
        .iter()
        .find(|item| item.category == ContextCategory::Pinned)
        .unwrap();
    assert!(item.content.contains("upload_retry"), "{}", item.content);
}

#[test]
fn a_pinned_file_the_policy_refuses_is_an_exclusion_not_an_item() {
    let repo = fixture_repo("pin-restricted");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let pin = ContextPin {
        path: "secrets/api_token.txt".to_string(),
        range: None,
    };

    let plan = assemble(&engine, &repo, "What is this?", &[pin], None);

    assert_eq!(
        excluded_reason(&plan, "secrets/api_token.txt"),
        Some(ExclusionReason::RestrictedPath)
    );
    assert!(
        !included_paths(&plan).contains(&"secrets/api_token.txt"),
        "{:?}",
        included_paths(&plan)
    );
    assert!(
        !plan
            .manifest
            .excluded
            .iter()
            .find(|record| record.path.as_deref() == Some("secrets/api_token.txt"))
            .is_some_and(|record| record
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("not a real token"))),
        "the exclusion must carry no content"
    );
}

#[test]
fn a_pinned_file_that_no_longer_reads_is_missing() {
    let repo = fixture_repo("pin-deleted");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let pin = ContextPin {
        path: "src/gone.rs".to_string(),
        range: None,
    };

    let plan = assemble(&engine, &repo, "What is this?", &[pin], None);

    assert_eq!(
        excluded_reason(&plan, "src/gone.rs"),
        Some(ExclusionReason::Missing)
    );
}

#[test]
fn a_restriction_filters_each_file_bearing_source() {
    let repo = fixture_repo("restriction");
    let engine = WorkspaceEngine::new(test_config(&repo));
    write_fixture(&repo, "src/mentioned.rs", "pub fn mentioned_zzq() {}\n");
    let restriction = vec!["src/upload.rs".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        "Look at src/upload.rs and src/mentioned.rs and other_marker_zzq",
        &[ContextPin {
            path: "src/other.rs".to_string(),
            range: None,
        }],
        Some(&restriction),
    );

    for path in ["src/mentioned.rs", "src/other.rs", "README.md"] {
        assert_eq!(
            excluded_reason(&plan, path),
            Some(ExclusionReason::PathNotSelected),
            "{path} should be outside the restriction: {:?}",
            plan.manifest.excluded
        );
    }
    assert!(
        included_paths(&plan).contains(&"src/upload.rs"),
        "{:?}",
        included_paths(&plan)
    );
}

#[test]
fn a_restriction_cannot_include_a_path_the_policy_refuses() {
    let repo = fixture_repo("restriction-restricted");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let restriction = vec!["secrets/api_token.txt".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        "Look at secrets/api_token.txt",
        &[ContextPin {
            path: "secrets/api_token.txt".to_string(),
            range: None,
        }],
        Some(&restriction),
    );

    assert_eq!(
        excluded_reason(&plan, "secrets/api_token.txt"),
        Some(ExclusionReason::RestrictedPath)
    );
    assert!(
        !included_paths(&plan).contains(&"secrets/api_token.txt"),
        "{:?}",
        included_paths(&plan)
    );
}

#[test]
fn a_restriction_does_not_remove_agent_instructions() {
    let repo = fixture_repo("restriction-instructions");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let restriction = vec!["src/upload.rs".to_string()];

    let plan = assemble(&engine, &repo, "What is this?", &[], Some(&restriction));

    assert!(
        included_paths(&plan).contains(&"AGENTS.md"),
        "the instruction hierarchy is policy, not content: {:?}",
        included_paths(&plan)
    );
}

#[test]
fn the_required_item_is_still_first_and_cannot_be_removed() {
    let repo = fixture_repo("required-order");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let plan = assemble(&engine, &repo, "What is this?", &[], None);
    let mut prepared = workspace_engine::PreparedContext::from_plan(&plan);
    let required = prepared
        .items
        .iter()
        .position(|item| item.category == ContextCategory::Required)
        .expect("the user's message is required");
    assert_eq!(
        prepared.items[required].kind,
        ContextKind::UserPrompt,
        "Required holds the user's own message"
    );

    let error = prepared
        .remove_optional(required)
        .expect_err("Required must not be removable");
    assert!(
        error.to_string().to_lowercase().contains("required"),
        "{error}"
    );
    assert_eq!(prepared.items.len(), plan.items.len());
}

#[test]
fn removing_an_instruction_reduces_tokens_and_records_user_removed() {
    let repo = fixture_repo("remove-instruction");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let plan = assemble(&engine, &repo, "What is this?", &[], None);
    let mut prepared = workspace_engine::PreparedContext::from_plan(&plan);
    let index = prepared
        .items
        .iter()
        .position(|item| item.kind == ContextKind::AgentInstruction)
        .expect("the fixture has AGENTS.md");
    let before = prepared.manifest.total_tokens;
    let item = prepared.items[index].clone();

    let record = prepared.remove_optional(index).unwrap();

    assert_eq!(record.reason, ExclusionReason::UserRemoved);
    assert_eq!(record.path.as_deref(), Some("AGENTS.md"));
    assert_eq!(record.category, ContextCategory::Instructions);
    assert!(prepared.manifest.total_tokens < before);
    assert!(
        prepared
            .manifest
            .included
            .iter()
            .all(|included| included.path.as_deref() != Some("AGENTS.md"))
    );
    let usage = prepared
        .manifest
        .categories
        .iter()
        .find(|usage| usage.category == ContextCategory::Instructions)
        .unwrap();
    assert!(
        !prepared
            .items
            .iter()
            .any(|item| item.kind == ContextKind::AgentInstruction)
    );
    let remaining_tokens: usize = prepared
        .items
        .iter()
        .filter(|item| item.category == ContextCategory::Instructions)
        .map(|item| item.tokens)
        .sum();
    assert_eq!(usage.used, remaining_tokens);
    assert!(item.tokens > 0);
}

#[test]
fn removing_a_whole_file_removes_it_from_files() {
    let repo = fixture_repo("remove-file");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let plan = assemble(&engine, &repo, "Look at src/other.rs", &[], None);
    let mut prepared = workspace_engine::PreparedContext::from_plan(&plan);
    let index = prepared
        .items
        .iter()
        .position(|item| item.path.as_deref() == Some("src/other.rs"))
        .expect("the mention includes the file");

    prepared.remove_optional(index).unwrap();

    assert!(!prepared.files().iter().any(|path| path == "src/other.rs"));
}

#[test]
fn removing_by_index_twice_uses_the_new_indices() {
    let repo = fixture_repo("remove-twice");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let plan = assemble(&engine, &repo, "Look at src/other.rs", &[], None);
    let mut prepared = workspace_engine::PreparedContext::from_plan(&plan);
    // Remove the first non-required item, then the first non-required item of
    // what is left; both removals use the manifest the previous one returned.
    let first = prepared
        .items
        .iter()
        .position(|item| item.category != ContextCategory::Required)
        .unwrap();
    let first_item = prepared.items[first].clone();
    prepared.remove_optional(first).unwrap();
    let second = prepared
        .items
        .iter()
        .position(|item| item.category != ContextCategory::Required)
        .unwrap();
    let second_item = prepared.items[second].clone();
    prepared.remove_optional(second).unwrap();

    assert!(!prepared.items.contains(&first_item));
    assert!(!prepared.items.contains(&second_item));
    assert_eq!(
        prepared
            .manifest
            .excluded
            .iter()
            .filter(|record| record.reason == ExclusionReason::UserRemoved)
            .count(),
        2
    );
}

#[test]
fn removing_the_summary_records_its_category_and_hides_the_row() {
    let repo = fixture_repo("remove-summary");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let plan = engine
        .context_manager
        .build_context(ContextRequest {
            repository_root: &repo,
            repository_id: &index.repository_id,
            task_id: "task_summary_remove",
            prompt: "What next?",
            index: Some(&index),
            explicit_paths: &[],
            limit: 16_000,
            hook_context: &[],
            plan: None,
            pinned: &[],
            restriction: None,
            conversation_summary: Some(&summary()),
        })
        .unwrap();
    let mut prepared = workspace_engine::PreparedContext::from_plan(&plan);

    let record = prepared.remove_summary().unwrap();

    assert_eq!(record.reason, ExclusionReason::UserRemoved);
    assert_eq!(record.category, ContextCategory::ConversationSummary);
    assert!(prepared.manifest.conversation_summary.is_none());
    assert!(
        prepared
            .manifest
            .excluded
            .iter()
            .any(
                |excluded| excluded.category == ContextCategory::ConversationSummary
                    && excluded.reason == ExclusionReason::UserRemoved
            )
    );
    assert!(
        prepared.remove_summary().is_err(),
        "a second removal has nothing to remove"
    );
}

// ---- Task 6: the pre-send pause and the send gate ----

fn pause_config(repo: &Path) -> Config {
    let mut config = test_config(repo);
    config.pause_for_context_review = true;
    config
}

type Progress = std::rc::Rc<std::cell::RefCell<Vec<TurnProgress>>>;

fn ask_with(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> (ChatTurnResult, Progress) {
    let cancel = CancelToken::new();
    let mut tokens = |_token: &str| {};
    let progress: Progress = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let collected = progress.clone();
    let mut on_progress = move |event: TurnProgress| collected.borrow_mut().push(event);
    let mut sink = TurnSink {
        on_token: &mut tokens,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run or pause");
    (result, progress)
}

fn resume_context(
    engine: &WorkspaceEngine,
    proposal: &str,
    decision: ContextDecision,
    adapter: &mut dyn ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut tokens = |_token: &str| {};
    let mut progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut tokens,
        on_progress: &mut progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.resume_after_context_decision(
        proposal,
        decision,
        "desktop_user",
        adapter,
        &mut sink,
    )
}

#[test]
fn a_turn_pauses_before_its_first_model_call_when_the_setting_is_on() {
    let repo = fixture_repo("pause-review");
    let engine = WorkspaceEngine::new(pause_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");

    let (result, progress) = ask_with(&engine, &repo, "What is this?", &mut adapter);

    assert!(
        adapter.requests.is_empty(),
        "a paused review must not call the model: {:?}",
        adapter.requests
    );
    let review = result.context_review.expect("the review is present");
    assert!(!review.manifest.included.is_empty());
    let pending = engine
        .config
        .data_dir
        .join("chat")
        .join("pending")
        .join(format!("{}.json", review.id));
    assert!(pending.exists(), "the pause is on disk at {pending:?}");
    let json = fs::read_to_string(&pending).unwrap();
    assert!(json.contains("\"context_review\""), "{json}");
    assert!(
        progress
            .borrow()
            .iter()
            .any(|event| matches!(event, TurnProgress::Context(_))),
        "the pre-send manifest was not emitted"
    );
}

#[test]
fn the_default_turn_emits_its_manifest_before_the_first_request() {
    let repo = fixture_repo("no-pause");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");

    let (result, progress) = ask_with(&engine, &repo, "What is this?", &mut adapter);

    assert!(result.context_review.is_none());
    assert_eq!(adapter.requests.len(), 1);
    let events = progress.borrow();
    let context_at = events
        .iter()
        .position(|event| matches!(event, TurnProgress::Context(_)))
        .expect("the manifest was emitted");
    let model_at = events
        .iter()
        .position(
            |event| matches!(event, TurnProgress::Phase(phase) if phase.kind.as_str() == "model"),
        )
        .expect("the model phase was emitted");
    assert!(
        context_at < model_at,
        "the manifest must precede the first model phase: {events:?}"
    );
}

#[test]
fn sending_a_reviewed_context_makes_exactly_one_request() {
    let repo = fixture_repo("send-review");
    let engine = WorkspaceEngine::new(pause_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let (result, _) = ask_with(&engine, &repo, "What is this?", &mut adapter);
    let review = result.context_review.expect("the review is present");

    let resumed = resume_context(&engine, &review.id, ContextDecision::Send, &mut adapter)
        .expect("sending the reviewed context should run");

    assert_eq!(adapter.requests.len(), 1);
    assert_eq!(resumed.task.status, TaskStatus::Complete);
    assert!(
        adapter.requests[0].messages[1]
            .content
            .contains("What is this?")
    );
}

#[test]
fn declining_a_review_makes_no_model_call_and_does_not_send() {
    let repo = fixture_repo("decline-review");
    let engine = WorkspaceEngine::new(pause_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let (result, _) = ask_with(&engine, &repo, "What is this?", &mut adapter);
    let review = result.context_review.expect("the review is present");

    let resumed = resume_context(&engine, &review.id, ContextDecision::Decline, &mut adapter)
        .expect("declining is a result, not an error");

    assert!(adapter.requests.is_empty());
    assert_eq!(resumed.task.status, TaskStatus::Cancelled);
    assert!(resumed.cancelled);
    assert!(resumed.response.contains("No request was sent"));
}

#[test]
fn removing_an_instruction_changes_the_request_that_is_sent() {
    let repo = fixture_repo("remove-before-send");
    let engine = WorkspaceEngine::new(pause_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let (result, _) = ask_with(&engine, &repo, "What is this?", &mut adapter);
    let review = result.context_review.expect("the review is present");
    let index = review
        .manifest
        .included
        .iter()
        .position(|included| included.kind == ContextKind::AgentInstruction)
        .expect("the fixture has AGENTS.md");
    let instruction_tokens = review.manifest.included[index].tokens;

    let amended = engine
        .chat_orchestrator
        .remove_context_item(&review.id, index)
        .expect("an Instructions item is removable");
    assert!(amended.total_tokens < review.manifest.total_tokens);
    assert_eq!(
        amended.total_tokens + instruction_tokens,
        review.manifest.total_tokens
    );
    assert!(
        amended
            .excluded
            .iter()
            .any(|record| record.reason == ExclusionReason::UserRemoved
                && record.path.as_deref() == Some("AGENTS.md"))
    );

    let _ = resume_context(&engine, &review.id, ContextDecision::Send, &mut adapter)
        .expect("the amended context should send");
    let sent = &adapter.requests[0].messages[1].content;
    assert!(
        !sent.contains("Follow the repository rules."),
        "the removed instruction was still sent: {sent}"
    );
    assert!(sent.contains("What is this?"));
}

#[test]
fn removing_a_required_item_is_refused_at_the_api() {
    let repo = fixture_repo("required-refused");
    let engine = WorkspaceEngine::new(pause_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let (result, _) = ask_with(&engine, &repo, "What is this?", &mut adapter);
    let review = result.context_review.expect("the review is present");
    let index = review
        .manifest
        .included
        .iter()
        .position(|included| included.category == ContextCategory::Required)
        .expect("the request is required");

    let error = engine
        .chat_orchestrator
        .remove_context_item(&review.id, index)
        .expect_err("Required must be refused");

    assert!(
        error.to_string().to_lowercase().contains("required"),
        "{error}"
    );
    // The review is still open and can be sent.
    let resumed = resume_context(&engine, &review.id, ContextDecision::Send, &mut adapter);
    assert!(resumed.is_ok(), "{resumed:?}");
    assert_eq!(adapter.requests.len(), 1);
    assert!(
        adapter.requests[0].messages[1]
            .content
            .contains("What is this?")
    );
}

#[test]
fn a_command_approval_cannot_resume_a_context_review() {
    let repo = fixture_repo("cross-resume");
    let engine = WorkspaceEngine::new(pause_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let (result, _) = ask_with(&engine, &repo, "What is this?", &mut adapter);
    let review = result.context_review.expect("the review is present");

    let error = engine.chat_orchestrator.resume_after_command_decision(
        &review.id,
        true,
        "desktop_user",
        &mut adapter,
        &mut TurnSink {
            on_token: &mut |_token: &str| {},
            on_progress: &mut |_event: TurnProgress| {},
            cancel: &CancelToken::new(),
            steering: None,
        },
    );

    assert!(
        error.is_err(),
        "a review must not be resumed as an approval"
    );
    assert!(
        fs::read_dir(engine.config.data_dir.join("chat").join("pending"))
            .map(|entries| entries.count())
            .unwrap_or(0)
            > 0,
        "the refused resume must not consume the review"
    );
}

#[test]
fn a_pending_turn_written_without_the_review_field_still_loads() {
    let repo = fixture_repo("old-pending");
    let engine = WorkspaceEngine::new(pause_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let (result, _) = ask_with(&engine, &repo, "What is this?", &mut adapter);
    let review = result.context_review.expect("the review is present");
    let path = engine
        .config
        .data_dir
        .join("chat")
        .join("pending")
        .join(format!("{}.json", review.id));
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("context_review");
    fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();

    // The turn still parses; it is now an action pause, so this route refuses
    // it by kind rather than failing to read the file.
    let error = engine
        .chat_orchestrator
        .remove_context_item(&review.id, 0)
        .expect_err("without the field it is not a review");
    let message = error.to_string();
    assert!(!message.contains("Failed to parse"), "{message}");
}

#[test]
fn a_context_review_cannot_resume_a_command_approval() {
    let repo = fixture_repo("cross-resume-2");
    let engine = WorkspaceEngine::new(test_config(&repo));
    // A side-effecting command pauses for approval, producing a real command
    // pause to try to resume as a review.
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Done.".to_string()],
        vec![
            vec![ToolCall {
                id: "call_run_command".to_string(),
                name: "run_command".to_string(),
                arguments_json: serde_json::json!({
                    "command": "sed -i '' 's/1/2/' src/other.rs",
                    "reason": "Change the value"
                })
                .to_string(),
            }],
            Vec::new(),
        ],
    );
    let (result, _) = ask_with(&engine, &repo, "Change the value", &mut adapter);
    let proposal = result
        .command_proposal
        .expect("the command should pause for approval");

    let error = resume_context(&engine, &proposal.id, ContextDecision::Send, &mut adapter)
        .expect_err("a command pause is not a context review");
    assert!(
        error
            .to_string()
            .contains("resume it with resume_after_command_decision"),
        "{error}"
    );
}

// ---- Task 7: the post-send record, past turns, and the edit flow ----

#[test]
fn the_newest_manifest_per_task_is_what_was_sent() {
    let repo = fixture_repo("newest-manifest");
    let engine = WorkspaceEngine::new(pause_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let (result, _) = ask_with(&engine, &repo, "What is this?", &mut adapter);
    let review = result.context_review.expect("the review is present");
    let index = review
        .manifest
        .included
        .iter()
        .position(|included| included.kind == ContextKind::AgentInstruction)
        .expect("the fixture has AGENTS.md");
    engine
        .chat_orchestrator
        .remove_context_item(&review.id, index)
        .expect("the instruction is removable");
    let resumed = resume_context(&engine, &review.id, ContextDecision::Send, &mut adapter)
        .expect("the amended context should send");

    let manifests = engine
        .session_store
        .read_session_context_manifests(&resumed.session.id)
        .expect("the session log should read");
    let manifest = manifests
        .get(&resumed.task.id)
        .expect("the task has a manifest");
    assert!(
        manifest
            .included
            .iter()
            .all(|included| included.path.as_deref() != Some("AGENTS.md")),
        "the sent manifest still lists the removed instruction"
    );
    assert!(
        manifest
            .excluded
            .iter()
            .any(|record| record.reason == ExclusionReason::UserRemoved
                && record.path.as_deref() == Some("AGENTS.md"))
    );
}

#[test]
fn a_past_turns_context_is_readable_after_another_turn() {
    let repo = fixture_repo("past-turns");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let first = ask_with(&engine, &repo, "What is this?", &mut adapter).0;
    let second = ask_with_again(&engine, &repo, &first.session.id, "And now?", &mut adapter);

    let manifests = engine
        .session_store
        .read_session_context_manifests(&second.session.id)
        .expect("the session log should read");
    assert!(manifests.contains_key(&first.task.id), "{manifests:?}");
    assert!(manifests.contains_key(&second.task.id), "{manifests:?}");
    assert_ne!(first.task.id, second.task.id);
}

fn ask_with_again(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: &str,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let cancel = CancelToken::new();
    let mut tokens = |_token: &str| {};
    let mut progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut tokens,
        on_progress: &mut progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], Some(session_id), adapter, &mut sink)
        .expect("the second turn should run")
}

#[test]
fn an_old_context_assembled_record_without_the_new_fields_still_parses() {
    let repo = fixture_repo("old-manifest");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let result = ask_with(&engine, &repo, "What is this?", &mut adapter).0;
    let manifests = engine
        .session_store
        .read_session_context_manifests(&result.session.id)
        .unwrap();
    let manifest = manifests.get(&result.task.id).unwrap();
    let mut value = serde_json::to_value(manifest).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("conversationSummary");
    object.remove("symbolProvenance");
    let parsed: workspace_engine::ContextManifest = serde_json::from_value(value).unwrap();
    assert!(parsed.conversation_summary.is_none());
    assert!(parsed.symbol_provenance.is_none());
}

#[test]
fn an_edit_from_a_session_honours_its_pins_and_restriction() {
    let repo = fixture_repo("edit-controls");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let session = engine
        .session_store
        .create_session(&index.repository_id, "Editing")
        .unwrap();
    engine
        .session_store
        .add_context_pin(&session.id, "src/other.rs", None)
        .unwrap();
    engine
        .session_store
        .set_context_restriction(&session.id, &["src/other.rs".to_string()])
        .unwrap();
    let mut adapter = MockModelAdapter::new(
        "DAMAIAN_EDIT_V1\nSUMMARY: Update other\nFILE: src/other.rs\nSTATUS: modified\nCONTENT:\npub fn other_marker_zzq() -> usize { 8 }\nEND_FILE\nEND_PATCH\n",
    );

    let result = engine
        .edit_orchestrator
        .propose_edit(&repo, "Update other", &[], Some(&session.id), &mut adapter)
        .expect("the edit should be proposed");

    let manifests = engine
        .session_store
        .read_session_context_manifests(&result.session.id)
        .unwrap();
    let manifest = manifests.get(&result.task.id).unwrap();
    assert!(
        manifest
            .included
            .iter()
            .any(|included| included.category == ContextCategory::Pinned
                && included.path.as_deref() == Some("src/other.rs")),
        "the origin session's pin did not reach the edit: {:?}",
        manifest.included
    );
    assert!(
        manifest.included.iter().all(|included| matches!(
            included.path.as_deref(),
            None | Some("AGENTS.md") | Some("src/other.rs")
        )),
        "the restriction did not reach the edit: {:?}",
        manifest.included
    );
}

fn context_events_for(engine: &WorkspaceEngine, task_id: &str) -> usize {
    let path = engine.config.data_dir.join("audit").join("events.jsonl");
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["eventType"] == "context_assembled")
        .filter(|event| event["taskId"] == task_id)
        .count()
}

#[test]
fn an_unchanged_review_is_recorded_once_and_a_changed_one_twice() {
    let repo = fixture_repo("review-record-count");
    let engine = WorkspaceEngine::new(pause_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");
    let (first, _) = ask_with(&engine, &repo, "What is this?", &mut adapter);
    let first_review = first.context_review.expect("the review is present");
    assert_eq!(context_events_for(&engine, &first.task.id), 1);
    let sent = resume_context(
        &engine,
        &first_review.id,
        ContextDecision::Send,
        &mut adapter,
    )
    .expect("an unchanged review sends");
    assert_eq!(
        context_events_for(&engine, &sent.task.id),
        1,
        "an unchanged turn must not record a second assembly"
    );

    let second = ask_with_again(&engine, &repo, &sent.session.id, "And now?", &mut adapter);
    let second_review = second.context_review.expect("the review is present");
    let index = second_review
        .manifest
        .included
        .iter()
        .position(|included| included.category == ContextCategory::RepositoryMap)
        .expect("the map is included");
    engine
        .chat_orchestrator
        .remove_context_item(&second_review.id, index)
        .expect("the map is removable");
    let sent = resume_context(
        &engine,
        &second_review.id,
        ContextDecision::Send,
        &mut adapter,
    )
    .expect("the amended review sends");
    assert_eq!(
        context_events_for(&engine, &sent.task.id),
        2,
        "a changed turn records the amended assembly as the newest"
    );
}
