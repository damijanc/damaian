//! Hooks (spec 32): external programs that see a lifecycle event and answer
//! with a verdict.
//!
//! The contract (`context.md` §5, §6, §12): what a hook is sent, what it may
//! answer, how an answer is checked, the order hooks run in and how their
//! verdicts combine. That part is pure.
//!
//! The runner (`context.md` §2, §8): `run_hook` starts one hook with a
//! cleared environment, hands it the envelope, and kills it at its deadline
//! or at the output ceiling. It does not read the answer; that is
//! `parse_response`'s job.

use crate::command_runner::kill_process_group;
use crate::finding::Severity;
use crate::process_registry::{ProcessKind, ProcessRegistry, RegistrationHandle};
use crate::secret_scanner::SecretScanner;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};
use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use crate::config::{HookConfig, HookEvent, HookMode, HookSource, MAX_HOOK_TIMEOUT_MS};

/// The contract version this build speaks. A hook must name it, and a hook
/// naming another is a failure, not assumed benign (proposal §5.5).
pub const HOOK_API_VERSION: u32 = 1;
/// Larger stdout is a failure, not a truncation (proposal §5.9).
pub const MAX_HOOK_STDOUT_BYTES: usize = 64 * 1024;
pub const MAX_HOOK_REASON_CHARS: usize = 1_000;
/// Each finding's own text is bounded by `Finding::new`.
pub const MAX_HOOK_FINDINGS: usize = 20;
pub const MAX_HOOK_CONTEXT_ENTRIES: usize = 4;
pub const MAX_HOOK_CONTEXT_CHARS: usize = 4_000;
pub const MAX_HOOK_COMMANDS: usize = 3;
/// stderr is diagnostic, so it is cut to its last 8 KiB rather than failed.
pub const MAX_HOOK_STDERR_BYTES: usize = 8 * 1024;

/// What a hook can say about an action, weakest first, so the derived `Ord`
/// is the resolution order and combining verdicts is `max`.
///
/// There is deliberately no verdict that grants anything, such as an
/// `approve` that would make an approval-gated action automatic: that would
/// be a hook widening `profile ∩ mode`, which proposal §5.2 and requirement
/// 6 forbid. A hook that answers with one is audited as a widening attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verdict {
    Allow,
    Warn,
    RequestApproval,
    Deny,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Warn => "warn",
            Verdict::RequestApproval => "request_approval",
            Verdict::Deny => "deny",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Verdict::Allow),
            "warn" => Some(Verdict::Warn),
            "request_approval" => Some(Verdict::RequestApproval),
            "deny" => Some(Verdict::Deny),
            _ => None,
        }
    }
}

/// `deny` > `request_approval` > `warn` > `allow`, whatever order the hooks
/// ran in (proposal §5.6). No hooks is `allow`.
pub fn resolve(verdicts: impl IntoIterator<Item = Verdict>) -> Verdict {
    verdicts.into_iter().max().unwrap_or(Verdict::Allow)
}

/// Whether the remaining hooks for an event are skipped after this verdict.
/// Only a `deny`: the action is not happening. A `request_approval` does not,
/// because a later hook may still deny and a denial outranks it.
pub fn short_circuits(verdict: Verdict) -> bool {
    verdict == Verdict::Deny
}

/// The hooks that run at `event`, in the order they run: enabled ones only,
/// by `(order, id)`. Scope plays no part, so a repository cannot get its hook
/// in first by where it defined it (`context.md` §12). Whether a repository
/// hook has been reviewed is decided by the dispatcher, not here.
pub fn ordered(hooks: &[HookConfig], event: HookEvent) -> Vec<&HookConfig> {
    let mut active: Vec<&HookConfig> = hooks
        .iter()
        .filter(|hook| hook.enabled && hook.events.contains(&event))
        .collect();
    active.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
    active
}

/// What a hook is sent on stdin (proposal §5.1, `context.md` §6). The
/// payload carries paths, ids and metadata, never file content.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookEnvelope {
    #[serde(serialize_with = "serialize_event")]
    pub event: HookEvent,
    pub hook_api_version: u32,
    pub session_id: Option<String>,
    /// `None` where the event has no task: `session_end`, or a patch applied
    /// outside a session.
    pub task_id: Option<String>,
    pub mode: String,
    pub profile: String,
    pub repository_root: PathBuf,
    /// How deep inside another hook's requested command this dispatch is
    /// (`context.md` §9). 0 for an action of the turn itself.
    pub hook_depth: u32,
    pub payload: Value,
}

