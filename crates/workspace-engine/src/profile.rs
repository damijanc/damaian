//! Named permission profiles (spec 31, `docs/specs/31_permission_profiles/`).
//!
//! A profile is a bundle of capability-key values applied after admin config
//! at [`ConfigScope::Profile`](crate::config::ConfigScope::Profile), which is
//! restrict-only: a profile can narrow what user, repository and admin config
//! resolved, never loosen it (`context.md` §4). The selection is the user's,
//! stored in user config per checkout as `permission_profile.<repository_id>`.

use crate::audit::AuditLog;
use crate::config::{
    CommandAccess, Config, ConfigOverlay, RejectedConfigKey, RepositoryConfigReport,
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
