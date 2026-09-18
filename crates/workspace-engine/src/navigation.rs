//! Listing and content search, as engine tools.
//!
//! Spec 47 requirement 2. Both are visitors over [`crate::tree_walk::walk`],
//! and both resolve through [`PathPolicy`] and redact through [`SecretScanner`]
//! on the same path `FileAccessController::read_file` uses.
//!
//! Requirement 3 is met by where this sits rather than by a policy decision:
//! these are engine tools dispatched in `chat.rs`, so nothing here reaches
//! `command_policy.rs`. The agent gains navigation *as a tool* and gains
//! nothing *at the shell* — no `command_allowlist` entry, no change to
//! `is_low_risk_read_only`, no relaxation of the shell-control gate.

use crate::audit::AuditLog;
use crate::config::{Config, DEFAULT_IGNORE_PATTERNS};
use crate::error::Result;
use crate::ignore::parse_ignore_patterns;
use crate::path_policy::PathPolicy;
use crate::secret_scanner::SecretScanner;
use crate::tree_walk::{WalkEvent, walk};
use std::path::Path;

/// Repository-relative paths, with what the cap cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListing {
    pub paths: Vec<String>,
    /// What the walk found before the cap applied, so a truncated listing can
    /// say "200 of 251" rather than reading as the whole repository. Spec 47
    /// §5.5: a truncated result that reads as complete is the failure mode this
    /// design exists to prevent.
    pub total_found: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct NavigationController {
    config: Config,
    audit_log: AuditLog,
    #[allow(dead_code)] // Used by `search_content` in the next task.
    scanner: SecretScanner,
    path_policy: PathPolicy,
}

impl NavigationController {
    pub fn new(
        config: Config,
        audit_log: AuditLog,
        scanner: SecretScanner,
        path_policy: PathPolicy,
    ) -> Self {
        Self {
            config,
            audit_log,
            scanner,
            path_policy,
        }
    }

    /// Lists repository-relative file paths under `dir`, honouring the ignore
    /// rules and refusing restricted paths.
    ///
    /// Paths only — no file is read, so there is nothing to redact. The
    /// restricted check still matters because a *path* can name a secret
    /// (`deploy/prod-key.pem`), which is why it is applied per entry rather
    /// than only to the starting directory.
    pub fn list_directory(
        &self,
        root_path: impl AsRef<Path>,
        dir: Option<&str>,
        depth: Option<usize>,
        task_id: Option<&str>,
        repository_id: Option<&str>,
    ) -> Result<DirectoryListing> {
        let requested = dir.unwrap_or(".");
        // `resolve_existing` with `allow_outside_root: false` is what refuses
        // `../..`, on the same path a read would take.
        let target = self
            .path_policy
            .resolve_existing(&root_path, requested, false)?;
        self.path_policy
            .assert_not_restricted(&target.relative_path, false)?;

        let rules = parse_ignore_patterns(&self.ignore_patterns(), "");
        let base = if target.relative_path == "." {
            String::new()
        } else {
            target.relative_path.clone()
        };

        let mut paths = Vec::new();
        let mut total_found = 0usize;
        walk(
            &target.root,
            &target.absolute_path,
            &base,
            &rules,
            &mut |event| {
                let WalkEvent::File(file) = event else {
                    return Ok(());
                };
                if self
                    .path_policy
                    .assert_not_restricted(&file.relative_path, false)
                    .is_err()
                {
                    return Ok(());
                }
                if let Some(limit) = depth
                    && file.relative_path.matches('/').count() > limit
                {
                    return Ok(());
                }
                total_found += 1;
                if paths.len() < self.config.max_list_entries {
                    paths.push(file.relative_path.clone());
                }
                Ok(())
            },
        )?;

        let listing = DirectoryListing {
            truncated: total_found > paths.len(),
            paths,
            total_found,
        };

        self.audit_log.record(
            "directory_listed",
            &[
                ("actor", "assistant".to_string()),
                (
                    "repositoryId",
                    repository_id.unwrap_or_default().to_string(),
                ),
                ("taskId", task_id.unwrap_or_default().to_string()),
                ("resourcePath", target.relative_path.clone()),
                ("status", "allowed".to_string()),
                ("entryCount", listing.paths.len().to_string()),
                ("totalFound", listing.total_found.to_string()),
            ],
        )?;

        Ok(listing)
    }

    /// The same resolution `index_repository` uses, so a listing and the index
    /// agree about what the repository contains.
    fn ignore_patterns(&self) -> Vec<String> {
        if self.config.ignore_patterns.is_empty() {
            DEFAULT_IGNORE_PATTERNS
                .iter()
                .map(|pattern| pattern.to_string())
                .collect()
        } else {
            self.config.ignore_patterns.clone()
        }
    }
}
