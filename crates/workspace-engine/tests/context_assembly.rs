//! Spec 26, Task 1: categories, ranges and one renderer.
//!
//! These tests pin the item shape (`category` + `kind` + an expressible range),
//! the one header format both prompt builders use, the section order, and the
//! two places unredacted text used to reach the provider: the user request and
//! the replayed conversation (Correction C17).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::context_manager::ExclusionReason;
use workspace_engine::{
    CancelToken, CategoryUsage, ChatTurnResult, ClientError, CollectionSource, Config,
    ConfigOverlay, ContextCategory, ContextItem, ContextKind, ContextManifest, ContextPlan,
    ContextRequest, CurlModelTransport, ExclusionRecord, LimitSource, LineRange, MockModelAdapter,
    ModelAdapter, ModelMessage, ModelRequest, OpenAICompatibleAdapter, PlanStep, ProcessRegistry,
    Provenance, RepositoryIndex, StepStatus, Symbol, SymbolIndexStore, TaskPlan, TokenEstimate,
    TurnProgress, TurnSink, UsageSource, WorkspaceEngine,
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

fn try_ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: Option<&str>,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
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
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: Option<&str>,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    try_ask(engine, repo, session_id, prompt, adapter).expect("the turn should run")
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

/// One audit event type, parsed. `AuditLog::record` writes every field value as
/// a string, so the manifest is a JSON string inside a JSON object.
fn audit_events(data_dir: &Path, event_type: &str) -> Vec<serde_json::Value> {
    let path = data_dir.join("audit").join("events.jsonl");
    let contents = fs::read_to_string(path).unwrap_or_default();
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["eventType"] == event_type)
        .collect()
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

/// Replaces Task 1's `a_prompt_larger_than_the_budget_still_reaches_the_model_redacted`:
/// the interim exemption that let an over-budget `Required` item through is
/// gone, so the same fixture now asserts the visible error on both callers.
#[test]
fn a_required_item_that_does_not_fit_is_an_error() {
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
    let limit = engine.config.context_token_budget();
    assert!(
        prompt.len().div_ceil(4) > limit,
        "the prompt must exceed the budget or this test proves nothing"
    );

    // Both callers propagate the assembly error: the turn fails visibly instead
    // of sending an empty `User request:` (Task 4 replaced Task 1's exemption).
    let error = try_ask(&engine, &repo, None, &prompt, &mut adapter)
        .expect_err("the oversized request must be refused");
    assert!(
        matches!(&error, ClientError::InvalidInput(message) if message.contains(&limit.to_string())),
        "the error must name the limit {limit}: {error:?}"
    );
    let direct = engine.context_manager.build_context(ContextRequest {
        repository_root: &repo,
        repository_id: "repo_oversized",
        task_id: "task_oversized",
        prompt: &prompt,
        index: None,
        explicit_paths: &[],
        limit,
        hook_context: &[],
        plan: None,
        pinned: &[],
        restriction: None,
        conversation_summary: None,
    });
    assert!(
        matches!(&direct, Err(ClientError::InvalidInput(message)) if message.contains(&limit.to_string())),
        "assembly itself must name the limit {limit}: {direct:?}"
    );
    let edit_error = engine
        .edit_orchestrator
        .propose_edit(&repo, &prompt, &[], None, &mut adapter)
        .expect_err("the edit flow must refuse it too");
    assert!(
        matches!(&edit_error, ClientError::InvalidInput(message) if message.contains(&limit.to_string())),
        "the edit error must name the limit {limit}: {edit_error:?}"
    );
    assert!(
        adapter.requests.is_empty(),
        "no request may reach the provider: {:?}",
        adapter
            .requests
            .iter()
            .map(|request| &request.messages[1].content)
            .collect::<Vec<_>>()
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

// ---- Task 2: collection sources and exclusion at collection time ----

/// Every context share is zero, so a test can give one category an exact share
/// and see only that category compete for the limit.
const ZERO_CONTEXT_SHARES: &str = concat!(
    "context_budget.instructions=0\n",
    "context_budget.hook_context=0\n",
    "context_budget.pinned=0\n",
    "context_budget.plan_state=0\n",
    "context_budget.repository_map=0\n",
    "context_budget.symbols=0\n",
    "context_budget.search_results=0\n",
    "context_budget.file_ranges=0\n",
    "context_budget.findings=0\n",
    "context_budget.conversation_summary=0\n",
);

fn config_with_shares(repo: &Path, extra: &str) -> Config {
    let mut config = test_config(repo);
    config.apply_overlay(
        ConfigOverlay::parse(&format!("{ZERO_CONTEXT_SHARES}{extra}"))
            .expect("the test's budget keys should parse"),
    );
    config
}

/// One assembly with the Task 2 request shape, at the default budget.
fn assemble(
    engine: &WorkspaceEngine,
    repo: &Path,
    repository_id: &str,
    prompt: &str,
    explicit_paths: &[String],
    index: Option<&RepositoryIndex>,
    plan: Option<&TaskPlan>,
) -> ContextPlan {
    assemble_with_limit(
        engine,
        repo,
        repository_id,
        prompt,
        explicit_paths,
        index,
        plan,
        16_000,
    )
}

#[allow(clippy::too_many_arguments)]
fn assemble_with_limit(
    engine: &WorkspaceEngine,
    repo: &Path,
    repository_id: &str,
    prompt: &str,
    explicit_paths: &[String],
    index: Option<&RepositoryIndex>,
    plan: Option<&TaskPlan>,
    limit: usize,
) -> ContextPlan {
    engine
        .context_manager
        .build_context(ContextRequest {
            repository_root: repo,
            repository_id,
            task_id: "task-2",
            prompt,
            index,
            explicit_paths,
            limit,
            hook_context: &[],
            plan,
            pinned: &[],
            restriction: None,
            conversation_summary: None,
        })
        .expect("the assembly should succeed")
}

fn usage(plan: &ContextPlan, category: ContextCategory) -> &CategoryUsage {
    plan.manifest
        .categories
        .iter()
        .find(|usage| usage.category == category)
        .unwrap_or_else(|| {
            panic!(
                "{category:?} has no usage row in {:?}",
                plan.manifest.categories
            )
        })
}

fn budget_records<'a>(plan: &'a ContextPlan, path: &str) -> Vec<&'a ExclusionRecord> {
    plan.manifest
        .excluded
        .iter()
        .filter(|record| {
            record.reason == ExclusionReason::CategoryBudgetExhausted
                && record.path.as_deref() == Some(path)
        })
        .collect()
}

/// Every restriction record for `path`, so a test can state which category it
/// came from instead of accepting any record.
fn restrictions_for<'a>(plan: &'a ContextPlan, path: &str) -> Vec<&'a ExclusionRecord> {
    plan.manifest
        .excluded
        .iter()
        .filter(|record| {
            record.reason == ExclusionReason::RestrictedPath && record.path.as_deref() == Some(path)
        })
        .collect()
}

fn assert_absent(plan: &ContextPlan, path: &str) {
    assert!(
        !plan
            .items
            .iter()
            .any(|item| item.path.as_deref() == Some(path)),
        "{path} became an included item: {:?}",
        plan.items
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        !plan.files.iter().any(|included| included == path),
        "{path} reached the included file list: {:?}",
        plan.files
    );
}

