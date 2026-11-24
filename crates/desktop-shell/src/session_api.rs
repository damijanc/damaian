//! Session routes: the session view, conversation, context, archive and
//! delete, checkpoints and rewind.

use super::*;

pub(crate) fn handle_session_export(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let session_id = required_param(request, "session_id")?;
    let format = request
        .param("format")
        .as_deref()
        .map_or(ExportFormat::Markdown, |value| match value {
            "json" => ExportFormat::Json,
            _ => ExportFormat::Markdown,
        });
    let engine = default_engine()?;
    let content = engine
        .session_store
        .export_session(&session_id, format, &engine.scanner)
        .map_err(|error| error.to_string())?;
    let (content_type, extension) = match format {
        ExportFormat::Markdown => ("text/markdown; charset=utf-8", "md"),
        ExportFormat::Json => ("application/json", "json"),
    };
    // The browser's save dialog is the path the user chose: the export
    // writes nowhere itself and makes no network request.
    let filename = export_filename(&engine, &session_id, extension)?;
    let disposition = format!("content-disposition: attachment; filename=\"{filename}\"");
    write_response_with_extra_headers(stream, request, 200, content_type, &content, &disposition)
}

pub(crate) fn handle_recovery(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let payload = recovery::sweep_json()?;
    write_response(stream, request, 200, "application/json", &payload)
}

/// The pending card of one waiting task, read without side effects, so a
/// reload can redraw a decision raised while the app runs.
pub(crate) fn handle_pending_card(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let session_id = required_param(request, "session_id")?;
    let task_id = required_param(request, "task_id")?;
    let payload = recovery::pending_card_json(&session_id, &task_id)?;
    write_response(stream, request, 200, "application/json", &payload)
}

pub(crate) fn handle_recovery_decision(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let payload = recovery::decide(&form)?;
    write_response(stream, request, 200, "application/json", &payload)
}

pub(crate) fn handle_recovery_approval_resolved(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let payload = recovery::resolve_reattached_approval(&form)?;
    write_response(stream, request, 200, "application/json", &payload)
}

