//! Configuration routes and their helpers: config files, model and
//! provider keys, permission profiles, repository config, hooks and MCP.

use super::*;

pub(crate) fn handle_config(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = request.param("repo");
    let config = Config::load_for_repository(repo.as_deref().map(Path::new))
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"policy\":\"{}\"}}",
            escape_json(&config.to_policy_text())
        ),
    )
}

/// `GET /api/provider-token-defaults?provider=<id>&models=<a,b>`: what each
/// model gets when the provider editor's *Max output tokens* and *Context
/// budget* are left blank. Only the engine knows these per-model numbers, so
/// the editor asks rather than showing "Automatic" with no value. No models
/// asks for the provider's own default.
pub(crate) fn handle_provider_token_defaults(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let requested = request.param("provider").unwrap_or_default();
    // A provider still being named in the editor has no id yet; it gets the
    // defaults of an unknown provider.
    let provider = if requested.trim().is_empty() {
        String::new()
    } else {
        normalize_model_provider(&requested).map_err(|error| error.to_string())?
    };
    let models_param = request.param("models").unwrap_or_default();
    let mut models: Vec<&str> = models_param
        .split(',')
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .collect();
    if models.is_empty() {
        models.push("");
    }
    let entries = models
        .iter()
        .map(|model| {
            let (output, context) = builtin_token_defaults(&provider, model);
            let window = builtin_context_window(&provider, model);
            format!(
                "{{\"model\":\"{}\",\"maxOutputTokens\":{},\"contextTokenBudget\":{},\"contextWindow\":{}}}",
                escape_json(model),
                output.map_or_else(|| "null".to_string(), |value| value.to_string()),
                context,
                window.map_or_else(|| "null".to_string(), |value| value.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"models\":[{entries}]}}"),
    )
}

pub(crate) fn handle_get_config_file(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let scope = request.param("scope");
    let repo = request.param("repo").unwrap_or_default();
    let path = desktop_settings_config_path(scope.as_deref())?;
    let content = if path.exists() {
        fs::read_to_string(&path).map_err(|error| error.to_string())?
    } else {
        String::new()
    };
    let (effective_policy, effective_error) = effective_policy_for_repo(&repo);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"path\":\"{}\",\"exists\":{},\"content\":\"{}\",\"effectivePolicy\":\"{}\",\"effectiveError\":\"{}\"}}",
            escape_json(&path.to_string_lossy()),
            path.exists(),
            escape_json(&content),
            escape_json(&effective_policy),
            escape_json(&effective_error)
        ),
    )
}

pub(crate) fn handle_model_key_status(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    let model_provider = request.param("model_provider");
    write_response(
        stream,
        request,
        200,
        "application/json",
        &model_key_status_json(&repo, model_provider.as_deref())?,
    )
}

pub(crate) fn handle_repository_config_review(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    write_response(
        stream,
        request,
        200,
        "application/json",
        &repository_config_review_json(&repo)?,
    )
}

pub(crate) fn handle_repository_config_allowlist(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    // Pipe-separated, matching how `command_allowlist` is written on
    // disk — which is also why an entry can never contain a pipe.
    let keep = form
        .get("keep")
        .map(String::as_str)
        .unwrap_or_default()
        .split('|')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    write_response(
        stream,
        request,
        200,
        "application/json",
        &resolve_repository_allowlist_migration(&repo, &keep)?,
    )
}

/// Every configured hook for `repo`, with its activation, so Settings can list
/// and act on them. `repo` is the only input: there is no path parameter that
/// could name a config file.
fn hooks_json(repo: &str) -> Result<String, String> {
    if repo.trim().is_empty() {
        return Ok("{\"repositoryPath\":\"\",\"hooks\":[]}".to_string());
    }
    let config = config_for_repo(repo)?;
    let hooks = list_hooks(&config, Path::new(repo));
    serde_json::to_string(&serde_json::json!({
        "repositoryPath": repo,
        "hooks": hooks,
    }))
    .map_err(|error| error.to_string())
}

