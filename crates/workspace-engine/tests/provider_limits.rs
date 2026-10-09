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
use workspace_engine::plan::{PlanStep, StepStatus, TaskPlan};
use workspace_engine::{
    AgentFallbackProposal, AuditLog, CancelToken, ChatTurnResult, ClientError, CommandStore,
    Config, MockModelAdapter, MockModelTransport, ModelAdapter, ModelProviderConfig, ModelRequest,
    ModelRun, OpenAICompatibleAdapter, PatchStore, PausedTurns, ProviderRefusal,
    ReattachedApproval, SecretScanner, SessionStore, TaskStatus, TurnProgress, TurnSink,
    UsageSource, WorkspaceEngine, reattach_pending_approvals, repository_id_for_root,
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
    /// How many times the turn called this adapter: a switch that happened
    /// without consent would show up as a second call somewhere, and a pause
    /// as exactly one here.
    calls: usize,
    /// What each call was sent, so a test can compare the refused request
    /// with the one a fallback is later given.
    requests: Vec<ModelRequest>,
}

impl RefusingAdapter {
    fn rate_limited() -> Self {
        Self {
            refusal: ProviderRefusal::RateLimited {
                retry_after_secs: None,
            },
            message: "rate limited".to_string(),
            calls: 0,
            requests: Vec::new(),
        }
    }

    fn quota_exhausted() -> Self {
        Self {
            refusal: ProviderRefusal::QuotaExhausted,
            message: "You have run out of credit. Please top up your account.".to_string(),
            calls: 0,
            requests: Vec::new(),
        }
    }
}

impl ModelAdapter for RefusingAdapter {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        _cancel: &CancelToken,
        _on_token: &mut dyn FnMut(&str),
        _on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        self.calls += 1;
        self.requests.push(request.clone());
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
        supports_explicit_cache_breakpoints: false,
    }
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
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
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
}

fn drive_refused_turn(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ClientError {
    ask(engine, repo, prompt, adapter).unwrap_err()
}

fn decide_fallback(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    approved: bool,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    // An adapter that would answer, so a test can tell "not called" from
    // "called and refused".
    let mut adapter = MockModelAdapter::new("answered");
    let result = engine.chat_orchestrator.resume_after_fallback_decision(
        proposal_id,
        approved,
        "test_user",
        &mut adapter,
        &mut sink,
    );
    assert!(
        adapter.requests.is_empty(),
        "a decline, or a refused decision, must never call a model"
    );
    result
}

fn approve_with(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine.chat_orchestrator.resume_after_fallback_decision(
        proposal_id,
        true,
        "test_user",
        adapter,
        &mut sink,
    )
}

/// The engine the shell builds to take an approved offer: the same repository
/// and providers, with `fallback` active in memory (`tasks.md` C1). Nothing is
/// written back — `config_with` still says `primary`.
fn fallback_engine(repo: &Path, others: Vec<ModelProviderConfig>) -> WorkspaceEngine {
    let mut config = config_with(repo, others);
    config.model_provider = "fallback".to_string();
    config.model_name = "model-b".to_string();
    config.model_api_key_env = "FALLBACK_KEY".to_string();
    WorkspaceEngine::new(config)
}

/// The `toolRound` of every `model_request_prepared` audit row, in order.
fn prepared_rounds(repo: &Path) -> Vec<String> {
    audit(repo)
        .lines()
        .filter(|line| line.contains("model_request_prepared"))
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(value.get("toolRound")?.as_str()?.to_string())
        })
        .collect()
}

/// `primary` active, with `others` configured beside it. The active
/// provider's key variable is `PRIMARY_KEY`, as `provider("primary")` names it.
fn config_with(repo: &Path, others: Vec<ModelProviderConfig>) -> Config {
    let mut config = test_config(repo);
    config.model_provider = "primary".to_string();
    config.model_name = "model-a".to_string();
    config.model_api_key_env = "PRIMARY_KEY".to_string();
    config.model_providers = std::iter::once(provider("primary")).chain(others).collect();
    config
}

