use super::*;

/// The user-facing first line is the policy's own deciding reason (spec 62
/// §5.2). `reasons` is rendered from the classification's causes, so this
/// leads with the same words the card's **Why this command** shows.
pub(super) fn command_proposal_response(proposal: &CommandProposal) -> String {
    let cause = proposal.reasons.first().map(String::as_str);
    if proposal.blocked {
        format!(
            "{}\n`{}`\nDamaian will not run it. Nothing has run.",
            cause.unwrap_or("Blocked by local policy"),
            proposal.command
        )
    } else {
        match cause {
            Some(cause) => format!("{cause}\n`{}`", proposal.command),
            // A proposal always carries a cause, but never print an empty first
            // line if one is ever missing.
            None => format!("Damaian asks before running this.\n`{}`", proposal.command),
        }
    }
}

pub(super) fn agent_command_proposal(
    config: &Config,
    proposal: &CommandProposal,
) -> AgentCommandProposal {
    let session_refusal = crate::command_policy::session_approval_refusal(
        config,
        &proposal.command,
        &proposal.causes,
        proposal.blocked,
        proposal.requires_approval,
        Path::new(&proposal.repository_root),
        Path::new(&proposal.working_directory),
    );
    AgentCommandProposal {
        id: proposal.id.clone(),
        command: proposal.command.clone(),
        prompt: command_approval_prompt(proposal),
        risk: proposal.risk.as_str().to_string(),
        requires_approval: proposal.requires_approval,
        blocked: proposal.blocked,
        // `reasons` is rendered from the classification's causes, so the first
        // entry is the deciding cause's plain words. Kept as the rendered string
        // rather than read from `causes` so a proposal stored before spec 62
        // (which has reasons but no causes) still shows its cause on reattach.
        cause: proposal.reasons.first().cloned().unwrap_or_default(),
        reason: proposal.reason.clone(),
        allow_always: allow_always_eligible(config, &proposal.command, proposal.blocked),
        allow_browser_diagnostics_for_session: false,
        session_approvable: session_refusal.is_none(),
        session_approval_refusal: session_refusal.unwrap_or_default(),
        directory: Path::new(&proposal.working_directory)
            .strip_prefix(&proposal.repository_root)
            .map(|relative| relative.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

pub(super) fn normalize_tool_arguments(arguments_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments_json)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| arguments_json.trim().to_string())
}

pub(super) fn model_output_requests_tool(model_run: &ModelRun, content: &str) -> bool {
    !model_run.tool_calls.is_empty()
        || parse_command_request(content).is_some()
        || content.contains("DAMAIAN_COMMAND_V1")
        || content.contains("tool_calls")
        || content.contains("DSML")
}

/// The one sentence a hook denial reaches the user with, always naming the
/// hook so it is distinguishable from a mode or profile refusal (spec 32
/// `context.md` §10).
pub(super) fn hook_refusal(outcome: &HookOutcome) -> String {
    let hook_id = outcome.deciding_hook.as_deref().unwrap_or("unknown");
    match outcome
        .reasons
        .iter()
        .find(|(id, _)| id == hook_id)
        .map(|(_, reason)| reason.as_str())
    {
        Some(reason) => format!("Refused by hook `{hook_id}`: {reason}"),
        None => format!("Refused by hook `{hook_id}`"),
    }
}

pub(super) fn patch_proposal_response(patch: &ProposedPatch) -> String {
    format!(
        "I've prepared a patch (`{}`) for {} file{}. Review the diff and apply or reject it when ready.",
        patch.id,
        patch.files.len(),
        if patch.files.len() == 1 { "" } else { "s" }
    )
}

pub(super) fn agent_patch_proposal(patch: &ProposedPatch) -> AgentPatchProposal {
    AgentPatchProposal {
        patch_id: patch.id.clone(),
        summary: patch.summary.clone(),
        files: patch.files.clone(),
        repair_scope: patch.repair_scope.clone(),
    }
}

pub(super) fn format_search_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No matching files found.".to_string();
    }
    results
        .iter()
        .map(|result| {
            // The snippet is only the chunk's first 500 characters; the line
            // range is what lets the model read the rest of the match.
            let lines = result
                .lines
                .map(|lines| format!(" lines {}-{}", lines.start, lines.end))
                .unwrap_or_default();
            format!(
                "{}{lines} (score {})\n{}",
                result.path,
                result.score,
                truncate_for_prompt(&result.snippet, 500)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn format_git_status(status: &GitStatus) -> String {
    if status.clean {
        "Working tree clean.".to_string()
    } else {
        let mut listed = status
            .files
            .iter()
            .map(|file| file.raw.clone())
            .collect::<Vec<_>>()
            .join("\n");
        if status.omitted > 0 {
            // Spec 47 §5.5: a cut listing says so, or it reads as complete.
            listed.push_str(&format!(
                "\n[Status truncated: {} of {} entries shown.]",
                status.files.len(),
                status.files.len() + status.omitted
            ));
        }
        listed
    }
}

/// `list_worktrees`' result text: one line per listed worktree with path,
/// branch (or `detached`), short `HEAD`, changed-file count and, when a base
/// exists, ahead/behind (spec 60 §5.2). The rest are counted, not listed.
pub(super) fn format_worktree_listing(listing: &WorktreeListing) -> String {
    let mut text = match &listing.base {
        Some(base) => format!(
            "Worktrees ({} listed, ahead/behind vs `{base}`):\n",
            listing.entries.len()
        ),
        None => format!("Worktrees ({} listed):\n", listing.entries.len()),
    };
    for entry in &listing.entries {
        let branch = entry.branch.as_deref().unwrap_or("detached");
        let mut line = format!(
            "{}: {} @ {}, {} changed files",
            entry.path,
            branch,
            entry.head.as_deref().unwrap_or("unknown"),
            entry.changed_files,
        );
        if let (Some(ahead), Some(behind)) = (entry.ahead, entry.behind) {
            line.push_str(&format!(", {ahead} ahead, {behind} behind"));
        }
        text.push_str(&line);
        text.push('\n');
    }
    if listing.omitted > 0 {
        text.push_str(&format!(
            "[Worktrees truncated: {} of {} listed.]\n",
            listing.entries.len(),
            listing.entries.len() + listing.omitted
        ));
    }
    // Spec 60 §5.1: without this section a survey that cannot read a worktree
    // reads as complete, and a model concludes the worktree does not exist.
    if !listing.unreachable.is_empty() {
        text.push_str(&format!(
            "Not readable from this session ({}); the read tools refuse these. Branch, HEAD and ahead/behind come from git's worktree list and this repository's commits, not from the worktree:\n",
            listing.unreachable.len() + listing.unreachable_omitted
        ));
        for worktree in &listing.unreachable {
            let mut line = format!(
                "{}: {} @ {}",
                worktree.path,
                worktree.branch.as_deref().unwrap_or("detached"),
                worktree.head.as_deref().unwrap_or("unknown"),
            );
            if let (Some(ahead), Some(behind)) = (worktree.ahead, worktree.behind) {
                line.push_str(&format!(", {ahead} ahead, {behind} behind"));
            }
            line.push_str(&format!("; it {}", worktree.reason));
            text.push_str(&line);
            text.push('\n');
        }
        if listing.unreachable_omitted > 0 {
            text.push_str(&format!(
                "[Unreadable worktrees truncated: {} of {} listed.]\n",
                listing.unreachable.len(),
                listing.unreachable.len() + listing.unreachable_omitted
            ));
        }
    }
    text
}

pub(super) fn tool_call_summary(command_request: &CommandRequest) -> String {
    format!(
        "Ran `{}` — {}",
        command_request.command, command_request.reason
    )
}

/// [`tool_call_summary`] for a run **Approve for this session** let through
/// (spec 109 D5). Still starts "Ran `", which the transcript groups on.
pub(super) fn session_approved_call_summary(command_request: &CommandRequest) -> String {
    format!(
        "Ran `{}` (approved for this session) — {}",
        command_request.command, command_request.reason
    )
}

/// Repository-relative file paths under the configured ignore rules, walked
/// the way the indexer walks (`indexer.rs`).
pub(super) fn repository_files(repository_root: &Path, ignore_patterns: &[String]) -> Vec<String> {
    let rules = crate::ignore::parse_ignore_patterns(ignore_patterns, "");
    let mut files = Vec::new();
    let _ = crate::tree_walk::walk(repository_root, repository_root, "", &rules, &mut |event| {
        if let crate::tree_walk::WalkEvent::File(file) = event {
            files.push(file.relative_path.clone());
        }
        Ok(())
    });
    files
}

pub(super) fn sandbox_command_context(execution: &CommandExecution) -> String {
    let mut output = String::new();
    output.push_str("Command: ");
    output.push_str(&execution.command);
    output.push('\n');
    output.push_str("Working directory: ");
    output.push_str(&execution.working_directory);
    output.push('\n');
    output.push_str("Exit code: ");
    match execution.termination {
        CommandTermination::Exited => {
            output.push_str(&execution.exit_code.unwrap_or(-1).to_string());
        }
        CommandTermination::TimedOut => output.push_str("none (timed out)"),
        CommandTermination::Cancelled => output.push_str("none (cancelled by user)"),
    }
    output.push_str("\n\nSTDOUT:\n");
    output.push_str(&truncate_for_prompt(&execution.stdout, 8_000));
    output.push_str("\n\nSTDERR:\n");
    output.push_str(&truncate_for_prompt(&execution.stderr, 4_000));
    output
}

pub(super) fn truncate_for_prompt(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("\n[truncated]");
    }
    output
}

/// What the checkpoint list shows for a turn: the prompt it precedes, short
/// enough to read in a list and long enough to recognise.
pub(super) fn checkpoint_summary(prompt: &str) -> String {
    let summary = prompt
        .split_whitespace()
        .take(12)
        .collect::<Vec<_>>()
        .join(" ");
    if summary.is_empty() {
        "Before this turn".to_string()
    } else {
        format!("Before: {summary}")
    }
}

pub(super) fn session_title(prompt: &str) -> String {
    let title = prompt
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        format!("Chat {}", create_id("session_title"))
    } else {
        title
    }
}

/// A command's tool result, with the line saying its session approval was
/// revoked when the run changed the repository (spec 109 D5), so the model
/// knows the next run will ask.
pub(super) fn command_context_with_revocation(
    record: &crate::validation::CommandRunRecord,
) -> String {
    let mut context = sandbox_command_context(&record.execution);
    if let Some(changed) = &record.session_approval_revoked {
        context.push_str("\n\n");
        context.push_str(&crate::validation::session_approval_revoked_note(
            &record.execution.command,
            changed,
        ));
    }
    context
}
