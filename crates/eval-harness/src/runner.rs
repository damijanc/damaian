use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    AgentCommandProposal, AgentPatchProposal, CancelToken, ClientError, Config, MockModelAdapter,
    ModelProviderConfig, Result, SecretScanner, ToolCall, TurnProgress, TurnSink, WorkspaceEngine,
};

use crate::fixture;
use crate::record::{RecordedApproval, RecordedCheck, RecordedToolCall, RunRecord, Tokens};
use crate::scenario::{Scenario, Tier};
use crate::trace::Trace;

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
    }
}

pub fn run(scenario: &Scenario) -> Result<Run> {
    if scenario.tier != Tier::Deterministic {
        return Err(ClientError::InvalidInput(format!(
            "{}: runner::run drives the deterministic tier; the live tier has its own entry point",
            scenario.name
        )));
    }

    // A blocked scenario is skipped before anything is materialized: it measures
    // a capability that does not exist, so running it would either fail for the
    // wrong reason or pass without measuring anything.
    if let Some(blocking) = &scenario.blocked_on {
        let mut skipped = RunRecord::new(
            &scenario.name,
            "n/a",
            scenario.tier.as_str(),
            "none",
            "none",
        );
        skipped.final_status = "not_applicable".to_string();
        skipped.not_applicable = Some(blocking.clone());
        return Ok(Run {
            record: skipped,
            repo_root: PathBuf::new(),
            data_dir: PathBuf::new(),
            response: String::new(),
            context_files: Vec::new(),
            command_proposal: None,
            patch_proposal: None,
            apply_error: None,
            trace: Trace::default(),
        });
    }

    let materialized = fixture::materialize(&scenario.fixture)?;
    let mut config = Config {
        data_dir: materialized.data_dir.clone(),
        model_provider: "mock".to_string(),
        model_name: "mock".to_string(),
        ..Config::default()
    };
    config.model_providers.push(mock_provider());
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let engine = WorkspaceEngine::new(config);

    let (responses, tool_calls, truncated) = script(scenario);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, tool_calls)
        .with_truncated(truncated);
    let mut on_token = |_token: &str| {};

    let started_at_ms = now_millis();
    let outcome = engine.chat_orchestrator.ask(
        &materialized.root,
        &scenario.prompt,
        &[],
        &mut adapter,
        &mut on_token,
    );
    // Captured before any resume: the resumed turn does not carry the proposal
    // that stopped the original one, and `approval_required` asserts on it.
    let original_command_proposal = outcome
        .as_ref()
        .ok()
        .and_then(|result| result.command_proposal.clone());

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
            &mut adapter,
            &mut sink,
        ));
    }

    // The resumed turn is the one that finished, so it supersedes the original.
    let outcome = resumed.unwrap_or(outcome);
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

    let trace = Trace::read(&materialized.data_dir)?;
    let mut run_record = RunRecord::new(
        &scenario.name,
        &materialized.version,
        "deterministic",
        "mock",
        "mock",
    );
    run_record.started_at_ms = started_at_ms;
    run_record.duration_ms = duration_ms;
    run_record.model_calls = adapter.requests.len() as u64;
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
    // metric in §5.6 reads it.
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
            None,
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
    run_record.tokens = Tokens {
        input: 0,
        output: 0,
        measured: false,
    };
    run_record.cost = None;
    run_record.sanitize(&scanner);

    Ok(Run {
        record: run_record,
        repo_root: materialized.root,
        data_dir: materialized.data_dir,
        response,
        context_files,
        command_proposal,
        patch_proposal,
        apply_error,
        trace,
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