fn only_session(engine: &WorkspaceEngine, repo: &Path) -> workspace_engine::Session {
    engine
        .session_store
        .list_sessions(Some(&repository_id_for_root(repo)))
        .unwrap()
        .into_iter()
        .next()
        .expect("a turn creates a session")
}

fn statuses(engine: &WorkspaceEngine, repo: &Path) -> Vec<String> {
    let session = only_session(engine, repo);
    engine
        .session_store
        .read_task_statuses(&session.id)
        .unwrap()
        .into_values()
        .collect()
}

fn audit(repo: &Path) -> String {
    fs::read_to_string(repo.join(".damaian").join("audit").join("events.jsonl")).unwrap_or_default()
}

fn pending_turn_exists(repo: &Path, proposal_id: &str) -> bool {
    repo.join(".damaian")
        .join("chat")
        .join("pending")
        .join(format!("{proposal_id}.json"))
        .exists()
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
    // A second provider is configured, so the refusal is *offered* a switch —
    // and nothing more. Until the user decides, the turn is paused on the
    // offer, and the only model ever asked is the one that refused. Spec 48
    // pinned this as "the task fails"; spec 56 turns the failure into a
    // declined-by-default choice without weakening the guarantee.
    let repo = temp_dir("no-fallback");
    write_fixture(&repo, "README.md", "# Fallback\n");
    let engine = WorkspaceEngine::new(config_with(&repo, vec![provider("fallback")]));
    let mut adapter = RefusingAdapter::rate_limited();

    let result = ask(&engine, &repo, "Explain this", &mut adapter)
        .expect("a refusal with a candidate pauses rather than failing");

    assert_eq!(adapter.calls, 1, "only the refusing provider was asked");
    let offer = result
        .fallback_proposal
        .expect("the pause is a fallback offer");
    assert_eq!(offer.refused_provider, "primary");
    assert_eq!(offer.refused_model, "model-a");
    assert_eq!(offer.fallback_provider, "fallback");
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let statuses = statuses(&engine, &repo);
    assert!(
        statuses
            .iter()
            .any(|status| status == "waiting_for_approval"),
        "statuses were {statuses:?}"
    );
    assert!(
        !statuses.iter().any(|status| status == "complete"),
        "nothing answered, so nothing completed"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_refusal_with_no_candidate_still_fails_closed() {
    // Spec 48's behaviour, unchanged: the only configured provider refused,
    // so there is nothing to offer and the task fails with the refusal's kind.
    let repo = temp_dir("no-candidate");
    write_fixture(&repo, "README.md", "# Closed\n");
    let engine = WorkspaceEngine::new(config_with(&repo, Vec::new()));
    let mut adapter = RefusingAdapter::rate_limited();

    let error = drive_refused_turn(&engine, &repo, "Explain this", &mut adapter);

    assert_eq!(error.code(), "provider_rate_limited");
    let statuses = statuses(&engine, &repo);
    assert!(statuses.iter().any(|status| status == "failed"));
    assert!(
        !statuses
            .iter()
            .any(|status| status == "waiting_for_approval")
    );
    assert!(!audit(&repo).contains("provider_fallback_offered"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_terminal_refusal_with_a_candidate_pauses_naming_both_providers() {
    let repo = temp_dir("offer-bookkeeping");
    write_fixture(&repo, "README.md", "# Offer\n");
    let engine = WorkspaceEngine::new(config_with(&repo, vec![provider("fallback")]));
    let mut adapter = RefusingAdapter::rate_limited();

    let result = ask(&engine, &repo, "Explain this", &mut adapter).unwrap();
    let offer = result.fallback_proposal.expect("an offer is made");
    assert_eq!(offer.refusal_code, "provider_rate_limited");
    assert_eq!(offer.message, "rate limited");
    assert!(
        result.response.contains("primary") && result.response.contains("fallback"),
        "the offer's text names both providers: {}",
        result.response
    );

    // The task records which offer it waits on, so recovery can find it.
    let session = only_session(&engine, &repo);
    let pending = engine
        .session_store
        .pending_approval_for(&session.id, &result.task.id)
        .unwrap()
        .expect("the waiting task names its pending approval");
    assert_eq!(pending.kind, "provider_fallback");
    assert_eq!(pending.proposal_id, offer.id);
    assert!(pending_turn_exists(&repo, &offer.id));
    assert_eq!(
        PausedTurns::new(repo.join(".damaian")).provider_fallback_target(&offer.id),
        Some("fallback".to_string())
    );

    // Spec 48's bookkeeping happened anyway: a measured zero, not the
    // pre-call estimate, and the refusal is audited before the offer.
    let usage = engine.session_store.read_task_usage(&session.id).unwrap();
    let total = usage
        .get(&result.task.id)
        .expect("the refused call is booked");
    assert_eq!((total.input_tokens, total.output_tokens), (0, 0));
    assert_eq!(total.source, UsageSource::Measured);
    let audit = audit(&repo);
    let refusal_at = audit.find("provider_refusal").expect("refusal audited");
    let offer_at = audit
        .find("provider_fallback_offered")
        .expect("offer audited");
    assert!(refusal_at < offer_at, "the refusal is recorded first");
    assert!(
        !audit.contains("Explain this"),
        "no request body in the audit"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_transient_refusal_that_later_succeeds_never_offers() {
    // Requirement 2: a refusal still being retried never reaches the offer.
    // The real adapter retries the 429 inside one call (spec 48 §5.3), so the
    // turn only ever sees the answer.
    let repo = temp_dir("retried-no-offer");
    write_fixture(&repo, "README.md", "# Retry\n");
    let engine = WorkspaceEngine::new(config_with(&repo, vec![provider("fallback")]));
    let transport = MockModelTransport::sequence_with_status(vec![
        (
            "{\"error\":{\"message\":\"rate limited\"}}".to_string(),
            Some(429),
            Some(0),
        ),
        (
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n".to_string(),
            Some(200),
            None,
        ),
    ]);
    let mut adapter = OpenAICompatibleAdapter::new("model-a", transport);

    let result = ask(&engine, &repo, "Explain this", &mut adapter).unwrap();

    assert!(result.fallback_proposal.is_none());
    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(!audit(&repo).contains("provider_fallback_offered"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_quota_exhausted_offer_names_only_a_provider_with_its_own_credentials() {
    // `same-account` is listed first and would win on order alone, but it
    // reads the exhausted provider's own key variable: the same empty account.
    let repo = temp_dir("quota-offer");
    write_fixture(&repo, "README.md", "# Quota\n");
    let mut same_account = provider("same-account");
    same_account.api_key_env = "PRIMARY_KEY".to_string();
    let engine = WorkspaceEngine::new(config_with(
        &repo,
        vec![same_account, provider("own-account")],
    ));
    let mut adapter = RefusingAdapter::quota_exhausted();

    let result = ask(&engine, &repo, "Explain this", &mut adapter).unwrap();

    let offer = result
        .fallback_proposal
        .expect("a provider with its own key exists");
    assert_eq!(offer.fallback_provider, "own-account");
    assert_eq!(offer.refusal_code, "provider_quota_exhausted");

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_quota_exhaustion_with_only_a_shared_key_fails_closed() {
    let repo = temp_dir("quota-shared-only");
    write_fixture(&repo, "README.md", "# Quota\n");
    let mut same_account = provider("same-account");
    same_account.api_key_env = "PRIMARY_KEY".to_string();
    let engine = WorkspaceEngine::new(config_with(&repo, vec![same_account]));
    let mut adapter = RefusingAdapter::quota_exhausted();

    let error = drive_refused_turn(&engine, &repo, "Explain this", &mut adapter);

    assert_eq!(error.code(), "provider_quota_exhausted");
    assert!(
        statuses(&engine, &repo)
            .iter()
            .any(|status| status == "failed")
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_declined_offer_fails_with_the_refusals_failure_kind() {
    let repo = temp_dir("declined-offer");
    write_fixture(&repo, "README.md", "# Decline\n");
    let engine = WorkspaceEngine::new(config_with(&repo, vec![provider("fallback")]));
    let mut adapter = RefusingAdapter::quota_exhausted();
    let offer = ask(&engine, &repo, "Explain this", &mut adapter)
        .unwrap()
        .fallback_proposal
        .unwrap();

    let error = decide_fallback(&engine, &offer.id, false).unwrap_err();

    // Exactly spec 48's outcome: the refusal's own error, `failureKind`, and
    // the provider's message, with no second booking of the refused call.
    assert_eq!(error.code(), "provider_quota_exhausted");
    let session = only_session(&engine, &repo);
    let statuses = statuses(&engine, &repo);
    assert!(statuses.iter().any(|status| status == "failed"));
    assert!(
        !statuses
            .iter()
            .any(|status| status == "waiting_for_approval")
    );
    let kinds = engine
        .session_store
        .read_task_failure_kinds(&session.id)
        .unwrap();
    assert_eq!(
        kinds.values().next().map(String::as_str),
        Some("provider_quota_exhausted")
    );
    let usage = engine.session_store.read_task_usage(&session.id).unwrap();
    let total = usage.values().next().unwrap();
    assert_eq!((total.input_tokens, total.output_tokens), (0, 0));
    assert!(
        !pending_turn_exists(&repo, &offer.id),
        "a decided offer cannot be decided again"
    );
    let audit = audit(&repo);
    let decision = audit
        .lines()
        .find(|line| line.contains("provider_fallback_decision"))
        .expect("the decision is audited");
    assert!(decision.contains("declined"), "{decision}");

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_fallback_pending_turn_cannot_be_resumed_as_a_command_or_a_plan() {
    let repo = temp_dir("wrong-resume");
    write_fixture(&repo, "README.md", "# Wrong\n");
    let engine = WorkspaceEngine::new(config_with(&repo, vec![provider("fallback")]));
    let mut adapter = RefusingAdapter::rate_limited();
    let offer = ask(&engine, &repo, "Explain this", &mut adapter)
        .unwrap()
        .fallback_proposal
        .unwrap();

    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    let mut answering = MockModelAdapter::new("answered");
    let as_command = engine.chat_orchestrator.resume_after_command_decision(
        &offer.id,
        true,
        "test_user",
        &mut answering,
        &mut sink,
    );
    assert_eq!(as_command.unwrap_err().code(), "invalid_input");
    let as_plan = engine.chat_orchestrator.resume_after_plan_decision(
        &offer.id,
        true,
        None,
        "test_user",
        &mut answering,
        &mut sink,
    );
    assert_eq!(as_plan.unwrap_err().code(), "invalid_input");

    assert!(
        answering.requests.is_empty(),
        "neither wrong resume called a model"
    );
    assert!(
        pending_turn_exists(&repo, &offer.id),
        "a wrong resume puts the offer back"
    );
    // And the right resume still finds it.
    assert_eq!(
        decide_fallback(&engine, &offer.id, false)
            .unwrap_err()
            .code(),
        "provider_rate_limited"
    );

    fs::remove_dir_all(repo).unwrap();
}

fn reattach_after_restart(repo: &Path, session_id: &str) -> Vec<ReattachedApproval> {
    // A fresh view of the data directory, as a relaunched process has.
    let data_dir = repo.join(".damaian");
    reattach_pending_approvals(
        &SessionStore::new(&data_dir),
        &AuditLog::new(&data_dir, true, SecretScanner::default()),
        &CommandStore::new(&data_dir),
        &PatchStore::new(&data_dir),
        &PausedTurns::new(&data_dir),
        session_id,
    )
    .unwrap()
}

#[test]
fn a_pending_fallback_reattaches_on_restart() {
    let repo = temp_dir("reattach-offer");
    write_fixture(&repo, "README.md", "# Restart\n");
    let engine = WorkspaceEngine::new(config_with(&repo, vec![provider("fallback")]));
    let mut adapter = RefusingAdapter::rate_limited();
    let result = ask(&engine, &repo, "Explain this", &mut adapter).unwrap();
    let offer = result.fallback_proposal.unwrap();
    let session = only_session(&engine, &repo);

    let reattached = reattach_after_restart(&repo, &session.id);

    match reattached.as_slice() {
        [
            ReattachedApproval::ProviderFallback {
                task_id,
                proposal_id,
                fallback_provider,
            },
        ] => {
            assert_eq!(task_id, &result.task.id);
            assert_eq!(proposal_id, &offer.id);
            assert_eq!(fallback_provider, "fallback");
        }
        other => panic!("the offer should be reattached, got {other:?}"),
    }
    assert!(
        statuses(&engine, &repo)
            .iter()
            .any(|status| status == "waiting_for_approval"),
        "still the user's to decide"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_fallback_offer_whose_paused_turn_is_gone_fails_on_restart() {
    let repo = temp_dir("reattach-offer-gone");
    write_fixture(&repo, "README.md", "# Gone\n");
    let engine = WorkspaceEngine::new(config_with(&repo, vec![provider("fallback")]));
    let mut adapter = RefusingAdapter::rate_limited();
    let offer = ask(&engine, &repo, "Explain this", &mut adapter)
        .unwrap()
        .fallback_proposal
        .unwrap();
    let session = only_session(&engine, &repo);
    fs::remove_file(
        repo.join(".damaian")
            .join("chat")
            .join("pending")
            .join(format!("{}.json", offer.id)),
    )
    .unwrap();

    let reattached = reattach_after_restart(&repo, &session.id);

    assert!(
        matches!(
            reattached.as_slice(),
            [ReattachedApproval::Unavailable { .. }]
        ),
        "got {reattached:?}"
    );
    assert!(
        statuses(&engine, &repo)
            .iter()
            .any(|status| status == "failed")
    );

    fs::remove_dir_all(repo).unwrap();
}

// Spec 56 Task 2: approving an offer re-issues the refused round through the
// fallback, records the switch, and is offered at most once per turn.

/// Pauses a turn on an offer from `primary`, returning the engine, the offer,
/// and the request `primary` refused.
fn paused_offer(
    repo: &Path,
    others: Vec<ModelProviderConfig>,
) -> (WorkspaceEngine, AgentFallbackProposal, ModelRequest) {
    write_fixture(repo, "README.md", "# Fallback\n");
    let engine = WorkspaceEngine::new(config_with(repo, others));
    let mut refusing = RefusingAdapter::rate_limited();
    let offer = ask(&engine, repo, "Explain this", &mut refusing)
        .unwrap()
        .fallback_proposal
        .expect("an offer is made");
    let refused_request = refusing.requests.pop().expect("the refused request");
    (engine, offer, refused_request)
}

#[test]
fn an_approved_offer_reissues_the_refused_call_through_the_fallback() {
    let repo = temp_dir("approve-reissues");
    let (_, offer, refused_request) = paused_offer(&repo, vec![provider("fallback")]);
    let fallback = fallback_engine(&repo, vec![provider("fallback")]);
    let mut answering = MockModelAdapter::new("answered by the fallback");

    let result = approve_with(&fallback, &offer.id, &mut answering).expect("the fallback answers");

    assert_eq!(answering.requests.len(), 1, "one call, to the fallback");
    let sent = &answering.requests[0];
    assert_eq!(sent.provider, "fallback");
    assert_eq!(sent.model, "model-b");
    assert_eq!(
        sent.messages, refused_request.messages,
        "the fallback is sent exactly what the refused provider was"
    );
    assert_eq!(result.response, "answered by the fallback");
    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(result.fallback_proposal.is_none());
    assert!(!pending_turn_exists(&repo, &offer.id));
    // The round is re-entered, not advanced: the refused one never completed.
    let rounds = prepared_rounds(&repo);
    assert_eq!(rounds.len(), 2, "one prepared request each: {rounds:?}");
    assert_eq!(
        rounds[0], rounds[1],
        "the same round both times: {rounds:?}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn an_approval_from_an_engine_still_on_the_refused_provider_is_rejected() {
    // `tasks.md` C1: the request reads its provider and model from the
    // engine's config, so approving on the refused provider's engine would
    // send `model-a` to whichever endpoint the adapter points at.
    let repo = temp_dir("approve-wrong-engine");
    let (engine, offer, _) = paused_offer(&repo, vec![provider("fallback")]);
    let mut answering = MockModelAdapter::new("must not be asked");

    let error = approve_with(&engine, &offer.id, &mut answering).unwrap_err();

    assert_eq!(error.code(), "invalid_input");
    assert!(answering.requests.is_empty(), "no model was called");
    assert!(
        pending_turn_exists(&repo, &offer.id),
        "the offer is put back for the right engine to take"
    );
    assert!(!audit(&repo).contains("provider_fallback_decision"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn an_approved_switch_is_recorded_on_the_task_and_in_the_transcript() {
    let repo = temp_dir("approve-recorded");
    let (engine, offer, _) = paused_offer(&repo, vec![provider("fallback")]);
    let fallback = fallback_engine(&repo, vec![provider("fallback")]);
    let mut answering = MockModelAdapter::new("answered");

    let result = approve_with(&fallback, &offer.id, &mut answering).unwrap();

    let session = only_session(&engine, &repo);
    assert_eq!(
        engine
            .session_store
            .read_provider_fallbacks(&session.id)
            .unwrap(),
        vec![(
            result.task.id.clone(),
            "primary".to_string(),
            "fallback".to_string()
        )]
    );
    let messages = engine.session_store.read_messages(&session.id).unwrap();
    let marker_at = messages
        .iter()
        .position(|message| {
            message.role == "system" && message.content.contains("Switched to fallback")
        })
        .expect("the transcript marks the switch");
    assert!(messages[marker_at].content.contains("primary"));
    let answer_at = messages
        .iter()
        .position(|message| message.role == "assistant" && message.content == "answered")
        .expect("the answer is in the transcript");
    assert!(
        marker_at < answer_at,
        "the marker precedes the answer it explains"
    );
    let decision = audit(&repo)
        .lines()
        .find(|line| line.contains("provider_fallback_decision"))
        .map(str::to_string)
        .expect("the decision is audited");
    assert!(decision.contains("approved"), "{decision}");
    assert!(decision.contains("model-b"), "{decision}");

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_fallback_that_also_refuses_fails_without_a_second_offer() {
    // On the fallback's engine `primary` is itself a candidate, so a second
    // offer would be possible — and would start the loop §5.5 rules out.
    let repo = temp_dir("approve-refused-again");
    let (engine, offer, _) = paused_offer(&repo, vec![provider("fallback")]);
    let fallback = fallback_engine(&repo, vec![provider("fallback")]);
    let mut refusing_again = RefusingAdapter::quota_exhausted();

    let error = approve_with(&fallback, &offer.id, &mut refusing_again).unwrap_err();

    assert_eq!(error.code(), "provider_quota_exhausted");
    assert_eq!(refusing_again.calls, 1);
    let session = only_session(&engine, &repo);
    let statuses = statuses(&engine, &repo);
    assert!(statuses.iter().any(|status| status == "failed"));
    assert!(
        !statuses
            .iter()
            .any(|status| status == "waiting_for_approval"),
        "no second offer: {statuses:?}"
    );
    assert_eq!(
        engine
            .session_store
            .read_task_failure_kinds(&session.id)
            .unwrap()
            .values()
            .next()
            .map(String::as_str),
        Some("provider_quota_exhausted"),
        "the failure names the fallback's refusal"
    );
    assert_eq!(
        audit(&repo).matches("provider_fallback_offered").count(),
        1,
        "offered once"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn the_plan_survives_an_approved_switch() {
    let repo = temp_dir("approve-plan");
    let (engine, offer, _) = paused_offer(&repo, vec![provider("fallback")]);
    let session = only_session(&engine, &repo);
    let task = engine
        .session_store
        .read_tasks(&session.id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(PlanStep {
        id: "step_1".to_string(),
        title: "Read the README".to_string(),
        detail: None,
        status: StepStatus::Pending,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
    });
    engine.session_store.create_plan(&task, &plan).unwrap();
    let fallback = fallback_engine(&repo, vec![provider("fallback")]);
    let mut answering = MockModelAdapter::new("answered");

    approve_with(&fallback, &offer.id, &mut answering).unwrap();

    let after = engine
        .session_store
        .read_task_plan(&session.id, &task.id)
        .unwrap()
        .expect("the plan survives");
    assert_eq!(after.steps.len(), 1);
    assert_eq!(after.steps[0].title, "Read the README");

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_later_turn_is_offered_again_rather_than_switched() {
    // Not remembered: the next turn starts on the configured provider, and a
    // refusal there is a fresh offer — neither an automatic switch nor a
    // suppressed one carried over from the approved turn.
    let repo = temp_dir("approve-not-remembered");
    let (engine, offer, _) = paused_offer(&repo, vec![provider("fallback")]);
    let fallback = fallback_engine(&repo, vec![provider("fallback")]);
    approve_with(&fallback, &offer.id, &mut MockModelAdapter::new("answered")).unwrap();
    let session = only_session(&engine, &repo);

    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    let mut refusing = RefusingAdapter::rate_limited();
    let next = engine
        .chat_orchestrator
        .ask_with_session(
            &repo,
            "And now?",
            &[],
            Some(&session.id),
            &mut refusing,
            &mut sink,
        )
        .unwrap();

    assert_eq!(refusing.requests[0].provider, "primary");
    let second = next.fallback_proposal.expect("offered again");
    assert_ne!(second.id, offer.id);
    assert_eq!(second.fallback_provider, "fallback");

    fs::remove_dir_all(repo).unwrap();
}

// `Config::fallback_provider_for` is pure, so it is pinned directly as well as
// through the turns above (spec 56 §5.2, `tasks.md` C2 and C3).

fn rate_limited() -> ProviderRefusal {
    ProviderRefusal::RateLimited {
        retry_after_secs: None,
    }
}

fn fallback_id(config: &Config, refusal: &ProviderRefusal) -> Option<String> {
    config
        .fallback_provider_for(refusal)
        .map(|candidate| candidate.id.clone())
}

#[test]
fn no_candidate_when_only_the_active_provider_is_configured() {
    let config = config_with(Path::new("/unused"), Vec::new());
    assert_eq!(fallback_id(&config, &rate_limited()), None);
}

#[test]
fn a_provider_with_an_empty_key_env_is_not_a_candidate() {
    let mut keyless = provider("keyless");
    keyless.api_key_env = "  ".to_string();
    let config = config_with(Path::new("/unused"), vec![keyless]);
    assert_eq!(fallback_id(&config, &rate_limited()), None);
}

#[test]
fn quota_exhaustion_skips_a_provider_sharing_the_active_key_env() {
    let mut shared = provider("shared");
    shared.api_key_env = "PRIMARY_KEY".to_string();
    let config = config_with(Path::new("/unused"), vec![shared]);
    assert_eq!(fallback_id(&config, &ProviderRefusal::QuotaExhausted), None);
}

#[test]
fn rate_limiting_accepts_a_provider_sharing_the_key_env() {
    // A different provider id on the same key is a different endpoint, which
    // need not share the busy one's limit — only an empty *account* is
    // certain to be empty everywhere it is used.
    let mut shared = provider("shared");
    shared.api_key_env = "PRIMARY_KEY".to_string();
    let config = config_with(Path::new("/unused"), vec![shared]);
    assert_eq!(
        fallback_id(&config, &rate_limited()),
        Some("shared".to_string())
    );
}

#[test]
fn a_candidate_must_match_native_tool_support() {
    let mut native = provider("native");
    native.supports_native_tools = true;
    let config = config_with(Path::new("/unused"), vec![native]);
    assert_eq!(fallback_id(&config, &rate_limited()), None);
}

#[test]
fn the_first_matching_provider_in_config_order_is_chosen() {
    let config = config_with(
        Path::new("/unused"),
        vec![provider("second"), provider("third")],
    );
    assert_eq!(
        fallback_id(&config, &rate_limited()),
        Some("second".to_string())
    );
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
