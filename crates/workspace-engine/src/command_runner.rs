use crate::audit::AuditLog;
use crate::cancel::CancelToken;
use crate::command_policy::{CommandPolicy, CommandRisk};
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis};
use crate::process_registry::{ProcessKind, ProcessRegistry, RegistrationHandle};
use crate::secret_scanner::SecretScanner;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

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
    /// Why the child stopped being waited on. Not derivable from `exit_code`:
    /// a timed-out or cancelled command and one that died from a signal both
    /// report `None`, and a timeout must be distinguishable from an exit for
    /// the report and the repair the user is offered.
    pub termination: CommandTermination,
    pub stdout: String,
    pub stderr: String,
}

/// Why a running child stopped being waited on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandTermination {
    /// The process exited on its own; `exit_code` says how.
    Exited,
    /// The configured deadline passed and the process group was killed.
    TimedOut,
    /// The user stopped the turn and the process group was killed.
    Cancelled,
}

impl CommandTermination {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Exited => "exited",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

/// What the poll loop decided about a child that is still running.
///
/// `None` means keep waiting. A free function over plain values so the
/// decision — the part that must not be a guess — is covered by the default
/// suite; `AGENTS.md` requires tests that spawn a shell to be `#[ignore]`d.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitDecision {
    TimedOut,
    Cancelled,
}

fn classify_wait(cancelled: bool, deadline: Instant, now: Instant) -> Option<WaitDecision> {
    // Cancel first: a stop is more specific than a deadline, and naming a stop
    // a timeout would tell the user Damaian gave up when they told it to.
    if cancelled {
        return Some(WaitDecision::Cancelled);
    }
    if now >= deadline {
        return Some(WaitDecision::TimedOut);
    }
    None
}

/// The per-run side channel. Grouped rather than passed as four more
/// parameters because `run` already took six, and because approval, identity,
/// the stop and the live output all belong to one "how this run was invoked"
/// question.
pub struct CommandRunOptions<'a> {
    pub approved: bool,
    pub approved_by: Option<&'a str>,
    pub task_id: Option<&'a str>,
    pub cancel: &'a CancelToken,
    pub on_output: &'a mut dyn FnMut(&str),
}

/// How long the poll loop sleeps between `try_wait` calls. Short enough that a
/// stop is felt promptly, long enough not to spin a core.
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

struct OutputChunk {
    stream: OutputStream,
    bytes: Vec<u8>,
}