pub(crate) fn handle_session(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let session_id = required_param(request, "session_id")?;
    let engine = default_engine()?;
    let Some(session) = engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
    else {
        return Err(format!("Unknown session: {session_id}"));
    };
    let messages = engine
        .session_store
        .read_messages_with_seq(&session_id)
        .map_err(|error| error.to_string())?;
    // A question's options are in its `question_asked` event, not on the
    // transcript message, and a reopened card is answerable only with them.
    // Read once per question so `messages_json` stays a formatter.
    let mut question_states = HashMap::new();
    for (_, message) in &messages {
        if message.role != "question" {
            continue;
        }
        if let Some(state) = engine
            .session_store
            .question_state(&session_id, &message.id)
            .map_err(|error| error.to_string())?
        {
            question_states.insert(message.id.clone(), state);
        }
    }
    // Message roles alone cannot say whether a turn was stopped, so the
    // task statuses ride along and the UI joins them by `taskId`.
    let task_statuses = engine
        .session_store
        .read_task_statuses(&session_id)
        .map_err(|error| error.to_string())?;
    // What each turn spent, joined by the same `taskId` (spec 19 §5.6).
    let task_usage = engine
        .session_store
        .read_task_usage(&session_id)
        .map_err(|error| error.to_string())?;
    // The plan each turn worked through, joined by the same `taskId`
    // (spec 21 §5.6). Without this the panel — and with it the
    // completion report — would live only for the turn that produced
    // it, so reopening a session would lose the record of what a plan
    // came to, which is the part a user comes back for.
    let task_plans = engine
        .session_store
        .read_session_plans(&session_id)
        .map_err(|error| error.to_string())?;
    // A named failure reason, joined by `taskId` (spec 48 §5.4), so a
    // refused turn says *why* it failed rather than only that it did.
    let task_failure_kinds = engine
        .session_store
        .read_task_failure_kinds(&session_id)
        .map_err(|error| error.to_string())?;
    // Each turn's browser diagnostics, joined by `taskId` (spec 12
    // `context.md` §3.3). Already redacted when recorded. Without
    // this the card would exist only for the turn that ran it, and a
    // reload would fall back to bare thumbnails.
    let task_web_diagnostics = engine
        .session_store
        .read_session_web_diagnostics(&session_id)
        .map_err(|error| error.to_string())?;
    // Each turn's final notes, joined by `taskId` (spec 65 §5.8). Already
    // redacted when recorded.
    let task_notes = engine
        .session_store
        .read_turn_notes(&session_id)
        .map_err(|error| error.to_string())?;
    // Each task's typed terminal outcome (spec 103 Task 6), joined by `taskId`.
    let task_outcomes = engine
        .session_store
        .read_task_outcomes(&session_id)
        .map_err(|error| error.to_string())?;
    // What each waiting task needs from the user (spec 107 Task 1), read by
    // the engine from the durable pause link — the same reader a live turn's
    // presentation uses, so a reload shows the decision the stream showed.
    let mut task_pending_decisions = HashMap::new();
    let pending_reader = workspace_engine::TaskControlService::new(&engine.session_store, None);
    for (task_id, raw_status) in &task_statuses {
        let Some(status) = workspace_engine::TaskStatus::parse(raw_status) else {
            continue;
        };
        if let Some(decision) = pending_reader
            .pending_decision(&session_id, task_id, &status)
            .map_err(|error| error.to_string())?
        {
            task_pending_decisions.insert(task_id.clone(), decision);
        }
    }
    // Each turn's newest context manifest, joined by `taskId` (spec 27 §5.3),
    // and the session's pins and restriction (spec 27 §5.4).
    let task_contexts = engine
        .session_store
        .read_session_context_manifests(&session_id)
        .map_err(|error| error.to_string())?;
    let context_pins = engine
        .session_store
        .context_pins(&session_id)
        .unwrap_or_default();
    let context_restriction = engine
        .session_store
        .context_restriction(&session_id)
        .map_err(|error| error.to_string())?;
    // The last verification's outcome survives a reload only as its text; this
    // is what lets the report's explicit *Continue repair* action (spec 96)
    // be offered again.
    let verification = session_verification_json(&engine, &session_id);
    // Pending worker results (spec 100 §4). A return is durable control state
    // in this session's log; only the ones not yet consumed are shown.
    let worker_returns = engine
        .session_store
        .pending_worker_returns(&session_id)
        .map_err(|error| error.to_string())?;
    // Spec 101 Task 3: the ordinary session's outcome and primary action are
    // engine data. The UI renders this model and does not infer a status.
    let task_control = workspace_engine::TaskControlService::new(&engine.session_store, None)
        .session_control(&session_id)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"session\":{},\"messages\":[{}],\"tasks\":[{}],\"conversationView\":{},\"contextState\":{{\"pins\":{},\"restrictedPaths\":{}}},\"verification\":{},\"taskControl\":{},\"workerReturns\":{}}}",
            session_json(&session, engine.session_store.session_mode(&session.id)),
            messages_json(&messages, &question_states),
            task_states_json_with_outcomes(
                &task_statuses,
                &task_usage,
                &task_plans,
                &task_failure_kinds,
                &task_web_diagnostics,
                &task_notes,
                &task_contexts,
                &engine.config,
                &task_outcomes,
                &task_pending_decisions
            ),
            conversation_json(&engine, &session_id),
            serde_json::to_string(&context_pins).unwrap_or_else(|_| "[]".to_string()),
            serde_json::to_string(&context_restriction).unwrap_or_else(|_| "null".to_string()),
            verification,
            serde_json::to_string(&task_control).unwrap_or_else(|_| "null".to_string()),
            worker_returns_json(&engine, &worker_returns)
        ),
    )
}