fn serialize_event<S: Serializer>(event: &HookEvent, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(event.as_str())
}

/// A hook's answer, checked against the closed schema and its bounds.
/// Nothing in it is redacted yet; the dispatcher does that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookResponse {
    pub verdict: Verdict,
    /// Present whenever the verdict is `deny` or `request_approval`.
    pub reason: Option<String>,
    pub findings: Vec<HookFinding>,
    pub context: Vec<String>,
    /// Command lines the hook asks Damaian to run, which run only if policy
    /// would run them unasked (`context.md` §9).
    pub commands: Vec<String>,
}

impl HookResponse {
    fn allow() -> Self {
        HookResponse {
            verdict: Verdict::Allow,
            reason: None,
            findings: Vec::new(),
            context: Vec::new(),
            commands: Vec::new(),
        }
    }
}

/// One `findings` entry (`context.md` §5). Whether `path` stays inside the
/// repository is checked when it becomes a `Finding`, which knows the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookFinding {
    pub severity: Severity,
    pub summary: String,
    pub details: Option<String>,
    pub path: Option<String>,
    pub line: Option<u32>,
}

/// Something in a response that is not in the contract. Named, never quoted:
/// the value may be anything the hook chose to put there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WideningAttempt {
    /// An unknown field, as `findings.<name>` inside a finding.
    Field(String),
    /// A `verdict` string that is not one of the four.
    Verdict(String),
}

/// How a hook failed, as audited (`context.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookFailureKind {
    SpawnFailed,
    NonZeroExit,
    Timeout,
    Malformed,
    VersionMismatch,
    OutputTooLarge,
    /// A repository hook whose definition no longer matches the reviewed one.
    ChangedSinceReview,
}

impl HookFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HookFailureKind::SpawnFailed => "spawn_failed",
            HookFailureKind::NonZeroExit => "non_zero_exit",
            HookFailureKind::Timeout => "timeout",
            HookFailureKind::Malformed => "malformed",
            HookFailureKind::VersionMismatch => "version_mismatch",
            HookFailureKind::OutputTooLarge => "output_too_large",
            HookFailureKind::ChangedSinceReview => "changed_since_review",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseError {
    /// Not one JSON object of the contract's shape, or over a bound.
    Malformed(String),
    /// The `hookApiVersion` the hook named, which is not this build's.
    VersionMismatch(u64),
    OutputTooLarge,
    /// Unknown fields or verdicts. The dispatcher audits each one, then
    /// handles the response as malformed (`context.md` §5).
    Widening {
        attempts: Vec<WideningAttempt>,
    },
}

impl ResponseError {
    pub fn failure_kind(&self) -> HookFailureKind {
        match self {
            ResponseError::Malformed(_) | ResponseError::Widening { .. } => {
                HookFailureKind::Malformed
            }
            ResponseError::VersionMismatch(_) => HookFailureKind::VersionMismatch,
            ResponseError::OutputTooLarge => HookFailureKind::OutputTooLarge,
        }
    }
}

impl fmt::Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResponseError::Malformed(why) => write!(f, "malformed response: {why}"),
            ResponseError::VersionMismatch(version) => write!(
                f,
                "hookApiVersion {version} is not supported (expected {HOOK_API_VERSION})"
            ),
            ResponseError::OutputTooLarge => {
                write!(f, "output exceeds {MAX_HOOK_STDOUT_BYTES} bytes")
            }
            ResponseError::Widening { attempts } => {
                let names: Vec<String> = attempts
                    .iter()
                    .map(|attempt| match attempt {
                        WideningAttempt::Field(name) => format!("field `{name}`"),
                        WideningAttempt::Verdict(name) => format!("verdict `{name}`"),
                    })
                    .collect();
                write!(f, "not in the hook contract: {}", names.join(", "))
            }
        }
    }
}

const RESPONSE_FIELDS: [&str; 6] = [
    "hookApiVersion",
    "verdict",
    "reason",
    "findings",
    "context",
    "commands",
];
const FINDING_FIELDS: [&str; 5] = ["severity", "summary", "details", "path", "line"];

