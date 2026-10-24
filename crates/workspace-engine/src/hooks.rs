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
//!
//! The dispatcher (`context.md` §7, §10, §11): `HookDispatcher` orders the
//! hooks for an event, applies the review gate to a repository's own hooks,
//! runs each one, audits every invocation, and turns answers into redacted
//! findings, warnings, context and requested commands. It does not decide
//! what a verdict means at an event; that is the call sites' job
//! (`context.md` §4).
//!
//! Requested commands (`context.md` §9): `HookDispatcher::run_requested_commands`
//! runs a hook's `commands` only when policy would run them unasked, brackets
//! each with a `hook_command` action marker, and bounds the nested dispatch
//! through `before_tool` and `after_tool` by `MAX_HOOK_DEPTH`.

use crate::audit::AuditLog;
use crate::cancel::CancelToken;
use crate::chat::{CommandRequest, ToolAction};
use crate::command_runner::CommandTermination;
use crate::command_runner::kill_process_group;
use crate::config::Config;
use crate::finding::{
    Finding, FindingDraft, FindingSource, Severity, SourceRange, default_parsers,
    findings_from_execution,
};
use crate::hash::{repository_id_for_root, sha256};
use crate::mode::{SessionMode, permits};
use crate::process_registry::{ProcessKind, ProcessRegistry, RegistrationHandle};
use crate::profile::ProfileId;
use crate::secret_scanner::SecretScanner;
use crate::session::{Session, SessionStore, Task};
use crate::validation::ValidationOrchestrator;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
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
/// How deep a hook's requested command may nest before hook dispatch is
/// skipped for the nested action (`context.md` §9). Past this, the nested
/// command still runs: blocking it would make a hook unable to run any
/// command at all (proposal §5.7).
pub const MAX_HOOK_DEPTH: u32 = 2;

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

/// Whether a hook runs, and why not when it does not (`context.md` §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookActivation {
    Active,
    /// A repository-defined hook the user has not reviewed yet.
    PendingReview,
    /// The user declined this repository hook.
    Declined,
    /// The definition is not the one the user approved.
    ChangedSinceReview,
    Disabled,
}

impl HookActivation {
    pub fn as_str(self) -> &'static str {
        match self {
            HookActivation::Active => "active",
            HookActivation::PendingReview => "pending_review",
            HookActivation::Declined => "declined",
            HookActivation::ChangedSinceReview => "changed_since_review",
            HookActivation::Disabled => "disabled",
        }
    }
}

/// The definition hash a review records (`context.md` §7). It covers the
/// definition's fields and the content of every command or argument that
/// resolves to a file inside the repository, so a repository cannot keep an
/// approved `command=./scripts/check.sh` and change the script after review.
pub fn hook_definition_hash(
    hook: &HookConfig,
    repository_root: &Path,
) -> crate::error::Result<String> {
    let events: Vec<&str> = hook.events.iter().map(|event| event.as_str()).collect();
    let timeout = hook
        .timeout_ms
        .map(|milliseconds| milliseconds.to_string())
        .unwrap_or_else(|| "none".to_string());
    let mut material = format!(
        "command={}\nargs={}\nevents={}\nmode={}\norder={}\ntimeout_ms={timeout}\n",
        hook.command,
        hook.args.join("\u{1f}"),
        events.join(","),
        hook.mode.as_str(),
        hook.order,
    );

    let root = fs::canonicalize(repository_root).unwrap_or_else(|_| repository_root.to_path_buf());
    let mut named = Vec::with_capacity(hook.args.len() + 1);
    named.push(Path::new(&hook.command).to_path_buf());
    named.extend(hook.args.iter().map(PathBuf::from));
    for path in named {
        let candidate = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        let Ok(resolved) = fs::canonicalize(&candidate) else {
            continue;
        };
        if !resolved.starts_with(&root) || !resolved.is_file() {
            continue;
        }
        let relative = resolved.strip_prefix(&root).unwrap_or(&resolved);
        material.push_str(&format!(
            "file={}={}\n",
            relative.to_string_lossy(),
            sha256(fs::read(&resolved)?)
        ));
    }
    Ok(sha256(material))
}