/// Spec 101 Task 3: a parent's pending worker returns plus the engine's own
/// outcome model for each returned task.
///
/// The durable return keeps its literal `taskStatus` (`incomplete` stays
/// `incomplete`); the added `control` is what the result card draws. Reading
/// the worker's completion report is transport preparation, not a second
/// decision: the labels come from `returned_task_control`.
pub(crate) fn worker_returns_json(engine: &WorkspaceEngine, returns: &[WorkerReturn]) -> String {
    let views: Vec<serde_json::Value> = returns
        .iter()
        .map(|worker_return| {
            let report = engine
                .session_store
                .read_completion_report(&worker_return.worker_session_id, &worker_return.task_id)
                .ok()
                .flatten();
            let control = workspace_engine::returned_task_control_with_outcome(
                &worker_return.task_status,
                report.as_ref(),
                worker_return.terminal_outcome.as_ref(),
            );
            let mut value = serde_json::to_value(worker_return)
                .unwrap_or_else(|_| serde_json::json!({"returnId": worker_return.return_id}));
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "control".to_string(),
                    serde_json::to_value(control).unwrap_or(serde_json::Value::Null),
                );
            }
            value
        })
        .collect();
    serde_json::to_string(&views).unwrap_or_else(|_| "[]".to_string())
}

/// The display half of conversation compaction (spec 55 §5.9): every active
/// summary, and where the model's view begins.
///
/// The transcript itself is unchanged — `messages_json` above still carries
/// every message — so the marker sits *between* messages and the originals are
/// below it.
pub(crate) fn conversation_json(engine: &WorkspaceEngine, session_id: &str) -> String {
    let compactions = engine
        .session_store
        .read_conversation_compactions(session_id)
        .unwrap_or_default();
    let conversation = engine
        .session_store
        .read_conversation(session_id)
        .unwrap_or_default();
    let sequenced = engine
        .session_store
        .read_messages_with_seq(session_id)
        .unwrap_or_default();
    let previous_through = conversation
        .summary
        .as_ref()
        .map(|summary| summary.through_seq)
        .unwrap_or(0);
    // The prompt filters the engine's own plan reminder before slicing, so the
    // counts here must use the same definition; otherwise the marker reports a
    // window the model never had.
    let window_tokens = engine.chat_orchestrator.conversation_window_tokens();
    let after_summary_sendable = sequenced
        .iter()
        .filter(|(seq, _)| *seq > previous_through)
        .map(|(_, message)| message)
        .collect::<Vec<_>>();
    let sendable = workspace_engine::compaction::sendable_messages(&after_summary_sendable);
    let window_len = workspace_engine::compaction::window_slice(&sendable, window_tokens).len();
    let outside =
        workspace_engine::compaction::window_start(&sequenced, previous_through, window_tokens);
    let (kind, through_seq, omitted) = if let Some(summary) = &conversation.summary {
        (
            "summary",
            summary.through_seq,
            conversation.summarised_messages + (sendable.len() - window_len),
        )
    } else if outside.is_some() {
        (
            "elision",
            outside.unwrap_or(0),
            sendable.len().saturating_sub(window_len),
        )
    } else {
        ("none", 0, 0)
    };
    serde_json::json!({
        "compactions": compactions,
        "conversation": {
            "kind": kind,
            "throughSeq": through_seq,
            "omittedMessages": omitted,
            "windowMessages": window_len,
            "autoCompact": engine.config.conversation_auto_compact,
            "windowShare": engine.config.conversation_window_share,
            "canCompact": outside.is_some(),
            "lastFailure": engine
                .session_store
                .read_conversation_compaction_failure(session_id)
                .ok()
                .flatten()
        }
    })
    .to_string()
}

