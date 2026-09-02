//! What the user has been told, and has decided, about one repository's
//! configuration.
//!
//! Repository config is untrusted input (spec 34). The overlay layer refuses
//! the keys that would weaken the user's policy and reports them; this module
//! is the memory that turns those reports into something the user sees exactly
//! once, and holds the one-time migration of `command_allowlist` entries that
//! predate the boundary.

use crate::audit::AuditLog;
use crate::config::{Config, ConfigOverlay, RejectedConfigKey, RepositoryConfigReport};
use crate::error::{ClientError, Result};
use crate::hash::repository_id_for_root;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Keys a repository's config asked for and did not get, worth telling the
/// user about because a repository attempting to set `shell` or
/// `model_base_url` is information about that repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryConfigNotice {
    pub repository_root: PathBuf,
    pub repository_id: String,
    pub rejected: Vec<RejectedConfigKey>,
}

impl RepositoryConfigNotice {
    pub fn rejected_key_names(&self) -> Vec<&str> {
        self.rejected
            .iter()
            .map(|rejected| rejected.key.as_str())
            .collect()
    }
}

/// `command_allowlist` entries found in a repository's config, offered once
/// for itemised keep-or-discard. Damaian genuinely cannot tell which of these
/// the user created through `Allow Always` and which arrived with the clone,
/// so it asks rather than guessing in the permissive direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryAllowlistMigration {
    pub repository_root: PathBuf,
    pub repository_id: String,
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustState {
    /// Recorded for the human reading the file; identity is the file name.
    #[serde(default)]
    repository_path: String,
    #[serde(default)]
    reported_keys: Vec<String>,
    #[serde(default)]
    allowlist_migration_answered: bool,
}

#[derive(Debug, Clone)]
pub struct RepositoryTrustStore {
    data_dir: PathBuf,
}

impl RepositoryTrustStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    /// Audits every newly refused key and returns the notice to show, or
    /// `None` when this repository's refused keys have already been reported.
    ///
    /// Auditing happens here rather than at every config load because config
    /// is loaded per request: the audit trail wants one entry per key per
    /// repository, not one per keystroke. A key the repository adds later is
    /// new, so it is audited and shown then.
    pub fn review(
        &self,
        report: &RepositoryConfigReport,
        audit_log: &AuditLog,
    ) -> Result<Option<RepositoryConfigNotice>> {
        let Some(root) = report.repository_root.as_ref() else {
            return Ok(None);
        };
        if report.rejected_keys.is_empty() {
            return Ok(None);
        }
        let repository_id = repository_id_for_root(root);
        let mut state = self.load_state(&repository_id)?;
        let fresh = report
            .rejected_keys
            .iter()
            .filter(|rejected| !state.reported_keys.contains(&rejected.key))
            .cloned()
            .collect::<Vec<_>>();
        if fresh.is_empty() {
            return Ok(None);
        }

        let resource_path = Config::repository_config_path(root)
            .to_string_lossy()
            .to_string();
        for rejected in &fresh {
            audit_log.record(
                "repository_config_key_rejected",
                &[
                    ("actor", "system".to_string()),
                    ("repositoryId", repository_id.clone()),
                    ("resourcePath", resource_path.clone()),
                    ("key", rejected.key.clone()),
                    ("class", rejected.class.as_str().to_string()),
                ],
            )?;
            state.reported_keys.push(rejected.key.clone());
        }
        state.repository_path = root.to_string_lossy().to_string();
        self.save_state(&repository_id, &state)?;

        Ok(Some(RepositoryConfigNotice {
            repository_root: root.clone(),
            repository_id,
            rejected: fresh,
        }))
    }

    /// The allowlist entries still awaiting the user's decision, if any.
    pub fn pending_allowlist_migration(
        &self,
        report: &RepositoryConfigReport,
    ) -> Result<Option<RepositoryAllowlistMigration>> {
        let Some(root) = report.repository_root.as_ref() else {
            return Ok(None);
        };
        if report.repository_allowlist_entries.is_empty() {
            return Ok(None);
        }
        let repository_id = repository_id_for_root(root);
        if self
            .load_state(&repository_id)?
            .allowlist_migration_answered
        {
            return Ok(None);
        }
        Ok(Some(RepositoryAllowlistMigration {
            repository_root: root.clone(),
            repository_id,
            entries: report.repository_allowlist_entries.clone(),
        }))
    }

    /// Records the user's itemised decision. `kept` entries are written to user
    /// config under this repository's key; everything else is discarded. The
    /// repository's own file is never touched, so a user who declines loses
    /// nothing but the automatic approval. Returns the user config path.
    pub fn resolve_allowlist_migration(
        &self,
        report: &RepositoryConfigReport,
        kept: &[String],
        audit_log: &AuditLog,
    ) -> Result<PathBuf> {
        let root = report.repository_root.clone().ok_or_else(|| {
            ClientError::InvalidInput(
                "No repository config to migrate allowlist entries from".to_string(),
            )
        })?;
        for entry in kept {
            if !report
                .repository_allowlist_entries
                .iter()
                .any(|offered| offered.trim() == entry.trim())
            {
                return Err(ClientError::InvalidInput(format!(
                    "Command was not offered by this repository's config: {entry}"
                )));
            }
        }

        let repository_id = repository_id_for_root(&root);
        let path = self.user_config_path();
        let mut overlay = ConfigOverlay::load_or_default(&path)?;
        let mut allowlist = overlay
            .command_allowlist_by_repository
            .remove(&repository_id)
            .unwrap_or_default();
        for entry in kept {
            let entry = entry.trim().to_string();
            if !allowlist.iter().any(|existing| existing.trim() == entry) {
                allowlist.push(entry);
            }
        }
        if !allowlist.is_empty() {
            overlay
                .command_allowlist_by_repository
                .insert(repository_id.clone(), allowlist);
        }
        overlay.save(&path)?;

        let mut state = self.load_state(&repository_id)?;
        state.repository_path = root.to_string_lossy().to_string();
        state.allowlist_migration_answered = true;
        self.save_state(&repository_id, &state)?;

        audit_log.record(
            "repository_allowlist_migrated",
            &[
                ("actor", "user".to_string()),
                ("repositoryId", repository_id),
                (
                    "offeredCount",
                    report.repository_allowlist_entries.len().to_string(),
                ),
                ("keptCount", kept.len().to_string()),
                ("kept", kept.join("|")),
                ("resourcePath", path.to_string_lossy().to_string()),
            ],
        )?;
        Ok(path)
    }

    fn user_config_path(&self) -> PathBuf {
        self.data_dir.join("config").join("user.conf")
    }

    fn state_path(&self, repository_id: &str) -> PathBuf {
        self.data_dir
            .join("config")
            .join("repository-trust")
            .join(format!("{repository_id}.json"))
    }

    fn load_state(&self, repository_id: &str) -> Result<TrustState> {
        let path = self.state_path(repository_id);
        if !path.exists() {
            return Ok(TrustState::default());
        }
        let content = fs::read_to_string(&path)?;
        // A corrupt state file must not make a repository unopenable: the
        // worst case of treating it as absent is that a notice is shown twice.
        Ok(serde_json::from_str(&content).unwrap_or_default())
    }

    fn save_state(&self, repository_id: &str, state: &TrustState) -> Result<()> {
        let path = self.state_path(repository_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(state).map_err(|error| {
            ClientError::InvalidInput(format!(
                "Failed to serialize repository trust state: {error}"
            ))
        })?;
        fs::write(path, json)?;
        Ok(())
    }
}