fn spawn_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    stream: OutputStream,
    sender: mpsc::Sender<OutputChunk>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(reader);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match std::io::BufRead::read_until(&mut reader, b'\n', &mut buffer) {
                Ok(0) => break,
                Ok(_) => {
                    if sender
                        .send(OutputChunk {
                            stream,
                            bytes: buffer.clone(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn drain_output(
    receiver: &mpsc::Receiver<OutputChunk>,
    scanner: &SecretScanner,
    on_output: &mut dyn FnMut(&str),
    raw_stdout: &mut String,
    raw_stderr: &mut String,
) {
    while let Ok(chunk) = receiver.try_recv() {
        let text = String::from_utf8_lossy(&chunk.bytes);
        match chunk.stream {
            OutputStream::Stdout => raw_stdout.push_str(&text),
            OutputStream::Stderr => raw_stderr.push_str(&text),
        }
        // Redact line by line for the live stream; the final whole-output
        // redaction below is still authoritative, so a secret split across a
        // chunk boundary cannot survive in the persisted copy. The alternative
        // — streaming the raw text — would make the live view the one path
        // around the scanner.
        on_output(&scanner.redact(&text).text);
    }
}

fn kill_process_group(pid: u32) {
    // SAFETY: `kill` takes two integers by value and touches no memory. The
    // group id is the child's own pid, because it was spawned with
    // `process_group(0)`.
    unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
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
        options: CommandRunOptions<'_>,
    ) -> Result<CommandExecution> {
        let classification = self.command_policy.classify(command, cwd.as_ref());
        self.audit_log.record(
            "command_proposed",
            &[
                ("actor", "assistant".to_string()),
                ("taskId", options.task_id.unwrap_or_default().to_string()),
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
        if classification.requires_approval && !options.approved {
            return Err(ClientError::ApprovalRequired(
                "Command requires user approval before execution".to_string(),
            ));
        }

        let started_at_ms = now_millis();
        // `spawn` rather than `output` so the child's pid is knowable. `output`
        // hides it, which is the only reason spec 17 concluded a command could
        // not be cleaned up — it orphans under `SIGKILL` like anything else.
        // Its own group so a pipeline's members are reachable too.
        let mut child = Command::new(&self.config.shell)
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
                options.task_id.unwrap_or_default(),
                child.id(),
            )?,
        };

        // Drain both pipes on their own threads: a child that fills a pipe
        // buffer and blocks waiting for the reader is a hung command, and
        // `wait_with_output` was the only thing draining them before.
        let stdout_pipe = child.stdout.take().expect("stdout was piped");
        let stderr_pipe = child.stderr.take().expect("stderr was piped");
        let (sender, receiver) = mpsc::channel::<OutputChunk>();
        let stdout_reader = spawn_reader(stdout_pipe, OutputStream::Stdout, sender.clone());
        let stderr_reader = spawn_reader(stderr_pipe, OutputStream::Stderr, sender.clone());
        drop(sender);

        let deadline =
            Instant::now() + Duration::from_secs(self.config.command_timeout_secs as u64);
        let mut termination = CommandTermination::Exited;
        // Assigned on both break paths, and the loop has no other exit, so it
        // is definitely initialised by the time it is read.
        let status: Option<ExitStatus>;
        let mut raw_stdout = String::new();
        let mut raw_stderr = String::new();
        loop {
            drain_output(
                &receiver,
                &self.scanner,
                options.on_output,
                &mut raw_stdout,
                &mut raw_stderr,
            );
            if let Some(exit) = child.try_wait()? {
                status = Some(exit);
                break;
            }
            if let Some(decision) =
                classify_wait(options.cancel.is_cancelled(), deadline, Instant::now())
            {
                termination = match decision {
                    WaitDecision::TimedOut => CommandTermination::TimedOut,
                    WaitDecision::Cancelled => CommandTermination::Cancelled,
                };
                kill_process_group(child.id());
                // Reap so the guard does not kill a pid that could have been
                // reused, and so the readers see EOF and return.
                status = child.wait().ok();
                break;
            }
            std::thread::sleep(COMMAND_POLL_INTERVAL);
        }

        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        drain_output(
            &receiver,
            &self.scanner,
            options.on_output,
            &mut raw_stdout,
            &mut raw_stderr,
        );

        // Reaped, so the pid is free and its group may already be someone
        // else's. Disarm the kill — but let the guard drop normally, because
        // its handle is what removes the registry entry.
        guard.armed = false;
        drop(guard);
        let completed_at_ms = now_millis();

        let stdout = truncate_output(&raw_stdout, self.config.max_command_output_bytes);
        let stderr = truncate_output(&raw_stderr, self.config.max_command_output_bytes);
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
                .then(|| options.approved_by.unwrap_or("local_user").to_string()),
            started_at_ms,
            completed_at_ms,
            exit_code: status.and_then(|status| status.code()),
            termination,
            stdout: redacted_stdout,
            stderr: redacted_stderr,
        };

        self.audit_log.record(
            "command_executed",
            &[
                ("actor", "command".to_string()),
                ("taskId", options.task_id.unwrap_or_default().to_string()),
                ("command", execution.command.clone()),
                ("workingDirectory", execution.working_directory.clone()),
                ("risk", execution.risk.as_str().to_string()),
                (
                    "approvedBy",
                    execution.approved_by.clone().unwrap_or_default(),
                ),
                ("exitCode", execution.exit_code.unwrap_or(-1).to_string()),
                ("termination", execution.termination.as_str().to_string()),
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
        // The child is known not to have been reaped on this path, so the pid
        // cannot have been reused.
        kill_process_group(self.pid);
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
    use super::{
        CommandRunOptions, CommandRunner, WaitDecision, append_docker_diagnostic, classify_wait,
    };
    use crate::audit::AuditLog;
    use crate::cancel::CancelToken;
    use crate::command_policy::CommandPolicy;
    use crate::config::Config;
    use crate::process_registry::ProcessRegistry;
    use crate::secret_scanner::SecretScanner;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// Approval as a shell-invoking test wants it: granted, by the local user,
    /// with a token that is never cancelled and output that goes nowhere.
    fn granted_options<'a>(
        cancel: &'a CancelToken,
        on_output: &'a mut dyn FnMut(&str),
    ) -> CommandRunOptions<'a> {
        CommandRunOptions {
            approved: true,
            approved_by: Some("local_user"),
            task_id: None,
            cancel,
            on_output,
        }
    }

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

        let cancel = CancelToken::new();
        let mut on_output = |_line: &str| {};
        let execution = runner
            .run(
                "echo hello",
                &data_dir,
                "test",
                granted_options(&cancel, &mut on_output),
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
        let cancel = CancelToken::new();
        let mut on_output = |_line: &str| {};
        let execution = runner
            .run(
                "ps -o pgid= -p $$",
                &data_dir,
                "test",
                granted_options(&cancel, &mut on_output),
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

    /// The wait/terminate decision is the part a shell-spawning test cannot
    /// cheaply cover, so it is a pure function and tested here on the default
    /// suite. A command that ran past its deadline must be classified as timed
    /// out rather than exited: `exit_code` is `None` either way, and treating a
    /// kill as a clean exit is how a hung command would read as a finished one.
    #[test]
    fn a_command_past_its_deadline_is_timed_out() {
        let now = Instant::now();
        let deadline = now - Duration::from_secs(1);

        assert_eq!(
            classify_wait(false, deadline, now),
            Some(WaitDecision::TimedOut)
        );
    }

    /// Cancellation is checked before the deadline, so a user's stop is named
    /// as a stop even when the clock had also run out.
    #[test]
    fn a_cancel_outranks_a_deadline_that_also_passed() {
        let now = Instant::now();
        let deadline = now - Duration::from_secs(1);

        assert_eq!(
            classify_wait(true, deadline, now),
            Some(WaitDecision::Cancelled)
        );
    }

    #[test]
    fn a_cancelled_command_with_time_left_is_cancelled() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(600);

        assert_eq!(
            classify_wait(true, deadline, now),
            Some(WaitDecision::Cancelled)
        );
    }

    #[test]
    fn a_running_command_inside_its_deadline_keeps_waiting() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(600);

        assert_eq!(classify_wait(false, deadline, now), None);
    }
}