/// `POST /api/compact` — the manual control (spec 55 requirement 4).
///
/// It takes no approval and touches no file. The engine appends one event or
/// nothing; the response says which, so the UI can report it.
pub(crate) fn handle_compact(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let body = run_compact_request(&repo, &session_id, &form)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// The body of the manual compaction route, split out so the refusal path is
/// testable without a stream.
pub(crate) fn run_compact_request(
    repo: &str,
    session_id: &str,
    form: &HashMap<String, String>,
) -> Result<String, String> {
    let (config, report) = config_for_repo_with_model_options(repo, form)?;
    let engine = WorkspaceEngine::new_with_report(config, &report);
    compact_session(&engine, repo, session_id)
}

/// The manual compaction against an already-built engine, so the refusal paths
/// are testable without a stream or a re-loaded config.
pub(crate) fn compact_session(
    engine: &WorkspaceEngine,
    repo: &str,
    session_id: &str,
) -> Result<String, String> {
    let Some(session) = engine
        .session_store
        .read_session(session_id)
        .map_err(|error| error.to_string())?
    else {
        return Err(format!("Unknown session: {session_id}"));
    };
    let repository_id = engine
        .indexer
        .repository_id_for_path(repo)
        .map_err(|error| error.to_string())?;
    if session.repository_id != repository_id {
        return Err("Session belongs to a different repository".to_string());
    }
    // Compaction runs at a turn boundary. A manual click during a turn would
    // land inside an action span, so it waits; automatic compaction is already
    // before the request by construction.
    let statuses = engine
        .session_store
        .read_task_statuses(session_id)
        .map_err(|error| error.to_string())?;
    if statuses.values().any(|status| {
        workspace_engine::TaskStatus::parse(status).is_some_and(|status| status.is_active())
    }) {
        return Err("A turn is running in this session; compact after it finishes.".to_string());
    }
    let mut adapter = chat_model_adapter(&engine.config, || {
        resolve_shell_model_api_key(&engine.config)
    })?;
    let cancel = CancelToken::new();
    let body = match engine.chat_orchestrator.compact_conversation(
        Path::new(repo),
        session_id,
        adapter.as_mut(),
        &cancel,
    ) {
        Ok(workspace_engine::compaction::CompactionOutcome::Compacted(summary)) => {
            serde_json::json!({
                "kind": "compacted",
                "summaryId": summary.id,
                "throughSeq": summary.through_seq,
                "messageCount": summary.message_count,
                "factsOnly": summary.is_facts_only(),
                "narrativeCarriedForward": summary.narrative_carried_forward,
            })
        }
        Ok(workspace_engine::compaction::CompactionOutcome::NothingToCompact) => {
            serde_json::json!({"kind": "nothing"})
        }
        Ok(workspace_engine::compaction::CompactionOutcome::Cancelled) => {
            serde_json::json!({"kind": "cancelled"})
        }
        Ok(workspace_engine::compaction::CompactionOutcome::Failed(reason)) => {
            serde_json::json!({"kind": "failed", "reason": reason})
        }
        Err(error) => serde_json::json!({"kind": "failed", "reason": error.to_string()}),
    };
    Ok(body.to_string())
}

pub(crate) fn handle_session_rename(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let title = required_form(&form, "title")?;
    let engine = default_engine()?;
    let session = engine
        .session_store
        .rename_session(&session_id, &title)
        .map_err(|error| error.to_string())?;
    let mode = engine.session_store.session_mode(&session.id);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"session\":{}}}", session_json(&session, mode)),
    )
}

pub(crate) fn handle_session_mode(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let requested = required_form(&form, "mode")?;
    // Rejected, never defaulted: falling back to Code would turn a
    // malformed request into the most permissive mode there is.
    let mode = SessionMode::parse(&requested).ok_or_else(|| {
        format!("Unknown mode: {requested}. Expected ask, plan, code, or review.")
    })?;
    let engine = default_engine()?;
    if engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err(format!("Unknown session: {session_id}"));
    }
    // Always "user": this endpoint is the user's own selection, and
    // nothing the model emits reaches it (spec 20 requirement 4).
    engine
        .session_store
        .set_session_mode(&session_id, mode, "user")
        .map_err(|error| error.to_string())?;
    let Some(session) = engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
    else {
        return Err(format!("Unknown session: {session_id}"));
    };
    let mode = engine.session_store.session_mode(&session.id);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"session\":{}}}", session_json(&session, mode)),
    )
}

/// Adds, removes or clears a session pin (spec 27 §5.4).
///
/// `action` is required and unknown values are refused rather than defaulted:
/// a typo must not turn into a clear.
pub(crate) fn handle_context_pin(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let action = required_form(&form, "action")?;
    let engine = default_engine()?;
    if engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err(format!("Unknown session: {session_id}"));
    }
    match action.as_str() {
        "add" => {
            let path = required_form(&form, "path")?;
            let range = parse_context_range(&form)?;
            engine
                .session_store
                .add_context_pin(&session_id, &path, range)
                .map_err(|error| error.to_string())?;
        }
        "remove" => {
            let path = required_form(&form, "path")?;
            let range = parse_context_range(&form)?;
            engine
                .session_store
                .remove_context_pin(&session_id, &path, range)
                .map_err(|error| error.to_string())?;
        }
        "clear" => engine
            .session_store
            .clear_context_pins(&session_id)
            .map_err(|error| error.to_string())?,
        other => return Err(format!("Unknown context pin action: {other}")),
    }
    let pins = engine
        .session_store
        .context_pins(&session_id)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"pins\":{}}}",
            serde_json::to_string(&pins).unwrap_or_else(|_| "[]".to_string())
        ),
    )
}

