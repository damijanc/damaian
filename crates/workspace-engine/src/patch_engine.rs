use crate::audit::AuditLog;
use crate::config::Config;
use crate::diff::create_unified_diff;
use crate::error::{ClientError, Result};
use crate::hash::{create_id, file_hash, now_millis, sha256};
use crate::path_policy::PathPolicy;
use crate::secret_scanner::SecretScanner;
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
            let diff = create_unified_diff(
                old_content.as_deref().unwrap_or_default(),
                &change.new_content,
                &target.relative_path,
            );
            let diff = self.scanner.redact(&diff).text;
            files.push(ProposedFilePatch {
                path: target.relative_path.clone(),
                status,
                base_hash: old_content.as_ref().map(sha256),
                new_hash: sha256(change.new_content.as_bytes()),
                diff,
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

    pub fn apply_patch(
        &self,
        root_path: impl AsRef<Path>,
        patch: &ProposedPatch,
        approved_paths: Option<&[String]>,
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

            let findings = self.scanner.scan(&file.new_content);
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
            prepared.push((file, target.absolute_path, current_content));
        }

        let rollback_dir = self.config.data_dir.join("rollback").join(&patch.id);
        fs::create_dir_all(&rollback_dir)?;
        let mut applied_files = Vec::new();

        for (file, absolute_path, current_content) in prepared {
            let rollback_path = rollback_dir.join(file.path.replace('/', "__"));
            fs::write(
                &rollback_path,
                format!(
                    "{{\"path\":\"{}\",\"baseHash\":\"{}\",\"capturedAtMs\":\"{}\",\"content\":\"{}\"}}",
                    file.path,
                    file.base_hash.clone().unwrap_or_default(),
                    now_millis(),
                    current_content
                        .clone()
                        .unwrap_or_default()
                        .replace('\\', "\\\\")
                        .replace('"', "\\\"")
                        .replace('\n', "\\n")
                ),
            )?;

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
                fs::write(&temp_path, &file.new_content)?;
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