/// Whether a hook runs now (`context.md` §7.1). A repository's `command` must
/// resolve inside the repository or be an absolute system path, so a
/// repository cannot point a relative path out of the clone. Failing that
/// check is `ChangedSinceReview`: the definition is not one the user
/// approved, and the fail direction is to block a mandatory hook rather than
/// skip it silently.
pub fn activation(
    config: &Config,
    hook: &HookConfig,
    repository_id: &str,
    repository_root: &Path,
) -> HookActivation {
    if !hook.enabled {
        return HookActivation::Disabled;
    }
    if hook.source != HookSource::Repository {
        return HookActivation::Active;
    }
    if !repository_command_is_contained(hook, repository_root) {
        return HookActivation::ChangedSinceReview;
    }
    let Some(review) = config
        .hook_reviews_by_repository
        .get(repository_id)
        .and_then(|reviews| reviews.get(&hook.id))
    else {
        return HookActivation::PendingReview;
    };
    match review {
        crate::config::HookReview::Declined => HookActivation::Declined,
        crate::config::HookReview::Approved { hash } => {
            match hook_definition_hash(hook, repository_root) {
                Ok(current) if &current == hash => HookActivation::Active,
                // A hash that cannot be computed is not the reviewed one.
                _ => HookActivation::ChangedSinceReview,
            }
        }
    }
}

fn repository_command_is_contained(hook: &HookConfig, repository_root: &Path) -> bool {
    let command = Path::new(&hook.command);
    if command.is_absolute() {
        return true;
    }
    let root = fs::canonicalize(repository_root).unwrap_or_else(|_| repository_root.to_path_buf());
    fs::canonicalize(root.join(command)).is_ok_and(|resolved| resolved.starts_with(&root))
}

/// Where a dispatch sits in the recursion graph (`context.md` §9). Task 5
/// increments `depth` and adds the requesting hook to `excluded`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookContext {
    pub depth: u32,
    pub excluded: BTreeSet<String>,
}

impl HookContext {
    pub fn root() -> Self {
        Self::default()
    }
}

/// What a dispatch has to know besides the event and the repository. The
/// dispatcher builds one envelope per hook from it.
#[derive(Debug, Clone)]
pub struct HookEnvelopeParts {
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub mode: String,
    pub profile: String,
    pub payload: Value,
}

/// A command a hook asked Damaian to run, kept with its requester so the
/// nested dispatch can exclude the hook that asked (`context.md` §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedCommand {
    pub hook_id: String,
    /// Already redacted.
    pub command: String,
}

/// What a dispatch decided, with every string already redacted.
#[derive(Debug, Clone, PartialEq)]
pub struct HookOutcome {
    pub verdict: Verdict,
    /// The hook whose verdict set the outcome.
    pub deciding_hook: Option<String>,
    /// Denials and approval requests, by hook id. These are the reasons a
    /// call site shows to the user.
    pub reasons: Vec<(String, String)>,
    /// Warnings, including an optional hook's failure and a `warn` reason.
    /// Each one is also a `Finding` below.
    pub warnings: Vec<String>,
    pub findings: Vec<Finding>,
    /// `(hook id, text)`, honoured only at `session_start` and
    /// `before_context` (`context.md` §10).
    pub context: Vec<(String, String)>,
    pub commands: Vec<RequestedCommand>,
    pub depth_exceeded: bool,
}

impl HookOutcome {
    pub fn allow() -> Self {
        HookOutcome {
            verdict: Verdict::Allow,
            deciding_hook: None,
            reasons: Vec::new(),
            warnings: Vec::new(),
            findings: Vec::new(),
            context: Vec::new(),
            commands: Vec::new(),
            depth_exceeded: false,
        }
    }
}

impl Default for HookOutcome {
    fn default() -> Self {
        Self::allow()
    }
}

/// The stable facts a requested command's nested dispatch needs. `depth` is
/// the depth of the dispatch that asked for the command; its own
/// `before_tool` and `after_tool` run at `depth + 1` (`context.md` §9).
struct RequestedRun<'a> {
    session: &'a Session,
    task: &'a Task,
    mode: SessionMode,
    repository_root: &'a Path,
    depth: u32,
}

