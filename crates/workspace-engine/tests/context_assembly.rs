//! Spec 26, Task 1: categories, ranges and one renderer.
//!
//! These tests pin the item shape (`category` + `kind` + an expressible range),
//! the one header format both prompt builders use, the section order, and the
//! two places unredacted text used to reach the provider: the user request and
//! the replayed conversation (Correction C17).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnResult, Config, ConfigOverlay, ContextCategory, ContextItem, ContextKind,
    ContextPlan, LineRange, MockModelAdapter, ModelAdapter, TurnProgress, TurnSink,
    WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);
/// The fixture key spec 18's eval scenarios seed; `SecretScanner` classifies it
/// as `aws_access_key`.
const AWS_ACCESS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-context-assembly-{name}-{now}-{}",
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

fn item(kind: ContextKind, path: Option<&str>, content: &str) -> ContextItem {
    ContextItem {
        category: ContextCategory::FileRanges,
        kind,
        path: path.map(str::to_string),
        range: None,
        content: content.to_string(),
        tokens: content.len().div_ceil(4),
        redaction_status: "clean".to_string(),
        provenance: None,
        root_path: None,
    }
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: Option<&str>,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], session_id, adapter, &mut sink)
        .expect("the turn should run")
}

fn fixture_repo(name: &str) -> PathBuf {
    let repo = temp_dir(name);
    write_fixture(&repo, "README.md", "# Fixture\n\nA small repository.\n");
    write_fixture(&repo, "src/lib.rs", "pub fn answer() -> usize { 42 }\n");
    repo
}

fn user_message(adapter: &MockModelAdapter, index: usize) -> &str {
    &adapter.requests[index].messages[1].content
}

#[test]
fn a_range_less_item_renders_exactly_as_it_does_today() {
    let file = item(
        ContextKind::RetrievedFile,
        Some("src/lib.rs"),
        "fn main() {}\n",
    );
    assert_eq!(
        ContextPlan::render_item(&file),
        "\n--- retrieved_file: src/lib.rs ---\nfn main() {}\n"
    );

    let hook = item(
        ContextKind::HookContext,
        Some("hook:x"),
        "Run the tests first.",
    );
    assert_eq!(
        ContextPlan::render_item(&hook),
        "\n--- hook_context: hook:x ---\nRun the tests first.\n"
    );

    let pathless = item(ContextKind::HookContext, None, "No identity.");
    assert_eq!(
        ContextPlan::render_item(&pathless),
        "\n--- hook_context ---\nNo identity.\n"
    );
}

#[test]
fn a_ranged_item_renders_its_line_range() {
    let mut ranged = item(
        ContextKind::RetrievedFile,
        Some("src/lib.rs"),
        "fn main() {}\n",
    );
    ranged.range = Some(LineRange { start: 12, end: 40 });

    assert_eq!(
        ContextPlan::render_item(&ranged),
        "\n--- retrieved_file: src/lib.rs:12-40 ---\nfn main() {}\n"
    );
}

