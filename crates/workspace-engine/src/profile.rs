//! Named permission profiles (spec 31, `docs/specs/31_permission_profiles/`).
//!
//! A profile is a bundle of capability-key values applied after admin config
//! at [`ConfigScope::Profile`](crate::config::ConfigScope::Profile), which is
//! restrict-only: a profile can narrow what user, repository and admin config
//! resolved, never loosen it (`context.md` §4). The selection is the user's,
//! stored in user config per checkout as `permission_profile.<repository_id>`.

use crate::audit::AuditLog;
use crate::config::{
    CommandAccess, Config, ConfigOverlay, ConfigScope, McpServerConfigOverlay, RejectedConfigKey,
    RepositoryConfigReport, RepositoryKeyClass,
};
use crate::error::{ClientError, Result};
use crate::hash::repository_id_for_root;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_CUSTOM_NAME_CHARS: usize = 40;

/// Which profile a checkout runs under. No selection behaves as
/// [`ProfileId::Full`], the empty overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileId {
    ReadOnly,
    SafeLocal,
    Full,
    OfflinePrivate,
    /// A user-authored overlay file under `<data_dir>/config/profiles/`.
    Custom(String),
}

impl ProfileId {
    /// A built-in id, or a custom name of `[a-z0-9_]{1,40}`.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "read_only" => Ok(ProfileId::ReadOnly),
            "safe_local" => Ok(ProfileId::SafeLocal),
            "full" => Ok(ProfileId::Full),
            "offline_private" => Ok(ProfileId::OfflinePrivate),
            _ => Self::custom(value),
        }
    }

    /// A custom profile name. The built-in ids are reserved, so an imported
    /// file can never be mistaken for one.
    pub fn custom(name: &str) -> Result<Self> {
        if matches!(
            name,
            "read_only" | "safe_local" | "full" | "offline_private"
        ) {
            return Err(ClientError::InvalidInput(format!(
                "{name} is a built-in permission profile and cannot be used as a custom name"
            )));
        }
        if !is_valid_custom_name(name) {
            return Err(ClientError::InvalidInput(format!(
                "A permission profile name must be 1 to {MAX_CUSTOM_NAME_CHARS} characters \
                 of a-z, 0-9 and _"
            )));
        }
        Ok(ProfileId::Custom(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        match self {
            ProfileId::ReadOnly => "read_only",
            ProfileId::SafeLocal => "safe_local",
            ProfileId::Full => "full",
            ProfileId::OfflinePrivate => "offline_private",
            ProfileId::Custom(name) => name,
        }
    }

    /// The name a person reads, as `context.md` §3 names the built-ins. A
    /// custom profile is shown by its own name.
    pub fn label(&self) -> &str {
        match self {
            ProfileId::ReadOnly => "Read-only",
            ProfileId::SafeLocal => "Safe local development",
            ProfileId::Full => "Full repository development",
            ProfileId::OfflinePrivate => "Offline private",
            ProfileId::Custom(name) => name,
        }
    }

    /// Where a custom profile's file lives; `None` for a built-in. The name is
    /// checked again here because `Custom` can be built without [`Self::parse`],
    /// and it becomes a path segment.
    pub fn custom_path(&self, data_dir: impl AsRef<Path>) -> Option<PathBuf> {
        match self {
            ProfileId::Custom(name) if is_valid_custom_name(name) => Some(
                data_dir
                    .as_ref()
                    .join("config")
                    .join("profiles")
                    .join(format!("{name}.conf")),
            ),
            _ => None,
        }
    }

    /// The profile's overlay, and the lines of a custom file that did not
    /// parse. A custom file is parsed like repository config, because Task 8
    /// imports them: a bad line is reported and skipped, not fatal. A missing
    /// file is an error, because resolving a selected profile as Full would
    /// silently widen what the user chose.
    pub fn overlay(
        &self,
        data_dir: impl AsRef<Path>,
    ) -> Result<(ConfigOverlay, Vec<RejectedConfigKey>)> {
        // The values in `context.md` §3's table.
        let overlay = match self {
            ProfileId::ReadOnly => ConfigOverlay {
                allow_file_edits: Some(false),
                command_access: Some(CommandAccess::None),
                allow_browser_diagnostics: Some(false),
                allow_mutating_mcp_tools: Some(false),
                ..ConfigOverlay::default()
            },
            ProfileId::SafeLocal => ConfigOverlay {
                command_access: Some(CommandAccess::Local),
                require_approval_for_file_edits: Some(true),
                ..ConfigOverlay::default()
            },
            ProfileId::Full => ConfigOverlay::default(),
            ProfileId::OfflinePrivate => ConfigOverlay {
                command_access: Some(CommandAccess::Local),
                allow_browser_diagnostics: Some(false),
                mcp_enabled: Some(false),
                audit_retention_days: Some(7),
                checkpoint_retention_days: Some(7),
                ..ConfigOverlay::default()
            },
            ProfileId::Custom(name) => {
                let path = self.custom_path(&data_dir).ok_or_else(|| {
                    ClientError::InvalidInput(format!("Invalid permission profile name: {name}"))
                })?;
                let content = fs::read_to_string(&path).map_err(|error| {
                    ClientError::InvalidInput(format!(
                        "Permission profile {name} is selected but {} cannot be read ({error}). \
                         Restore the file, or choose another profile with \
                         `damaian profile-set <repo> full`.",
                        path.display()
                    ))
                })?;
                return Ok(ConfigOverlay::parse_untrusted(&content));
            }
        };
        Ok((overlay, Vec::new()))
    }
}

fn is_valid_custom_name(name: &str) -> bool {
    (1..=MAX_CUSTOM_NAME_CHARS).contains(&name.len())
        && name
            .chars()
            .all(|character| matches!(character, 'a'..='z' | '0'..='9' | '_'))
}

/// The resolved values the refusal points consult (spec 31 Task 5): the four
/// profile keys plus the MCP kill-switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileCapabilities {
    pub allow_file_edits: bool,
    pub command_access: CommandAccess,
    pub allow_browser_diagnostics: bool,
    pub allow_mutating_mcp_tools: bool,
    pub mcp_enabled: bool,
}