/// What `after_tool` sees for a requested command (`context.md` §6).
fn after_tool_payload(
    request: &RequestedCommand,
    execution: Option<&crate::command_runner::CommandExecution>,
) -> Value {
    let ok = execution.is_some_and(|execution| {
        execution.termination == CommandTermination::Exited && execution.exit_code == Some(0)
    });
    let mut payload = json!({
        "tool": "run_command",
        "paths": [],
        "command": request.command,
        "outcome": if ok { "ok" } else { "failed" },
    });
    if let Some(execution) = execution {
        payload["termination"] = json!(execution.termination.as_str());
        if let Some(code) = execution.exit_code {
            payload["exitCode"] = json!(code);
        }
    }
    payload
}

/// Runs the hooks for one event (`context.md` §7, §10, §11) and the commands
/// they ask for (`§9`).
#[derive(Debug, Clone)]
pub struct HookDispatcher {
    config: Config,
    scanner: SecretScanner,
    audit_log: AuditLog,
    data_dir: PathBuf,
    validation: ValidationOrchestrator,
    session_store: SessionStore,
}

/// The fields of one `hook_invoked` event that depend on the run
/// (`context.md` §11).
struct InvocationAudit<'a> {
    verdict: Verdict,
    failure: Option<HookFailureKind>,
    duration: Duration,
    /// Denials and requests only.
    reason: Option<&'a str>,
}

impl HookDispatcher {
    pub fn new(
        config: Config,
        scanner: SecretScanner,
        audit_log: AuditLog,
        validation: ValidationOrchestrator,
        session_store: SessionStore,
    ) -> Self {
        let data_dir = config.data_dir.clone();
        Self {
            config,
            scanner,
            audit_log,
            data_dir,
            validation,
            session_store,
        }
    }

    /// Orders the active hooks for `event`, runs them, and resolves their
    /// answers. With no hook left, nothing is spawned and nothing is
    /// audited.
    pub fn dispatch(
        &self,
        event: HookEvent,
        parts: HookEnvelopeParts,
        repository_root: &Path,
        ctx: &HookContext,
    ) -> HookOutcome {
        let hooks: Vec<&HookConfig> = ordered(&self.config.hooks, event)
            .into_iter()
            .filter(|hook| !ctx.excluded.contains(&hook.id))
            .collect();
        let mut outcome = HookOutcome::allow();
        if hooks.is_empty() {
            return outcome;
        }

        let repository_id = repository_id_for_root(repository_root);
        let registry_owner = parts
            .task_id
            .clone()
            .or_else(|| parts.session_id.clone())
            .unwrap_or_default();
        for hook in hooks {
            match activation(&self.config, hook, &repository_id, repository_root) {
                HookActivation::Active => {}
                // Not invoked and not audited per event (`context.md` §11).
                HookActivation::PendingReview
                | HookActivation::Declined
                | HookActivation::Disabled => continue,
                HookActivation::ChangedSinceReview => {
                    let kind = HookFailureKind::ChangedSinceReview;
                    self.record_failure(hook, event, &parts, kind, Duration::ZERO, &mut outcome);
                    if short_circuits(outcome.verdict) {
                        break;
                    }
                    continue;
                }
            }

            let envelope = HookEnvelope {
                event,
                hook_api_version: HOOK_API_VERSION,
                session_id: parts.session_id.clone(),
                task_id: parts.task_id.clone(),
                mode: parts.mode.clone(),
                profile: parts.profile.clone(),
                repository_root: repository_root.to_path_buf(),
                hook_depth: ctx.depth,
                payload: parts.payload.clone(),
            };
            let run = run_hook(
                hook,
                &envelope,
                repository_root,
                &self.data_dir,
                &registry_owner,
                &self.scanner,
                Duration::from_millis(hook.deadline_ms(event)),
            );

            let mut failure = run.failure;
            if failure.is_none() {
                match parse_response(&run.stdout) {
                    Ok(response) => {
                        let reason = match response.verdict {
                            Verdict::Deny | Verdict::RequestApproval => response
                                .reason
                                .as_deref()
                                .map(|reason| self.scanner.redact(reason).text),
                            Verdict::Allow | Verdict::Warn => None,
                        };
                        self.audit_invocation(
                            hook,
                            event,
                            &parts,
                            InvocationAudit {
                                verdict: response.verdict,
                                failure: None,
                                duration: run.duration,
                                reason: reason.as_deref(),
                            },
                        );
                        self.apply_response(&mut outcome, hook, event, response);
                        if short_circuits(outcome.verdict) {
                            break;
                        }
                        continue;
                    }
                    Err(ResponseError::Widening { attempts }) => {
                        self.audit_widening(hook, event, &attempts);
                        failure = Some(HookFailureKind::Malformed);
                    }
                    Err(error) => failure = Some(error.failure_kind()),
                }
            }
            let Some(kind) = failure else {
                continue;
            };
            self.record_failure(hook, event, &parts, kind, run.duration, &mut outcome);
            if short_circuits(outcome.verdict) {
                break;
            }
        }
        outcome
    }

