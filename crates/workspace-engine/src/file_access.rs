use crate::audit::AuditLog;
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::hash::file_hash;
use crate::path_policy::PathPolicy;
use crate::secret_scanner::SecretScanner;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: String,
    pub absolute_path: String,
    pub hash: String,
    pub content: String,
    pub redaction_status: String,
    pub finding_count: usize,
}

#[derive(Debug, Clone)]
pub struct FileAccessController {
    config: Config,
    audit_log: AuditLog,
    scanner: SecretScanner,
    path_policy: PathPolicy,
}

impl FileAccessController {
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

    pub fn read_file(
        &self,
        root_path: impl AsRef<Path>,
        requested_path: impl AsRef<Path>,
        task_id: Option<&str>,
        repository_id: Option<&str>,
        allow_restricted: bool,
    ) -> Result<FileRead> {
        let target = self
            .path_policy
            .resolve_existing(root_path, requested_path)?;
        self.path_policy
            .assert_not_restricted(&target.relative_path, allow_restricted)?;
        let metadata = fs::metadata(&target.absolute_path)?;
        if !metadata.is_file() {
            return Err(ClientError::AccessDenied(
                "Path is not a regular file".to_string(),
            ));
        }
        if metadata.len() > self.config.max_file_bytes {
            return Err(ClientError::AccessDenied(
                "File exceeds configured size limit".to_string(),
            ));
        }

        let bytes = fs::read(&target.absolute_path)?;
        if bytes.iter().take(8000).any(|byte| *byte == 0) {
            return Err(ClientError::AccessDenied(
                "Binary file reads are denied by default".to_string(),
            ));
        }
        let raw_content = String::from_utf8_lossy(&bytes).to_string();
        let redaction = self.scanner.redact(&raw_content);
        let result = FileRead {
            path: target.relative_path.clone(),
            absolute_path: target.absolute_path.to_string_lossy().to_string(),
            hash: file_hash(&target.absolute_path)?,
            content: redaction.text,
            redaction_status: if redaction.findings.is_empty() {
                "clean".to_string()
            } else {
                "redacted".to_string()
            },
            finding_count: redaction.findings.len(),
        };

        self.audit_log.record(
            "file_read",
            &[
                ("actor", "assistant".to_string()),
                (
                    "repositoryId",
                    repository_id.unwrap_or_default().to_string(),
                ),
                ("taskId", task_id.unwrap_or_default().to_string()),
                ("resourcePath", result.path.clone()),
                ("status", "allowed".to_string()),
                ("redactionStatus", result.redaction_status.clone()),
                ("findingCount", result.finding_count.to_string()),
            ],
        )?;

        Ok(result)
    }
}