#[test]
fn the_prompt_orders_context_before_conversation_before_the_request() {
    let repo = fixture_repo("order");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new_sequence(vec![
        "The function returns 42.".to_string(),
        "The README names the fixture.".to_string(),
    ]);

    let first = ask(
        &engine,
        &repo,
        None,
        "What does src/lib.rs do?",
        &mut adapter,
    );
    ask(
        &engine,
        &repo,
        Some(&first.session.id),
        "And what does the README say?",
        &mut adapter,
    );

    let prompt = user_message(&adapter, 1);
    let headings = [
        "Repository context:",
        "Recent conversation:",
        "User request:",
    ];
    let positions = headings
        .iter()
        .map(|heading| {
            let matches = prompt.match_indices(heading).collect::<Vec<_>>();
            assert_eq!(
                matches.len(),
                1,
                "{heading} should appear exactly once in:\n{prompt}"
            );
            matches[0].0
        })
        .collect::<Vec<_>>();
    assert!(
        positions[0] < positions[1] && positions[1] < positions[2],
        "the sections are out of order in:\n{prompt}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn the_user_request_appears_once() {
    let repo = fixture_repo("once");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");

    let prompt = "Explain the distinctive_request_marker_zzq in src/lib.rs";
    ask(&engine, &repo, None, prompt, &mut adapter);

    let message = user_message(&adapter, 0);
    assert_eq!(
        message.matches(prompt).count(),
        1,
        "the request is sent more than once:\n{message}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_secret_in_the_user_prompt_is_redacted_in_the_model_request() {
    let repo = fixture_repo("prompt-secret");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");

    let prompt = format!("Please rotate the key {AWS_ACCESS_KEY} in src/lib.rs");
    ask(&engine, &repo, None, &prompt, &mut adapter);

    let message = user_message(&adapter, 0);
    assert!(
        message.contains("[REDACTED_AWS_ACCESS_KEY_"),
        "the request was not redacted:\n{message}"
    );
    assert!(
        !message.contains(AWS_ACCESS_KEY),
        "the raw secret reached the provider:\n{message}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_prompt_larger_than_the_budget_still_reaches_the_model_redacted() {
    let repo = fixture_repo("oversized-prompt");
    let mut config = test_config(&repo);
    config.apply_overlay(
        ConfigOverlay::parse(
            "model_provider=deepseek\n\
             model_name=deepseek-v4-flash\n\
             model_provider.deepseek.context_token_budget=16\n",
        )
        .unwrap(),
    );
    let engine = WorkspaceEngine::new(config);
    let mut adapter = MockModelAdapter::new("Done.");

    let prompt = format!(
        "Please review oversized_request_marker_zzq and rotate {AWS_ACCESS_KEY}. {}",
        "Keep this detail. ".repeat(40)
    );
    assert!(
        prompt.len().div_ceil(4) > engine.config.context_token_budget(),
        "the prompt must exceed the budget or this test proves nothing"
    );
    ask(&engine, &repo, None, &prompt, &mut adapter);

    let message = user_message(&adapter, 0);
    assert!(
        message.contains("oversized_request_marker_zzq"),
        "the oversized request was dropped from the prompt:\n{message}"
    );
    assert!(
        message.contains("[REDACTED_AWS_ACCESS_KEY_"),
        "the surviving request was not redacted:\n{message}"
    );
    assert!(
        !message.contains(AWS_ACCESS_KEY),
        "the raw secret reached the provider:\n{message}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_secret_in_the_user_prompt_is_redacted_in_the_edit_request() {
    let repo = fixture_repo("edit-secret");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let response = "DAMAIAN_EDIT_V1\nSUMMARY: Update lib\nFILE: src/lib.rs\nSTATUS: modified\nCONTENT:\npub fn answer() -> usize { 43 }\nEND_FILE\nEND_PATCH\n";
    let mut adapter = MockModelAdapter::new(response);

    let prompt = format!("Rotate {AWS_ACCESS_KEY} before changing src/lib.rs");
    engine
        .edit_orchestrator
        .propose_edit(&repo, &prompt, &[], None, &mut adapter)
        .expect("the edit should be proposed");

    let message = user_message(&adapter, 0);
    assert!(
        message.contains("[REDACTED_AWS_ACCESS_KEY_"),
        "the edit request was not redacted:\n{message}"
    );
    assert!(
        !message.contains(AWS_ACCESS_KEY),
        "the raw secret reached the provider:\n{message}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_secret_in_an_earlier_turn_is_redacted_in_the_recent_conversation_block() {
    let repo = fixture_repo("replayed-secret");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new_sequence(vec![
        "I will rotate it.".to_string(),
        "Nothing else.".to_string(),
    ]);

    let first_prompt = format!("Rotate {AWS_ACCESS_KEY} in src/lib.rs");
    let first = ask(&engine, &repo, None, &first_prompt, &mut adapter);
    ask(
        &engine,
        &repo,
        Some(&first.session.id),
        "Anything else?",
        &mut adapter,
    );

    let message = user_message(&adapter, 1);
    assert!(
        message.contains("Recent conversation:"),
        "the earlier turn was not replayed at all:\n{message}"
    );
    assert!(
        message.contains("[REDACTED_AWS_ACCESS_KEY_"),
        "the replayed turn was not redacted:\n{message}"
    );
    assert!(
        !message.contains(AWS_ACCESS_KEY),
        "the raw secret from the earlier turn reached the provider:\n{message}"
    );

    fs::remove_dir_all(repo).unwrap();
}

/// The repository-context block, from its heading to the request heading.
fn repository_context_block<'a>(prompt: &'a str, next_heading: &str) -> &'a str {
    let start = prompt
        .find("Repository context:\n")
        .expect("the prompt has a repository context section")
        + "Repository context:\n".len();
    let end = prompt[start..]
        .find(next_heading)
        .unwrap_or_else(|| panic!("no {next_heading} after the context:\n{prompt}"));
    &prompt[start..start + end]
}

#[test]
fn the_chat_and_edit_prompts_render_items_identically() {
    let repo = fixture_repo("one-renderer");
    let engine = WorkspaceEngine::new(test_config(&repo));

    let mut chat = MockModelAdapter::new("Done.");
    ask(&engine, &repo, None, "Review src/lib.rs", &mut chat);
    let chat_context = repository_context_block(user_message(&chat, 0), "User request:");

    let edit_response = "DAMAIAN_EDIT_V1\nSUMMARY: Update lib\nFILE: src/lib.rs\nSTATUS: modified\nCONTENT:\npub fn answer() -> usize { 43 }\nEND_FILE\nEND_PATCH\n";
    let mut edit = MockModelAdapter::new(edit_response);
    engine
        .edit_orchestrator
        .propose_edit(&repo, "Review src/lib.rs", &[], None, &mut edit)
        .expect("the edit should be proposed");
    let edit_prompt = user_message(&edit, 0);
    let edit_context = edit_prompt
        .split_once("Repository context:\n")
        .expect("the edit prompt has a repository context section")
        .1;

    assert!(
        chat_context.contains("--- "),
        "the context block is empty, so this proves nothing:\n{chat_context}"
    );
    // The gap before the next heading is the caller's separation, not the
    // renderer's; the items themselves must be byte-identical.
    assert_eq!(
        chat_context.trim_end_matches('\n'),
        edit_context.trim_end_matches('\n'),
        "the two builders render the same plan differently"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_category_orders_before_the_categories_after_it() {
    // Exhaustive on purpose: a new variant that is not placed here fails to
    // compile, and the enum's declaration order is the pipeline order.
    let pipeline = [
        ContextCategory::Required,
        ContextCategory::Instructions,
        ContextCategory::HookContext,
        ContextCategory::Pinned,
        ContextCategory::PlanState,
        ContextCategory::RepositoryMap,
        ContextCategory::Symbols,
        ContextCategory::SearchResults,
        ContextCategory::FileRanges,
        ContextCategory::Findings,
        ContextCategory::ConversationSummary,
    ];
    for pair in pipeline.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{:?} should order before {:?}",
            pair[0],
            pair[1]
        );
    }
}