    /// Runs the commands one dispatch's hooks asked for (`context.md` §9).
    ///
    /// Only a command policy would run unasked is run: it is classified at
    /// the repository root and put through the same `permits` check as a
    /// turn's own command, so Ask mode and a narrower profile refuse it. A
    /// hook never raises an approval card, so a blocked command, one that
    /// needs approval, or one the mode or profile refuses is not run; it
    /// becomes a warning finding and a `hook_command_refused` audit entry.
    ///
    /// Each command that runs is bracketed by a `hook_command` action marker,
    /// and its output becomes `FindingSource::Hook` findings recorded in the
    /// session through `findings_from_execution`. Before and after it runs,
    /// `before_tool` and `after_tool` are dispatched at `ctx.depth + 1` with
    /// the requesting hook excluded. Past [`MAX_HOOK_DEPTH`] that dispatch is
    /// skipped, `hook_depth_exceeded` is audited, a warning finding is
    /// returned, and the command still runs.
    ///
    /// The commands are taken from `outcome`, so a caller that runs them in
    /// turn does not run them twice. Nothing here changes the verdict the
    /// requesting hook produced: its command's result is not evidence for or
    /// against its answer, and the only field this sets is
    /// `depth_exceeded`. The returned findings are the ones a call site must
    /// surface: refusals, the depth report, nested hooks' findings and an
    /// `after_tool` denial. A command's own output is recorded in the session,
    /// not returned, so a caller cannot record it twice.
    pub fn run_requested_commands(
        &self,
        outcome: &mut HookOutcome,
        session: &Session,
        task: Option<&Task>,
        mode: SessionMode,
        repository_root: &Path,
        ctx: &HookContext,
    ) -> Vec<Finding> {
        let commands = std::mem::take(&mut outcome.commands);
        let mut findings = Vec::new();
        let mut depth_exceeded = outcome.depth_exceeded;
        for request in &commands {
            let Some(task) = task else {
                self.refuse_requested_command(
                    request,
                    None,
                    "a requested command needs the task it belongs to, and this event has none",
                    &mut findings,
                );
                continue;
            };
            let run = RequestedRun {
                session,
                task,
                mode,
                repository_root,
                depth: ctx.depth,
            };
            self.run_requested_command(request, &run, &mut findings, &mut depth_exceeded);
        }
        outcome.depth_exceeded = depth_exceeded;
        findings
    }