/// `start`/`end` as an optional 1-based inclusive range. Both absent is a
/// whole-file pin; one absent or `start > end` is a malformed request.
fn parse_context_range(form: &HashMap<String, String>) -> Result<Option<LineRange>, String> {
    let start = form
        .get("start")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "start must be a line number".to_string())
        })
        .transpose()?;
    let end = form
        .get("end")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "end must be a line number".to_string())
        })
        .transpose()?;
    match (start, end) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) if start >= 1 && start <= end => {
            Ok(Some(LineRange { start, end }))
        }
        _ => Err("A context pin range needs start and end with 1 <= start <= end".to_string()),
    }
}

/// Sets or clears the session's path restriction (spec 27 §5.4).
pub(crate) fn handle_context_restriction(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let engine = default_engine()?;
    if engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err(format!("Unknown session: {session_id}"));
    }
    if form.get("clear").map(String::as_str) == Some("true") {
        engine
            .session_store
            .clear_context_restriction(&session_id)
            .map_err(|error| error.to_string())?;
    } else {
        let paths = form
            .get("paths")
            .map(|value| {
                value
                    .lines()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        engine
            .session_store
            .set_context_restriction(&session_id, &paths)
            .map_err(|error| error.to_string())?;
    }
    let restriction = engine
        .session_store
        .context_restriction(&session_id)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"restrictedPaths\":{}}}",
            serde_json::to_string(&restriction).unwrap_or_else(|_| "null".to_string())
        ),
    )
}

/// Removes one optional item from a paused review (spec 27 §5.4).
pub(crate) fn handle_context_remove(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let proposal_id = required_form(&form, "proposal_id")?;
    let index = required_form(&form, "index")?
        .parse::<usize>()
        .map_err(|_| "index must be a number".to_string())?;
    let engine = default_engine()?;
    let manifest = engine
        .chat_orchestrator
        .remove_context_item(&proposal_id, index)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"manifest\":{}}}",
            serde_json::to_string(&manifest).unwrap_or_else(|_| "null".to_string())
        ),
    )
}

/// Removes the conversation summary from a paused review (spec 27 §5.4).
pub(crate) fn handle_context_remove_summary(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let proposal_id = required_form(&form, "proposal_id")?;
    let engine = default_engine()?;
    let manifest = engine
        .chat_orchestrator
        .remove_context_summary(&proposal_id)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"manifest\":{}}}",
            serde_json::to_string(&manifest).unwrap_or_else(|_| "null".to_string())
        ),
    )
}

/// Resolves the memory rows an inspector manifest names (spec 30 §5.4, C17).
///
/// The manifest carries entry ids, match reasons and recall-time verification,
/// never statements (C3), so the panel posts the rows it was shown and the
/// engine reads the statements from the scopes this repository and session own.
/// A client cannot send a statement, and an id from another project resolves to
/// `deleted` rather than exposing it.
pub(crate) fn handle_context_memory(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let references: Vec<MemoryRecallRef> = match form.get("memory") {
        Some(value) if !value.is_empty() => serde_json::from_str(value)
            .map_err(|_| "memory must be a JSON list of recalled entries".to_string())?,
        _ => Vec::new(),
    };
    let engine = engine_for_repo(&repo)?;
    require_memory_session_in_repository(&engine, &session_id, &repo)?;
    let entries = engine
        .chat_orchestrator
        .resolve_memory_display(Path::new(&repo), &session_id, &references)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"entries\":{}}}",
            serde_json::to_string(&entries).map_err(|error| error.to_string())?
        ),
    )
}

