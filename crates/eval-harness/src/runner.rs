use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    AgentCommandProposal, AgentPatchProposal, CancelToken, ChatTurnOptions, ClientError, Config,
    ContextCategory, ContextDecision, CurlModelTransport, MemoryProposal, MemoryScope, MemoryState,
    MemoryStore, MemoryStoreConfig, MemoryTarget, MockModelAdapter, ModelAdapter,
    ModelProviderConfig, ModelRequest, OpenAICompatibleAdapter, ProcessRegistry, ProjectIdentity,
    ProjectKeySource, ProposalOrigin, Result, RoundBudgetDecision, SecretScanner, SessionStore,
    Task, TaskStatus, ToolCall, TurnProgress, TurnSink, UsageSource, UserConfirmation,
    VerificationReport, WorkspaceEngine, classify_session, evidence_ref_for, resume,
};

use crate::fixture::{self, Materialized};
use crate::record::{
    CacheUsage, MemoryRecallCounts, RecordedApproval, RecordedCheck, RecordedPlan,
    RecordedRecovery, RunRecord, Tokens,
};
use crate::scenario::{
    CrashMidAction, RoundBudgetDecision as ScenarioRoundBudgetDecision, STUB_BASE_PLACEHOLDER,
    Scenario, Tier,
};
use crate::stub::StubServer;
use crate::trace::{self, Trace};

/// The synthetic origin a seeded memory records. A seed is a fixture for a
/// confirmation that happened before the run; there is no earlier session to
/// name, and the provenance is only ever shown, never re-read.
const SEED_ORIGIN_SESSION: &str = "eval-seed";
const SEED_ORIGIN_MESSAGE: &str = "eval-seed-message";

#[derive(Debug)]
pub struct Run {
    pub record: RunRecord,
    pub repo_root: PathBuf,
    pub data_dir: PathBuf,
    pub response: String,
    pub context_files: Vec<String>,
    pub command_proposal: Option<AgentCommandProposal>,
    pub patch_proposal: Option<AgentPatchProposal>,
    /// Set only when the scenario declared `modify_after_proposal` and the
    /// resulting apply was refused. `None` means either no apply was attempted
    /// or it succeeded.
    pub apply_error: Option<String>,
    /// The message contents of the first model request made after a
    /// `turn_budget_extended`, joined for substring assertions. Only the
    /// deterministic tier can supply it: the mock adapter records its requests,
    /// a live provider does not (and the scenario's script does not describe a
    /// live request). `None` means the run did not continue a turn.
    pub continuation_request: Option<String>,
    /// The message contents of the first model request made after the turn
    /// paused on a question, which is the request that carried the scripted
    /// answer back to the model (spec 50 §9). `None` means no question was
    /// put up for an answer, and, like `continuation_request`, only the
    /// deterministic tier can supply it.
    pub answer_request: Option<String>,
    /// The report the verification continuation produced, when the scenario
    /// asked for one (spec 23 Task 9). `None` for every other scenario.
    pub verification: Option<VerificationReport>,
    /// The task's last assistant message as the session log holds it, which is
    /// not always `response`: after a plan reminder the pre-reminder answer is
    /// the response and the post-reminder reply follows it (spec 66 `context.md`
    /// D4). `None` when the turn wrote no assistant message.
    pub last_assistant_message: Option<String>,
    /// The first model request after the run's `conversation_compacted` audit
    /// event, flattened (spec 55 §5.9). `None` when the run did not compact or
    /// when the live tier has no recorded request.
    pub compacted_request: Option<String>,
    /// The system message of that same request, kept separately so an
    /// assertion can prove a summary never becomes an instruction.
    pub compacted_system_prompt: Option<String>,
    /// How many `conversation_compacted` events the run appended.
    pub conversation_compactions: u64,
    /// Whether the newest summary is facts-only.
    pub compaction_facts_only: bool,
    /// How many HTTP requests the scenario's loopback page server accepted
    /// (spec 51 Task 9). Captured before the server is dropped, because the
    /// count is a measurement of the run rather than of a live listener.
    pub stub_requests: u64,
    /// The first model request's message contents, joined (spec 30 Task 6).
    /// Only the deterministic tier can supply it: the mock adapter records its
    /// requests. This is the rendered context the model actually received, so
    /// the memory delivery assertions read it rather than the manifest.
    pub request: Option<String>,
    /// The system message of that same request, kept separately so an assertion
    /// can prove a memory never became an instruction.
    pub system_prompt: Option<String>,
    pub trace: Trace,
}

/// The provider the deterministic tier runs as. `supports_native_tools` must be
/// on or the orchestrator offers no tool schemas and every scripted tool call is
/// ignored (`Config::supports_native_tools`, `config.rs:718`). `base_url` and
/// `api_key_env` are deliberately empty: nothing may reach the network.
pub fn mock_provider() -> ModelProviderConfig {
    ModelProviderConfig {
        id: "mock".to_string(),
        label: "Mock".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: vec!["mock".to_string()],
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
        // The mock adapter is not a provider and reports no usage, so asking
        // for it changes nothing here. Left on so the deterministic tier sends
        // the same request shape the live tier does.
        provider_reports_usage: true,
        // No rates: an eval run reports tokens, and a priced figure would be
        // a number the baseline could not reproduce on another machine.
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    }
}

/// The deterministic tier: a scripted `MockModelAdapter`, no credentials and no
/// network.
pub fn run(scenario: &Scenario) -> Result<Run> {
    if scenario.tier != Tier::Deterministic {
        return Err(ClientError::InvalidInput(format!(
            "{}: runner::run drives the deterministic tier; use run_live for the live tier",
            scenario.name
        )));
    }
    if let Some(skipped) = skip_if_blocked(scenario) {
        return Ok(skipped);
    }

    // Spec 51 Task 9: one page server per scenario, started before the fixture
    // so the substituted URL is already in the prompt and tool arguments. It is
    // dropped when this function returns.
    let stub = StubServer::start(&scenario.stub_pages)?;
    let scenario = with_stub_base(scenario, stub.as_ref());

    let materialized = fixture::materialize(&scenario.fixture)?;
    for file in &scenario.seed_files {
        let path = materialized.root.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                workspace_engine::ClientError::Io(format!(
                    "{}: seed file parent: {error}",
                    path.display()
                ))
            })?;
        }
        std::fs::write(&path, &file.content).map_err(|error| {
            workspace_engine::ClientError::Io(format!("{}: seed file: {error}", path.display()))
        })?;
    }
    let mut config = base_config(&materialized);
    apply_scenario_config(&mut config, &scenario);
    pre_approve_stub(&mut config, stub.as_ref());
    config.model_provider = "mock".to_string();
    config.model_name = "mock".to_string();
    config.model_providers.push(mock_provider());
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let mut engine = WorkspaceEngine::new(config);
    // The stub page server binds 127.0.0.1. This is the harness's own
    // allowance, not something a scenario can configure: the scenario only
    // pre-approves the stub's `host:port`, which the SSRF check would still
    // refuse without this.
    engine.chat_orchestrator.allow_loopback_fetch_for_tests();

    let (responses, tool_calls, truncated) = script(&scenario);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, tool_calls)
        .with_truncated(truncated);

    let mut run = drive(
        &scenario,
        &materialized,
        &engine,
        &scanner,
        &mut adapter,
        "deterministic",
        "mock",
        "mock",
    )?;
    run.stub_requests = stub.as_ref().map(StubServer::requests).unwrap_or(0);
    // The adapter is the only place the tier can see what a request actually
    // contained, and the resume happens inside `drive`. Read it here, after the
    // run, and locate the first post-continuation request from the trace: the
    // `turn_budget_extended` record is written before the resumed loop makes
    // its next call, so the calls before it are the pre-pause ones.
    run.continuation_request = continuation_request(&run.trace, &adapter.requests);
    run.answer_request = answer_request(&run.trace, &adapter.requests);
    (run.compacted_request, run.compacted_system_prompt) = compacted_requests(&adapter.requests);
    run.request = adapter.requests.first().map(flattened_request);
    run.system_prompt = adapter
        .requests
        .first()
        .and_then(|request| {
            request
                .messages
                .iter()
                .find(|message| message.role == "system")
        })
        .map(|message| message.content.clone());
    Ok(run)
}

