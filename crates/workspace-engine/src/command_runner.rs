use crate::audit::AuditLog;
use crate::command_policy::{CommandPolicy, CommandRisk};
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis};
use crate::secret_scanner::SecretScanner;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandExecution {
    pub id: String,
    pub command: String,
    pub working_directory: String,
    pub risk: CommandRisk,
    pub approved_by: Option<String>,
    pub started_at_ms: u128,
    pub completed_at_ms: u128,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct CommandRunner {
    config: Config,
    command_policy: CommandPolicy,
    audit_log: AuditLog,
    scanner: SecretScanner,
}

impl CommandRunner {
    pub fn new(
        config: Config,
        command_policy: CommandPolicy,
        audit_log: AuditLog,
        scanner: SecretScanner,
    ) -> Self {
        Self {
            config,
            command_policy,
            audit_log,
            scanner,
        }
    }

    pub fn run(
        &self,
        command: &str,
        cwd: impl AsRef<Path>,
        reason: &str,
        approved: bool,
        approved_by: Option<&str>,
        task_id: Option<&str>,
    ) -> Result<CommandExecution> {
        let classification = self.command_policy.classify(command, cwd.as_ref());
        self.audit_log.record(
            "command_proposed",
            &[
                ("actor", "assistant".to_string()),
                ("taskId", task_id.unwrap_or_default().to_string()),
                ("command", classification.command.clone()),
                (
                    "workingDirectory",
                    cwd.as_ref().to_string_lossy().to_string(),
                ),
                ("risk", classification.risk.as_str().to_string()),
                ("reason", reason.to_string()),
                (
                    "requiresApproval",
                    classification.requires_approval.to_string(),
                ),
                ("blocked", classification.blocked.to_string()),
            ],
        )?;

        if classification.blocked {
            return Err(ClientError::PolicyBlocked(
                "Command is blocked by policy".to_string(),
            ));
        }
        if classification.requires_approval && !approved {
            return Err(ClientError::ApprovalRequired(
                "Command requires user approval before execution".to_string(),
            ));
        }

        let started_at_ms = now_millis();
        let output = Command::new(&self.config.shell)
            .arg("-lc")
            .arg(command)
            .current_dir(cwd.as_ref())
            .output()?;
        let completed_at_ms = now_millis();
        let stdout = truncate_output(
            String::from_utf8_lossy(&output.stdout).as_ref(),
            self.config.max_command_output_bytes,
        );
        let stderr = truncate_output(
            String::from_utf8_lossy(&output.stderr).as_ref(),
            self.config.max_command_output_bytes,
        );
        let execution = CommandExecution {
            id: create_id("cmd"),
            command: command.to_string(),
            working_directory: cwd.as_ref().to_string_lossy().to_string(),
            risk: classification.risk,
            approved_by: classification
                .requires_approval
                .then(|| approved_by.unwrap_or("local_user").to_string()),
            started_at_ms,
            completed_at_ms,
            exit_code: output.status.code(),
            stdout: self.scanner.redact(&stdout).text,
            stderr: self.scanner.redact(&stderr).text,
        };

        self.audit_log.record(
            "command_executed",
            &[
                ("actor", "command".to_string()),
                ("taskId", task_id.unwrap_or_default().to_string()),
                ("command", execution.command.clone()),
                ("workingDirectory", execution.working_directory.clone()),
                ("risk", execution.risk.as_str().to_string()),
                (
                    "approvedBy",
                    execution.approved_by.clone().unwrap_or_default(),
                ),
                ("exitCode", execution.exit_code.unwrap_or(-1).to_string()),
                (
                    "stdoutSummary",
                    execution.stdout.chars().take(2000).collect(),
                ),
                (
                    "stderrSummary",
                    execution.stderr.chars().take(2000).collect(),
                ),
            ],
        )?;

        Ok(execution)
    }
}

fn truncate_output(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut start = value.len() - max_bytes;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_string()
}
