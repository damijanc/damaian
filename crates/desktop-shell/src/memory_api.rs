//! Memory routes: the proposal list and the two decisions (spec 29).

use super::*;

/// `GET /api/memory-proposals`: what is still waiting for a decision in this
/// session, with the origin line the card shows. Read from the session log, so
/// a reload or a restart shows the same list.
pub(crate) fn handle_memory_proposals(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let session_id = required_param(request, "session_id")?;
    let engine = default_engine()?;
    if engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err(format!("Unknown session: {session_id}"));
    }
    let pending = engine
        .memory_store
        .pending_proposals(&session_id)
        .map_err(|error| error.to_string())?;
    let mut proposals = Vec::new();
    for proposal in pending {
        let mut value = serde_json::to_value(&proposal).map_err(|error| error.to_string())?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "sourceLabel".to_string(),
                serde_json::json!(proposal.source_label()),
            );
            object.insert(
                "originKind".to_string(),
                serde_json::json!(proposal.origin.kind()),
            );
        }
        proposals.push(value);
    }
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"proposals\":{},\"userScopeEnabled\":{}}}",
            serde_json::to_string(&proposals).map_err(|error| error.to_string())?,
            engine.config.memory_user_scope_enabled
        ),
    )
}

fn parse_memory_scope(value: &str) -> Result<MemoryScope, String> {
    match value {
        "session" => Ok(MemoryScope::Session),
        "project" => Ok(MemoryScope::Project),
        "user" => Ok(MemoryScope::User),
        other => Err(format!("unknown memory scope: {other}")),
    }
}

/// A memory proposal belongs to the session that produced it, and the session
/// belongs to the repository it was created in. The client supplies both ids,
/// so both are checked here; otherwise `repo=B` with a session from repository
/// A would write A's confirmed statement into B's project memory.
pub(crate) fn require_memory_session_in_repository(
    engine: &WorkspaceEngine,
    session_id: &str,
    repo: &str,
) -> Result<(), String> {
    let session = engine
        .session_store
        .read_session(session_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Unknown session: {session_id}"))?;
    let expected = repository_id_for_root(Path::new(repo));
    if session.repository_id != expected {
        return Err("That memory proposal belongs to a session in another repository".to_string());
    }
    Ok(())
}

/// `POST /api/memory-confirm`: one proposal id, never a statement. The route
/// reloads the stored proposal and the server resolves the project identity;
/// a client cannot choose either. The confirmation is recorded here because
/// this route is the user action.
pub(crate) fn handle_memory_confirm(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let proposal_id = required_form(&form, "proposal_id")?;
    let engine = engine_for_repo(&repo)?;
    require_memory_session_in_repository(&engine, &session_id, &repo)?;
    let identity = ProjectIdentity::resolve_for_root(Path::new(&repo), &engine.git)
        .map_err(|error| error.to_string())?;
    let mut confirmation = UserConfirmation::new(&proposal_id);
    if let Some(scope) = form.get("scope").filter(|value| !value.is_empty()) {
        confirmation = confirmation.with_scope(parse_memory_scope(scope)?);
    }
    let entry = engine
        .memory_store
        .confirm_proposal(
            Path::new(&repo),
            Some(&identity),
            &session_id,
            &proposal_id,
            confirmation,
        )
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"proposalId\":{},\"entry\":{}}}",
            serde_json::to_string(&proposal_id).map_err(|error| error.to_string())?,
            serde_json::to_string(&entry).map_err(|error| error.to_string())?
        ),
    )
}

/// `POST /api/memory-dismiss`: one proposal id. A dismissal is a decision, not
/// a deletion: the proposal and the dismissal stay in the session log, and no
/// entry is written.
pub(crate) fn handle_memory_dismiss(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let proposal_id = required_form(&form, "proposal_id")?;
    let engine = engine_for_repo(&repo)?;
    require_memory_session_in_repository(&engine, &session_id, &repo)?;
    engine
        .memory_store
        .dismiss_proposal(&session_id, &proposal_id)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"proposalId\":{},\"dismissed\":true}}",
            serde_json::to_string(&proposal_id).map_err(|error| error.to_string())?
        ),
    )
}