/// Replaces `{stub_base}` with the running page server's base URL.
///
/// The port is ephemeral, so a scenario file cannot name it. The prompt, the
/// follow-up prompt, seeded conversation and every string in a scripted
/// tool-call argument are substituted, because those are the places a scripted
/// URL can appear.
pub fn substitute_stub_base(scenario: &Scenario, base: &str) -> Scenario {
    let mut scenario = scenario.clone();
    scenario.prompt = scenario.prompt.replace(STUB_BASE_PLACEHOLDER, base);
    if let Some(prompt) = &mut scenario.follow_up_prompt {
        *prompt = prompt.replace(STUB_BASE_PLACEHOLDER, base);
    }
    for message in &mut scenario.seed_conversation {
        message.content = message.content.replace(STUB_BASE_PLACEHOLDER, base);
    }
    for turn in &mut scenario.turns {
        for call in &mut turn.tool_calls {
            replace_in_value(&mut call.arguments, base);
        }
    }
    scenario
}

fn with_stub_base(scenario: &Scenario, stub: Option<&StubServer>) -> Scenario {
    match stub {
        Some(stub) => substitute_stub_base(scenario, stub.base()),
        None => scenario.clone(),
    }
}

fn replace_in_value(value: &mut serde_json::Value, base: &str) {
    match value {
        serde_json::Value::String(text) => *text = text.replace(STUB_BASE_PLACEHOLDER, base),
        serde_json::Value::Array(items) => {
            for item in items {
                replace_in_value(item, base);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values_mut() {
                replace_in_value(item, base);
            }
        }
        _ => {}
    }
}

/// The harness is the user here, and the user has approved the page server.
///
/// A `fetch_url` to a host with no approval pauses for a dialog, and the
/// harness has no action that answers one, so each fetch scenario must arrive
/// with its stub already allowed. The port is ephemeral, so this cannot be a
/// line in the scenario file; the runner appends the exact `host:port` the
/// server bound to the user-scope list. This does not weaken the pause — a
/// scenario that names any other host still pauses — it only says the user
/// approved the loopback page the scenario itself serves.
fn pre_approve_stub(config: &mut Config, stub: Option<&StubServer>) {
    if let Some(stub) = stub {
        config
            .web_domain_allowlist
            .push(stub.approval_key().to_string());
    }
}

/// The request that followed the first `turn_budget_extended`, flattened to the
/// message contents it sent. `None` when the run granted no continuation.
fn continuation_request(trace: &Trace, requests: &[ModelRequest]) -> Option<String> {
    let extended_at = trace
        .events
        .iter()
        .position(|event| event.event_type == "turn_budget_extended")?;
    let index = trace.events[..extended_at]
        .iter()
        .filter(|event| event.event_type == "model_request_prepared")
        .count();
    requests.get(index).map(flattened_request)
}

/// The request that followed the first turn paused on a question, flattened to
/// the message contents it sent. `None` when no question was put up for an
/// answer.
///
/// The pause is located by the turn's own audit record (`question_pending`)
/// rather than by counting requests, so a question asked after any number of
/// earlier rounds still resolves to the right one.
fn answer_request(trace: &Trace, requests: &[ModelRequest]) -> Option<String> {
    let paused_at = trace.events.iter().position(|event| {
        event.event_type == "model_response_completed"
            && event.field("status") == Some("question_pending")
    })?;
    let index = trace.events[..paused_at]
        .iter()
        .filter(|event| event.event_type == "model_request_prepared")
        .count();
    requests.get(index).map(flattened_request)
}

/// The first request after the summarisation call, and its system message.
///
/// The summarisation call is not preceded by a `model_request_prepared` audit
/// event (it is not a turn request), so it is located by its own prompt text
/// rather than by counting trace events. `None` when the run never compacted.
fn compacted_requests(requests: &[ModelRequest]) -> (Option<String>, Option<String>) {
    let Some(index) = requests.iter().position(|request| {
        request.messages.iter().any(|message| {
            message
                .content
                .contains("You are recording what happened in an earlier part of a coding session")
        })
    }) else {
        return (None, None);
    };
    let Some(next) = requests.get(index + 1) else {
        return (None, None);
    };
    let system = next
        .messages
        .iter()
        .find(|message| message.role == "system")
        .map(|message| message.content.clone());
    (Some(flattened_request(next)), system)
}

/// One request's message contents, joined for substring assertions.
fn flattened_request(request: &ModelRequest) -> String {
    request
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The live tier: the same scenario files against a real provider, **ignoring
/// the `[[turn]]` scripts** and keeping the `[assert]` block (§5.3).
/// Credential-gated and never run by CI.
///
/// NOT YET VERIFIED against a real provider, though the CLI's own live path
/// (`crates/damaian-cli/src/main.rs:313`) that this is built from now has been:
/// it reaches DeepSeek, and the provider reports usage.
///
/// This function stayed unreachable longer than that note implied. `run_tier`
/// selected no scenarios for the live tier and called the deterministic runner
/// for anything it did select, so `--tier live` never arrived here and exited 0
/// having measured nothing. Both are fixed; the `#[ignore]`d
/// `live_tier_runs_one_scenario_against_a_real_provider` test is still what
/// proves this path works.
pub fn run_live(scenario: &Scenario) -> Result<Run> {
    if let Some(skipped) = skip_if_blocked(scenario) {
        return Ok(skipped);
    }

    let provider = std::env::var("DAMAIAN_EVAL_PROVIDER").map_err(|_| {
        ClientError::InvalidInput(
            "the live tier needs DAMAIAN_EVAL_PROVIDER (and that provider's API-key variable); \
             it is never run by CI"
                .to_string(),
        )
    })?;
    let model = std::env::var("DAMAIAN_EVAL_MODEL").map_err(|_| {
        ClientError::InvalidInput("the live tier needs DAMAIAN_EVAL_MODEL".to_string())
    })?;

    // The page server is tier-independent: the live tier asks a real model the
    // same question the deterministic one scripts, so it fetches the same
    // loopback pages rather than reaching a real host (spec 51 Task 9).
    let stub = StubServer::start(&scenario.stub_pages)?;
    let scenario = with_stub_base(scenario, stub.as_ref());

    let materialized = fixture::materialize(&scenario.fixture)?;
    let mut config = live_config(&materialized.data_dir, &provider, &model);
    apply_scenario_config(&mut config, &scenario);
    pre_approve_stub(&mut config, stub.as_ref());

    let api_key = std::env::var(&config.model_api_key_env).map_err(|_| {
        ClientError::InvalidInput(format!(
            "{} is required for the live tier",
            config.model_api_key_env
        ))
    })?;
    let transport = CurlModelTransport::new(
        &config.model_base_url,
        api_key,
        ProcessRegistry::open(&config.data_dir)?,
        &config.data_dir,
    );
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let mut adapter = OpenAICompatibleAdapter::with_provider(&provider, &model, transport);
    let mut engine = WorkspaceEngine::new(config);
    // Same allowance as the deterministic tier: a scenario's `[[stub_page]]`
    // is served on loopback, and no scenario config can grant it.
    engine.chat_orchestrator.allow_loopback_fetch_for_tests();

    let mut run = drive(
        &scenario,
        &materialized,
        &engine,
        &scanner,
        &mut adapter,
        "live",
        &provider,
        &model,
    )?;
    run.stub_requests = stub.as_ref().map(StubServer::requests).unwrap_or(0);
    Ok(run)
}

/// The live tier's configuration for a provider and model.
///
/// Separate from [`run_live`] so the capability set it produces can be asserted
/// without credentials — the tiers disagreeing about that set is a defect that
/// no credential-free test could otherwise reach.
pub fn live_config(data_dir: &Path, provider: &str, model: &str) -> Config {
    let mut config = Config {
        data_dir: data_dir.to_path_buf(),
        ..Config::default()
    };
    config.model_provider = provider.to_string();
    // Fills `model_base_url` and `model_api_key_env` from the provider entry,
    // which is what the CLI and the desktop shell both rely on before building
    // a transport.
    config.apply_model_provider_defaults();
    // After the defaults, which replace `model_name` with the provider's first
    // listed model: set before them, every OpenRouter run called Claude while
    // its record named the requested model.
    config.model_name = model.to_string();

    // Both tiers must offer the model the same tools, or their assertions
    // measure different assistants and §5.3's single scenario definition buys
    // nothing. The deterministic tier's mock provider enables the native tool
    // contract, so the live tier does too — setting it explicitly rather than
    // relying on the provider entry's default, so a result does not change
    // with that default.
    //
    // This is a deliberate property of the eval configuration and not of the
    // shipped default. `RunRecord::native_tools` carries it into every record
    // so a reader can see which capability set produced a result.
    config.model_providers.push(ModelProviderConfig {
        id: provider.to_string(),
        label: provider.to_string(),
        base_url: config.model_base_url.clone(),
        api_key_env: config.model_api_key_env.clone(),
        models: vec![model.to_string()],
        supports_native_tools: true,
        // Left unset so the built-in per-model ceiling applies where one
        // exists. Only DeepSeek's models have one: an OpenRouter run sends no
        // `max_tokens`, deliberately (spec 18 §7, observation #44), and
        // budgets the default 16,000-token context.
        max_output_tokens: None,
        context_token_budget: None,
        provider_reports_usage: true,
        // No rates, for the same reason `mock_provider` carries none: a priced
        // figure could not be reproduced on another machine.
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    });
    config
}

/// A blocked scenario is skipped before anything is materialized: it measures a
/// capability that does not exist, so running it would either fail for the wrong
/// reason or pass without measuring anything.
fn skip_if_blocked(scenario: &Scenario) -> Option<Run> {
    let blocking = scenario.blocked_on.as_ref()?;
    let mut skipped = RunRecord::new(
        &scenario.name,
        "n/a",
        scenario.tier.as_str(),
        "none",
        "none",
    );
    skipped.final_status = "not_applicable".to_string();
    skipped.not_applicable = Some(blocking.clone());
    Some(Run {
        record: skipped,
        repo_root: PathBuf::new(),
        data_dir: PathBuf::new(),
        response: String::new(),
        context_files: Vec::new(),
        command_proposal: None,
        patch_proposal: None,
        apply_error: None,
        continuation_request: None,
        answer_request: None,
        verification: None,
        last_assistant_message: None,
        compacted_request: None,
        compacted_system_prompt: None,
        conversation_compactions: 0,
        compaction_facts_only: false,
        stub_requests: 0,
        request: None,
        system_prompt: None,
        trace: Trace::default(),
    })
}

fn base_config(materialized: &Materialized) -> Config {
    Config {
        data_dir: materialized.data_dir.clone(),
        // Every scenario materializes a fresh fixture repository, runs one turn
        // against it and discards it. Nothing ever changes underneath the
        // index, so a watcher buys no freshness here and costs seconds of
        // FSEvents registration per scenario.
        enable_index_watcher: false,
        ..Config::default()
    }
}

/// The turn options a run drives with.
///
/// A scenario that scripts answers is interactive, so `ask_user` is offered and
/// the system prompt carries no headless paragraph. A scenario can also say a
/// user is present without scripting an answer: `fetch_url` is offered on the
/// same condition as `ask_user` [spec 51 §5.3], and a fetch scenario
/// pre-approves its stub host so nothing actually pauses for an answer the
/// script does not have. Everything else keeps the defaults, which is headless:
/// the tool is absent and the prompt tells the model to proceed under a stated
/// assumption (spec 50 `context.md` §4, §9).
fn turn_options(scenario: &Scenario) -> ChatTurnOptions {
    ChatTurnOptions {
        user_present: !scenario.answers.is_empty() || scenario.user_present,
        ..ChatTurnOptions::default()
    }
}

/// Applies the config a scenario declares for itself, at user scope — the
/// harness is the user in a run. Parsed in `scenario::load`, so a malformed
/// line fails there rather than being ignored here.
///
/// The harness keeps ownership of `data_dir`: a scenario may tune budgets and
/// tools, but it may not redirect a run's state into the user's real
/// application-support directory (`guard::assert_safe_data_dir` exists for the
/// same reason).
fn apply_scenario_config(config: &mut Config, scenario: &Scenario) {
    let data_dir = config.data_dir.clone();
    if let Some(overlay) = scenario.config_overlay.clone() {
        config.apply_overlay(overlay);
    }
    config.data_dir = data_dir;
}

/// One entry [`seed_memory`] stored, kept for the review hook and the metric.
///
/// The identity travels with the entry because a review deletion must address
/// the scope the entry actually lives in, and a project entry planted under
/// another project key lives in that key's file.
struct SeededMemoryEntry {
    id: String,
    statement: String,
    identity: Option<ProjectIdentity>,
}

impl SeededMemoryEntry {
    fn target(&self) -> MemoryTarget<'_> {
        match &self.identity {
            Some(identity) => MemoryTarget::Project(identity),
            None => MemoryTarget::User,
        }
    }
}

/// Writes a scenario's declared memories through the production confirmation
/// path (spec 30 Task 6).
///
/// The store is built with user scope enabled for the seeding write even when
/// the run itself has it off. That is exactly the state the "user scope off"
/// scenario needs: an entry that was stored while the scope was on, which a
/// later run must not read. The run's own engine keeps its configured switch,
/// so nothing is widened for the turn.
fn seed_memory(
    engine: &WorkspaceEngine,
    root: &Path,
    scenario: &Scenario,
) -> Result<Vec<SeededMemoryEntry>> {
    if scenario.seed_memory.is_empty() {
        return Ok(Vec::new());
    }
    let identity = ProjectIdentity::resolve_for_root(root, &engine.git)?;
    let store = MemoryStore::new(
        MemoryStoreConfig {
            data_dir: engine.config.data_dir.clone(),
            user_scope_enabled: true,
            ignore_patterns: engine.config.ignore_patterns.clone(),
            retention_days: engine.config.memory_retention_days,
        },
        engine.audit_log.clone(),
        engine.scanner.clone(),
        engine.path_policy.clone(),
        engine.session_store.clone(),
    );

    let mut seeded = Vec::with_capacity(scenario.seed_memory.len());
    for seed in &scenario.seed_memory {
        let entry_identity = match seed.scope {
            MemoryScope::Project => {
                let key = seed
                    .project_key
                    .clone()
                    .unwrap_or_else(|| identity.key.clone());
                Some(ProjectIdentity {
                    source: ProjectIdentity::source_of_key(&key)
                        .unwrap_or(ProjectKeySource::RootCommit),
                    key,
                    repository_id: identity.repository_id.clone(),
                })
            }
            MemoryScope::User => None,
            MemoryScope::Session => {
                return Err(ClientError::InvalidInput(format!(
                    "{}: session-scope memory cannot be seeded before the turn creates its session",
                    scenario.name
                )));
            }
        };
        // The origin decides the provenance the user sees and the verification
        // the entry starts in: an evidence-backed entry is `EvidenceCurrent`,
        // everything else is `UserConfirmed`. Both are entries a user could
        // legitimately have confirmed; the instruction shape is allowed only
        // because `UserRequest` is the trusted origin (spec 29 §5.3).
        let (origin, evidence) = match &seed.evidence_path {
            Some(path) => (
                ProposalOrigin::RepositoryObservation {
                    session_id: SEED_ORIGIN_SESSION.to_string(),
                },
                vec![evidence_ref_for(root, path)?],
            ),
            None => (
                ProposalOrigin::UserRequest {
                    session_id: SEED_ORIGIN_SESSION.to_string(),
                    message_id: SEED_ORIGIN_MESSAGE.to_string(),
                },
                Vec::new(),
            ),
        };
        let proposal = MemoryProposal::new(
            seed.scope,
            seed.category,
            seed.statement.clone(),
            origin,
            evidence,
            None,
        );
        let confirmation = UserConfirmation::new(proposal.id.clone());
        let entry = proposal.confirm(
            confirmation,
            store.proposal_gate(root, entry_identity.as_ref()),
        )?;
        let stored = store.insert(root, entry_identity.as_ref(), entry)?;
        seeded.push(SeededMemoryEntry {
            id: stored.id,
            statement: stored.statement,
            identity: entry_identity,
        });
    }
    Ok(seeded)
}

/// The declared seed a scenario names by statement, or a clear error.
fn seeded_entry<'a>(
    seeded: &'a [SeededMemoryEntry],
    statement: &str,
) -> Result<&'a SeededMemoryEntry> {
    seeded
        .iter()
        .find(|entry| entry.statement == statement)
        .ok_or_else(|| {
            ClientError::InvalidInput(format!("no seed_memory statement matches {statement:?}"))
        })
}