/// Spec 57's routes. Each answers from files or one short socket exchange
/// with a worker this shell started (`workers.rs`), and every one but
/// `GET /api/workers` and `GET /api/workers-enabled` refuses while
/// `enable_worker_sessions` is off.
pub(crate) fn handle_worker_route(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let form = parse_form(&request.body);
    let answer = workers::route(
        workers::global(),
        &config,
        &|config: &Config| resolve_shell_model_api_key(config),
        &workers::WorkerRequest {
            method: &request.method,
            path: &request.path,
            query: &request.query,
            form: &form,
        },
    );
    match answer {
        Ok(body) => write_response(stream, request, 200, "application/json", &body),
        Err(refusal) => write_response(
            stream,
            request,
            refusal.status,
            "application/json",
            &json_error(&refusal.message),
        ),
    }
}

/// `POST /api/session-archive` and `/api/session-unarchive` (spec 108 D3).
pub(crate) fn handle_session_archive(
    stream: &mut TcpStream,
    request: &Request,
    archive: bool,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    if archive && let Some(refusal) = working_session_refusal(&session_id, "archive") {
        return write_response(stream, request, 409, "application/json", &refusal);
    }
    let engine = default_engine()?;
    // Only the supervisor knows whether a worker's process is still alive.
    let worker_running = archive && workers::global().is_alive(&session_id);
    let (status, body) = session_archive_answer(&engine, &session_id, archive, worker_running);
    write_response(stream, request, status, "application/json", &body)
}

/// `POST /api/command-session-approval` (spec 109 D5): **Approve for this
/// session** of a command, check or baseline card's proposal. Records the
/// session approval of the exact command in its exact directory, after the
/// engine checks it qualifies under the repository's config; the client then
/// approves the card as usual, and that run is the first one it covers.
pub(crate) fn handle_command_session_approval(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let proposal_id = required_form(&form, "proposal_id")?;
    let repo = required_form(&form, "repo")?;
    let engine = engine_for_repo(&repo)?;
    let (status, body) = match engine.validation_orchestrator.approve_for_session(
        &session_id,
        &proposal_id,
        "desktop_user",
    ) {
        Ok(approval) => (
            200,
            serde_json::json!({
                "approved": true,
                "command": approval.command,
                "directory": approval.directory,
            })
            .to_string(),
        ),
        Err(error) => (
            409,
            serde_json::json!({ "error": error.to_string() }).to_string(),
        ),
    };
    write_response(stream, request, status, "application/json", &body)
}

/// `POST /api/session-approvals-revoke` (spec 109 D5): **Revoke session
/// approvals** from the session ⋮ menu. Answers how many it ended.
pub(crate) fn handle_session_approvals_revoke(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let engine = match form.get("repo").filter(|repo| !repo.is_empty()) {
        Some(repo) => engine_for_repo(repo)?,
        None => default_engine()?,
    };
    let revoked = engine
        .validation_orchestrator
        .revoke_session_approvals(&session_id, "desktop_user")
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::json!({ "revoked": revoked }).to_string(),
    )
}

/// Spec 110 requirement 3: archive and delete are the only actions refused on
/// a working session. The body names what it is doing, so the user knows what
/// to stop. Checked before anything is read or written.
fn working_session_refusal(session_id: &str, action: &str) -> Option<String> {
    let running = lanes::working_label(session_id)?;
    Some(
        serde_json::json!({
            "error": format!("This session is working ({running}). Stop it first, then {action} it."),
            "code": "session_working",
            "sessionId": session_id,
            "label": running,
        })
        .to_string(),
    )
}

/// `GET /api/working-sessions` (spec 110 D3): every session with work running,
/// what it is doing, for how long, and whether it can be cancelled.
/// The ids `/api/working-sessions` lists, for `/api/sessions`' `state`.
fn working_session_ids() -> std::collections::HashSet<String> {
    lanes::working()
        .into_iter()
        .map(|row| row.session_id)
        .collect()
}

pub(crate) fn handle_working_sessions(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let sessions = lanes::working()
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "sessionId": row.session_id,
                "label": row.label,
                "elapsedMs": row.elapsed_ms as u64,
                "cancellable": row.cancellable,
            })
        })
        .collect::<Vec<_>>();
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::json!({ "sessions": sessions }).to_string(),
    )
}