pub(crate) fn handle_hooks(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    write_response(
        stream,
        request,
        200,
        "application/json",
        &hooks_json(&repo)?,
    )
}

pub(crate) fn handle_hook_review(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let hook = required_form(&form, "hook")?;
    let decision = match required_form(&form, "decision")?.as_str() {
        "approve" => HookReviewDecision::Approve {
            hash: required_form(&form, "hash")?,
        },
        "decline" => HookReviewDecision::Decline,
        other => {
            return Err(format!(
                "A hook review decision is approve or decline, not {other}"
            ));
        }
    };
    let config = config_for_repo(&repo)?;
    let path = review_hook(&config, Path::new(&repo), &hook, decision)
        .map_err(|error| error.to_string())?;
    // Re-read so the response carries the review that was just written.
    let config = config_for_repo(&repo)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::to_string(&serde_json::json!({
            "path": path.to_string_lossy(),
            "hooks": list_hooks(&config, Path::new(&repo)),
        }))
        .map_err(|error| error.to_string())?,
    )
}

pub(crate) fn handle_hook_enabled(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let hook = required_form(&form, "hook")?;
    let enabled = match required_form(&form, "enabled")?.as_str() {
        "true" | "1" => true,
        "false" | "0" => false,
        other => return Err(format!("A hook toggle is true or false, not {other}")),
    };
    let config = config_for_repo(&repo)?;
    let path = set_hook_enabled(&config, &hook, enabled).map_err(|error| error.to_string())?;
    let config = config_for_repo(&repo)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::to_string(&serde_json::json!({
            "path": path.to_string_lossy(),
            "hooks": list_hooks(&config, Path::new(&repo)),
        }))
        .map_err(|error| error.to_string())?,
    )
}

pub(crate) fn handle_hook_test(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let hook_id = required_form(&form, "hook")?;
    let event_name = required_form(&form, "event")?;
    let event =
        HookEvent::parse(&event_name).ok_or_else(|| format!("Unknown hook event: {event_name}"))?;
    let config = config_for_repo(&repo)?;
    let hook = config
        .hook(&hook_id)
        .cloned()
        .ok_or_else(|| format!("Hook `{hook_id}` is not configured"))?;
    let run =
        test_run(&config, &hook, event, Path::new(&repo)).map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::to_string(&serde_json::json!({
            "id": hook_id,
            "event": event.as_str(),
            "verdict": run.verdict.as_str(),
            "failure": run.failure.map(|kind| kind.as_str()),
            "stderr": run.stderr,
            "durationMs": run.duration_ms,
        }))
        .map_err(|error| error.to_string())?,
    )
}

pub(crate) fn handle_config_set(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let scope = form.get("scope").map(String::as_str);
    let key = required_form(&form, "key")?;
    let value = required_form(&form, "value")?;
    let path = desktop_settings_config_path(scope)?;
    let path = update_config_overlay(path, &key, &value)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
    )
}

pub(crate) fn handle_post_config_file(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let scope = form.get("scope").map(String::as_str);
    let repo = form.get("repo").cloned().unwrap_or_default();
    let content = form.get("content").cloned().unwrap_or_default();
    let path = desktop_settings_config_path(scope)?;
    save_config_file(&path, &content)?;
    let (effective_policy, effective_error) = effective_policy_for_repo(&repo);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"path\":\"{}\",\"effectivePolicy\":\"{}\",\"effectiveError\":\"{}\"}}",
            escape_json(&path.to_string_lossy()),
            escape_json(&effective_policy),
            escape_json(&effective_error)
        ),
    )
}