    /// One requested command: the policy gate, the nested dispatch, the run
    /// and the recursion. `findings` and `depth_exceeded` outlive the
    /// recursion so everything raised deep inside reaches the caller of
    /// [`Self::run_requested_commands`] and marks its outcome.
    fn run_requested_command(
        &self,
        request: &RequestedCommand,
        run: &RequestedRun<'_>,
        findings: &mut Vec<Finding>,
        depth_exceeded: &mut bool,
    ) {
        let classification = self
            .validation
            .classify_command(run.repository_root, &request.command);
        let action = ToolAction::Command(CommandRequest {
            command: request.command.clone(),
            reason: format!("Requested by hook `{}`", request.hook_id),
            working_directory: None,
        });
        let permission = permits(
            run.mode,
            &self.config.profile_capabilities(),
            &action,
            Some(&classification),
            None,
        );
        if classification.blocked {
            let reason = classification.reasons.join("; ");
            let reason = if reason.is_empty() {
                "the command policy blocks it".to_string()
            } else {
                reason
            };
            self.refuse_requested_command(request, Some(run.task), &reason, findings);
            return;
        }
        if classification.requires_approval {
            self.refuse_requested_command(
                request,
                Some(run.task),
                "the command policy would ask for approval, and a hook never raises an approval card",
                findings,
            );
            return;
        }
        if !permission.is_allowed() {
            self.refuse_requested_command(
                request,
                Some(run.task),
                &crate::mode::refusal_message(permission),
                findings,
            );
            return;
        }

        let nested_depth = run.depth + 1;
        let nested = HookContext {
            depth: nested_depth,
            // The requesting hook only: a deeper command belongs to another
            // hook, which is free to see this one (`context.md` §9).
            excluded: BTreeSet::from([request.hook_id.clone()]),
        };
        let mut before = None;
        if nested_depth > MAX_HOOK_DEPTH {
            *depth_exceeded = true;
            self.audit_depth_exceeded(&request.hook_id, nested_depth);
            findings.push(self.requested_finding(
                Some(run.task),
                &request.hook_id,
                Severity::Warning,
                format!(
                    "hook `{}` requested `{}` at hook depth {nested_depth}, past the limit of \
                     {MAX_HOOK_DEPTH}; it ran without a before_tool or after_tool check",
                    request.hook_id, request.command
                ),
                None,
            ));
        } else {
            let outcome = self.dispatch_command_event(
                HookEvent::BeforeTool,
                run,
                json!({
                    "tool": "run_command",
                    "paths": [],
                    "command": request.command,
                }),
                &nested,
            );
            findings.extend(outcome.findings.iter().cloned());
            if matches!(outcome.verdict, Verdict::Deny | Verdict::RequestApproval) {
                let (denier, reason) = outcome
                    .reasons
                    .first()
                    .cloned()
                    .unwrap_or_else(|| (request.hook_id.clone(), String::new()));
                let reason = match outcome.verdict {
                    Verdict::Deny => format!("hook `{denier}` denied it: {reason}"),
                    _ => format!(
                        "hook `{denier}` asked for approval ({reason}), and a hook never raises \
                         an approval card"
                    ),
                };
                self.refuse_requested_command(request, Some(run.task), &reason, findings);
                return;
            }
            before = Some(outcome);
        }

        let execution = self.execute_requested_command(request, run, findings);

        if let Some(before) = before {
            let after = self.dispatch_command_event(
                HookEvent::AfterTool,
                run,
                after_tool_payload(request, execution.as_ref()),
                &nested,
            );
            findings.extend(after.findings.iter().cloned());
            if after.verdict == Verdict::Deny {
                // Nothing is undone (context.md §4); the hook's reason is
                // recorded as an error so the failure is visible.
                for (hook_id, reason) in &after.reasons {
                    findings.push(self.requested_finding(
                        Some(run.task),
                        hook_id,
                        Severity::Error,
                        format!(
                            "hook `{hook_id}` was denied after `{}` ran: {reason}",
                            request.command
                        ),
                        None,
                    ));
                }
            }
            for nested_request in before.commands.iter().chain(after.commands.iter()) {
                self.run_requested_command(
                    nested_request,
                    &RequestedRun {
                        depth: nested_depth,
                        ..*run
                    },
                    findings,
                    depth_exceeded,
                );
            }
        }
    }

    /// Dispatches `event` for one requested command at `run.depth + 1`.
    fn dispatch_command_event(
        &self,
        event: HookEvent,
        run: &RequestedRun<'_>,
        payload: Value,
        nested: &HookContext,
    ) -> HookOutcome {
        self.dispatch(
            event,
            HookEnvelopeParts {
                session_id: Some(run.session.id.clone()),
                task_id: Some(run.task.id.clone()),
                mode: run.mode.as_str().to_string(),
                profile: self.profile_id(run.session),
                payload,
            },
            run.repository_root,
            nested,
        )
    }