/// Checks a hook's stdout against the contract (`context.md` §5).
///
/// Empty or whitespace-only output is `allow`. Anything else must be one
/// JSON object with only the contract's fields, `hookApiVersion` 1, and every
/// bound kept; a bound exceeded fails the whole response rather than being
/// cut to fit. `null` counts as absent for an optional field.
///
/// Widening attempts are collected before the version is looked at: the
/// version is the hook's own claim, so naming another one must not hide an
/// attempt from the audit.
pub fn parse_response(stdout: &[u8]) -> Result<HookResponse, ResponseError> {
    if stdout.len() > MAX_HOOK_STDOUT_BYTES {
        return Err(ResponseError::OutputTooLarge);
    }
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(HookResponse::allow());
    }
    let value: Value = serde_json::from_slice(stdout)
        .map_err(|error| ResponseError::Malformed(format!("not JSON: {error}")))?;
    let Value::Object(object) = value else {
        return Err(malformed("the response is not a JSON object"));
    };

    let attempts = widening_attempts(&object);
    if !attempts.is_empty() {
        return Err(ResponseError::Widening { attempts });
    }

    match object.get("hookApiVersion") {
        None | Some(Value::Null) => return Err(malformed("`hookApiVersion` is missing")),
        Some(version) => match version.as_u64() {
            Some(version) if version == u64::from(HOOK_API_VERSION) => {}
            Some(version) => return Err(ResponseError::VersionMismatch(version)),
            None => return Err(malformed("`hookApiVersion` is not a whole number")),
        },
    }

    let verdict = match present(&object, "verdict") {
        None => Verdict::Allow,
        Some(Value::String(verdict)) => {
            // An unknown string was already reported as a widening attempt.
            Verdict::parse(verdict).ok_or_else(|| malformed("`verdict` is not known"))?
        }
        Some(_) => return Err(malformed("`verdict` is not a string")),
    };

    let reason = optional_string(&object, "reason")?;
    if let Some(reason) = &reason
        && reason.chars().count() > MAX_HOOK_REASON_CHARS
    {
        return Err(malformed(&format!(
            "`reason` is longer than {MAX_HOOK_REASON_CHARS} characters"
        )));
    }
    if matches!(verdict, Verdict::Deny | Verdict::RequestApproval)
        && reason
            .as_deref()
            .is_none_or(|reason| reason.trim().is_empty())
    {
        return Err(malformed(&format!(
            "`{}` needs a `reason`",
            verdict.as_str()
        )));
    }

    let findings = array(&object, "findings", MAX_HOOK_FINDINGS)?
        .iter()
        .map(parse_finding)
        .collect::<Result<Vec<_>, _>>()?;

    let context = strings(&object, "context", MAX_HOOK_CONTEXT_ENTRIES)?;
    if context
        .iter()
        .any(|entry| entry.chars().count() > MAX_HOOK_CONTEXT_CHARS)
    {
        return Err(malformed(&format!(
            "a `context` entry is longer than {MAX_HOOK_CONTEXT_CHARS} characters"
        )));
    }

    let commands = strings(&object, "commands", MAX_HOOK_COMMANDS)?;
    if commands.iter().any(|command| command.trim().is_empty()) {
        return Err(malformed("a `commands` entry is empty"));
    }

    Ok(HookResponse {
        verdict,
        reason,
        findings,
        context,
        commands,
    })
}

/// Every unknown field, top level then inside findings, sorted by name,
/// then an unknown verdict. Only names are kept.
fn widening_attempts(object: &Map<String, Value>) -> Vec<WideningAttempt> {
    let mut fields: Vec<String> = object
        .keys()
        .filter(|key| !RESPONSE_FIELDS.contains(&key.as_str()))
        .cloned()
        .collect();
    if let Some(Value::Array(findings)) = object.get("findings") {
        for finding in findings {
            if let Value::Object(finding) = finding {
                for key in finding.keys() {
                    if !FINDING_FIELDS.contains(&key.as_str()) {
                        fields.push(format!("findings.{key}"));
                    }
                }
            }
        }
    }
    fields.sort();
    fields.dedup();
    let mut attempts: Vec<WideningAttempt> =
        fields.into_iter().map(WideningAttempt::Field).collect();
    if let Some(Value::String(verdict)) = object.get("verdict")
        && Verdict::parse(verdict).is_none()
    {
        attempts.push(WideningAttempt::Verdict(verdict.clone()));
    }
    attempts
}

