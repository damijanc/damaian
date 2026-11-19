//! JSON payloads the UI reads: plans, proposals, results, sessions, findings
//! and checkpoints.

use super::*;

/// Adds `explorationRound`/`maxExplorationRounds` beside the action counts;
/// older readers ignore the new fields (spec 59 §5.5). `notesChars` is the
/// turn's notes size for the progress label (spec 65 §5.8).
pub(crate) fn phase_json(phase: &TurnPhase) -> String {
    format!(
        "{{\"phase\":\"{}\",\"label\":\"{}\",\"round\":{},\"maxRounds\":{},\"explorationRound\":{},\"maxExplorationRounds\":{},\"notesChars\":{},\"reasoningChars\":{}}}",
        phase.kind.as_str(),
        escape_json(&phase.label),
        phase.round,
        phase.max_rounds,
        phase.exploration_round,
        phase.max_exploration_rounds,
        phase.notes_chars,
        phase.reasoning_chars
    )
}

/// The plan as the panel needs it: the steps as the engine holds them, plus
/// the two things the engine has already decided and the frontend must not
/// decide again — each step's outcome and the plan's own summary line.
///
/// `violatesSingleInProgress` is sent rather than left for the panel to work
/// out, because a panel that silently rendered the first of two in-progress
/// steps would conceal exactly the invariant requirement 2 exists to catch.
pub(crate) fn plan_json(plan: &TaskPlan, active: bool) -> String {
    plan_json_with_waiting(plan, active, false)
}

/// [`plan_json`] for a task that may be waiting on the user (spec 107 Task 1).
///
/// `waiting` is true exactly when the engine reports a pending decision for
/// the plan's task, so the panel can say *waiting for you* instead of
/// *running* for the open step without reading the task status itself.
fn plan_json_with_waiting(plan: &TaskPlan, active: bool, waiting: bool) -> String {
    let report = plan.report();
    let steps = plan
        .steps
        .iter()
        .zip(report.steps.iter())
        .map(|(step, reported)| {
            format!(
                "{{\"id\":\"{}\",\"title\":\"{}\",\"detail\":{},\"status\":\"{}\",\"outcome\":\"{}\",\"evidence\":{},\"check\":{},\"build\":{}}}",
                escape_json(&step.id),
                escape_json(&step.title),
                step.detail
                    .as_ref()
                    .map(|detail| format!("\"{}\"", escape_json(detail)))
                    .unwrap_or_else(|| "null".to_string()),
                serde_status(step.status),
                reported.outcome.as_str(),
                serde_json::to_string(&step.evidence).unwrap_or_else(|_| "[]".to_string()),
                check_json(&step.check),
                // Spec 109 D10: shown on the card, never edited there; a
                // revision keeps the step's build check as it is.
                check_json(&step.build)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"taskId\":\"{}\",\"steps\":[{}],\"summary\":\"{}\",\"active\":{},\"waiting\":{},\"isComplete\":{},\"verified\":{},\"unverified\":{},\"blocked\":{},\"skipped\":{},\"outstanding\":{},\"violatesSingleInProgress\":{}}}",
        escape_json(&plan.task_id),
        steps,
        escape_json(&report.summary(active)),
        active,
        waiting,
        report.is_complete,
        report.verified,
        report.unverified,
        report.blocked,
        report.skipped,
        report.outstanding,
        plan.violates_single_in_progress()
    )
}

/// The wire spelling of a step status — the same `snake_case` serde writes into
/// the session log, so the panel and a hand-read log agree.
fn serde_status(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Pending => "pending",
        StepStatus::InProgress => "in_progress",
        StepStatus::Completed => "completed",
        StepStatus::Blocked => "blocked",
        StepStatus::Skipped => "skipped",
    }
}

/// A step's structured check for the review card (spec 105 §7): the fields the
/// user sees — command, directory, purpose and required — plus the
/// engine-internal `version`, which the card transports but does not display.
/// Carrying `version` keeps a re-approval from re-stamping a future-version
/// check to the current one when the user changed nothing.
fn check_json(check: &Option<PlannedCheck>) -> String {
    match check {
        None => "null".to_string(),
        Some(check) => format!(
            "{{\"command\":\"{}\",\"workingDirectory\":\"{}\",\"purpose\":\"{}\",\"required\":{},\"version\":{}}}",
            escape_json(&check.command),
            escape_json(&check.working_directory),
            escape_json(&check.purpose),
            check.required,
            check.version,
        ),
    }
}

/// A plan stopped for review (spec 21 §5.5). Shaped like `plan_json` plus the
/// proposal id and what the approval unblocks, so the review card and the live
/// panel render from the same fields.
pub(crate) fn plan_proposal_json(proposal: Option<&AgentPlanProposal>) -> String {
    let Some(proposal) = proposal else {
        return "null".to_string();
    };
    // The engine owns the budget's title, note, rows, labels and units. This is
    // transport: the object goes out exactly as the engine built it, and the
    // client returns `budgetReview.budget` unchanged on approval. When the
    // engine offered none, the key is absent rather than null, so an inline or
    // historical review keeps the exact shape it had before task budgets
    // existed (spec 103 Task 2).
    let budget_review = proposal
        .budget_review
        .as_ref()
        .and_then(|review| serde_json::to_string(review).ok())
        .map(|review| format!(",\"budgetReview\":{review}"))
        .unwrap_or_default();
    format!(
        "{{\"proposalId\":\"{}\",\"deferredAction\":\"{}\",\"plan\":{}{}}}",
        escape_json(&proposal.id),
        escape_json(&proposal.deferred_action),
        // The review card is a turn waiting for a human, not one that ended.
        plan_json(&proposal.plan, true),
        budget_review
    )
}

/// Spec 56's offer: both providers, the refusal, and the provider's own words.
/// Deliberately has no `allowAlways` — the one field the command card would
/// read to offer a standing decision.
pub(crate) fn fallback_proposal_json(proposal: Option<&AgentFallbackProposal>) -> String {
    let Some(proposal) = proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"proposalId\":\"{}\",\"refusedProvider\":\"{}\",\"refusedModel\":\"{}\",\"fallbackProvider\":\"{}\",\"fallbackModel\":\"{}\",\"refusalCode\":\"{}\",\"message\":\"{}\"}}",
        escape_json(&proposal.id),
        escape_json(&proposal.refused_provider),
        escape_json(&proposal.refused_model),
        escape_json(&proposal.fallback_provider),
        escape_json(&proposal.fallback_model),
        escape_json(&proposal.refusal_code),
        escape_json(&proposal.message)
    )
}