/// A run's recall counts, from the store and the run's own output (spec 30
/// Task 6, context.md C11).
///
/// **Proxy, deliberately.** "Useful" means a statement term reappeared in the
/// answer or in a file the run changed. That is evidence the memory was not
/// inert; it is not proof the agent acted on it, and it cannot see a memory the
/// model correctly ignored. The correction half is C11's: an entry the user
/// removed from the review, or one edited or superseded during the run. A
/// verification transition and a world-state deletion are lifecycle events, not
/// corrections of what the user was shown, so they are not counted. The phase's
/// real validation is the elapsed-time observation in the proposal's §7.
fn memory_counts(
    data_dir: &Path,
    root: &Path,
    recalled: &HashSet<String>,
    corrected: &HashSet<String>,
    response: &str,
    files_changed: &[String],
) -> MemoryRecallCounts {
    if recalled.is_empty() {
        return MemoryRecallCounts::default();
    }
    let statements = memory_statements(data_dir);
    let mut haystack = content_terms(response);
    for path in files_changed {
        if let Ok(text) = std::fs::read_to_string(root.join(path)) {
            haystack.extend(content_terms(&text));
        }
    }
    let useful = recalled
        .iter()
        .filter(|id| {
            statements.get(*id).is_some_and(|statement| {
                let terms = content_terms(statement);
                !terms.is_empty() && terms.iter().any(|term| haystack.contains(term))
            })
        })
        .count() as u64;
    MemoryRecallCounts {
        recalled: recalled.len() as u64,
        useful,
        corrected: recalled.intersection(corrected).count() as u64,
    }
}

