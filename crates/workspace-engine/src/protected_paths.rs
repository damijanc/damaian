//! Task-scoped paths that a user has excluded from patches (spec 98).
//!
//! This module owns the durable shape and the two pieces that must not grow
//! beyond their stated grammar: the direct-user prompt parser and the
//! canonicalisation of an existing path inside the task repository. The session
//! store owns replay and ancestry; the workspace engine exposes the user-facing
//! add/remove operations.
//!
//! Provenance is a property of the API, not of the text. Only
//! [`parse_protected_path_prompt`] reads prose, and its caller passes the prompt
//! the user typed. There is deliberately no function that accepts model output,
//! fetched content, repository text or tool output and establishes a
//! protection.

use crate::error::{ClientError, Result};
use crate::hash::{now_millis, repository_id_for_root};
use crate::path_policy::PathPolicy;
use crate::session::SessionStore;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::Path;

/// The session-event kind that carries a protected-path change.
pub const TASK_PROTECTED_PATHS_EVENT: &str = "task_protected_paths";

/// The only phrases the direct-user parser recognises.
pub const PROTECTED_PATH_PROMPT_PHRASES: [&str; 2] = ["do not touch", "do not modify"];

/// Whether a protected path names one file or a directory and its descendants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedPathKind {
    File,
    Directory,
}

impl ProtectedPathKind {
    /// The display label every front end uses, so a directory's descendant
    /// meaning is stated once rather than re-derived by each UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory (and everything under it)",
        }
    }
}

/// A canonical repository-relative path the user excluded from patch targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedPath {
    /// Repository-relative, with `/` separators and no leading `./`.
    pub path: String,
    pub kind: ProtectedPathKind,
}

impl ProtectedPath {
    /// Whether this protection covers `candidate`.
    ///
    /// A file covers only itself. A directory covers itself and every
    /// descendant, compared by whole path components so `src` does not cover
    /// `src2`.
    pub fn covers(&self, candidate: &str) -> bool {
        if self.path == candidate {
            return true;
        }
        self.kind == ProtectedPathKind::Directory
            && candidate
                .strip_prefix(&self.path)
                .is_some_and(|rest| rest.starts_with('/'))
    }

    /// A path the store will accept: relative, no `..`, no empty components.
    pub(crate) fn is_valid_repository_relative(&self) -> bool {
        is_valid_repository_relative_path(&self.path)
    }
}

/// The resolved protections of one task, in the order they were added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProtectedPaths {
    pub task_id: String,
    pub paths: Vec<ProtectedPath>,
}

impl TaskProtectedPaths {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The protected path that covers `repository_relative_path`, if any.
    ///
    /// Later patch-enforcement work reads this rather than comparing strings
    /// itself, so a directory protection cannot be missed by a caller that only
    /// checks exact equality.
    pub fn covering_path(&self, repository_relative_path: &str) -> Option<&ProtectedPath> {
        self.paths
            .iter()
            .find(|protected| protected.covers(repository_relative_path))
    }

    pub fn covers(&self, repository_relative_path: &str) -> bool {
        self.covering_path(repository_relative_path).is_some()
    }

    pub fn path_list(&self) -> Vec<String> {
        self.paths
            .iter()
            .map(|protected| protected.path.clone())
            .collect()
    }
}

/// One active protection with every display string a front end needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedPathRow {
    pub path: String,
    pub kind: ProtectedPathKind,
    pub kind_label: String,
    pub remove_label: String,
}

/// The complete protected-path control model the shell renders.
///
/// The engine chooses the target task from durable creation order and binds the
/// read to the repository, so every front end shows the same task and the same
/// availability decision. `available` false always carries a note explaining
/// why there is nothing to act on; `available` true with no rows carries the
/// empty note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProtectedPathsControl {
    pub available: bool,
    pub task_id: String,
    pub task_label: String,
    pub note: String,
    pub rows: Vec<ProtectedPathRow>,
}

impl TaskProtectedPathsControl {
    pub fn unavailable(note: impl Into<String>) -> Self {
        Self {
            available: false,
            task_id: String::new(),
            task_label: String::new(),
            note: note.into(),
            rows: Vec::new(),
        }
    }

    pub fn for_task(task_id: &str, active: &TaskProtectedPaths) -> Self {
        let rows = active
            .paths
            .iter()
            .map(|path| ProtectedPathRow {
                path: path.path.clone(),
                kind: path.kind,
                kind_label: path.kind.label().to_string(),
                remove_label: format!("Stop protecting {}", path.path),
            })
            .collect();
        Self {
            available: true,
            task_id: task_id.to_string(),
            task_label: format!("Task {task_id}"),
            note: if active.is_empty() {
                "No protected paths for this task.".to_string()
            } else {
                String::new()
            },
            rows,
        }
    }
}

/// Refuses `paths` if any is the protected path itself or a descendant of a
/// protected directory.
///
/// The whole proposal is refused, even when only one of several files is
/// covered: a review card that silently omitted the protected file would let
/// the user believe they approved the patch they saw (spec 98 §2.4). The error
/// names the protected paths, not only the requested target, so the model can
/// choose an in-scope alternative.
pub fn ensure_patch_paths_allowed<'a>(
    active: &TaskProtectedPaths,
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    let mut blocked: Vec<String> = Vec::new();
    for path in paths {
        if let Some(protected) = active.covering_path(path)
            && !blocked.contains(&protected.path)
        {
            blocked.push(protected.path.clone());
        }
    }
    if blocked.is_empty() {
        return Ok(());
    }
    Err(ClientError::AccessDenied(format!(
        "Patch would modify protected task path: {}",
        blocked.join(", ")
    )))
}

/// The concise model-context reminder for a task's active paths.
///
/// This is a reminder, never the enforcement boundary: proposal and apply both
/// call [`ensure_patch_paths_allowed`] from the durable record. The caller must
/// redact the returned text before it enters a model request;
/// `ChatOrchestrator::run_agentic_turn` and the legacy edit flow do so through
/// `apply_protected_paths_reminder`.
pub fn protected_paths_reminder(active: &TaskProtectedPaths) -> Option<String> {
    if active.is_empty() {
        return None;
    }
    let paths = active
        .paths
        .iter()
        .map(|protected| {
            format!(
                "- `{}`{}",
                protected.path,
                if protected.kind == ProtectedPathKind::Directory {
                    " (and everything under it)"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "The user excluded these task paths from patches. Do not propose or apply changes to them:\n{paths}"
    ))
}

/// Who established a protection. The store records this with every event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedPathSource {
    /// The narrow direct-user prompt grammar.
    Prompt,
    /// The desktop's **Protect paths** control or `damaian protect-paths`.
    ExplicitControl,
    /// A worker task that inherited its parent task's already-recorded
    /// protections at a handoff. This is neither prompt text nor an explicit
    /// control action in the worker session: the worker's prompt is constructed
    /// (or model-generated), so it is deliberately not re-parsed.
    InheritedWorkerHandoff,
}

impl ProtectedPathSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::ExplicitControl => "explicit_control",
            Self::InheritedWorkerHandoff => "inherited_worker_handoff",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedPathAction {
    Add,
    Remove,
}

impl ProtectedPathAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
        }
    }
}

/// One append-only `task_protected_paths` event.
///
/// The event is keyed by the task the user's request started. It records the
/// canonical paths, the source, the action and the time it was written; it never
/// carries model text or the prose that produced the paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProtectedPathsEvent {
    pub task_id: String,
    pub action: ProtectedPathAction,
    pub source: ProtectedPathSource,
    pub paths: Vec<ProtectedPath>,
    pub created_at_ms: u128,
}

impl TaskProtectedPathsEvent {
    pub(crate) fn new(
        task_id: impl Into<String>,
        action: ProtectedPathAction,
        source: ProtectedPathSource,
        paths: Vec<ProtectedPath>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            action,
            source,
            paths,
            created_at_ms: now_millis(),
        }
    }
}

/// Why a requested path could not become a protection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProtectedPathError {
    Empty,
    /// The request named an absolute path; only repository-relative paths are
    /// accepted, whatever the absolute path points at.
    Absolute,
    Missing,
    OutsideRepository,
    NotFileOrDirectory,
    RepositoryRoot,
    RepositoryNotAllowed,
    /// An explicit removal named a path that is not currently protected.
    NotProtected,
    Invalid {
        message: String,
    },
}

impl Display for ProtectedPathError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("The protected path is empty."),
            Self::Absolute => {
                formatter.write_str("The protected path must be repository-relative, not absolute.")
            }
            Self::Missing => {
                formatter.write_str("The protected path does not exist in this repository.")
            }
            Self::OutsideRepository => {
                formatter.write_str("The protected path resolves outside the task repository.")
            }
            Self::NotFileOrDirectory => {
                formatter.write_str("The protected path is neither a file nor a directory.")
            }
            Self::RepositoryRoot => formatter.write_str(
                "The repository root cannot be protected; choose a file or directory inside it.",
            ),
            Self::RepositoryNotAllowed => {
                formatter.write_str("The task repository is not an allowed root.")
            }
            Self::NotProtected => {
                formatter.write_str("The path is not currently protected for this task.")
            }
            Self::Invalid { message } => formatter.write_str(message),
        }
    }
}

/// A requested path and why it was refused, both typed for a UI and printable
/// for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedPathRefusal {
    pub requested: String,
    pub error: ProtectedPathError,
}

impl ProtectedPathRefusal {
    pub fn message(&self) -> String {
        if self.requested.is_empty() {
            self.error.to_string()
        } else {
            format!("{} ({})", self.error, self.requested)
        }
    }
}

/// The result of an explicit add or removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProtectedPathUpdate {
    pub task_id: String,
    pub action: ProtectedPathAction,
    pub source: ProtectedPathSource,
    /// The canonical paths the event recorded. Empty when every request was
    /// refused, in which case no event was appended.
    pub accepted: Vec<ProtectedPath>,
    pub refused: Vec<ProtectedPathRefusal>,
    /// The task's active protections after the change.
    pub active: Vec<ProtectedPath>,
    /// The refreshed control model for this task, so both front ends render
    /// the same rows and labels without re-deriving them.
    pub control: TaskProtectedPathsControl,
    /// The server-written sentence describing what the change accepted and
    /// refused. Adapters print or display it; they do not compose their own.
    pub outcome: String,
}

