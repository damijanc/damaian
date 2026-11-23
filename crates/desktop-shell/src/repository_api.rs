//! Repository routes: findings, the repository map, symbols and language
//! servers.

use super::*;

/// `GET /api/findings`. Every findings route takes `repo`, because staleness
/// is derived against the repository's files, and refuses a session from
/// another repository (spec 22 `context.md` §16).
pub(crate) fn handle_findings(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let session_id = required_param(request, "session_id")?;
    let engine = engine_for_repo(&repo)?;
    session_in_repository(&engine, &repo, &session_id)?;
    // Language-server diagnostics become findings here, from what the hub
    // already holds (spec 25 `context.md` §10). A failure must not cost the
    // panel what it already has.
    if let Err(error) = engine.record_language_server_findings(&session_id, &repo) {
        eprintln!("Could not record language-server findings for session {session_id}: {error}");
    }
    let body = findings_json(&engine, &repo, &session_id)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/finding-status`, answered with the refreshed findings so the
/// panel redraws from what was stored.
pub(crate) fn handle_finding_status(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let finding_id = required_form(&form, "finding_id")?;
    let requested = required_form(&form, "status")?;
    // Rejected, never defaulted, as for `/api/session-mode`. `stale`
    // parses, and the store refuses it (spec 22 `context.md` §13.2).
    let status = FindingStatus::parse(&requested).ok_or_else(|| {
        format!("Unknown finding status: {requested}. Expected open, dismissed, or fixed.")
    })?;
    let engine = engine_for_repo(&repo)?;
    session_in_repository(&engine, &repo, &session_id)?;
    engine
        .session_store
        .set_finding_status(&session_id, &finding_id, status)
        .map_err(|error| error.to_string())?;
    let body = findings_json(&engine, &repo, &session_id)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/findings-repair`. Marks nothing: it returns the request and its
/// rendered prompt, or `null` when nothing is left to repair (§15, §16).
pub(crate) fn handle_findings_repair(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let selected: Vec<String> = required_form(&form, "finding_ids")?
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    let engine = engine_for_repo(&repo)?;
    session_in_repository(&engine, &repo, &session_id)?;
    // Built from the findings as they are now, so an edit since the
    // panel was drawn still makes its finding stale (§15).
    let findings = engine
        .session_store
        .read_findings(&session_id, Path::new(&repo))
        .map_err(|error| error.to_string())?;
    let repair = RepairRequest::select(&findings, &selected);
    let repair_json = serde_json::to_string(&repair).map_err(|error| error.to_string())?;
    let prompt = repair.render().map_or_else(
        || "null".to_string(),
        |text| format!("\"{}\"", escape_json(&text)),
    );
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"request\":{repair_json},\"prompt\":{prompt}}}"),
    )
}

