//! Provider refusal outcomes and accounting, per
//! `docs/specs/48_provider_limits_and_backpressure/`.
//!
//! The rules these tests hold: a refused call is a *named* failure with a
//! measured-zero cost, and the estimate written before the call does not
//! survive it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CancelToken, ClientError, Config, ModelAdapter, ModelProviderConfig, ModelRequest, ModelRun,
    ProviderRefusal, SessionStore, TaskStatus, TurnProgress, TurnSink, UsageSource,
    WorkspaceEngine, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-refusal-{name}-{now}-{}",
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
        // Throwaway repository: a watcher would only cost FSEvents registration.
        enable_index_watcher: false,
        ..Config::default()
    }
}

/// A model adapter that always refuses, so a turn exercises the refusal arm
/// without a real provider or network. The refusal and its message are fixed at
/// construction, so a test can pick the classification it is asserting about.
struct RefusingAdapter {
    refusal: ProviderRefusal,
    message: String,
}

impl RefusingAdapter {
    fn rate_limited() -> Self {
        Self {
            refusal: ProviderRefusal::RateLimited {
                retry_after_secs: None,
            },
            message: "rate limited".to_string(),
        }
    }

    fn quota_exhausted() -> Self {
        Self {
            refusal: ProviderRefusal::QuotaExhausted,
            message: "You have run out of credit. Please top up your account.".to_string(),
        }
    }
}

impl ModelAdapter for RefusingAdapter {
    fn stream_response(
        &mut self,
        _request: &ModelRequest,
        _cancel: &CancelToken,
        _on_token: &mut dyn FnMut(&str),
        _on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        Err(ClientError::Provider(
            self.refusal.clone(),
            self.message.clone(),
        ))
    }
}

/// Streams a token and *then* refuses, so a test can pin the mid-stream case:
/// the provider billed for something, so the refusal must be estimated rather
/// than a measured zero (spec 48 §5.5).
struct RefusingMidStreamAdapter;

impl ModelAdapter for RefusingMidStreamAdapter {
    fn stream_response(
        &mut self,
        _request: &ModelRequest,
        _cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        _on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        on_token("partial answer");
        Err(ClientError::Provider(
            ProviderRefusal::RateLimited {
                retry_after_secs: None,
            },
            "rate limited mid-stream".to_string(),
        ))
    }
}

/// A configured provider, just enough for a config to hold a fallback
/// candidate that the feature-under-test must not silently use.
fn provider(id: &str) -> ModelProviderConfig {
    ModelProviderConfig {
        id: id.to_string(),
        label: id.to_string(),
        base_url: format!("https://{id}.example.test"),
        api_key_env: format!("{}_KEY", id.to_uppercase()),
        models: Vec::new(),
        supports_native_tools: false,
        max_output_tokens: None,
        context_token_budget: None,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
    }
}

