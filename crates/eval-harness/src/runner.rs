use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    AgentCommandProposal, AgentPatchProposal, CancelToken, ChatTurnOptions, ClientError, Config,
    CurlModelTransport, MockModelAdapter, ModelAdapter, ModelProviderConfig, ModelRequest,
    OpenAICompatibleAdapter, ProcessRegistry, Result, RoundBudgetDecision, SecretScanner,
    SessionStore, Task, TaskStatus, ToolCall, TurnProgress, TurnSink, UsageSource, WorkspaceEngine,
    classify_session, resume,
};

use crate::fixture::{self, Materialized};
use crate::record::{
    CacheUsage, RecordedApproval, RecordedCheck, RecordedPlan, RecordedRecovery, RunRecord, Tokens,
};
use crate::scenario::{
    CrashMidAction, RoundBudgetDecision as ScenarioRoundBudgetDecision, Scenario, Tier,
};
use crate::trace::{self, Trace};

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

    let materialized = fixture::materialize(&scenario.fixture)?;
    let mut config = base_config(&materialized);
    apply_scenario_config(&mut config, scenario);
    config.model_provider = "mock".to_string();
    config.model_name = "mock".to_string();
    config.model_providers.push(mock_provider());
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let engine = WorkspaceEngine::new(config);

    let (responses, tool_calls, truncated) = script(scenario);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, tool_calls)
        .with_truncated(truncated);

    let mut run = drive(
        scenario,
        &materialized,
        &engine,
        &scanner,
        &mut adapter,
        "deterministic",
        "mock",
        "mock",
    )?;
    // The adapter is the only place the tier can see what a request actually
    // contained, and the resume happens inside `drive`. Read it here, after the
    // run, and locate the first post-continuation request from the trace: the
    // `turn_budget_extended` record is written before the resumed loop makes
    // its next call, so the calls before it are the pre-pause ones.
    run.continuation_request = continuation_request(&run.trace, &adapter.requests);
    run.answer_request = answer_request(&run.trace, &adapter.requests);
    Ok(run)
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

    let materialized = fixture::materialize(&scenario.fixture)?;
    let mut config = live_config(&materialized.data_dir, &provider, &model);
    apply_scenario_config(&mut config, scenario);

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
    let engine = WorkspaceEngine::new(config);

    drive(
        scenario,
        &materialized,
        &engine,
        &scanner,
        &mut adapter,
        "live",
        &provider,
        &model,
    )
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
        // `max_tokens` and budgets the default 16,000-token context.
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
/// the system prompt carries no headless paragraph. Everything else keeps the
/// defaults, which is headless: the tool is absent and the prompt tells the
/// model to proceed under a stated assumption (spec 50 `context.md` §4, §9).
fn turn_options(scenario: &Scenario) -> ChatTurnOptions {
    ChatTurnOptions {
        user_present: !scenario.answers.is_empty(),
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
    let outcome = engine.chat_orchestrator.ask_with_session_with_options(
        &materialized.root,
        &scenario.prompt,
        &[],
        None,
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
            RecordedPlan {
                steps_planned: report.steps.len() as u64,
                steps_verified: report.verified as u64,
                steps_unverified: report.unverified as u64,
                steps_blocked: report.blocked as u64,
                steps_skipped: report.skipped as u64,
                steps_outstanding: report.outstanding as u64,
            }
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

    Ok(Run {
        record: run_record,
        repo_root: materialized.root.clone(),
        data_dir: materialized.data_dir.clone(),
        response,
        context_files,
        command_proposal,
        patch_proposal,
        apply_error,
        // Filled by `run` after the deterministic adapter's requests are
        // available; the live tier has no request record to read.
        continuation_request: None,
        answer_request: None,
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
