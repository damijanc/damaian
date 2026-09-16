use crate::audit::AuditLog;
use crate::command_policy::{CommandPolicy, CommandRisk};
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis};
use crate::process_registry::{ProcessKind, ProcessRegistry, RegistrationHandle};
use crate::secret_scanner::SecretScanner;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

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
        // `spawn` rather than `output` so the child's pid is knowable. `output`
        // hides it, which is the only reason spec 17 concluded a command could
        // not be cleaned up — it orphans under `SIGKILL` like anything else.
        // Its own group so a pipeline's members are reachable too.
        let child = Command::new(&self.config.shell)
            .arg("-lc")
            .arg(command)
            .current_dir(cwd.as_ref())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let mut guard = CommandGuard {
            pid: child.id(),
            armed: true,
            _registration: ProcessRegistry::open(&self.config.data_dir)?.register(
                ProcessKind::Command,
                task_id.unwrap_or_default(),
                child.id(),
            )?,
        };
        let output = child.wait_with_output()?;
        // Reaped, so the pid is free and its group may already be someone
        // else's. Disarm the kill — but let the guard drop normally, because
        // its handle is what removes the registry entry.
        guard.armed = false;
        drop(guard);
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

/// Kills the command's process group if this frame unwinds — a panic between
/// the spawn and the reap would otherwise leave the command running with
/// nothing waiting on it. A `SIGKILL` skips this, which is what the registry
/// entry is for.
///
/// `armed` is cleared once the child is reaped. After that its pid is free and
/// its group may belong to someone else, so killing would be exactly the
/// widening `docs/specs/46_process_registry_and_orphan_sweep/proposal.md` §5.5
/// forbids. The handle drops either way, so the registry entry goes on both
/// paths.
struct CommandGuard {
    pid: u32,
    armed: bool,
    _registration: RegistrationHandle,
}

impl Drop for CommandGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // SAFETY: `kill` takes two integers by value and touches no memory.
        // The group id is this child's own pid, because it was spawned with
        // `process_group(0)`, and the child is known not to have been reaped.
        unsafe { libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL) };
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
    use super::{CommandRunner, append_docker_diagnostic};
    use crate::audit::AuditLog;
    use crate::command_policy::CommandPolicy;
    use crate::config::Config;
    use crate::process_registry::ProcessRegistry;
    use crate::secret_scanner::SecretScanner;
    use std::path::PathBuf;

    /// A runner whose data directory is a scratch path, so a test never writes
    /// to the user's real `~/Library/Application Support/DamaianClient`.
    fn runner_with_scratch_data_dir() -> (CommandRunner, PathBuf) {
        let data_dir = std::env::temp_dir().join(format!(
            "damaian-cmd-registry-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(&data_dir).expect("scratch data dir");
        let config = Config {
            data_dir: data_dir.clone(),
            ..Config::default()
        };
        let scanner = SecretScanner::new(config.secret_patterns.clone());
        let policy = CommandPolicy::new(config.clone());
        let audit = AuditLog::new(&data_dir, false, scanner.clone());
        (CommandRunner::new(config, policy, audit, scanner), data_dir)
    }

    /// A command that runs to completion must leave nothing behind, and the
    /// child must be in its own group so the sweep can reach a pipeline.
    ///
    /// `#[ignore]`d per `AGENTS.md` because it runs a real shell command. Run
    /// it by hand:
    ///
    /// ```sh
    /// cargo test -p workspace-engine --lib -- --ignored --exact \
    ///   command_runner::tests::a_completed_command_leaves_no_registry_entry
    /// ```
    #[test]
    #[ignore]
    fn a_completed_command_leaves_no_registry_entry() {
        let (runner, data_dir) = runner_with_scratch_data_dir();
        let registry = ProcessRegistry::open(&data_dir).unwrap();

        let execution = runner
            .run(
                "echo hello",
                &data_dir,
                "test",
                true,
                Some("local_user"),
                None,
            )
            .expect("the command should run");

        assert_eq!(execution.exit_code, Some(0));
        assert!(execution.stdout.contains("hello"));
        assert!(
            registry.entries().unwrap().is_empty(),
            "requirement 4: a command that exited leaves no entry"
        );
    }

    /// The child must be its own group leader, or the sweep's `kill(-pgid)`
    /// would either miss a pipeline's members or reach outside the command.
    ///
    /// `#[ignore]`d per `AGENTS.md` because it runs a real shell command. Run
    /// it by hand:
    ///
    /// ```sh
    /// cargo test -p workspace-engine --lib -- --ignored --exact \
    ///   command_runner::tests::a_command_runs_as_its_own_process_group_leader
    /// ```
    #[test]
    #[ignore]
    fn a_command_runs_as_its_own_process_group_leader() {
        let (runner, data_dir) = runner_with_scratch_data_dir();

        // `$$` is the shell's own pid; `ps` reports the group it belongs to.
        let execution = runner
            .run(
                "ps -o pgid= -p $$",
                &data_dir,
                "test",
                true,
                Some("local_user"),
                None,
            )
            .expect("the command should run");
        let pgid: u32 = execution
            .stdout
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("expected a pgid, got {:?}", execution.stdout));

        assert_ne!(
            pgid,
            std::process::id(),
            "the command must not share this process's group, or a sweep \
             would signal Damaian itself"
        );
    }

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
