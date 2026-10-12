//! Hooks (spec 32): external programs that see a lifecycle event and answer
//! with a verdict.
//!
//! This part is the contract alone (`context.md` §5, §6, §12): what a hook
//! is sent, what it may answer, how an answer is checked, the order hooks
//! run in and how their verdicts combine. Nothing here spawns a process or
//! reads a file.

use crate::finding::Severity;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};
use std::fmt;
use std::path::PathBuf;

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