fn drive_refused_turn(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ClientError {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .unwrap_err()
}

#[test]
fn a_task_failed_by_a_refusal_carries_the_kind_and_a_measured_zero() {
    let repo = temp_dir("refusal-outcome");
    write_fixture(&repo, "README.md", "# Refusal\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = RefusingAdapter::rate_limited();

    let error = drive_refused_turn(&engine, &repo, "Explain this", &mut adapter);
    assert_eq!(error.code(), "provider_rate_limited");

    let repository_id = repository_id_for_root(&repo);
    let sessions = engine
        .session_store
        .list_sessions(Some(&repository_id))
        .unwrap();
    let session = sessions
        .first()
        .expect("a failed turn still creates a session");

    // The task failed, and the failure is named rather than only described.
    let statuses = engine
        .session_store
        .read_task_statuses(&session.id)
        .unwrap();
    assert!(
        statuses.values().any(|status| status == "failed"),
        "statuses were {statuses:?}"
    );
    let kinds = engine
        .session_store
        .read_task_failure_kinds(&session.id)
        .unwrap();
    assert!(
        kinds.values().any(|kind| kind == "provider_rate_limited"),
        "failure kinds were {kinds:?}"
    );

    // The refusal before any token bills nothing: the pre-call estimate is
    // overridden by a measured zero, so the task's total is unchanged by it.
    let usage = engine.session_store.read_task_usage(&session.id).unwrap();
    let total = usage
        .values()
        .next()
        .expect("the refused call is still accounted");
    assert_eq!(total.input_tokens, 0);
    assert_eq!(total.output_tokens, 0);
    assert_eq!(total.source, UsageSource::Measured);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_failure_kind_round_trips_through_the_store() {
    let repo = temp_dir("failure-kind-round-trip");
    let store = SessionStore::new(repo.join(".damaian"));
    let session = store.create_session("repo_1", "Refused").unwrap();
    let task = store
        .create_task(&session.id, "do the thing", "mock", "m")
        .unwrap();

    let failed = store
        .update_task_status_with_kind(
            &task,
            TaskStatus::Failed,
            Some("rate limited"),
            Some("provider_rate_limited"),
        )
        .unwrap();

    assert_eq!(failed.status, TaskStatus::Failed);
    assert_eq!(
        store
            .read_task_failure_kinds(&session.id)
            .unwrap()
            .get(&task.id),
        Some(&"provider_rate_limited".to_string())
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_failure_kind_is_absent_for_a_failure_that_was_not_named() {
    // The field is absent, not empty, so a session written before this spec —
    // or a task that failed for a reason no one named — is unchanged.
    let repo = temp_dir("unnamed-failure");
    let store = SessionStore::new(repo.join(".damaian"));
    let session = store.create_session("repo_1", "Failed").unwrap();
    let task = store
        .create_task(&session.id, "do the thing", "mock", "m")
        .unwrap();

    store
        .update_task_status(&task, TaskStatus::Failed, Some("something broke"))
        .unwrap();

    assert!(
        !store
            .read_task_failure_kinds(&session.id)
            .unwrap()
            .contains_key(&task.id)
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_quota_exhaustion_fails_permanently_with_the_providers_message() {
    let repo = temp_dir("quota-exhaustion");
    write_fixture(&repo, "README.md", "# Quota\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = RefusingAdapter::quota_exhausted();

    let error = drive_refused_turn(&engine, &repo, "Explain this", &mut adapter);
    assert_eq!(error.code(), "provider_quota_exhausted");
    // The provider's message is the only place the user learns what to top up,
    // so it is carried verbatim rather than paraphrased.
    assert!(error.to_string().contains("run out of credit"));

    let repository_id = repository_id_for_root(&repo);
    let sessions = engine
        .session_store
        .list_sessions(Some(&repository_id))
        .unwrap();
    let session = sessions
        .first()
        .expect("a failed turn still creates a session");
    let kinds = engine
        .session_store
        .read_task_failure_kinds(&session.id)
        .unwrap();
    assert!(
        kinds
            .values()
            .any(|kind| kind == "provider_quota_exhausted"),
        "failure kinds were {kinds:?}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn no_fallback_happens_without_approval() {
    // Even with a second provider configured, a refusal fails the task rather
    // than silently switching to the fallback: switching requires the user's
    // explicit consent, and none was given here.
    let repo = temp_dir("no-fallback");
    write_fixture(&repo, "README.md", "# Fallback\n");
    let mut config = test_config(&repo);
    config.model_provider = "primary".to_string();
    config.model_name = "model-a".to_string();
    config.model_providers = vec![provider("primary"), provider("fallback")];
    let engine = WorkspaceEngine::new(config);
    let mut adapter = RefusingAdapter::rate_limited();

    let error = drive_refused_turn(&engine, &repo, "Explain this", &mut adapter);
    assert_eq!(error.code(), "provider_rate_limited");

    // The task failed, not switched: no waiting-for-approval pause, no answer
    // from a second provider, just the refusal.
    let repository_id = repository_id_for_root(&repo);
    let sessions = engine
        .session_store
        .list_sessions(Some(&repository_id))
        .unwrap();
    let session = sessions
        .first()
        .expect("a failed turn still creates a session");
    let statuses = engine
        .session_store
        .read_task_statuses(&session.id)
        .unwrap();
    assert!(
        statuses.values().any(|status| status == "failed"),
        "a refusal with a fallback configured must still fail, statuses were {statuses:?}"
    );
    assert!(
        !statuses
            .values()
            .any(|status| status == "waiting_for_approval"),
        "no approval was offered, so none may be pending"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_refusal_is_audited_with_its_classification_and_not_the_request() {
    let repo = temp_dir("refusal-audit");
    write_fixture(&repo, "README.md", "# Audit\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = RefusingAdapter::rate_limited();

    let _ = drive_refused_turn(&engine, &repo, "Explain this", &mut adapter);

    let audit = fs::read_to_string(repo.join(".damaian").join("audit").join("events.jsonl"))
        .expect("the refusal is audited");
    let refusal_line = audit
        .lines()
        .find(|line| line.contains("provider_refusal"))
        .expect("a provider_refusal event is recorded");
    assert!(
        refusal_line.contains("provider_rate_limited"),
        "the audit names the classification: {refusal_line}"
    );
    // The request body (the user's prompt) and any key never reach the audit.
    assert!(
        !refusal_line.contains("Explain this"),
        "the audit must not carry the request body: {refusal_line}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_refusal_mid_stream_records_estimated_usage() {
    let repo = temp_dir("mid-stream-refusal");
    write_fixture(&repo, "README.md", "# Mid\n");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let mut adapter = RefusingMidStreamAdapter;

    let _ = drive_refused_turn(&engine, &repo, "Explain this", &mut adapter);

    let repository_id = repository_id_for_root(&repo);
    let sessions = engine
        .session_store
        .list_sessions(Some(&repository_id))
        .unwrap();
    let session = sessions
        .first()
        .expect("a failed turn still creates a session");
    let usage = engine.session_store.read_task_usage(&session.id).unwrap();
    let total = usage
        .values()
        .next()
        .expect("the refused call is still accounted");
    // The provider streamed before refusing, so this is estimated non-zero —
    // reporting it as measured zero would understate what was billed.
    assert_eq!(total.source, UsageSource::Estimated);
    assert!(total.output_tokens > 0, "the streamed tokens are counted");

    fs::remove_dir_all(repo).unwrap();
}