#[test]
fn a_restricted_explicit_path_is_excluded_at_collection() {
    let repo = fixture_repo("restricted-explicit");
    write_fixture(&repo, "secrets/x.rs", "pub fn leaked() {}\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["secrets/x.rs".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Review the code",
        &explicit,
        Some(&index),
        None,
    );

    assert_absent(&plan, "secrets/x.rs");
    let records = restrictions_for(&plan, "secrets/x.rs");
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    assert_eq!(records[0].category, ContextCategory::FileRanges);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_restricted_prompt_mention_is_excluded_at_collection() {
    let repo = fixture_repo("restricted-mention");
    write_fixture(&repo, "secrets/x.rs", "pub fn leaked() {}\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Explain secrets/x.rs",
        &[],
        Some(&index),
        None,
    );

    assert_absent(&plan, "secrets/x.rs");
    let records = restrictions_for(&plan, "secrets/x.rs");
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    assert_eq!(records[0].category, ContextCategory::FileRanges);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_restricted_keyword_hit_is_excluded_at_collection() {
    let repo = fixture_repo("restricted-keyword");
    write_fixture(
        &repo,
        "secrets/keyword_probe.rs",
        "pub fn keywordprobe() {}\n",
    );
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    assert!(
        index
            .keyword_search("keywordprobe", 8)
            .iter()
            .any(|hit| hit.path == "secrets/keyword_probe.rs"),
        "the fixture is not a keyword hit, so the test proves nothing"
    );

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "keywordprobe",
        &[],
        Some(&index),
        None,
    );

    assert_absent(&plan, "secrets/keyword_probe.rs");
    let records = restrictions_for(&plan, "secrets/keyword_probe.rs");
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    // A search hit is `SearchResults`, not `FileRanges`: the two are ordered
    // and budgeted differently (§5.3), while the header still says
    // `retrieved_file`.
    assert_eq!(records[0].category, ContextCategory::SearchResults);

    fs::remove_dir_all(repo).unwrap();
}

/// A keyword search sees a file's whole content and a semantic search sees its
/// chunks, so with eight path-named hits and one content-only hit the semantic
/// list includes the content-only path and the keyword list does not.
fn semantic_only_fixture(name: &str, content_only_path: &str) -> PathBuf {
    let repo = temp_dir(name);
    for number in 1..=8 {
        write_fixture(
            &repo,
            &format!("probe/semanticprobe_{number:02}.rs"),
            "// filler\n",
        );
    }
    write_fixture(&repo, content_only_path, "// semanticprobe\n");
    repo
}

#[test]
fn a_restricted_semantic_hit_is_excluded_at_collection() {
    let repo = semantic_only_fixture("restricted-semantic", "aaa/secrets/notes.rs");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let restricted = "aaa/secrets/notes.rs";
    assert!(
        !index
            .keyword_search("semanticprobe", 8)
            .iter()
            .any(|hit| hit.path == restricted),
        "the fixture is a keyword hit too, so the semantic source is not tested"
    );
    assert!(
        index
            .semantic_search("semanticprobe", 8)
            .iter()
            .any(|hit| hit.path == restricted),
        "the fixture is not a semantic hit, so the test proves nothing"
    );

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "semanticprobe",
        &[],
        Some(&index),
        None,
    );

    assert_absent(&plan, restricted);
    let records = restrictions_for(&plan, restricted);
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    assert_eq!(records[0].category, ContextCategory::SearchResults);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_restricted_instruction_path_is_excluded_at_collection() {
    let repo = fixture_repo("restricted-instruction");
    write_fixture(&repo, "secrets/AGENTS.md", "Never leak a token.\n");
    write_fixture(&repo, "secrets/x.rs", "pub fn leaked() {}\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Explain secrets/x.rs",
        &[],
        Some(&index),
        None,
    );

    assert_absent(&plan, "secrets/AGENTS.md");
    let records = restrictions_for(&plan, "secrets/AGENTS.md");
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    assert_eq!(records[0].category, ContextCategory::Instructions);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_restricted_project_rule_is_excluded_at_collection() {
    let repo = fixture_repo("restricted-project-rule");
    let mut config = test_config(&repo);
    config.restricted_patterns = vec!["README.md".to_string()];
    let engine = WorkspaceEngine::new(config);
    let index = engine.indexer.index_repository(&repo).unwrap();

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Review the code",
        &[],
        Some(&index),
        None,
    );

    assert_absent(&plan, "README.md");
    let records = restrictions_for(&plan, "README.md");
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    assert_eq!(records[0].category, ContextCategory::Instructions);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_restricted_path_is_never_an_included_item() {
    let repo = temp_dir("restricted-everywhere");
    write_fixture(&repo, "README.md", "# Fixture\n");
    write_fixture(&repo, "src/lib.rs", "pub fn answer() -> usize { 42 }\n");
    write_fixture(&repo, "secrets/x.rs", "pub fn leaked() {}\n");
    write_fixture(&repo, "secrets/AGENTS.md", "Never leak a token.\n");
    write_fixture(&repo, "aaa/secrets/notes.rs", "// semanticprobe\n");
    for number in 1..=8 {
        write_fixture(
            &repo,
            &format!("probe/semanticprobe_{number:02}.rs"),
            "// filler\n",
        );
    }
    let mut config = test_config(&repo);
    config.restricted_patterns.push("README.md".to_string());
    let engine = WorkspaceEngine::new(config);
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["secrets/x.rs".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Explain secrets/x.rs and semanticprobe",
        &explicit,
        Some(&index),
        None,
    );

    for path in [
        "secrets/x.rs",
        "secrets/AGENTS.md",
        "aaa/secrets/notes.rs",
        "README.md",
    ] {
        assert_absent(&plan, path);
        assert!(
            !restrictions_for(&plan, path).is_empty(),
            "{path} was never recorded as restricted: {:?}",
            plan.manifest.excluded
        );
    }

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn every_collection_source_refuses_a_restricted_path() {
    let repo = fixture_repo("collection-sources");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let restricted = "secrets/api_token.txt";
    // Exhaustive on purpose: a new `CollectionSource` variant fails to compile
    // here until its case says whether the policy applies.
    let sources = [
        CollectionSource::Explicit,
        CollectionSource::Mention,
        CollectionSource::KeywordHit,
        CollectionSource::SemanticHit,
        CollectionSource::Instruction,
        CollectionSource::ProjectRule,
        CollectionSource::Map,
        CollectionSource::Symbols,
        CollectionSource::HookContext,
        CollectionSource::PlanState,
        CollectionSource::Pinned,
    ];
    for source in sources {
        let expected = match source {
            CollectionSource::Explicit
            | CollectionSource::Mention
            | CollectionSource::KeywordHit
            | CollectionSource::SemanticHit
            | CollectionSource::Instruction
            | CollectionSource::ProjectRule
            | CollectionSource::Map
            | CollectionSource::Symbols
            | CollectionSource::Pinned => Some(ExclusionReason::RestrictedPath),
            // These carry session content, not a file path, so there is no
            // path for the policy to refuse.
            CollectionSource::HookContext | CollectionSource::PlanState => None,
        };
        assert_eq!(
            engine
                .context_manager
                .collection_exclusion(source, restricted),
            expected,
            "{source:?} decided wrongly for {restricted}"
        );
    }

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn keyword_hits_are_ordered_before_semantic_hits() {
    let repo = semantic_only_fixture("hit-order", "aaa/notes.rs");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let keyword = index.keyword_search("semanticprobe", 8);
    let semantic = index.semantic_search("semanticprobe", 8);
    assert!(
        keyword
            .iter()
            .all(|hit| hit.path.starts_with("probe/semanticprobe_")),
        "the keyword list is not the path-named files: {keyword:?}"
    );
    assert!(
        semantic.iter().any(|hit| hit.path == "aaa/notes.rs"),
        "the semantic list does not hold the content-only file: {semantic:?}"
    );

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "semanticprobe",
        &[],
        Some(&index),
        None,
    );
    let position = |path: &str| {
        plan.items
            .iter()
            .position(|item| item.path.as_deref() == Some(path))
            .unwrap_or_else(|| panic!("{path} is not in the plan: {:?}", plan.files))
    };
    let semantic_only = position("aaa/notes.rs");
    for hit in &keyword {
        assert!(
            position(&hit.path) < semantic_only,
            "{} followed a semantic hit: {:?}",
            hit.path,
            plan.files
        );
    }

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_keyword_hit_and_a_semantic_hit_for_one_path_become_one_item() {
    let repo = temp_dir("hit-dedup");
    write_fixture(&repo, "src/probe.rs", "pub fn dedupeprobe() {}\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    assert!(
        index
            .keyword_search("dedupeprobe", 8)
            .iter()
            .any(|hit| hit.path == "src/probe.rs")
    );
    assert!(
        index
            .semantic_search("dedupeprobe", 8)
            .iter()
            .any(|hit| hit.path == "src/probe.rs")
    );

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "dedupeprobe",
        &[],
        Some(&index),
        None,
    );

    assert_eq!(
        plan.items
            .iter()
            .filter(|item| item.path.as_deref() == Some("src/probe.rs"))
            .count(),
        1,
        "{:?}",
        plan.files
    );
    assert_eq!(
        plan.files
            .iter()
            .filter(|path| path.as_str() == "src/probe.rs")
            .count(),
        1,
        "{:?}",
        plan.files
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_carried_plan_becomes_a_plan_state_item() {
    let repo = fixture_repo("plan-state");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let session = engine
        .session_store
        .create_session("repo-plan-state", "plan")
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "carry on", "provider", "model")
        .unwrap();
    let mut plan = TaskPlan::new(&task.id, 1);
    plan.steps = vec![
        PlanStep {
            id: "1".to_string(),
            title: "Inspect the parser".to_string(),
            detail: None,
            status: StepStatus::Completed,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
        },
        PlanStep {
            id: "2".to_string(),
            title: "Add the range check".to_string(),
            detail: Some("start with the empty case".to_string()),
            status: StepStatus::InProgress,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
        },
    ];
    engine.session_store.create_plan(&task, &plan).unwrap();
    let stored = engine
        .session_store
        .read_task_plan(&session.id, &task.id)
        .unwrap()
        .expect("the plan was just written");

    let context = assemble(
        &engine,
        &repo,
        "repo-plan-state",
        "Continue the task",
        &[],
        None,
        Some(&stored),
    );

    let item = context
        .items
        .iter()
        .find(|item| item.kind == ContextKind::PlanState)
        .expect("the carried plan is in the plan");
    assert_eq!(item.category, ContextCategory::PlanState);
    assert_eq!(item.path, None);
    assert!(
        item.content.contains("Inspect the parser"),
        "{}",
        item.content
    );
    assert!(
        item.content.contains("Add the range check"),
        "{}",
        item.content
    );
    assert!(
        item.content.contains("start with the empty case"),
        "{}",
        item.content
    );

    fs::remove_dir_all(repo).unwrap();
}

/// `src/upload.rs` declares `upload_retry` on line 42 and `upload_once` on
/// line 44; `src/main.rs` calls the first (the fixture `symbol_index.rs` uses).
fn upload_repo(name: &str) -> PathBuf {
    let repo = temp_dir(name);
    let mut upload = "// filler\n".repeat(41);
    upload.push_str("pub fn upload_retry() {}\n\npub fn upload_once() {}\n");
    write_fixture(&repo, "src/upload.rs", &upload);
    write_fixture(
        &repo,
        "src/main.rs",
        "fn main() {\n    upload_retry();\n}\n",
    );
    repo
}

#[test]
fn the_symbols_item_keeps_the_lowest_provenance_it_contains() {
    let repo = upload_repo("symbols-lowest-provenance");
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
        &index.repository_id,
        "Compare upload_retry and upload_once",
        &[],
        Some(&index),
        None,
    );

    let item = plan
        .items
        .iter()
        .find(|item| item.kind == ContextKind::Symbols)
        .expect("the prompt names two symbols");
    assert!(
        item.content
            .contains("upload_retry (function, src/upload.rs:42)"),
        "{}",
        item.content
    );
    assert!(
        item.content
            .contains("upload_once (function, src/upload.rs:44"),
        "{}",
        item.content
    );
    assert_eq!(item.provenance, Some(Provenance::Heuristic));

    fs::remove_dir_all(repo).unwrap();
}

// ---- Task 3: range deduplication and the range-source finding ----

/// Task 3 discovery, pinned so it cannot go stale silently: **no collection
/// source yields a `LineRange` today.** `indexer::SearchResult` carries a path
/// and a snippet and drops the chunk offsets; `symbol_index::Symbol` carries
/// one starting line and is collected as a rendered block, not a file range.
/// Every file-bearing source is therefore whole-file, and range merging has no
/// production producer yet. The merge rules themselves are unit-tested in
/// `context_manager.rs` (where the private merge function lives); read windows
/// are exercised through `read_file` in `tests/agent_tools.rs`.
///
/// When a source starts supplying ranges this assertion fails, which is the
/// signal to cover the merge through `build_context` and to stop calling the
/// machinery unit-only.
#[test]
fn unpinned_collection_sources_still_yield_whole_files() {
    let repo = temp_dir("range-source-finding");
    write_fixture(
        &repo,
        "src/probe.rs",
        "pub fn rangeprobe() -> usize { 42 }\n",
    );
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    assert!(
        index
            .keyword_search("rangeprobe", 8)
            .iter()
            .any(|hit| hit.path == "src/probe.rs"),
        "the fixture must reach collection as a search hit"
    );

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "rangeprobe src/probe.rs",
        &[],
        Some(&index),
        None,
    );

    assert!(
        plan.items
            .iter()
            .any(|item| item.path.as_deref() == Some("src/probe.rs")),
        "the fixture should be in the plan: {:?}",
        plan.files
    );
    let ranged = plan
        .items
        .iter()
        .filter(|item| item.range.is_some())
        .map(|item| (item.path.clone(), item.range))
        .collect::<Vec<_>>();
    assert!(
        ranged.is_empty(),
        "an unpinned collection source now produces line ranges; cover merging through build_context: {ranged:?}"
    );

    fs::remove_dir_all(repo).unwrap();
}

// ---- Task 4: per-category budgets and the effective limit ----

#[test]
fn each_category_stops_at_its_configured_share() {
    let repo = fixture_repo("share-cap");
    let engine = WorkspaceEngine::new(config_with_shares(
        &repo,
        "context_budget.file_ranges=0.0091\n",
    ));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["src/lib.rs".to_string()];

    let plan = assemble_with_limit(
        &engine,
        &repo,
        &index.repository_id,
        "Explain src/lib.rs and README.md",
        &explicit,
        Some(&index),
        None,
        1_000,
    );

    for usage in &plan.manifest.categories {
        assert!(
            usage.used <= usage.allocated,
            "{:?} used {} of {} allocated",
            usage.category,
            usage.used,
            usage.allocated
        );
        assert!(
            usage.estimated,
            "{:?} must be marked estimated",
            usage.category
        );
    }
    let file_ranges = usage(&plan, ContextCategory::FileRanges);
    assert!(file_ranges.truncated, "{file_ranges:?}");
    assert_eq!(file_ranges.allocated, 9, "0.91% of ~991 tokens");
    assert_eq!(
        file_ranges.used, 9,
        "the explicit file only, at the code ratio's estimate for src/lib.rs"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn an_unused_share_moves_to_later_categories() {
    let repo = fixture_repo("redistribute");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let explicit = vec!["src/lib.rs".to_string()];

    let plan = assemble_with_limit(
        &engine,
        &repo,
        "repo_redistribute",
        "Review",
        &explicit,
        None,
        None,
        4_000,
    );

    let required: usize = plan
        .items
        .iter()
        .filter(|item| item.category == ContextCategory::Required)
        .map(|item| item.tokens)
        .sum();
    let available = plan.manifest.limit.tokens - required;
    let nominal = available * 2_500 / 10_000;
    let file_ranges = usage(&plan, ContextCategory::FileRanges);
    assert!(
        file_ranges.allocated > nominal,
        "unused shares should reach FileRanges: allocated {}, nominal {nominal}",
        file_ranges.allocated
    );
    assert!(file_ranges.used <= file_ranges.allocated);
    assert_eq!(
        file_ranges.used, 9,
        "the explicit file fits either way, at the code ratio's estimate for src/lib.rs"
    );
    assert!(
        usage(&plan, ContextCategory::Pinned).used == 0,
        "Pinned is reserved and releases its share"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_category_over_its_share_drops_its_lowest_priority_members() {
    let repo = fixture_repo("file-drop");
    write_fixture(
        &repo,
        "src/mentioned_only.rs",
        &format!("// mentioned only\n{}\n", "pub fn filler() {}\n".repeat(20)),
    );
    let engine = WorkspaceEngine::new(config_with_shares(
        &repo,
        "context_budget.file_ranges=0.10\n",
    ));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["src/lib.rs".to_string()];

    let plan = assemble_with_limit(
        &engine,
        &repo,
        &index.repository_id,
        "Explain src/lib.rs and src/mentioned_only.rs",
        &explicit,
        Some(&index),
        None,
        300,
    );

    let paths: Vec<&str> = plan
        .items
        .iter()
        .filter_map(|item| item.path.as_deref())
        .collect();
    assert!(
        paths.contains(&"src/lib.rs"),
        "the explicit path has priority: {paths:?}"
    );
    assert!(
        !paths.contains(&"src/mentioned_only.rs"),
        "the mentioned path should be dropped: {paths:?}"
    );
    let records = budget_records(&plan, "src/mentioned_only.rs");
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    assert_eq!(records[0].category, ContextCategory::FileRanges);
    assert!(usage(&plan, ContextCategory::FileRanges).truncated);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn semantic_hits_are_dropped_before_exact_matches() {
    let repo = semantic_only_fixture("search-budget", "zzz/semantic_only.rs");
    let engine = WorkspaceEngine::new(config_with_shares(
        &repo,
        "context_budget.search_results=0.10\n",
    ));
    let index = engine.indexer.index_repository(&repo).unwrap();
    assert!(
        !index
            .keyword_search("semanticprobe", 8)
            .iter()
            .any(|hit| hit.path == "zzz/semantic_only.rs"),
        "the fixture is a keyword hit too, so the semantic drop is not tested"
    );
    assert!(
        index
            .semantic_search("semanticprobe", 8)
            .iter()
            .any(|hit| hit.path == "zzz/semantic_only.rs"),
        "the fixture is not a semantic hit, so the test proves nothing"
    );

    let plan = assemble_with_limit(
        &engine,
        &repo,
        &index.repository_id,
        "semanticprobe",
        &[],
        Some(&index),
        None,
        100,
    );

    let included: Vec<&str> = plan
        .items
        .iter()
        .filter(|item| item.category == ContextCategory::SearchResults)
        .filter_map(|item| item.path.as_deref())
        .collect();
    assert!(
        !included.is_empty(),
        "some exact hit must survive the tiny share: {:?}",
        plan.manifest.excluded
    );
    assert!(
        included.iter().all(|path| index
            .keyword_search("semanticprobe", 8)
            .iter()
            .any(|hit| &hit.path == path)),
        "every surviving hit is an exact keyword hit: {included:?}"
    );
    let records = budget_records(&plan, "zzz/semantic_only.rs");
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    assert_eq!(records[0].category, ContextCategory::SearchResults);
    assert!(usage(&plan, ContextCategory::SearchResults).truncated);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn the_most_specific_agents_md_survives_truncation() {
    let repo = temp_dir("instruction-truncation");
    write_fixture(
        &repo,
        "AGENTS.md",
        &format!(
            "# Root rules\n{}\n",
            "Always run the full suite.\n".repeat(30)
        ),
    );
    write_fixture(
        &repo,
        "src/deep/AGENTS.md",
        "# Nested rules\nUse the local helper.\n",
    );
    write_fixture(
        &repo,
        "src/deep/lib.rs",
        "pub fn answer() -> usize { 42 }\n",
    );
    let engine = WorkspaceEngine::new(config_with_shares(
        &repo,
        "context_budget.instructions=0.01\n",
    ));
    let index = engine.indexer.index_repository(&repo).unwrap();

    let plan = assemble_with_limit(
        &engine,
        &repo,
        &index.repository_id,
        "Review src/deep/lib.rs",
        &[],
        Some(&index),
        None,
        2_000,
    );

    let instruction_paths: Vec<&str> = plan
        .items
        .iter()
        .filter(|item| item.category == ContextCategory::Instructions)
        .filter_map(|item| item.path.as_deref())
        .collect();
    assert_eq!(instruction_paths, vec!["src/deep/AGENTS.md"]);
    let records = budget_records(&plan, "AGENTS.md");
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    assert_eq!(records[0].category, ContextCategory::Instructions);
    assert!(usage(&plan, ContextCategory::Instructions).truncated);

    // The prompt keeps today's order for whatever survives: root first.
    let roomy_engine = WorkspaceEngine::new(test_config(&repo));
    let roomy = assemble(
        &roomy_engine,
        &repo,
        &index.repository_id,
        "Review src/deep/lib.rs",
        &[],
        Some(&index),
        None,
    );
    let roomy_paths: Vec<&str> = roomy
        .items
        .iter()
        .filter(|item| item.category == ContextCategory::Instructions)
        .filter_map(|item| item.path.as_deref())
        .collect();
    assert_eq!(roomy_paths, vec!["AGENTS.md", "src/deep/AGENTS.md"]);

    fs::remove_dir_all(repo).unwrap();
}

/// A repository with enough structure that the map's full render is well over
/// a small category share.
fn map_heavy_repo(name: &str) -> PathBuf {
    let repo = temp_dir(name);
    write_fixture(&repo, "README.md", "# Map fixture\n");
    for directory in 1..=24 {
        for file in 1..=3 {
            write_fixture(
                &repo,
                &format!("area{directory:02}/module{file}.rs"),
                "pub fn helper() -> usize { 0 }\n",
            );
        }
    }
    repo
}

#[test]
fn a_block_item_too_large_for_its_share_is_rerendered_smaller() {
    let repo = map_heavy_repo("block-rerender");
    let engine = WorkspaceEngine::new(config_with_shares(
        &repo,
        "context_budget.repository_map=0.05\n",
    ));
    let index = engine.indexer.index_repository(&repo).unwrap();

    let small = assemble_with_limit(
        &engine,
        &repo,
        &index.repository_id,
        "Summarize the repository",
        &[],
        Some(&index),
        None,
        2_000,
    );
    let small_map = small
        .items
        .iter()
        .find(|item| item.kind == ContextKind::RepositoryMap)
        .expect("the map should survive at a smaller ceiling");
    assert!(!small_map.content.is_empty());
    let truncated = small
        .manifest
        .excluded
        .iter()
        .filter(|record| {
            record.reason == ExclusionReason::Truncated
                && record.category == ContextCategory::RepositoryMap
        })
        .count();
    assert_eq!(truncated, 1, "{:?}", small.manifest.excluded);
    assert!(usage(&small, ContextCategory::RepositoryMap).truncated);

    let roomy = assemble_with_limit(
        &engine,
        &repo,
        &index.repository_id,
        "Summarize the repository",
        &[],
        Some(&index),
        None,
        100_000,
    );
    let roomy_map = roomy
        .items
        .iter()
        .find(|item| item.kind == ContextKind::RepositoryMap)
        .expect("the map should be there at a roomy limit");
    assert!(
        roomy_map.tokens > small_map.tokens,
        "the small share must shorten the block: {} vs {}",
        small_map.tokens,
        roomy_map.tokens
    );
    assert!(
        !roomy.manifest.excluded.iter().any(|record| {
            record.reason == ExclusionReason::Truncated
                && record.category == ContextCategory::RepositoryMap
        }),
        "a roomy share is not a truncation: {:?}",
        roomy.manifest.excluded
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn with_no_observed_profile_the_limit_is_the_config_budget_labelled_declared() {
    let repo = fixture_repo("declared-limit");
    let engine = WorkspaceEngine::new(test_config(&repo));

    let plan = assemble(&engine, &repo, "repo_declared", "Review", &[], None, None);

    assert_eq!(
        plan.manifest.limit.tokens,
        engine.config.context_token_budget()
    );
    assert_eq!(plan.manifest.limit.tokens, 16_000);
    assert_eq!(plan.manifest.limit.source, LimitSource::Declared);
    assert_eq!(plan.manifest.limit.safety_margin_bps, 0);
    assert_eq!(plan.manifest.limit.window, None);

    fs::remove_dir_all(repo).unwrap();
}

// ---- Task 5: the manifest, its audit record, and the callers ----

/// Any record with this reason, and this path when one is named.
fn has_reason(plan: &ContextPlan, reason: ExclusionReason, path: Option<&str>) -> bool {
    plan.manifest.excluded.iter().any(|record| {
        record.reason == reason && path.is_none_or(|path| record.path.as_deref() == Some(path))
    })
}

#[test]
fn the_manifest_lists_every_included_item_by_path_range_and_root() {
    let repo = fixture_repo("manifest-included");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["src/lib.rs".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Explain src/lib.rs",
        &explicit,
        Some(&index),
        None,
    );

    assert_eq!(plan.manifest.repository_id, index.repository_id);
    assert_eq!(plan.manifest.task_id, "task-2");
    assert!(plan.manifest.assembled_at_ms > 0);
    assert_eq!(plan.manifest.total_tokens, plan.token_estimate);
    assert_eq!(plan.manifest.included.len(), plan.items.len());
    for (item, included) in plan.items.iter().zip(&plan.manifest.included) {
        assert_eq!(included.category, item.category);
        assert_eq!(included.kind, item.kind);
        assert_eq!(included.path, item.path);
        assert_eq!(included.range, item.range);
        assert_eq!(included.tokens, item.tokens);
        assert_eq!(included.root_path, item.root_path);
        assert_eq!(included.provenance, item.provenance);
    }

    let file = plan
        .items
        .iter()
        .position(|item| item.path.as_deref() == Some("src/lib.rs"))
        .expect("the explicit file should be included");
    assert_eq!(
        plan.manifest.included[file].path.as_deref(),
        Some("src/lib.rs")
    );
    assert_eq!(plan.manifest.included[file].range, None);
    // The repository root is the map's `""`, and this item belongs to it.
    assert_eq!(plan.manifest.included[file].root_path.as_deref(), Some(""));

    let request = plan
        .items
        .iter()
        .position(|item| item.category == ContextCategory::Required)
        .expect("the prompt is a required item");
    assert_eq!(plan.manifest.included[request].path, None);
    assert_eq!(plan.manifest.included[request].range, None);
    assert_eq!(plan.manifest.included[request].root_path, None);

    fs::remove_dir_all(repo).unwrap();
}

/// `DeduplicatedIntoRange` is not producible through `build_context` today:
/// no collection source yields a line range (Task 3's discovery), so the merge
/// runs only in its unit tests. It is covered end to end here for the other
/// three reasons, and by `every_exclusion_reason_reaches_the_manifest` in
/// `context_manager.rs` for the manifest's carry-through of every variant.
#[test]
fn the_manifest_names_the_reason_for_every_exclusion() {
    // RestrictedPath: a non-normalised explicit path the policy refuses.
    let repo = fixture_repo("manifest-reason-restricted");
    write_fixture(&repo, "secrets/x.rs", "pub fn leaked() {}\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["./secrets/x.rs".to_string()];
    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Review the file",
        &explicit,
        Some(&index),
        None,
    );
    assert!(
        has_reason(&plan, ExclusionReason::RestrictedPath, Some("secrets/x.rs")),
        "{:?}",
        plan.manifest.excluded
    );
    fs::remove_dir_all(repo).unwrap();

    // CategoryBudgetExhausted: one per cent of a 1000-token limit holds the
    // explicit file and drops the mentioned one.
    let repo = fixture_repo("manifest-reason-budget");
    write_fixture(
        &repo,
        "src/mentioned_only.rs",
        &format!("// mentioned only\n{}\n", "pub fn filler() {}\n".repeat(20)),
    );
    let engine = WorkspaceEngine::new(config_with_shares(
        &repo,
        "context_budget.file_ranges=0.01\n",
    ));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["src/lib.rs".to_string()];
    let plan = assemble_with_limit(
        &engine,
        &repo,
        &index.repository_id,
        "Explain src/lib.rs and src/mentioned_only.rs",
        &explicit,
        Some(&index),
        None,
        1_000,
    );
    assert!(
        has_reason(
            &plan,
            ExclusionReason::CategoryBudgetExhausted,
            Some("src/mentioned_only.rs")
        ),
        "{:?}",
        plan.manifest.excluded
    );
    fs::remove_dir_all(repo).unwrap();

    // Truncated: the map is re-rendered at a smaller share.
    let repo = map_heavy_repo("manifest-reason-truncated");
    let engine = WorkspaceEngine::new(config_with_shares(
        &repo,
        "context_budget.repository_map=0.05\n",
    ));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let plan = assemble_with_limit(
        &engine,
        &repo,
        &index.repository_id,
        "Summarize the repository",
        &[],
        Some(&index),
        None,
        2_000,
    );
    assert!(
        has_reason(&plan, ExclusionReason::Truncated, None),
        "{:?}",
        plan.manifest.excluded
    );
    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn no_manifest_field_contains_item_content() {
    const MARKER: &str = "content_marker_zzq";
    let repo = fixture_repo("manifest-content");
    write_fixture(
        &repo,
        "src/leaky.rs",
        &format!("// {MARKER}\npub const KEY: &str = \"{AWS_ACCESS_KEY}\";\n"),
    );
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = MockModelAdapter::new("Done.");

    let prompt = format!("Review src/leaky.rs and the marker {MARKER}");
    ask(&engine, &repo, None, &prompt, &mut adapter);

    let events = audit_events(&engine.config.data_dir, "context_assembled");
    assert_eq!(events.len(), 1, "{events:?}");
    let manifest_json = events[0]["manifest"]
        .as_str()
        .expect("the manifest is a JSON field");
    // The file was included, or the absence of its content proves nothing.
    assert!(
        manifest_json.contains("src/leaky.rs"),
        "the file is not in the manifest: {manifest_json}"
    );
    assert!(
        !manifest_json.contains(AWS_ACCESS_KEY),
        "the manifest carries the secret: {manifest_json}"
    );
    assert!(
        !manifest_json.contains(MARKER),
        "the manifest carries item content: {manifest_json}"
    );
    assert!(
        !manifest_json.contains("pub const KEY"),
        "the manifest carries item content: {manifest_json}"
    );

    // The audit file as a whole, not only this event: a content leak anywhere
    // in the trail is the failure the manifest exists to make impossible.
    let audit = fs::read_to_string(engine.config.data_dir.join("audit/events.jsonl")).unwrap();
    assert!(
        !audit.contains(AWS_ACCESS_KEY),
        "the seeded secret reached the audit file"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn the_manifest_is_recorded_once_per_turn_as_context_assembled() {
    let repo = fixture_repo("manifest-audit");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let mut adapter = MockModelAdapter::new("Done.");

    ask(&engine, &repo, None, "Review src/lib.rs", &mut adapter);

    let events = audit_events(&engine.config.data_dir, "context_assembled");
    assert_eq!(events.len(), 1, "one event per turn: {events:?}");
    let event = &events[0];
    assert_eq!(event["repositoryId"], index.repository_id);
    let task_id = event["taskId"].as_str().expect("a task id");
    assert!(!task_id.is_empty());
    let token_estimate = event["tokenEstimate"]
        .as_str()
        .and_then(|value| value.parse::<usize>().ok())
        .expect("a token estimate");

    let manifest: ContextManifest =
        serde_json::from_str(event["manifest"].as_str().expect("a manifest field"))
            .expect("the manifest must be valid JSON");
    assert_eq!(manifest.repository_id, index.repository_id);
    assert_eq!(manifest.task_id, task_id);
    assert_eq!(manifest.total_tokens, token_estimate);
    assert_eq!(
        manifest
            .categories
            .iter()
            .map(|usage| usage.items)
            .sum::<usize>(),
        manifest.included.len(),
        "every included item belongs to exactly one category"
    );
    assert!(
        manifest
            .included
            .iter()
            .any(|item| item.path.as_deref() == Some("src/lib.rs")),
        "the explicit file is missing: {manifest:?}"
    );
    assert!(
        !serde_json::to_string(event)
            .unwrap()
            .contains("pub fn answer"),
        "the audit event carries item content: {event:?}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn the_edit_flow_records_its_manifest_too() {
    let repo = fixture_repo("manifest-edit-audit");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let response = "DAMAIAN_EDIT_V1\nSUMMARY: Update lib\nFILE: src/lib.rs\nSTATUS: modified\nCONTENT:\npub fn answer() -> usize { 43 }\nEND_FILE\nEND_PATCH\n";
    let mut adapter = MockModelAdapter::new(response);

    engine
        .edit_orchestrator
        .propose_edit(&repo, "Update src/lib.rs", &[], None, &mut adapter)
        .expect("the edit should be proposed");

    let events = audit_events(&engine.config.data_dir, "context_assembled");
    assert_eq!(events.len(), 1, "one event per edit: {events:?}");
    let event = &events[0];
    assert_eq!(event["repositoryId"], index.repository_id);
    let manifest: ContextManifest =
        serde_json::from_str(event["manifest"].as_str().expect("a manifest field"))
            .expect("the manifest must be valid JSON");
    assert_eq!(manifest.repository_id, index.repository_id);
    assert!(
        manifest
            .included
            .iter()
            .any(|item| item.path.as_deref() == Some("src/lib.rs")),
        "the edit's context is missing: {manifest:?}"
    );
    assert_eq!(
        event["tokenEstimate"].as_str().unwrap(),
        manifest.total_tokens.to_string()
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn context_files_are_the_unique_included_paths_in_inclusion_order() {
    let repo = fixture_repo("manifest-files");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    // `src/lib.rs` is named both explicitly and in the prompt, in a
    // non-normalised spelling; `README.md` is mentioned and is also a project
    // rule. Each path must appear once, in first-inclusion order.
    let explicit = vec!["./src/lib.rs".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Explain src/lib.rs and README.md",
        &explicit,
        Some(&index),
        None,
    );

    let mut expected: Vec<String> = Vec::new();
    for item in &plan.items {
        if let Some(path) = &item.path
            && !expected.contains(path)
        {
            expected.push(path.clone());
        }
    }
    assert_eq!(plan.files, expected);
    assert_eq!(plan.included_paths(), expected.as_slice());
    assert_eq!(
        plan.files
            .iter()
            .filter(|path| *path == "src/lib.rs")
            .count(),
        1,
        "the same file was included twice: {:?}",
        plan.files
    );
    assert!(
        plan.files.iter().any(|path| path == "README.md"),
        "{:?}",
        plan.files
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_dot_prefixed_restricted_path_is_recorded() {
    let repo = fixture_repo("restricted-dot");
    write_fixture(&repo, "secrets/x.rs", "pub fn leaked() {}\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    // `ignore::normalize_path` already strips `./` inside the rule matcher, so
    // this spelling reached collection before Task 5. What was not normalised
    // was the record: it named `./secrets/x.rs`, not the path the read path
    // refuses.
    let explicit = vec!["./secrets/x.rs".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Review the file",
        &explicit,
        Some(&index),
        None,
    );

    assert_absent(&plan, "secrets/x.rs");
    let records = restrictions_for(&plan, "secrets/x.rs");
    assert_eq!(records.len(), 1, "{:?}", plan.manifest.excluded);
    assert_eq!(records[0].category, ContextCategory::FileRanges);
    assert!(
        !plan
            .manifest
            .excluded
            .iter()
            .any(|record| record.path.as_deref() == Some("./secrets/x.rs")),
        "a restriction record kept a non-normalised spelling: {:?}",
        plan.manifest.excluded
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn an_absolute_restricted_path_inside_the_repository_is_recorded() {
    let repo = fixture_repo("restricted-absolute");
    write_fixture(&repo, "secrets/x.rs", "pub fn leaked() {}\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec![repo.join("secrets/x.rs").to_string_lossy().to_string()];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Review the file",
        &explicit,
        Some(&index),
        None,
    );

    assert_absent(&plan, "secrets/x.rs");
    let records = restrictions_for(&plan, "secrets/x.rs");
    assert_eq!(
        records.len(),
        1,
        "the read refused it but no record says why: {:?}",
        plan.manifest.excluded
    );
    assert_eq!(records[0].category, ContextCategory::FileRanges);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_binary_file_read_is_recorded_as_binary() {
    let repo = fixture_repo("manifest-binary");
    fs::write(repo.join("src/image.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["src/image.bin".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Review the image",
        &explicit,
        Some(&index),
        None,
    );

    assert_absent(&plan, "src/image.bin");
    let records = plan
        .manifest
        .excluded
        .iter()
        .filter(|record| {
            record.reason == ExclusionReason::Binary
                && record.path.as_deref() == Some("src/image.bin")
        })
        .count();
    assert_eq!(records, 1, "{:?}", plan.manifest.excluded);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn an_oversized_file_read_is_recorded_too_large() {
    let repo = fixture_repo("manifest-too-large");
    write_fixture(&repo, "src/big.rs", &"// a long line\n".repeat(64));
    let mut config = test_config(&repo);
    config.apply_overlay(ConfigOverlay::parse("max_file_bytes=128\n").unwrap());
    let engine = WorkspaceEngine::new(config);
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["src/big.rs".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Review the big file",
        &explicit,
        Some(&index),
        None,
    );

    assert_absent(&plan, "src/big.rs");
    let records = plan
        .manifest
        .excluded
        .iter()
        .filter(|record| {
            record.reason == ExclusionReason::TooLarge
                && record.path.as_deref() == Some("src/big.rs")
        })
        .count();
    assert_eq!(records, 1, "{:?}", plan.manifest.excluded);

    fs::remove_dir_all(repo).unwrap();
}

// ---- Task 5 review fix: every included file names its owning root ----

/// A repository with a nested project root (`packages/api`), so "the root that
/// owns this path" has a non-empty answer to give.
fn monorepo_fixture(name: &str) -> PathBuf {
    let repo = temp_dir(name);
    write_fixture(
        &repo,
        "Cargo.toml",
        "[workspace]\nmembers = [\"packages/api\"]\n",
    );
    write_fixture(&repo, "README.md", "# Root\n");
    write_fixture(&repo, "src/lib.rs", "pub fn root_level() -> usize { 1 }\n");
    write_fixture(
        &repo,
        "packages/api/Cargo.toml",
        "[package]\nname = \"api\"\nversion = \"0.1.0\"\n",
    );
    write_fixture(
        &repo,
        "packages/api/src/lib.rs",
        "pub fn api() -> usize { 2 }\n",
    );
    repo
}

fn included_root(plan: &ContextPlan, path: &str) -> Option<String> {
    plan.manifest
        .included
        .iter()
        .find(|included| included.path.as_deref() == Some(path))
        .unwrap_or_else(|| panic!("{path} is not in the manifest: {:?}", plan.files))
        .root_path
        .clone()
}

/// The kinds a collection source reads from a repository file. A hook's
/// `hook:id` label and a plan's text have a `path`-shaped label too, so the
/// assertion is by kind rather than by `path.is_some()`.
const FILE_KINDS: [ContextKind; 4] = [
    ContextKind::ExplicitFile,
    ContextKind::AgentInstruction,
    ContextKind::ProjectRule,
    ContextKind::RetrievedFile,
];

#[test]
fn every_included_file_carries_its_owning_root() {
    let repo = monorepo_fixture("owning-root");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec![
        "packages/api/src/lib.rs".to_string(),
        "src/lib.rs".to_string(),
    ];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Review packages/api/src/lib.rs and src/lib.rs",
        &explicit,
        Some(&index),
        None,
    );

    assert_eq!(
        included_root(&plan, "packages/api/src/lib.rs").as_deref(),
        Some("packages/api"),
        "the nested root must own its file: {:?}",
        plan.manifest.included
    );
    assert_eq!(
        included_root(&plan, "src/lib.rs").as_deref(),
        Some(""),
        "the repository root is the map's `\"\"` and owns a root-level file"
    );
    for (item, included) in plan.items.iter().zip(&plan.manifest.included) {
        if !FILE_KINDS.contains(&item.kind) {
            continue;
        }
        assert_eq!(
            item.root_path, included.root_path,
            "the manifest must mirror the item's root: {item:?}"
        );
        assert!(
            item.root_path.is_some(),
            "a file item has no owning root: {item:?}"
        );
    }

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn the_owning_root_is_set_even_with_the_map_item_turned_off() {
    let repo = monorepo_fixture("owning-root-no-map");
    let mut config = test_config(&repo);
    config.apply_overlay(
        ConfigOverlay::parse(
            "repository_map_max_tokens=0\n\
             symbol_context_max_tokens=0\n",
        )
        .unwrap(),
    );
    let engine = WorkspaceEngine::new(config);
    let index = engine.indexer.index_repository(&repo).unwrap();
    let explicit = vec!["packages/api/src/lib.rs".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        &index.repository_id,
        "Review packages/api/src/lib.rs",
        &explicit,
        Some(&index),
        None,
    );

    for kind in [ContextKind::RepositoryMap, ContextKind::Symbols] {
        assert!(
            !plan.items.iter().any(|item| item.kind == kind),
            "{kind:?} must stay out when its ceiling is 0"
        );
    }
    assert_eq!(
        included_root(&plan, "packages/api/src/lib.rs").as_deref(),
        Some("packages/api"),
        "the root is resolved from the map even when the map item is off: {:?}",
        plan.manifest.included
    );

    fs::remove_dir_all(repo).unwrap();
}

// ---- Task 6: a content-aware estimate ----

/// The three paths that select the three measured ratios. ASCII fixture
/// names, so a character is a byte and `chars / tokens` needs no conversion.
const PROSE_PATH: &str = "docs/notes.md";
const CODE_PATH: &str = "src/billing.rs";
const STRUCTURED_PATH: &str = "assets/config.json";

#[test]
fn prose_code_and_json_get_different_estimates_for_the_same_length() {
    let text = "x".repeat(1_000);
    let prose = TokenEstimate::for_item(ContextKind::AgentInstruction, Some(PROSE_PATH));
    let code = TokenEstimate::for_item(ContextKind::RetrievedFile, Some(CODE_PATH));
    let structured = TokenEstimate::for_item(ContextKind::RetrievedFile, Some(STRUCTURED_PATH));

    assert_eq!(prose, TokenEstimate::Prose);
    assert_eq!(code, TokenEstimate::Code);
    assert_eq!(structured, TokenEstimate::Structured);

    // The measured ratios of proposal §5.8, at 1,000 bytes: 4, 3.6 and 2.4
    // bytes per token.
    assert_eq!(prose.tokens(&text), 250, "prose keeps 4 bytes per token");
    assert_eq!(code.tokens(&text), 278, "1,000 * 5 / 18, rounded up");
    assert_eq!(structured.tokens(&text), 417, "1,000 * 5 / 12, rounded up");

    let mut estimates = [
        prose.tokens(&text),
        code.tokens(&text),
        structured.tokens(&text),
    ];
    estimates.sort_unstable();
    assert_eq!(estimates, [250, 278, 417], "three types, three estimates");
}

#[test]
fn an_identifier_heavy_snippet_is_estimated_higher_than_prose_of_the_same_length() {
    let code =
        "let total_invoice_amount = calculate_line_item_subtotal(quantity, unit_price_cents);";
    let mut prose = String::from(
        "The invoice total is calculated from each line item's quantity and unit price.",
    );
    while prose.len() < code.len() {
        prose.push('.');
    }
    prose.truncate(code.len());
    assert_eq!(
        code.len(),
        prose.len(),
        "the comparison is about content type"
    );

    let code_estimate = TokenEstimate::for_item(ContextKind::RetrievedFile, Some(CODE_PATH));
    let prose_estimate = TokenEstimate::for_item(ContextKind::AgentInstruction, Some(PROSE_PATH));
    assert!(
        code_estimate.tokens(code) > prose_estimate.tokens(&prose),
        "identifier-heavy code ({}) must be estimated above prose ({})",
        code_estimate.tokens(code),
        prose_estimate.tokens(&prose)
    );
}

#[test]
fn the_estimate_is_monotone_in_length() {
    for (kind, path) in [
        (ContextKind::AgentInstruction, PROSE_PATH),
        (ContextKind::RetrievedFile, CODE_PATH),
        (ContextKind::RetrievedFile, STRUCTURED_PATH),
    ] {
        let estimate = TokenEstimate::for_item(kind, Some(path));
        let mut previous = 0;
        for length in [1, 2, 3, 4, 5, 7, 8, 15, 16, 31, 64, 255, 1_000, 4_096] {
            let tokens = estimate.tokens(&"x".repeat(length));
            assert!(
                tokens >= previous,
                "{path} at {length} bytes estimated {tokens}, below {previous}"
            );
            assert!(tokens > 0, "{path} at {length} bytes estimated nothing");
            previous = tokens;
        }
        assert!(
            previous > estimate.tokens("x"),
            "{path} must not be flat in length"
        );
    }
}

#[test]
fn the_estimate_is_never_zero_for_non_empty_content() {
    for (kind, path) in [
        (ContextKind::AgentInstruction, PROSE_PATH),
        (ContextKind::RetrievedFile, CODE_PATH),
        (ContextKind::RetrievedFile, STRUCTURED_PATH),
    ] {
        let estimate = TokenEstimate::for_item(kind, Some(path));
        for length in 1..=12 {
            assert!(
                estimate.tokens(&"x".repeat(length)) >= 1,
                "{path} estimated nothing for {length} bytes"
            );
        }
    }
}

#[test]
fn the_assembly_measures_each_item_with_its_content_type_ratio() {
    let repo = fixture_repo("estimate-wiring");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let explicit = vec!["src/lib.rs".to_string(), "README.md".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        "repo_estimate",
        "Review",
        &explicit,
        None,
        None,
    );

    for path in ["src/lib.rs", "README.md"] {
        let item = plan
            .items
            .iter()
            .find(|item| item.path.as_deref() == Some(path))
            .unwrap_or_else(|| panic!("{path} should be an included item"));
        let estimate = TokenEstimate::for_item(item.kind, item.path.as_deref());
        assert_eq!(
            item.tokens,
            estimate.tokens(&item.content),
            "{path} was not measured with its content type's ratio"
        );
    }

    let code = plan
        .items
        .iter()
        .find(|item| item.path.as_deref() == Some("src/lib.rs"))
        .unwrap();
    assert!(
        code.tokens > code.content.len().div_ceil(4),
        "the code ratio must differ from len / 4: {} vs {}",
        code.tokens,
        code.content.len().div_ceil(4)
    );
    let prose = plan
        .items
        .iter()
        .find(|item| item.path.as_deref() == Some("README.md"))
        .unwrap();
    assert_eq!(
        prose.tokens,
        prose.content.len().div_ceil(4),
        "prose keeps today's figure"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn each_category_reports_whether_its_tokens_are_measured_or_estimated() {
    let repo = fixture_repo("estimate-marked");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let explicit = vec!["src/lib.rs".to_string()];

    let plan = assemble(
        &engine,
        &repo,
        "repo_marked",
        "Explain src/lib.rs",
        &explicit,
        None,
        None,
    );

    assert_eq!(plan.manifest.categories.len(), 11);
    for usage in &plan.manifest.categories {
        assert!(
            usage.estimated,
            "{:?} reports an estimate as measured",
            usage.category
        );
    }
    assert_eq!(
        plan.manifest.total_tokens,
        plan.items.iter().map(|item| item.tokens).sum::<usize>(),
        "the manifest total is the sum of the estimates"
    );

    fs::remove_dir_all(repo).unwrap();
}

/// Compares the estimate recorded with each request against the input tokens
/// the provider reported, over an existing Damaian data directory. Offline: it
/// reads `model_request_prepared` from the audit log and `task_usage_recorded`
/// from the session logs, reads no API key, and writes nothing.
///
/// Ignored because it inspects a real data directory instead of a fixture. Run
/// it manually:
///
/// ```text
/// DAMAIAN_CALIBRATION_DATA_DIR="$HOME/Library/Application Support/DamaianClient" \
///   CARGO_TARGET_DIR="$PWD/target" \
///   cargo nextest run -p workspace-engine --test context_assembly \
///   --run-ignored ignored-only -E 'test(calibration_compares)'
/// ```
///
/// With the variable unset it prints how to set it and returns.
#[test]
#[ignore = "reads a real Damaian data directory; run manually (see the doc comment)"]
fn calibration_compares_estimates_with_recorded_usage() {
    let Some(data_dir) = std::env::var_os("DAMAIAN_CALIBRATION_DATA_DIR") else {
        println!(
            "calibration_compares_estimates_with_recorded_usage needs a data directory: \
             DAMAIAN_CALIBRATION_DATA_DIR=<data dir> CARGO_TARGET_DIR=\"$PWD/target\" \
             cargo nextest run -p workspace-engine --test context_assembly \
             --run-ignored ignored-only -E 'test(calibration_compares)'"
        );
        return;
    };
    let data_dir = PathBuf::from(data_dir);

    // Estimates are recorded per request round; usage is recorded per call.
    // Both are keyed by (session, task) and paired in file order, which is the
    // order the rounds were attempted in.
    let mut estimates: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for event in audit_events(&data_dir, "model_request_prepared") {
        let (Some(session), Some(task)) = (event["sessionId"].as_str(), event["taskId"].as_str())
        else {
            continue;
        };
        let Some(estimate) = event["tokenEstimate"]
            .as_str()
            .and_then(|value| value.parse::<usize>().ok())
        else {
            continue;
        };
        estimates
            .entry((session.to_string(), task.to_string()))
            .or_default()
            .push(estimate);
    }

    let measured = recorded_input_tokens(&data_dir);
    let mut pairs: Vec<(String, usize, u64)> = Vec::new();
    let mut unpaired = 0usize;
    for (key, estimates) in &estimates {
        let Some(measured) = measured.get(key) else {
            unpaired += estimates.len();
            continue;
        };
        for (estimate, actual) in estimates.iter().zip(measured) {
            pairs.push((format!("{} / {}", key.1, key.0), *estimate, *actual));
        }
        unpaired += estimates.len().saturating_sub(measured.len());
        unpaired += measured.len().saturating_sub(estimates.len());
    }
    pairs.sort_by(|left, right| left.0.cmp(&right.0));

    if pairs.is_empty() {
        println!(
            "no paired model_request_prepared / task_usage_recorded rounds in {}: nothing to compare",
            data_dir.display()
        );
        return;
    }

    let mut estimated_total = 0u64;
    let mut measured_total = 0u64;
    for (key, estimate, actual) in &pairs {
        estimated_total += *estimate as u64;
        measured_total += actual;
        let bps = (*estimate as u128 * 10_000) / u128::from(*actual);
        println!(
            "task {key}: estimate {estimate}, measured {actual} input tokens, {:.2}% of measured",
            format_basis_points(bps)
        );
    }
    let overall = (u128::from(estimated_total) * 10_000) / u128::from(measured_total);
    println!(
        "{} paired rounds ({unpaired} unpaired): estimate {estimated_total}, measured {measured_total} input tokens, {:.2}%",
        pairs.len(),
        format_basis_points(overall)
    );
    println!(
        "positive error means the estimate runs high; spec 19 §7 measured the \
         whole-request len / 4 estimate at +2.5% to +6.3% on DeepSeek"
    );
}

/// `model_request_prepared`'s token estimates, paired with the usage recorded
/// for the same (session, task). Reads `sessions/*.jsonl`; the file stem is the
/// session id the audit event names.
fn recorded_input_tokens(data_dir: &Path) -> HashMap<(String, String), Vec<u64>> {
    let mut measured: HashMap<(String, String), Vec<u64>> = HashMap::new();
    let Ok(entries) = fs::read_dir(data_dir.join("sessions")) else {
        return measured;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let Some(session) = entry
            .path()
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let Ok(contents) = fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in contents.lines() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if event["eventType"] != "task_usage_recorded" {
                continue;
            }
            let (Some(task), Some(input)) =
                (event["taskId"].as_str(), event["inputTokens"].as_u64())
            else {
                continue;
            };
            measured
                .entry((session.clone(), task.to_string()))
                .or_default()
                .push(input);
        }
    }
    measured
}

/// Basis points as a percentage with two decimals, in integer arithmetic: the
/// same reason assembly decides inclusion without floats.
fn format_basis_points(bps: u128) -> String {
    format!("{}.{:02}", bps / 100, bps % 100)
}

/// The live half of the calibration. Sends three fixed documents — a prose
/// paragraph, a Rust function and a JSON object, each about 1–2 kB — one per
/// call, and prints the provider-reported input tokens against the document's
/// size for each.
///
/// It resolves the key exactly as a normal non-desktop turn does: the variable
/// `config.model_api_key_env` names in the data directory's `user.conf`, read
/// with `env::var`. It never prints, logs or stores the value, adds no
/// environment variable of its own, and does not read the Keychain (that module
/// belongs to `desktop-shell`). A `keychain:` reference therefore cannot be
/// resolved from this crate: point `DAMAIAN_DATA_DIR` at a temporary directory
/// whose `user.conf` names an environment variable, or measure the same three
/// documents through the app instead.
///
/// Ignored because it calls a real model provider over the network; it spends
/// the user's money and only runs when explicitly selected. Run it manually,
/// with a **temporary** data directory so it cannot touch real state:
///
/// ```text
/// DAMAIAN_DATA_DIR=/tmp/damaian-calibration \
///   CARGO_TARGET_DIR="$PWD/target" \
///   cargo nextest run -p workspace-engine --test context_assembly \
///   --run-ignored ignored-only -E 'test(calibration_measures_bytes_per_token)'
/// ```
#[test]
#[ignore = "calls a real model provider over the network; run manually (see the doc comment)"]
fn calibration_measures_bytes_per_token() {
    let Some(data_dir) = std::env::var_os("DAMAIAN_DATA_DIR") else {
        println!(
            "calibration_measures_bytes_per_token needs a temporary data directory: \
             DAMAIAN_DATA_DIR=/tmp/damaian-calibration CARGO_TARGET_DIR=\"$PWD/target\" \
             cargo nextest run -p workspace-engine --test context_assembly \
             --run-ignored ignored-only -E 'test(calibration_measures_bytes_per_token)'"
        );
        return;
    };
    let data_dir = PathBuf::from(data_dir);
    let real = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Library")
        .join("Application Support")
        .join("DamaianClient");
    assert_ne!(
        data_dir, real,
        "point DAMAIAN_DATA_DIR at a temporary directory, never the real one"
    );

    let (config, _report) = Config::load_for_repository_reporting(None)
        .expect("the data directory's config should load");
    let api_key = std::env::var(&config.model_api_key_env).unwrap_or_else(|_| {
        panic!(
            "set the environment variable `{}`, which this config's model_api_key_env names",
            config.model_api_key_env
        )
    });
    let transport = CurlModelTransport::new(
        &config.model_base_url,
        api_key,
        ProcessRegistry::open(&config.data_dir).expect("the process registry should open"),
        &config.data_dir,
    );
    let mut adapter = OpenAICompatibleAdapter::with_provider(
        &config.model_provider,
        &config.model_name,
        transport,
    );

    println!(
        "provider {} model {}",
        config.model_provider, config.model_name
    );

    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_wait = |_seconds: u64| {};
    for (name, document) in [
        ("prose", CALIBRATION_PROSE),
        ("rust", CALIBRATION_RUST),
        ("json", CALIBRATION_JSON),
    ] {
        let request = ModelRequest {
            provider: config.model_provider.clone(),
            model: config.model_name.clone(),
            messages: vec![ModelMessage::user(format!(
                "Reply with the single word OK. Do not explain.\n\n{document}"
            ))],
            temperature: Some("0".to_string()),
            reasoning_level: Some(config.model_reasoning_level.clone()),
            stream: true,
            tools: None,
            // Room for a reasoning model's hidden output: deepseek-v4-flash
            // spends output tokens on reasoning before any content, so a small
            // ceiling fails with "no assistant content". Only input usage is
            // calibrated, so the reply may be as short as the provider allows.
            max_tokens: Some(1024),
            request_usage: true,
            emit_cache_breakpoints: false,
        };
        let run = adapter
            .stream_response(&request, &cancel, &mut on_token, &mut on_wait)
            .expect("the calibration call should succeed");
        assert_eq!(
            run.usage.source,
            UsageSource::Measured,
            "{name}: the provider did not report usage, so bytes per token cannot be calibrated"
        );
        let tokens = run.usage.input_tokens;
        let bytes = document.len() as u64;
        assert!(tokens > 0, "{name}: a measured call has input tokens");
        // 1 to 8 bytes per token is the band any real tokenizer falls in for
        // these documents. Outside it, the instruction or the reply dominated
        // the count and the ratio says nothing about the document.
        assert!(
            tokens <= bytes && tokens * 8 >= bytes,
            "{name}: {bytes} bytes measured {tokens} input tokens"
        );
        // Hundredths, so the print shows two decimals without a float. The
        // provider's count includes instruction overhead, so this ratio reads
        // slightly below the document-only one; the constants in
        // `TokenEstimate` are rounded down from it, which over-counts tokens.
        let bytes_per_token_hundredths = bytes * 100 / tokens;
        println!(
            "{name}: {bytes} bytes, {tokens} input tokens = {}.{:02} bytes/token; \
             len / 4 would have estimated {} input tokens",
            bytes_per_token_hundredths / 100,
            bytes_per_token_hundredths % 100,
            bytes.div_ceil(4)
        );
    }
}

/// ASCII, so a character is a byte in the `chars / tokens` figures. Fixed
/// content, not files: no repository and no index are involved.
const CALIBRATION_PROSE: &str = "\
Context assembly decides what a model sees before a turn begins. The pipeline \
collects candidates from explicit paths, prompt mentions, keyword and semantic \
search, the instruction hierarchy, project rules, the repository map, the \
symbol index, hook additions and a carried plan. Each candidate is redacted, \
measured and then charged to a category budget, so the sum of what is sent \
stays inside the configured limit. A category that runs out of its share drops \
its lowest-priority members first and records why it did so; an unused share \
moves forward to later categories in pipeline order. The manifest describes \
every included item by path, range and token estimate, and every exclusion by \
reason, without ever carrying the content itself. That distinction matters, \
because the manifest is written to the audit log where a reader can inspect \
what the turn actually saw. The estimate is deliberately still an estimate: \
assembly must choose before the call, while a provider reports the real figure \
only afterwards. What improves here is the ratio behind the estimate, chosen \
per kind of content rather than fixed for every byte.";

const CALIBRATION_RUST: &str = "\
use std::collections::HashMap;

/// Accumulates per-category token use while a plan is assembled.
pub struct BudgetLedger {
    remaining_by_category: HashMap<u32, usize>,
    dropped: Vec<(u32, String)>,
}

impl BudgetLedger {
    pub fn new(shares: &[u16; 10], available: usize) -> Self {
        let mut remaining_by_category = HashMap::with_capacity(shares.len());
        for (category, share) in shares.iter().enumerate() {
            let allocation = available * usize::from(*share) / 10_000;
            remaining_by_category.insert(category as u32, allocation);
        }
        Self { remaining_by_category, dropped: Vec::new() }
    }

    /// Charge one item to its category, or record it as excluded.
    pub fn charge(&mut self, category: u32, path: &str, tokens: usize) -> bool {
        let Some(remaining) = self.remaining_by_category.get_mut(&category) else {
            return false;
        };
        if tokens > *remaining {
            self.dropped.push((category, path.to_string()));
            return false;
        }
        *remaining -= tokens;
        true
    }

    pub fn carry_forward(&mut self, from: u32, to: u32) -> usize {
        let unused = self.remaining_by_category.remove(&from).unwrap_or(0);
        *self.remaining_by_category.entry(to).or_insert(0) += unused;
        unused
    }
}";

const CALIBRATION_JSON: &str = "\
{
  \"schemaVersion\": 3,
  \"repository\": { \"id\": \"repo_01hzy\", \"root\": \"packages/api\" },
  \"categories\": [
    { \"name\": \"required\", \"shareBps\": 0, \"used\": 41, \"truncated\": false },
    { \"name\": \"instructions\", \"shareBps\": 1000, \"used\": 214, \"truncated\": false },
    { \"name\": \"repository_map\", \"shareBps\": 500, \"used\": 388, \"truncated\": true },
    { \"name\": \"symbols\", \"shareBps\": 1000, \"used\": 640, \"truncated\": false },
    { \"name\": \"search_results\", \"shareBps\": 1500, \"used\": 1173, \"truncated\": false },
    { \"name\": \"file_ranges\", \"shareBps\": 2500, \"used\": 2044, \"truncated\": false }
  ],
  \"included\": [
    { \"path\": \"src/lib.rs\", \"lineRange\": { \"start\": 12, \"end\": 40 }, \"tokens\": 188 },
    { \"path\": \"src/budget.rs\", \"lineRange\": null, \"tokens\": 731 }
  ],
  \"excluded\": [
    { \"reason\": \"category_budget_exhausted\", \"path\": \"src/legacy.rs\", \"detail\": null },
    { \"reason\": \"restricted_path\", \"path\": \"secrets/token.txt\", \"detail\": null }
  ],
  \"limit\": { \"tokens\": 64000, \"source\": \"declared\", \"safetyMarginBps\": 0 }
}";
