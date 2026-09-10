use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    AgentCommandProposal, AgentPatchProposal, CancelToken, ClientError, Config, CurlModelTransport,
    MockModelAdapter, ModelAdapter, ModelProviderConfig, OpenAICompatibleAdapter, Result,
    SecretScanner, SessionStore, Task, TaskStatus, ToolCall, TurnProgress, TurnSink, UsageSource,
    WorkspaceEngine, classify_session, resume,
};

use crate::fixture::{self, Materialized};
use crate::record::{
    RecordedApproval, RecordedCheck, RecordedRecovery, RecordedToolCall, RunRecord, Tokens,
};
use crate::scenario::{CrashMidAction, Scenario, Tier};
use crate::trace::Trace;

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
    config.model_provider = "mock".to_string();
    config.model_name = "mock".to_string();
    config.model_providers.push(mock_provider());
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let engine = WorkspaceEngine::new(config);

    let (responses, tool_calls, truncated) = script(scenario);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, tool_calls)
        .with_truncated(truncated);

    drive(
        scenario,
        &materialized,
        &engine,
        &scanner,
        &mut adapter,
        "deterministic",
        "mock",
        "mock",
    )
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
    let mut config = base_config(&materialized);
    config.model_provider = provider.clone();
    config.model_name = model.clone();
    // Fills `model_base_url` and `model_api_key_env` from the provider entry,
    // which is what the CLI and the desktop shell both rely on before building
    // a transport.
    config.apply_model_provider_defaults();

    let api_key = std::env::var(&config.model_api_key_env).map_err(|_| {
        ClientError::InvalidInput(format!(
            "{} is required for the live tier",
            config.model_api_key_env
        ))
    })?;
    let transport = CurlModelTransport::new(&config.model_base_url, api_key);
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
        trace: Trace::default(),
    })
}

fn base_config(materialized: &Materialized) -> Config {
    Config {
        data_dir: materialized.data_dir.clone(),
        ..Config::default()
    }
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
    let outcome = engine.chat_orchestrator.ask(
        &materialized.root,
        &scenario.prompt,
        &[],
        adapter,
        &mut on_token,
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
    // proposal. `resume_after_command_decision` takes a `TurnSink`, unlike `ask`.
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
        };
        resumed = Some(engine.chat_orchestrator.resume_after_command_decision(
            &proposal.id,
            approved,
            "eval-harness",
            adapter,
            &mut sink,
        ));
    }

    // The resumed turn is the one that finished, so it supersedes the original.
    let outcome = resumed.unwrap_or(outcome);
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
    let mut run_record =
        RunRecord::new(&scenario.name, &materialized.version, tier, provider, model);
    run_record.recovery = recovery;
    run_record.started_at_ms = started_at_ms;
    run_record.duration_ms = duration_ms;
    // From the trace rather than a `MockModelAdapter`'s recorded requests, so
    // both tiers count the same way. Verified equal to `adapter.requests.len()`
    // on the deterministic scenarios (9/9, 2/2, 2/2). Note that
    // `model_response_completed` is NOT a substitute: it fires once per turn,
    // not once per call.
    run_record.model_calls = trace.count("model_request_prepared");
    run_record.tool_rounds = scenario
        .turns
        .iter()
        .filter(|turn| !turn.tool_calls.is_empty())
        .count() as u64;

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

    for _ in 0..trace.count("command_proposal_stored") {
        run_record.approvals.push(RecordedApproval {
            kind: "command".to_string(),
            decision: "requested".to_string(),
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
            });
        }
    }

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

    // Tool-call outcomes: scripted name, outcome from whether the turn survived.
    // A finer-grained per-call outcome would need an engine-side event that does
    // not exist; recording the turn's outcome is honest, and the tool-error
    // metric in §5.6 reads it. The live tier has no script, so it records none.
    let turn_ok = outcome.is_ok();
    for turn in &scenario.turns {
        for call in &turn.tool_calls {
            run_record.tool_calls.push(RecordedToolCall {
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                outcome: if turn_ok { "ok" } else { "error" }.to_string(),
            });
        }
    }

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