pub(crate) fn handle_model_key(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let scope = form.get("scope").map(String::as_str);
    let repo = form.get("repo").cloned().unwrap_or_default();
    let account = required_form(&form, "account")?;
    let api_key = required_form(&form, "api_key")?;
    let reference = keychain::reference_for_account(&account)?;
    keychain::write_password(&account, &api_key)?;
    remember_model_api_key(&account, &api_key);
    let path = desktop_settings_config_path(scope)?;
    update_config_overlay(path.clone(), "model_api_key_env", &reference)?;
    let (effective_policy, effective_error) = effective_policy_for_repo(&repo);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"path\":\"{}\",\"reference\":\"{}\",\"account\":\"{}\",\"configured\":true,\"effectivePolicy\":\"{}\",\"effectiveError\":\"{}\"}}",
            escape_json(&path.to_string_lossy()),
            escape_json(&reference),
            escape_json(account.trim()),
            escape_json(&effective_policy),
            escape_json(&effective_error)
        ),
    )
}

pub(crate) fn handle_provider_key(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let account = required_form(&form, "account")?;
    let api_key = required_form(&form, "api_key")?;
    let reference = keychain::reference_for_account(&account)?;
    keychain::write_password(&account, &api_key)?;
    remember_model_api_key(&account, &api_key);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"reference\":\"{}\",\"account\":\"{}\",\"configured\":true}}",
            escape_json(&reference),
            escape_json(account.trim())
        ),
    )
}

pub(crate) fn handle_model_key_delete(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let account = required_form(&form, "account")?;
    let deleted = keychain::delete_password(&account)?;
    forget_model_api_key(&account);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"account\":\"{}\",\"deleted\":{},\"configured\":false}}",
            escape_json(account.trim()),
            deleted
        ),
    )
}

pub(crate) fn handle_mcp_test(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let body = match mcp_test_connection(&form, &config.data_dir) {
        Ok(tools) => format!(
            "{{\"ok\":true,\"toolCount\":{},\"tools\":[{}]}}",
            tools.len(),
            json_string_array(&tools)
        ),
        Err(error) => {
            format!("{{\"ok\":false,\"error\":\"{}\"}}", escape_json(&error))
        }
    };
    write_response(stream, request, 200, "application/json", &body)
}

pub(crate) fn default_engine() -> Result<WorkspaceEngine, String> {
    let (config, report) =
        Config::load_for_repository_reporting(None).map_err(|error| error.to_string())?;
    Ok(WorkspaceEngine::new_with_report(config, &report))
}

pub(crate) fn desktop_settings_config_path(scope: Option<&str>) -> Result<PathBuf, String> {
    match scope.unwrap_or("user") {
        "user" => Ok(Config::default().user_config_path()),
        "repo" => Err(
            "desktop settings only write user config; edit repository config in .damaian/config.conf"
                .to_string(),
        ),
        _ => Err("scope must be user".to_string()),
    }
}

pub(crate) fn effective_policy_for_repo(repo: &str) -> (String, String) {
    match Config::load_for_repository(if repo.is_empty() {
        None
    } else {
        Some(Path::new(repo))
    }) {
        Ok(config) => (config.to_policy_text(), String::new()),
        Err(error) => (String::new(), error.to_string()),
    }
}