/// The one sentence describing an explicit change. Kept in the engine because
/// acceptance, refusal and directory meaning are engine decisions; a front end
/// may print or display this string but must not rebuild it.
pub fn protected_path_update_outcome(
    action: ProtectedPathAction,
    accepted: &[ProtectedPath],
    refused: &[ProtectedPathRefusal],
) -> String {
    let accepted = accepted
        .iter()
        .map(|path| format!("{} ({})", path.path, path.kind.label()))
        .collect::<Vec<_>>();
    let refused = refused
        .iter()
        .map(ProtectedPathRefusal::message)
        .collect::<Vec<_>>();
    let mut parts = Vec::new();
    if !accepted.is_empty() {
        let verb = match action {
            ProtectedPathAction::Add => "Protected",
            ProtectedPathAction::Remove => "Removed protection for",
        };
        parts.push(format!("{verb} {}.", accepted.join(", ")));
    }
    if !refused.is_empty() {
        parts.push(format!("Refused: {}.", refused.join("; ")));
    }
    if parts.is_empty() {
        "Enter at least one repository-relative path.".to_string()
    } else {
        parts.join(" ")
    }
}

/// Why the direct-user parser did not accept a phrase it recognised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedPathPromptError {
    /// The phrase appeared but no backtick-delimited path followed it.
    NoBacktickPaths,
    /// The phrase was followed by prose the narrow grammar does not read.
    AmbiguousWording,
    /// A backtick span was opened and not closed.
    MalformedBackticks,
    /// A backtick span contained no path.
    EmptyPath,
}

impl Display for ProtectedPathPromptError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBacktickPaths => formatter.write_str(
                "Name each path in backticks after “do not touch” or “do not modify”.",
            ),
            Self::AmbiguousWording => formatter.write_str(
                "Put only the backtick paths after “do not touch” or “do not modify”; the wording was ambiguous, so no path was protected.",
            ),
            Self::MalformedBackticks => formatter.write_str(
                "A backtick-delimited path was not closed, so no path was protected.",
            ),
            Self::EmptyPath => {
                formatter.write_str("A backtick span was empty, so no path was protected.")
            }
        }
    }
}

/// What [`parse_protected_path_prompt`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectedPathPromptOutcome {
    /// Neither phrase appeared outside a code span.
    Absent,
    /// Raw spans in the order the user wrote them. They are canonicalised by
    /// the engine before anything is stored.
    Request { paths: Vec<String> },
    /// A phrase was recognised but the request cannot become a constraint.
    Refused(ProtectedPathPromptError),
}

/// The engine's result for a user prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectedPathPromptResult {
    NoRequest,
    /// The narrow grammar refused the phrase; nothing was written.
    Refused(ProtectedPathPromptError),
    /// Valid spans were canonicalised and recorded. `update.refused` carries
    /// the paths that did not exist or were invalid.
    Recorded(Box<TaskProtectedPathUpdate>),
}

/// Reads the deliberately narrow direct-user grammar.
///
/// Only `do not touch` or `do not modify` followed by backtick-delimited spans
/// in the same sentence is recognised. The phrase must be outside a code span.
/// A phrase with no span, malformed backticks, an empty span, or prose between
/// the phrase and the first span is refused rather than guessed at.
///
/// This is for the prompt a user typed. It must never be called with model
/// output, fetched content, a repository file, or tool output.
pub fn parse_protected_path_prompt(prompt: &str) -> ProtectedPathPromptOutcome {
    let lowered = prompt.to_ascii_lowercase();
    let raw_phrase = PROTECTED_PATH_PROMPT_PHRASES
        .iter()
        .filter_map(|phrase| find_phrase(&lowered, phrase, 0))
        .min();
    if raw_phrase.is_none() {
        return ProtectedPathPromptOutcome::Absent;
    }

    let Some(spans) = backtick_spans(prompt) else {
        return ProtectedPathPromptOutcome::Refused(ProtectedPathPromptError::MalformedBackticks);
    };

    let mut paths = Vec::new();
    let mut search_from = 0;
    let mut saw_request_phrase = false;
    loop {
        let next = PROTECTED_PATH_PROMPT_PHRASES
            .iter()
            .filter_map(|phrase| find_phrase(&lowered, phrase, search_from))
            .min();
        let Some(start) = next else {
            break;
        };
        let phrase = PROTECTED_PATH_PROMPT_PHRASES
            .iter()
            .find(|phrase| lowered[start..].starts_with(**phrase))
            .copied()
            .unwrap_or(PROTECTED_PATH_PROMPT_PHRASES[0]);
        let phrase_end = start + phrase.len();

        // A phrase inside backticks is quoted text, not a request.
        if spans
            .iter()
            .any(|(open, close)| start >= *open && start < *close)
        {
            search_from = phrase_end;
            continue;
        }
        saw_request_phrase = true;

        let sentence_end = sentence_end(prompt, phrase_end);
        // Another recognisable phrase in the same sentence starts its own
        // clause; a phrase inside backticks is quoted text and is not one.
        let next_phrase = PROTECTED_PATH_PROMPT_PHRASES
            .iter()
            .filter_map(|other| find_phrase(&lowered, other, phrase_end))
            .filter(|start| {
                !spans
                    .iter()
                    .any(|(open, close)| *start >= *open && *start < *close)
            })
            .min();
        let clause_end = next_phrase.map_or(sentence_end, |start| start.min(sentence_end));
        let clause = &prompt[phrase_end..clause_end];
        match parse_clause(clause) {
            Ok(mut found) => paths.append(&mut found),
            Err(error) => return ProtectedPathPromptOutcome::Refused(error),
        }

        search_from = clause_end.max(phrase_end);
    }

    if !saw_request_phrase {
        return ProtectedPathPromptOutcome::Absent;
    }
    if paths.is_empty() {
        return ProtectedPathPromptOutcome::Refused(ProtectedPathPromptError::NoBacktickPaths);
    }
    // A repeated path is one protection; keep the user's first position.
    let mut deduped: Vec<String> = Vec::new();
    for path in paths {
        if !deduped.contains(&path) {
            deduped.push(path);
        }
    }
    ProtectedPathPromptOutcome::Request { paths: deduped }
}

/// The engine's prompt-recording operation, shared by the workspace-engine
/// facade and the two direct-user turn entry points.
///
/// The repository check runs before canonicalisation or any event, so a prompt
/// read while looking at repository A cannot write a protection onto a task in
/// repository B. Only [`parse_protected_path_prompt`] touches prose; the caller
/// must pass the prompt the user typed.
pub(crate) fn record_task_protected_paths_from_prompt(
    session_store: &SessionStore,
    path_policy: &PathPolicy,
    repository_root: &Path,
    session_id: &str,
    task_id: &str,
    prompt: &str,
) -> Result<ProtectedPathPromptResult> {
    let Some(session) = session_store.read_session(session_id)? else {
        return Err(ClientError::InvalidInput(format!(
            "Unknown session: {session_id}"
        )));
    };
    let repository_id = repository_id_for_root(repository_root);
    if repository_id != session.repository_id {
        return Err(ClientError::AccessDenied(format!(
            "Session {session_id} does not belong to repository {}",
            repository_root.display()
        )));
    }
    session_store.task(session_id, task_id)?;
    let paths = match parse_protected_path_prompt(prompt) {
        ProtectedPathPromptOutcome::Absent => return Ok(ProtectedPathPromptResult::NoRequest),
        ProtectedPathPromptOutcome::Refused(error) => {
            return Ok(ProtectedPathPromptResult::Refused(error));
        }
        ProtectedPathPromptOutcome::Request { paths } => paths,
    };
    let (accepted, refused) =
        canonicalize_protected_path_requests(path_policy, repository_root, &paths);
    if !accepted.is_empty() {
        session_store.record_task_protected_paths(
            session_id,
            task_id,
            ProtectedPathAction::Add,
            &accepted,
            ProtectedPathSource::Prompt,
        )?;
    }
    let active = session_store.task_protected_paths(session_id, task_id)?;
    let control = TaskProtectedPathsControl::for_task(task_id, &active);
    let outcome = protected_path_update_outcome(ProtectedPathAction::Add, &accepted, &refused);
    Ok(ProtectedPathPromptResult::Recorded(Box::new(
        TaskProtectedPathUpdate {
            task_id: task_id.to_string(),
            action: ProtectedPathAction::Add,
            source: ProtectedPathSource::Prompt,
            accepted,
            refused,
            active: active.paths,
            control,
            outcome,
        },
    )))
}

/// Canonicalises a batch of requested paths, keeping the accepted and refused
/// halves together so a caller can report both.
pub(crate) fn canonicalize_protected_path_requests(
    path_policy: &PathPolicy,
    repository_root: &Path,
    requested: &[String],
) -> (Vec<ProtectedPath>, Vec<ProtectedPathRefusal>) {
    let mut accepted = Vec::new();
    let mut refused = Vec::new();
    for path in requested {
        match canonicalize_protected_path(path_policy, repository_root, path) {
            Ok(protected) => {
                if !accepted.contains(&protected) {
                    accepted.push(protected);
                }
            }
            Err(error) => refused.push(ProtectedPathRefusal {
                requested: path.trim().to_string(),
                error,
            }),
        }
    }
    (accepted, refused)
}