/// Every stored memory statement, keyed by entry id, across project files, the
/// user file and session logs.
///
/// Read from the store rather than from the audit: the manifest and every audit
/// event are deliberately content-free (spec 30 C3), so the statement is only
/// available where the user's store keeps it. Later records win, which is how
/// the append-only format expresses an edit or a state change.
fn memory_statements(data_dir: &Path) -> HashMap<String, String> {
    let mut statements = HashMap::new();
    let memory = data_dir.join("memory");
    if let Ok(entries) = std::fs::read_dir(memory.join("project")) {
        for entry in entries.flatten() {
            read_memory_lines(&entry.path(), &mut statements);
        }
    }
    read_memory_lines(&memory.join("user.jsonl"), &mut statements);

    // Session-scope entries live in the session log as `memory_recorded` rows.
    if let Ok(entries) = std::fs::read_dir(data_dir.join("sessions")) {
        for entry in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for line in text.lines() {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                if value.get("eventType").and_then(|kind| kind.as_str()) != Some("memory_recorded")
                {
                    continue;
                }
                if let Some(payload) = value.get("payload")
                    && let Some((id, statement)) = memory_record(payload)
                {
                    statements.insert(id, statement);
                }
            }
        }
    }
    statements
}

fn read_memory_lines(path: &Path, statements: &mut HashMap<String, String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if let Some((id, statement)) = memory_record(&value) {
            statements.insert(id, statement);
        }
    }
}

fn memory_record(value: &serde_json::Value) -> Option<(String, String)> {
    let id = value.get("id")?.as_str()?.to_string();
    let statement = value.get("statement")?.as_str()?.to_string();
    Some((id, statement))
}

/// Terms a statement or an answer is scored on.
///
/// A copy of the recall tokeniser's shape, not a shared function: the engine
/// keeps `tokenize` crate-private on purpose (spec 30 C1), and widening the
/// production API for an eval proxy is the wrong trade. Same separators and the
/// same common-word filter, so a statement made only of `the`/`project` does not
/// count as useful merely because the answer said "the project".
fn content_terms(text: &str) -> BTreeSet<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "about", "after", "again", "all", "also", "an", "and", "any", "are", "as", "at", "be",
        "because", "been", "before", "being", "between", "both", "but", "by", "can", "could",
        "did", "do", "does", "done", "down", "during", "each", "every", "for", "from", "further",
        "had", "has", "have", "here", "how", "i", "if", "in", "into", "is", "it", "its", "just",
        "may", "me", "might", "more", "most", "must", "my", "no", "nor", "not", "of", "off", "on",
        "once", "only", "or", "other", "our", "out", "over", "own", "project", "projects", "same",
        "shall", "should", "so", "some", "such", "than", "that", "the", "their", "them", "then",
        "there", "these", "they", "this", "those", "through", "to", "too", "under", "up", "us",
        "use", "used", "uses", "using", "very", "was", "we", "were", "what", "when", "where",
        "which", "who", "why", "will", "with", "within", "without", "would", "you", "your",
    ];
    let mut terms = BTreeSet::new();
    let mut current = String::new();
    let mut push = |current: &mut String| {
        let term = current.trim_end_matches(['.', '/']).to_ascii_lowercase();
        current.clear();
        if term.len() > 1 && !STOP_WORDS.contains(&term.as_str()) {
            terms.insert(term);
        }
    };
    // The cap keeps a pathological file from making an eval run quadratic; a
    // fixture file that matters is far below it.
    for character in text
        .chars()
        .take(1_000_000)
        .flat_map(|character| character.to_lowercase())
    {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '$' | '/' | '-') {
            current.push(character);
        } else if !current.is_empty() {
            push(&mut current);
        }
    }
    if !current.is_empty() {
        push(&mut current);
    }
    terms
}