fn malformed(why: &str) -> ResponseError {
    ResponseError::Malformed(why.to_string())
}

fn present<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ResponseError> {
    match present(object, key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(malformed(&format!("`{key}` is not a string"))),
    }
}

fn array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<&'a [Value], ResponseError> {
    match present(object, key) {
        None => Ok(&[]),
        Some(Value::Array(items)) if items.len() > max => {
            Err(malformed(&format!("`{key}` has more than {max} entries")))
        }
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(malformed(&format!("`{key}` is not an array"))),
    }
}

fn strings(
    object: &Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Vec<String>, ResponseError> {
    array(object, key, max)?
        .iter()
        .map(|item| match item {
            Value::String(text) => Ok(text.clone()),
            _ => Err(malformed(&format!("a `{key}` entry is not a string"))),
        })
        .collect()
}

fn parse_finding(value: &Value) -> Result<HookFinding, ResponseError> {
    let Value::Object(finding) = value else {
        return Err(malformed("a `findings` entry is not an object"));
    };
    let severity = match present(finding, "severity") {
        Some(Value::String(severity)) => match severity.as_str() {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            "info" => Severity::Info,
            _ => {
                return Err(malformed(
                    "a finding's `severity` is not error, warning or info",
                ));
            }
        },
        _ => return Err(malformed("a finding has no `severity`")),
    };
    let summary = optional_string(finding, "summary")?
        .ok_or_else(|| malformed("a finding has no `summary`"))?;
    let line = match present(finding, "line") {
        None => None,
        Some(line) => Some(
            line.as_u64()
                .filter(|line| *line >= 1)
                .and_then(|line| u32::try_from(line).ok())
                .ok_or_else(|| malformed("a finding's `line` is not a line number"))?,
        ),
    };
    Ok(HookFinding {
        severity,
        summary,
        details: optional_string(finding, "details")?,
        path: optional_string(finding, "path")?,
        line,
    })
}

/// The variables a hook inherits from Damaian, by name (`context.md` §8).
/// Nothing else is passed on: not the model key's variable, not an MCP
/// server's token, so a hook cannot be a way to read the user's API key.
const HOOK_INHERITED_ENV: [&str; 5] = ["PATH", "HOME", "USER", "LANG", "TMPDIR"];

/// How often the runner checks on a running hook. Shorter than
/// `CommandRunner`'s 50 ms because a `before_tool` hook runs on every tool
/// call, and a hook that finishes just after a poll waits out the interval.
const HOOK_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How much of stderr is held while the hook runs. More than is returned,
/// so the redaction that runs before the cut sees a secret whole even if it
/// straddles the last 8 KiB.
const HOOK_STDERR_HELD_BYTES: usize = 4 * MAX_HOOK_STDERR_BYTES;

/// One hook run, before its answer is read.
#[derive(Debug)]
pub struct HookRun {
    /// Raw stdout, at most `MAX_HOOK_STDOUT_BYTES + 1` bytes: reading stops
    /// one byte past the ceiling, which is how `OutputTooLarge` is known.
    pub stdout: Vec<u8>,
    /// Redacted, and only the last `MAX_HOOK_STDERR_BYTES`. For the user
    /// debugging their hook; never for the model (proposal §5.9).
    pub stderr: String,
    /// `None` when the hook never started.
    pub exit: Option<ExitStatus>,
    /// Why the run cannot be taken as an answer. `None` means the hook
    /// exited 0 within its deadline and its bounds.
    pub failure: Option<HookFailureKind>,
    pub duration: Duration,
}

/// Runs one hook to completion, its deadline, or its output ceiling.
///
/// The same shape as `CommandRunner::run_at` but not a call to it
/// (`context.md` §2): the hook is executed directly, not through the user's
/// login shell; it gets the envelope on stdin; and oversized output is a
/// failure rather than a truncation. The child runs in its own process
/// group, registered as `ProcessKind::Hook` under `registry_owner` (the task
/// or session id), so a crash mid-hook leaves it for the sweep.
///
/// `timeout` is the caller's `HookConfig::deadline_ms(event)`.
pub fn run_hook(
    hook: &HookConfig,
    envelope: &HookEnvelope,
    repository_root: &Path,
    data_dir: &Path,
    registry_owner: &str,
    scanner: &SecretScanner,
    timeout: Duration,
) -> HookRun {
    let started = Instant::now();
    let (mut child, mut guard) = match spawn_hook(
        hook,
        envelope.event,
        repository_root,
        data_dir,
        registry_owner,
    ) {
        Ok(spawned) => spawned,
        Err(why) => {
            return HookRun {
                stdout: Vec::new(),
                stderr: scanner.redact(&why).text,
                exit: None,
                failure: Some(HookFailureKind::SpawnFailed),
                duration: started.elapsed(),
            };
        }
    };

    // On its own thread, so a hook that never reads stdin cannot hold the
    // runner in a blocked write past its deadline. A write that fails because
    // the hook exited without reading is not a failure: it may answer
    // without looking.
    let input = serde_json::to_vec(envelope).unwrap_or_default();
    let mut stdin = child.stdin.take().expect("stdin was piped");
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });

    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let stdout_reader = spawn_reader(
        child.stdout.take().expect("stdout was piped"),
        Arc::clone(&stdout),
        keep_up_to_the_ceiling,
    );
    let stderr_reader = spawn_reader(
        child.stderr.take().expect("stderr was piped"),
        Arc::clone(&stderr),
        keep_the_tail,
    );

    let deadline = started + timeout;
    let mut status: Option<ExitStatus> = None;
    let mut failure = loop {
        if lock(&stdout).len() > MAX_HOOK_STDOUT_BYTES {
            break Some(HookFailureKind::OutputTooLarge);
        }
        if status.is_none()
            && let Ok(Some(exit)) = child.try_wait()
        {
            status = Some(exit);
        }
        // Exited is not enough: the answer is whatever reached stdout, so the
        // pipes must have closed too.
        if status.is_some() && stdout_reader.is_finished() && stderr_reader.is_finished() {
            break None;
        }
        if Instant::now() >= deadline {
            break Some(HookFailureKind::Timeout);
        }
        std::thread::sleep(HOOK_POLL_INTERVAL);
    };

    if status.is_none() {
        kill_process_group(child.id());
        // Reap so the guard does not kill a pid that could have been reused.
        status = child.wait().ok();
    }
    // Reaped, so the pid is free and its group may already be someone
    // else's. A leader that exited while something it started still holds a
    // pipe is not killed here for that reason; the reader is abandoned
    // instead, as `CommandGuard` does.
    guard.armed = false;
    drop(guard);
    for reader in [stdout_reader, stderr_reader] {
        if reader.is_finished() {
            let _ = reader.join();
        }
    }

    let stdout = std::mem::take(&mut *lock(&stdout));
    if failure.is_none() && stdout.len() > MAX_HOOK_STDOUT_BYTES {
        failure = Some(HookFailureKind::OutputTooLarge);
    }
    if failure.is_none() && !status.is_some_and(|exit| exit.success()) {
        failure = Some(HookFailureKind::NonZeroExit);
    }
    let stderr = String::from_utf8_lossy(&lock(&stderr)).into_owned();
    HookRun {
        stdout,
        stderr: last_bytes(&scanner.redact(&stderr).text, MAX_HOOK_STDERR_BYTES).to_string(),
        exit: status,
        failure,
        duration: started.elapsed(),
    }
}

