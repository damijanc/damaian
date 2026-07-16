use crate::audit::AuditLog;
use crate::config::Config;
use crate::diff::{Hunk, diff_file, reconstruct_content};
use crate::error::{ClientError, Result};
use crate::hash::{create_id, file_hash, now_millis, sha256};
use crate::path_policy::PathPolicy;
use crate::secret_scanner::SecretScanner;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedFilePatch {
    pub path: String,
    pub status: String,
    pub base_hash: Option<String>,
    pub new_content: String,
    pub new_hash: String,
    pub diff: String,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedPatch {
    pub id: String,
    pub task_id: Option<String>,
    pub summary: String,
    pub status: String,
    pub created_at_ms: u128,
    pub files: Vec<ProposedFilePatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchApplyResult {
    pub patch_id: String,
    pub applied_files: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchRollbackResult {
    pub patch_id: String,
    pub restored_files: Vec<String>,
    pub deleted_files: Vec<String>,
    pub warnings: Vec<String>,
}

/// Pre-apply per-file snapshot used to roll back an applied patch. `content`
/// has already passed through secret redaction at capture time (see
/// `apply_patch`), so a genuine secret value that existed before the patch
/// can never be restored from this snapshot alone — rollback is best-effort
/// and surfaces a warning for any file whose restored content still contains
/// a redaction placeholder.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RollbackSnapshot {
    path: String,
    base_hash: String,
    /// Whether the file existed before the patch was applied. When false,
    /// the file was newly created by the patch, so rollback deletes it
    /// instead of writing back `content`.
    existed: bool,
    captured_at_ms: u128,
    content: String,
    /// Hash of the content actually written to disk when the patch was
    /// applied (which may differ from `ProposedFilePatch::new_hash` if only
    /// some hunks were accepted). Rollback's conflict check compares against
    /// this instead of `new_hash` so partial-hunk applies can still be
    /// safely rolled back.
    applied_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedChange {
    pub path: String,
    pub new_content: String,
    pub status: Option<String>,
    pub allow_restricted: bool,
}

#[derive(Debug, Clone)]
pub struct PatchEngine {
    config: Config,
    audit_log: AuditLog,
    scanner: SecretScanner,
    path_policy: PathPolicy,
}

impl PatchEngine {
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

    pub fn create_patch(
        &self,
        root_path: impl AsRef<Path>,
        changes: &[ProposedChange],
        task_id: Option<&str>,
        summary: &str,
    ) -> Result<ProposedPatch> {
        let mut files = Vec::new();
        for change in changes {
            let target = self
                .path_policy
                .resolve_for_write(&root_path, &change.path)?;
            self.path_policy
                .assert_not_restricted(&target.relative_path, change.allow_restricted)?;
            let old_content = read_existing(&target.absolute_path)?;
            let status = old_content
                .as_ref()
                .map(|_| {
                    change
                        .status
                        .clone()
                        .unwrap_or_else(|| "modified".to_string())
                })
                .unwrap_or_else(|| "added".to_string());
            let file_diff = diff_file(
                old_content.as_deref().unwrap_or_default(),
                &change.new_content,
                &target.relative_path,
            );
            let diff = self.scanner.redact(&file_diff.text).text;
            let hunks = self.redact_hunks(file_diff.hunks);
            files.push(ProposedFilePatch {
                path: target.relative_path.clone(),
                status,
                base_hash: old_content.as_ref().map(sha256),
                new_hash: sha256(change.new_content.as_bytes()),
                diff,
                hunks,
                new_content: change.new_content.clone(),
            });
        }

        let patch = ProposedPatch {
            id: create_id("patch"),
            task_id: task_id.map(|value| value.to_string()),
            summary: summary.to_string(),
            status: "pending".to_string(),
            created_at_ms: now_millis(),
            files,
        };

        self.audit_log.record(
            "patch_proposed",
            &[
                ("actor", "assistant".to_string()),
                ("taskId", task_id.unwrap_or_default().to_string()),
                ("patchId", patch.id.clone()),
                ("status", "pending".to_string()),
                (
                    "files",
                    patch
                        .files
                        .iter()
                        .map(|file| file.path.clone())
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                ("summary", summary.to_string()),
            ],
        )?;

        Ok(patch)
    }

    /// Redacts the display text of each hunk's lines. Position metadata
    /// (`old_start`/`old_lines`/`new_start`/`new_lines`) is left untouched
    /// since `reconstruct_content` only ever uses those offsets against
    /// freshly-recomputed, unredacted content at apply time — never the
    /// stored (possibly redacted) line text.
    fn redact_hunks(&self, hunks: Vec<Hunk>) -> Vec<Hunk> {
        hunks
            .into_iter()
            .map(|hunk| Hunk {
                lines: hunk
                    .lines
                    .into_iter()
                    .map(|line| crate::diff::DiffLine {
                        text: self.scanner.redact(&line.text).text,
                        ..line
                    })
                    .collect(),
                ..hunk
            })
            .collect()
    }

    pub fn apply_patch(
        &self,
        root_path: impl AsRef<Path>,
        patch: &ProposedPatch,
        approved_paths: Option<&[String]>,
        hunk_selection: Option<&HashMap<String, Vec<String>>>,
        approved_by: &str,
        allow_generated_secrets: bool,
    ) -> Result<PatchApplyResult> {
        let selected_paths = approved_paths
            .map(|paths| paths.to_vec())
            .unwrap_or_else(|| patch.files.iter().map(|file| file.path.clone()).collect());
        if selected_paths.is_empty() {
            return Err(ClientError::InvalidInput(
                "No patch files selected for apply".to_string(),
            ));
        }
        let patch_paths = patch
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        if let Some(path) = selected_paths
            .iter()
            .find(|path| !patch_paths.contains(&path.as_str()))
        {
            return Err(ClientError::InvalidInput(format!(
                "Selected patch file was not found: {path}"
            )));
        }
        let selected = patch
            .files
            .iter()
            .filter(|file| selected_paths.contains(&file.path))
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Err(ClientError::InvalidInput(
                "Selected patch files were not found in the stored patch".to_string(),
            ));
        }

        let mut prepared = Vec::new();
        let mut warnings = Vec::new();
        for file in selected {
            let target = self.path_policy.resolve_for_write(&root_path, &file.path)?;
            self.path_policy
                .assert_not_restricted(&target.relative_path, false)?;
            let current_content = read_existing(&target.absolute_path)?;
            let current_hash = current_content.as_ref().map(sha256);
            if current_hash != file.base_hash {
                return Err(ClientError::PatchConflict(format!(
                    "Target file changed after patch generation: {}",
                    file.path
                )));
            }

            let content_to_write = if file.status == "deleted" {
                String::new()
            } else {
                match hunk_selection.and_then(|selection| selection.get(&file.path)) {
                    Some(accepted_hunk_ids) => reconstruct_content(
                        current_content.as_deref().unwrap_or_default(),
                        &file.new_content,
                        &file.hunks,
                        accepted_hunk_ids,
                    ),
                    None => file.new_content.clone(),
                }
            };

            let findings = self.scanner.scan(&content_to_write);
            if !findings.is_empty() {
                warnings.push(format!(
                    "{}: generated_secret:{}",
                    file.path,
                    findings.len()
                ));
                if self.config.block_generated_secrets && !allow_generated_secrets {
                    return Err(ClientError::PolicyBlocked(
                        "Generated content appears to contain a hardcoded secret".to_string(),
                    ));
                }
            }
            prepared.push((file, target.absolute_path, current_content, content_to_write));
        }

        let rollback_dir = self.config.data_dir.join("rollback").join(&patch.id);
        fs::create_dir_all(&rollback_dir)?;
        let mut applied_files = Vec::new();

        for (file, absolute_path, current_content, content_to_write) in prepared {
            let rollback_path = rollback_dir.join(file.path.replace('/', "__"));
            let rollback_content = self
                .scanner
                .redact(current_content.as_deref().unwrap_or_default())
                .text;
            let snapshot = RollbackSnapshot {
                path: file.path.clone(),
                base_hash: file.base_hash.clone().unwrap_or_default(),
                existed: current_content.is_some(),
                captured_at_ms: now_millis(),
                content: rollback_content,
                applied_hash: if file.status == "deleted" {
                    String::new()
                } else {
                    sha256(content_to_write.as_bytes())
                },
            };
            let serialized = serde_json::to_string(&snapshot)
                .map_err(|error| ClientError::Io(format!("Failed to write rollback snapshot: {error}")))?;
            fs::write(&rollback_path, serialized)?;

            if file.status == "deleted" {
                if current_content.is_none() {
                    return Err(ClientError::AccessDenied(
                        "Cannot delete a file that does not exist".to_string(),
                    ));
                }
                fs::remove_file(&absolute_path)?;
            } else {
                if let Some(parent) = absolute_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let temp_path = temp_path_for(&absolute_path);
                fs::write(&temp_path, &content_to_write)?;
                fs::rename(&temp_path, &absolute_path)?;
            }

            self.audit_log.record(
                "file_modified",
                &[
                    ("actor", "system".to_string()),
                    ("taskId", patch.task_id.clone().unwrap_or_default()),
                    ("patchId", patch.id.clone()),
                    ("approvedBy", approved_by.to_string()),
                    ("resourcePath", file.path.clone()),
                    ("status", file.status.clone()),
                    ("baseHash", file.base_hash.clone().unwrap_or_default()),
                    (
                        "newHash",
                        if file.status == "deleted" {
                            String::new()
                        } else {
                            file_hash(&absolute_path)?
                        },
                    ),
                    ("rollbackPath", rollback_path.to_string_lossy().to_string()),
                ],
            )?;
            applied_files.push(file.path.clone());
        }

        self.audit_log.record(
            "patch_applied",
            &[
                ("actor", "system".to_string()),
                ("taskId", patch.task_id.clone().unwrap_or_default()),
                ("patchId", patch.id.clone()),
                ("approvedBy", approved_by.to_string()),
                ("status", "applied".to_string()),
                ("files", applied_files.join(",")),
                ("warningCount", warnings.len().to_string()),
            ],
        )?;

        Ok(PatchApplyResult {
            patch_id: patch.id.clone(),
            applied_files,
            warnings,
        })
    }

    /// Restores the workspace to its pre-apply state using the redacted
    /// snapshot captured in `apply_patch`. Refuses per-file if the target has
    /// changed since the patch was applied (mirroring the conflict check in
    /// `apply_patch`), so an unrelated later edit is never clobbered.
    pub fn rollback_patch(
        &self,
        root_path: impl AsRef<Path>,
        patch: &ProposedPatch,
        selected_paths: Option<&[String]>,
        approved_by: &str,
    ) -> Result<PatchRollbackResult> {
        let rollback_dir = self.config.data_dir.join("rollback").join(&patch.id);
        if !rollback_dir.exists() {
            return Err(ClientError::InvalidInput(format!(
                "No rollback snapshot found for patch {}",
                patch.id
            )));
        }
        if let Some(paths) = selected_paths {
            let patch_paths = patch
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>();
            if let Some(path) = paths.iter().find(|path| !patch_paths.contains(&path.as_str())) {
                return Err(ClientError::InvalidInput(format!(
                    "Selected patch file was not found: {path}"
                )));
            }
        }

        self.audit_log.record(
            "patch_rollback_started",
            &[
                ("actor", "user".to_string()),
                ("taskId", patch.task_id.clone().unwrap_or_default()),
                ("patchId", patch.id.clone()),
                ("approvedBy", approved_by.to_string()),
            ],
        )?;

        let mut restored_files = Vec::new();
        let mut deleted_files = Vec::new();
        let mut warnings = Vec::new();

        let files_to_roll_back = patch
            .files
            .iter()
            .filter(|file| selected_paths.is_none_or(|paths| paths.contains(&file.path)));

        for file in files_to_roll_back {
            let rollback_path = rollback_dir.join(file.path.replace('/', "__"));
            if !rollback_path.exists() {
                warnings.push(format!(
                    "{}: no rollback snapshot available, skipped",
                    file.path
                ));
                continue;
            }
            let raw_snapshot = fs::read_to_string(&rollback_path)?;
            let snapshot: RollbackSnapshot = serde_json::from_str(&raw_snapshot).map_err(|error| {
                ClientError::Io(format!(
                    "Corrupt rollback snapshot for {}: {error}",
                    file.path
                ))
            })?;

            let target = self.path_policy.resolve_for_write(&root_path, &file.path)?;
            self.path_policy
                .assert_not_restricted(&target.relative_path, false)?;
            let current_content = read_existing(&target.absolute_path)?;

            let conflict = if file.status == "deleted" {
                // apply_patch removed this file; its reappearance means something
                // else recreated it after the patch was applied.
                current_content.is_some()
            } else {
                // Compare against what was actually written (`applied_hash`), not
                // `file.new_hash`, since a partial hunk accept writes content that
                // differs from the patch's full proposed `new_content`.
                current_content.as_ref().map(sha256).as_deref() != Some(snapshot.applied_hash.as_str())
            };
            if conflict {
                return Err(ClientError::PatchConflict(format!(
                    "Target file changed since the patch was applied, refusing rollback: {}",
                    file.path
                )));
            }

            if snapshot.existed {
                if let Some(parent) = target.absolute_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let temp_path = temp_path_for(&target.absolute_path);
                fs::write(&temp_path, &snapshot.content)?;
                fs::rename(&temp_path, &target.absolute_path)?;
                restored_files.push(file.path.clone());
            } else if target.absolute_path.exists() {
                fs::remove_file(&target.absolute_path)?;
                deleted_files.push(file.path.clone());
            } else {
                deleted_files.push(file.path.clone());
            }

            if snapshot.content.contains("[REDACTED_") {
                warnings.push(format!(
                    "{}: original content contained secrets that were redacted before rollback capture and cannot be restored",
                    file.path
                ));
            }

            self.audit_log.record(
                "file_restored",
                &[
                    ("actor", "user".to_string()),
                    ("taskId", patch.task_id.clone().unwrap_or_default()),
                    ("patchId", patch.id.clone()),
                    ("approvedBy", approved_by.to_string()),
                    ("resourcePath", file.path.clone()),
                    (
                        "status",
                        if snapshot.existed { "restored" } else { "deleted" }.to_string(),
                    ),
                ],
            )?;
        }

        self.audit_log.record(
            "patch_rolled_back",
            &[
                ("actor", "user".to_string()),
                ("taskId", patch.task_id.clone().unwrap_or_default()),
                ("patchId", patch.id.clone()),
                ("approvedBy", approved_by.to_string()),
                ("restoredFiles", restored_files.join(",")),
                ("deletedFiles", deleted_files.join(",")),
                ("warningCount", warnings.len().to_string()),
            ],
        )?;

        Ok(PatchRollbackResult {
            patch_id: patch.id.clone(),
            restored_files,
            deleted_files,
            warnings,
        })
    }
}

fn read_existing(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ClientError::from(error)),
    }
}

fn temp_path_for(path: &Path) -> PathBuf {
    let mut temp_path = path.to_path_buf();
    let extension = format!(
        "damaian-{}-{}.tmp",
        std::process::id(),
        crate::hash::now_millis()
    );
    temp_path.set_extension(extension);
    temp_path
}