/// The result of [`select_profile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSelection {
    pub repository_id: String,
    pub previous: Option<ProfileId>,
    pub selected: ProfileId,
}

/// Writes `permission_profile.<repository_id>=<id>` to user config and audits
/// the change. A custom profile must already exist, since selecting one that
/// does not would make every later load of this checkout fail.
///
/// `config` supplies the data directory and user config path; it should be
/// loaded without a repository, so a checkout whose current profile file is
/// missing can still be switched away from it.
pub fn select_profile(
    config: &Config,
    repository_root: &Path,
    id: ProfileId,
    audit_log: &AuditLog,
) -> Result<ProfileSelection> {
    if let ProfileId::Custom(name) = &id {
        let exists = id
            .custom_path(&config.data_dir)
            .is_some_and(|path| path.is_file());
        if !exists {
            return Err(ClientError::InvalidInput(format!(
                "No custom permission profile named {name}"
            )));
        }
    }
    let repository_id = repository_id_for_root(repository_root);
    let path = config.user_config_path();
    let mut overlay = ConfigOverlay::load_or_default(&path)?;
    let previous = overlay
        .permission_profile_by_repository
        .insert(repository_id.clone(), id.clone());
    overlay.save(&path)?;
    audit_log.record(
        "permission_profile_set",
        &[
            ("actor", "user".to_string()),
            ("repositoryId", repository_id.clone()),
            (
                "repositoryPath",
                repository_root.to_string_lossy().to_string(),
            ),
            (
                "from",
                previous
                    .as_ref()
                    .map_or("none", ProfileId::as_str)
                    .to_string(),
            ),
            ("to", id.as_str().to_string()),
        ],
    )?;
    Ok(ProfileSelection {
        repository_id,
        previous,
        selected: id,
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileReviewState {
    #[serde(default)]
    reported_keys: Vec<String>,
}

/// Audits the keys the selected profile carried and could not apply, each once
/// per profile, and returns the ones not reported before.
///
/// Kept apart from `RepositoryTrustStore::review` on purpose: a profile's
/// refusals are not a repository's, so they must not reach spec 34's
/// repository notice. Once per profile rather than per load for the same
/// reason as that store: config is loaded on every request.
pub fn review_profile_rejections(
    data_dir: &Path,
    report: &RepositoryConfigReport,
    audit_log: &AuditLog,
) -> Result<Vec<RejectedConfigKey>> {
    let Some(profile) = report.permission_profile.as_ref() else {
        return Ok(Vec::new());
    };
    if report.profile_rejected_keys.is_empty() {
        return Ok(Vec::new());
    }
    let state_path = data_dir
        .join("config")
        .join("profile-review")
        .join(format!("{}.json", profile.as_str()));
    // A corrupt state file must not make a repository unopenable; the worst
    // case of treating it as absent is a key audited twice.
    let mut state: ProfileReviewState = fs::read_to_string(&state_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();
    let fresh = report
        .profile_rejected_keys
        .iter()
        .filter(|rejected| !state.reported_keys.contains(&rejected.key))
        .cloned()
        .collect::<Vec<_>>();
    if fresh.is_empty() {
        return Ok(fresh);
    }

    let repository_id = report
        .repository_root
        .as_ref()
        .map(repository_id_for_root)
        .unwrap_or_default();
    for rejected in &fresh {
        audit_log.record(
            "permission_profile_key_rejected",
            &[
                ("actor", "system".to_string()),
                ("profileId", profile.as_str().to_string()),
                ("repositoryId", repository_id.clone()),
                ("key", rejected.key.clone()),
                ("class", rejected.class.as_str().to_string()),
            ],
        )?;
        state.reported_keys.push(rejected.key.clone());
    }
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string(&state).map_err(|error| {
        ClientError::InvalidInput(format!(
            "Failed to serialize permission profile review state: {error}"
        ))
    })?;
    fs::write(state_path, json)?;
    Ok(fresh)
}

/// Splits an overlay into the keys a profile may carry and the ones it may
/// not (spec 31, `context.md` §4 and §9). Export and import both go through
/// it, so an exported file cannot carry a credential reference and an
/// imported one cannot smuggle in a redirecting key.
///
/// It agrees with what [`Config::apply_overlay_scoped`] refuses at
/// [`ConfigScope::Profile`] for any reason other than direction, and a test
/// holds the two together. Exhaustive, deliberately without `..`, so a new
/// field is a decision here too. A refused key gets the class the merge would
/// give it, and never its value.
pub fn split_profile_keys(overlay: ConfigOverlay) -> (ConfigOverlay, Vec<RejectedConfigKey>) {
    let ConfigOverlay {
        data_dir,
        max_file_bytes,
        max_read_lines,
        max_list_entries,
        max_search_matches,
        max_match_line_chars,
        max_command_output_bytes,
        command_timeout_secs,
        allowed_roots,
        ignore_patterns,
        restricted_patterns,
        command_allowlist,
        command_allowlist_by_repository,
        permission_profile_by_repository,
        command_blocklist,
        secret_patterns,
        require_approval_for_file_edits,
        require_approval_for_risky_commands,
        require_approval_for_all_commands,
        allow_file_edits,
        command_access,
        allow_browser_diagnostics,
        allow_mutating_mcp_tools,
        block_generated_secrets,
        audit_enabled,
        audit_retention_days,
        checkpoint_retention_days,
        checkpoint_max_total_bytes,
        checkpoint_census_max_paths,
        enable_semantic_search,
        agent_max_tool_rounds,
        agent_web_debug_max_tool_rounds,
        agent_tool_retry_limit,
        agent_max_task_tokens,
        agent_max_turn_messages,
        project_roots_added,
        project_roots_removed,
        shell,
        model_provider,
        model_name,
        model_base_url,
        model_api_key_env,
        model_reasoning_level,
        model_providers,
        mcp_enabled,
        mcp_server_allowlist,
        mcp_servers,
    } = overlay;

    let mut refused = Vec::new();
    let mut refuse = |key: String, class: RepositoryKeyClass| {
        refused.push(RejectedConfigKey { key, class });
    };
    // Redirecting keys, budgets a profile may not touch, and preferences: the
    // merge refuses a preference at profile scope as Forbidden too.
    for (key, present) in [
        ("data_dir", data_dir.is_some()),
        ("allowed_roots", allowed_roots.is_some()),
        ("secret_patterns", secret_patterns.is_some()),
        ("block_generated_secrets", block_generated_secrets.is_some()),
        ("audit_enabled", audit_enabled.is_some()),
        ("shell", shell.is_some()),
        ("model_provider", model_provider.is_some()),
        ("model_name", model_name.is_some()),
        ("model_base_url", model_base_url.is_some()),
        ("model_api_key_env", model_api_key_env.is_some()),
        ("model_reasoning_level", model_reasoning_level.is_some()),
        (
            "checkpoint_max_total_bytes",
            checkpoint_max_total_bytes.is_some(),
        ),
        (
            "checkpoint_census_max_paths",
            checkpoint_census_max_paths.is_some(),
        ),
        ("max_file_bytes", max_file_bytes.is_some()),
        (
            "max_command_output_bytes",
            max_command_output_bytes.is_some(),
        ),
        ("enable_semantic_search", enable_semantic_search.is_some()),
        ("agent_max_tool_rounds", agent_max_tool_rounds.is_some()),
        (
            "agent_web_debug_max_tool_rounds",
            agent_web_debug_max_tool_rounds.is_some(),
        ),
        ("agent_tool_retry_limit", agent_tool_retry_limit.is_some()),
        ("project_roots_added", project_roots_added.is_some()),
        ("project_roots_removed", project_roots_removed.is_some()),
    ] {
        if present {
            refuse(key.to_string(), RepositoryKeyClass::Forbidden);
        }
    }
    for provider in model_providers {
        refuse(
            format!("model_provider.{}", provider.id),
            RepositoryKeyClass::Forbidden,
        );
    }
    // The user's own decisions about a checkout.
    if command_allowlist.is_some() {
        refuse(
            "command_allowlist".to_string(),
            RepositoryKeyClass::UserOwned,
        );
    }
    for repository_id in command_allowlist_by_repository.keys() {
        refuse(
            format!("command_allowlist.{repository_id}"),
            RepositoryKeyClass::UserOwned,
        );
    }
    for repository_id in permission_profile_by_repository.keys() {
        refuse(
            format!("permission_profile.{repository_id}"),
            RepositoryKeyClass::UserOwned,
        );
    }
    // A profile may disable or gate a server the user has, never define one:
    // the definition is where `auth_token_env` lives.
    let mut carried_servers = Vec::new();
    for server in mcp_servers {
        let McpServerConfigOverlay {
            id,
            label,
            transport,
            command,
            args,
            env,
            url,
            auth_token_env,
            enabled,
            require_approval,
        } = server;
        for (field, present) in [
            ("label", label.is_some()),
            ("transport", transport.is_some()),
            ("command", command.is_some()),
            ("args", args.is_some()),
            ("env", env.is_some()),
            ("url", url.is_some()),
            ("auth_token_env", auth_token_env.is_some()),
        ] {
            if present {
                refuse(
                    format!("mcp_server.{id}.{field}"),
                    RepositoryKeyClass::Forbidden,
                );
            }
        }
        if enabled.is_some() || require_approval.is_some() {
            carried_servers.push(McpServerConfigOverlay {
                id,
                enabled,
                require_approval,
                ..McpServerConfigOverlay::default()
            });
        }
    }

    let carried = ConfigOverlay {
        max_read_lines,
        max_list_entries,
        max_search_matches,
        max_match_line_chars,
        command_timeout_secs,
        ignore_patterns,
        restricted_patterns,
        command_blocklist,
        require_approval_for_file_edits,
        require_approval_for_risky_commands,
        require_approval_for_all_commands,
        allow_file_edits,
        command_access,
        allow_browser_diagnostics,
        allow_mutating_mcp_tools,
        audit_retention_days,
        checkpoint_retention_days,
        agent_max_task_tokens,
        agent_max_turn_messages,
        mcp_enabled,
        mcp_server_allowlist,
        mcp_servers: carried_servers,
        ..ConfigOverlay::default()
    };
    (carried, refused)
}

/// A profile as a file someone can share: only the keys a profile may carry,
/// through the exhaustive overlay serializer (`context.md` §9). A built-in
/// reads nothing from disk; a custom profile reads its own file, never user
/// config, so no credential reference can reach the text.
pub fn export_profile(id: &ProfileId, data_dir: &Path) -> Result<String> {
    let (overlay, _) = id.overlay(data_dir)?;
    let (carried, _) = split_profile_keys(overlay);
    Ok(format!(
        "# Damaian permission profile: {}\n{}",
        id.as_str(),
        carried.to_policy_text()
    ))
}

/// The custom profiles in `<data_dir>/config/profiles/`, by name. A file whose
/// name is not a valid custom id, or is a reserved one, is not a profile.
pub fn custom_profile_ids(data_dir: &Path) -> Result<Vec<ProfileId>> {
    let directory = data_dir.join("config").join("profiles");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if !path.is_file() || path.extension().is_none_or(|extension| extension != "conf") {
            continue;
        }
        if let Some(id) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| ProfileId::custom(stem).ok())
        {
            ids.push(id);
        }
    }
    ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok(ids)
}

/// The config a profile would narrow for this checkout: user, repository and
/// admin config, without the selected profile. An import compares against
/// this, so a key is called loosening for what it is, not for what the
/// currently selected profile already narrowed.
pub fn profile_import_base(repository_root: Option<&Path>) -> Result<Config> {
    let config = Config::default();
    let user_path = config.user_config_path();
    let admin_path = config.admin_config_path();
    let repository_path = repository_root.map(Config::repository_config_path);
    let (base, _) = Config::load_scoped(
        config,
        Some(&user_path),
        repository_path.as_deref(),
        Some(&admin_path),
        None,
    )?;
    Ok(base)
}

/// What an import would write and what it would not apply, before anything
/// is written (`context.md` §9). Key names and classes only: an imported file
/// is untrusted input, so no refused value is kept.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileImportReview {
    /// The keys the written file carries, as it names them.
    pub carried: Vec<String>,
    /// Keys a profile cannot carry, and lines that did not parse. Not written.
    pub not_carried: Vec<RejectedConfigKey>,
    /// Keys written but with no effect against the base config, because they
    /// would loosen it and a profile only narrows.
    pub loosening: Vec<RejectedConfigKey>,
    overlay: ConfigOverlay,
}

/// Reviews an import against `base`, the config the profile would narrow
/// ([`profile_import_base`]). Writes nothing.
pub fn review_profile_import(base: &Config, text: &str) -> ProfileImportReview {
    let (overlay, mut not_carried) = ConfigOverlay::parse_untrusted(text);
    let (carried, refused) = split_profile_keys(overlay);
    not_carried.extend(refused);
    // Applying the carried keys at profile scope is the merge that will run
    // when the profile is selected, so its direction refusals are exactly the
    // keys that would have no effect.
    let mut probe = base.clone();
    let loosening = probe
        .apply_overlay_scoped(carried.clone(), ConfigScope::Profile)
        .rejected
        .into_iter()
        .filter(|rejected| rejected.class == RepositoryKeyClass::RestrictOnly)
        .filter(|rejected| !equals_base_limit(&rejected.key, &carried, base))
        .collect();
    let carried_keys = carried
        .to_policy_text()
        .lines()
        .filter_map(|line| line.split_once('=').map(|(key, _)| key.to_string()))
        .collect();
    ProfileImportReview {
        carried: carried_keys,
        not_carried,
        loosening,
        overlay: carried,
    }
}

/// The merge refuses a limit or ceiling equal to the current one at an
/// untrusted scope (spec 31 Task 3's note for Task 8). Equal changes nothing
/// and loosens nothing, so it is not listed as loosening.
fn equals_base_limit(key: &str, carried: &ConfigOverlay, base: &Config) -> bool {
    match key {
        "max_read_lines" => carried.max_read_lines == Some(base.max_read_lines),
        "max_list_entries" => carried.max_list_entries == Some(base.max_list_entries),
        "max_search_matches" => carried.max_search_matches == Some(base.max_search_matches),
        "max_match_line_chars" => carried.max_match_line_chars == Some(base.max_match_line_chars),
        "command_timeout_secs" => carried.command_timeout_secs == Some(base.command_timeout_secs),
        "agent_max_turn_messages" => {
            carried.agent_max_turn_messages == Some(base.agent_max_turn_messages)
        }
        "agent_max_task_tokens" => {
            carried.agent_max_task_tokens.is_some()
                && carried.agent_max_task_tokens == base.agent_max_task_tokens
        }
        _ => false,
    }
}

/// The result of [`import_profile`].
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileImport {
    pub id: ProfileId,
    pub path: PathBuf,
    pub replaced: bool,
    pub review: ProfileImportReview,
}

/// Imports `text` as the custom profile `name`: reviews it against `base`,
/// writes only the keys a profile may carry to
/// `<data_dir>/config/profiles/<name>.conf`, and audits
/// `permission_profile_imported` with counts and key names. It never touches
/// user config and never selects the profile.
///
/// A reserved id is refused, and so is an existing profile unless `replace`.
pub fn import_profile(
    base: &Config,
    text: &str,
    name: &str,
    replace: bool,
    audit_log: &AuditLog,
) -> Result<ProfileImport> {
    let id = ProfileId::custom(name)?;
    let path = id.custom_path(&base.data_dir).ok_or_else(|| {
        ClientError::InvalidInput(format!("Invalid permission profile name: {name}"))
    })?;
    let review = review_profile_import(base, text);
    if review.carried.is_empty() {
        return Err(ClientError::InvalidInput(
            "The file sets no key a permission profile can carry, so there is nothing to import"
                .to_string(),
        ));
    }
    let replaced = path.exists();
    if replaced && !replace {
        return Err(ClientError::InvalidInput(format!(
            "A custom permission profile named {name} already exists. Choose another name, \
             or replace it."
        )));
    }
    review.overlay.save(&path)?;
    // The once-per-key record belongs to the file it was made for: the new
    // file's refusals must be audited afresh.
    let review_state = base
        .data_dir
        .join("config")
        .join("profile-review")
        .join(format!("{name}.json"));
    if review_state.exists() {
        fs::remove_file(review_state)?;
    }
    let names = |keys: &[RejectedConfigKey]| {
        keys.iter()
            .map(|rejected| rejected.key.as_str())
            .collect::<Vec<_>>()
            .join(",")
    };
    audit_log.record(
        "permission_profile_imported",
        &[
            ("actor", "user".to_string()),
            ("profileId", id.as_str().to_string()),
            ("replaced", replaced.to_string()),
            ("carriedCount", review.carried.len().to_string()),
            ("notCarriedCount", review.not_carried.len().to_string()),
            ("notCarriedKeys", names(&review.not_carried)),
            ("looseningCount", review.loosening.len().to_string()),
            ("looseningKeys", names(&review.loosening)),
        ],
    )?;
    Ok(ProfileImport {
        id,
        path,
        replaced,
        review,
    })
}
