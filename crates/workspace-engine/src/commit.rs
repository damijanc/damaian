//! Commit preparation: Damaian's first write to a user's Git repository.
//!
//! See `docs/specs/35_commit_preparation/proposal.md` and its `context.md`.
//! The module's whole reason for existing is that a commit is the one action
//! where being approximately right is unsafe:
//!
//! - **The user's staging area is theirs.** Nothing here reads or writes
//!   `.git/index`. Every tree is built through a private `GIT_INDEX_FILE`, so a
//!   failure leaves nothing to restore because nothing was changed.
//! - **The preview is redacted; the commit is not.** The prepared record stores
//!   each candidate's exact blob identity, and `commit` recomputes it. A file
//!   that changed after review refuses the commit, so there is no window in
//!   which the approved preview and the committed bytes differ.
//! - **A repository's hooks are its own.** `commit-tree` runs none of them, so
//!   they are executed explicitly, in Git's order, with the private index in
//!   their environment. No `--no-verify`, no skip flag.
//! - **A commit is a capability.** Code mode, a profile that permits
//!   `allow_git_commits`, and a typed per-commit approval.

use crate::audit::AuditLog;
use crate::cancel::CancelToken;
use crate::checkpoint::{CheckpointOrigin, CheckpointStore};
use crate::command_purpose::CommandPurpose;
use crate::command_runner::{CommandRunOptions, CommandRunner};
use crate::config::Config;
use crate::diff::diff_file;
use crate::error::{ClientError, Result};
use crate::git_service::{DiffOptions, GitService};
use crate::hash::{create_id, now_millis, repository_id_for_root};
use crate::mode::SessionMode;
use crate::patch_engine::GeneratedSecretWarning;
use crate::path_policy::PathPolicy;
use crate::plan::TaskPlan;
use crate::secret_scanner::SecretScanner;
use crate::session::{ChangeEvent, SessionStore, TaskStatus};
use crate::verification::{VerificationReport, fold_verification};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The hooks Git itself would run for a commit, in its order. `prepare-commit-msg`
/// is included because a repository that uses it expects it to run even though
/// `commit-tree` skips it (proposal §5.5 lists it; context §3 D4 runs it).
pub const GIT_COMMIT_HOOKS: [&str; 4] = [
    "pre-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
];

/// Where a candidate came from. The label the user reads is [`CommitCandidate::label`];
/// this is the machine-readable form the UI and tests branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitCandidateSource {
    /// Damaian applied it and the file is still exactly as Damaian left it.
    AcceptedPatch,
    /// Damaian applied it, then the file was edited by hand or another tool.
    AcceptedThenEdited,
    /// An approved command changed it (from the session's checkpoint).
    CommandEffect,
    /// A patch Damaian applied whose acceptance predates the decision record.
    DamaianChange,
    /// Present in `git status` and unknown to this session.
    External,
    /// Untracked and unknown to this session.
    Untracked,
    /// The working tree has an unresolved conflict at this path.
    Conflict,
}

impl CommitCandidateSource {
    /// Whether this source is part of the commit's candidate set by default.
    fn selected_by_default(self) -> bool {
        matches!(self, Self::AcceptedPatch)
    }

    fn label(self) -> &'static str {
        match self {
            Self::AcceptedPatch => "Accepted change",
            Self::AcceptedThenEdited => {
                "Damaian changed this file, then it was edited by hand — not selected"
            }
            Self::CommandEffect => "Changed by an approved command — not selected",
            Self::DamaianChange => {
                "Damaian changed this file, but the session has no acceptance record — not selected"
            }
            Self::External => "Changed outside Damaian — not selected",
            Self::Untracked => "Untracked file — not selected",
            Self::Conflict => "Unresolved conflict — resolve it before committing",
        }
    }
}

/// One path the commit dialog can show, with the exact bytes on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitCandidate {
    pub path: String,
    /// `added`, `modified` or `deleted`, as the commit will express it.
    pub status: String,
    pub source: CommitCandidateSource,
    pub label: String,
    pub selected_by_default: bool,
    /// Git's object id for the exact bytes on disk, or `None` for a deletion.
    /// Compared again at commit time; a mismatch is the redacted-preview trap's
    /// guard.
    pub blob_oid: Option<String>,
    /// SHA-256 of the exact bytes, when they were readable. This is what the
    /// session's applied hash records; `blob_oid` is Git's identity for the
    /// same bytes.
    pub content_sha256: Option<String>,
    /// `100644`, `100755` or `120000`; `None` for a deletion.
    pub mode: Option<String>,
    pub size: u64,
    /// True when the file was too large to scan for secrets, so the override
    /// is required even though no finding was produced.
    pub unscanned: bool,
    /// Categories and counts only — never matched text.
    pub secret_warnings: Vec<GeneratedSecretWarning>,
    /// The redacted diff for this path, for display. The commit never uses it.
    pub diff: String,
    /// The hunk ids the user accepted for this path, when recorded.
    pub accepted_hunks: Vec<String>,
}

/// One Git hook as the preparation reports it. The body is never summarised:
/// a confident description of arbitrary code is exactly the kind of claim that
/// would be wrong at the worst moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHookInfo {
    pub name: String,
    pub path: String,
    pub exists: bool,
    pub executable: bool,
    pub size: u64,
    pub interpreter: Option<String>,
    /// Whether Damaian would run it: exists, is a file and has an execute bit.
    pub runs: bool,
    /// Why it will not run, when it exists but Git would skip it.
    pub skipped_reason: Option<String>,
}

/// The hooks a commit would trigger, and where they came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHookReport {
    pub directory: String,
    /// `"core.hooksPath"` or `"default"`.
    pub source: String,
    pub hooks: Vec<GitHookInfo>,
}

impl Default for GitHookReport {
    fn default() -> Self {
        Self {
            directory: String::new(),
            source: "default".to_string(),
            hooks: Vec::new(),
        }
    }
}

/// What #23's verification loop last said about the files being committed,
/// reduced to what a commit card needs. Staleness is decided against the bytes
/// on disk now, not against what was checked earlier.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitVerification {
    pub due: bool,
    pub passed: Vec<String>,
    pub failed: Vec<String>,
    pub skipped: Vec<String>,
    pub stale: Vec<String>,
    pub summary: String,
}

/// A prepared commit. The record behind it is durable, so a restart can still
/// commit what the user reviewed or explain why it cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitPreparation {
    pub prepare_id: String,
    pub repository_root: String,
    pub session_id: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub candidates: Vec<CommitCandidate>,
    pub hooks: GitHookReport,
    pub message: String,
    pub verification: CommitVerification,
    /// Why this commit cannot be created, in words a person reads. Empty when
    /// it can.
    pub refusals: Vec<String>,
    pub can_commit: bool,
    pub selected_count: usize,
    /// Always true today: the diff is redacted for display while the commit
    /// contains the real bytes. Sent explicitly so the UI cannot forget to say
    /// so.
    pub diff_redacted: bool,
}

/// The cheap answer the repo bar asks for: is there anything to commit, and
/// can this session commit it?
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitStatus {
    pub candidate_count: usize,
    pub selected_count: usize,
    pub can_commit: bool,
    pub refusal: Option<String>,
}

/// The approval. Owned strings because it crosses the shell boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRequest {
    pub session_id: String,
    pub prepare_id: String,
    pub selected_paths: Vec<String>,
    pub message: String,
    pub allow_generated_secrets: bool,
    pub approved_by: String,
}

/// What happened to one hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitHookOutcome {
    pub name: String,
    pub outcome: String,
    pub exit_code: Option<i32>,
    pub termination: String,
    /// Already truncated and redacted by `CommandRunner`.
    pub stdout: String,
    pub stderr: String,
}

/// A created commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitOutcome {
    pub commit_sha: String,
    pub branch: String,
    pub message: String,
    pub path_count: usize,
    pub hunk_count: usize,
    pub hook_outcomes: Vec<CommitHookOutcome>,
    /// Non-fatal problems, such as a failing `post-commit` hook. The commit
    /// exists; pretending it did not would be worse.
    pub warnings: Vec<String>,
    pub generated_secret_override: bool,
}