/// `GET /api/effective-policy`: spec 31 Task 6's `EffectivePolicy`, which
/// Settings › General draws as a table. With a session, the header shows the
/// profile intersected with that session's mode (proposal §5.6).
pub(crate) fn handle_effective_policy(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    let session_id = request.param("session").unwrap_or_default();
    let body = effective_policy_json(&repo, &session_id)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/permission-profile`: the steps `damaian profile-set` runs.
/// Config is loaded without the repository, so a checkout whose custom profile
/// file has gone missing can still be switched away from it (spec 31 Task 3,
/// deviation 2).
pub(crate) fn handle_permission_profile(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let requested = required_form(&form, "profile")?;
    let session_id = form.get("session").cloned().unwrap_or_default();
    if !Path::new(&repo).is_dir() {
        return Err(format!("Not a repository folder: {repo}"));
    }
    let id = ProfileId::parse(&requested).map_err(|error| error.to_string())?;
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let engine = WorkspaceEngine::new(config.clone());
    select_profile(&config, Path::new(&repo), id, &engine.audit_log)
        .map_err(|error| error.to_string())?;
    let body = effective_policy_json(&repo, &session_id)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `GET /api/permission-profiles`: the custom profiles the picker offers
/// beside the built-ins (spec 31 Task 7, deviation 4).
pub(crate) fn handle_permission_profiles(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let custom: Vec<String> = custom_profile_ids(&config.data_dir)
        .map_err(|error| error.to_string())?
        .iter()
        .map(|id| id.as_str().to_string())
        .collect();
    let body = serde_json::json!({ "custom": custom }).to_string();
    write_response(stream, request, 200, "application/json", &body)
}

/// `GET /api/permission-profile-export?profile=<id>`: the profile as a file
/// someone can share, carrying only profile keys (spec 31, `context.md` §9).
pub(crate) fn handle_permission_profile_export(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let requested = request.param("profile").unwrap_or_default();
    let id = ProfileId::parse(&requested).map_err(|error| error.to_string())?;
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let text = export_profile(&id, &config.data_dir).map_err(|error| error.to_string())?;
    let body = serde_json::json!({ "profile": id.as_str(), "text": text }).to_string();
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/permission-profile-import` with `name`, `text`, and optional
/// `repo`, `replace` and `preview`. With `preview=true` it only reviews, so
/// Settings can show what will not apply before anything is written
/// (`context.md` §9). The import writes a profile file, never user config,
/// so the editor's copy of `user.conf` cannot go stale (Task 7, deviation 6).
pub(crate) fn handle_permission_profile_import(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let name = required_form(&form, "name")?;
    let text = required_form(&form, "text")?;
    let repo = form.get("repo").cloned().unwrap_or_default();
    let flag = |key: &str| form.get(key).is_some_and(|value| value == "true");
    let root = (!repo.is_empty()).then(|| Path::new(&repo));
    let base = profile_import_base(root).map_err(|error| error.to_string())?;
    let (review, written, replaced) = if flag("preview") {
        // The name is checked even for a preview, so a reserved or invalid
        // name is refused before the user confirms anything.
        ProfileId::custom(&name).map_err(|error| error.to_string())?;
        (review_profile_import(&base, &text), false, false)
    } else {
        let engine = WorkspaceEngine::new(base.clone());
        let imported = import_profile(&base, &text, &name, flag("replace"), &engine.audit_log)
            .map_err(|error| error.to_string())?;
        (imported.review, true, imported.replaced)
    };
    let refusals = |keys: &[workspace_engine::RejectedConfigKey]| {
        keys.iter()
            .map(|rejected| serde_json::json!({ "key": rejected.key, "class": rejected.class.as_str() }))
            .collect::<Vec<_>>()
    };
    let exists = ProfileId::custom(&name)
        .ok()
        .and_then(|id| id.custom_path(&base.data_dir))
        .is_some_and(|path| path.is_file());
    let body = serde_json::json!({
        "profile": name,
        "written": written,
        "replaced": replaced,
        "exists": exists,
        "carried": review.carried,
        "notCarried": refusals(&review.not_carried),
        "loosening": refusals(&review.loosening),
    })
    .to_string();
    write_response(stream, request, 200, "application/json", &body)
}

/// The attributed policy for `repo`, from one fresh load. A custom profile's
/// refused keys are audited here, once per key, because this is where the
/// shell shows them (spec 31 criterion 4).
fn effective_policy_json(repo: &str, session_id: &str) -> Result<String, String> {
    let root = (!repo.is_empty()).then(|| Path::new(repo));
    let (config, report) =
        Config::load_for_repository_reporting(root).map_err(|error| error.to_string())?;
    let engine = WorkspaceEngine::new(config.clone());
    review_profile_rejections(&config.data_dir, &report, &engine.audit_log)
        .map_err(|error| error.to_string())?;
    let mode = if session_id.is_empty() {
        None
    } else if repo.is_empty() {
        return Err("A session's policy needs its repository".to_string());
    } else {
        let session = session_in_repository(&engine, repo, session_id)?;
        Some(engine.session_store.session_mode(&session.id))
    };
    serde_json::to_string(&EffectivePolicy::from_load(&config, &report, mode))
        .map_err(|error| error.to_string())
}

pub(crate) fn resolve_model_api_key(reference: &str) -> Result<String, String> {
    if let Some(account) = keychain::account_from_reference(reference) {
        if let Some(api_key) = cached_model_api_key(account) {
            return Ok(api_key);
        }
        let api_key = keychain::read_password(account).map_err(|error| {
            format!(
                "Keychain API key '{}' is required. Open Settings and save the model API key. {error}",
                account
            )
        })?;
        remember_model_api_key(account, &api_key);
        Ok(api_key)
    } else {
        env::var(reference).map_err(|_| format!("{reference} is required"))
    }
}

/// The shell's key resolution for model calls.
///
/// A missing key is fatal for a remote endpoint, but a loopback runtime such
/// as Ollama serves without credentials. There the key is empty and the
/// transport sends no `Authorization` header, rather than making the user
/// invent and store a credential that is never checked.
pub(crate) fn resolve_shell_model_api_key(config: &Config) -> Result<String, String> {
    match resolve_model_api_key(&config.model_api_key_env) {
        Ok(key) => Ok(key),
        Err(_) if config.model_endpoint_is_loopback() => Ok(String::new()),
        Err(error) => Err(error),
    }
}

/// Resolver handed to the chat orchestrator so it can turn an MCP server's
/// `auth_token_env` reference into a bearer token via the keychain (or an
/// environment variable), without the engine ever touching the keychain
/// directly. Mirrors [`resolve_model_api_key`], but returns `None` instead of
/// erroring so a missing token just means "no auth header".
fn mcp_token_resolver() -> McpTokenResolver {
    McpTokenResolver::new(resolve_mcp_token)
}

pub(crate) fn configure_chat_integrations(engine: &mut WorkspaceEngine) {
    engine
        .chat_orchestrator
        .set_mcp_token_resolver(mcp_token_resolver());
    if let Some(runner) = browser_diagnostics_runner_for_config(&engine.config) {
        engine.chat_orchestrator.set_web_diagnostics_runner(runner);
    }
}

pub(crate) fn resolve_mcp_token(reference: &str) -> Option<String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return None;
    }
    if let Some(account) = keychain::account_from_reference(reference) {
        if let Some(token) = cached_model_api_key(account) {
            return Some(token);
        }
        if let Ok(token) = keychain::read_password(account) {
            remember_model_api_key(account, &token);
            return Some(token);
        }
        return None;
    }
    env::var(reference).ok()
}