/// Canonicalises one requested path against the task repository.
///
/// Containment is established lexically and through existing symlink
/// components *before* existence is checked, so a path that leaves the
/// repository is `OutsideRepository` whether or not its target exists. Only an
/// existing file or directory strictly inside the canonical root is accepted.
/// The returned path is stored verbatim as the durable repository-relative
/// name.
pub fn canonicalize_protected_path(
    path_policy: &PathPolicy,
    repository_root: impl AsRef<Path>,
    requested: &str,
) -> std::result::Result<ProtectedPath, ProtectedPathError> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(ProtectedPathError::Empty);
    }
    if Path::new(requested).is_absolute() {
        return Err(ProtectedPathError::Absolute);
    }

    let root = path_policy
        .canonical_root(repository_root)
        .map_err(map_root_error)?;
    // `resolve_for_write` is the existing containment check: it rejects a
    // lexical escape even when the target does not exist, and resolves symlink
    // components that do.
    let resolved = path_policy
        .resolve_for_write(&root, requested)
        .map_err(map_write_resolve_error)?;
    let metadata = fs::metadata(&resolved.absolute_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ProtectedPathError::Missing
        } else {
            ProtectedPathError::Invalid {
                message: format!("The protected path could not be read: {error}"),
            }
        }
    })?;
    let kind = if metadata.is_file() {
        ProtectedPathKind::File
    } else if metadata.is_dir() {
        ProtectedPathKind::Directory
    } else {
        return Err(ProtectedPathError::NotFileOrDirectory);
    };
    if resolved.relative_path.is_empty() {
        return Err(ProtectedPathError::RepositoryRoot);
    }
    if !is_valid_repository_relative_path(&resolved.relative_path) {
        return Err(ProtectedPathError::Invalid {
            message: format!(
                "The protected path is not a normal repository-relative path: {}",
                resolved.relative_path
            ),
        });
    }
    Ok(ProtectedPath {
        path: resolved.relative_path,
        kind,
    })
}

/// Resolves a removal request against the task's active canonical paths.
///
/// Removal does not establish a protection, so it must not consult the
/// filesystem: the protected file or directory may have been deleted or renamed
/// since it was added. The request must name an active canonical path exactly.
/// A non-canonical, outside-repository or non-active value is refused so a
/// typo cannot silently remove a different protection.
pub fn resolve_active_protected_path(
    active: &TaskProtectedPaths,
    requested: &str,
) -> std::result::Result<ProtectedPath, ProtectedPathError> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(ProtectedPathError::Empty);
    }
    if Path::new(requested).is_absolute() {
        return Err(ProtectedPathError::Absolute);
    }
    if requested.split('/').any(|component| component == "..") {
        return Err(ProtectedPathError::OutsideRepository);
    }
    if !is_valid_repository_relative_path(requested) {
        return Err(ProtectedPathError::Invalid {
            message:
                "The protected path is not canonical; choose it from the task's protected paths."
                    .to_string(),
        });
    }
    active
        .paths
        .iter()
        .find(|protected| protected.path == requested)
        .cloned()
        .ok_or(ProtectedPathError::NotProtected)
}

fn map_root_error(error: ClientError) -> ProtectedPathError {
    match error {
        ClientError::AccessDenied(_) => ProtectedPathError::RepositoryNotAllowed,
        ClientError::Io(message) => ProtectedPathError::Invalid {
            message: format!("The task repository could not be read: {message}"),
        },
        other => ProtectedPathError::Invalid {
            message: other.to_string(),
        },
    }
}

fn map_write_resolve_error(error: ClientError) -> ProtectedPathError {
    match error {
        ClientError::AccessDenied(message) if message.contains("outside") => {
            ProtectedPathError::OutsideRepository
        }
        ClientError::AccessDenied(_) => ProtectedPathError::RepositoryNotAllowed,
        ClientError::Io(message) if message.to_lowercase().contains("no such file") => {
            ProtectedPathError::Missing
        }
        other => ProtectedPathError::Invalid {
            message: other.to_string(),
        },
    }
}

fn is_valid_repository_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