/// `GET /api/repository-map?repo=`: the repository's map, plus how the store
/// got it (spec 24 Task 8). Shaped like `damaian repo-map --json`, so the UI
/// and the CLI read the same JSON.
pub(crate) fn handle_repository_map(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let engine = engine_for_repo(&repo)?;
    let (map, load) = engine
        .repository_map(&repo)
        .map_err(|error| error.to_string())?;
    let body = format!(
        "{{\"load\":{},\"map\":{}}}",
        map_load_json(&load),
        map.to_json()
    );
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/repository-roots`: add, remove or clear a user root override in
/// the repository's own config, then answer with the rebuilt map.
///
/// The write goes to `<repo>/.damaian/config.conf`, in the user's working
/// tree, so Git shows it; the UI says so before the first write. A repo that
/// is not a usable directory, or lies outside `allowed_roots`, is refused, so
/// an override is only ever written inside a repository the user may open.
pub(crate) fn handle_repository_roots(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let action = required_form(&form, "action")?;
    let path = required_form(&form, "path")?;
    validate_working_folder(&repo)?;
    let config_path = repository_config_write_target(&engine_for_repo(&repo)?, &repo)?;
    let edit = match action.as_str() {
        "add" => workspace_engine::RootOverrideEdit::Add,
        "remove" => workspace_engine::RootOverrideEdit::Remove,
        "clear" => workspace_engine::RootOverrideEdit::Clear,
        other => {
            return Err(format!(
                "repository-roots action must be add, remove or clear, got {other}"
            ));
        }
    };
    let mut overlay = workspace_engine::ConfigOverlay::load_or_default(&config_path)
        .map_err(|error| error.to_string())?;
    workspace_engine::edit_root_overrides(&mut overlay, edit, &path);
    overlay
        .save(&config_path)
        .map_err(|error| error.to_string())?;

    // A fresh engine, so the map is built from the config just written.
    let engine = engine_for_repo(&repo)?;
    let (map, load) = engine
        .repository_map(&repo)
        .map_err(|error| error.to_string())?;
    let body = format!(
        "{{\"configPath\":\"{}\",\"load\":{},\"map\":{}}}",
        escape_json(&config_path.to_string_lossy()),
        map_load_json(&load),
        map.to_json()
    );
    write_response(stream, request, 200, "application/json", &body)
}

/// How many symbol matches `/api/symbols` returns. A search box wants a short
/// list; the CLI takes an explicit limit for anything larger.
const SYMBOL_SEARCH_LIMIT: usize = 50;

/// `GET /api/symbols?repo=&q=`: the repository's symbol lookup, each match
/// carrying its `Symbol`, match quality and `provenance`. A heuristic match is
/// labelled as one by the payload, so the UI never has to infer it.
pub(crate) fn handle_symbols(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let query = required_param(request, "q")?;
    let engine = engine_for_repo(&repo)?;
    let root = symbol_repository(&engine, &repo)?;
    let matches = engine
        .symbol_search(&root, &query, SYMBOL_SEARCH_LIMIT)
        .map_err(|error| error.to_string())?;
    let body = format!(
        "{{\"matches\":{}}}",
        workspace_engine::symbol_matches_json(&matches)
    );
    write_response(stream, request, 200, "application/json", &body)
}

/// `GET /api/symbol-references?repo=&path=&line=&name=`: Task 9's on-demand
/// answer, unchanged, for the UI. `available: false` is a normal answer: the
/// heuristic tier does not guess references.
pub(crate) fn handle_symbol_references(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let path = required_param(request, "path")?;
    let line = required_param(request, "line")?
        .parse::<u32>()
        .ok()
        .filter(|line| *line > 0)
        .ok_or_else(|| "line must be a positive integer".to_string())?;
    let name = required_param(request, "name")?;
    let engine = engine_for_repo(&repo)?;
    let root = symbol_repository(&engine, &repo)?;
    // The path arrives from the browser, so it goes through the same policy
    // every other browser-supplied path does before a server sees it.
    let target = engine
        .path_policy
        .resolve_existing(&root, &path, false)
        .map_err(|error| error.to_string())?;
    engine
        .path_policy
        .assert_not_restricted(&target.relative_path, false)
        .map_err(|error| error.to_string())?;
    let symbol = SymbolRef {
        path: target.relative_path,
        name,
        line,
    };
    let references = engine
        .symbol_references(&root, &symbol)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &workspace_engine::symbol_references_json(&references),
    )
}

/// `GET /api/language-servers?repo=`: every language the user configured, plus
/// the hub's live state once a lookup has started one. Settings can therefore
/// show a configured server before anything has spawned it.
pub(crate) fn handle_language_servers(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let engine = engine_for_repo(&repo)?;
    let root = symbol_repository(&engine, &repo)?;
    let statuses = engine
        .language_servers()
        .status(&repository_id_for_root(&root));
    let body = language_servers_json(&engine.config.language_servers, statuses);
    write_response(stream, request, 200, "application/json", &body)
}

/// The JSON `/api/language-servers` serves. A configured server that no lookup
/// has started yet is `idle`; a hub entry's own state replaces that once it
/// exists, so an unavailable or crashed server keeps its reason.
pub(crate) fn language_servers_json(
    configured: &BTreeMap<String, String>,
    statuses: Vec<LanguageServerStatus>,
) -> String {
    let mut servers: BTreeMap<String, serde_json::Value> = configured
        .iter()
        .map(|(language, command)| {
            (
                language.clone(),
                serde_json::json!({
                    "language": language,
                    "command": command,
                    "state": "idle",
                    "pid": serde_json::Value::Null,
                }),
            )
        })
        .collect();
    for status in statuses {
        // Strings, numbers and unit variants: none of this can fail.
        let value = serde_json::to_value(&status).expect("a language server status serialises");
        servers.insert(status.language.clone(), value);
    }
    let servers: Vec<serde_json::Value> = servers.into_values().collect();
    serde_json::to_string(&serde_json::json!({ "servers": servers }))
        .expect("a language server list serialises")
}

/// A symbol endpoint's repository root, canonical and inside `allowed_roots`.
/// Indexing does not consult `allowed_roots` either, so without this check the
/// endpoints would read any directory they are named.
pub(crate) fn symbol_repository(engine: &WorkspaceEngine, repo: &str) -> Result<PathBuf, String> {
    let root = validate_working_folder(repo)?;
    engine
        .path_policy
        .canonical_root(&root)
        .map_err(|error| error.to_string())
}

/// Where a root override for `repo` is written, once `repo` passes the path
/// policy. Indexing does not consult `allowed_roots`, so without this check
/// the endpoint would write into any directory it is named.
pub(crate) fn repository_config_write_target(
    engine: &WorkspaceEngine,
    repo: &str,
) -> Result<PathBuf, String> {
    engine
        .path_policy
        .canonical_root(repo)
        .map_err(|error| error.to_string())?;
    Ok(Config::repository_config_path(repo))
}

/// `GET /api/command-proposal?repo=&proposal_id=`: the stored proposal's
/// working directory and repository root, so a command-approval card can say
/// "Runs in `packages/api`".
///
/// The card's payload streams out of `chat.rs`'s `AgentCommandProposal`, which
/// carries neither (spec 24 Task 8); the shell reads them from the
/// `CommandStore` the proposal was written to instead of changing `chat.rs`.
pub(crate) fn handle_command_proposal(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let proposal_id = required_param(request, "proposal_id")?;
    let engine = engine_for_repo(&repo)?;
    let proposal = engine
        .validation_orchestrator
        .load_proposal(&proposal_id)
        .map_err(|error| error.to_string())?;
    // What decided, for the card (spec 111 D7): the rule as written and its
    // source, the line naming it, and the warning when a rule of the user's
    // lifted a built-in block. `null` when no rule decided.
    let optional = |text: Option<String>| {
        text.map_or_else(
            || "null".to_string(),
            |text| format!("\"{}\"", escape_json(&text)),
        )
    };
    let rule = proposal.rule.as_ref();
    let body = format!(
        "{{\"proposalId\":\"{}\",\"workingDirectory\":\"{}\",\"repositoryRoot\":\"{}\",\"rule\":{},\"ruleSource\":{},\"ruleSummary\":{},\"ruleWarning\":{}}}",
        escape_json(&proposal.id),
        escape_json(&proposal.working_directory),
        escape_json(&proposal.repository_root),
        optional(rule.map(|rule| rule.rule.clone())),
        optional(rule.map(|rule| rule.source.clone())),
        optional(rule.map(|rule| rule.summary())),
        optional(rule.and_then(|rule| rule.warning())),
    );
    write_response(stream, request, 200, "application/json", &body)
}
