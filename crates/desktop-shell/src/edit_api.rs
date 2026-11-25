//! Patch and command routes: propose, apply, roll back and reject.

use super::*;

pub(crate) fn handle_propose_edit(stream: &mut dyn Write, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let prompt = required_form(&form, "prompt")?;
    // The chat session the prompt was typed in, so its mode governs
    // this flow too. Absent for a caller with no session open.
    let session_id = form
        .get("session_id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let (config, report) = config_for_repo_with_model_options(&repo, &form)?;
    let engine = WorkspaceEngine::new_with_report(config, &report);
    let context_files = form
        .get("context_files")
        .map(|value| validate_context_files(&engine, &repo, value))
        .transpose()?
        .unwrap_or_default();
    let api_key = resolve_shell_model_api_key(&engine.config)?;
    let transport = CurlModelTransport::new(
        &engine.config.model_base_url,
        api_key,
        ProcessRegistry::open(&engine.config.data_dir).map_err(|error| error.to_string())?,
        &engine.config.data_dir,
    );
    let mut adapter = OpenAICompatibleAdapter::with_provider(
        &engine.config.model_provider,
        &engine.config.model_name,
        transport,
    );
    let result = engine
        .edit_orchestrator
        .propose_edit(
            &repo,
            &prompt,
            &context_files,
            session_id.as_deref(),
            &mut adapter,
        )
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"patchId\":\"{}\",\"summary\":\"{}\",\"diff\":\"{}\",\"files\":[{}],\"contextFiles\":[{}]}}",
            escape_json(&result.patch.id),
            escape_json(&result.patch.summary),
            escape_json(&patch_diff_text(&result.patch)),
            patch_files_json(&result.patch.files),
            json_string_array(&result.context_files)
        ),
    )
}

pub(crate) fn handle_apply_patch(stream: &mut dyn Write, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let patch_id = required_form(&form, "patch_id")?;
    let approved_paths = form
        .get("paths")
        .map(|value| parse_path_list(value))
        .transpose()?;
    let hunk_selection = form
        .get("hunk_selection")
        .map(|value| parse_hunk_selection(value).map_err(|error| error.to_string()))
        .transpose()?;
    // Explicit per-apply user decision, sent only after the UI has
    // shown what the scanner found. Absent on the first attempt.
    let allow_generated_secrets = form
        .get("allow_secrets")
        .is_some_and(|value| value == "1" || value == "true");
    let engine = engine_for_repo(&repo)?;
    // Without the override, report what would be blocked instead of
    // failing: the user needs to see which file tripped the check to
    // decide whether to accept it.
    if !allow_generated_secrets && engine.config.block_generated_secrets {
        let flagged = engine
            .edit_orchestrator
            .preview_stored_patch_secrets(
                &repo,
                &patch_id,
                approved_paths.as_deref(),
                hunk_selection.as_ref(),
            )
            .map_err(|error| error.to_string())?;
        if !flagged.is_empty() {
            return write_response(
                stream,
                request,
                200,
                "application/json",
                &format!(
                    "{{\"patchId\":\"{}\",\"appliedFiles\":[],\"warningCount\":{},\"blockedBySecrets\":[{}]}}",
                    escape_json(&patch_id),
                    flagged.len(),
                    generated_secret_warnings_json(&flagged)
                ),
            );
        }
    }
    let result = engine
        .edit_orchestrator
        .apply_stored_patch(
            &repo,
            &patch_id,
            approved_paths.as_deref(),
            hunk_selection.as_ref(),
            "desktop_user",
            allow_generated_secrets,
        )
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"patchId\":\"{}\",\"appliedFiles\":[{}],\"warningCount\":{},\"hookWarnings\":[{}]}}",
            escape_json(&result.patch_id),
            json_string_array(&result.applied_files),
            result.warnings.len(),
            json_string_array(&result.hook_warnings)
        ),
    )
}

pub(crate) fn handle_rollback_patch(
    stream: &mut dyn Write,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let patch_id = required_form(&form, "patch_id")?;
    let selected_paths = form
        .get("paths")
        .map(|value| parse_path_list(value))
        .transpose()?;
    let engine = engine_for_repo(&repo)?;
    let result = engine
        .edit_orchestrator
        .rollback_stored_patch(&repo, &patch_id, selected_paths.as_deref(), "desktop_user")
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"patchId\":\"{}\",\"restoredFiles\":[{}],\"deletedFiles\":[{}],\"warnings\":[{}]}}",
            escape_json(&result.patch_id),
            json_string_array(&result.restored_files),
            json_string_array(&result.deleted_files),
            json_string_array(&result.warnings)
        ),
    )
}

pub(crate) fn handle_reject_patch_files(
    stream: &mut dyn Write,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let patch_id = required_form(&form, "patch_id")?;
    let paths = parse_path_list(&required_form(&form, "paths")?)?;
    let engine = engine_for_repo(&repo)?;
    let path = engine
        .edit_orchestrator
        .reject_stored_patch_files(&patch_id, &paths, "desktop_user")
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"patchId\":\"{}\",\"rejectedFiles\":[{}],\"path\":\"{}\"}}",
            escape_json(&patch_id),
            json_string_array(&paths),
            escape_json(&path.to_string_lossy())
        ),
    )
}