/// Why a commit did not happen, as a value the shell can branch on instead of
/// parsing prose. Every variant means the ref did not move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitError {
    /// A policy or state refusal, checked before anything was written.
    Refused { code: &'static str, message: String },
    /// A hook exited non-zero. `pre-commit`, `prepare-commit-msg` and
    /// `commit-msg` abort; `post-commit` does not and is a warning instead.
    HookFailed {
        name: String,
        exit_code: Option<i32>,
        termination: String,
        stdout: String,
        stderr: String,
    },
    /// `prepare-commit-msg` or `commit-msg` changed the approved message.
    /// Nothing was created; the user reviews the final text and approves again.
    MessageRewritten { message: String },
    /// An engine operation failed (an IO or git error). The ref did not move.
    Engine(String),
}

impl CommitError {
    fn refused(code: &'static str, message: impl Into<String>) -> Self {
        Self::Refused {
            code,
            message: message.into(),
        }
    }

    /// The stable code the HTTP layer returns beside the message.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused { code, .. } => code,
            Self::HookFailed { .. } => "commit_hook_failed",
            Self::MessageRewritten { .. } => "commit_message_rewritten",
            Self::Engine(_) => "commit_engine_error",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Refused { message, .. } => message.clone(),
            Self::HookFailed {
                name,
                exit_code,
                termination,
                stdout,
                stderr,
            } => {
                let code = exit_code
                    .map(|code| format!("exit code {code}"))
                    .unwrap_or_else(|| termination.clone());
                let detail = [stdout.trim(), stderr.trim()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if detail.is_empty() {
                    format!("The {name} hook failed ({code}). Nothing was committed.")
                } else {
                    format!("The {name} hook failed ({code}). Nothing was committed.\n{detail}")
                }
            }
            Self::MessageRewritten { .. } => {
                "A commit-message hook rewrote the message. Review the new text and commit \
                 again; nothing was committed."
                    .to_string()
            }
            Self::Engine(message) => message.clone(),
        }
    }
}

impl From<ClientError> for CommitError {
    fn from(error: ClientError) -> Self {
        Self::Engine(error.to_string())
    }
}

/// The durable record behind a preparation. Recovery reads it to say what a
/// dangling `commit` action was doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRecord {
    pub prepare_id: String,
    pub repository_root: String,
    pub session_id: String,
    /// The synthetic task the marker belongs to. `None` for a record written
    /// before markers existed.
    #[serde(default)]
    pub task_id: Option<String>,
    /// The paths the user actually approved, which may be a subset of the
    /// candidates.
    #[serde(default)]
    pub selected_paths: Vec<String>,
    pub branch: Option<String>,
    pub expected_old: Option<String>,
    pub candidates: Vec<CommitCandidate>,
    pub hooks: GitHookReport,
    pub message: String,
    pub verification: CommitVerification,
    pub prepared_at_ms: u128,
    /// `prepared`, `created` or `failed`.
    pub state: String,
    pub prepared_commit: Option<String>,
    pub head_after: Option<String>,
    pub hook_outcomes: Vec<CommitHookOutcome>,
    pub generated_secret_override: bool,
    pub hunk_count: usize,
}

#[derive(Debug, Clone)]
pub struct CommitEngine {
    config: Config,
    audit_log: AuditLog,
    scanner: SecretScanner,
    path_policy: PathPolicy,
    session_store: SessionStore,
    checkpoint_store: CheckpointStore,
    git: GitService,
    command_runner: CommandRunner,
}

struct GitState {
    branch: Option<String>,
    head: Option<String>,
    refusal: Option<String>,
}

#[derive(Debug, Clone)]
struct BlobInfo {
    oid: String,
    mode: String,
    size: u64,
    symlink: bool,
    /// SHA-256 of the bytes, when they were read for the secret check. `None`
    /// for a file past `max_file_bytes`, which cannot then be matched against
    /// the applied hash a decision recorded.
    content_sha256: Option<String>,
}