    /// The profile the repository is running under, for the envelope. No
    /// selection applies nothing, which is `Full` (`config.rs`, the profile
    /// merge).
    fn profile_id(&self, session: &Session) -> String {
        self.config
            .permission_profile_by_repository
            .get(&session.repository_id)
            .map_or("full", ProfileId::as_str)
            .to_string()
    }

    /// The envelope fields a call site supplies for an event it dispatches
    /// itself (Tasks 6–8). `session` is `None` only where the event precedes
    /// the session (`session_start`), which then runs as the default mode and
    /// profile — the same values an envelope without a session has always
    /// carried.
    pub(crate) fn envelope_parts(
        &self,
        session: Option<&Session>,
        task_id: Option<&str>,
        payload: Value,
    ) -> HookEnvelopeParts {
        HookEnvelopeParts {
            session_id: session.map(|session| session.id.clone()),
            task_id: task_id.map(str::to_string),
            mode: session
                .map(|session| self.session_store.session_mode(&session.id))
                .unwrap_or(SessionMode::Code)
                .as_str()
                .to_string(),
            profile: session.map_or_else(|| "full".to_string(), |session| self.profile_id(session)),
            payload,
        }
    }

    /// Proposes and runs one permitted command behind a `hook_command`
    /// marker, and turns its output into findings. `None` means it did not
    /// run; the reason is already a finding.
    fn execute_requested_command(
        &self,
        request: &RequestedCommand,
        run: &RequestedRun<'_>,
        findings: &mut Vec<Finding>,
    ) -> Option<crate::command_runner::CommandExecution> {
        // A command is a repository write as far as spec 17 is concerned,
        // exactly as `run_command` is at the turn's own marker.
        let marker =
            match self
                .session_store
                .start_action(run.task, "hook_command", &request.command, true)
            {
                Ok(marker) => marker,
                Err(error) => {
                    findings.push(self.requested_finding(
                        Some(run.task),
                        &request.hook_id,
                        Severity::Error,
                        format!(
                            "hook `{}` requested `{}`, which could not be started: {error}",
                            request.hook_id, request.command
                        ),
                        None,
                    ));
                    return None;
                }
            };
        let reason = format!("Requested by hook `{}`", request.hook_id);
        let proposal = match self.validation.propose_command_at(
            run.repository_root,
            run.repository_root,
            &request.command,
            &reason,
        ) {
            Ok(proposal) => proposal,
            Err(error) => {
                let _ = self.session_store.finish_action(marker, "failed");
                findings.push(self.requested_finding(
                    Some(run.task),
                    &request.hook_id,
                    Severity::Error,
                    format!(
                        "hook `{}` requested `{}`, which could not be proposed: {error}",
                        request.hook_id, request.command
                    ),
                    None,
                ));
                return None;
            }
        };

        let cancel = CancelToken::new();
        match self.validation.run_proposal(
            &proposal.id,
            false,
            "policy",
            Some(&run.task.id),
            &cancel,
            &mut |_| {},
        ) {
            Ok(record) => {
                let _ = self
                    .session_store
                    .finish_command_action(marker, record.execution.exit_code);
                for finding in
                    findings_from_execution(&record.execution, &default_parsers(), &self.scanner)
                {
                    let finding = finding
                        .with_task_id(run.task.id.clone())
                        .with_origin_ref(format!("hook:{}", request.hook_id));
                    if let Err(error) = self.session_store.record_finding(&run.session.id, &finding)
                    {
                        findings.push(self.requested_finding(
                            Some(run.task),
                            &request.hook_id,
                            Severity::Error,
                            format!(
                                "hook `{}` requested `{}`, whose findings could not be recorded: \
                                 {error}",
                                request.hook_id, request.command
                            ),
                            None,
                        ));
                    }
                }
                Some(record.execution)
            }
            Err(error) => {
                let _ = self.session_store.finish_action(marker, "failed");
                findings.push(self.requested_finding(
                    Some(run.task),
                    &request.hook_id,
                    Severity::Error,
                    format!(
                        "hook `{}` requested `{}`, which did not run: {error}",
                        request.hook_id, request.command
                    ),
                    None,
                ));
                None
            }
        }
    }