/// Builds a one-off [`McpServerConfig`] (plus resolved token) from the MCP
/// editor form, for the Test-connection endpoint. Accepts either a raw
/// `auth_token` (typed but not yet saved) or an `auth_token_env` reference to
/// resolve from the keychain.
fn mcp_config_from_form(
    form: &HashMap<String, String>,
) -> Result<(McpServerConfig, Option<String>), String> {
    let raw_id = form
        .get("id")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("test");
    let id = normalize_mcp_server_id(raw_id).map_err(|error| error.to_string())?;
    let transport =
        parse_mcp_transport(form.get("transport").map(String::as_str).unwrap_or("stdio"))
            .map_err(|error| error.to_string())?;

    let split = |value: &str| {
        value
            .split('|')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let env = form
        .get("env")
        .map(|value| {
            value
                .split('|')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .filter_map(|item| {
                    item.split_once('=')
                        .map(|(key, val)| (key.trim().to_string(), val.trim().to_string()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let auth_token_env = form.get("auth_token_env").cloned().unwrap_or_default();
    let token = if let Some(raw) = form
        .get("auth_token")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        Some(raw)
    } else if !auth_token_env.trim().is_empty() {
        resolve_mcp_token(&auth_token_env)
    } else {
        None
    };

    let config = McpServerConfig {
        label: form.get("label").cloned().unwrap_or_else(|| id.clone()),
        transport,
        command: form.get("command").cloned().unwrap_or_default(),
        args: form
            .get("args")
            .map(|value| split(value))
            .unwrap_or_default(),
        env,
        url: form
            .get("url")
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_default(),
        auth_token_env,
        enabled: true,
        require_approval: true,
        id,
    };
    Ok((config, token))
}

/// Connects to the server described by the form and lists its tools. Returns
/// the discovered tool names on success.
fn mcp_test_connection(
    form: &HashMap<String, String>,
    data_dir: &Path,
) -> Result<Vec<String>, String> {
    let (config, token) = mcp_config_from_form(form)?;
    if config.transport == McpTransport::Stdio && config.command.trim().is_empty() {
        return Err("A command is required for a local (stdio) server.".to_string());
    }
    if config.transport == McpTransport::Http && config.url.trim().is_empty() {
        return Err("A URL is required for a remote (http) server.".to_string());
    }
    // Testing a server still spawns one, so it is registered like any other.
    // No session: the user is on the settings screen, not in a turn.
    let registry = ProcessRegistry::open(data_dir).map_err(|error| error.to_string())?;
    let mut client = McpClient::connect(&config, token, &registry, "", Duration::from_secs(15))
        .map_err(|error| error.to_string())?;
    let tools = client
        .list_tools(Duration::from_secs(15))
        .map_err(|error| error.to_string())?;
    Ok(tools.into_iter().map(|tool| tool.name).collect())
}

fn model_api_key_cache() -> &'static Mutex<HashMap<String, String>> {
    MODEL_API_KEY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn cached_model_api_key(account: &str) -> Option<String> {
    model_api_key_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(account.trim()).cloned())
}

pub(crate) fn remember_model_api_key(account: &str, api_key: &str) {
    if let Ok(mut cache) = model_api_key_cache().lock() {
        cache.insert(account.trim().to_string(), api_key.to_string());
    }
}

pub(crate) fn forget_model_api_key(account: &str) {
    if let Ok(mut cache) = model_api_key_cache().lock() {
        cache.remove(account.trim());
    }
}

fn model_key_status_json(repo: &str, provider: Option<&str>) -> Result<String, String> {
    let (config, _) = config_for_repo_with_provider(repo, provider)?;
    let reference = config.model_api_key_env;
    if let Some(account) = keychain::account_from_reference(&reference) {
        let status = match keychain::password_exists(account) {
            Ok(configured) => (configured, String::new()),
            Err(error) => (false, error),
        };
        return Ok(format!(
            "{{\"reference\":\"{}\",\"kind\":\"keychain\",\"account\":\"{}\",\"configured\":{},\"message\":\"{}\"}}",
            escape_json(&reference),
            escape_json(account),
            status.0,
            escape_json(&status.1)
        ));
    }

    Ok(format!(
        "{{\"reference\":\"{}\",\"kind\":\"environment\",\"account\":\"\",\"configured\":{},\"message\":\"{}\"}}",
        escape_json(&reference),
        env::var(&reference).is_ok(),
        escape_json(&format!("Environment variable {reference}"))
    ))
}

pub(crate) fn save_config_file(path: &Path, content: &str) -> Result<(), String> {
    workspace_engine::ConfigOverlay::parse(content).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, content).map_err(|error| error.to_string())
}

pub(crate) fn update_config_overlay(
    path: std::path::PathBuf,
    key: &str,
    value: &str,
) -> Result<std::path::PathBuf, String> {
    let mut overlay = workspace_engine::ConfigOverlay::load_or_default(&path)
        .map_err(|error| error.to_string())?;
    overlay.set(key, value).map_err(|error| error.to_string())?;
    overlay.save(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

/// What the selected repository's config asked for and did not get, plus any
/// `command_allowlist` entries still awaiting the user's keep-or-discard.
/// Called when a repository is selected; both halves are reported once per
/// repository, so an unremarkable repository answers with empty lists.
pub(crate) fn repository_config_review_json(repo: &str) -> Result<String, String> {
    if repo.trim().is_empty() {
        return Ok("{\"rejectedKeys\":[],\"allowlistEntries\":[],\"hooks\":[]}".to_string());
    }
    let (config, report) = Config::load_for_repository_reporting(Some(Path::new(repo)))
        .map_err(|error| error.to_string())?;
    // A repository hook that is waiting on the user, or whose definition has
    // changed since they approved it, is an itemised review entry: the notice
    // shows the full command line, its events and its fail direction.
    let hooks: Vec<_> = list_hooks(&config, Path::new(repo))
        .into_iter()
        .filter(|hook| {
            matches!(
                hook.activation,
                HookActivation::PendingReview | HookActivation::ChangedSinceReview
            )
        })
        .collect();
    let engine = WorkspaceEngine::new(config);
    let notice = engine
        .repository_trust
        .review(&report, &engine.audit_log)
        .map_err(|error| error.to_string())?;
    let migration = engine
        .repository_trust
        .pending_allowlist_migration(&report)
        .map_err(|error| error.to_string())?;
    // Audited only. A profile's refusals are the user's own choice of file,
    // not the repository's, so they stay out of this notice (spec 31 Task 3).
    review_profile_rejections(&engine.config.data_dir, &report, &engine.audit_log)
        .map_err(|error| error.to_string())?;

    let rejected = notice
        .as_ref()
        .map(|notice| {
            notice
                .rejected
                .iter()
                .map(|rejected| {
                    serde_json::json!({
                        "key": rejected.key,
                        "class": rejected.class.as_str(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let entries = migration
        .as_ref()
        .map(|migration| migration.entries.clone())
        .unwrap_or_default();
    serde_json::to_string(&serde_json::json!({
        "repositoryPath": repo,
        "rejectedKeys": rejected,
        "allowlistEntries": entries,
        "hooks": hooks,
    }))
    .map_err(|error| error.to_string())
}

fn resolve_repository_allowlist_migration(repo: &str, keep: &[String]) -> Result<String, String> {
    let (config, report) = Config::load_for_repository_reporting(Some(Path::new(repo)))
        .map_err(|error| error.to_string())?;
    let engine = WorkspaceEngine::new(config);
    let path = engine
        .repository_trust
        .resolve_allowlist_migration(&report, keep, &engine.audit_log)
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "{{\"path\":\"{}\",\"keptCount\":{}}}",
        escape_json(&path.to_string_lossy()),
        keep.len()
    ))
}

pub(crate) fn engine_for_repo(repo: &str) -> Result<WorkspaceEngine, String> {
    let (config, report) = config_and_report_for_repo(repo)?;
    Ok(WorkspaceEngine::new_with_report(config, &report))
}

/// The config with the chat's model selection applied. The first request of a
/// turn and every route that resumes one take it, so a paused turn continues on
/// the provider it started with, not on whatever the saved config names
/// (spec 74).
pub(crate) fn config_for_repo_with_model_options(
    repo: &str,
    form: &HashMap<String, String>,
) -> Result<(Config, RepositoryConfigReport), String> {
    let (mut config, report) = config_and_report_for_repo(repo)?;
    apply_model_form_options(&mut config, form)?;
    Ok((config, report))
}

pub(crate) fn config_for_repo(repo: &str) -> Result<Config, String> {
    config_and_report_for_repo(repo).map(|(config, _)| config)
}

/// The loaded config and the report that says which scope set each key. Chat
/// and command-approval paths build their engine with the report so an
/// approval raised by a setting can name who set it (spec 62 Task 3).
pub(crate) fn config_and_report_for_repo(
    repo: &str,
) -> Result<(Config, RepositoryConfigReport), String> {
    let repo_path = if repo.is_empty() {
        None
    } else {
        Some(Path::new(repo))
    };
    Config::load_for_repository_reporting(repo_path).map_err(|error| error.to_string())
}

pub(crate) fn config_for_repo_with_provider(
    repo: &str,
    provider: Option<&str>,
) -> Result<(Config, RepositoryConfigReport), String> {
    let (mut config, report) = config_and_report_for_repo(repo)?;
    if let Some(provider) = provider.map(str::trim).filter(|value| !value.is_empty()) {
        switch_model_provider(&mut config, provider)?;
    }
    Ok((config, report))
}

/// Makes `provider` the active one, in memory only.
pub(crate) fn switch_model_provider(config: &mut Config, provider: &str) -> Result<(), String> {
    config.model_provider =
        normalize_model_provider(provider).map_err(|error| error.to_string())?;
    config.apply_model_provider_defaults();
    Ok(())
}

fn apply_model_form_options(
    config: &mut Config,
    form: &HashMap<String, String>,
) -> Result<(), String> {
    if let Some(provider) = form
        .get("model_provider")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.model_provider =
            normalize_model_provider(provider).map_err(|error| error.to_string())?;
        config.apply_model_provider_defaults();
    }
    if let Some(model) = form
        .get("model")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.model_name = model.to_string();
    }
    if let Some(reasoning_level) = form
        .get("reasoning_level")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.model_reasoning_level =
            normalize_model_reasoning_level(reasoning_level).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// `GET /api/project-checks?repo=`: the checks the user added to this
/// repository's task-completion floor (spec 109 D9). `checks` is the user's
/// own list, which Settings edits; `effective` is what applies after the
/// repository's config narrowed it, if it did.
pub(crate) fn handle_project_checks(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    validate_working_folder(&repo)?;
    let body = project_checks_json(&repo)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/project-checks` with `repo` and `checks`, one entry per line
/// (`<command>` or `<directory>::<command>`): replaces the user's list for this
/// repository in user config. An empty list removes the key. Each entry is one
/// command; a `|` would split it into two, so it is refused rather than
/// silently changing what was typed.
pub(crate) fn handle_project_checks_set(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    validate_working_folder(&repo)?;
    let mut checks = Vec::new();
    for line in form
        .get("checks")
        .map(String::as_str)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.contains('|') {
            return Err(format!(
                "A project check is one command, without `|`: {line}"
            ));
        }
        let check =
            workspace_engine::ProjectCheck::parse(line).map_err(|error| error.to_string())?;
        if !checks.contains(&check) {
            checks.push(check);
        }
    }
    let path = desktop_settings_config_path(None)?;
    let mut overlay = workspace_engine::ConfigOverlay::load_or_default(&path)
        .map_err(|error| error.to_string())?;
    let repository_id = workspace_engine::repository_id_for_root(Path::new(&repo));
    if checks.is_empty() {
        overlay.project_checks_by_repository.remove(&repository_id);
    } else {
        overlay
            .project_checks_by_repository
            .insert(repository_id, checks);
    }
    overlay.save(&path).map_err(|error| error.to_string())?;
    let body = project_checks_json(&repo)?;
    write_response(stream, request, 200, "application/json", &body)
}

fn project_checks_json(repo: &str) -> Result<String, String> {
    let path = desktop_settings_config_path(None)?;
    let overlay = workspace_engine::ConfigOverlay::load_or_default(&path)
        .map_err(|error| error.to_string())?;
    let repository_id = workspace_engine::repository_id_for_root(Path::new(repo));
    let own = overlay
        .project_checks_by_repository
        .get(&repository_id)
        .cloned()
        .unwrap_or_default();
    let effective = config_for_repo(repo)?.project_checks_for(Path::new(repo));
    let list = |checks: &[workspace_engine::ProjectCheck]| {
        checks
            .iter()
            .map(|check| {
                format!(
                    "{{\"workingDirectory\":\"{}\",\"command\":\"{}\",\"entry\":\"{}\"}}",
                    escape_json(&check.working_directory),
                    escape_json(&check.command),
                    escape_json(&check.to_entry())
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    Ok(format!(
        "{{\"configPath\":\"{}\",\"repositoryId\":\"{}\",\"checks\":[{}],\"effective\":[{}]}}",
        escape_json(&path.to_string_lossy()),
        escape_json(&repository_id),
        list(&own),
        list(&effective)
    ))
}