/// Drives one turn and builds its record. Shared by both tiers so there is one
/// implementation of "what a run record means" rather than two that drift.
#[allow(clippy::too_many_arguments)]
fn drive(
    scenario: &Scenario,
    materialized: &Materialized,
    engine: &WorkspaceEngine,
    scanner: &SecretScanner,
    adapter: &mut dyn ModelAdapter,
    tier: &str,
    provider: &str,
    model: &str,
) -> Result<Run> {
    let mut on_token = |_token: &str| {};
    let started_at_ms = now_millis();
    let never_cancelled = CancelToken::new();
    let mut discard_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut discard_progress,
        cancel: &never_cancelled,
        steering: None,
    };
    // Spec 55 §5.9: a seeded conversation lets a deterministic scenario start
    // long enough to compact, and `compact_before_prompt` runs the public
    // manual path before the ask. The compaction consumes the first scripted
    // turn, which a scenario therefore writes as its narrative reply.
    //
    // Spec 30 Task 6 adds two more reasons to create the session up front: a
    // scenario that sets a working mode needs a session to set it on, and a
    // seeded-memory deletion needs the paused review's session to still be the
    // one the resumed send uses.
    let seeded_entries = seed_memory(engine, &materialized.root, scenario)?;
    let seeded_session = if !scenario.seed_conversation.is_empty()
        || scenario.compact_before_prompt
        || scenario.mode.is_some()
    {
        let repository_id = engine
            .indexer
            .repository_id_for_path(&materialized.root)
            .map_err(|error| ClientError::Io(error.to_string()))?;
        let session = engine
            .session_store
            .create_session(&repository_id, &scenario.prompt)?;
        if let Some(mode) = scenario.mode {
            engine
                .session_store
                .set_session_mode(&session.id, mode, "eval-harness")?;
        }
        for message in &scenario.seed_conversation {
            engine.session_store.append_message(
                &session.id,
                None,
                &message.role,
                &message.content,
            )?;
        }
        if scenario.compact_before_prompt {
            engine.chat_orchestrator.compact_conversation(
                &materialized.root,
                &session.id,
                adapter,
                &CancelToken::new(),
            )?;
        }
        Some(session.id)
    } else {
        None
    };
    // Spec 30 Task 6: a file changed after its memory was stored is what makes
    // the entry stale. Written here, after seeding and before the turn, so recall
    // compares the recorded hash with the new content.
    if let Some((path, content)) = &scenario.modify_after_memory_seed {
        let target = materialized.root.join(path);
        std::fs::write(&target, content).map_err(|error| {
            ClientError::Io(format!("{}: seed change: {error}", target.display()))
        })?;
    }
    let outcome = engine.chat_orchestrator.ask_with_session_with_options(
        &materialized.root,
        &scenario.prompt,
        &[],
        seeded_session.as_deref(),
        adapter,
        &mut sink,
        turn_options(scenario),
    );

    // Captured before any resume: the resumed turn does not carry the proposal
    // that stopped the original one, and `approval_required` asserts on it.
    let original_command_proposal = outcome
        .as_ref()
        .ok()
        .and_then(|result| result.command_proposal.clone());

    // Captured here for the same reason the proposal is: both are consumed by
    // the status match below. A resumed turn re-reads the same task, so its
    // total already covers the rounds before the approval and wins when there
    // is one.
    let mut recorded_usage = outcome.as_ref().ok().and_then(|result| result.usage);

    // A scenario that scripts an approval decision resumes the turn with it, so
    // the denied path is exercised end to end rather than stopping at the
    // proposal.
    let mut resumed = None;
    // Spec 30 Task 6's metric inputs. Distinct ids, because one entry recalled
    // twice in a session is one observation for a rate. The review rows are
    // captured before the review is amended: an entry the user removes, or the
    // world deletes before the send, was still recalled into the review and
    // belongs in the denominator.
    let mut recalled_ids: HashSet<String> = HashSet::new();
    let mut corrected_ids: HashSet<String> = HashSet::new();
    if !scenario.delete_recalled_memory_before_send.is_empty()
        || !scenario.remove_recalled_memory_before_send.is_empty()
    {
        let review = outcome
            .as_ref()
            .ok()
            .and_then(|result| result.context_review.as_ref())
            .ok_or_else(|| {
                ClientError::InvalidInput(format!(
                    "{}: a before-send memory mutation requires a paused context review; set \
                     `pause_for_context_review = true` in config_overlay",
                    scenario.name
                ))
            })?;
        recalled_ids.extend(
            review
                .manifest
                .memory
                .iter()
                .map(|row| row.entry_id.clone()),
        );
        // Indices in `manifest.included`, not in `manifest.memory`: the review's
        // removal route addresses items, and every memory item is pathless with
        // the same kind, so only its position identifies it.
        let mut removals: Vec<usize> = Vec::new();
        for statement in &scenario.remove_recalled_memory_before_send {
            let entry = seeded_entry(&seeded_entries, statement)?;
            let position = review
                .manifest
                .memory
                .iter()
                .position(|row| row.entry_id == entry.id)
                .ok_or_else(|| {
                    ClientError::InvalidInput(format!(
                        "{}: {statement:?} was not recalled, so it cannot be removed from the review",
                        scenario.name
                    ))
                })?;
            let item_index = review
                .manifest
                .included
                .iter()
                .enumerate()
                .filter(|(_, item)| item.category == ContextCategory::Memory)
                .nth(position)
                .map(|(index, _)| index)
                .ok_or_else(|| {
                    ClientError::InvalidInput(format!(
                        "{}: the review manifest has no item for {statement:?}",
                        scenario.name
                    ))
                })?;
            removals.push(item_index);
            corrected_ids.insert(entry.id.clone());
        }
        // Highest index first, so an earlier removal cannot shift a later one.
        removals.sort_unstable_by(|left, right| right.cmp(left));
        for index in removals {
            engine
                .chat_orchestrator
                .remove_context_item(&review.id, index)?;
        }
        for statement in &scenario.delete_recalled_memory_before_send {
            let entry = seeded_entry(&seeded_entries, statement)?;
            if !review
                .manifest
                .memory
                .iter()
                .any(|row| row.entry_id == entry.id)
            {
                return Err(ClientError::InvalidInput(format!(
                    "{}: {statement:?} was not recalled, so it cannot be deleted before the send",
                    scenario.name
                )));
            }
            engine
                .memory_store
                .mark_state(entry.target(), &entry.id, MemoryState::Deleted)?;
            // Not a correction under C11: the world changed, the user did not
            // correct what the review showed. It stays in the denominator
            // because it was recalled into the review.
        }

        let cancel = CancelToken::new();
        let mut on_resume_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token: &mut on_resume_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        resumed = Some(engine.chat_orchestrator.resume_after_context_decision(
            &review.id,
            ContextDecision::Send,
            "eval-harness",
            adapter,
            &mut sink,
        ));
    }

    if let Some(approved) = scenario.approval_decision
        && let Some(proposal) = &original_command_proposal
    {
        let cancel = CancelToken::new();
        let mut on_resume_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token: &mut on_resume_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        resumed = Some(engine.chat_orchestrator.resume_after_command_decision(
            &proposal.id,
            approved,
            "eval-harness",
            adapter,
            &mut sink,
        ));
    }

    // Spec 61 §5.8: a scenario that answers a round-budget pause resumes it with
    // the decision it names, so the continued turn (or the forced final call)
    // is exercised end to end. The pause is on the original outcome, which is
    // why the proposal is read before the resume can supersede it.
    if let Some(decision) = scenario.round_budget_decision
        && let Some(proposal) = outcome
            .as_ref()
            .ok()
            .and_then(|result| result.round_budget_proposal.as_ref())
    {
        let cancel = CancelToken::new();
        let mut on_resume_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token: &mut on_resume_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        let decision = match decision {
            ScenarioRoundBudgetDecision::Continue => RoundBudgetDecision::Continue,
            ScenarioRoundBudgetDecision::AnswerNow => RoundBudgetDecision::AnswerNow,
        };
        resumed = Some(engine.chat_orchestrator.resume_round_budget(
            &proposal.id,
            decision,
            "eval-harness",
            adapter,
            &mut sink,
        ));
    }

    // The resumed turn is the one that finished, so it supersedes the original.
    let mut outcome = resumed.unwrap_or(outcome);

    // Spec 50 §9: a scenario that scripts answers is interactive, so a turn may
    // pause on a question. Answer each one in order, through the engine's own
    // resume, until the turn finishes or asks something the script did not
    // anticipate.
    let mut answer_index = 0;
    loop {
        let question = outcome
            .as_ref()
            .ok()
            .and_then(|result| result.question_proposal.clone());
        let Some(question) = question else { break };
        let Some(answer) = scenario.answers.get(answer_index) else {
            return Err(ClientError::InvalidInput(format!(
                "{}: the model asked more questions than the scenario scripts: question {} was \
                 asked, but only {} answer(s) are scripted",
                scenario.name,
                answer_index + 1,
                scenario.answers.len()
            )));
        };
        answer_index += 1;
        // By option label when one matches — which is how a user clicking an
        // option answers — and otherwise as free text, which the engine then
        // accepts or refuses on the question's own terms.
        let selected_option = question.options.iter().position(|option| option == answer);

        let cancel = CancelToken::new();
        let mut on_resume_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token: &mut on_resume_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        outcome = engine.chat_orchestrator.resume_after_answer(
            &question.question_id,
            answer,
            selected_option,
            adapter,
            &mut sink,
        );
    }

    // Spec 65 §5.9: a scenario may run a second user turn in the same session.
    // The scripted responses continue where the first turn's script stopped, so
    // the follow-up is a real new turn that inherits the first turn's notes
    // rather than a replay of the same outcome.
    if let Some(prompt) = &scenario.follow_up_prompt
        && let Ok(first) = &outcome
    {
        let session_id = first.session.id.clone();
        let cancel = CancelToken::new();
        let mut on_follow_up_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token: &mut on_follow_up_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        outcome = engine.chat_orchestrator.ask_with_session_with_options(
            &materialized.root,
            prompt,
            &[],
            Some(&session_id),
            adapter,
            &mut sink,
            turn_options(scenario),
        );
    }

    if let Ok(result) = &outcome
        && let Some(usage) = result.usage
    {
        recorded_usage = Some(usage);
    }
    let duration_ms = now_millis().saturating_sub(started_at_ms);

    // The conflict scenario is the only one that applies a patch, and it does so
    // only after deliberately dirtying the file. Every other scenario asserts
    // that a proposal waits for a human, so applying by default would destroy
    // exactly the property they check.
    //
    // This runs before the trace is read: an apply emits `file_modified` and
    // `patch_applied`, and reading the trace first would miss them.
    let mut apply_error = None;
    if let Some((path, content)) = &scenario.modify_after_proposal
        && let Ok(result) = &outcome
        && let Some(proposal) = &result.patch_proposal
    {
        let target = materialized.root.join(path);
        std::fs::write(&target, content)
            .map_err(|error| ClientError::Io(format!("{}: {error}", target.display())))?;
        // `apply_stored_patch` rather than `PatchEngine::apply_patch`: it takes
        // the id, loads the proposal from the store and checkpoints first, which
        // is the same path the desktop app's apply route uses. No approved_paths
        // and no hunk selection means the whole proposal, which is what a user
        // clicking apply without touching the checkboxes does.
        if let Err(error) = engine.edit_orchestrator.apply_stored_patch(
            &materialized.root,
            &proposal.patch_id,
            None,
            None,
            "eval-harness",
            false,
        ) {
            apply_error = Some(format!("{error:?}"));
        }
    }

    // The patch the user reviewed, kept because the final outcome of a
    // verification scenario is the report turn, which proposes no patch.
    let applied_patch = outcome
        .as_ref()
        .ok()
        .and_then(|result| result.patch_proposal.clone());

    // Spec 23 Task 9: apply the proposed patch and drive the verification
    // continuation — the check card, any repair patch, and the final report —
    // exactly as the shell does after a patch settles.
    let mut verification = None;
    if scenario.verify_after_apply
        && let Ok(result) = &outcome
        && result.patch_proposal.is_some()
    {
        if let Some(patch) = result.patch_proposal.clone() {
            engine.edit_orchestrator.apply_stored_patch(
                &materialized.root,
                &patch.patch_id,
                None,
                None,
                "eval-harness",
                false,
            )?;
        }
        let session_id = result.session.id.clone();
        let mut current = None;
        for _ in 0..8 {
            let turn = match current.take() {
                Some(turn) => turn,
                None => {
                    let cancel = CancelToken::new();
                    let mut on_token = |_token: &str| {};
                    let mut on_progress = |_event: TurnProgress| {};
                    let mut sink = TurnSink {
                        on_token: &mut on_token,
                        on_progress: &mut on_progress,
                        cancel: &cancel,
                        steering: None,
                    };
                    engine.chat_orchestrator.verify_due(
                        &materialized.root,
                        &session_id,
                        adapter,
                        &mut sink,
                    )?
                }
            };
            if let Some(batch) = turn.check_batch.clone() {
                let decisions = batch
                    .checks
                    .iter()
                    .map(|check| {
                        let approved = scenario
                            .check_decisions
                            .iter()
                            .find(|decision| decision.command == check.command)
                            .is_some_and(|decision| decision.approved);
                        (check.id.clone(), approved)
                    })
                    .collect::<Vec<_>>();
                let cancel = CancelToken::new();
                let mut on_token = |_token: &str| {};
                let mut on_progress = |_event: TurnProgress| {};
                let mut sink = TurnSink {
                    on_token: &mut on_token,
                    on_progress: &mut on_progress,
                    cancel: &cancel,
                    steering: None,
                };
                current = Some(engine.chat_orchestrator.resume_after_check_decision(
                    &batch.id,
                    &decisions,
                    "eval-harness",
                    adapter,
                    &mut sink,
                )?);
                continue;
            }
            if let Some(patch) = turn.patch_proposal.clone() {
                engine.edit_orchestrator.apply_stored_patch(
                    &materialized.root,
                    &patch.patch_id,
                    None,
                    None,
                    "eval-harness",
                    false,
                )?;
                continue;
            }
            verification = turn.verification_report.clone();
            outcome = Ok(turn);
            break;
        }
        if verification.is_none() {
            return Err(ClientError::InvalidInput(format!(
                "{}: verification did not reach a report in 8 continuations",
                scenario.name
            )));
        }
    }

    // A scenario declaring a crash leaves the turn's task mid-action, then
    // reopens the store the way a restart does and classifies. Deliberately
    // before the trace is read: classification and the resume decision are both
    // audited, and that trail is what the assertions and §5.6's recovery row
    // read.
    let recovery = match &scenario.crash_mid_action {
        Some(crash) => Some(interrupt_and_recover(engine, materialized, crash)?),
        None => None,
    };

    let trace = Trace::read(&materialized.data_dir)?;
    // Read before the status match consumes `outcome`. `finalStatus` says what
    // the run did; this says what the task ended as, and the two differ on a
    // pause (`completed` vs `waiting_for_approval`).
    let final_task_status = outcome
        .as_ref()
        .ok()
        .map(|result| result.task.status.as_str().to_string())
        .unwrap_or_default();
    let mut run_record =
        RunRecord::new(&scenario.name, &materialized.version, tier, provider, model);
    run_record.recovery = recovery;
    run_record.started_at_ms = started_at_ms;
    run_record.duration_ms = duration_ms;
    run_record.continuations = trace.count("turn_budget_extended");
    run_record.task_count = count_tasks(engine)?;
    // Read from the engine both tiers actually ran with, so the record cannot
    // claim a capability set the run did not have.
    run_record.native_tools = engine.config.supports_native_tools();
    // From the trace rather than a `MockModelAdapter`'s recorded requests, so
    // both tiers count the same way. Verified equal to `adapter.requests.len()`
    // on the deterministic scenarios (9/9, 2/2, 2/2). Note that
    // `model_response_completed` is NOT a substitute: it fires once per turn,
    // not once per call.
    run_record.model_calls = trace.count("model_request_prepared");
    // From the session log for the same reason `tool_calls` is: counting the
    // script's turns described the TOML file, which in the live tier is not
    // even a description of the run. See `trace::tool_rounds`.
    run_record.tool_rounds = trace::tool_rounds(&materialized.data_dir)?;
    // Spec 59 §5.6: exploration rounds are derived from the same session log,
    // not from the script, so the figure describes what the run dispatched.
    run_record.exploration_rounds = trace::exploration_rounds(&materialized.data_dir)?;
    // Spec 65 requirement 11: the notes and repeat metrics read from the same
    // log, so they describe what the run dispatched rather than the script.
    run_record.notes_calls = trace::notes_calls(&materialized.data_dir)?;
    run_record.repeated_elided_reads = trace::repeated_elided_reads(&materialized.data_dir)?;
    run_record.carried_notes = trace::carried_notes(&materialized.data_dir)?;

    // filesChanged is the union of what the engine says it wrote — never a walk
    // of the working tree, which would also catch git's own bookkeeping.
    // `file_modified` gives one `resourcePath` per file; `patch_applied` gives a
    // comma-joined `files` list, so the two need different accessors.
    let mut files_changed = trace.paths_from("file_modified", "resourcePath");
    for path in trace.csv_from("patch_applied", "files") {
        if !files_changed.contains(&path) {
            files_changed.push(path);
        }
    }
    run_record.files_changed = files_changed;

    // Spec 30 Task 6: the audit is the source for what the model was actually
    // sent, so a `memory_recalled` event is the recall observation. C11's
    // correction half is narrower than "anything that changed": an edit or a
    // supersede is a correction of what was recalled; a plain state change or a
    // verification transition is not, and is left out.
    for event in trace.of_type("memory_recalled") {
        if let Some(id) = event.field("entryId") {
            recalled_ids.insert(id.to_string());
        }
    }
    for event_type in ["memory_edited", "memory_superseded"] {
        for event in trace.of_type(event_type) {
            if let Some(id) = event.field("entryId") {
                corrected_ids.insert(id.to_string());
            }
        }
    }

    // Spec 62 §5.5: the prompt's deciding cause is the first name in the
    // event's `causes` field, which `validation.rs::store_proposal` writes in
    // classifier order. Empty for a proposal that ran without asking, so
    // `None` there rather than a made-up cause.
    for event in trace.of_type("command_proposal_stored") {
        run_record.approvals.push(RecordedApproval {
            kind: "command".to_string(),
            decision: "requested".to_string(),
            cause: first_cause(event),
        });
    }
    for (event_type, decision) in [
        ("stored_command_executed", "approved"),
        ("stored_command_rejected", "denied"),
        ("command_allowlisted", "allow_always"),
    ] {
        for _ in 0..trace.count(event_type) {
            run_record.approvals.push(RecordedApproval {
                kind: "command".to_string(),
                decision: decision.to_string(),
                cause: None,
            });
        }
    }
    run_record.group_approvals_by_cause();

    // Checks are the commands the engine actually ran, with their exit status.
    // `command_executed` carries both `command` and `exitCode`
    // (`command_runner.rs:121`); `stored_command_executed` carries the exit code
    // but not the command, so it is not a substitute here.
    //
    // Without this, `check_pass_rate` is computed over an empty set and reports
    // 0.000 — which reads as "nothing passed" rather than "nothing ran".
    for event in trace.of_type("command_executed") {
        let Some(command) = event.field("command") else {
            continue;
        };
        run_record.checks.push(RecordedCheck {
            command: command.to_string(),
            passed: event.field("exitCode") == Some("0"),
        });
    }

    // A violation is a proposal that was both declined and executed: the engine
    // ran something a human said no to. Matched on `proposalId`, which is why
    // this is computed here and not in `metrics.rs` — the trace carries the id
    // and the record's `approvals` list does not.
    //
    // Limits worth knowing: this catches "executed despite denial", which is the
    // case the audit log can prove. It does not catch a side effect that
    // bypassed the proposal machinery altogether, since such an action would
    // leave no proposal to match against. An auto-executed low-risk command is
    // not a violation — `requires_approval` is false for those by policy.
    let rejected_ids: Vec<&str> = trace
        .of_type("stored_command_rejected")
        .iter()
        .filter_map(|event| event.field("proposalId"))
        .collect();
    run_record.approval_policy_violations = trace
        .of_type("stored_command_executed")
        .iter()
        .filter_map(|event| event.field("proposalId"))
        .filter(|executed| rejected_ids.contains(executed))
        .count() as u64;

    // What the run dispatched, from the engine's action markers — not from
    // `scenario.turns`. The script was the wrong source twice over: the live
    // tier ignores it, so a live record listed calls that never happened and
    // carried scripted payloads the model never sent; and a script cannot say
    // how a call ended, so every entry was stamped with whether the turn as a
    // whole survived. Spec 17's markers carry the engine's own per-call
    // outcome, which did not exist when this was first written.
    run_record.tool_calls = trace::tool_actions(&materialized.data_dir)?;
    run_record.findings_by_purpose = trace::findings_by_purpose(&trace, &materialized.data_dir)?;

    // Read before the match below consumes `outcome`. The plan comes from the
    // session log rather than being accumulated here — the same reason tokens
    // are read back rather than counted: an eval figure and a real session's
    // figure must not be able to drift.
    //
    // `None` when the turn planned nothing, which is most of the deterministic
    // tier: those scenarios are short enough that §5.1 says a plan would be
    // ceremony, and the metric reports `notApplicable` rather than a zero.
    let reminders_sent = trace.count("plan_reminder_sent");
    let left_open_at_end = trace.count("plan_left_open") > 0;
    run_record.plan = outcome
        .as_ref()
        .ok()
        .and_then(|result| {
            engine
                .session_store
                .read_task_plan(&result.session.id, &result.task.id)
                .ok()
                .flatten()
        })
        .map(|plan| {
            let report = plan.report();
            let open_step_evidence = plan
                .steps
                .iter()
                .filter(|step| !step.status.is_terminal())
                .map(|step| step.evidence.len() as u64)
                .sum();
            RecordedPlan {
                steps_planned: report.steps.len() as u64,
                steps_verified: report.verified as u64,
                steps_unverified: report.unverified as u64,
                steps_blocked: report.blocked as u64,
                steps_skipped: report.skipped as u64,
                steps_outstanding: report.outstanding as u64,
                reminders_sent,
                left_open_at_end,
                open_step_evidence,
            }
        });

    // Read before `outcome` is consumed below: the session log is where the
    // post-reminder reply lives, and `result.response` is deliberately the
    // earlier answer on a reminded turn (spec 66 `context.md` D4).
    let last_assistant_message = outcome.as_ref().ok().and_then(|result| {
        engine
            .session_store
            .read_messages(&result.session.id)
            .ok()
            .and_then(|messages| {
                messages
                    .iter()
                    .rev()
                    .find(|message| message.role == "assistant")
                    .map(|message| message.content.clone())
            })
    });

    let (final_status, response, context_files, command_proposal, patch_proposal) = match outcome {
        Ok(result) => (
            if result.cancelled {
                "cancelled"
            } else {
                "completed"
            }
            .to_string(),
            result.response,
            result.context_files,
            // Fall back to the proposal that stopped the original turn: a
            // resumed turn has already consumed it, and `approval_required`
            // would otherwise read as false on every scenario that resumes.
            result
                .command_proposal
                .or(original_command_proposal.clone()),
            result.patch_proposal,
        ),
        Err(ClientError::ApprovalRequired(_)) => (
            "awaiting_approval".to_string(),
            String::new(),
            Vec::new(),
            original_command_proposal.clone(),
            None,
        ),
        // A refusal is a first-class outcome, not a harness failure: three
        // scenarios exist precisely to assert one happened.
        Err(ClientError::AccessDenied(message)) => {
            ("refused".to_string(), message, Vec::new(), None, None)
        }
        Err(ClientError::PolicyBlocked(message)) => {
            ("blocked".to_string(), message, Vec::new(), None, None)
        }
        Err(ClientError::PatchConflict(message)) => {
            ("conflict".to_string(), message, Vec::new(), None, None)
        }
        Err(error) => (
            "failed".to_string(),
            format!("{error:?}"),
            Vec::new(),
            None,
            None,
        ),
    };
    run_record.final_status = final_status;
    run_record.final_task_status = final_task_status;
    // Spec 19's per-task accounting is the one definition of what a task
    // spent, and `ChatTurnResult.usage` is that figure read back from the
    // session log. The harness reports it rather than counting model calls
    // itself, so an eval figure and a real session's figure cannot drift.
    //
    // A resumed turn re-reads the same task, so its total already includes the
    // rounds before the approval; the resumed result therefore wins when there
    // is one.
    run_record.tokens = Tokens {
        input: recorded_usage.map(|usage| usage.input_tokens).unwrap_or(0),
        output: recorded_usage.map(|usage| usage.output_tokens).unwrap_or(0),
        measured: recorded_usage
            .map(|usage| usage.source == UsageSource::Measured)
            .unwrap_or(false),
    };
    run_record.cost = recorded_usage.and_then(|usage| usage.reported_cost);
    // Spec 49 §5.6: read through the same `TaskUsage`, not a second
    // aggregation. `cached_input_tokens` is `None` when no run in the task
    // reported a split, which `and_then` carries straight into `record.cache`.
    run_record.cache = recorded_usage.and_then(|usage| {
        usage.cached_input_tokens.map(|cached| CacheUsage {
            cached_input_tokens: cached,
            cache_reported_input_tokens: usage.cache_reported_input_tokens,
        })
    });
    run_record.sanitize(scanner);
    run_record.memory = memory_counts(
        &materialized.data_dir,
        &materialized.root,
        &recalled_ids,
        &corrected_ids,
        &response,
        &run_record.files_changed,
    );

    // Read from the log rather than from the scripted replies: a scenario that
    // claims a compaction happened is checked against what the engine wrote.
    let conversation_compactions = match seeded_session.as_deref() {
        Some(session_id) => engine
            .session_store
            .read_conversation_compactions(session_id)
            .map(|summaries| summaries.len() as u64)
            .unwrap_or(0),
        None => 0,
    };
    let compaction_facts_only = seeded_session
        .as_deref()
        .and_then(|session_id| {
            engine
                .session_store
                .read_conversation_compactions(session_id)
                .ok()
        })
        .and_then(|summaries| summaries.last().map(|summary| summary.is_facts_only()))
        .unwrap_or(false);
    Ok(Run {
        record: run_record,
        repo_root: materialized.root.clone(),
        data_dir: materialized.data_dir.clone(),
        response,
        context_files,
        command_proposal,
        patch_proposal: patch_proposal.or(applied_patch),
        apply_error,
        // Filled by `run` after the deterministic adapter's requests are
        // available; the live tier has no request record to read.
        continuation_request: None,
        answer_request: None,
        verification,
        last_assistant_message,
        compacted_request: None,
        compacted_system_prompt: None,
        conversation_compactions,
        compaction_facts_only,
        stub_requests: 0,
        request: None,
        system_prompt: None,
        trace,
    })
}

