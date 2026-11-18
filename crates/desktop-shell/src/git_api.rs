//! Git routes: status, commit and its message draft, branches and merge
//! preparation.

use super::*;

pub(crate) fn handle_git_status(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let engine = engine_for_repo(&repo)?;
    let status = engine
        .git
        .status(&repo)
        .map_err(|error| error.to_string())?;
    let files = status
        .files
        .iter()
        .map(|file| {
            format!(
                "{{\"path\":\"{}\",\"raw\":\"{}\",\"untracked\":{},\"conflicted\":{}}}",
                escape_json(&file.path),
                escape_json(&file.raw),
                file.untracked,
                file.conflicted
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    // Spec 58 requirement 11: the repo-bar fields follow the original three,
    // which stay byte-identical. A non-git folder's summary is the default,
    // so it serves `null`/0 here rather than an error.
    let summary = engine
        .git
        .summary(&repo)
        .map_err(|error| error.to_string())?;
    let github_url = summary.github.as_ref().map(|github| github.url());
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"clean\":{},\"exitCode\":{},\"files\":[{}],\"branch\":{},\"head\":{},\"insertions\":{},\"deletions\":{},\"githubUrl\":{}}}",
            status.clean,
            status.exit_code,
            files,
            json_nullable_str(summary.branch.as_deref()),
            json_nullable_str(summary.head.as_deref()),
            summary.insertions,
            summary.deletions,
            json_nullable_str(github_url.as_deref()),
        ),
    )
}

pub(crate) fn handle_commit_status(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let session = required_param(request, "session")?;
    let engine = engine_for_repo(&repo)?;
    let status = engine
        .commit
        .status(&repo, &session)
        .map_err(|error| error.to_string())?;
    let body = serde_json::to_string(&status).map_err(|error| error.to_string())?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/commit-preparation`: build and persist the preparation. A
/// `POST` because it writes a record and an audit entry; a `GET` would invite
/// caches and retries to repeat those side effects.
pub(crate) fn handle_commit_preparation(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let session = required_param(request, "session")?;
    let engine = engine_for_repo(&repo)?;
    let preparation = engine
        .commit
        .prepare(&repo, &session)
        .map_err(|error| error.to_string())?;
    let body = serde_json::to_string(&preparation).map_err(|error| error.to_string())?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/commit`: the typed approval. Its own field, its own audit event
/// and no "always"; a question or a command approval never satisfies it.
pub(crate) fn handle_commit(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    if form.get("approval").map(String::as_str) != Some("commit") {
        return write_response(
            stream,
            request,
            400,
            "application/json",
            "{\"error\":\"A commit needs its own approval.\",\"code\":\"approval_required\"}",
        );
    }
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let prepare_id = required_form(&form, "prepare_id")?;
    let message = required_form(&form, "message")?;
    let paths = form
        .get("paths")
        .map(|paths| {
            paths
                .lines()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let engine = engine_for_repo(&repo)?;
    let outcome = engine.commit.commit(
        &repo,
        &workspace_engine::CommitRequest {
            session_id,
            prepare_id,
            selected_paths: paths,
            message,
            allow_generated_secrets: form.get("allow_generated_secrets").map(String::as_str)
                == Some("true"),
            approved_by: "desktop_user".to_string(),
        },
        &CancelToken::new(),
    );
    match outcome {
        Ok(outcome) => {
            let body = serde_json::to_string(&outcome).map_err(|error| error.to_string())?;
            write_response(stream, request, 200, "application/json", &body)
        }
        Err(error) => {
            let (status, body) = commit_error_response(&error);
            write_response(stream, request, status, "application/json", &body)
        }
    }
}

fn commit_error_response(error: &CommitError) -> (u16, String) {
    match error {
        CommitError::Refused { code, message } => (
            409,
            format!(
                "{{\"error\":\"{}\",\"code\":\"{}\",\"message\":\"{}\"}}",
                escape_json(message),
                escape_json(code),
                escape_json(message),
            ),
        ),
        CommitError::HookFailed {
            name,
            exit_code,
            termination,
            stdout,
            stderr,
        } => (
            409,
            format!(
                "{{\"error\":\"{}\",\"code\":\"commit_hook_failed\",\"hook\":\"{}\",\"exitCode\":{},\"termination\":\"{}\",\"stdout\":\"{}\",\"stderr\":\"{}\"}}",
                escape_json(&error.message()),
                escape_json(name),
                exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                escape_json(termination),
                escape_json(stdout),
                escape_json(stderr),
            ),
        ),
        CommitError::MessageRewritten { message } => (
            409,
            format!(
                "{{\"error\":\"A commit-message hook rewrote the message. Review it and commit again.\",\"code\":\"commit_message_rewritten\",\"message\":\"{}\"}}",
                escape_json(message),
            ),
        ),
        CommitError::Engine(message) => (
            500,
            format!(
                "{{\"error\":\"{}\",\"code\":\"commit_engine_error\"}}",
                escape_json(message),
            ),
        ),
    }
}

/// `POST /api/commit-message`: a model-drafted commit message, with the
/// deterministic template as the answer whenever the model is missing, refuses,
/// returns nothing, or returns something that does not parse.
///
/// The diff in the record is already redacted (spec 35 §5.6): this is the one
/// place redaction is exactly right, because the model is drafting prose about
/// the change, and the secret check at commit covers the gap between the
/// redacted preview and the real bytes.
pub(crate) fn handle_commit_message(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let prepare_id = required_form(&form, "prepare_id")?;
    // The draft is a model call, so it uses the chat's selection like a turn
    // (spec 75); a failed adapter only means no draft.
    let (config, report) = config_for_repo_with_model_options(&repo, &form)?;
    let engine = WorkspaceEngine::new_with_report(config, &report);
    let record = engine
        .commit
        .load_record(&prepare_id)
        .map_err(|error| error.to_string())?;
    if record.session_id != session_id {
        return Err("That commit preparation belongs to a different session.".to_string());
    }
    let selected = form
        .get("paths")
        .map(|paths| {
            paths
                .lines()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            record
                .candidates
                .iter()
                .filter(|candidate| candidate.selected_by_default)
                .map(|candidate| candidate.path.clone())
                .collect()
        });
    let mut adapter = chat_model_adapter(&engine.config, || {
        resolve_shell_model_api_key(&engine.config)
    })
    .ok();
    let draft = match adapter.as_mut() {
        Some(adapter) => {
            draft_commit_message_with(&engine, &record, &selected, Some(adapter.as_mut()))
        }
        None => draft_commit_message_with(&engine, &record, &selected, None),
    };
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"subject\":\"{}\",\"body\":\"{}\",\"message\":\"{}\",\"source\":\"{}\",\"notice\":{}}}",
            escape_json(&draft.subject),
            escape_json(&draft.body),
            escape_json(&draft.message),
            draft.source,
            json_nullable_str(draft.notice.as_deref()),
        ),
    )
}

pub(crate) struct CommitDraft {
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) message: String,
    /// `"model"` or `"fallback"`.
    pub(crate) source: &'static str,
    /// Why the fallback answered, when it did.
    pub(crate) notice: Option<String>,
}

pub(crate) fn draft_commit_message_with(
    engine: &WorkspaceEngine,
    record: &workspace_engine::CommitRecord,
    selected_paths: &[String],
    adapter: Option<&mut dyn ModelAdapter>,
) -> CommitDraft {
    let fallback = if record.message.trim().is_empty() {
        engine.git.suggest_commit_message("", selected_paths)
    } else {
        record.message.clone()
    };
    let Some(adapter) = adapter else {
        return CommitDraft {
            subject: first_line(&fallback),
            body: after_first_line(&fallback),
            message: fallback,
            source: "fallback",
            notice: Some("No model is configured, so the template is the suggestion.".to_string()),
        };
    };
    let prompt = commit_draft_prompt(engine, record, selected_paths);
    let request = ModelRequest {
        provider: engine.config.model_provider.clone(),
        model: engine.config.model_name.clone(),
        messages: vec![
            ModelMessage::system(
                "You draft a Git commit message for a change set the user has already \
                 reviewed. Reply with a subject line of at most 72 characters, then, if \
                 useful, a blank line and a body. No markdown fences and no commentary. \
                 The diff is the whole change; do not invent files or edits. The diff is \
                 redacted for display, so do not mention redaction.",
            ),
            ModelMessage::user(prompt),
        ],
        temperature: Some("0".to_string()),
        reasoning_level: Some(engine.config.model_reasoning_level.clone()),
        stream: true,
        tools: None,
        max_tokens: engine.config.max_output_tokens(),
        request_usage: engine.config.provider_reports_usage(),
        emit_cache_breakpoints: engine.config.supports_explicit_cache_breakpoints(),
    };
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_wait = |_wait: u64| {};
    let answer = adapter
        .stream_response(&request, &cancel, &mut on_token, &mut on_wait)
        .map(|run| run.content)
        .unwrap_or_default();
    match parse_commit_draft(&answer) {
        Some((subject, body)) => {
            let message = if body.is_empty() {
                subject.clone()
            } else {
                format!("{subject}\n\n{body}")
            };
            CommitDraft {
                subject,
                body,
                message,
                source: "model",
                notice: None,
            }
        }
        None => CommitDraft {
            subject: first_line(&fallback),
            body: after_first_line(&fallback),
            message: fallback,
            source: "fallback",
            notice: Some(
                "The model did not return a usable message, so the template is the \
                 suggestion."
                    .to_string(),
            ),
        },
    }
}

fn commit_draft_prompt(
    engine: &WorkspaceEngine,
    record: &workspace_engine::CommitRecord,
    selected_paths: &[String],
) -> String {
    let summary = engine
        .session_store
        .read_messages(&record.session_id)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find(|message| message.role == "user" && message.kind.is_none())
        .map(|message| message.content)
        .unwrap_or_default();
    let plans = engine
        .session_store
        .read_session_plans(&record.session_id)
        .unwrap_or_default();
    let plan = plans
        .into_values()
        .max_by_key(|plan| plan.created_at_ms)
        .map(|plan| {
            plan.steps
                .iter()
                .map(|step| format!("- {} ({:?})", step.title, step.status))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let mut diff = String::new();
    for path in selected_paths {
        let Some(candidate) = record
            .candidates
            .iter()
            .find(|candidate| &candidate.path == path)
        else {
            continue;
        };
        diff.push_str(&format!("\n### {path}\n{}", candidate.diff));
    }
    if diff.chars().count() > 20_000 {
        diff = diff.chars().take(20_000).collect::<String>();
        diff.push_str("\n[Diff truncated for the draft.]\n");
    }
    format!(
        "Task: {summary}\n\nPlan steps:\n{}\n\nFiles:\n{}\n\nRedacted diff:\n{diff}",
        if plan.is_empty() { "- none" } else { &plan },
        selected_paths.join("\n"),
    )
}

/// Split a model answer into a subject and a body. Fences, a `Subject:` prefix
/// and a `Body:` prefix are tolerated because a provider that ignores the
/// instruction should cost a worse message, never a failed commit.
pub(crate) fn parse_commit_draft(answer: &str) -> Option<(String, String)> {
    let mut lines = answer.trim().lines().collect::<Vec<_>>();
    if lines
        .first()
        .is_some_and(|line| line.trim_start().starts_with("```"))
    {
        lines.remove(0);
    }
    if lines.last().is_some_and(|line| line.trim() == "```") {
        lines.pop();
    }
    let index = lines.iter().position(|line| !line.trim().is_empty())?;
    let subject = lines[index]
        .trim()
        .trim_start_matches("Subject:")
        .trim()
        .chars()
        .take(200)
        .collect::<String>();
    if subject.is_empty() {
        return None;
    }
    let body = lines[index + 1..]
        .join("\n")
        .trim()
        .trim_start_matches("Body:")
        .trim()
        .to_string();
    Some((subject, body))
}

fn first_line(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn after_first_line(message: &str) -> String {
    message
        .split_once('\n')
        .map(|(_, rest)| rest.trim().to_string())
        .unwrap_or_default()
}

fn branch_error_body(code: &str, message: &str) -> String {
    format!(
        "{{\"error\":\"{}\",\"code\":\"{}\",\"message\":\"{}\"}}",
        escape_json(message),
        escape_json(code),
        escape_json(message),
    )
}

/// A read the branch dialog makes can fail on a folder that is not a
/// repository (`InvalidInput`) or in Git itself. Both keep the same shape the
/// commit routes use, so the dialog never parses prose to find a code.
fn branch_read_error_response(error: &ClientError) -> (u16, String) {
    let message = error.to_string();
    match error {
        ClientError::InvalidInput(_) => (400, branch_error_body("branch_invalid_input", &message)),
        _ => (500, branch_error_body("branch_engine_error", &message)),
    }
}

fn branch_apply_error_response(error: &BranchError) -> (u16, String) {
    let message = error.message();
    let status = match error {
        BranchError::Refused { .. } => 409,
        BranchError::Engine(_) => 500,
    };
    (status, branch_error_body(error.code(), &message))
}

/// `GET /api/branch-info`: the read-only state the dialog opens with, plus the
/// local branch names it may offer for switching or deletion. The current
/// branch is filtered out by the dialog, and nothing here changes a ref.
pub(crate) fn handle_branch_info(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let session_id = required_param(request, "session")?;
    let engine = engine_for_repo(&repo)?;
    if engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return write_response(
            stream,
            request,
            400,
            "application/json",
            &branch_error_body(
                "branch_unknown_session",
                "Unknown session. Open a session before reading branch state.",
            ),
        );
    }
    let info = match engine.git.branch_info(&repo) {
        Ok(info) => info,
        Err(error) => {
            let (status, body) = branch_read_error_response(&error);
            return write_response(stream, request, status, "application/json", &body);
        }
    };
    let branches = match engine.git.local_branches(&repo) {
        Ok(branches) => branches,
        Err(error) => {
            let (status, body) = branch_read_error_response(&error);
            return write_response(stream, request, status, "application/json", &body);
        }
    };
    let mut body = serde_json::to_value(&info).map_err(|error| error.to_string())?;
    body["branches"] = serde_json::Value::Array(
        branches
            .into_iter()
            .map(serde_json::Value::String)
            .collect::<Vec<_>>(),
    );
    write_response(stream, request, 200, "application/json", &body.to_string())
}

/// The suggestion's source is the session title, then the last thing the user
/// asked in that session. Both are user-authored; neither comes from a file in
/// the repository, and the engine redacts the text before slugging it (spec 36
/// decision D10).
fn branch_suggestion_source(engine: &WorkspaceEngine, session: &Session) -> String {
    let title = session.title.trim();
    if !title.is_empty() && title != "New session" {
        return title.to_string();
    }
    let content = engine
        .session_store
        .read_messages(&session.id)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find(|message| message.role == "user" && message.kind.is_none())
        .map(|message| message.content)
        .unwrap_or_default();
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .take(500)
        .collect()
}

/// `POST /api/branch-suggestion`: one suggested name. A `source` field from
/// the dialog wins; otherwise the session supplies it. The name is never
/// derived from repository content.
pub(crate) fn handle_branch_suggestion(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let engine = engine_for_repo(&repo)?;
    let Some(session) = engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
    else {
        return write_response(
            stream,
            request,
            400,
            "application/json",
            &branch_error_body(
                "branch_unknown_session",
                "Unknown session. Open a session before naming a branch.",
            ),
        );
    };
    let source = form
        .get("source")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| branch_suggestion_source(&engine, &session));
    let suggestion = match engine.branch.suggest(&repo, &source) {
        Ok(suggestion) => suggestion,
        Err(error) => {
            let (status, body) = branch_read_error_response(&error);
            return write_response(stream, request, status, "application/json", &body);
        }
    };
    let body = serde_json::to_string(&suggestion).map_err(|error| error.to_string())?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/branch-prepare`: a prepared `create`, `switch`, or `delete`.
/// A refusal means no record was written, so it is a 409 with the preparation
/// that explains it rather than a 200 the dialog would have to inspect.
pub(crate) fn handle_branch_prepare(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let operation = required_form(&form, "operation")?;
    let name = required_form(&form, "name")?;
    let start_point = form
        .get("start_point")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let engine = engine_for_repo(&repo)?;
    let prepared = match operation.as_str() {
        "create" => engine
            .branch
            .prepare_create(&repo, &name, start_point, &session_id),
        "switch" => engine.branch.prepare_switch(&repo, &name, &session_id),
        "delete" => engine.branch.prepare_delete(&repo, &name, &session_id),
        other => {
            return write_response(
                stream,
                request,
                400,
                "application/json",
                &branch_error_body(
                    "branch_invalid_operation",
                    &format!(
                        "Unknown branch operation `{other}`. Expected create, switch, or delete."
                    ),
                ),
            );
        }
    };
    let preparation = match prepared {
        Ok(preparation) => preparation,
        Err(error) => {
            let (status, body) = branch_read_error_response(&error);
            return write_response(stream, request, status, "application/json", &body);
        }
    };
    if preparation.can_apply {
        let body = serde_json::to_string(&preparation).map_err(|error| error.to_string())?;
        return write_response(stream, request, 200, "application/json", &body);
    }
    let message = preparation
        .refusal
        .clone()
        .unwrap_or_else(|| "The branch operation was refused.".to_string());
    let preparation = serde_json::to_string(&preparation).map_err(|error| error.to_string())?;
    let body = format!(
        "{{\"error\":\"{}\",\"code\":\"branch_refused\",\"message\":\"{}\",\"preparation\":{preparation}}}",
        escape_json(&message),
        escape_json(&message),
    );
    write_response(stream, request, 409, "application/json", &body)
}

/// `POST /api/branch`: the typed approval. `approval=branch` is its own field;
/// a commit approval, a question answer, or a command approval never satisfies
/// it. The exact prepared `GitMutation` is sent back as JSON and compared with
/// the durable record, so this route cannot run a command the record did not
/// name.
pub(crate) fn handle_branch_change(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    if form.get("approval").map(String::as_str) != Some("branch") {
        return write_response(
            stream,
            request,
            400,
            "application/json",
            &branch_error_body(
                "approval_required",
                "A branch change needs its own approval (approval=branch). Nothing ran.",
            ),
        );
    }
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let prepare_id = required_form(&form, "prepare_id")?;
    let mutation = required_form(&form, "mutation")?;
    let mutation = match serde_json::from_str::<GitMutation>(&mutation) {
        Ok(mutation) => mutation,
        Err(_) => {
            return write_response(
                stream,
                request,
                400,
                "application/json",
                &branch_error_body(
                    "branch_mutation_invalid",
                    "The approved branch operation is not readable. Nothing ran.",
                ),
            );
        }
    };
    let engine = engine_for_repo(&repo)?;
    match engine.branch.load_record(&prepare_id) {
        Ok(record) if record.repository_root == repo && record.session_id == session_id => {}
        Ok(record) if record.repository_root != repo => {
            return write_response(
                stream,
                request,
                400,
                "application/json",
                &branch_error_body(
                    "branch_repository_mismatch",
                    "That branch preparation belongs to a different folder. Nothing ran.",
                ),
            );
        }
        Ok(_) => {
            return write_response(
                stream,
                request,
                400,
                "application/json",
                &branch_error_body(
                    "branch_session_mismatch",
                    "That branch preparation belongs to a different session. Nothing ran.",
                ),
            );
        }
        Err(error) => {
            let (status, body) = branch_read_error_response(&error);
            return write_response(stream, request, status, "application/json", &body);
        }
    }
    let confirm_unmerged = form.get("confirm_unmerged").map(String::as_str) == Some("true");
    match engine
        .branch
        .apply(&prepare_id, &mutation, "desktop_user", confirm_unmerged)
    {
        Ok(outcome) => {
            let body = serde_json::to_string(&outcome).map_err(|error| error.to_string())?;
            write_response(stream, request, 200, "application/json", &body)
        }
        Err(error) => {
            let (status, body) = branch_apply_error_response(&error);
            write_response(stream, request, status, "application/json", &body)
        }
    }
}

/// `GET /api/merge-preparation?mode=merge|rebase`: what a merge would do, or
/// which commits a rebase would replay. Both are reports; neither runs the
/// operation, and this handler has no branch that could.
pub(crate) fn handle_merge_preparation(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let session = required_param(request, "session")?;
    let mode = required_param(request, "mode")?;
    let base = required_param(request, "base")?;
    let engine = engine_for_repo(&repo)?;
    if engine
        .session_store
        .read_session(&session)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return write_response(
            stream,
            request,
            400,
            "application/json",
            &branch_error_body(
                "branch_unknown_session",
                "Unknown session. Open a session before preparing a merge or rebase.",
            ),
        );
    }
    match mode.as_str() {
        "merge" => match engine.branch.prepare_merge(&repo, &base, &session) {
            Ok(plan) => {
                let body = serde_json::to_string(&plan).map_err(|error| error.to_string())?;
                write_response(stream, request, 200, "application/json", &body)
            }
            Err(error) => {
                let (status, body) = branch_read_error_response(&error);
                write_response(stream, request, status, "application/json", &body)
            }
        },
        "rebase" => match engine.branch.prepare_rebase(&repo, &base, &session) {
            Ok(plan) => {
                let body = serde_json::to_string(&plan).map_err(|error| error.to_string())?;
                write_response(stream, request, 200, "application/json", &body)
            }
            Err(error) => {
                let (status, body) = branch_read_error_response(&error);
                write_response(stream, request, status, "application/json", &body)
            }
        },
        other => write_response(
            stream,
            request,
            400,
            "application/json",
            &branch_error_body(
                "branch_invalid_mode",
                &format!("Unknown preparation mode `{other}`. Expected merge or rebase."),
            ),
        ),
    }
}

fn json_nullable_str(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("\"{}\"", escape_json(value)),
        None => "null".to_string(),
    }
}