/// Spec 50's question, shaped for the card. It carries no `proposalId`,
/// `command`, `patchId` or `requiresApproval`: a question has nothing to
/// approve, and a field the command or patch route recognises is how one would
/// become an approval in the client.
pub(crate) fn question_proposal_json(proposal: Option<&AgentQuestionProposal>) -> String {
    let Some(proposal) = proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"questionId\":\"{}\",\"question\":\"{}\",\"options\":[{}],\"allowFreeText\":{}}}",
        escape_json(&proposal.question_id),
        escape_json(&proposal.question),
        json_string_array(&proposal.options),
        proposal.allow_free_text
    )
}

/// Spec 51 §5.3's host approval, shaped for the dialog.
///
/// The URL is the string the model wrote, unshortened. `longComponents` says
/// which part is over the display threshold; `query` and `fragment` carry its
/// exact text so the dialog can break it out rather than parse the URL again in
/// JavaScript and risk showing a different string than the request carries.
pub(crate) fn fetch_proposal_json(proposal: Option<&AgentFetchProposal>) -> String {
    let Some(proposal) = proposal else {
        return "null".to_string();
    };
    let flags = proposal
        .long_components
        .iter()
        .map(|flag| {
            format!(
                "{{\"component\":\"{}\",\"bytes\":{}}}",
                escape_json(&flag.component),
                flag.bytes
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let optional = |value: &Option<String>| match value {
        Some(value) => format!("\"{}\"", escape_json(value)),
        None => "null".to_string(),
    };
    format!(
        "{{\"proposalId\":\"{}\",\"host\":\"{}\",\"url\":\"{}\",\"approvalKey\":\"{}\",\"longComponents\":[{}],\"query\":{},\"fragment\":{}}}",
        escape_json(&proposal.id),
        escape_json(&proposal.host),
        escape_json(&proposal.url),
        escape_json(&proposal.approval_key),
        flags,
        optional(&proposal.query),
        optional(&proposal.fragment)
    )
}

/// Spec 61's round-budget pause: the rounds used and granted, and whether the
/// turn is at the hard cap (so the shell can offer only *Answer now*).
pub(crate) fn round_budget_proposal_json(proposal: Option<&AgentRoundBudgetProposal>) -> String {
    let Some(proposal) = proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"proposalId\":\"{}\",\"roundsUsed\":{},\"roundsTotal\":{},\"roundsGranted\":{},\"roundsMore\":{},\"atHardCap\":{}}}",
        escape_json(&proposal.id),
        proposal.rounds_used,
        proposal.rounds_total,
        proposal.rounds_granted,
        proposal.rounds_more,
        proposal.at_hard_cap,
    )
}

pub(crate) fn chat_result_json(result: &ChatTurnResult) -> String {
    let turn_notes = match &result.turn_notes {
        Some(notes) => format!(
            "{{\"text\":\"{}\",\"carried\":{}}}",
            escape_json(&notes.text),
            notes.carried
        ),
        None => "null".to_string(),
    };
    format!(
        "{{\"response\":\"{}\",\"contextFiles\":[{}],\"sessionId\":\"{}\",\"taskId\":\"{}\",\"taskStatus\":\"{}\",\"taskControl\":{},\"turnPresentation\":{},\"modelRunId\":\"{}\",\"incomplete\":{},\"cancelled\":{},\"commandProposal\":{},\"patchProposal\":{},\"planProposal\":{},\"fallbackProposal\":{},\"roundBudgetProposal\":{},\"questionProposal\":{},\"fetchProposal\":{},\"checkBatch\":{},\"verificationReport\":{},\"turnNotes\":{},\"usage\":{},\"contextReview\":{}}}",
        escape_json(&result.response),
        json_string_array(&result.context_files),
        escape_json(&result.session.id),
        escape_json(&result.task.id),
        result.task.status.as_str(),
        serde_json::to_string(&result.task_control).unwrap_or_else(|_| "null".to_string()),
        serde_json::to_string(&result.turn_presentation).unwrap_or_else(|_| "null".to_string()),
        escape_json(&result.model_run.run_id),
        result.model_run.incomplete,
        result.cancelled,
        command_proposal_json(result),
        patch_proposal_json(result),
        plan_proposal_json(result.plan_proposal.as_ref()),
        fallback_proposal_json(result.fallback_proposal.as_ref()),
        round_budget_proposal_json(result.round_budget_proposal.as_ref()),
        question_proposal_json(result.question_proposal.as_ref()),
        fetch_proposal_json(result.fetch_proposal.as_ref()),
        check_batch_json(result.check_batch.as_ref()),
        verification_report_json(result.verification_report.as_ref()),
        turn_notes,
        task_usage_json(result.usage.as_ref(), result.estimated_cost),
        context_review_json(result.context_review.as_ref())
    )
}

/// A paused pre-send context review (spec 27 §5.1). The manifest only: the
/// prepared items never leave the engine.
fn context_review_json(review: Option<&AgentContextReview>) -> String {
    let Some(review) = review else {
        return "null".to_string();
    };
    format!(
        "{{\"proposalId\":\"{}\",\"manifest\":{}}}",
        escape_json(&review.id),
        serde_json::to_string(&review.manifest).unwrap_or_else(|_| "null".to_string())
    )
}

/// One task's usage, or `null` when nothing was recorded. Shaped like the
/// per-task fields on `/api/session` so the frontend renders both with the
/// same code.
/// `estimated_cost` is the user's own rates applied to these tokens, and is a
/// separate field from `reportedCost` on purpose: one is what the provider
/// charged, the other is arithmetic the user configured.
///
/// Takes the whole [`CostEstimate`] rather than its amount so the upper-bound
/// label cannot be lost on the way to the client: this is the single funnel
/// every caller goes through. Spec 49 §5.3.
pub(crate) fn task_usage_json(
    usage: Option<&TaskUsage>,
    estimated_cost: Option<CostEstimate>,
) -> String {
    let Some(usage) = usage else {
        return "null".to_string();
    };
    // Omitted entirely when no run reported a split, for the same reason
    // `reportedCost` is: a zero renders as a measurement of nothing, and here
    // that reads as "caching is broken" rather than "we cannot see it".
    // `runsWithoutCacheReport` is always present, because it is what lets the
    // client say which of the two it is. Spec 49 §5.6.
    let cache = match usage.cached_input_tokens {
        Some(cached) => {
            // The denominator is the input tokens of the runs that reported,
            // not the task's — see `TaskUsage::cache_reported_input_tokens`.
            // Zero is reachable: a refused call records a measured zero.
            let rate = match usage.cache_reported_input_tokens {
                0 => String::new(),
                total => format!(",\"cacheHitRate\":{}", cached as f64 / total as f64),
            };
            format!(",\"cachedInputTokens\":{cached}{rate}")
        }
        None => String::new(),
    };
    // Every model the task's calls ran on, the starting model first (spec 108
    // D5). Always present beside the token fields; empty only for a task whose
    // log names no model at all.
    let models = serde_json::to_string(&usage.models).unwrap_or_else(|_| "[]".to_string());
    format!(
        "{{\"inputTokens\":{},\"outputTokens\":{},\"usageSource\":\"{}\",\"runCount\":{}{}\
         ,\"runsWithoutCacheReport\":{},\"models\":{}{}{}}}",
        usage.input_tokens,
        usage.output_tokens,
        usage.source.as_str(),
        usage.run_count,
        cache,
        usage.runs_without_cache_report,
        models,
        match usage.reported_cost {
            Some(cost) => format!(",\"reportedCost\":{cost}"),
            None => String::new(),
        },
        match estimated_cost {
            // `estimatedCostIsUpperBound` rides with the figure rather than
            // being derivable from it: a client cannot tell a ceiling from an
            // exact number by looking at the number.
            Some(estimate) => format!(
                ",\"estimatedCost\":{},\"estimatedCostIsUpperBound\":{}",
                estimate.amount(),
                estimate.is_upper_bound()
            ),
            None => String::new(),
        }
    )
}

pub(crate) fn command_proposal_json(result: &ChatTurnResult) -> String {
    result
        .command_proposal
        .as_ref()
        .map(agent_command_json)
        .unwrap_or_else(|| "null".to_string())
}

/// One check or command proposal as the card renders it. Shared by the single
/// command card and the check batch so a check cannot show a different risk,
/// cause or `Allow Always` answer from the command it names.
fn agent_command_json(proposal: &workspace_engine::AgentCommandProposal) -> String {
    format!(
        "{{\"proposalId\":\"{}\",\"command\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{},\"cause\":\"{}\",\"reason\":\"{}\",\"allowAlways\":{},\"allowBrowserDiagnosticsForSession\":{},\"directory\":\"{}\"}}",
        escape_json(&proposal.id),
        escape_json(&proposal.command),
        escape_json(&proposal.prompt),
        escape_json(&proposal.risk),
        proposal.requires_approval,
        proposal.blocked,
        escape_json(&proposal.cause),
        escape_json(&proposal.reason),
        proposal.allow_always,
        proposal.allow_browser_diagnostics_for_session,
        escape_json(&proposal.directory)
    )
}

/// One card listing every check the verification loop wants to run.
pub(crate) fn check_batch_json(batch: Option<&AgentCheckBatch>) -> String {
    let Some(batch) = batch else {
        return "null".to_string();
    };
    // A failed check's baseline card (spec 109 D1) carries the engine's own
    // heading and explanation; `null` keeps the ordinary check card's wording.
    let optional = |text: Option<&String>| {
        text.map_or_else(
            || "null".to_string(),
            |text| format!("\"{}\"", escape_json(text)),
        )
    };
    format!(
        "{{\"proposalId\":\"{}\",\"title\":{},\"note\":{},\"checks\":[{}]}}",
        escape_json(&batch.id),
        optional(batch.title.as_ref()),
        optional(batch.note.as_ref()),
        batch
            .checks
            .iter()
            .map(agent_command_json)
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// The structured report the report card renders. The type is already
/// camelCase-serialised; the text copy is in `response`.
pub(crate) fn verification_report_json(report: Option<&VerificationReport>) -> String {
    report
        .and_then(|report| serde_json::to_string(report).ok())
        .unwrap_or_else(|| "null".to_string())
}

/// The last verification's durable outcome, and whether spec 96's explicit
/// *Continue repair* action applies.
///
/// A reloaded report is text only, so this is what lets the shell offer the
/// action again without keeping in-memory turn state. The engine repeats the
/// same check when the action is used: the limit's verification must still be
/// the latest task, and that task must still be `failed`.
pub(crate) fn session_verification_json(engine: &WorkspaceEngine, session_id: &str) -> String {
    let Ok(events) = engine
        .session_store
        .read_verification_events(session_id, &[])
    else {
        return "null".to_string();
    };
    let state = fold_verification(events.iter());
    let Some(outcome) = state.finished else {
        return "null".to_string();
    };
    let Some(task_id) = events.iter().rev().find_map(|event| match event {
        VerificationEvent::Finished { task_id, .. }
        | VerificationEvent::RepairStalled { task_id, .. } => Some(task_id.clone()),
        _ => None,
    }) else {
        return "null".to_string();
    };
    let latest_is_limited_task = engine
        .session_store
        .read_tasks(session_id)
        .ok()
        .and_then(|tasks| tasks.into_iter().last())
        .is_some_and(|task| {
            task.id == task_id && task.status == workspace_engine::TaskStatus::Failed
        });
    let can_continue = outcome == VerificationOutcome::RepairLimitExhausted
        && state.due.is_none()
        && latest_is_limited_task;
    // A stale reload still needs the engine's typed stop, including its label,
    // explanation and action. The shell prints those strings and never derives
    // a stall kind from the outcome or from a check count.
    let repair_stall = state
        .repair_stall
        .as_ref()
        .and_then(|stall| serde_json::to_string(stall).ok())
        .unwrap_or_else(|| "null".to_string());
    format!(
        "{{\"taskId\":\"{}\",\"outcome\":\"{}\",\"canContinueRepair\":{},\"repairStall\":{}}}",
        escape_json(&task_id),
        outcome.as_str(),
        can_continue,
        repair_stall
    )
}

/// Shaped identically to `/api/propose-edit`'s response (`patchId` +
/// `summary` + `files`) so the frontend can render both with the exact same
/// `createPatchPreview` component regardless of whether the patch came from
/// a `propose_patch` tool call mid-chat or the dedicated one-shot edit flow.
pub(crate) fn patch_proposal_json(result: &ChatTurnResult) -> String {
    let Some(proposal) = &result.patch_proposal else {
        return "null".to_string();
    };
    patch_payload_json(
        &proposal.patch_id,
        &proposal.summary,
        &proposal.files,
        proposal.repair_scope.as_ref(),
    )
}

/// The one patch-preview payload for a live turn, a worker result and a
/// reattached card. The engine-owned repair-scope review travels with it, so
/// every route renders the same fact (spec 102 Task 5).
pub(crate) fn patch_payload_json(
    patch_id: &str,
    summary: &str,
    files: &[ProposedFilePatch],
    repair_scope: Option<&workspace_engine::RepairScopeReview>,
) -> String {
    let repair_scope = repair_scope
        .map(|scope| serde_json::to_string(scope).unwrap_or_else(|_| "null".to_string()))
        .unwrap_or_else(|| "null".to_string());
    format!(
        "{{\"patchId\":\"{}\",\"summary\":\"{}\",\"files\":[{}],\"repairScope\":{}}}",
        escape_json(patch_id),
        escape_json(summary),
        patch_files_json(files),
        repair_scope
    )
}

pub(crate) fn patch_files_json(files: &[ProposedFilePatch]) -> String {
    files
        .iter()
        .map(|file| {
            format!(
                "{{\"path\":\"{}\",\"status\":\"{}\",\"baseHash\":{},\"newHash\":\"{}\",\"diff\":\"{}\",\"hunks\":{}}}",
                escape_json(&file.path),
                escape_json(&file.status),
                file.base_hash
                    .as_ref()
                    .map(|hash| format!("\"{}\"", escape_json(hash)))
                    .unwrap_or_else(|| "null".to_string()),
                escape_json(&file.new_hash),
                escape_json(&file.diff),
                serde_json::to_string(&file.hunks).unwrap_or_else(|_| "[]".to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The `/api/sessions` rows, each with the `state` the sidebar draws as a
/// dot (spec 112 requirement 7), or none.
pub(crate) fn sessions_json(
    store: &SessionStore,
    sessions: &[Session],
    working: &std::collections::HashSet<String>,
) -> String {
    sessions
        .iter()
        .map(|session| {
            // A log that cannot be read shows no dot rather than failing the
            // whole list.
            let latest = store.read_latest_task_status(&session.id).ok().flatten();
            let state = session_state(working.contains(&session.id), latest.as_deref());
            match state {
                Some(state) => {
                    format!("{{{},\"state\":\"{state}\"}}", session_fields_json(session))
                }
                None => session_summary_json(session),
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Which of the sidebar's three states a session is in (spec 112 D6):
/// `working` while it holds a lane — the source the working-sessions poll
/// reads, so the dot and the composer slot agree — else its newest task's
/// status. A turn paused for a decision holds no lane, so the two never
/// compete. Every other status (idle, complete, stopped, cancelled) has none.
pub(crate) fn session_state(
    working: bool,
    latest_task_status: Option<&str>,
) -> Option<&'static str> {
    if working {
        return Some("working");
    }
    match latest_task_status {
        Some(status) if status == workspace_engine::TaskStatus::WaitingForApproval.as_str() => {
            Some("needs_you")
        }
        Some(status) if status == workspace_engine::TaskStatus::Failed.as_str() => Some("failed"),
        _ => None,
    }
}

pub(crate) fn export_filename(
    engine: &WorkspaceEngine,
    session_id: &str,
    extension: &str,
) -> Result<String, String> {
    let title = engine
        .session_store
        .read_session(session_id)
        .map_err(|error| error.to_string())?
        .map(|session| session.title)
        .unwrap_or_else(|| session_id.to_string());
    let sanitized: String = title
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('_');
    Ok(format!(
        "{}.{extension}",
        if sanitized.is_empty() {
            session_id.to_string()
        } else {
            sanitized.to_string()
        }
    ))
}

/// One session as the session list shows it. Carries no `mode`: the mode lives
/// in a separate event, and reading it for every listed session would read
/// every session log a second time. The open session's mode arrives through
/// [`session_json`] instead.
fn session_summary_json(session: &Session) -> String {
    format!("{{{}}}", session_fields_json(session))
}

pub(crate) fn map_load_json(load: &workspace_engine::MapLoad) -> String {
    use workspace_engine::MapLoad;
    match load {
        MapLoad::Reused => "{\"outcome\":\"reused\"}".to_string(),
        MapLoad::Built => "{\"outcome\":\"built\"}".to_string(),
        MapLoad::Rebuilt { reason } => match reason {
            workspace_engine::RebuildReason::SchemaMismatch { found } => format!(
                "{{\"outcome\":\"rebuilt\",\"reason\":\"{}\",\"foundSchemaVersion\":{found}}}",
                reason.as_str()
            ),
            _ => format!(
                "{{\"outcome\":\"rebuilt\",\"reason\":\"{}\"}}",
                reason.as_str()
            ),
        },
    }
}

/// The session, when it exists and belongs to `repo`. A finding's staleness
/// is judged against `repo`'s files, so a session from another checkout must
/// not be read against this one (spec 22 `context.md` §16).
pub(crate) fn session_in_repository(
    engine: &WorkspaceEngine,
    repo: &str,
    session_id: &str,
) -> Result<Session, String> {
    let session = engine
        .session_store
        .read_session(session_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Unknown session: {session_id}"))?;
    let repository_id = engine
        .indexer
        .repository_id_for_path(repo)
        .map_err(|error| error.to_string())?;
    if session.repository_id != repository_id {
        return Err(format!(
            "Session {session_id} belongs to another repository"
        ));
    }
    Ok(session)
}

/// `{"findings":[…]}` in record order, with status derived against `repo`.
pub(crate) fn findings_json(
    engine: &WorkspaceEngine,
    repo: &str,
    session_id: &str,
) -> Result<String, String> {
    let findings = engine
        .session_store
        .read_findings(session_id, Path::new(repo))
        .map_err(|error| error.to_string())?;
    let findings = serde_json::to_string(&findings).map_err(|error| error.to_string())?;
    Ok(format!("{{\"findings\":{findings}}}"))
}

/// One session with its working mode (spec 20), for the responses that open,
/// create, rename, or change the mode of a single session.
pub(crate) fn session_json(session: &Session, mode: SessionMode) -> String {
    format!(
        "{{{},\"mode\":\"{}\"}}",
        session_fields_json(session),
        mode.as_str()
    )
}

fn session_fields_json(session: &Session) -> String {
    format!(
        "\"id\":\"{}\",\"repositoryId\":\"{}\",\"title\":\"{}\",\"createdAtMs\":{},\"updatedAtMs\":{},\"summary\":\"{}\",\"origin\":\"{}\",\"archived\":{}",
        escape_json(&session.id),
        escape_json(&session.repository_id),
        escape_json(&session.title),
        session.created_at_ms,
        session.updated_at_ms,
        escape_json(&session.summary),
        escape_json(&session.origin),
        session.archived
    )
}

/// Each task's status and, when it has any, what it spent.
///
/// Usage fields are **omitted** for a task with no usage events rather than
/// sent as zero: a session written before spec 19 has no figures, and a zero
/// would render as a turn that cost nothing. Same for `reportedCost`, which is
/// absent unless the provider actually reported one.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn task_states_json(
    statuses: &HashMap<String, String>,
    usage: &HashMap<String, TaskUsage>,
    plans: &HashMap<String, TaskPlan>,
    failure_kinds: &HashMap<String, String>,
    web_diagnostics: &HashMap<String, Vec<WebDiagnosticRecord>>,
    notes: &BTreeMap<String, TurnNotes>,
    contexts: &HashMap<String, ContextManifest>,
    config: &Config,
) -> String {
    task_states_json_with_outcomes(
        statuses,
        usage,
        plans,
        failure_kinds,
        web_diagnostics,
        notes,
        contexts,
        config,
        &HashMap::new(),
        &HashMap::new(),
    )
}

/// [`task_states_json`] with each task's typed terminal outcome (spec 103
/// Task 6), so a reloaded turn's presentation carries the same engine-owned
/// label, explanation and actions as the live turn.
#[allow(clippy::too_many_arguments)]
pub(crate) fn task_states_json_with_outcomes(
    statuses: &HashMap<String, String>,
    usage: &HashMap<String, TaskUsage>,
    plans: &HashMap<String, TaskPlan>,
    failure_kinds: &HashMap<String, String>,
    web_diagnostics: &HashMap<String, Vec<WebDiagnosticRecord>>,
    notes: &BTreeMap<String, TurnNotes>,
    contexts: &HashMap<String, ContextManifest>,
    config: &Config,
    outcomes: &HashMap<String, workspace_engine::TaskTerminalOutcome>,
    pending_decisions: &HashMap<String, workspace_engine::TurnPendingDecision>,
) -> String {
    let mut entries: Vec<&String> = statuses.keys().collect();
    // Sorted so the payload is stable between requests.
    entries.sort();
    entries
        .iter()
        .map(|id| {
            let usage_json = match usage.get(*id) {
                Some(total) => {
                    let estimated_cost = config.estimated_cost(&TokenUsage {
                        input_tokens: total.input_tokens,
                        output_tokens: total.output_tokens,
                        cached_input_tokens: total.cached_input_tokens,
                        source: total.source,
                    });
                    let body = task_usage_json(Some(total), estimated_cost);
                    // Spliced into this entry rather than nested, so the shape
                    // matches what the turn response sends and the frontend
                    // reads both with the same code.
                    format!(",{}", body.trim_start_matches('{').trim_end_matches('}'))
                }
                None => String::new(),
            };
            // Absent rather than an empty plan when the turn had none: a
            // trivial turn and a plan that proposed nothing are different
            // facts, and the panel must not appear for the first.
            //
            // `active` comes from the task status already in this payload, so a
            // reloaded session agrees with the live stream about whether an
            // open step is "running" (spec 66 §5.4). An unknown or legacy
            // status is treated as ended: saying "running" about a turn we
            // cannot classify is the wrong direction.
            let active = statuses
                .get(*id)
                .and_then(|status| workspace_engine::TaskStatus::parse(status))
                .is_some_and(|status| status.is_active());
            // Spec 107 Task 1: a task waiting on a decision says so, from the
            // same engine value the turn presentation carries below.
            let pending_decision = pending_decisions.get(*id);
            let plan_json_field = match plans.get(*id) {
                Some(plan) => format!(
                    ",\"plan\":{}",
                    plan_json_with_waiting(plan, active, pending_decision.is_some())
                ),
                None => String::new(),
            };
            // A named failure reason, when the turn failed with one (spec 48
            // §5.4). Absent for every other outcome, so old sessions and
            // successful turns are unchanged.
            let failure_kind_field = match failure_kinds.get(*id) {
                Some(kind) => format!(",\"failureKind\":\"{}\"", escape_json(kind)),
                None => String::new(),
            };
            // Absent when the turn ran no diagnostic, like `plan`, so the
            // card appears only for turns that have one.
            let web_diagnostics_field = match web_diagnostics.get(*id) {
                Some(records) if !records.is_empty() => format!(
                    ",\"webDiagnostics\":[{}]",
                    records
                        .iter()
                        .map(web_diagnostic_json)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                _ => String::new(),
            };
            // The turn's final notes, joined by `taskId` (spec 65 §5.8).
            // Absent when the turn recorded none, so a reload shows the
            // disclosure only for turns that have one.
            let notes_field = match notes.get(*id) {
                Some(notes) if !notes.text.is_empty() => format!(
                    ",\"notes\":{{\"text\":\"{}\",\"carried\":{}}}",
                    escape_json(&notes.text),
                    notes.carried
                ),
                _ => String::new(),
            };
            // The turn's newest context manifest (spec 27 §5.3). Absent when
            // the turn has no recorded assembly, so an old session stays
            // panel-free rather than showing an empty one.
            let context_field = match contexts.get(*id) {
                Some(manifest) => format!(
                    ",\"context\":{}",
                    serde_json::to_string(manifest).unwrap_or_else(|_| "null".to_string())
                ),
                None => String::new(),
            };
            // Spec 101 Task 3: a reloaded turn draws its stop row and any
            // action from the engine's presentation, not from its status.
            // Spec 103 Task 6 adds the typed terminal outcome so a bounded stop
            // keeps its engine-owned label/explanation after reload.
            let outcome = outcomes.get(*id);
            let turn_presentation = statuses
                .get(*id)
                .and_then(|status| workspace_engine::TaskStatus::parse(status))
                .map(|status| {
                    let mut presentation = workspace_engine::turn_presentation_with_outcome(
                        &status,
                        false,
                        false,
                        Some(id.as_str()),
                        outcome,
                    );
                    presentation.pending_decision = pending_decision.cloned();
                    presentation
                });
            let turn_presentation_field = match turn_presentation {
                Some(presentation) => format!(
                    ",\"turnPresentation\":{}",
                    serde_json::to_string(&presentation).unwrap_or_else(|_| "null".to_string())
                ),
                None => String::new(),
            };
            let terminal_outcome_field = match outcome {
                Some(outcome) => format!(
                    ",\"terminalOutcome\":{}",
                    serde_json::to_string(outcome).unwrap_or_else(|_| "null".to_string())
                ),
                None => String::new(),
            };
            format!(
                "{{\"id\":\"{}\",\"status\":\"{}\"{}{}{}{}{}{}{}{}}}",
                escape_json(id),
                escape_json(&statuses[*id]),
                usage_json,
                failure_kind_field,
                plan_json_field,
                notes_field,
                web_diagnostics_field,
                turn_presentation_field,
                terminal_outcome_field,
                context_field
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn messages_json(
    messages: &[(u64, ChatMessage)],
    questions: &HashMap<String, QuestionState>,
) -> String {
    messages
        .iter()
        .map(|(seq, message)| message_json(*seq, message, questions.get(&message.id)))
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn message_json(
    seq: u64,
    message: &ChatMessage,
    question: Option<&QuestionState>,
) -> String {
    // A question card needs what the question was asked with; no other role
    // has anything to add, so the fields appear only on `question`.
    let question_fields = question
        .map(|state| {
            format!(
                ",\"options\":[{}],\"allowFreeText\":{}",
                json_string_array(&state.options),
                state.allow_free_text
            )
        })
        .unwrap_or_default();
    // `kind` is absent for an ordinary message, so the payload is unchanged
    // for every session written before the plan reminder existed.
    let kind_field = message
        .kind
        .as_ref()
        .map(|kind| format!(",\"kind\":\"{}\"", escape_json(kind)))
        .unwrap_or_default();
    format!(
        "{{\"id\":\"{}\",\"sessionId\":\"{}\",\"taskId\":{},\"role\":\"{}\",\"content\":\"{}\",\"createdAtMs\":{},\"seq\":{}{}{}}}",
        escape_json(&message.id),
        escape_json(&message.session_id),
        message
            .task_id
            .as_ref()
            .map(|value| format!("\"{}\"", escape_json(value)))
            .unwrap_or_else(|| "null".to_string()),
        escape_json(&message.role),
        escape_json(&message.content),
        message.created_at_ms,
        seq,
        kind_field,
        question_fields
    )
}

pub(crate) fn friendly_chat_error(error: &str) -> String {
    let lower = error.to_lowercase();
    if !workspace_engine::error::is_retryable_message(error) {
        return error.to_string();
    }
    if lower.contains("rate limit") || lower.contains("429") {
        "Model provider rate limit. Wait for the provider retry window, then try again.".to_string()
    } else if lower.contains("timeout") || lower.contains("timed out") || lower.contains("too slow")
    {
        "Model provider request timed out. Try again, or lower the context size.".to_string()
    } else {
        "Model provider network request failed. Check connectivity and provider URL.".to_string()
    }
}

pub(crate) fn json_error(message: &str) -> String {
    format!("{{\"error\":\"{}\"}}", escape_json(message))
}

/// Serialises secret-scan warnings for the patch UI. Categories and counts
/// only — the matched values never leave the engine.
pub(crate) fn generated_secret_warnings_json(warnings: &[GeneratedSecretWarning]) -> String {
    warnings
        .iter()
        .map(|warning| {
            format!(
                "{{\"path\":\"{}\",\"count\":{},\"categories\":[{}]}}",
                escape_json(&warning.path),
                warning.count,
                json_string_array(&warning.categories)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Renders an optional path as a JSON string or `null`, so clients can tell
/// "no allowlist entry was written" apart from "written to the empty path".
pub(crate) fn json_optional_string(value: Option<&Path>) -> String {
    match value {
        Some(path) => format!("\"{}\"", escape_json(&path.to_string_lossy())),
        None => "null".to_string(),
    }
}

/// What the checkpoint list view needs: when, what turn, how much it covers,
/// and whether it has been rewound to. Never file content — the manifest holds
/// none, and this must not become the place that changes.
pub(crate) fn checkpoint_list_json(manifests: &[workspace_engine::CheckpointManifest]) -> String {
    let entries = manifests
        .iter()
        .map(|manifest| {
            format!(
                "{{\"checkpointId\":\"{}\",\"sessionId\":\"{}\",\"taskId\":{},\"createdAtMs\":{},\"summary\":\"{}\",\"userMessageId\":{},\"fileCount\":{},\"excludedCount\":{},\"commandEffectsCovered\":{},\"restoredAtMs\":{},\"files\":[{}],\"pendingApprovals\":[{}]}}",
                escape_json(&manifest.checkpoint_id),
                escape_json(&manifest.session_id),
                manifest
                    .task_id
                    .as_ref()
                    .map(|id| format!("\"{}\"", escape_json(id)))
                    .unwrap_or_else(|| "null".to_string()),
                manifest.created_at_ms,
                escape_json(&manifest.summary),
                manifest
                    .user_message_id
                    .as_ref()
                    .map(|id| format!("\"{}\"", escape_json(id)))
                    .unwrap_or_else(|| "null".to_string()),
                manifest.files.len(),
                manifest.excluded.len(),
                manifest.command_effects_covered,
                manifest
                    .restored_at_ms
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                checkpoint_files_json(&manifest.files),
                checkpoint_pending_approvals_json(&manifest.pending_approvals),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"checkpoints\":[{entries}]}}")
}

/// The paths one checkpoint covers, so the UI can offer to restore a single
/// file. Path and origin only — a hash here would be noise, and content would
/// be a leak.
fn checkpoint_files_json(files: &[workspace_engine::CheckpointFile]) -> String {
    files
        .iter()
        .map(|file| {
            format!(
                "{{\"path\":\"{}\",\"origin\":\"{}\"}}",
                escape_json(&file.path),
                match file.origin {
                    workspace_engine::CheckpointOrigin::Patch => "patch",
                    workspace_engine::CheckpointOrigin::Command => "command",
                }
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn checkpoint_pending_approvals_json(approvals: &[workspace_engine::PendingApproval]) -> String {
    approvals
        .iter()
        .map(|approval| {
            format!(
                "{{\"kind\":\"{}\",\"proposalId\":\"{}\"}}",
                escape_json(&approval.kind),
                escape_json(&approval.proposal_id)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Skipped and conflicted stay apart all the way to the UI: "there was nothing
/// to do" and "the file changed under you" call for different next steps.
pub(crate) fn checkpoint_restore_json(
    result: &workspace_engine::CheckpointRestoreResult,
) -> String {
    format!(
        "{{\"checkpointId\":\"{}\",\"restoredFiles\":[{}],\"deletedFiles\":[{}],\"skippedFiles\":[{}],\"conflictedFiles\":[{}],\"conversationRestored\":{},\"warnings\":[{}]}}",
        escape_json(&result.checkpoint_id),
        json_string_array(&result.restored_files),
        json_string_array(&result.deleted_files),
        json_string_array(&result.skipped_files),
        json_string_array(&result.conflicted_files),
        result.conversation_restored,
        json_string_array(&result.warnings)
    )
}

pub(crate) fn json_string_array(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("\"{}\"", escape_json(value)))
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn escape_json(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}