/// `POST /api/session-work-cancel` (spec 110 D3): stops a session's running
/// work at its next safe point, exactly as closing the window that started it
/// does. A UI route, so it is answered while that work runs. It never resolves
/// an approval: a turn paused for one is not running and holds no lane.
pub(crate) fn handle_cancel_session_work(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let (status, body) = match lanes::cancel(&session_id) {
        Ok(()) => (
            200,
            serde_json::json!({ "sessionId": session_id, "status": "cancelling" }).to_string(),
        ),
        Err(lanes::CancelRefusal::Idle) => (409, json_error("This session is not working.")),
        Err(lanes::CancelRefusal::NotCancellable) => (
            409,
            json_error("This work cannot be stopped part-way; it ends by itself."),
        ),
    };
    write_response(stream, request, status, "application/json", &body)
}

/// `POST /api/session-note` (spec 110 D7): a message sent while the session
/// works. It is queued into the running turn, which gives it to the model at
/// its next round boundary as the user's own message. A UI route, so it is
/// answered while that turn runs. It only ever enters a running turn: a turn
/// paused for an approval or a question is not running, and gets
/// `not_working` here, so a note can never answer or approve anything.
pub(crate) fn handle_session_note(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let text = form.get("text").map(|text| text.trim()).unwrap_or_default();
    let (status, body) = if text.is_empty() {
        (400, json_error("A note needs text."))
    } else if text.len() > worker::MAX_NOTE_BYTES {
        (
            400,
            json_error(&format!(
                "A note is limited to {} KB.",
                worker::MAX_NOTE_BYTES / 1024
            )),
        )
    } else if lanes::queue_note(&session_id, text.to_string()) {
        (
            200,
            serde_json::json!({ "sessionId": session_id, "status": "queued" }).to_string(),
        )
    } else {
        (
            409,
            serde_json::json!({
                "error": "This session has no running turn to take the message.",
                "code": "not_working",
                "sessionId": session_id,
            })
            .to_string(),
        )
    };
    write_response(stream, request, status, "application/json", &body)
}

/// The status and body of an archive or unarchive request. Split from the
/// route so the running-worker refusal is testable without a live worker.
pub(crate) fn session_archive_answer(
    engine: &WorkspaceEngine,
    session_id: &str,
    archive: bool,
    worker_running: bool,
) -> (u16, String) {
    let result = if archive {
        engine
            .session_store
            .archive_session(session_id, worker_running)
    } else {
        engine.session_store.unarchive_session(session_id)
    };
    match result {
        Ok(session) => {
            let mode = engine.session_store.session_mode(&session.id);
            (
                200,
                format!("{{\"session\":{}}}", session_json(&session, mode)),
            )
        }
        // A conflict, not a bad request: the same request succeeds once the
        // worker has stopped.
        Err(error) if worker_running => (409, json_error(&error.to_string())),
        Err(error) => (400, json_error(&error.to_string())),
    }
}

pub(crate) fn handle_session_delete(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    if let Some(refusal) = working_session_refusal(&session_id, "delete") {
        return write_response(stream, request, 409, "application/json", &refusal);
    }
    // A `session_end` hook runs in the repository, and a session stores only
    // its repository id, so the caller says which checkout this session
    // belongs to (spec 32 Task 6).
    let repo = required_form(&form, "repo")?;
    // Loaded for the repository, not with `default_engine`: a repository hook
    // the user has reviewed should see `session_end` as a user hook does, and
    // only a repository-scoped load carries it.
    let engine = engine_for_repo(&repo)?;
    // Before the delete: the children are recorded in the log it removes.
    for worker in workers::workers_to_stop_for_deleted_session(&engine.session_store, &session_id) {
        workers::global().shutdown(&worker);
    }
    engine
        .delete_session(&session_id, &repo)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"sessionId\":\"{}\",\"status\":\"deleted\"}}",
            escape_json(&session_id)
        ),
    )
}