pub(crate) fn handle_reject_patch(stream: &mut dyn Write, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let patch_id = required_form(&form, "patch_id")?;
    let engine = engine_for_repo(form.get("repo").map(String::as_str).unwrap_or_default())?;
    let path = engine
        .edit_orchestrator
        .reject_stored_patch(&patch_id, "desktop_user")
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"patchId\":\"{}\",\"status\":\"rejected\",\"path\":\"{}\"}}",
            escape_json(&patch_id),
            escape_json(&path.to_string_lossy())
        ),
    )
}

pub(crate) fn handle_propose_command(
    stream: &mut dyn Write,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let command = required_form(&form, "command")?;
    let engine = engine_for_repo(&repo)?;
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, &command, "Desktop command proposal")
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"proposalId\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{},\"cause\":\"{}\",\"reason\":\"{}\",\"allowAlways\":{},\"allowBrowserDiagnosticsForSession\":false}}",
            escape_json(&proposal.id),
            escape_json(&command_approval_prompt(&proposal)),
            proposal.risk.as_str(),
            proposal.requires_approval,
            proposal.blocked,
            escape_json(proposal.reasons.first().map(String::as_str).unwrap_or("")),
            escape_json(&proposal.reason),
            allow_always_eligible(&engine.config, &proposal.command, proposal.blocked)
        ),
    )
}

pub(crate) fn handle_run_command(stream: &mut dyn Write, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let proposal_id = required_form(&form, "proposal_id")?;
    let repo = form.get("repo").map(String::as_str).unwrap_or_default();
    let (config, report) = config_for_repo_with_model_options(repo, &form)?;
    let mut engine = WorkspaceEngine::new_with_report(config, &report);

    // Persist the permanent allowance *before* running. If the write
    // fails the command must not run either: silently downgrading
    // "allow always" to a one-time approval would leave the user
    // believing they'd never be asked again.
    let allowlist_path = if form.get("always").map(String::as_str) == Some("true") {
        Some(
            engine
                .validation_orchestrator
                .allow_command_always(&proposal_id, "desktop_user")
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };

    if engine
        .chat_orchestrator
        .has_pending_chat_command(&proposal_id)
    {
        // This proposal paused a chat turn awaiting approval — run
        // the command and feed the result back to the model so it
        // can actually answer, instead of just executing it in
        // isolation and leaving the user without a synthesized
        // response.
        let result = resume_chat_command(
            &mut engine,
            &proposal_id,
            true,
            resume_decision_options(&form),
        )?;
        write_response(
            stream,
            request,
            200,
            "application/json",
            &chat_result_json(&result),
        )
    } else {
        let record = {
            // A command run from this endpoint has no turn behind it and
            // no progress to stream, but it can still run for minutes, so
            // it is stoppable (spec 109 D8): the window names its session,
            // which holds the lane this token is set on.
            let cancel = CancelToken::new();
            lanes::with_current(|lane| lane.set_cancel(&cancel));
            let mut on_output = |_line: &str| {};
            engine.validation_orchestrator.run_proposal(
                &proposal_id,
                true,
                "desktop_user",
                None,
                &cancel,
                &mut on_output,
            )
        }
        .map_err(|error| error.to_string())?;
        write_response(
            stream,
            request,
            200,
            "application/json",
            &format!(
                "{{\"proposalId\":\"{}\",\"commandId\":\"{}\",\"exitCode\":{},\"termination\":\"{}\",\"stdout\":\"{}\",\"stderr\":\"{}\",\"allowlistPath\":{}}}",
                escape_json(&record.proposal_id),
                escape_json(&record.execution.id),
                record.execution.exit_code.unwrap_or(-1),
                // Spec 109 D8: `-1` is any run without an exit code; the card
                // tells a stopped run from a timed-out one by this.
                record.execution.termination.as_str(),
                escape_json(&record.execution.stdout),
                escape_json(&record.execution.stderr),
                json_optional_string(allowlist_path.as_deref())
            ),
        )
    }
}

pub(crate) fn handle_reject_command(
    stream: &mut dyn Write,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let proposal_id = required_form(&form, "proposal_id")?;
    let repo = form.get("repo").map(String::as_str).unwrap_or_default();
    let (config, report) = config_for_repo_with_model_options(repo, &form)?;
    let mut engine = WorkspaceEngine::new_with_report(config, &report);

    if engine
        .chat_orchestrator
        .has_pending_chat_command(&proposal_id)
    {
        let result = resume_chat_command(
            &mut engine,
            &proposal_id,
            false,
            ResumeDecisionOptions::default(),
        )?;
        write_response(
            stream,
            request,
            200,
            "application/json",
            &chat_result_json(&result),
        )
    } else {
        let path = engine
            .validation_orchestrator
            .reject_proposal(&proposal_id, "desktop_user")
            .map_err(|error| error.to_string())?;
        write_response(
            stream,
            request,
            200,
            "application/json",
            &format!(
                "{{\"proposalId\":\"{}\",\"status\":\"rejected\",\"path\":\"{}\"}}",
                escape_json(&proposal_id),
                escape_json(&path.to_string_lossy())
            ),
        )
    }
}
