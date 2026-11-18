use super::{TurnEvent, chat_result_json, resume_fallback_decision, switch_model_provider};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CancelToken, ChatTurnResult, ClientError, Config, MockModelAdapter, ModelAdapter,
    ModelProviderConfig, ModelRequest, ModelRun, ProviderRefusal, TaskStatus, TurnProgress,
    TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "damaian-shell-fallback-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), "# Fallback\n").unwrap();
    dir
}

fn provider(id: &str, model: &str) -> ModelProviderConfig {
    ModelProviderConfig {
        id: id.to_string(),
        label: id.to_string(),
        base_url: format!("https://{id}.example.test"),
        api_key_env: format!("{}_KEY", id.to_uppercase()),
        models: vec![model.to_string()],
        supports_native_tools: false,
        max_output_tokens: None,
        context_token_budget: None,
        context_window: None,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    }
}

/// `primary` active and `fallback` beside it, each with its own key, in the
/// repository's own data directory. Built directly rather than loaded, so
/// the test touches neither the user's config nor the binary's shared one.
fn two_provider_config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        model_provider: "primary".to_string(),
        model_name: "model-a".to_string(),
        model_api_key_env: "PRIMARY_KEY".to_string(),
        model_providers: vec![
            provider("primary", "model-a"),
            provider("fallback", "model-b"),
        ],
        ..Config::default()
    }
}

struct RateLimited;

impl ModelAdapter for RateLimited {
    fn stream_response(
        &mut self,
        _request: &ModelRequest,
        _cancel: &CancelToken,
        _on_token: &mut dyn FnMut(&str),
        _on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        Err(ClientError::Provider(
            ProviderRefusal::RateLimited {
                retry_after_secs: None,
            },
            "rate limited".to_string(),
        ))
    }
}

/// A refused turn, paused on an offer to answer with `fallback`.
fn paused_offer(repo: &Path) -> ChatTurnResult {
    let engine = WorkspaceEngine::new(two_provider_config(repo));
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(
            repo,
            "explain the readme",
            &[],
            None,
            &mut RateLimited,
            &mut sink,
        )
        .expect("a refusal with a candidate pauses rather than failing");
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    result
}

fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Runs the route's decision with engines built the way the shell builds
/// them, and returns the result plus every provider an engine was built for
/// and every provider an adapter (and so a key) was asked for.
fn decide(
    repo: &Path,
    form: &HashMap<String, String>,
    answer: &str,
) -> (
    Result<ChatTurnResult, String>,
    Vec<Option<String>>,
    Vec<String>,
) {
    let engines = RefCell::new(Vec::new());
    let mut adapters = Vec::new();
    let (events, _receiver) = std::sync::mpsc::channel::<TurnEvent>();
    let result = resume_fallback_decision(
        form,
        &CancelToken::new(),
        &events,
        &|provider| {
            engines.borrow_mut().push(provider.map(str::to_string));
            let mut config = two_provider_config(repo);
            if let Some(provider) = provider {
                switch_model_provider(&mut config, provider)?;
            }
            Ok(WorkspaceEngine::new(config))
        },
        &mut |config| {
            adapters.push(format!("{}/{}", config.model_provider, config.model_name));
            Ok(Box::new(MockModelAdapter::new(answer)))
        },
    );
    (result, engines.into_inner(), adapters)
}

fn task_status(repo: &Path, result: &ChatTurnResult) -> String {
    let engine = WorkspaceEngine::new(two_provider_config(repo));
    engine
        .session_store
        .read_task_statuses(&result.session.id)
        .unwrap()
        .remove(&result.task.id)
        .expect("the task has a status")
}

#[test]
fn the_turn_json_names_both_providers_and_offers_no_always() {
    let repo = temp_repo();
    let result = paused_offer(&repo);

    let json: serde_json::Value = serde_json::from_str(&chat_result_json(&result)).unwrap();
    let offer = &json["fallbackProposal"];
    assert_eq!(offer["refusedProvider"], "primary");
    assert_eq!(offer["refusedModel"], "model-a");
    assert_eq!(offer["fallbackProvider"], "fallback");
    assert_eq!(offer["fallbackModel"], "model-b");
    assert_eq!(offer["refusalCode"], "provider_rate_limited");
    assert_eq!(offer["message"], "rate limited");
    assert!(
        offer["proposalId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    let fields: Vec<&String> = offer.as_object().unwrap().keys().collect();
    assert!(
        fields
            .iter()
            .all(|field| !field.to_lowercase().contains("always")),
        "requirement 3: there is no always, {fields:?}"
    );
    assert_eq!(json["taskStatus"], "waiting_for_approval");
}

#[test]
fn an_ordinary_turn_carries_a_null_fallback_proposal() {
    let repo = temp_repo();
    let engine = WorkspaceEngine::new(two_provider_config(&repo));
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(
            &repo,
            "hello",
            &[],
            None,
            &mut MockModelAdapter::new("hi"),
            &mut sink,
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&chat_result_json(&result)).unwrap();
    assert!(json["fallbackProposal"].is_null());
}

#[test]
fn a_declined_offer_fails_the_task_and_resolves_no_key() {
    let repo = temp_repo();
    let offer = paused_offer(&repo);
    let proposal_id = offer.fallback_proposal.as_ref().unwrap().id.clone();

    // `always` is ignored, not honoured: it cannot turn a decline into
    // anything, and on an approval it must not make the switch stick.
    let (result, engines, adapters) = decide(
        &repo,
        &form(&[
            ("repo", "unused"),
            ("proposal_id", &proposal_id),
            ("approved", "false"),
            ("always", "true"),
        ]),
        "must not be used",
    );

    let error = result.expect_err("a decline fails the turn as spec 48 does");
    assert!(error.contains("rate limited"), "{error}");
    assert!(
        adapters.is_empty(),
        "a decline builds no transport: {adapters:?}"
    );
    assert_eq!(
        engines,
        vec![None],
        "a decline stays on the configured provider"
    );
    assert_eq!(task_status(&repo, &offer), "failed");
}

#[test]
fn an_unknown_offer_errors_before_any_key_is_resolved() {
    let repo = temp_repo();
    let (result, _engines, adapters) = decide(
        &repo,
        &form(&[
            ("repo", "unused"),
            ("proposal_id", "chatprop_missing"),
            ("approved", "true"),
        ]),
        "must not be used",
    );

    let error = result.expect_err("there is no such offer");
    assert!(
        error.contains("No pending provider fallback offer"),
        "{error}"
    );
    assert!(adapters.is_empty(), "no key for an offer that is not there");
}

#[test]
fn an_approval_builds_the_fallbacks_engine_and_adapter() {
    let repo = temp_repo();
    let offer = paused_offer(&repo);
    let proposal_id = offer.fallback_proposal.as_ref().unwrap().id.clone();

    let (result, engines, adapters) = decide(
        &repo,
        &form(&[
            ("repo", "unused"),
            ("proposal_id", &proposal_id),
            ("approved", "true"),
        ]),
        "answered by the fallback",
    );

    let result = result.expect("the fallback answers");
    assert_eq!(result.response, "answered by the fallback");
    assert_eq!(engines, vec![None, Some("fallback".to_string())]);
    assert_eq!(
        adapters,
        vec!["fallback/model-b".to_string()],
        "C1: the key and the request belong to the fallback"
    );
    assert_eq!(task_status(&repo, &offer), "complete");
}