pub(crate) fn handle_sessions(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let engine = engine_for_repo(&repo)?;
    let repository_id = engine
        .indexer
        .repository_id_for_path(&repo)
        .map_err(|error| error.to_string())?;
    // `archived=exclude|only|include` (spec 108 D3); without it every session
    // comes back, each carrying its `archived` flag.
    let archived = match request.param("archived") {
        Some(value) => workspace_engine::ArchivedFilter::parse(&value)
            .ok_or_else(|| format!("archived must be include, exclude or only, not {value}"))?,
        None => workspace_engine::ArchivedFilter::Include,
    };
    let sessions = engine
        .session_store
        .list_sessions_filtered(Some(&repository_id), archived)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"sessions\":[{}]}}",
            sessions_json(&engine.session_store, &sessions, &working_session_ids())
        ),
    )
}

pub(crate) fn handle_session_search(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let query = required_param(request, "query")?;
    let whole_word = request.param("whole_word").as_deref() == Some("true");
    let literal_phrase = request.param("literal").as_deref() != Some("false");
    let max_results = request
        .param("max")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(50);
    let (engine, repository_id) = if request.param("scope").as_deref() == Some("all") {
        (default_engine()?, None)
    } else {
        let repo = required_param(request, "repo")?;
        let engine = engine_for_repo(&repo)?;
        let repository_id = engine
            .indexer
            .repository_id_for_path(&repo)
            .map_err(|error| error.to_string())?;
        (engine, Some(repository_id))
    };
    let result = engine
        .session_store
        .search_sessions(
            repository_id.as_deref(),
            &query,
            SearchOptions {
                whole_word,
                literal_phrase,
                max_results,
            },
            &engine.scanner,
        )
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::to_string(&result).map_err(|error| error.to_string())?,
    )
}

pub(crate) fn handle_checkpoints(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let engine = engine_for_repo(&repo)?;
    let repository_id = engine
        .indexer
        .repository_id_for_path(&repo)
        .map_err(|error| error.to_string())?;
    let mut checkpoints = engine
        .checkpoint_store
        .list_checkpoints(&repository_id)
        .map_err(|error| error.to_string())?;
    if let Some(session_id) = request.param("session_id") {
        checkpoints.retain(|manifest| manifest.session_id == session_id);
    }
    write_response(
        stream,
        request,
        200,
        "application/json",
        &checkpoint_list_json(&checkpoints),
    )
}

pub(crate) fn handle_rewind(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let checkpoint_id = required_form(&form, "checkpoint_id")?;
    let files = form.get("files").map(String::as_str) != Some("false");
    let conversation = form.get("conversation").map(String::as_str) == Some("true");
    let only_path = form
        .get("path")
        .map(String::as_str)
        .filter(|path| !path.is_empty());
    if !files && !conversation {
        return Err("Choose files, conversation, or both".to_string());
    }
    let engine = engine_for_repo(&repo)?;
    let repository_id = engine
        .indexer
        .repository_id_for_path(&repo)
        .map_err(|error| error.to_string())?;
    let manifest = engine
        .checkpoint_store
        .read_checkpoint(&repository_id, &checkpoint_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Unknown checkpoint: {checkpoint_id}"))?;
    let result = engine
        .checkpoint_store
        .restore(
            &repo,
            &manifest,
            workspace_engine::CheckpointRestoreOptions {
                files,
                conversation,
                only_path,
            },
            "desktop_user",
        )
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &checkpoint_restore_json(&result),
    )
}

pub(crate) fn handle_session_create(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let title = form
        .get("title")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "New session".to_string());
    let engine = engine_for_repo(&repo)?;
    let repository_id = engine
        .indexer
        .repository_id_for_path(&repo)
        .map_err(|error| error.to_string())?;
    let session = engine
        .session_store
        .create_session(&repository_id, &title)
        .map_err(|error| error.to_string())?;
    let mode = engine.session_store.session_mode(&session.id);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"session\":{}}}", session_json(&session, mode)),
    )
}

pub(crate) fn handle_context_file(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let path = required_form(&form, "path")?;
    let engine = engine_for_repo(&repo)?;
    let files = validate_context_files(&engine, &repo, &path)?;
    let Some(path) = files.first() else {
        return Err("context file is required".to_string());
    };
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"path\":\"{}\"}}", escape_json(path)),
    )
}