/// Starts the hook and registers it. The error is the message the user sees
/// for a `SpawnFailed` hook.
fn spawn_hook(
    hook: &HookConfig,
    event: HookEvent,
    repository_root: &Path,
    data_dir: &Path,
    registry_owner: &str,
) -> Result<(Child, HookGuard), String> {
    let mut command = Command::new(&hook.command);
    command.args(&hook.args).env_clear();
    for name in HOOK_INHERITED_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = command
        .env("DAMAIAN_HOOK_ID", &hook.id)
        .env("DAMAIAN_HOOK_EVENT", event.as_str())
        .env("DAMAIAN_REPOSITORY_ROOT", repository_root)
        .current_dir(repository_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("could not start `{}`: {error}", hook.command))?;
    let registration = ProcessRegistry::open(data_dir)
        .and_then(|registry| registry.register(ProcessKind::Hook, registry_owner, child.id()));
    match registration {
        Ok(registration) => {
            let guard = HookGuard {
                pid: child.id(),
                armed: true,
                _registration: registration,
            };
            Ok((child, guard))
        }
        Err(error) => {
            // An unregistered hook could not be swept after a crash, so it
            // does not get to run.
            kill_process_group(child.id());
            let _ = child.wait();
            Err(format!("could not register `{}`: {error}", hook.command))
        }
    }
}