/// Leaves the turn's task mid-action, restarts, and classifies — §5.4's resume
/// row, and the reason spec 17 was sequenced ahead of specs 45 and 46.
///
/// Two properties are measured, matching what §5.6 asks of this row: that a
/// session killed mid-task **classifies**, and that it is **not auto-retried**.
/// The second is checked by actually asking the engine to resume and recording
/// the refusal — requirement 5's enforcement point is `recovery::resume`, so
/// reading `auto_resume_permitted` alone would test the classifier's opinion
/// rather than the guarantee.
fn interrupt_and_recover(
    engine: &WorkspaceEngine,
    materialized: &Materialized,
    crash: &CrashMidAction,
) -> Result<RecordedRecovery> {
    let sessions = engine.session_store.list_sessions(None)?;
    let [session] = sessions.as_slice() else {
        return Err(ClientError::InvalidInput(format!(
            "crash_mid_action expects the turn to have created exactly one session, found {}",
            sessions.len()
        )));
    };
    let statuses = engine.session_store.read_task_statuses(&session.id)?;
    let task_ids: Vec<&String> = statuses.keys().collect();
    let [task_id] = task_ids.as_slice() else {
        return Err(ClientError::InvalidInput(format!(
            "crash_mid_action expects exactly one task, found {}",
            statuses.len()
        )));
    };

    // The status matters as much as the marker: a crash *after* the approval and
    // *during* the command leaves `running_tool`, not `waiting_for_approval`,
    // which §5.4 rule 3 would treat as a task merely awaiting a human.
    let task = engine.session_store.update_task_status(
        &Task {
            id: (*task_id).clone(),
            session_id: session.id.clone(),
            status: TaskStatus::PreparingContext,
            user_prompt: String::new(),
            model_provider: String::new(),
            model_name: String::new(),
            created_at_ms: 0,
            completed_at_ms: None,
        },
        TaskStatus::RunningTool,
        None,
    )?;
    // Started and never finished. `ActionMarker` has no `Drop` impl by design,
    // so letting it fall out of scope leaves the action open — which is the
    // signature being injected.
    let _marker = engine.session_store.start_action(
        &task,
        &crash.action,
        &crash.reference,
        crash.side_effecting,
    )?;

    // The restart. A *fresh* `SessionStore` rather than `engine.session_store`,
    // because that one carries the in-memory sequence cache a new process would
    // not have — reusing it would let this pass on state a real restart lacks.
    let restarted = SessionStore::new(&materialized.data_dir);
    let recovered = classify_session(&restarted, &engine.audit_log, &session.id)?;
    let [task] = recovered.as_slice() else {
        return Err(ClientError::InvalidInput(format!(
            "the interrupted task should be the one recovered, got {recovered:?}"
        )));
    };

    // Asked for real, not predicted from `auto_resume_permitted`.
    let resume_refused = match resume(&restarted, &engine.audit_log, task) {
        Ok(()) => false,
        Err(ClientError::PolicyBlocked(_)) => true,
        Err(error) => return Err(error),
    };

    Ok(RecordedRecovery {
        interrupted_action: task
            .dangling
            .as_ref()
            .map(|action| action.action.clone())
            .unwrap_or_default(),
        classification: task.classification.as_str().to_string(),
        auto_resume_permitted: task.auto_resume_permitted,
        resume_refused,
    })
}

