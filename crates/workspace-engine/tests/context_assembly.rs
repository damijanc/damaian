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

use workspace_engine::context_manager::ExclusionReason;
use workspace_engine::{
    CancelToken, ChatTurnResult, CollectionSource, Config, ConfigOverlay, ContextCategory,
    ContextItem, ContextKind, ContextPlan, ContextRequest, ExclusionRecord, LineRange,
    MockModelAdapter, ModelAdapter, PlanStep, Provenance, RepositoryIndex, StepStatus, Symbol,
    SymbolIndexStore, TaskPlan, TurnProgress, TurnSink, WorkspaceEngine,
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

// ---- Task 2: collection sources and exclusion at collection time ----

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
    engine.context_manager.build_context(ContextRequest {
        repository_root: repo,
        repository_id,
        task_id: "task-2",
        prompt,
        index,
        explicit_paths,
        limit: 16_000,
        hook_context: &[],
        plan,
    })
}

/// Every restriction record for `path`, so a test can state which category it
/// came from instead of accepting any record.
fn restrictions_for<'a>(plan: &'a ContextPlan, path: &str) -> Vec<&'a ExclusionRecord> {
    plan.excluded
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
    assert_eq!(records.len(), 1, "{:?}", plan.excluded);
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
    assert_eq!(records.len(), 1, "{:?}", plan.excluded);
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
    assert_eq!(records.len(), 1, "{:?}", plan.excluded);
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
    assert_eq!(records.len(), 1, "{:?}", plan.excluded);
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
    assert_eq!(records.len(), 1, "{:?}", plan.excluded);
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
    assert_eq!(records.len(), 1, "{:?}", plan.excluded);
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
            plan.excluded
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
            | CollectionSource::Symbols => Some(ExclusionReason::RestrictedPath),
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