impl CommitEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        audit_log: AuditLog,
        scanner: SecretScanner,
        path_policy: PathPolicy,
        session_store: SessionStore,
        checkpoint_store: CheckpointStore,
        git: GitService,
        command_runner: CommandRunner,
    ) -> Self {
        Self {
            config,
            audit_log,
            scanner,
            path_policy,
            session_store,
            checkpoint_store,
            git,
            command_runner,
        }
    }

    /// The repo bar's question, without the diffs, hooks or secret scans a
    /// preparation needs.
    pub fn status(&self, root: impl AsRef<Path>, session_id: &str) -> Result<CommitStatus> {
        let root = root.as_ref();
        let refusal = self.refusal(root, session_id)?;
        let candidates = self.candidates(root, session_id)?;
        let selected = candidates
            .iter()
            .filter(|candidate| candidate.selected_by_default)
            .count();
        Ok(CommitStatus {
            candidate_count: candidates.len(),
            selected_count: selected,
            can_commit: refusal.is_none() && selected > 0,
            refusal,
        })
    }

    /// The full preparation. Writes only Damaian's own record under
    /// `<data_dir>/commits/`; the repository, index and working tree are read.
    pub fn prepare(&self, root: impl AsRef<Path>, session_id: &str) -> Result<CommitPreparation> {
        let root = root.as_ref();
        let state = self.git_state(root)?;
        let refusal = self.refusal_from_state(root, session_id, &state)?;
        let mut candidates = self.candidates(root, session_id)?;
        for candidate in &mut candidates {
            candidate.diff = self.diff_for(
                root,
                &candidate.path,
                &candidate.status,
                state.head.is_some(),
            )?;
        }
        let selected_paths = candidates
            .iter()
            .filter(|candidate| candidate.selected_by_default)
            .map(|candidate| candidate.path.clone())
            .collect::<Vec<_>>();
        let selected_count = selected_paths.len();
        let message = self.suggest_message(session_id, &selected_paths);
        let hooks = self.hook_report(root)?;
        let verification = self.verification(root, session_id, &candidates)?;
        let prepare_id = create_id("commit");
        let can_commit = refusal.is_none() && selected_count > 0;
        let preparation = CommitPreparation {
            prepare_id: prepare_id.clone(),
            repository_root: root.to_string_lossy().to_string(),
            session_id: session_id.to_string(),
            branch: state.branch.clone(),
            head: state.head.clone(),
            candidates: candidates.clone(),
            hooks: hooks.clone(),
            message,
            verification: verification.clone(),
            refusals: refusal.into_iter().collect(),
            can_commit,
            selected_count,
            diff_redacted: true,
        };
        let record = CommitRecord {
            prepare_id: prepare_id.clone(),
            repository_root: root.to_string_lossy().to_string(),
            session_id: session_id.to_string(),
            task_id: None,
            selected_paths: Vec::new(),
            branch: state.branch,
            expected_old: state.head,
            candidates,
            hooks,
            message: preparation.message.clone(),
            verification,
            prepared_at_ms: now_millis(),
            state: "prepared".to_string(),
            prepared_commit: None,
            head_after: None,
            hook_outcomes: Vec::new(),
            generated_secret_override: false,
            hunk_count: 0,
        };
        self.save_record(&record)?;
        self.audit_log.record(
            "commit_prepared",
            &[
                ("actor", "user".to_string()),
                ("sessionId", session_id.to_string()),
                ("prepareId", prepare_id),
                ("head", record.expected_old.clone().unwrap_or_default()),
                ("candidateCount", record.candidates.len().to_string()),
                (
                    "selectedCount",
                    record
                        .candidates
                        .iter()
                        .filter(|candidate| candidate.selected_by_default)
                        .count()
                        .to_string(),
                ),
                (
                    "secretWarningCount",
                    record
                        .candidates
                        .iter()
                        .filter(|candidate| !candidate.secret_warnings.is_empty())
                        .count()
                        .to_string(),
                ),
                (
                    "hookCount",
                    record
                        .hooks
                        .hooks
                        .iter()
                        .filter(|hook| hook.runs)
                        .count()
                        .to_string(),
                ),
                ("canCommit", can_commit.to_string()),
            ],
        )?;
        Ok(preparation)
    }

    /// Load a prepared record. The shell uses this after a restart and the
    /// recovery sweep uses it for a dangling action.
    pub fn load_record(&self, prepare_id: &str) -> Result<CommitRecord> {
        let content = fs::read_to_string(self.record_path(prepare_id))?;
        serde_json::from_str(&content).map_err(|error| {
            ClientError::Io(format!("Commit record {prepare_id} is unreadable: {error}"))
        })
    }

    /// Create the commit. Everything before `update-ref` is a refusal with
    /// nothing written; a failure after it is reported as a created commit,
    /// because that is what happened.
    pub fn commit(
        &self,
        root: impl AsRef<Path>,
        request: &CommitRequest,
        cancel: &CancelToken,
    ) -> std::result::Result<CommitOutcome, CommitError> {
        let root = root.as_ref();
        if let Some(refusal) = self.refusal(root, &request.session_id)? {
            return Err(CommitError::refused("commit_not_allowed", refusal));
        }
        let mut record = self
            .load_record(&request.prepare_id)
            .map_err(CommitError::from)?;
        if record.session_id != request.session_id {
            return Err(CommitError::refused(
                "commit_not_prepared",
                "That preparation belongs to a different session.",
            ));
        }
        let prepared_root = Path::new(&record.repository_root);
        if fs::canonicalize(prepared_root).ok() != fs::canonicalize(root).ok() {
            return Err(CommitError::refused(
                "commit_not_prepared",
                "That preparation belongs to a different repository.",
            ));
        }
        if record.state != "prepared" {
            return Err(CommitError::refused(
                "commit_not_prepared",
                "That commit preparation is no longer current. Prepare it again.",
            ));
        }
        let selected = self.selected_candidates(&record, &request.selected_paths)?;
        for candidate in &selected {
            if self
                .path_policy
                .resolve_for_write(root, &candidate.path)
                .is_err()
                || self.path_is_restricted(&candidate.path)
            {
                return Err(CommitError::refused(
                    "commit_restricted_path",
                    format!(
                        "{} is outside this checkout or is restricted, so it cannot be committed.",
                        candidate.path
                    ),
                ));
            }
        }
        let message = request.message.trim_end().to_string();
        if message.trim().is_empty() {
            return Err(CommitError::refused(
                "commit_message_required",
                "A commit needs a message.",
            ));
        }
        let state = self.git_state(root).map_err(CommitError::from)?;
        if let Some(refusal) = state.refusal {
            return Err(CommitError::refused("commit_git_state", refusal));
        }
        if state.head != record.expected_old {
            return Err(CommitError::refused(
                "commit_head_moved",
                format!(
                    "HEAD moved from {} to {} since you reviewed this commit. \
                     Nothing was committed; prepare it again.",
                    record.expected_old.as_deref().unwrap_or("no commit"),
                    state.head.as_deref().unwrap_or("no commit"),
                ),
            ));
        }
        let branch = state.branch.clone().ok_or_else(|| {
            CommitError::refused(
                "commit_git_state",
                "HEAD is detached. Check out a branch before committing.",
            )
        })?;
        if record.branch.as_deref() != Some(branch.as_str()) {
            return Err(CommitError::refused(
                "commit_head_moved",
                "The checked-out branch changed since you reviewed this commit.                  Nothing was committed; prepare it again.",
            ));
        }
        // A hook added or changed after the report was shown would run against
        // a review that did not include it.
        let hooks_now = self.hook_report(root).map_err(CommitError::from)?;
        if hooks_now != record.hooks {
            return Err(CommitError::refused(
                "commit_hooks_changed",
                "The repository's Git hooks changed since you reviewed this commit.                  Nothing was committed; prepare it again.",
            ));
        }
        // The redacted-preview trap: the preview was redacted, so the bytes
        // that will be committed are proven here, not inferred from it.
        let mut fresh = Vec::new();
        let mut secret_warnings = Vec::new();
        let mut unscanned = Vec::new();
        for candidate in &selected {
            let blob = self
                .blob_info(root, &candidate.path)
                .map_err(CommitError::from)?;
            let matches = match (&blob, &candidate.blob_oid, &candidate.mode) {
                (None, None, None) => true,
                (Some(blob), Some(oid), Some(mode)) => &blob.oid == oid && &blob.mode == mode,
                _ => false,
            };
            if !matches {
                return Err(CommitError::refused(
                    "commit_content_changed",
                    format!(
                        "{} changed after you reviewed it. Nothing was committed. \
                         Prepare the commit again.",
                        candidate.path
                    ),
                ));
            }
            if blob.as_ref().is_some_and(|blob| blob.symlink) {
                // A symlink's bytes are its target; scanning it as a file is
                // meaningless and a secret in a path is not this check's job.
                fresh.push((candidate.clone(), blob));
                continue;
            }
            if let Some(blob) = &blob {
                if blob.size > self.config.max_file_bytes {
                    unscanned.push(candidate.path.clone());
                } else {
                    let bytes = fs::read(root.join(&candidate.path))
                        .map_err(|error| CommitError::Engine(error.to_string()))?;
                    let warnings = self.scan_bytes(&candidate.path, &bytes);
                    secret_warnings.extend(warnings);
                }
                fresh.push((candidate.clone(), Some(blob.clone())));
            } else {
                fresh.push((candidate.clone(), None));
            }
        }
        for path in &unscanned {
            secret_warnings.push(GeneratedSecretWarning {
                path: path.clone(),
                categories: vec!["unscanned".to_string()],
                count: 1,
            });
        }
        if !secret_warnings.is_empty() && !request.allow_generated_secrets {
            let paths = secret_warnings
                .iter()
                .map(|warning| warning.path.clone())
                .collect::<Vec<_>>()
                .join(", ");
            self.audit_refusal(&request.session_id, "secrets_require_override", &paths);
            return Err(CommitError::refused(
                "commit_secrets_require_override",
                format!(
                    "The content that would be committed contains something the secret scanner \
                     flags ({paths}). The displayed diff is redacted; the commit is not. \
                     Review the files and approve with the override to commit them anyway."
                ),
            ));
        }
        if !secret_warnings.is_empty() {
            for warning in &secret_warnings {
                self.audit_log
                    .record(
                        "commit_secret_override",
                        &[
                            ("actor", "user".to_string()),
                            ("sessionId", request.session_id.clone()),
                            ("prepareId", request.prepare_id.clone()),
                            ("approvedBy", request.approved_by.clone()),
                            ("resourcePath", warning.path.clone()),
                            ("categories", warning.categories.join(",")),
                            ("count", warning.count.to_string()),
                        ],
                    )
                    .map_err(CommitError::from)?;
            }
        }
        // A commit is a side-effecting action with an unknown-outcome window
        // (spec 17). The synthetic task exists so a crash between the markers
        // classifies as `unknown_external_outcome` instead of being invisible;
        // it carries no plan and no model call.
        let task = self
            .session_store
            .create_task(
                &request.session_id,
                // Not a prompt: an empty one keeps a crash-interrupted commit
                // task from being offered a "resume" that would run a chat
                // turn. The recovery card explains the action instead.
                "",
                &self.config.model_provider,
                &self.config.model_name,
            )
            .map_err(CommitError::from)?;
        let task = self
            .session_store
            .update_task_status(&task, TaskStatus::RunningTool, None)
            .map_err(CommitError::from)?;
        let marker = self
            .session_store
            .start_action(&task, "commit", &record.prepare_id, true)
            .map_err(CommitError::from)?;
        record.task_id = Some(task.id.clone());
        record.selected_paths = selected
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect();
        let _ = self.save_record(&record);
        let private = self.private_dir(&record.prepare_id)?;
        let outcome = self.compose_and_commit(
            root,
            &mut record,
            &selected,
            &fresh,
            &message,
            &branch,
            &private,
            cancel,
            !secret_warnings.is_empty(),
        );
        let _ = fs::remove_dir_all(&private);
        match &outcome {
            Ok(_) => {
                let _ = self
                    .session_store
                    .update_task_status(&task, TaskStatus::Complete, None);
                let _ = self.session_store.finish_action(marker, "ok");
            }
            Err(error) => {
                // A rewritten message or a failed hook is a review outcome,
                // not a dead preparation: the user fixes it and approves the
                // same record again. Only an engine failure may have left the
                // repository in an unknown state, so only that retires it.
                if matches!(error, CommitError::Engine(_)) {
                    record.state = "failed".to_string();
                    let _ = self.save_record(&record);
                }
                let _ = self.session_store.update_task_status(
                    &task,
                    TaskStatus::Failed,
                    Some(&error.message()),
                );
                let _ = self.session_store.finish_action(marker, "failed");
            }
        }
        let outcome = outcome?;
        record.state = "created".to_string();
        record.prepared_commit = Some(outcome.commit_sha.clone());
        record.head_after = Some(outcome.commit_sha.clone());
        record.hook_outcomes = outcome.hook_outcomes.clone();
        record.generated_secret_override = outcome.generated_secret_override;
        record.hunk_count = outcome.hunk_count;
        let _ = self.save_record(&record);
        let _ = self.session_store.append_commit_created(
            &request.session_id,
            &crate::session::CommitSessionEvent {
                task_id: task.id.clone(),
                prepare_id: record.prepare_id.clone(),
                commit_sha: outcome.commit_sha.clone(),
                paths: selected
                    .iter()
                    .map(|candidate| candidate.path.clone())
                    .collect(),
            },
        );
        let hook_summary = outcome
            .hook_outcomes
            .iter()
            .map(|hook| format!("{}:{}", hook.name, hook.outcome))
            .collect::<Vec<_>>()
            .join(",");
        let _ = self.audit_log.record(
            "commit_created",
            &[
                ("actor", "user".to_string()),
                ("sessionId", request.session_id.clone()),
                ("prepareId", record.prepare_id.clone()),
                ("approvedBy", request.approved_by.clone()),
                ("commitSha", outcome.commit_sha.clone()),
                ("branch", branch.clone()),
                ("pathCount", outcome.path_count.to_string()),
                ("hunkCount", outcome.hunk_count.to_string()),
                ("messageLength", outcome.message.chars().count().to_string()),
                ("hooks", hook_summary),
                (
                    "generatedSecretOverride",
                    outcome.generated_secret_override.to_string(),
                ),
            ],
        );
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_and_commit(
        &self,
        root: &Path,
        record: &mut CommitRecord,
        selected: &[CommitCandidate],
        fresh: &[(CommitCandidate, Option<BlobInfo>)],
        message: &str,
        branch: &str,
        private: &Path,
        cancel: &CancelToken,
        generated_secret_override: bool,
    ) -> std::result::Result<CommitOutcome, CommitError> {
        let index = private.join("index");
        let expected_old = record.expected_old.clone();
        let head = expected_old.as_deref();
        let mut command = git(root, Some(&index));
        command.args(["read-tree"]);
        command.arg(if head.is_some() { "HEAD" } else { "--empty" });
        expect_success(command, "read-tree")?;

        let mut expected = self.tree_entries(root, head).map_err(CommitError::from)?;
        for (candidate, blob) in fresh {
            match blob {
                Some(blob) => {
                    let oid = self
                        .write_blob(root, &candidate.path, blob)
                        .map_err(CommitError::from)?;
                    if oid != blob.oid {
                        return Err(CommitError::Engine(format!(
                            "The blob for {} changed while it was being written.",
                            candidate.path
                        )));
                    }
                    let info = format!("{} {}\t{}\0", blob.mode, oid, candidate.path);
                    let mut command = git(root, Some(&index));
                    command.args(["update-index", "-z", "--index-info"]);
                    command.stdin(Stdio::piped());
                    let mut child = command.spawn().map_err(git_spawn_error)?;
                    child
                        .stdin
                        .as_mut()
                        .expect("stdin was piped")
                        .write_all(info.as_bytes())
                        .map_err(|error| CommitError::Engine(error.to_string()))?;
                    let output = child
                        .wait_with_output()
                        .map_err(|error| CommitError::Engine(error.to_string()))?;
                    if !output.status.success() {
                        return Err(CommitError::Engine(format!(
                            "update-index failed: {}",
                            String::from_utf8_lossy(&output.stderr).trim()
                        )));
                    }
                    expected.insert(candidate.path.clone(), (blob.mode.clone(), oid));
                }
                None => {
                    let mut command = git(root, Some(&index));
                    command.args(["update-index", "--force-remove", "--"]);
                    command.arg(&candidate.path);
                    expect_success(command, "update-index --force-remove")?;
                    expected.remove(&candidate.path);
                }
            }
        }
        let tree = {
            let mut command = git(root, Some(&index));
            command.arg("write-tree");
            let output = command
                .output()
                .map_err(|error| CommitError::Engine(error.to_string()))?;
            if !output.status.success() {
                return Err(CommitError::Engine(format!(
                    "write-tree failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        // Requirement 1 proved against the commit's tree, not the working
        // tree: the tree contains exactly the expected entries.
        let actual = self
            .tree_entries_for(root, Some(&tree))
            .map_err(CommitError::from)?;
        if actual != expected {
            return Err(CommitError::Engine(
                "The composed tree did not match the selected set; nothing was committed."
                    .to_string(),
            ));
        }
        let message_path = private.join("COMMIT_EDITMSG");
        fs::write(&message_path, format!("{message}\n"))
            .map_err(|error| CommitError::Engine(error.to_string()))?;
        let hooks = self
            .run_hooks(root, &index, &message_path, cancel, record)
            .map_err(CommitError::from)?;
        let final_message = fs::read_to_string(&message_path)
            .map_err(|error| CommitError::Engine(error.to_string()))?;
        let final_message = final_message.trim_end().to_string();
        if final_message != message.trim_end() {
            return Err(CommitError::MessageRewritten {
                message: final_message,
            });
        }
        let mut outcomes = hooks;
        if let Some(failure) = outcomes.iter().find(|hook| hook.outcome != "ok") {
            return Err(CommitError::HookFailed {
                name: failure.name.clone(),
                exit_code: failure.exit_code,
                termination: failure.termination.clone(),
                stdout: failure.stdout.clone(),
                stderr: failure.stderr.clone(),
            });
        }
        let mut command = git(root, Some(&index));
        command.args(["commit-tree", &tree]);
        if let Some(head) = head {
            command.args(["-p", head]);
        }
        command.args(["-F"]).arg(&message_path);
        let output = command
            .output()
            .map_err(|error| CommitError::Engine(error.to_string()))?;
        if !output.status.success() {
            return Err(CommitError::Engine(format!(
                "commit-tree failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // Recorded before the ref moves, so a crash in this window can still
        // be inspected: the prepared commit and the expected old are both on
        // disk (spec 35 §5.7).
        record.prepared_commit = Some(commit.clone());
        let _ = self.save_record(record);
        let now = self.git_state(root).map_err(CommitError::from)?;
        if now.head != record.expected_old {
            return Err(CommitError::refused(
                "commit_head_moved",
                "HEAD moved while the commit was being prepared. Nothing was committed.",
            ));
        }
        let old = record.expected_old.clone().unwrap_or_default();
        let mut command = git(root, None);
        command.args(["update-ref"]);
        command.arg(format!("refs/heads/{branch}"));
        command.arg(&commit);
        command.arg(old);
        let output = command
            .output()
            .map_err(|error| CommitError::Engine(error.to_string()))?;
        if !output.status.success() {
            return Err(CommitError::refused(
                "commit_head_moved",
                format!(
                    "HEAD moved while the commit was being created, so the ref was not \
                     updated. Nothing was committed. {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        let mut warnings = Vec::new();
        let head_now = self
            .git_line(root, &["rev-parse", "HEAD"])
            .map_err(CommitError::from)?
            .unwrap_or_default();
        if head_now != commit {
            // update-ref succeeded, so the commit exists; failing here would
            // report a false negative for a commit that is real.
            warnings.push(format!(
                "The commit was created as {commit}, but HEAD did not read back as it.                  Check the repository before relying on it."
            ));
        }
        // post-commit runs after the ref moves. A failure is a warning: the
        // commit is real, and pretending otherwise would be worse.
        if let Some(post) = self.run_post_commit(root, &index, cancel, record)? {
            outcomes.push(post.clone());
            if post.outcome == "failed" {
                warnings.push(format!(
                    "The post-commit hook failed ({}). The commit was created.",
                    post.exit_code
                        .map(|code| format!("exit code {code}"))
                        .unwrap_or_else(|| post.termination.clone())
                ));
            }
        }
        let hunk_count = selected
            .iter()
            .map(|candidate| candidate.accepted_hunks.len())
            .sum();
        Ok(CommitOutcome {
            commit_sha: commit,
            branch: branch.to_string(),
            message: final_message,
            path_count: selected.len(),
            hunk_count,
            hook_outcomes: outcomes,
            warnings,
            generated_secret_override,
        })
    }

    fn selected_candidates(
        &self,
        record: &CommitRecord,
        paths: &[String],
    ) -> std::result::Result<Vec<CommitCandidate>, CommitError> {
        if paths.is_empty() {
            return Err(CommitError::refused(
                "commit_selection_empty",
                "Select at least one file to commit.",
            ));
        }
        let unique = paths.iter().collect::<BTreeSet<_>>();
        if unique.len() != paths.len() {
            return Err(CommitError::refused(
                "commit_selection_invalid",
                "A file was selected twice.",
            ));
        }
        let mut selected = Vec::new();
        for path in paths {
            let Some(candidate) = record
                .candidates
                .iter()
                .find(|candidate| &candidate.path == path)
            else {
                return Err(CommitError::refused(
                    "commit_selection_invalid",
                    format!("{path} was not part of this preparation."),
                ));
            };
            if candidate.source == CommitCandidateSource::Conflict {
                return Err(CommitError::refused(
                    "commit_git_state",
                    format!("{path} has an unresolved conflict."),
                ));
            }
            selected.push(candidate.clone());
        }
        Ok(selected)
    }

    /// Everything a commit needs about the repository's state, and why a commit
    /// cannot happen when it cannot.
    fn git_state(&self, root: &Path) -> Result<GitState> {
        let inside = self.git_line(root, &["rev-parse", "--is-inside-work-tree"])?;
        if inside.as_deref() != Some("true") {
            return Ok(GitState {
                branch: None,
                head: None,
                refusal: Some("This folder is not a Git repository.".to_string()),
            });
        }
        let branch = self.git_line(root, &["symbolic-ref", "--short", "-q", "HEAD"])?;
        let head = self.git_line(root, &["rev-parse", "--verify", "-q", "HEAD"])?;
        let top = self
            .git_line(root, &["rev-parse", "--show-toplevel"])?
            .map(PathBuf::from)
            .unwrap_or_else(|| root.to_path_buf());
        let mut refusal = None;
        if branch.is_none() {
            refusal = Some("HEAD is detached. Check out a branch before committing.".to_string());
        } else if let Some(branch) = &branch {
            // A branch checked out in another worktree is that worktree's
            // HEAD. Moving it would change a checkout this session is not in.
            if let Ok(entries) = self.git.worktrees(root) {
                let canonical = fs::canonicalize(&top).unwrap_or_else(|_| top.clone());
                let elsewhere = entries.iter().any(|entry| {
                    entry.branch.as_deref() == Some(branch.as_str())
                        && fs::canonicalize(&entry.path)
                            .map(|path| path != canonical)
                            .unwrap_or(false)
                });
                if elsewhere {
                    refusal = Some(format!(
                        "The branch {branch} is checked out in another worktree. Commit there, \
                         or switch this worktree to its own branch."
                    ));
                }
            }
        }
        for marker in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-apply",
            "sequencer",
        ] {
            let path = self.git_path(root, marker)?;
            if path.exists() {
                refusal = Some(format!(
                    "A Git operation is in progress ({marker}). Finish or abort it before \
                     committing."
                ));
                break;
            }
        }
        if refusal.is_none() && self.git.changes(root)?.iter().any(|file| file.conflicted) {
            refusal = Some(
                "The working tree has unresolved conflicts. Resolve them before committing."
                    .to_string(),
            );
        }
        if refusal.is_none() {
            let identity = self.git_output(root, &["var", "GIT_COMMITTER_IDENT"])?;
            if !identity.status.success() {
                refusal = Some(
                    "Git has no committer identity. Set user.name and user.email in this \
                     repository or globally; Damaian does not set them for you."
                        .to_string(),
                );
            }
        }
        Ok(GitState {
            branch,
            head,
            refusal,
        })
    }

    fn refusal(&self, root: &Path, session_id: &str) -> Result<Option<String>> {
        let state = self.git_state(root)?;
        self.refusal_from_state(root, session_id, &state)
    }

    fn refusal_from_state(
        &self,
        _root: &Path,
        session_id: &str,
        state: &GitState,
    ) -> Result<Option<String>> {
        if self.session_store.read_session(session_id)?.is_none() {
            return Ok(Some(
                "Unknown session. Open a session before committing.".to_string(),
            ));
        }
        let mode = self.session_store.session_mode(session_id);
        if mode != SessionMode::Code {
            return Ok(Some(format!(
                "Committing is available in Code mode only. This session is in {} mode.",
                mode.label()
            )));
        }
        let capabilities = self.config.profile_capabilities();
        if !capabilities.allow_git_commits {
            return Ok(Some(
                "The permission profile does not allow Git commits (allow_git_commits=false). \
                 Switching mode will not allow it."
                    .to_string(),
            ));
        }
        Ok(state.refusal.clone())
    }

    /// The session's uncommitted accepted changes, then everything else the
    /// working tree reports, each with the label that explains why it is there.
    fn candidates(&self, root: &Path, session_id: &str) -> Result<Vec<CommitCandidate>> {
        let changes = self.git.changes(root)?;
        if changes.is_empty() {
            return Ok(Vec::new());
        }
        let mut pending = BTreeMap::<String, PendingAcceptance>::new();
        for event in self.session_store.read_change_timeline(session_id)? {
            match event {
                ChangeEvent::Applied {
                    paths,
                    applied_hashes,
                    accepted_hunks,
                } => {
                    for path in paths {
                        pending.insert(
                            path.clone(),
                            PendingAcceptance {
                                applied_hash: applied_hashes.get(&path).cloned(),
                                hunks: accepted_hunks.get(&path).cloned().unwrap_or_default(),
                            },
                        );
                    }
                }
                ChangeEvent::Committed { paths } => {
                    for path in paths {
                        pending.remove(&path);
                    }
                }
            }
        }
        let checkpoint = self.checkpoint_paths(root, session_id)?;
        let mut candidates = Vec::new();
        for change in changes {
            let path = change.path.clone();
            let acceptance = pending.get(&path);
            let checkpoint_entry = checkpoint.get(&path);
            let blob = self.blob_info(root, &path)?;
            // Nothing in the working tree differs from HEAD: there is nothing
            // to commit at this path, whatever the index may say. The user's
            // staged copy is deliberately not the commit's base.
            if let Some(blob) = &blob
                && self.blob_at_head(root, &path)?.as_deref() == Some(blob.oid.as_str())
            {
                continue;
            }
            let (content_sha256, warnings) = match &blob {
                Some(blob) if !blob.symlink => {
                    if blob.size > self.config.max_file_bytes {
                        // Too large to scan, but still hashed: the acceptance
                        // record is a SHA-256, and refusing to recognise an
                        // accepted file because it is big would mislabel it as
                        // hand-edited. Reading it once to hash is the price of
                        // committing it at all.
                        (
                            Some(crate::hash::file_hash(root.join(&path))?),
                            vec![GeneratedSecretWarning {
                                path: path.clone(),
                                categories: vec!["unscanned".to_string()],
                                count: 1,
                            }],
                        )
                    } else {
                        let bytes = fs::read(root.join(&path))?;
                        (
                            Some(crate::hash::sha256(&bytes)),
                            self.scan_bytes(&path, &bytes),
                        )
                    }
                }
                Some(blob) => (blob.content_sha256.clone(), Vec::new()),
                None => (None, Vec::new()),
            };
            let status = match &blob {
                Some(_) => {
                    if self.head_has(root, &path)? {
                        "modified"
                    } else {
                        "added"
                    }
                }
                None => "deleted",
            };
            let source = if change.conflicted {
                CommitCandidateSource::Conflict
            } else if let Some(acceptance) = acceptance {
                let recorded = acceptance.applied_hash.as_deref();
                let matches = match recorded {
                    // A deletion records an empty hash, and the file is gone.
                    Some(hash) if hash.is_empty() && blob.is_none() => true,
                    // A recorded hash is SHA-256 of what Damaian wrote.
                    Some(hash) => content_sha256.as_deref() == Some(hash),
                    // A decision written before spec 35 has no hash; fall back
                    // to what the checkpoint says Damaian last left.
                    None => {
                        checkpoint_entry
                            .and_then(|entry| entry.expected_hash.as_deref())
                            .is_some_and(|hash| content_sha256.as_deref() == Some(hash))
                            || (blob.is_none() && acceptance.hunks.is_empty())
                    }
                };
                if matches {
                    CommitCandidateSource::AcceptedPatch
                } else {
                    CommitCandidateSource::AcceptedThenEdited
                }
            } else if let Some(entry) = checkpoint_entry {
                match entry.origin {
                    CheckpointOrigin::Command => CommitCandidateSource::CommandEffect,
                    CheckpointOrigin::Patch => {
                        if content_sha256.as_deref() == entry.expected_hash.as_deref() {
                            CommitCandidateSource::AcceptedPatch
                        } else {
                            CommitCandidateSource::DamaianChange
                        }
                    }
                }
            } else if change.untracked {
                CommitCandidateSource::Untracked
            } else {
                CommitCandidateSource::External
            };
            let hunks = acceptance
                .map(|acceptance| acceptance.hunks.clone())
                .unwrap_or_default();
            candidates.push(CommitCandidate {
                path,
                status: if change.conflicted {
                    "conflicted".to_string()
                } else {
                    status.to_string()
                },
                source,
                label: source.label().to_string(),
                selected_by_default: source.selected_by_default(),
                blob_oid: blob.as_ref().map(|blob| blob.oid.clone()),
                content_sha256,
                mode: blob.as_ref().map(|blob| blob.mode.clone()),
                size: blob.as_ref().map(|blob| blob.size).unwrap_or(0),
                unscanned: warnings.iter().any(|warning| {
                    warning
                        .categories
                        .iter()
                        .any(|category| category == "unscanned")
                }),
                secret_warnings: warnings,
                diff: String::new(),
                accepted_hunks: hunks,
            });
        }
        candidates.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(candidates)
    }

    fn path_is_restricted(&self, path: &str) -> bool {
        self.path_policy.is_restricted(path, false)
            || path
                .match_indices('/')
                .any(|(at, _)| self.path_policy.is_restricted(&path[..at], true))
    }

    fn checkpoint_paths(
        &self,
        root: &Path,
        session_id: &str,
    ) -> Result<BTreeMap<String, crate::checkpoint::CheckpointFile>> {
        let repository_id = repository_id_for_root(root);
        let manifests = self.checkpoint_store.list_checkpoints(&repository_id)?;
        let mut paths = BTreeMap::new();
        for manifest in manifests {
            if manifest.session_id != session_id {
                continue;
            }
            for file in manifest.files {
                paths.entry(file.path.clone()).or_insert(file);
            }
        }
        Ok(paths)
    }

    fn scan_bytes(&self, path: &str, bytes: &[u8]) -> Vec<GeneratedSecretWarning> {
        let text = String::from_utf8_lossy(bytes);
        let findings = self.scanner.scan(&text);
        if findings.is_empty() {
            return Vec::new();
        }
        let mut categories = findings
            .iter()
            .map(|finding| finding.category.clone())
            .collect::<Vec<_>>();
        categories.sort();
        categories.dedup();
        vec![GeneratedSecretWarning {
            path: path.to_string(),
            categories,
            count: findings.len(),
        }]
    }

    fn suggest_message(&self, session_id: &str, selected_paths: &[String]) -> String {
        let summary = self
            .session_store
            .read_messages(session_id)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .find(|message| message.role == "user" && message.kind.is_none())
            .map(|message| message.content)
            .unwrap_or_default();
        self.git.suggest_commit_message(&summary, selected_paths)
    }

    fn verification(
        &self,
        root: &Path,
        session_id: &str,
        candidates: &[CommitCandidate],
    ) -> Result<CommitVerification> {
        let events = self
            .session_store
            .read_verification_events(session_id, &[])?;
        if events.is_empty() {
            return Ok(CommitVerification {
                summary: "No check evidence was recorded for this session.".to_string(),
                ..CommitVerification::default()
            });
        }
        let state = fold_verification(events.iter());
        let plans = self.session_store.read_session_plans(session_id)?;
        let tasks = self.session_store.read_tasks(session_id)?;
        let latest = tasks.last().map(|task| task.id.clone()).unwrap_or_default();
        let plan = plans
            .get(&latest)
            .cloned()
            .unwrap_or_else(|| TaskPlan::new(latest, 0));
        let mut current_hashes = BTreeMap::new();
        let mut paths = candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect::<BTreeSet<_>>();
        for run in state.checks.values() {
            paths.extend(run.file_hashes.keys().cloned());
        }
        for path in paths {
            if let Ok(Some(hash)) = self.hash_current(root, &path) {
                current_hashes.insert(path, hash);
            }
        }
        let files = candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect::<Vec<_>>();
        let report = VerificationReport::build(&state, &plan, &files, &current_hashes);
        Ok(CommitVerification {
            due: report.due,
            passed: report.passed.iter().map(|check| check.label()).collect(),
            failed: report
                .failed
                .iter()
                .map(|check| format!("{} — {}", check.check.label(), check.reason))
                .collect(),
            skipped: report
                .skipped
                .iter()
                .map(|check| check.check.label())
                .collect(),
            stale: report.stale.iter().map(|check| check.label()).collect(),
            summary: verification_summary(&report),
        })
    }

    fn hash_current(&self, root: &Path, path: &str) -> Result<Option<String>> {
        let full = root.join(path);
        if !full.exists() {
            return Ok(None);
        }
        Ok(Some(crate::hash::file_hash(full)?))
    }

    fn diff_for(&self, root: &Path, path: &str, status: &str, has_head: bool) -> Result<String> {
        if has_head {
            let diff = self.git.diff_in(
                root,
                None,
                &DiffOptions {
                    paths: vec![path.to_string()],
                    ..DiffOptions::default()
                },
            )?;
            if !diff.trim().is_empty() {
                return Ok(diff);
            }
        }
        if status == "deleted" {
            return Ok(format!("[Deleted file: {path}]\n"));
        }
        let full = root.join(path);
        let metadata = match fs::symlink_metadata(&full) {
            Ok(metadata) => metadata,
            Err(_) => return Ok(String::new()),
        };
        if metadata.len() > self.config.max_file_bytes {
            return Ok(format!(
                "[File is {} bytes; the diff is not shown here.]\n",
                metadata.len()
            ));
        }
        let bytes = fs::read(&full)?;
        if bytes[..bytes.len().min(8 * 1024)].contains(&0) {
            return Ok(format!("[Binary file: {} bytes]\n", bytes.len()));
        }
        let text = String::from_utf8_lossy(&bytes);
        let diff = diff_file("", &text, path).text;
        Ok(self.scanner.redact(&diff).text)
    }

    fn hook_report(&self, root: &Path) -> Result<GitHookReport> {
        let (directory, source) = self.hook_directory(root)?;
        let mut hooks = Vec::new();
        for name in GIT_COMMIT_HOOKS {
            let path = directory.join(name);
            let metadata = fs::symlink_metadata(&path).ok();
            let exists = metadata.is_some();
            let is_file = metadata.as_ref().is_some_and(|meta| meta.is_file());
            let executable = metadata
                .as_ref()
                .is_some_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
            let size = metadata.as_ref().map(|meta| meta.len()).unwrap_or(0);
            let interpreter = if is_file {
                fs::read(&path).ok().and_then(|bytes| {
                    let first = bytes
                        .split(|&byte| byte == b'\n')
                        .next()
                        .map(String::from_utf8_lossy)
                        .map(|line| line.trim().to_string());
                    first
                        .filter(|line| line.starts_with("#!"))
                        .map(|line| line.chars().take(120).collect::<String>())
                })
            } else {
                None
            };
            let runs = exists && is_file && executable;
            let skipped_reason = if exists && !is_file {
                Some("not a file".to_string())
            } else if exists && !executable {
                Some("not executable, so Git would not run it".to_string())
            } else {
                None
            };
            hooks.push(GitHookInfo {
                name: name.to_string(),
                path: path.to_string_lossy().to_string(),
                exists,
                executable,
                size,
                interpreter,
                runs,
                skipped_reason,
            });
        }
        Ok(GitHookReport {
            directory: directory.to_string_lossy().to_string(),
            source,
            hooks,
        })
    }

    fn hook_directory(&self, root: &Path) -> Result<(PathBuf, String)> {
        let configured = self
            .git_line(root, &["config", "--get", "core.hooksPath"])?
            .filter(|value| !value.trim().is_empty());
        if let Some(configured) = configured {
            let path = PathBuf::from(&configured);
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            return Ok((path, "core.hooksPath".to_string()));
        }
        let relative = self
            .git_line(root, &["rev-parse", "--git-path", "hooks"])?
            .unwrap_or_else(|| ".git/hooks".to_string());
        let path = PathBuf::from(relative);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        Ok((path, "default".to_string()))
    }

    /// Runs `pre-commit`, `prepare-commit-msg` and `commit-msg`, in Git's
    /// order, with the private index in the environment. Returns every outcome;
    /// the caller decides which failure aborts.
    fn run_hooks(
        &self,
        root: &Path,
        index: &Path,
        message_path: &Path,
        cancel: &CancelToken,
        record: &CommitRecord,
    ) -> Result<Vec<CommitHookOutcome>> {
        let mut outcomes = Vec::new();
        let env = self.hook_env(root, index)?;
        for (name, args) in [
            ("pre-commit", Vec::new()),
            (
                "prepare-commit-msg",
                vec![
                    message_path.to_string_lossy().to_string(),
                    "message".to_string(),
                ],
            ),
            (
                "commit-msg",
                vec![message_path.to_string_lossy().to_string()],
            ),
        ] {
            let Some(hook) = record.hooks.hooks.iter().find(|hook| hook.name == name) else {
                continue;
            };
            if !hook.runs {
                continue;
            }
            let outcome = self.run_one_hook(
                root,
                &hook.path,
                &args,
                name,
                &env,
                cancel,
                &record.prepare_id,
            )?;
            let failed = outcome.outcome != "ok";
            outcomes.push(outcome);
            if failed {
                // Return now: the caller turns the last `failed` outcome into
                // the typed error. Later hooks must not run.
                return Ok(outcomes);
            }
        }
        Ok(outcomes)
    }

    fn run_post_commit(
        &self,
        root: &Path,
        index: &Path,
        cancel: &CancelToken,
        record: &CommitRecord,
    ) -> std::result::Result<Option<CommitHookOutcome>, CommitError> {
        let Some(hook) = record
            .hooks
            .hooks
            .iter()
            .find(|hook| hook.name == "post-commit")
        else {
            return Ok(None);
        };
        if !hook.runs {
            return Ok(None);
        }
        let env = self.hook_env(root, index).map_err(CommitError::from)?;
        let outcome = self
            .run_one_hook(
                root,
                &hook.path,
                &[],
                "post-commit",
                &env,
                cancel,
                &record.prepare_id,
            )
            .map_err(CommitError::from)?;
        Ok(Some(outcome))
    }

    #[allow(clippy::too_many_arguments)]
    fn run_one_hook(
        &self,
        root: &Path,
        path: &str,
        args: &[String],
        name: &str,
        env: &[(&str, String)],
        cancel: &CancelToken,
        prepare_id: &str,
    ) -> Result<CommitHookOutcome> {
        let mut command = shell_quote(path);
        for arg in args {
            command.push(' ');
            command.push_str(&shell_quote(arg));
        }
        let mut on_output = |_chunk: &str| {};
        let execution = self.command_runner.run_at_with_env(
            &command,
            root,
            root,
            &format!("Git hook {name}"),
            CommandRunOptions {
                approved: true,
                approved_by: Some("git_hook"),
                task_id: Some(prepare_id),
                cancel,
                on_output: &mut on_output,
                purpose: if name == "pre-commit" {
                    CommandPurpose::Check
                } else {
                    CommandPurpose::Other
                },
            },
            env,
        )?;
        let outcome = match execution.termination.as_str() {
            "exited" if execution.exit_code == Some(0) => "ok",
            "timed_out" => "timed_out",
            "cancelled" => "cancelled",
            _ => "failed",
        };
        Ok(CommitHookOutcome {
            name: name.to_string(),
            outcome: outcome.to_string(),
            exit_code: execution.exit_code,
            termination: execution.termination.as_str().to_string(),
            stdout: execution.stdout,
            stderr: execution.stderr,
        })
    }

    fn hook_env(&self, root: &Path, index: &Path) -> Result<Vec<(&'static str, String)>> {
        let git_dir = self
            .git_line(root, &["rev-parse", "--absolute-git-dir"])?
            .unwrap_or_else(|| root.join(".git").to_string_lossy().to_string());
        let top = self
            .git_line(root, &["rev-parse", "--show-toplevel"])?
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        Ok(vec![
            ("GIT_DIR", git_dir),
            ("GIT_WORK_TREE", top),
            ("GIT_INDEX_FILE", index.to_string_lossy().to_string()),
        ])
    }

    fn private_dir(&self, prepare_id: &str) -> Result<PathBuf> {
        let dir = self.config.data_dir.join("commits").join(prepare_id);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        // `GIT_INDEX_FILE` and hook arguments are interpreted by child
        // processes whose working directory is the repository, so a relative
        // `data_dir` (the documented `DAMAIAN_DATA_DIR=.damaian` form) must be
        // resolved here, not left relative.
        Ok(fs::canonicalize(&dir)?)
    }

    fn record_path(&self, prepare_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("commits")
            .join(format!("{prepare_id}.json"))
    }

    fn save_record(&self, record: &CommitRecord) -> Result<()> {
        let path = self.record_path(&record.prepare_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(record)
            .map_err(|error| ClientError::Io(format!("commit record serialization: {error}")))?;
        fs::write(path, content)?;
        Ok(())
    }

    fn audit_refusal(&self, session_id: &str, reason: &str, detail: &str) {
        let _ = self.audit_log.record(
            "commit_refused",
            &[
                ("actor", "user".to_string()),
                ("sessionId", session_id.to_string()),
                ("reason", reason.to_string()),
                ("detail", detail.to_string()),
            ],
        );
    }

    /// A file's blob identity without writing anything. A symlink's blob is
    /// its target, not the file it points at.
    fn blob_info(&self, root: &Path, path: &str) -> Result<Option<BlobInfo>> {
        let full = root.join(path);
        let metadata = match fs::symlink_metadata(&full) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let symlink = metadata.file_type().is_symlink();
        if !symlink && !metadata.is_file() {
            return Ok(None);
        }
        let (oid, content_sha256) = if symlink {
            let target = fs::read_link(&full)?;
            let bytes = target.as_os_str().as_bytes();
            (
                self.hash_object_stdin(root, bytes, false)?,
                Some(crate::hash::sha256(bytes)),
            )
        } else {
            (self.hash_object_path(root, path, false)?, None)
        };
        let mode = match self.head_entry(root, path)? {
            Some((mode, _)) => mode,
            None if symlink => "120000".to_string(),
            None => {
                if metadata.permissions().mode() & 0o111 != 0 {
                    "100755".to_string()
                } else {
                    "100644".to_string()
                }
            }
        };
        Ok(Some(BlobInfo {
            oid,
            mode,
            size: metadata.len(),
            symlink,
            content_sha256,
        }))
    }

    fn write_blob(&self, root: &Path, path: &str, blob: &BlobInfo) -> Result<String> {
        if blob.symlink {
            let target = fs::read_link(root.join(path))?;
            self.hash_object_stdin(root, target.as_os_str().as_bytes(), true)
        } else {
            self.hash_object_path(root, path, true)
        }
    }

    fn hash_object_path(&self, root: &Path, path: &str, write: bool) -> Result<String> {
        let mut command = git(root, None);
        command.arg("hash-object");
        if write {
            command.arg("-w");
        }
        command.arg("--").arg(path);
        let output = command.output()?;
        if !output.status.success() {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn hash_object_stdin(&self, root: &Path, bytes: &[u8], write: bool) -> Result<String> {
        // `-C root` so `-w` writes to this repository's object directory, not
        // whatever the shell's current directory happens to be.
        let mut command = git(root, None);
        command.args(["hash-object"]);
        if write {
            command.arg("-w");
        }
        command.arg("--stdin");
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let mut child = command.spawn()?;
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(bytes)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn head_entry(&self, root: &Path, path: &str) -> Result<Option<(String, String)>> {
        if self
            .git_line(root, &["rev-parse", "--verify", "-q", "HEAD"])?
            .is_none()
        {
            return Ok(None);
        }
        let mut command = git(root, None);
        command.args(["ls-tree", "-z", "HEAD", "--"]).arg(path);
        let output = command.output()?;
        if !output.status.success() {
            return Ok(None);
        }
        let record = output
            .stdout
            .split(|&byte| byte == 0)
            .find(|record| !record.is_empty());
        let Some(record) = record else {
            return Ok(None);
        };
        let text = String::from_utf8_lossy(record);
        let Some((meta, _name)) = text.split_once('\t') else {
            return Ok(None);
        };
        let mut fields = meta.split_whitespace();
        let mode = fields.next().unwrap_or_default().to_string();
        let _kind = fields.next();
        let oid = fields.next().unwrap_or_default().to_string();
        Ok(Some((mode, oid)))
    }

    fn head_has(&self, root: &Path, path: &str) -> Result<bool> {
        Ok(self.head_entry(root, path)?.is_some())
    }

    fn blob_at_head(&self, root: &Path, path: &str) -> Result<Option<String>> {
        Ok(self.head_entry(root, path)?.map(|(_, oid)| oid))
    }

    fn tree_entries(
        &self,
        root: &Path,
        head: Option<&str>,
    ) -> Result<BTreeMap<String, (String, String)>> {
        self.tree_entries_for(root, head)
    }

    fn tree_entries_for(
        &self,
        root: &Path,
        revision: Option<&str>,
    ) -> Result<BTreeMap<String, (String, String)>> {
        let Some(revision) = revision else {
            return Ok(BTreeMap::new());
        };
        let mut command = git(root, None);
        command
            .args(["ls-tree", "-r", "-z", "--full-tree"])
            .arg(revision);
        let output = command.output()?;
        if !output.status.success() {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let mut entries = BTreeMap::new();
        for record in output.stdout.split(|&byte| byte == 0) {
            if record.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(record);
            let Some((meta, path)) = text.split_once('\t') else {
                continue;
            };
            let mut fields = meta.split_whitespace();
            let mode = fields.next().unwrap_or_default().to_string();
            let _kind = fields.next();
            let oid = fields.next().unwrap_or_default().to_string();
            entries.insert(path.to_string(), (mode, oid));
        }
        Ok(entries)
    }

    fn git_path(&self, root: &Path, name: &str) -> Result<PathBuf> {
        let value = self
            .git_line(root, &["rev-parse", "--git-path", name])?
            .unwrap_or_else(|| name.to_string());
        let path = PathBuf::from(value);
        Ok(if path.is_absolute() {
            path
        } else {
            root.join(path)
        })
    }

    fn git_line(&self, root: &Path, args: &[&str]) -> Result<Option<String>> {
        let output = self.git_output(root, args)?;
        let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((output.status.success() && !line.is_empty()).then_some(line))
    }

    fn git_output(&self, root: &Path, args: &[&str]) -> Result<Output> {
        let mut command = git(root, None);
        command.args(args);
        Ok(command.output()?)
    }
}

struct PendingAcceptance {
    applied_hash: Option<String>,
    hunks: Vec<String>,
}

fn verification_summary(report: &VerificationReport) -> String {
    if report.due {
        return "Verification is due and has not run.".to_string();
    }
    if report.passed.is_empty()
        && report.failed.is_empty()
        && report.stale.is_empty()
        && report.skipped.is_empty()
    {
        return "No passing check evidence.".to_string();
    }
    let mut parts = Vec::new();
    if !report.passed.is_empty() {
        parts.push(format!("{} checks passed", report.passed.len()));
    }
    if !report.failed.is_empty() {
        parts.push(format!("{} failed", report.failed.len()));
    }
    if !report.stale.is_empty() {
        parts.push(format!("{} stale", report.stale.len()));
    }
    if !report.skipped.is_empty() {
        parts.push(format!("{} skipped", report.skipped.len()));
    }
    format!("Verification: {}.", parts.join(", "))
}

fn git(root: &Path, index: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false", "-C"])
        .arg(root)
        .stdin(Stdio::null());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    command
}

fn expect_success(mut command: Command, step: &str) -> std::result::Result<(), CommitError> {
    let output = command
        .output()
        .map_err(|error| CommitError::Engine(format!("{step} could not run: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CommitError::Engine(format!(
            "{step} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn git_spawn_error(error: std::io::Error) -> CommitError {
    CommitError::Engine(format!("git could not start: {error}"))
}

/// Single-quote a path for the configured shell. Hooks are executed through
/// the shell path `CommandRunner` already uses, so a path with a space or a
/// quote must arrive as one word.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Kept so a test can prove the private index is a file under the data
/// directory rather than the repository's own, without duplicating the layout.
#[cfg(test)]
fn private_index_path(data_dir: &Path, prepare_id: &str) -> PathBuf {
    data_dir.join("commits").join(prepare_id).join("index")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hook_path_is_quoted_as_one_shell_word() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn the_private_index_lives_under_the_data_directory() {
        let path = private_index_path(Path::new("/data"), "commit_x");
        assert_eq!(path, Path::new("/data/commits/commit_x/index"));
        assert!(!path.starts_with(".git"));
    }

    #[test]
    fn a_candidate_label_explains_an_edited_acceptance() {
        assert!(
            CommitCandidateSource::AcceptedThenEdited
                .label()
                .contains("edited by hand")
        );
        assert!(!CommitCandidateSource::AcceptedThenEdited.selected_by_default());
        assert!(CommitCandidateSource::AcceptedPatch.selected_by_default());
    }
}
