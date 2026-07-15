use crate::audit::AuditLog;
use crate::error::{ClientError, Result};
use crate::secret_scanner::SecretScanner;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFileStatus {
    pub path: String,
    pub raw: String,
    pub staged: bool,
    pub worktree: bool,
    pub untracked: bool,
    pub conflicted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub clean: bool,
    pub exit_code: i32,
    pub raw: String,
    pub stderr: String,
    pub files: Vec<GitFileStatus>,
}

#[derive(Debug, Clone)]
pub struct GitService {
    audit_log: AuditLog,
    scanner: SecretScanner,
}

impl GitService {
    pub fn new(audit_log: AuditLog, scanner: SecretScanner) -> Self {
        Self { audit_log, scanner }
    }

    pub fn status(&self, root_path: impl AsRef<Path>) -> Result<GitStatus> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root_path.as_ref())
            .arg("status")
            .arg("--porcelain=v1")
            .output()?;
        let exit_code = output.status.code().unwrap_or(-1);
        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let status = GitStatus {
            clean: exit_code == 0 && raw.trim().is_empty(),
            exit_code,
            files: if exit_code == 0 {
                parse_porcelain(&raw)
            } else {
                Vec::new()
            },
            raw,
            stderr,
        };
        self.audit_log.record(
            "git_status_read",
            &[
                ("actor", "system".to_string()),
                (
                    "resourcePath",
                    root_path.as_ref().to_string_lossy().to_string(),
                ),
                (
                    "status",
                    if exit_code == 0 { "complete" } else { "failed" }.to_string(),
                ),
                ("exitCode", exit_code.to_string()),
                ("fileCount", status.files.len().to_string()),
            ],
        )?;
        Ok(status)
    }

    pub fn diff(&self, root_path: impl AsRef<Path>, staged: bool) -> Result<String> {
        let mut command = Command::new("git");
        command.arg("-C").arg(root_path.as_ref()).arg("diff");
        if staged {
            command.arg("--staged");
        }
        let output = command.output()?;
        let exit_code = output.status.code().unwrap_or(-1);
        self.audit_log.record(
            "git_diff_read",
            &[
                ("actor", "system".to_string()),
                (
                    "resourcePath",
                    root_path.as_ref().to_string_lossy().to_string(),
                ),
                (
                    "status",
                    if exit_code == 0 { "complete" } else { "failed" }.to_string(),
                ),
                ("exitCode", exit_code.to_string()),
                ("staged", staged.to_string()),
            ],
        )?;
        if exit_code != 0 {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let raw_diff = String::from_utf8_lossy(&output.stdout);
        Ok(self.scanner.redact(raw_diff.as_ref()).text)
    }

    pub fn suggest_commit_message(&self, summary: &str, changed_files: &[String]) -> String {
        let normalized = summary.trim().trim_end_matches(['.', '!', '?']);
        if normalized.is_empty() {
            "chore: update workspace changes".to_string()
        } else if changed_files.len() == 1 {
            format!("chore: {} {normalized}", changed_files[0])
        } else {
            format!("chore: {normalized}")
        }
    }
}

fn parse_porcelain(raw: &str) -> Vec<GitFileStatus> {
    raw.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let index_status = line.as_bytes().first().copied().unwrap_or(b' ');
            let worktree_status = line.as_bytes().get(1).copied().unwrap_or(b' ');
            let path = line.get(3..).unwrap_or_default();
            let path = path.split(" -> ").last().unwrap_or(path).to_string();
            let pair = format!("{}{}", index_status as char, worktree_status as char);
            GitFileStatus {
                path,
                raw: line.to_string(),
                staged: index_status != b' ' && index_status != b'?',
                worktree: worktree_status != b' ',
                untracked: index_status == b'?' && worktree_status == b'?',
                conflicted: ["AA", "DD", "AU", "UD", "UA", "DU", "UU"].contains(&pair.as_str()),
            }
        })
        .collect()
}