    /// Records why a requested command did not run: the audit names the
    /// hook and the redacted command, and the warning finding names both.
    fn refuse_requested_command(
        &self,
        request: &RequestedCommand,
        task: Option<&Task>,
        reason: &str,
        findings: &mut Vec<Finding>,
    ) {
        let _ = self.audit_log.record(
            "hook_command_refused",
            &[
                ("hookId", request.hook_id.clone()),
                ("command", request.command.clone()),
                ("reason", reason.to_string()),
            ],
        );
        findings.push(self.requested_finding(
            task,
            &request.hook_id,
            Severity::Warning,
            format!(
                "hook `{}` requested `{}`, which was not run: {reason}",
                request.hook_id, request.command
            ),
            None,
        ));
    }

    fn requested_finding(
        &self,
        task: Option<&Task>,
        hook_id: &str,
        severity: Severity,
        summary: String,
        details: Option<String>,
    ) -> Finding {
        let draft = FindingDraft {
            source: FindingSource::Hook,
            severity,
            summary,
            details,
            range: None,
            code: None,
        };
        let finding = Finding::new(draft, &self.scanner).with_origin_ref(format!("hook:{hook_id}"));
        match task {
            Some(task) => finding.with_task_id(task.id.clone()),
            None => finding,
        }
    }

    fn audit_depth_exceeded(&self, hook_id: &str, depth: u32) {
        let _ = self.audit_log.record(
            "hook_depth_exceeded",
            &[
                ("hookId", hook_id.to_string()),
                ("event", HookEvent::BeforeTool.as_str().to_string()),
                ("depth", depth.to_string()),
            ],
        );
    }

    /// A mandatory failure denies; an optional one warns (`context.md` §4,
    /// §5.5). Both are audited as `hook_invoked` with the failure kind.
    fn record_failure(
        &self,
        hook: &HookConfig,
        event: HookEvent,
        parts: &HookEnvelopeParts,
        kind: HookFailureKind,
        duration: Duration,
        outcome: &mut HookOutcome,
    ) {
        let reason = failure_text(hook, kind);
        let verdict = match hook.mode {
            HookMode::Mandatory => Verdict::Deny,
            HookMode::Optional => Verdict::Warn,
        };
        let audit_reason = (hook.mode == HookMode::Mandatory).then_some(reason.as_str());
        self.audit_invocation(
            hook,
            event,
            parts,
            InvocationAudit {
                verdict,
                failure: Some(kind),
                duration,
                reason: audit_reason,
            },
        );
        match hook.mode {
            HookMode::Mandatory => {
                outcome.reasons.push((hook.id.clone(), reason));
                outcome.verdict = Verdict::Deny;
                outcome.deciding_hook = Some(hook.id.clone());
            }
            HookMode::Optional => self.push_warning(outcome, &hook.id, &reason),
        }
    }

    fn apply_response(
        &self,
        outcome: &mut HookOutcome,
        hook: &HookConfig,
        event: HookEvent,
        response: HookResponse,
    ) {
        for finding in response.findings {
            let range = hook_finding_range(&finding);
            let draft = FindingDraft {
                source: FindingSource::Hook,
                severity: finding.severity,
                summary: finding.summary,
                details: finding.details,
                range,
                code: None,
            };
            outcome.findings.push(
                Finding::new(draft, &self.scanner).with_origin_ref(format!("hook:{}", hook.id)),
            );
        }

        let context: Vec<String> = response
            .context
            .iter()
            .map(|entry| self.scanner.redact(entry).text)
            .collect();
        if matches!(event, HookEvent::SessionStart | HookEvent::BeforeContext) {
            for entry in context {
                outcome.context.push((hook.id.clone(), entry));
            }
        } else if !context.is_empty() {
            self.push_warning(
                outcome,
                &hook.id,
                &format!(
                    "hook `{}` returned context at {}; context is used only at session_start and before_context",
                    hook.id,
                    event.as_str()
                ),
            );
        }

        for command in response.commands {
            outcome.commands.push(RequestedCommand {
                hook_id: hook.id.clone(),
                command: self.scanner.redact(&command).text,
            });
        }

        if response.verdict > outcome.verdict {
            outcome.verdict = response.verdict;
            outcome.deciding_hook = Some(hook.id.clone());
        }
        match response.verdict {
            Verdict::Allow => {}
            Verdict::Warn => {
                if let Some(reason) = response.reason {
                    let reason = self.scanner.redact(&reason).text;
                    self.push_warning(outcome, &hook.id, &reason);
                }
            }
            Verdict::Deny | Verdict::RequestApproval => {
                if let Some(reason) = response.reason {
                    outcome
                        .reasons
                        .push((hook.id.clone(), self.scanner.redact(&reason).text));
                }
            }
        }
    }

