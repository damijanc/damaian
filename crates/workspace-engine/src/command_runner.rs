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
        let redacted_stdout = self.scanner.redact(&stdout).text;
        let redacted_stderr =
            append_docker_diagnostic(&classification.command, &self.scanner.redact(&stderr).text);
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
            stdout: redacted_stdout,
            stderr: redacted_stderr,
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

fn append_docker_diagnostic(command: &str, stderr: &str) -> String {
    let Some(diagnostic) = docker_diagnostic(command, stderr) else {
        return stderr.to_string();
    };

    if stderr.trim().is_empty() {
        return format!("{diagnostic}\n");
    }

    let mut output = stderr.to_string();
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output.push('\n');
    output.push_str(diagnostic);
    output.push('\n');
    output
}

fn docker_diagnostic(command: &str, stderr: &str) -> Option<&'static str> {
    if !is_docker_invocation(command) {
        return None;
    }

    let lower = stderr.to_ascii_lowercase();
    if lower.contains("docker: command not found")
        || lower.contains("docker: not found")
        || lower.contains("command not found: docker")
        || lower.contains("docker-compose: command not found")
        || lower.contains("docker-compose: not found")
        || lower.contains("command not found: docker-compose")
    {
        return Some(
            "Docker diagnostic: the Docker CLI was not found in Damaian's command environment. On macOS, a GUI-launched app may not inherit the same PATH as Terminal. Use an absolute Docker executable path or launch Damaian from a shell for development.",
        );
    }

    if lower.contains("cannot connect to the docker daemon")
        || lower.contains("is the docker daemon running")
        || lower.contains("docker daemon is not running")
        || lower.contains("docker desktop")
            && (lower.contains("not running")
                || lower.contains("not reachable")
                || lower.contains("socket"))
    {
        return Some(
            "Docker diagnostic: Damaian could not reach the Docker daemon. Docker Desktop or the Docker daemon may not be running or may not be reachable from this app environment.",
        );
    }

    if lower.contains("permission denied") && (lower.contains("docker") || lower.contains("sock")) {
        return Some(
            "Docker diagnostic: the current user or app environment does not have permission to access the Docker daemon socket.",
        );
    }

    if is_docker_compose_invocation(command)
        && (lower.contains("docker: 'compose' is not a docker command")
            || lower.contains("docker compose is not a docker command")
            || lower.contains("unknown shorthand flag")
            || lower.contains("no such command: compose"))
    {
        return Some(
            "Docker diagnostic: Docker Compose support was not available through `docker compose`. If this machine uses the legacy Compose binary, try `docker-compose`, or install or enable the Docker Compose plugin.",
        );
    }

    None
}

fn is_docker_invocation(command: &str) -> bool {
    let trimmed = command.trim();
    trimmed == "docker"
        || trimmed.starts_with("docker ")
        || trimmed == "docker-compose"
        || trimmed.starts_with("docker-compose ")
}

fn is_docker_compose_invocation(command: &str) -> bool {
    let trimmed = command.trim();
    trimmed == "docker-compose"
        || trimmed.starts_with("docker-compose ")
        || trimmed == "docker compose"
        || trimmed.starts_with("docker compose ")
}

#[cfg(test)]
mod tests {
    use super::append_docker_diagnostic;

    #[test]
    fn appends_diagnostic_when_docker_cli_is_missing() {
        let stderr = append_docker_diagnostic("docker ps", "zsh: command not found: docker\n");

        assert!(stderr.contains("Docker diagnostic"));
        assert!(stderr.contains("PATH"));
        assert!(stderr.contains("zsh: command not found: docker"));
    }

    #[test]
    fn appends_diagnostic_when_docker_daemon_is_unreachable() {
        let stderr = append_docker_diagnostic(
            "docker ps",
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
        );

        assert!(stderr.contains("Docker diagnostic"));
        assert!(stderr.contains("Docker Desktop or the Docker daemon"));
    }

    #[test]
    fn leaves_non_docker_stderr_unchanged() {
        let stderr = append_docker_diagnostic("git status", "command not found: git");

        assert_eq!(stderr, "command not found: git");
    }
}
