use super::*;
use crate::config::ModelProviderConfig;
use crate::workspace_engine::WorkspaceEngine;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-memory-proposal-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), "# Memory proposal test\n").unwrap();
    dir
}

fn engine_with(repo: &Path) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    };
    config.model_providers.push(ModelProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: Vec::new(),
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
        context_window: None,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    });
    WorkspaceEngine::new(config)
}

#[test]
fn an_explicit_remember_request_creates_a_proposal_not_an_entry() {
    let repo = temp_repo("explicit");
    let engine = engine_with(&repo);
    let sessions = SessionStore::new(repo.join(".damaian"));
    let session = sessions
        .create_session("repo_fixture", "explicit memory")
        .unwrap();
    let prompt = "Please remember that integration tests need Docker running.";
    let message = sessions
        .append_message(&session.id, None, "user", prompt)
        .unwrap();
    engine
        .chat_orchestrator
        .propose_explicit_memory(&repo, &session.id, None, &message.id, prompt)
        .unwrap();

    let pending = engine.memory_store.pending_proposals(&session.id).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].statement,
        "integration tests need Docker running"
    );
    assert!(matches!(
        pending[0].origin,
        ProposalOrigin::UserRequest { .. }
    ));
    assert!(
        !repo
            .join(".damaian")
            .join("memory")
            .join("project")
            .exists(),
        "a proposal must not write a memory file"
    );
}

#[test]
fn a_prompt_without_a_remember_cue_creates_no_proposal() {
    let repo = temp_repo("no-cue");
    let engine = engine_with(&repo);
    let sessions = SessionStore::new(repo.join(".damaian"));
    let session = sessions.create_session("repo_fixture", "no cue").unwrap();
    let message = sessions
        .append_message(&session.id, None, "user", "How does indexing work?")
        .unwrap();
    engine
        .chat_orchestrator
        .propose_explicit_memory(
            &repo,
            &session.id,
            None,
            &message.id,
            "How does indexing work?",
        )
        .unwrap();
    assert!(
        engine
            .memory_store
            .pending_proposals(&session.id)
            .unwrap()
            .is_empty()
    );
    assert!(
        refusal_messages(&engine, &session.id).is_empty(),
        "a message with no request is not refused; there was nothing to refuse"
    );
}

fn refusal_messages(engine: &WorkspaceEngine, session_id: &str) -> Vec<String> {
    engine
        .session_store
        .read_messages(session_id)
        .unwrap()
        .into_iter()
        .filter(|message| message.kind.as_deref() == Some("memory_refusal"))
        .map(|message| message.content)
        .collect()
}

fn audit_text(repo: &Path) -> String {
    fs::read_to_string(repo.join(".damaian").join("audit").join("events.jsonl")).unwrap_or_default()
}

#[test]
fn a_refused_explicit_request_says_why_instead_of_looking_ignored() {
    let repo = temp_repo("refused-secret");
    let engine = engine_with(&repo);
    let sessions = SessionStore::new(repo.join(".damaian"));
    let session = sessions.create_session("repo_fixture", "refused").unwrap();
    let prompt = "Remember that the deployment key is AKIAIOSFODNN7EXAMPLE.";
    let message = sessions
        .append_message(&session.id, None, "user", prompt)
        .unwrap();
    engine
        .chat_orchestrator
        .propose_explicit_memory(&repo, &session.id, None, &message.id, prompt)
        .unwrap();

    assert!(
        engine
            .memory_store
            .pending_proposals(&session.id)
            .unwrap()
            .is_empty(),
        "a secret-bearing request produces no card"
    );
    let messages = refusal_messages(&engine, &session.id);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("credential") && messages[0].contains("will not store"),
        "the plain refusal says why: {}",
        messages[0]
    );
    let audit = audit_text(&repo);
    assert!(audit.contains("memory_proposal_refused"), "{audit}");
    assert!(audit.contains("\"reason\":\"secret\""), "{audit}");
}

#[test]
fn an_over_long_explicit_request_says_why_and_is_audited() {
    let repo = temp_repo("refused-long");
    let engine = engine_with(&repo);
    let sessions = SessionStore::new(repo.join(".damaian"));
    let session = sessions.create_session("repo_fixture", "long").unwrap();
    let prompt = format!("Remember that {}", "a".repeat(300));
    let message = sessions
        .append_message(&session.id, None, "user", &prompt)
        .unwrap();
    engine
        .chat_orchestrator
        .propose_explicit_memory(&repo, &session.id, None, &message.id, &prompt)
        .unwrap();

    let messages = refusal_messages(&engine, &session.id);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("280") && messages[0].contains("not truncated"),
        "the plain refusal names the ceiling: {}",
        messages[0]
    );
    let audit = audit_text(&repo);
    assert!(
        audit.contains("memory_proposal_failed") && audit.contains("statement_too_long"),
        "the parser refusal is audited even though no proposal was created: {audit}"
    );
}