/// A phrase occurrence at or after `from`, with word boundaries so `undo not
/// touch` and `do not touching` are not requests.
fn find_phrase(lowered: &str, phrase: &str, from: usize) -> Option<usize> {
    let mut search = from;
    while let Some(relative) = lowered.get(search..)?.find(phrase) {
        let start = search + relative;
        let before_ok = start == 0
            || lowered[..start]
                .chars()
                .next_back()
                .is_none_or(|character| !is_word_character(character));
        let end = start + phrase.len();
        let after_ok = end >= lowered.len()
            || lowered[end..]
                .chars()
                .next()
                .is_none_or(|character| !is_word_character(character));
        if before_ok && after_ok {
            return Some(start);
        }
        search = start + 1;
    }
    None
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Byte ranges of balanced backtick spans. `None` means the text has an
/// unmatched backtick, so no span can be trusted.
fn backtick_spans(text: &str) -> Option<Vec<(usize, usize)>> {
    let mut spans = Vec::new();
    let mut open = None;
    for (index, character) in text.char_indices() {
        if character != '`' {
            continue;
        }
        match open.take() {
            Some(start) => spans.push((start, index)),
            None => open = Some(index),
        }
    }
    open.is_none().then_some(spans)
}

/// The end of the sentence containing a phrase, ignoring terminators inside a
/// code span. A newline ends it even without punctuation.
fn sentence_end(text: &str, from: usize) -> usize {
    let mut in_backticks = false;
    for (index, character) in text[from..].char_indices() {
        if character == '`' {
            in_backticks = !in_backticks;
            continue;
        }
        if in_backticks {
            continue;
        }
        if character == '\n' {
            return from + index;
        }
        if matches!(character, '.' | '!' | '?') {
            let after = from + index + character.len_utf8();
            if text[after..].chars().next().is_none_or(char::is_whitespace) {
                return from + index;
            }
        }
    }
    text.len()
}

fn parse_clause(clause: &str) -> std::result::Result<Vec<String>, ProtectedPathPromptError> {
    let mut paths = Vec::new();
    let mut cursor = 0;
    while let Some(relative_open) = clause[cursor..].find('`') {
        let open = cursor + relative_open;
        let between = &clause[cursor..open];
        let allowed = if paths.is_empty() {
            is_leading_separator(between)
        } else {
            is_between_separator(between)
        };
        if !allowed {
            return Err(ProtectedPathPromptError::AmbiguousWording);
        }
        let Some(relative_close) = clause[open + 1..].find('`') else {
            return Err(ProtectedPathPromptError::MalformedBackticks);
        };
        let close = open + 1 + relative_close;
        let path = clause[open + 1..close].trim();
        if path.is_empty() {
            return Err(ProtectedPathPromptError::EmptyPath);
        }
        paths.push(path.to_string());
        cursor = close + 1;
    }
    if paths.is_empty() {
        return Err(ProtectedPathPromptError::NoBacktickPaths);
    }
    Ok(paths)
}

fn is_leading_separator(text: &str) -> bool {
    separator_tokens(text, &[])
}

fn is_between_separator(text: &str) -> bool {
    separator_tokens(text, &["and", "or", "nor", "then"])
}

/// Separator text may contain whitespace and the punctuation a person writes
/// between paths, plus the connectors the grammar names. Anything else is prose
/// the parser refuses rather than guesses at.
fn separator_tokens(text: &str, connectors: &[&str]) -> bool {
    let cleaned: String = text
        .chars()
        .filter(|character| {
            !character.is_whitespace() && !matches!(character, ',' | ';' | ':' | '-' | '+' | '&')
        })
        .collect::<String>()
        .to_ascii_lowercase();
    if cleaned.is_empty() {
        return true;
    }
    if connectors.is_empty() {
        return false;
    }
    let mut rest = cleaned.as_str();
    while !rest.is_empty() {
        let Some(connector) = connectors
            .iter()
            .find(|connector| rest.starts_with(**connector))
        else {
            return false;
        };
        rest = &rest[connector.len()..];
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ModelProviderConfig};
    use crate::error::ClientError;
    use crate::model::{MockModelAdapter, ToolCall};
    use crate::plan::TaskPlan;
    use crate::session::{SessionStore, TaskStatus};
    use crate::verification::{VerificationEvent, VerificationOutcome};
    use crate::workspace_engine::WorkspaceEngine;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "damaian-protected-paths-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn repository(name: &str) -> PathBuf {
        let root = temp_dir(name);
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/a.rs"), "pub fn a() {}\n").unwrap();
        fs::write(root.join("src/nested/b.rs"), "pub fn b() {}\n").unwrap();
        fs::write(root.join("docs.md"), "# docs\n").unwrap();
        root
    }

    fn test_engine(root: &Path) -> WorkspaceEngine {
        WorkspaceEngine::new(Config {
            data_dir: root.join(".damaian"),
            enable_index_watcher: false,
            ..Config::default()
        })
    }

    fn native_provider() -> ModelProviderConfig {
        ModelProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            base_url: String::new(),
            api_key_env: String::new(),
            models: Vec::new(),
            supports_native_tools: true,
            max_output_tokens: None,
            context_token_budget: None,
            context_window: None,
            provider_reports_usage: true,
            price_per_million_input_tokens: None,
            price_per_million_output_tokens: None,
            price_per_million_cached_input_tokens: None,
            supports_explicit_cache_breakpoints: false,
        }
    }

    fn native_engine(root: &Path) -> WorkspaceEngine {
        let mut config = Config {
            data_dir: root.join(".damaian"),
            enable_index_watcher: false,
            ..Config::default()
        };
        config.model_providers.push(native_provider());
        WorkspaceEngine::new(config)
    }

    fn tool_call(id: &str, name: &str, arguments_json: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments_json: arguments_json.to_string(),
        }
    }

    fn calls_then_answer(calls: Vec<ToolCall>) -> MockModelAdapter {
        MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Understood.".to_string()],
            vec![calls, Vec::new()],
        )
    }

    fn assistant_refusals(store: &SessionStore, session_id: &str) -> Vec<String> {
        store
            .read_messages(session_id)
            .unwrap()
            .into_iter()
            .filter(|message| message.kind.as_deref() == Some("protected_path_refusal"))
            .map(|message| message.content)
            .collect()
    }

    fn stored_patch_files(root: &Path) -> usize {
        files_under(&root.join(".damaian").join("patches"))
    }

    fn files_under(dir: &Path) -> usize {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| {
                let path = entry.path();
                if path.is_dir() { files_under(&path) } else { 1 }
            })
            .sum()
    }

    #[test]
    fn a_direct_prompt_records_paths_and_a_malformed_one_refuses_without_protection() {
        let root = repository("prompt-boundary");
        let engine = native_engine(&root);
        let mut on_token = |_token: &str| {};

        let mut adapter = MockModelAdapter::new("Done.");
        let first = engine
            .chat_orchestrator
            .ask(
                &root,
                "Do not touch `src/a.rs` and `src/nested`.\nUpdate the rest.",
                &[],
                &mut adapter,
                &mut on_token,
            )
            .unwrap();
        let active = engine
            .task_protected_paths(&first.session.id, &first.task.id)
            .unwrap();
        assert_eq!(
            active.path_list(),
            vec!["src/a.rs".to_string(), "src/nested".to_string()]
        );
        assert!(active.covers("src/nested/b.rs"));
        assert!(
            engine
                .session_store
                .read_task_protected_path_events(&first.session.id)
                .unwrap()
                .iter()
                .all(|event| event.source == ProtectedPathSource::Prompt)
        );

        let mut refused = MockModelAdapter::new("Done.");
        let second = engine
            .chat_orchestrator
            .ask(
                &root,
                "do not touch the unrelated files `src/a.rs`",
                &[],
                &mut refused,
                &mut on_token,
            )
            .unwrap();
        assert!(
            engine
                .task_protected_paths(&second.session.id, &second.task.id)
                .unwrap()
                .is_empty()
        );
        assert!(
            engine
                .session_store
                .read_task_protected_path_events(&second.session.id)
                .unwrap()
                .is_empty(),
            "refused wording must write no protection event"
        );
        let refusals = assistant_refusals(&engine.session_store, &second.session.id);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].contains("no path was protected"));

        let mut invalid = MockModelAdapter::new("Done.");
        let third = engine
            .chat_orchestrator
            .ask(
                &root,
                "Do not touch `src/missing.rs`.",
                &[],
                &mut invalid,
                &mut on_token,
            )
            .unwrap();
        assert!(
            engine
                .task_protected_paths(&third.session.id, &third.task.id)
                .unwrap()
                .is_empty()
        );
        let refusals = assistant_refusals(&engine.session_store, &third.session.id);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(
            refusals[0].contains("does not exist") && refusals[0].contains("src/missing.rs"),
            "{refusals:?}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_protected_directory_descendant_refuses_the_whole_proposal_before_storage() {
        let root = repository("propose-directory");
        let engine = native_engine(&root);
        let mut on_token = |_token: &str| {};
        let mut adapter = calls_then_answer(vec![tool_call(
            "call_1",
            "propose_patch",
            r##"{"summary":"Change two files","files":[
                {"path":"src/nested/new.txt","content":"new\n"},
                {"path":"docs.md","content":"# changed\n"}
            ]}"##,
        )]);

        let result = engine
            .chat_orchestrator
            .ask(
                &root,
                "Do not touch `src/nested`.\nUpdate the other files.",
                &[],
                &mut adapter,
                &mut on_token,
            )
            .unwrap();

        assert!(
            result.patch_proposal.is_none(),
            "no review card may be shown"
        );
        assert_eq!(stored_patch_files(&root), 0, "no PatchStore record");
        let results = engine
            .session_store
            .read_messages(&result.session.id)
            .unwrap()
            .into_iter()
            .filter(|message| message.role == "tool")
            .map(|message| message.content)
            .collect::<Vec<_>>();
        assert!(
            results
                .iter()
                .any(|result| result.contains("Patch would modify protected task path: src/nested")),
            "{results:?}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn edit_file_refuses_a_protected_file_before_storage() {
        let root = repository("edit-file");
        let engine = native_engine(&root);
        let mut on_token = |_token: &str| {};
        let mut adapter = calls_then_answer(vec![tool_call(
            "call_1",
            "edit_file",
            r#"{"summary":"Change a protected file","edits":[
                {"path":"src/a.rs","old_text":"pub fn a() {}\n","new_text":"pub fn a() { let changed = true; }\n"}
            ]}"#,
        )]);

        let result = engine
            .chat_orchestrator
            .ask(
                &root,
                "Do not touch `src/a.rs`.\nChange the other files.",
                &[],
                &mut adapter,
                &mut on_token,
            )
            .unwrap();

        assert!(result.patch_proposal.is_none());
        assert_eq!(stored_patch_files(&root), 0);
        let results = engine
            .session_store
            .read_messages(&result.session.id)
            .unwrap()
            .into_iter()
            .filter(|message| message.role == "tool")
            .map(|message| message.content)
            .collect::<Vec<_>>();
        assert!(
            results.iter().any(|result| {
                result.contains("Patch would modify protected task path: src/a.rs")
            }),
            "{results:?}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_legacy_edit_flow_refuses_a_protected_file_before_storage() {
        let root = repository("legacy-edit");
        let engine = native_engine(&root);
        let envelope = "DAMAIAN_EDIT_V1\nSUMMARY: Change a protected file\nFILE: src/a.rs\nSTATUS: modified\nCONTENT:\npub fn a() { let changed = true; }\nEND_FILE\nEND_PATCH\n";
        let mut adapter = MockModelAdapter::new(envelope);

        let error = engine
            .edit_orchestrator
            .propose_edit(
                &root,
                "Do not touch `src/a.rs`.\nUpdate it.",
                &[],
                None,
                &mut adapter,
            )
            .unwrap_err();

        match error {
            ClientError::AccessDenied(message) => {
                assert!(message.contains("src/a.rs"), "{message}");
                assert!(message.contains("protected task path"), "{message}");
            }
            other => panic!("expected a protected-path refusal, got {other:?}"),
        }
        let system = &adapter.requests[0].messages[0].content;
        assert!(
            system.contains("The user excluded these task paths") && system.contains("src/a.rs"),
            "the edit model must receive the reminder: {system}"
        );
        assert_eq!(stored_patch_files(&root), 0, "no PatchStore record");
        assert_eq!(
            fs::read_to_string(root.join("src/a.rs")).unwrap(),
            "pub fn a() {}\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// Spec 98 §2.6: rejecting one proposed file is durable review evidence,
    /// never a permanent protection. The same file can be proposed again, and
    /// the rejected decision is still readable.
    #[test]
    fn rejecting_a_proposed_file_does_not_protect_it() {
        let root = repository("rejection-not-protection");
        let engine = native_engine(&root);
        let envelope = "DAMAIAN_EDIT_V1\nSUMMARY: Change two files\nFILE: src/a.rs\nSTATUS: modified\nCONTENT:\npub fn a() { let changed = true; }\nEND_FILE\nFILE: src/nested/b.rs\nSTATUS: modified\nCONTENT:\npub fn b() { let changed = true; }\nEND_FILE\nEND_PATCH\n";
        let mut adapter = MockModelAdapter::new(envelope);
        let first = engine
            .edit_orchestrator
            .propose_edit(&root, "Update both files.", &[], None, &mut adapter)
            .unwrap();

        engine
            .edit_orchestrator
            .reject_stored_patch_files(&first.patch.id, &["src/a.rs".to_string()], "tester")
            .unwrap();

        let active = engine
            .task_protected_paths(&first.session.id, &first.task.id)
            .unwrap();
        assert!(
            active.is_empty(),
            "a rejection must not become a protection: {active:?}"
        );
        assert!(
            engine
                .session_store
                .read_task_protected_path_events(&first.session.id)
                .unwrap()
                .is_empty(),
            "a rejection must write no protection event"
        );
        let decisions = engine
            .session_store
            .read_patch_file_decisions(&first.session.id, &first.task.id)
            .unwrap();
        assert!(
            decisions.iter().any(|decision| {
                decision.patch_id == first.patch.id
                    && decision.paths == vec!["src/a.rs".to_string()]
            }),
            "the rejected decision stays durable review evidence: {decisions:?}"
        );

        let envelope = "DAMAIAN_EDIT_V1\nSUMMARY: Change the same file again\nFILE: src/a.rs\nSTATUS: modified\nCONTENT:\npub fn a() { let changed = true; }\nEND_FILE\nEND_PATCH\n";
        let mut second_adapter = MockModelAdapter::new(envelope);
        let second = engine
            .edit_orchestrator
            .propose_edit(
                &root,
                "Change the same file again.",
                &[],
                None,
                &mut second_adapter,
            )
            .unwrap();
        assert!(
            second
                .patch
                .files
                .iter()
                .any(|file| file.path == "src/a.rs"),
            "a rejected path must remain proposable: {:?}",
            second.patch.files
        );

        let _ = fs::remove_dir_all(root);
    }

    fn proposed_patch(
        engine: &WorkspaceEngine,
        root: &Path,
        prompt: &str,
        arguments_json: &str,
    ) -> (String, String, String) {
        let mut on_token = |_token: &str| {};
        let mut adapter =
            calls_then_answer(vec![tool_call("call_1", "propose_patch", arguments_json)]);
        let result = engine
            .chat_orchestrator
            .ask(root, prompt, &[], &mut adapter, &mut on_token)
            .unwrap();
        let patch = result.patch_proposal.expect("the patch should be proposed");
        (result.session.id, result.task.id, patch.patch_id)
    }

    #[test]
    fn an_apply_race_is_refused_without_a_write_checkpoint_decision_or_verification_due() {
        let root = repository("apply-race");
        let engine = native_engine(&root);
        let (session_id, task_id, patch_id) = proposed_patch(
            &engine,
            &root,
            "Change the docs.",
            r##"{"summary":"Change docs","files":[{"path":"docs.md","content":"# changed\n"}]}"##,
        );
        engine
            .add_task_protected_paths(&root, &session_id, &task_id, &["docs.md".to_string()])
            .unwrap();

        let checkpoints_before = files_under(&root.join(".damaian").join("checkpoints"));
        let error = engine
            .edit_orchestrator
            .apply_stored_patch(&root, &patch_id, None, None, "tester", false)
            .unwrap_err();

        match error {
            ClientError::AccessDenied(message) => {
                assert!(message.contains("docs.md"), "{message}");
            }
            other => panic!("expected a protected-path refusal, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(root.join("docs.md")).unwrap(),
            "# docs\n",
            "nothing may be written"
        );
        assert_eq!(
            files_under(&root.join(".damaian").join("checkpoints")),
            checkpoints_before,
            "no checkpoint may be taken"
        );
        assert!(
            engine
                .session_store
                .read_patch_file_decisions(&session_id, &task_id)
                .unwrap()
                .is_empty(),
            "no patch decision may be recorded"
        );
        assert!(
            engine
                .session_store
                .read_verification_events(&session_id, &[task_id.as_str()])
                .unwrap()
                .is_empty(),
            "no verification_due may be recorded"
        );
        let log = fs::read_to_string(session_log(&root.join(".damaian"), &session_id)).unwrap();
        assert!(!log.contains("\"eventType\":\"apply_patch\""), "{log}");
        assert!(!log.contains("\"eventType\":\"verification_due\""), "{log}");
        let refusals = assistant_refusals(&engine.session_store, &session_id);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(
            refusals[0].contains("docs.md") && refusals[0].contains("protected task path"),
            "{refusals:?}"
        );
        assert_eq!(
            engine
                .session_store
                .read_patch_file_decisions(&session_id, &task_id)
                .unwrap()
                .len(),
            0,
            "the apply refusal is not a patch-file rejection decision"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_selected_unprotected_partial_apply_ignores_an_unselected_protected_file() {
        let root = repository("partial-apply");
        fs::write(root.join("src/open.rs"), "pub fn open() {}\n").unwrap();
        let engine = native_engine(&root);
        let (session_id, task_id, patch_id) = proposed_patch(
            &engine,
            &root,
            "Change both files.",
            r#"{"summary":"Change two files","files":[
                {"path":"src/a.rs","content":"pub fn a() { let changed = true; }\n"},
                {"path":"src/open.rs","content":"pub fn open() { let changed = true; }\n"}
            ]}"#,
        );
        engine
            .add_task_protected_paths(&root, &session_id, &task_id, &["src/a.rs".to_string()])
            .unwrap();

        let selected = vec!["src/open.rs".to_string()];
        let applied = engine
            .edit_orchestrator
            .apply_stored_patch(&root, &patch_id, Some(&selected), None, "tester", false)
            .unwrap();
        assert_eq!(applied.applied_files, vec!["src/open.rs".to_string()]);
        assert_eq!(
            fs::read_to_string(root.join("src/open.rs")).unwrap(),
            "pub fn open() { let changed = true; }\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("src/a.rs")).unwrap(),
            "pub fn a() {}\n",
            "the unselected protected file must not be written"
        );

        let protected = vec!["src/a.rs".to_string()];
        let error = engine
            .edit_orchestrator
            .apply_stored_patch(&root, &patch_id, Some(&protected), None, "tester", false)
            .unwrap_err();
        assert!(
            matches!(error, ClientError::AccessDenied(ref message) if message.contains("src/a.rs")),
            "a selected protected file must be refused: {error:?}"
        );

        let _ = fs::remove_dir_all(root);
    }

    fn session_and_task(
        store: &SessionStore,
        name: &str,
    ) -> (crate::session::Session, crate::session::Task) {
        let session = store.create_session("repo_test", name).unwrap();
        let task = store
            .create_task(&session.id, "Do the work.", "provider", "model")
            .unwrap();
        (session, task)
    }

    fn engine_session_and_task(
        engine: &WorkspaceEngine,
        root: &Path,
        name: &str,
    ) -> (crate::session::Session, crate::session::Task) {
        let session = engine
            .session_store
            .create_session(&crate::hash::repository_id_for_root(root), name)
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "Do the work.", "provider", "model")
            .unwrap();
        (session, task)
    }

    fn session_log(data_dir: &Path, session_id: &str) -> PathBuf {
        data_dir
            .join("sessions")
            .join(format!("{session_id}.jsonl"))
    }

    fn pending_patch(
        engine: &WorkspaceEngine,
        root: &Path,
        name: &str,
        files: &[&str],
    ) -> (String, String, String) {
        let session = engine
            .session_store
            .create_session(&repository_id_for_root(root), name)
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "Edit the files.", "mock", "m")
            .unwrap();
        let patch = crate::patch_engine::ProposedPatch {
            id: format!("patch_{name}"),
            session_id: session.id.clone(),
            origin_session_id: String::new(),
            task_id: Some(task.id.clone()),
            summary: "Waiting for review".to_string(),
            status: "pending".to_string(),
            created_at_ms: 1,
            repair_scope: None,
            files: files
                .iter()
                .map(|path| crate::patch_engine::ProposedFilePatch {
                    path: (*path).to_string(),
                    status: "modified".to_string(),
                    base_hash: None,
                    new_hash: String::new(),
                    diff: String::new(),
                    hunks: Vec::new(),
                    new_content: "new\n".to_string(),
                })
                .collect(),
        };
        crate::edit::PatchStore::new(root.join(".damaian"))
            .save(&patch)
            .unwrap();
        engine
            .session_store
            .await_approval(
                &task,
                &crate::session::PendingApprovalRef {
                    kind: "patch".to_string(),
                    proposal_id: patch.id.clone(),
                },
            )
            .unwrap();
        (session.id, task.id, patch.id)
    }

    fn reattach(
        engine: &WorkspaceEngine,
        root: &Path,
        session_id: &str,
    ) -> Vec<crate::recovery::ReattachedApproval> {
        let data_dir = root.join(".damaian");
        crate::recovery::reattach_pending_approvals(
            &engine.session_store,
            &crate::audit::AuditLog::new(&data_dir, true, Default::default()),
            &crate::validation::CommandStore::new(&data_dir),
            &crate::edit::PatchStore::new(&data_dir),
            &crate::chat::PausedTurns::new(&data_dir),
            session_id,
        )
        .unwrap()
    }

    #[test]
    fn the_parser_accepts_exact_phrases_and_multiple_spans() {
        assert_eq!(
            parse_protected_path_prompt("do not touch `src/a.rs` and `src/b.rs`."),
            ProtectedPathPromptOutcome::Request {
                paths: vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
            }
        );
        assert_eq!(
            parse_protected_path_prompt("Do not modify: `docs/`, `README.md`."),
            ProtectedPathPromptOutcome::Request {
                paths: vec!["docs/".to_string(), "README.md".to_string()]
            }
        );
        // Two separate sentences are two clauses in one request.
        assert_eq!(
            parse_protected_path_prompt(
                "do not touch `src/a.rs`. Also do not modify `src/b.rs`, please."
            ),
            ProtectedPathPromptOutcome::Request {
                paths: vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
            }
        );
        // Two phrases in one sentence are two clauses too.
        assert_eq!(
            parse_protected_path_prompt("do not touch `src/a.rs` and do not modify `src/b.rs`."),
            ProtectedPathPromptOutcome::Request {
                paths: vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
            }
        );
    }

    #[test]
    fn the_parser_refuses_ambiguous_and_malformed_wording() {
        assert_eq!(
            parse_protected_path_prompt("do not touch the unrelated files `src/a.rs`"),
            ProtectedPathPromptOutcome::Refused(ProtectedPathPromptError::AmbiguousWording)
        );
        assert_eq!(
            parse_protected_path_prompt("do not touch anything, please."),
            ProtectedPathPromptOutcome::Refused(ProtectedPathPromptError::NoBacktickPaths)
        );
        assert_eq!(
            parse_protected_path_prompt("do not touch `src/a.rs"),
            ProtectedPathPromptOutcome::Refused(ProtectedPathPromptError::MalformedBackticks)
        );
        assert_eq!(
            parse_protected_path_prompt("do not touch ``"),
            ProtectedPathPromptOutcome::Refused(ProtectedPathPromptError::EmptyPath)
        );
        // Quoted instructions are not a request.
        assert_eq!(
            parse_protected_path_prompt("The guide's `do not touch src/a.rs` line is an example."),
            ProtectedPathPromptOutcome::Absent
        );
        // A phrase with no path at all is absent only when the phrase itself is
        // not there; this one is a refusal.
        assert_eq!(
            parse_protected_path_prompt("Do not touch `src/a.rs` or also `src/b.rs`"),
            ProtectedPathPromptOutcome::Refused(ProtectedPathPromptError::AmbiguousWording)
        );
    }

    #[test]
    fn canonicalisation_stores_a_file_and_a_directory_and_directories_cover_descendants() {
        let root = repository("canonicalise-ok");
        let policy = PathPolicy::unrestricted();

        let file =
            canonicalize_protected_path(&policy, &root, "./src/a.rs").expect("existing file");
        assert_eq!(file.path, "src/a.rs");
        assert_eq!(file.kind, ProtectedPathKind::File);

        let directory =
            canonicalize_protected_path(&policy, &root, "src/nested/").expect("existing dir");
        assert_eq!(directory.path, "src/nested");
        assert_eq!(directory.kind, ProtectedPathKind::Directory);

        let active = TaskProtectedPaths {
            task_id: "task_1".to_string(),
            paths: vec![file.clone(), directory.clone()],
        };
        assert!(active.covers("src/a.rs"));
        assert!(active.covers("src/nested/b.rs"));
        assert!(active.covers("src/nested"));
        assert!(!active.covers("src/nestedness/b.rs"));
        assert_eq!(
            active
                .covering_path("src/nested/b.rs")
                .map(|path| &path.path),
            Some(&"src/nested".to_string())
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn canonicalisation_refuses_missing_absolute_outside_and_root_paths() {
        let root = repository("canonicalise-bad");
        let policy = PathPolicy::unrestricted();
        let outside = temp_dir("outside-file");
        fs::write(&outside, "outside\n").unwrap();
        let outside_request = format!(
            "../{}",
            outside
                .file_name()
                .expect("temp path has a name")
                .to_string_lossy()
        );

        assert_eq!(
            canonicalize_protected_path(&policy, &root, "src/missing.rs"),
            Err(ProtectedPathError::Missing)
        );
        assert_eq!(
            canonicalize_protected_path(
                &policy,
                &root,
                &root.join("src/a.rs").display().to_string()
            ),
            Err(ProtectedPathError::Absolute)
        );
        assert_eq!(
            canonicalize_protected_path(&policy, &root, &outside_request),
            Err(ProtectedPathError::OutsideRepository)
        );
        // Containment comes before existence: a missing target outside the
        // repository is outside, not merely missing.
        let missing_outside = format!("../missing-outside-{}.md", now_millis());
        assert_eq!(
            canonicalize_protected_path(&policy, &root, &missing_outside),
            Err(ProtectedPathError::OutsideRepository)
        );
        assert_eq!(
            canonicalize_protected_path(&policy, &root, "   "),
            Err(ProtectedPathError::Empty)
        );
        assert_eq!(
            canonicalize_protected_path(&policy, &root, "."),
            Err(ProtectedPathError::RepositoryRoot)
        );

        let link = root.join("outside-link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert_eq!(
            canonicalize_protected_path(&policy, &root, "outside-link"),
            Err(ProtectedPathError::OutsideRepository)
        );

        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_add_and_remove_record_source_and_timestamps() {
        let root = repository("explicit");
        let data_dir = root.join(".damaian");
        let engine = test_engine(&root);
        let (session, task) = engine_session_and_task(&engine, &root, "Explicit");

        let added = engine
            .add_task_protected_paths(
                &root,
                &session.id,
                &task.id,
                &["src/a.rs".to_string(), "src/nested".to_string()],
            )
            .unwrap();
        assert_eq!(added.action, ProtectedPathAction::Add);
        assert_eq!(added.source, ProtectedPathSource::ExplicitControl);
        assert_eq!(added.accepted.len(), 2);
        assert!(added.refused.is_empty());
        assert_eq!(added.active.len(), 2);

        let resolved = engine.task_protected_paths(&session.id, &task.id).unwrap();
        assert!(resolved.covers("src/nested/b.rs"));

        let removed = engine
            .remove_task_protected_paths(&root, &session.id, &task.id, &["src/nested".to_string()])
            .unwrap();
        assert_eq!(removed.action, ProtectedPathAction::Remove);
        assert_eq!(removed.source, ProtectedPathSource::ExplicitControl);
        assert_eq!(removed.accepted.len(), 1);
        assert_eq!(
            removed.active,
            vec![ProtectedPath {
                path: "src/a.rs".to_string(),
                kind: ProtectedPathKind::File,
            }]
        );

        let raw = fs::read_to_string(session_log(&data_dir, &session.id)).unwrap();
        assert_eq!(
            raw.matches("\"eventType\":\"task_protected_paths\"")
                .count(),
            2
        );
        assert!(raw.contains("\"source\":\"explicit_control\""));
        assert!(raw.contains("\"createdAtMs\":"));
        assert!(!raw.contains("Do not touch"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_session_repository_must_match_the_supplied_root() {
        let root_a = repository("repo-a");
        let root_b = repository("repo-b");
        let engine = test_engine(&root_a);
        let (session, task) = engine_session_and_task(&engine, &root_a, "Cross repository");

        for error in [
            engine
                .add_task_protected_paths(&root_b, &session.id, &task.id, &["src/a.rs".to_string()])
                .unwrap_err(),
            engine
                .remove_task_protected_paths(
                    &root_b,
                    &session.id,
                    &task.id,
                    &["src/a.rs".to_string()],
                )
                .unwrap_err(),
            engine
                .record_task_protected_paths_from_prompt(
                    &root_b,
                    &session.id,
                    &task.id,
                    "do not touch `src/a.rs`",
                )
                .unwrap_err(),
        ] {
            assert!(
                matches!(error, ClientError::AccessDenied(_)),
                "a repository mismatch must be an access refusal: {error:?}"
            );
        }

        assert!(
            engine
                .session_store
                .read_task_protected_path_events(&session.id)
                .unwrap()
                .is_empty(),
            "a refused cross-repository request must append no event"
        );
        assert!(
            engine
                .task_protected_paths(&session.id, &task.id)
                .unwrap()
                .is_empty()
        );

        let _ = fs::remove_dir_all(root_a);
        let _ = fs::remove_dir_all(root_b);
    }

    #[test]
    fn an_explicit_remove_works_after_the_protected_paths_are_deleted_or_renamed() {
        let root = repository("remove-missing");
        let data_dir = root.join(".damaian");
        let engine = test_engine(&root);
        let (session, task) = engine_session_and_task(&engine, &root, "Remove missing");

        let added = engine
            .add_task_protected_paths(
                &root,
                &session.id,
                &task.id,
                &["src/a.rs".to_string(), "src/nested".to_string()],
            )
            .unwrap();
        assert_eq!(added.active.len(), 2);

        fs::remove_file(root.join("src/a.rs")).unwrap();
        fs::rename(root.join("src/nested"), root.join("src/nested-renamed")).unwrap();

        // A restart still sees both protections even though neither path exists.
        let restarted_store = SessionStore::new(&data_dir);
        assert_eq!(
            restarted_store
                .task_protected_paths(&session.id, &task.id)
                .unwrap()
                .paths
                .len(),
            2
        );

        let restarted = test_engine(&root);
        let removed = restarted
            .remove_task_protected_paths(
                &root,
                &session.id,
                &task.id,
                &["src/a.rs".to_string(), "src/nested".to_string()],
            )
            .unwrap();
        assert_eq!(removed.accepted.len(), 2, "{removed:?}");
        assert!(removed.refused.is_empty(), "{removed:?}");
        assert!(removed.active.is_empty(), "{removed:?}");

        let fresh = SessionStore::new(&data_dir);
        assert!(
            fresh
                .task_protected_paths(&session.id, &task.id)
                .unwrap()
                .is_empty()
        );
        let raw = fs::read_to_string(session_log(&data_dir, &session.id)).unwrap();
        assert_eq!(
            raw.matches("\"eventType\":\"task_protected_paths\"")
                .count(),
            2,
            "the removal is one more appended event"
        );
        assert!(raw.contains("\"action\":\"remove\""));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_removal_refuses_noncanonical_outside_absolute_and_nonactive_paths() {
        let root = repository("remove-refusals");
        let engine = test_engine(&root);
        let (session, task) = engine_session_and_task(&engine, &root, "Remove refusals");
        engine
            .add_task_protected_paths(&root, &session.id, &task.id, &["src/a.rs".to_string()])
            .unwrap();

        let update = engine
            .remove_task_protected_paths(
                &root,
                &session.id,
                &task.id,
                &[
                    "./src/a.rs".to_string(),
                    "../outside.md".to_string(),
                    root.join("src/a.rs").display().to_string(),
                    "src/not-protected.rs".to_string(),
                ],
            )
            .unwrap();
        assert!(update.accepted.is_empty(), "{update:?}");
        assert_eq!(update.refused.len(), 4, "{update:?}");
        assert_eq!(update.refused[0].error, ProtectedPathError::Invalid {
            message: "The protected path is not canonical; choose it from the task's protected paths."
                .to_string(),
        });
        assert_eq!(
            update.refused[1].error,
            ProtectedPathError::OutsideRepository
        );
        assert_eq!(update.refused[2].error, ProtectedPathError::Absolute);
        assert_eq!(update.refused[3].error, ProtectedPathError::NotProtected);
        assert_eq!(
            update.active,
            vec![ProtectedPath {
                path: "src/a.rs".to_string(),
                kind: ProtectedPathKind::File,
            }]
        );

        // The exact canonical value still removes.
        let removed = engine
            .remove_task_protected_paths(&root, &session.id, &task.id, &["src/a.rs".to_string()])
            .unwrap();
        assert_eq!(removed.accepted.len(), 1);
        assert!(removed.active.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn add_replace_replay_survives_a_fresh_store_and_a_new_task_does_not_inherit() {
        let root = repository("replay");
        let data_dir = root.join(".damaian");
        let store = SessionStore::new(&data_dir);
        let (session, task) = session_and_task(&store, "Replay");

        store
            .record_task_protected_paths(
                &session.id,
                &task.id,
                ProtectedPathAction::Add,
                &[
                    ProtectedPath {
                        path: "src/a.rs".to_string(),
                        kind: ProtectedPathKind::File,
                    },
                    ProtectedPath {
                        path: "src/nested".to_string(),
                        kind: ProtectedPathKind::Directory,
                    },
                ],
                ProtectedPathSource::Prompt,
            )
            .unwrap();
        store
            .record_task_protected_paths(
                &session.id,
                &task.id,
                ProtectedPathAction::Remove,
                &[ProtectedPath {
                    path: "src/a.rs".to_string(),
                    kind: ProtectedPathKind::File,
                }],
                ProtectedPathSource::ExplicitControl,
            )
            .unwrap();

        let raw = fs::read_to_string(session_log(&data_dir, &session.id)).unwrap();
        assert_eq!(
            raw.matches("\"eventType\":\"task_protected_paths\"")
                .count(),
            2,
            "add and remove are two appended events, not a rewrite"
        );
        assert!(raw.contains("\"action\":\"add\""));
        assert!(raw.contains("\"action\":\"remove\""));

        let fresh = SessionStore::new(&data_dir);
        let replay = fresh.task_protected_paths(&session.id, &task.id).unwrap();
        assert_eq!(
            replay.paths,
            vec![ProtectedPath {
                path: "src/nested".to_string(),
                kind: ProtectedPathKind::Directory,
            }]
        );
        assert_eq!(
            fs::read_to_string(session_log(&data_dir, &session.id))
                .unwrap()
                .lines()
                .count(),
            raw.lines().count(),
            "replay must not append or rewrite anything"
        );

        // A new user-started task in the same session starts from nothing.
        let new_task = fresh
            .create_task(&session.id, "Unrelated request.", "provider", "model")
            .unwrap();
        assert!(
            fresh
                .task_protected_paths(&session.id, &new_task.id)
                .unwrap()
                .is_empty()
        );

        // The first task still has its protection after the new task exists.
        assert!(
            fresh
                .task_protected_paths(&session.id, &task.id)
                .unwrap()
                .covers("src/nested/b.rs")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plan_and_verification_continuations_inherit_but_unrelated_work_does_not() {
        let root = repository("ancestry");
        let data_dir = root.join(".damaian");
        let store = SessionStore::new(&data_dir);
        let (session, patch_task) = session_and_task(&store, "Ancestry");
        store
            .record_task_protected_paths(
                &session.id,
                &patch_task.id,
                ProtectedPathAction::Add,
                &[ProtectedPath {
                    path: "src/nested".to_string(),
                    kind: ProtectedPathKind::Directory,
                }],
                ProtectedPathSource::Prompt,
            )
            .unwrap();

        let plan = TaskPlan::new(&patch_task.id, now_millis());
        store.create_plan(&patch_task, &plan).unwrap();

        // Verification carries the plan; a repair continuation carries it again.
        let verification_task = store
            .create_task(&session.id, "Verify.", "provider", "model")
            .unwrap();
        store
            .resume_plan(&verification_task, &patch_task.id)
            .unwrap();
        let repair_task = store
            .create_task(&session.id, "Repair.", "provider", "model")
            .unwrap();
        store
            .resume_plan(&repair_task, &verification_task.id)
            .unwrap();

        // A verification with no plan is linked by the due/started events.
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Due {
                    task_id: patch_task.id.clone(),
                    patch_id: String::new(),
                    paths: vec!["src/nested/b.rs".to_string()],
                },
            )
            .unwrap();
        let command_verification = store
            .create_task(&session.id, "Verify the command.", "provider", "model")
            .unwrap();
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Started {
                    task_id: command_verification.id.clone(),
                    attempt: 0,
                    checks: Vec::new(),
                },
            )
            .unwrap();
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Finished {
                    task_id: command_verification.id.clone(),
                    outcome: VerificationOutcome::Passed,
                },
            )
            .unwrap();

        // A wholly separate due/started/finished chain with no protection on
        // its origin must not pick up the earlier chain's protection.
        let clean_patch_task = store
            .create_task(&session.id, "Clean patch.", "provider", "model")
            .unwrap();
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Due {
                    task_id: clean_patch_task.id.clone(),
                    patch_id: String::new(),
                    paths: vec!["docs.md".to_string()],
                },
            )
            .unwrap();
        let clean_verification = store
            .create_task(&session.id, "Verify clean.", "provider", "model")
            .unwrap();
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Started {
                    task_id: clean_verification.id.clone(),
                    attempt: 0,
                    checks: Vec::new(),
                },
            )
            .unwrap();
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Finished {
                    task_id: clean_verification.id.clone(),
                    outcome: VerificationOutcome::Passed,
                },
            )
            .unwrap();

        // A started event with no preceding due has no origin to inherit.
        let dangling_started = store
            .create_task(&session.id, "Verify nothing due.", "provider", "model")
            .unwrap();
        store
            .append_verification_event(
                &session.id,
                &VerificationEvent::Started {
                    task_id: dangling_started.id.clone(),
                    attempt: 0,
                    checks: Vec::new(),
                },
            )
            .unwrap();

        let unrelated = store
            .create_task(&session.id, "A new request.", "provider", "model")
            .unwrap();

        let fresh = SessionStore::new(&data_dir);
        for task_id in [
            &verification_task.id,
            &repair_task.id,
            &command_verification.id,
        ] {
            let resolved = fresh.task_protected_paths(&session.id, task_id).unwrap();
            assert!(
                resolved.covers("src/nested/b.rs"),
                "{task_id} should inherit the directory protection: {resolved:?}"
            );
        }
        for (task_id, why) in [
            (
                &clean_verification.id,
                "a separate chain must not inherit the earlier chain",
            ),
            (
                &dangling_started.id,
                "a started event with no due must not inherit",
            ),
            (
                &unrelated.id,
                "a task that shares only the session must not inherit",
            ),
        ] {
            assert!(
                fresh
                    .task_protected_paths(&session.id, task_id)
                    .unwrap()
                    .is_empty(),
                "{why}: {task_id}"
            );
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_prompt_api_records_valid_paths_and_reports_invalid_ones() {
        let root = repository("prompt-api");
        let data_dir = root.join(".damaian");
        let engine = test_engine(&root);
        let (session, task) = engine_session_and_task(&engine, &root, "Prompt");

        let result = engine
            .record_task_protected_paths_from_prompt(
                &root,
                &session.id,
                &task.id,
                "Please do not touch `src/a.rs` or `src/missing.rs`.",
            )
            .unwrap();
        let ProtectedPathPromptResult::Recorded(update) = result else {
            panic!("the valid span should record: {result:?}");
        };
        assert_eq!(update.action, ProtectedPathAction::Add);
        assert_eq!(update.source, ProtectedPathSource::Prompt);
        assert_eq!(update.accepted.len(), 1);
        assert_eq!(update.accepted[0].path, "src/a.rs");
        assert_eq!(update.refused.len(), 1);
        assert_eq!(update.refused[0].error, ProtectedPathError::Missing);
        assert!(update.refused[0].message().contains("src/missing.rs"));

        let resolved = SessionStore::new(&data_dir)
            .task_protected_paths(&session.id, &task.id)
            .unwrap();
        assert_eq!(resolved.path_list(), vec!["src/a.rs".to_string()]);
        let raw = fs::read_to_string(session_log(&data_dir, &session.id)).unwrap();
        assert!(raw.contains("\"source\":\"prompt\""));
        assert!(raw.contains(&format!("\"taskId\":\"{}\"", task.id)));

        // Ambiguous wording refuses the whole request and writes no event.
        let before = fs::read_to_string(session_log(&data_dir, &session.id))
            .unwrap()
            .lines()
            .count();
        let refused = engine
            .record_task_protected_paths_from_prompt(
                &root,
                &session.id,
                &task.id,
                "do not touch the unrelated files `src/nested/b.rs`",
            )
            .unwrap();
        assert_eq!(
            refused,
            ProtectedPathPromptResult::Refused(ProtectedPathPromptError::AmbiguousWording)
        );
        let after = fs::read_to_string(session_log(&data_dir, &session.id))
            .unwrap()
            .lines()
            .count();
        assert_eq!(before, after);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn message_and_repository_text_cannot_establish_a_protection() {
        let root = repository("provenance");
        let data_dir = root.join(".damaian");
        let engine = test_engine(&root);
        let (session, task) = engine_session_and_task(&engine, &root, "Provenance");

        // Model output lands in the conversation; it does not invoke the
        // parser and cannot write a protection event.
        engine
            .session_store
            .append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                "I will do not touch `src/a.rs` as instructed.",
            )
            .unwrap();
        // A repository file holding the same sentence is likewise just text.
        fs::write(root.join("NOTES.md"), "do not touch `src/a.rs`\n").unwrap();

        assert!(
            SessionStore::new(&data_dir)
                .task_protected_paths(&session.id, &task.id)
                .unwrap()
                .is_empty()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn worker_handoff_seeds_the_first_task_before_the_model_and_never_parses_the_prompt() {
        let root = repository("worker-handoff");
        let engine = native_engine(&root);
        let repository_id = repository_id_for_root(&root);
        let worker = engine
            .session_store
            .create_session(&repository_id, "Worker")
            .unwrap();
        let inherited = vec![
            ProtectedPath {
                path: "src/a.rs".to_string(),
                kind: ProtectedPathKind::File,
            },
            ProtectedPath {
                path: "src/nested".to_string(),
                kind: ProtectedPathKind::Directory,
            },
        ];
        engine
            .session_store
            .record_worker_spawned(
                &worker.id,
                &crate::session::WorkerRecord {
                    parent_session_id: Some("ses_parent".to_string()),
                    parent_task_id: Some("task_parent".to_string()),
                    inherited_protected_paths: inherited.clone(),
                    source_repository: root.clone(),
                    worktree_path: root.clone(),
                    branch: "damaian/worker".to_string(),
                    base_commit: "HEAD".to_string(),
                    included_uncommitted: false,
                    git_commands: Vec::new(),
                },
            )
            .unwrap();

        let mut progress = Vec::new();
        let mut on_token = |_token: &str| {};
        let mut adapter = MockModelAdapter::new("Done.");
        let first = {
            let never_cancelled = crate::cancel::CancelToken::new();
            let mut on_progress = |event| progress.push(event);
            let mut sink = crate::chat::TurnSink {
                on_token: &mut on_token,
                on_progress: &mut on_progress,
                cancel: &never_cancelled,
                steering: None,
            };
            engine
                .chat_orchestrator
                .ask_with_session(
                    &root,
                    "Model text says do not touch `docs.md`.",
                    &[],
                    Some(&worker.id),
                    &mut adapter,
                    &mut sink,
                )
                .unwrap()
        };
        assert!(
            !adapter.requests.is_empty(),
            "the first model request must have run"
        );

        let active = engine
            .task_protected_paths(&worker.id, &first.task.id)
            .unwrap();
        assert_eq!(
            active.paths, inherited,
            "the first worker task receives exactly the parent snapshot"
        );
        assert!(
            !active.covers("docs.md"),
            "model-authored prompt text must not broaden the snapshot"
        );
        let events = engine
            .session_store
            .read_task_protected_path_events(&worker.id)
            .unwrap();
        assert_eq!(events.len(), 1, "one handoff event, not one per path");
        assert_eq!(
            events[0].source,
            ProtectedPathSource::InheritedWorkerHandoff
        );

        // The seed is durable before the first model request, not written after
        // the turn happens to read it.
        let raw = fs::read_to_string(session_log(&root.join(".damaian"), &worker.id)).unwrap();
        let seq_for = |event_type: &str, action: Option<&str>| -> u64 {
            raw.lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find(|event| {
                    event["eventType"] == event_type
                        && action.is_none_or(|action| event["payload"]["action"] == action)
                })
                .and_then(|event| event["seq"].as_u64())
                .unwrap_or_else(|| panic!("no {event_type} {action:?} event"))
        };
        assert!(
            seq_for("task_protected_paths", None) < seq_for("action_started", Some("model_call")),
            "the handoff must be recorded before the first model request"
        );

        let reported = progress
            .iter()
            .find_map(|event| match event {
                crate::chat::TurnProgress::ProtectedPaths(active) => Some(active),
                _ => None,
            })
            .expect("the turn reports its protected paths");
        assert_eq!(reported.task_id, first.task.id);
        assert_eq!(reported.paths, inherited);

        // A restart reads the same durable task record and does not append a
        // second handoff event.
        let restarted = native_engine(&root);
        assert_eq!(
            restarted
                .task_protected_paths(&worker.id, &first.task.id)
                .unwrap()
                .paths,
            inherited
        );
        assert_eq!(
            restarted
                .session_store
                .read_task_protected_path_events(&worker.id)
                .unwrap()
                .len(),
            1
        );

        // A later user-started task in the same worker session starts empty.
        let mut second_adapter = MockModelAdapter::new("Done.");
        let second = {
            let never_cancelled = crate::cancel::CancelToken::new();
            let mut on_progress = |_event| {};
            let mut sink = crate::chat::TurnSink {
                on_token: &mut on_token,
                on_progress: &mut on_progress,
                cancel: &never_cancelled,
                steering: None,
            };
            restarted
                .chat_orchestrator
                .ask_with_session(
                    &root,
                    "Now do something new.",
                    &[],
                    Some(&worker.id),
                    &mut second_adapter,
                    &mut sink,
                )
                .unwrap()
        };
        assert_ne!(second.task.id, first.task.id);
        assert!(
            restarted
                .task_protected_paths(&worker.id, &second.task.id)
                .unwrap()
                .is_empty(),
            "a new worker task must not automatically inherit the first task's paths"
        );
        assert_eq!(
            restarted
                .session_store
                .read_task_protected_path_events(&worker.id)
                .unwrap()
                .len(),
            1,
            "the later task must not append another handoff event"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_refuses_a_pending_patch_whose_remaining_file_is_now_protected() {
        let root = repository("recovery-protected");
        let engine = native_engine(&root);
        let (session_id, task_id, patch_id) =
            pending_patch(&engine, &root, "protected", &["docs.md"]);
        engine
            .add_task_protected_paths(&root, &session_id, &task_id, &["docs.md".to_string()])
            .unwrap();
        let checkpoints = files_under(&root.join(".damaian").join("checkpoints"));

        let reattached = reattach(&engine, &root, &session_id);

        assert_eq!(reattached.len(), 1);
        match &reattached[0] {
            crate::recovery::ReattachedApproval::Unavailable {
                task_id: refused_task,
                reason,
            } => {
                assert_eq!(refused_task, &task_id);
                assert!(reason.contains("docs.md"), "{reason}");
                assert!(reason.contains("protected task path"), "{reason}");
            }
            other => panic!("a protected pending patch must not be actionable: {other:?}"),
        }
        assert_eq!(
            engine
                .session_store
                .task(&session_id, &task_id)
                .unwrap()
                .status,
            TaskStatus::Failed,
            "the normal unavailable recovery outcome closes the task"
        );
        assert!(
            engine
                .session_store
                .read_patch_file_decisions(&session_id, &task_id)
                .unwrap()
                .is_empty(),
            "the refusal is not a patch-file decision"
        );
        assert!(
            engine
                .session_store
                .read_verification_events(&session_id, &[task_id.as_str()])
                .unwrap()
                .is_empty(),
            "no verification may become due"
        );
        assert_eq!(
            fs::read_to_string(root.join("docs.md")).unwrap(),
            "# docs\n",
            "recovery must write nothing"
        );
        assert_eq!(
            files_under(&root.join(".damaian").join("checkpoints")),
            checkpoints,
            "no checkpoint may be taken"
        );
        let raw = fs::read_to_string(session_log(&root.join(".damaian"), &session_id)).unwrap();
        for forbidden in [
            "\"eventType\":\"apply_patch\"",
            "\"eventType\":\"patch_file_decided\"",
            "\"eventType\":\"verification_due\"",
            "\"eventType\":\"action_started\"",
        ] {
            assert!(!raw.contains(forbidden), "{forbidden} was recorded: {raw}");
        }
        let refusals = assistant_refusals(&engine.session_store, &session_id);
        assert!(
            refusals
                .iter()
                .any(|message| message.contains("docs.md")
                    && message.contains("protected task path")),
            "{refusals:?}"
        );
        let audit =
            fs::read_to_string(root.join(".damaian/audit/events.jsonl")).unwrap_or_default();
        assert!(
            audit.contains("task_protected_path_refusal") && audit.contains("docs.md"),
            "the refusal must be durably audited: {audit}"
        );
        assert!(
            raw.contains(&patch_id),
            "the pending patch id remains in the task log"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_keeps_unprotected_cards_and_ignores_decided_protected_files() {
        let root = repository("recovery-open");
        let engine = native_engine(&root);
        let (open_session, open_task, _) =
            pending_patch(&engine, &root, "open", &["src/nested/b.rs"]);
        let (decided_session, decided_task, decided_patch) =
            pending_patch(&engine, &root, "decided", &["docs.md", "src/nested/b.rs"]);
        engine
            .session_store
            .append_patch_file_decision(
                &decided_session,
                &crate::session::PatchFileDecision {
                    task_id: decided_task.clone(),
                    patch_id: decided_patch,
                    paths: vec!["docs.md".to_string()],
                    applied: true,
                    applied_hashes: Default::default(),
                    accepted_hunks: Default::default(),
                },
            )
            .unwrap();
        // The decided file became protected after the decision. It is no longer
        // actionable, so it must not turn the remaining review into a refusal.
        engine
            .add_task_protected_paths(
                &root,
                &decided_session,
                &decided_task,
                &["docs.md".to_string()],
            )
            .unwrap();

        let open = reattach(&engine, &root, &open_session);
        assert!(
            matches!(
                open.as_slice(),
                [crate::recovery::ReattachedApproval::Patch { task_id, .. }]
                    if task_id == &open_task
            ),
            "an unprotected pending card must stay reattachable: {open:?}"
        );

        let decided = reattach(&engine, &root, &decided_session);
        assert!(
            matches!(
                decided.as_slice(),
                [crate::recovery::ReattachedApproval::Patch { task_id, .. }]
                    if task_id == &decided_task
            ),
            "a protected already-decided file must not cause a false refusal: {decided:?}"
        );
        assert_eq!(
            engine
                .session_store
                .task(&decided_session, &decided_task)
                .unwrap()
                .status,
            TaskStatus::WaitingForApproval,
            "the card stays pending"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_control_selects_the_newest_created_task_and_reloads_durably() {
        let root = repository("control-newest");
        let engine = test_engine(&root);
        let (session, first) = engine_session_and_task(&engine, &root, "First");
        let first_update = engine
            .add_task_protected_paths(&root, &session.id, &first.id, &["src/a.rs".to_string()])
            .unwrap();
        assert!(
            first_update.outcome.contains("Protected src/a.rs (file)."),
            "{first_update:?}"
        );

        let second = engine
            .session_store
            .create_task(&session.id, "Second work.", "provider", "model")
            .unwrap();
        let second_update = engine
            .add_task_protected_paths(&root, &session.id, &second.id, &["src/nested".to_string()])
            .unwrap();
        assert_eq!(second_update.control.task_id, second.id);
        assert_eq!(
            second_update.control.rows[0].kind_label,
            "directory (and everything under it)"
        );
        assert_eq!(
            second_update.control.rows[0].remove_label,
            "Stop protecting src/nested"
        );

        let control = engine
            .task_protected_paths_control(&root, &session.id)
            .unwrap();
        assert!(control.available);
        assert_eq!(control.task_id, second.id);
        assert_eq!(control.task_label, format!("Task {}", second.id));
        assert_eq!(control.note, "");
        assert_eq!(control.rows.len(), 1);
        assert_eq!(control.rows[0].path, "src/nested");
        assert_eq!(control.rows[0].kind, ProtectedPathKind::Directory);

        let reloaded = test_engine(&root)
            .task_protected_paths_control(&root, &session.id)
            .unwrap();
        assert_eq!(reloaded, control);

        // A desktop control action names the old task after a new one exists:
        // the current-control API refuses it before any event.
        let events_before = engine
            .session_store
            .read_task_protected_path_events(&session.id)
            .unwrap()
            .len();
        let stale = engine
            .add_current_task_protected_paths(
                &root,
                &session.id,
                &first.id,
                &["docs.md".to_string()],
            )
            .unwrap_err();
        assert!(
            matches!(stale, ClientError::InvalidInput(ref message) if message.contains("now targets")),
            "{stale:?}"
        );
        assert_eq!(
            engine
                .session_store
                .read_task_protected_path_events(&session.id)
                .unwrap()
                .len(),
            events_before,
            "a stale desktop action appends no event"
        );

        // The generic explicit API keeps working on a named older task, which
        // is what the CLI uses.
        let explicit = engine
            .add_task_protected_paths(&root, &session.id, &first.id, &["docs.md".to_string()])
            .unwrap();
        assert_eq!(explicit.control.task_id, first.id);
        let named = engine
            .task_protected_paths_control_for_task(&root, &session.id, &first.id)
            .unwrap();
        assert_eq!(named.task_id, first.id);
        assert_eq!(
            named
                .rows
                .iter()
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/a.rs", "docs.md"]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_control_reports_no_task_and_refuses_another_repository() {
        let root = repository("control-unavailable");
        let engine = test_engine(&root);
        let session = engine
            .session_store
            .create_session(&repository_id_for_root(&root), "No tasks yet")
            .unwrap();
        let control = engine
            .task_protected_paths_control(&root, &session.id)
            .unwrap();
        assert!(!control.available);
        assert_eq!(control.task_id, "");
        assert!(control.rows.is_empty());
        assert!(control.note.contains("No task is available"), "{control:?}");

        let task = engine
            .session_store
            .create_task(&session.id, "Do the work.", "provider", "model")
            .unwrap();
        let other = repository("control-other-repository");
        let error = engine
            .task_protected_paths_control(&other, &session.id)
            .unwrap_err();
        assert!(
            matches!(error, ClientError::AccessDenied(ref message) if message.contains("does not belong to repository")),
            "{error:?}"
        );
        assert!(
            engine
                .session_store
                .read_task_protected_path_events(&session.id)
                .unwrap()
                .is_empty(),
            "a refused repository read appends no event"
        );
        assert_eq!(
            engine
                .session_store
                .task_protected_paths(&session.id, &task.id)
                .unwrap()
                .paths,
            Vec::new()
        );

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(other);
    }

    #[test]
    fn a_partial_update_carries_the_shared_outcome_and_control_rows() {
        let root = repository("control-partial");
        let engine = test_engine(&root);
        let (session, task) = engine_session_and_task(&engine, &root, "Partial");
        let update = engine
            .add_task_protected_paths(
                &root,
                &session.id,
                &task.id,
                &["src/a.rs".to_string(), "src/missing.rs".to_string()],
            )
            .unwrap();
        assert!(
            update.outcome.contains("Protected src/a.rs (file)."),
            "{update:?}"
        );
        assert!(
            update.outcome.contains("src/missing.rs") && update.outcome.contains("does not exist"),
            "{update:?}"
        );
        assert_eq!(update.control.task_id, task.id);
        assert_eq!(update.control.rows.len(), 1);
        assert_eq!(update.control.rows[0].path, "src/a.rs");
        assert_eq!(update.control.rows[0].kind_label, "file");

        let empty = engine
            .add_task_protected_paths(&root, &session.id, &task.id, &[])
            .unwrap();
        assert_eq!(
            empty.outcome,
            "Enter at least one repository-relative path."
        );
        assert_eq!(empty.control.rows, update.control.rows);

        let _ = fs::remove_dir_all(root);
    }
}