/// Every task the run's store holds, across every session.
///
/// A continued turn reuses its session and task, so this reads 1. A path that
/// started a new turn — the old *Continue debugging* behaviour, or a resume
/// entry point that created a task — reads more, which is the fact spec 61's
/// `no second task` assertion needs.
fn count_tasks(engine: &WorkspaceEngine) -> Result<u64> {
    let mut count = 0;
    for session in engine.session_store.list_sessions(None)? {
        count += engine.session_store.read_task_statuses(&session.id)?.len() as u64;
    }
    Ok(count)
}

/// The first cause name on a `command_proposal_stored` event, or `None` when
/// the proposal needed no approval (spec 62 §5.4–5.5).
///
/// The event joins the names with commas and always carries the field, empty
/// for a command that runs without asking, so an empty string means "no cause"
/// rather than a cause with a blank name.
fn first_cause(event: &trace::Event) -> Option<String> {
    event
        .field("causes")?
        .split(',')
        .find(|cause| !cause.is_empty())
        .map(str::to_string)
}

/// Turns scenario turns into the three parallel vectors `MockModelAdapter` wants.
fn script(scenario: &Scenario) -> (Vec<String>, Vec<Vec<ToolCall>>, Vec<bool>) {
    let mut responses = Vec::new();
    let mut tool_calls = Vec::new();
    let mut truncated = Vec::new();
    for (index, turn) in scenario.turns.iter().enumerate() {
        responses.push(turn.content.clone());
        truncated.push(turn.truncated);
        tool_calls.push(
            turn.tool_calls
                .iter()
                .enumerate()
                .map(|(position, call)| ToolCall {
                    id: format!("call_{index}_{position}"),
                    name: call.name.clone(),
                    arguments_json: call.arguments.to_string(),
                })
                .collect(),
        );
    }
    if responses.is_empty() {
        // A scenario with no script still needs one response, or the adapter
        // returns its last (nonexistent) entry.
        responses.push(String::new());
        tool_calls.push(Vec::new());
        truncated.push(false);
    }
    (responses, tool_calls, truncated)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}