/// `CommandGuard`'s counterpart: kills the hook's group if the runner
/// unwinds before reaping it, and removes the registry entry on every path.
struct HookGuard {
    pid: u32,
    armed: bool,
    _registration: RegistrationHandle,
}

impl Drop for HookGuard {
    fn drop(&mut self) {
        if self.armed {
            kill_process_group(self.pid);
        }
    }
}

fn lock(buffer: &Mutex<Vec<u8>>) -> MutexGuard<'_, Vec<u8>> {
    buffer.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads a pipe into `buffer` until EOF, or until `append` says stop.
fn spawn_reader<R: Read + Send + 'static>(
    mut pipe: R,
    buffer: Arc<Mutex<Vec<u8>>>,
    append: fn(&mut Vec<u8>, &[u8]) -> bool,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    if !append(&mut lock(&buffer), &chunk[..read]) {
                        break;
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    })
}

/// Stdout: one byte past the ceiling is enough to know it was exceeded, and
/// reading on would only let an endless writer run to its deadline.
fn keep_up_to_the_ceiling(buffer: &mut Vec<u8>, chunk: &[u8]) -> bool {
    buffer.extend_from_slice(chunk);
    if buffer.len() > MAX_HOOK_STDOUT_BYTES {
        buffer.truncate(MAX_HOOK_STDOUT_BYTES + 1);
        return false;
    }
    true
}

/// Stderr: read to the end so the hook never blocks on a full pipe, keeping
/// only the tail.
fn keep_the_tail(buffer: &mut Vec<u8>, chunk: &[u8]) -> bool {
    buffer.extend_from_slice(chunk);
    if buffer.len() > 2 * HOOK_STDERR_HELD_BYTES {
        buffer.drain(..buffer.len() - HOOK_STDERR_HELD_BYTES);
    }
    true
}

/// The last `max` bytes of `text`, starting on a character boundary.
fn last_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditLog;
    use crate::config::{HookMode, HookSource};

    /// The crash half of "leaves no process behind" (Task 3): a hook whose
    /// runner died without reaping it is found by spec 46's sweep through its
    /// registry entry. In-crate because simulating the crash needs
    /// `spawn_hook`'s guard, which `run_hook` never hands out.
    ///
    /// It spawns `/bin/sh` on a script and kills it, the same as the runner's
    /// own timeout test, and the sweep only reaches entries in its own scratch
    /// data directory, so it is not `#[ignore]`d.
    #[test]
    fn a_hook_left_running_by_a_crash_is_killed_by_the_sweep() {
        let root = std::env::temp_dir().join(format!(
            "damaian-hook-sweep-{}-{}",
            std::process::id(),
            crate::hash::now_millis()
        ));
        let data_dir = root.join(".damaian-data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let script = root.join("hang.sh");
        std::fs::write(&script, "sleep 30\n").unwrap();
        let hook = HookConfig {
            id: "hangs".to_string(),
            command: "/bin/sh".to_string(),
            args: vec![script.to_string_lossy().to_string()],
            events: vec![HookEvent::BeforeTool],
            mode: HookMode::Mandatory,
            order: 0,
            timeout_ms: None,
            enabled: true,
            source: HookSource::User,
        };

        let (mut child, guard) =
            spawn_hook(&hook, HookEvent::BeforeTool, &root, &data_dir, "task_1").unwrap();
        // The crash: nothing reaps the hook, kills its group or removes its
        // registry entry.
        std::mem::forget(guard);

        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let entries = registry.entries().unwrap();
        assert_eq!(entries.len(), 1, "the hook was not registered");
        let entry = entries[0].1.clone().expect("a parsable entry");
        assert_eq!(entry.kind, "hook");
        assert_eq!(entry.pid, child.id());
        assert_eq!(entry.session_id, "task_1");

        let audit = AuditLog::new(&data_dir, true, SecretScanner::new(Vec::new()));
        let report = registry.sweep_own(&audit).unwrap();

        let settle = Instant::now() + Duration::from_secs(2);
        let exited = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() > settle {
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if exited.is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(report.killed(), 1);
        assert!(exited.is_some(), "the sweep left the hook running");
    }
}