    fn push_warning(&self, outcome: &mut HookOutcome, hook_id: &str, text: &str) {
        let text = self.scanner.redact(text).text;
        outcome.warnings.push(text.clone());
        let draft = FindingDraft {
            source: FindingSource::Hook,
            severity: Severity::Warning,
            summary: text,
            details: None,
            range: None,
            code: None,
        };
        outcome
            .findings
            .push(Finding::new(draft, &self.scanner).with_origin_ref(format!("hook:{hook_id}")));
        if outcome.verdict < Verdict::Warn {
            outcome.verdict = Verdict::Warn;
            outcome.deciding_hook = Some(hook_id.to_string());
        }
    }

    fn audit_invocation(
        &self,
        hook: &HookConfig,
        event: HookEvent,
        parts: &HookEnvelopeParts,
        audit: InvocationAudit<'_>,
    ) {
        let mut fields = vec![
            ("hookId", hook.id.clone()),
            ("event", event.as_str().to_string()),
            ("sessionId", parts.session_id.clone().unwrap_or_default()),
            ("taskId", parts.task_id.clone().unwrap_or_default()),
            ("classification", hook.mode.as_str().to_string()),
            ("source", hook.source.as_str().to_string()),
            ("verdict", audit.verdict.as_str().to_string()),
            (
                "outcome",
                if audit.failure.is_some() {
                    "failed"
                } else {
                    "ok"
                }
                .to_string(),
            ),
            ("durationMs", audit.duration.as_millis().to_string()),
        ];
        if let Some(kind) = audit.failure {
            fields.push(("failureKind", kind.as_str().to_string()));
        }
        if let Some(reason) = audit.reason {
            fields.push(("reason", reason.to_string()));
        }
        let _ = self.audit_log.record("hook_invoked", &fields);
    }

    /// Each attempt is audited by the name of the field or verdict, never by
    /// the value the hook put there (`context.md` §5, §11).
    fn audit_widening(&self, hook: &HookConfig, event: HookEvent, attempts: &[WideningAttempt]) {
        for attempt in attempts {
            let (field, name) = match attempt {
                WideningAttempt::Field(name) => ("field", name),
                WideningAttempt::Verdict(name) => ("verdict", name),
            };
            let _ = self.audit_log.record(
                "hook_widening_attempt",
                &[
                    ("hookId", hook.id.clone()),
                    ("event", event.as_str().to_string()),
                    (field, name.clone()),
                ],
            );
        }
    }
}

fn failure_text(hook: &HookConfig, kind: HookFailureKind) -> String {
    format!("hook `{}` failed: {}", hook.id, kind.as_str())
}

/// A finding's range only when its path is repository-relative and does not
/// climb out, the rule `workspace_range` applies (`context.md` §5).
fn hook_finding_range(finding: &HookFinding) -> Option<SourceRange> {
    let path = finding.path.as_deref()?;
    let path_ref = Path::new(path);
    let outside = path_ref.is_absolute()
        || path_ref
            .components()
            .any(|part| part == Component::ParentDir);
    if outside {
        return None;
    }
    Some(SourceRange {
        path: path.to_string(),
        start_line: finding.line?,
        start_column: None,
        end_line: None,
        end_column: None,
    })
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
