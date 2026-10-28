//! Branch naming and branch delivery (spec 36).
//!
//! Naming does not change a ref and is kept here rather than in
//! `git_service.rs` because it is a Damaian convention built on Git's
//! validation, not another Git read. The delivery half is the prepared record
//! and the three mutations this spec owns: creating a branch, switching to one,
//! and deleting one. Creation runs `git branch` and never switches; switching
//! runs `git switch --no-guess`, so the index and working tree move with `HEAD`
//! (spec 36 `context.md` §3.17, decision D6). Deletion runs `git branch -d`
//! unless the commits that would be lost were separately confirmed, in which
//! case `-D` is the one exception (proposal §5.6).
//!
//! Every mutation is a Code-mode capability (`allow_git_branches`), is
//! approved as one exact [`GitMutation`] value, and is consumed once. The
//! mutation itself goes through `checkpoint::run_user_git`, the same runner
//! spec 57's worktrees use, so the recorded argv cannot drift from what ran
//! (decision D9).

use crate::audit::AuditLog;
use crate::checkpoint::{run_user_git, user_git_command_line};
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::git_service::{BranchInfo, GitService};
use crate::hash::{create_id, now_millis};
use crate::mode::SessionMode;
use crate::path_policy::PathPolicy;
use crate::secret_scanner::SecretScanner;
use crate::session::{SessionStore, TaskStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A branch slug stops here so the composed name stays comfortably under Git's
/// 255-byte ref-component limit even with a long configured prefix.
pub const BRANCH_SLUG_MAX_CHARS: usize = 50;

/// A prefix is naming, not a document; anything longer is a configuration
/// mistake rather than a convention.
const BRANCH_PREFIX_MAX_CHARS: usize = 200;

/// How many `-2`, `-3`, … alternatives are searched before giving up. A
/// collision this deep is almost certainly a naming loop, not a branch.
const MAX_COLLISION_SUFFIX: u32 = 99;

/// Decision D13: a rebase replays commits one at a time, which `merge-tree`
/// does not model, so the report says so rather than inventing a prediction.
const REBASE_CONFLICT_PREDICTION_UNAVAILABLE: &str = concat!(
    "Conflict prediction for a rebase is unavailable because it would require replaying ",
    "the commits one by one. Nothing was replayed."
);

/// A suggested branch name and what the user needs to know before accepting it.
/// The name is never silently changed (spec 36 §5.2): a collision leaves `name`
/// as it was and offers `alternative`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSuggestion {
    pub name: String,
    /// Whether Git accepts `name`.
    pub valid: bool,
    /// Git's refusal text, when `valid` is false.
    pub refusal: Option<String>,
    /// Whether `name` already exists locally.
    pub exists: bool,
    /// The first free `<name>-N`, when `exists` is true.
    pub alternative: Option<String>,
}

/// Lowercase ASCII words joined by `-`, at most [`BRANCH_SLUG_MAX_CHARS`]
/// characters, and `change` when nothing is left (spec 36 §5.2). Every
/// non-alphanumeric run collapses to one `-`; non-ASCII characters are
/// separators rather than content, matching `worktree::worker_slug`.
pub fn slugify_branch_title(text: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(character.to_ascii_lowercase());
        } else {
            separator = true;
        }
    }
    if slug.len() > BRANCH_SLUG_MAX_CHARS {
        slug.truncate(BRANCH_SLUG_MAX_CHARS);
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "change".to_string()
    } else {
        slug
    }
}

/// Whether `prefix` can be put in front of a slug without changing what the
/// name means. This runs before `git check-ref-format --branch`, because
/// `--branch` expands `@{-N}` and similar syntax: a prefix containing `@{`
/// could be interpreted as a previous branch instead of validated as text
/// (spec 36 `context.md` §3.14).
///
/// A trailing `/` is allowed and expected (`damaian/`); an empty prefix is
/// allowed and simply means no prefix.
pub fn validate_branch_prefix(prefix: &str) -> Result<()> {
    if prefix.is_empty() {
        return Ok(());
    }
    let invalid = prefix.chars().count() > BRANCH_PREFIX_MAX_CHARS
        || prefix.starts_with('-')
        || prefix.starts_with('/')
        || prefix.ends_with('.')
        || prefix.contains("..")
        || prefix.contains("@{")
        || prefix.contains("//")
        || prefix.chars().any(|character| {
            character.is_control()
                || matches!(character, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
        || prefix.trim_end_matches('/').split('/').any(|part| {
            part.is_empty()
                || part.starts_with('.')
                || part.ends_with('.')
                || part.ends_with(".lock")
        });
    if invalid {
        return Err(ClientError::InvalidInput(format!(
            "`{prefix}` is not a branch-name prefix Git would accept"
        )));
    }
    Ok(())
}

/// Suggest a branch name from the task title or the user's request.
///
/// `source` is free text and can contain a credential, so it is redacted before
/// it is slugged and before anything is returned (spec 36 `context.md` §3.15,
/// decision D10). The composed name is validated by Git and checked against the
/// local branches; a collision is reported with a free alternative rather than
/// silently suffixing the name.
pub fn suggest_branch_name(
    git: &GitService,
    scanner: &SecretScanner,
    root: impl AsRef<Path>,
    prefix: &str,
    source: &str,
) -> Result<BranchSuggestion> {
    let root = root.as_ref();
    validate_branch_prefix(prefix)?;
    let redacted = scanner.redact(source);
    let name = format!("{prefix}{}", slugify_branch_title(&redacted.text));
    checked_suggestion(git, root, name)
}

/// Git's answer about an already-composed name, plus the free alternative when
/// that name exists. The name itself is never altered (spec 36 §5.2), so both
/// [`suggest_branch_name`] and an explicitly typed name go through this one
/// collision rule.
fn checked_suggestion(git: &GitService, root: &Path, name: String) -> Result<BranchSuggestion> {
    let check = git.check_branch_name(root, &name)?;
    if !check.valid {
        return Ok(BranchSuggestion {
            name,
            valid: false,
            refusal: check.reason,
            exists: false,
            alternative: None,
        });
    }
    let branches = git.local_branches(root)?;
    if !branches.contains(&name) {
        return Ok(BranchSuggestion {
            name,
            valid: true,
            refusal: None,
            exists: false,
            alternative: None,
        });
    }
    let alternative = (2..=MAX_COLLISION_SUFFIX)
        .map(|suffix| format!("{name}-{suffix}"))
        .find(|candidate| !branches.contains(candidate));
    Ok(BranchSuggestion {
        name,
        valid: true,
        refusal: None,
        exists: true,
        alternative,
    })
}

/// One variant per branch operation. There is deliberately no `Other` and no
/// variant that covers two operations, so an approval for one can never apply
/// another (spec 36 §5.5, decisions D3 and D14).
///
/// `start_point` is the **resolved** commit object id, not the name the user
/// typed: the approval then names the exact commit that was reviewed, and a
/// ref that moves afterwards cannot silently become the start point.
///
/// `unmerged_commits` is `None` when the default branch could not be resolved
/// (decision D11). The count is then unknown, and the deletion may only proceed
/// through the separate unmerged confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GitMutation {
    #[serde(rename_all = "camelCase")]
    CreateBranch { name: String, start_point: String },
    #[serde(rename_all = "camelCase")]
    SwitchBranch { name: String },
    #[serde(rename_all = "camelCase")]
    DeleteBranch {
        name: String,
        unmerged_commits: Option<u32>,
    },
}

impl GitMutation {
    /// The branch this mutation names.
    pub fn name(&self) -> &str {
        match self {
            Self::CreateBranch { name, .. }
            | Self::SwitchBranch { name }
            | Self::DeleteBranch { name, .. } => name,
        }
    }

    /// The action marker and audit verb for this operation.
    fn action(&self) -> &'static str {
        match self {
            Self::CreateBranch { .. } => "git_branch_create",
            Self::SwitchBranch { .. } => "git_branch_switch",
            Self::DeleteBranch { .. } => "git_branch_delete",
        }
    }

    fn audited_verb(&self) -> &'static str {
        match self {
            Self::CreateBranch { .. } => "git_branch_created",
            Self::SwitchBranch { .. } => "git_branch_switched",
            Self::DeleteBranch { .. } => "git_branch_deleted",
        }
    }

    /// The full ref this mutation creates, moves `HEAD` onto, or deletes.
    fn reference(&self) -> String {
        format!("refs/heads/{}", self.name())
    }
}

/// What a preparation tells the dialog, and what `apply` accepts as the exact
/// approval. A refusal is a value with a reason, never a silent no-op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchPreparation {
    pub prepare_id: String,
    pub repository_root: String,
    pub session_id: String,
    pub mutation: GitMutation,
    /// The checked name, its collision alternative when it exists, and Git's
    /// reason when it is invalid.
    pub suggestion: Option<BranchSuggestion>,
    /// The start point as the user gave it, or `HEAD` by default. A switch
    /// has no start point; its target commit is `start_point_sha`.
    pub start_point: Option<String>,
    /// The commit the start point (or a switch's target ref) resolved to.
    pub start_point_sha: Option<String>,
    pub info: Option<BranchInfo>,
    pub refusal: Option<String>,
    pub can_apply: bool,
    /// For a switch: the changed paths that would be overwritten.
    pub dirty_paths: Vec<String>,
    /// For a deletion: commits not reachable from the default branch or any
    /// remote. `None` for other mutations, and for a deletion whose default
    /// branch could not be resolved, where the count is unknown.
    pub unmerged_commits: Option<u32>,
    /// For a deletion: the subjects of those commits, so the dialog can name
    /// what the separate confirmation would lose.
    pub unmerged_subjects: Vec<String>,
}

/// The durable record behind a preparation (decision D3). Recovery reads it to
/// say what a dangling `git_branch_create`/`git_branch_switch` action was
/// doing; `apply` compares the approved mutation with it and consumes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchRecord {
    pub prepare_id: String,
    pub repository_root: String,
    pub session_id: String,
    /// The synthetic task the marker belongs to.
    #[serde(default)]
    pub task_id: Option<String>,
    pub mutation: GitMutation,
    /// The start point as given: `HEAD` by default for creation.
    pub start_point: Option<String>,
    /// The commit the mutation was resolved against.
    pub resolved_start_sha: Option<String>,
    /// `HEAD` at preparation. Creation must leave it unchanged; switching must
    /// move it exactly once.
    pub expected_head: Option<String>,
    /// The only ref this record may create or move: `refs/heads/<name>`.
    pub expected_ref: String,
    /// The ref's commit at preparation. `None` for creation means it must not
    /// exist; after a successful creation it holds the created commit.
    pub expected_ref_oid: Option<String>,
    pub info: BranchInfo,
    /// `prepared`, `applied` or `failed`. Only `prepared` can be applied.
    pub state: String,
    /// The ref a successful creation created (`refs/heads/<name>`).
    pub created_ref: Option<String>,
    /// For a deletion: the subjects shown by the separate confirmation. Commit
    /// bodies are never read or audited.
    #[serde(default)]
    pub unmerged_subjects: Vec<String>,
    pub applied_at_ms: Option<u128>,
    /// Every git command actually run, exactly as it ran.
    pub git_commands: Vec<String>,
}

/// A successful mutation, as the dialog reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchOutcome {
    pub prepare_id: String,
    pub mutation: GitMutation,
    pub branch: String,
    pub start_point: Option<String>,
    pub start_point_sha: Option<String>,
    pub head_before: Option<String>,
    pub head_after: Option<String>,
    pub created_ref: Option<String>,
    pub git_commands: Vec<String>,
}

/// A read of a prepared record against the repository now. It never retries:
/// an interrupted action either landed or did not, and this says which
/// (decision D4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInspection {
    pub prepare_id: String,
    pub state: String,
    /// `landed`, `not_landed` or `other_ref_state`.
    pub outcome: String,
    pub expected_ref: String,
    pub expected_oid: Option<String>,
    pub actual_oid: Option<String>,
    pub current_branch: Option<String>,
    pub message: String,
}

/// What a merge would do, computed without attempting one (spec 36 §5.4).
///
/// The only Git command behind the conflict list is
/// `merge-tree --write-tree --name-only`, which computes in the object store
/// and touches neither the index, the working tree, nor `HEAD` (decision D12).
/// `problem` is set when the merge base itself could not be resolved, in which
/// case no prediction is claimed at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergePreparation {
    pub repository_root: String,
    pub session_id: String,
    pub base: String,
    pub head_oid: Option<String>,
    pub merge_base: Option<String>,
    /// `HEAD..base` as `<abbrev> <subject>`, newest first.
    pub incoming_commits: Vec<String>,
    /// Commits on `HEAD` that `base` does not have (`base..HEAD`).
    pub ahead: u32,
    /// Commits on `base` that `HEAD` does not have (`HEAD..base`), which are
    /// the incoming commits. Equal to `incoming_commits.len()` when the merge
    /// base resolved; both stay 0 when it did not.
    pub behind: u32,
    /// Paths that would differ between `HEAD` and the merged tree.
    pub changed_files: Vec<String>,
    /// Conflicted paths, restricted paths omitted like every other path this
    /// engine reports.
    pub conflicted_paths: Vec<String>,
    pub fast_forward: bool,
    pub conflict_prediction_available: bool,
    /// Git's error when `merge-tree --write-tree` is unsupported or its output
    /// shape is unrecognised. The fallback never runs a real merge.
    pub conflict_prediction_unavailable_reason: Option<String>,
    /// `merge-tree`'s exit status when it ran: 0 clean, 1 conflicts.
    pub merge_tree_exit_status: Option<i32>,
    /// Why the plan could not be computed: an unresolvable base or unrelated
    /// histories. `None` when the merge base resolved.
    pub problem: Option<String>,
}

/// What a rebase would replay, without replaying it (spec 36 decision D13).
/// Conflict prediction is always unavailable here: `merge-tree` models one
/// merge, not a sequential replay, so claiming a result would be dishonest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebasePreparation {
    pub repository_root: String,
    pub session_id: String,
    pub onto: String,
    pub onto_oid: Option<String>,
    pub head_oid: Option<String>,
    pub merge_base: Option<String>,
    /// `merge-base..HEAD` as `<abbrev> <subject>`, newest first.
    pub commits_to_replay: Vec<String>,
    /// Always false; kept so the shell reads both reports the same way.
    pub conflict_prediction_available: bool,
    pub conflict_prediction_unavailable_reason: String,
    /// Why the plan could not be computed: an unresolvable onto ref or
    /// unrelated histories.
    pub problem: Option<String>,
}

/// Why a branch mutation did not happen, as a value the shell can branch on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchError {
    /// A policy, state or approval refusal, checked before git runs.
    Refused { code: &'static str, message: String },
    /// An engine operation failed. The ref may or may not have moved; the
    /// record and `inspect` say what the repository actually shows.
    Engine(String),
}

impl BranchError {
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
            Self::Engine(_) => "branch_engine_error",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Refused { message, .. } | Self::Engine(message) => message.clone(),
        }
    }
}

impl From<ClientError> for BranchError {
    fn from(error: ClientError) -> Self {
        Self::Engine(error.to_string())
    }
}

/// Creates and switches branches under the same capabilities and approvals as
/// the rest of the engine's Git mutations (spec 36 §5.8).
#[derive(Debug, Clone)]
pub struct BranchEngine {
    config: Config,
    audit_log: AuditLog,
    scanner: SecretScanner,
    path_policy: PathPolicy,
    session_store: SessionStore,
    git: GitService,
}

impl BranchEngine {
    pub fn new(
        config: Config,
        audit_log: AuditLog,
        scanner: SecretScanner,
        path_policy: PathPolicy,
        session_store: SessionStore,
        git: GitService,
    ) -> Self {
        Self {
            config,
            audit_log,
            scanner,
            path_policy,
            session_store,
            git,
        }
    }

    /// A name suggestion using the configured prefix and redacted source text
    /// (decision D10). Read-only.
    pub fn suggest(&self, root: impl AsRef<Path>, source: &str) -> Result<BranchSuggestion> {
        suggest_branch_name(
            &self.git,
            &self.scanner,
            root,
            &self.config.git_branch_prefix,
            source,
        )
    }

    /// Prepare creating `name` at `start_point` (default `HEAD`). Creation
    /// never switches: `HEAD` and the working tree stay where they are.
    pub fn prepare_create(
        &self,
        root: impl AsRef<Path>,
        name: &str,
        start_point: Option<&str>,
        session_id: &str,
    ) -> Result<BranchPreparation> {
        let root = root.as_ref();
        let original = start_point.unwrap_or("HEAD").to_string();
        let info = match self.git.branch_info(root) {
            Ok(info) => info,
            Err(ClientError::InvalidInput(_)) => {
                return self.refused_preparation(
                    root,
                    session_id,
                    GitMutation::CreateBranch {
                        name: name.to_string(),
                        start_point: original,
                    },
                    None,
                    "This folder is not a Git repository.",
                );
            }
            Err(error) => return Err(error),
        };
        let suggestion = Some(self.suggestion_for(root, name)?);
        // A policy refusal is final for this preparation, but the name and git
        // state are still described so the dialog can show what is wrong.
        let mut refusal = self.policy_refusal(session_id)?;
        if refusal.is_none() {
            refusal = self.creation_refusal(&info, name, start_point, suggestion.as_ref());
        }
        let mut resolved = None;
        if refusal.is_none() {
            match self.resolve_start_point(root, &original) {
                Ok(sha) => resolved = Some(sha),
                Err(error) => refusal = Some(error.to_string()),
            }
        }
        if let Some(reason) = &refusal {
            self.audit_refusal(session_id, "git_branch_create", name, reason)?;
        }
        let mutation = GitMutation::CreateBranch {
            name: name.to_string(),
            start_point: resolved.clone().unwrap_or_else(|| original.clone()),
        };
        self.finish_preparation(
            root,
            session_id,
            mutation,
            suggestion,
            Some(original),
            resolved,
            Some(info),
            refusal,
            Vec::new(),
            Vec::new(),
        )
    }

    /// Prepare switching to the existing branch `name`. The switch runs
    /// `git switch --no-guess`, which moves the index and working tree with
    /// `HEAD`; a dirty tree that would lose work is refused with the paths
    /// named (decision D15).
    pub fn prepare_switch(
        &self,
        root: impl AsRef<Path>,
        name: &str,
        session_id: &str,
    ) -> Result<BranchPreparation> {
        let root = root.as_ref();
        let info = match self.git.branch_info(root) {
            Ok(info) => info,
            Err(ClientError::InvalidInput(_)) => {
                return self.refused_preparation(
                    root,
                    session_id,
                    GitMutation::SwitchBranch {
                        name: name.to_string(),
                    },
                    None,
                    "This folder is not a Git repository.",
                );
            }
            Err(error) => return Err(error),
        };
        let suggestion = Some(self.suggestion_for(root, name)?);
        let mut refusal = self.policy_refusal(session_id)?;
        let mut resolved = None;
        let mut dirty_paths = Vec::new();
        if refusal.is_none() {
            if let Some(operation) = &info.in_progress_operation {
                refusal = Some(format!(
                    "A Git operation is in progress ({operation}). Finish or abort it before \
                     switching branches."
                ));
            } else if let Err(error) = validate_branch_name_text(name) {
                refusal = Some(error);
            } else if let Some(suggestion) =
                suggestion.as_ref().filter(|suggestion| !suggestion.valid)
            {
                refusal = Some(
                    suggestion
                        .refusal
                        .clone()
                        .unwrap_or_else(|| format!("`{name}` is not a valid branch name.")),
                );
            } else if suggestion
                .as_ref()
                .is_some_and(|suggestion| !suggestion.exists)
            {
                refusal = Some(format!(
                    "There is no local branch named `{name}`. Create it first, then switch."
                ));
            } else {
                match self.resolve_ref_commit(root, &format!("refs/heads/{name}")) {
                    Ok(sha) => {
                        resolved = Some(sha.clone());
                        match self.dirty_switch_paths(root, &sha) {
                            Ok(paths) if !paths.is_empty() => {
                                let listed = paths.join(", ");
                                refusal = Some(format!(
                                    "Switching to `{name}` would overwrite uncommitted changes in: \
                                     {listed}. Commit or stash them, or switch after the working \
                                     tree matches."
                                ));
                                dirty_paths = paths;
                            }
                            Ok(_) => {}
                            Err(error) => refusal = Some(error.to_string()),
                        }
                    }
                    Err(error) => refusal = Some(error.to_string()),
                }
            }
        }
        if let Some(reason) = &refusal {
            self.audit_refusal(session_id, "git_branch_switch", name, reason)?;
        }
        let mutation = GitMutation::SwitchBranch {
            name: name.to_string(),
        };
        self.finish_preparation(
            root,
            session_id,
            mutation,
            suggestion,
            None,
            resolved,
            Some(info),
            refusal,
            dirty_paths,
            Vec::new(),
        )
    }

    /// Prepare deleting the existing local branch `name`.
    ///
    /// A branch whose commits are all on the default branch and the remotes is
    /// fully merged and deletes with `git branch -d`. A branch with unmerged
    /// commits is prepared with the count and subjects that would be lost;
    /// [`BranchEngine::apply`] then refuses a plain approval and runs
    /// `git branch -D` only after the separate `confirm_unmerged` confirmation
    /// (proposal §5.6).
    pub fn prepare_delete(
        &self,
        root: impl AsRef<Path>,
        name: &str,
        session_id: &str,
    ) -> Result<BranchPreparation> {
        let root = root.as_ref();
        let info = match self.git.branch_info(root) {
            Ok(info) => info,
            Err(ClientError::InvalidInput(_)) => {
                return self.refused_preparation(
                    root,
                    session_id,
                    GitMutation::DeleteBranch {
                        name: name.to_string(),
                        unmerged_commits: None,
                    },
                    None,
                    "This folder is not a Git repository.",
                );
            }
            Err(error) => return Err(error),
        };
        let suggestion = Some(self.suggestion_for(root, name)?);
        let mut refusal = self.policy_refusal(session_id)?;
        let mut resolved = None;
        let mut unmerged_commits = None;
        let mut unmerged_subjects = Vec::new();
        if refusal.is_none() {
            if let Some(operation) = &info.in_progress_operation {
                refusal = Some(format!(
                    "A Git operation is in progress ({operation}). Finish or abort it before \
                     deleting a branch."
                ));
            } else if let Err(error) = validate_branch_name_text(name) {
                refusal = Some(error);
            } else if let Some(suggestion) =
                suggestion.as_ref().filter(|suggestion| !suggestion.valid)
            {
                refusal = Some(
                    suggestion
                        .refusal
                        .clone()
                        .unwrap_or_else(|| format!("`{name}` is not a valid branch name.")),
                );
            } else if info.current.as_deref() == Some(name) {
                refusal = Some(format!(
                    "`{name}` is the current branch. Switch to another branch before deleting it."
                ));
            } else if let Some(path) = self.worktree_with_branch(root, name)? {
                refusal = Some(format!(
                    "`{name}` is checked out in the worktree at {}. Switch that worktree to \
                     another branch before deleting it.",
                    path.display()
                ));
            } else if suggestion
                .as_ref()
                .is_some_and(|suggestion| !suggestion.exists)
            {
                refusal = Some(format!("There is no local branch named `{name}`."));
            } else {
                match self.resolve_ref_commit(root, &format!("refs/heads/{name}")) {
                    Ok(sha) => {
                        resolved = Some(sha);
                        match self.unmerged_report(root, name) {
                            Ok((count, subjects)) => {
                                unmerged_commits = count;
                                unmerged_subjects = subjects;
                            }
                            Err(error) => refusal = Some(error.to_string()),
                        }
                    }
                    Err(error) => refusal = Some(error.to_string()),
                }
            }
        }
        if let Some(reason) = &refusal {
            self.audit_refusal(session_id, "git_branch_delete", name, reason)?;
        }
        let mutation = GitMutation::DeleteBranch {
            name: name.to_string(),
            unmerged_commits,
        };
        self.finish_preparation(
            root,
            session_id,
            mutation,
            suggestion,
            None,
            resolved,
            Some(info),
            refusal,
            Vec::new(),
            unmerged_subjects,
        )
    }

    /// Report what merging `base` into the current `HEAD` would do.
    ///
    /// Read-only: the only computation is `merge-tree --write-tree`, which
    /// writes tree objects and touches neither the index, the working tree, nor
    /// `HEAD` (spec 36 §5.4, decision D12). A merge base that does not resolve
    /// is reported as a problem with conflict prediction unavailable; it never
    /// falls back to a real merge.
    pub fn prepare_merge(
        &self,
        root: impl AsRef<Path>,
        base: &str,
        session_id: &str,
    ) -> Result<MergePreparation> {
        let root = root.as_ref();
        let info = self.git.branch_info(root)?;
        let mut preparation = MergePreparation {
            repository_root: root.to_string_lossy().to_string(),
            session_id: session_id.to_string(),
            base: base.to_string(),
            head_oid: info.head_oid,
            merge_base: None,
            incoming_commits: Vec::new(),
            ahead: 0,
            behind: 0,
            changed_files: Vec::new(),
            conflicted_paths: Vec::new(),
            fast_forward: false,
            conflict_prediction_available: false,
            conflict_prediction_unavailable_reason: None,
            merge_tree_exit_status: None,
            problem: None,
        };
        let merge_base = match self.git.merge_base(root, base) {
            Ok(sha) => sha,
            Err(ClientError::InvalidInput(message)) => {
                return Err(ClientError::InvalidInput(message));
            }
            Err(ClientError::Git(message)) => {
                let problem = format!(
                    "`{base}` and HEAD have no merge base, so a merge cannot be planned: {message}"
                );
                preparation.problem = Some(problem.clone());
                preparation.conflict_prediction_unavailable_reason = Some(problem);
                self.audit_merge_preparation(&preparation, 0, 0, 0)?;
                return Ok(preparation);
            }
            Err(error) => return Err(error),
        };
        preparation.merge_base = Some(merge_base);
        preparation.incoming_commits = self.commit_summaries(root, &format!("HEAD..{base}"))?;
        preparation.behind = u32::try_from(preparation.incoming_commits.len()).unwrap_or(u32::MAX);
        preparation.ahead = self.commit_count(root, &format!("{base}..HEAD"))?;
        preparation.fast_forward = self.is_ancestor(root, "HEAD", base);

        let run = self.git.merge_tree(root, base)?;
        preparation.merge_tree_exit_status = run.exit_code;
        let prediction = interpret_merge_tree(run.exit_code, &run.stdout, &run.stderr);
        let raw_conflicts = prediction.conflicted_paths.len();
        if let Some(tree) = &prediction.tree {
            preparation.changed_files =
                self.visible_paths(self.git.diff_names(root, "HEAD", tree)?);
        }
        preparation.conflicted_paths = self.visible_paths(prediction.conflicted_paths);
        preparation.conflict_prediction_available = prediction.unavailable_reason.is_none();
        preparation.conflict_prediction_unavailable_reason = prediction.unavailable_reason;
        self.audit_merge_preparation(
            &preparation,
            preparation.incoming_commits.len(),
            preparation.changed_files.len(),
            raw_conflicts,
        )?;
        Ok(preparation)
    }

    /// Report what rebasing the current `HEAD` onto `onto` would replay.
    ///
    /// Read-only: it resolves the merge base and lists commits, and replays
    /// nothing. Conflict prediction is unavailable by construction because it
    /// would require the replay itself (decision D13).
    pub fn prepare_rebase(
        &self,
        root: impl AsRef<Path>,
        onto: &str,
        session_id: &str,
    ) -> Result<RebasePreparation> {
        let root = root.as_ref();
        let info = self.git.branch_info(root)?;
        let mut preparation = RebasePreparation {
            repository_root: root.to_string_lossy().to_string(),
            session_id: session_id.to_string(),
            onto: onto.to_string(),
            onto_oid: None,
            head_oid: info.head_oid,
            merge_base: None,
            commits_to_replay: Vec::new(),
            conflict_prediction_available: false,
            conflict_prediction_unavailable_reason: REBASE_CONFLICT_PREDICTION_UNAVAILABLE
                .to_string(),
            problem: None,
        };
        let merge_base = match self.git.merge_base(root, onto) {
            Ok(sha) => sha,
            Err(ClientError::InvalidInput(message)) => {
                return Err(ClientError::InvalidInput(message));
            }
            Err(ClientError::Git(message)) => {
                preparation.problem = Some(format!(
                    "`{onto}` and HEAD have no merge base, so a rebase cannot be planned: {message}"
                ));
                self.audit_rebase_preparation(&preparation)?;
                return Ok(preparation);
            }
            Err(error) => return Err(error),
        };
        preparation.merge_base = Some(merge_base.clone());
        match self.resolve_ref_commit(root, &format!("{onto}^{{commit}}")) {
            Ok(sha) => preparation.onto_oid = Some(sha),
            Err(error) => preparation.problem = Some(error.to_string()),
        }
        preparation.commits_to_replay =
            self.commit_summaries(root, &format!("{merge_base}..HEAD"))?;
        self.audit_rebase_preparation(&preparation)?;
        Ok(preparation)
    }

    /// Apply an approved preparation exactly once. The approved mutation must
    /// equal the record's, the record must still be `prepared`, and the same
    /// Code-mode capability is checked again after the review. Nothing in this
    /// method constructs `-f` or `--force`.
    ///
    /// `confirm_unmerged` is the separate confirmation for deletion. It is the
    /// only thing that turns `git branch -d` into `git branch -D`, and only for
    /// a branch the preparation found to have unmerged commits (or whose count
    /// was unknown). A plain approval is never enough for that case.
    pub fn apply(
        &self,
        prepare_id: &str,
        approved_mutation: &GitMutation,
        approved_by: &str,
        confirm_unmerged: bool,
    ) -> std::result::Result<BranchOutcome, BranchError> {
        let mut record = self.load_record(prepare_id).map_err(BranchError::from)?;
        if record.state != "prepared" {
            return Err(self.refuse_apply(
                &record,
                "branch_not_prepared",
                "That branch preparation is no longer current. Prepare it again.",
            ));
        }
        if record.mutation != *approved_mutation {
            return Err(self.refuse_apply(
                &record,
                "branch_mutation_mismatch",
                "That approval is for a different branch operation. Nothing ran; approve the \
                 prepared operation.",
            ));
        }
        if let Some(refusal) = self
            .policy_refusal(&record.session_id)
            .map_err(BranchError::from)?
        {
            return Err(self.refuse_apply(&record, "branch_not_allowed", refusal));
        }
        let root = PathBuf::from(&record.repository_root);
        let info = self.git.branch_info(&root).map_err(BranchError::from)?;
        if let Some(operation) = &info.in_progress_operation {
            return Err(self.refuse_apply(
                &record,
                "branch_git_state",
                format!(
                    "A Git operation is in progress ({operation}). Nothing ran; finish or abort it \
                     and prepare again."
                ),
            ));
        }
        if let Some(refusal) = self
            .expected_state_refusal(&root, &record, &info)
            .map_err(BranchError::from)?
        {
            return Err(self.refuse_apply(&record, "branch_state_changed", refusal));
        }
        // Deletion has two extra decisions: a branch that is checked out
        // somewhere cannot be deleted, and commits that are not on the default
        // branch or a remote require the separate confirmation. Both are
        // re-checked here because the checkout or the branch can change after
        // the review. `git branch -D` is reachable only through `force_delete`.
        let mut force_delete = false;
        if let GitMutation::DeleteBranch {
            name,
            unmerged_commits,
        } = &record.mutation
        {
            if info.current.as_deref() == Some(name.as_str()) {
                return Err(self.refuse_apply(
                    &record,
                    "branch_delete_current",
                    format!(
                        "`{name}` is the current branch, so it cannot be deleted. Nothing ran."
                    ),
                ));
            }
            if let Some(path) = self
                .worktree_with_branch(&root, name)
                .map_err(BranchError::from)?
            {
                return Err(self.refuse_apply(
                    &record,
                    "branch_delete_checked_out",
                    format!(
                        "`{name}` is checked out in the worktree at {}. Nothing ran; switch that \
                         worktree to another branch first.",
                        path.display()
                    ),
                ));
            }
            if *unmerged_commits != Some(0) && !confirm_unmerged {
                let listed = unmerged_subjects_text(&record.unmerged_subjects);
                let message = match unmerged_commits {
                    Some(count) => format!(
                        "Deleting `{name}` would lose {count} commit{} that {} not on the default \
                         branch or any remote: {listed}. A plain approval is not enough: confirm \
                         explicitly that those commits may be lost.",
                        if *count == 1 { "" } else { "s" },
                        if *count == 1 { "is" } else { "are" },
                    ),
                    None => format!(
                        "Damaian could not determine the default branch, so it cannot count what \
                         deleting `{name}` would lose. The commit subjects are: {listed}. A plain \
                         approval is not enough: confirm explicitly that those commits may be lost."
                    ),
                };
                return Err(self.refuse_apply(
                    &record,
                    "branch_unmerged_confirmation_required",
                    message,
                ));
            }
            force_delete = *unmerged_commits != Some(0);
        }
        // Git would refuse this itself; naming the paths here is the earlier,
        // clearer version (decision D15), and it is re-checked because the
        // working tree can change after the review.
        if let GitMutation::SwitchBranch { name } = &record.mutation {
            let Some(target) = record.expected_ref_oid.clone() else {
                return Err(BranchError::Engine(
                    "The prepared switch has no target commit; prepare it again.".to_string(),
                ));
            };
            let paths = self
                .dirty_switch_paths(&root, &target)
                .map_err(BranchError::from)?;
            if !paths.is_empty() {
                return Err(self.refuse_apply(
                    &record,
                    "branch_dirty_switch",
                    format!(
                        "Switching to `{name}` would overwrite uncommitted changes in: {}. \
                         Nothing ran.",
                        paths.join(", ")
                    ),
                ));
            }
        }
        // The durable marker exists before the first write, so a crash between
        // the pair classifies as an unknown external outcome rather than
        // disappearing (spec 17; decision D4).
        let task = self
            .session_store
            .create_task(
                &record.session_id,
                "",
                &self.config.model_provider,
                &self.config.model_name,
            )
            .map_err(BranchError::from)?;
        let task = self
            .session_store
            .update_task_status(&task, TaskStatus::RunningTool, None)
            .map_err(BranchError::from)?;
        let marker = self
            .session_store
            .start_action(&task, record.mutation.action(), &record.prepare_id, true)
            .map_err(BranchError::from)?;
        record.task_id = Some(task.id.clone());
        let _ = self.save_record(&record);
        let args = mutation_args(&record.mutation, force_delete);
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        record
            .git_commands
            .push(user_git_command_line(&root, &arg_refs));
        let outcome = run_user_git(&root, &arg_refs);
        match outcome {
            Ok(_) => {
                record.state = "applied".to_string();
                record.applied_at_ms = Some(now_millis());
                record.created_ref = match &record.mutation {
                    GitMutation::CreateBranch { .. } => Some(record.expected_ref.clone()),
                    GitMutation::SwitchBranch { .. } | GitMutation::DeleteBranch { .. } => None,
                };
                // A deletion leaves its expected ref commit behind so `inspect`
                // can still tell a re-created ref from the one that was removed.
                if !matches!(record.mutation, GitMutation::DeleteBranch { .. }) {
                    record.expected_ref_oid = record.resolved_start_sha.clone();
                }
                let _ = self.save_record(&record);
                let _ = self.session_store.finish_action(marker, "ok");
                let _ = self
                    .session_store
                    .update_task_status(&task, TaskStatus::Complete, None);
                let head_after = self
                    .git
                    .branch_info(&root)
                    .ok()
                    .and_then(|info| info.head_oid);
                self.audit_outcome(
                    &record,
                    approved_by,
                    "ok",
                    head_after.as_deref(),
                    force_delete,
                )?;
                Ok(BranchOutcome {
                    prepare_id: record.prepare_id.clone(),
                    mutation: record.mutation.clone(),
                    branch: record.mutation.name().to_string(),
                    start_point: record.start_point.clone(),
                    start_point_sha: record.resolved_start_sha.clone(),
                    head_before: record.expected_head.clone(),
                    head_after,
                    created_ref: record.created_ref.clone(),
                    git_commands: record.git_commands.clone(),
                })
            }
            Err(error) => {
                record.state = "failed".to_string();
                let _ = self.save_record(&record);
                let _ = self.session_store.finish_action(marker, "failed");
                let _ = self.session_store.update_task_status(
                    &task,
                    TaskStatus::Failed,
                    Some(&error.to_string()),
                );
                let _ = self.audit_outcome(&record, approved_by, "failed", None, force_delete);
                Err(BranchError::Engine(format!(
                    "{} did not complete: {error}. Inspect the record before trying again; \
                     Damaian does not retry a branch mutation automatically.",
                    record.mutation.action()
                )))
            }
        }
    }

    /// Load a prepared record, for the shell after a restart and for recovery.
    pub fn load_record(&self, prepare_id: &str) -> Result<BranchRecord> {
        let content = fs::read_to_string(self.record_path(prepare_id))?;
        serde_json::from_str(&content).map_err(|error| {
            ClientError::Io(format!("Branch record {prepare_id} is unreadable: {error}"))
        })
    }

    /// Compare the prepared expectation with the repository now. It never
    /// retries: `landed` means the ref shows what the mutation promised,
    /// `not_landed` means it does not, and `other_ref_state` means the ref is
    /// present at a commit this record did not expect.
    pub fn inspect(&self, prepare_id: &str) -> Result<BranchInspection> {
        let record = self.load_record(prepare_id)?;
        let root = Path::new(&record.repository_root);
        let actual = self.ref_oid(root, &record.expected_ref)?;
        let current_branch = self
            .git
            .branch_info(root)
            .ok()
            .and_then(|info| info.current);
        // A creation that a crash interrupted before the record was saved still
        // has a known expectation: the start commit it resolved. Reporting
        // that as `landed` is what makes recovery a read rather than a retry.
        let expected_oid = match &record.mutation {
            GitMutation::CreateBranch { .. } => record.resolved_start_sha.clone(),
            GitMutation::SwitchBranch { .. } | GitMutation::DeleteBranch { .. } => {
                record.expected_ref_oid.clone()
            }
        };
        // A deletion lands when the ref is absent, including when the crash
        // happened after `git branch -d` but before the record was marked
        // applied. A ref that is still present did not land; one at another
        // commit is a state this record did not expect. None of these retries.
        let (outcome, message) = if matches!(record.mutation, GitMutation::DeleteBranch { .. }) {
            match (&expected_oid, &actual) {
                (_, None) => (
                    "landed",
                    format!("{} is absent; the deletion landed.", record.expected_ref),
                ),
                (Some(expected), Some(oid)) if expected == oid => (
                    "not_landed",
                    format!(
                        "{} is still at {expected}; the deletion did not land.",
                        record.expected_ref
                    ),
                ),
                (_, Some(oid)) => (
                    "other_ref_state",
                    format!(
                        "{} exists at {oid}, which this record did not expect.",
                        record.expected_ref
                    ),
                ),
            }
        } else {
            match (&expected_oid, &actual) {
                (None, None) => (
                    "not_landed",
                    format!("{} does not exist.", record.expected_ref),
                ),
                (None, Some(oid)) => (
                    "other_ref_state",
                    format!(
                        "{} exists at {oid}, which this record did not expect.",
                        record.expected_ref
                    ),
                ),
                (Some(expected), Some(oid)) if expected == oid => {
                    let on_it = match &record.mutation {
                        GitMutation::CreateBranch { .. } => true,
                        GitMutation::SwitchBranch { name } => {
                            current_branch.as_deref() == Some(name.as_str())
                        }
                        GitMutation::DeleteBranch { .. } => unreachable!("handled above"),
                    };
                    if on_it {
                        (
                            "landed",
                            format!("{} is at {expected}.", record.expected_ref),
                        )
                    } else {
                        (
                            "not_landed",
                            format!(
                                "{} is at {expected}, but HEAD is not on it.",
                                record.expected_ref
                            ),
                        )
                    }
                }
                (Some(expected), Some(oid)) => (
                    "other_ref_state",
                    format!(
                        "{} is at {oid}; this record expected {expected}.",
                        record.expected_ref
                    ),
                ),
                (Some(expected), None) => (
                    "not_landed",
                    format!(
                        "{} is missing; this record expected {expected}.",
                        record.expected_ref
                    ),
                ),
            }
        };
        Ok(BranchInspection {
            prepare_id: record.prepare_id.clone(),
            state: record.state.clone(),
            outcome: outcome.to_string(),
            expected_ref: record.expected_ref.clone(),
            expected_oid,
            actual_oid: actual,
            current_branch,
            message: message.to_string(),
        })
    }

    // The preparation is one value assembled from the fields each caller
    // already holds; a builder struct here would only rename the arguments.
    #[allow(clippy::too_many_arguments)]
    fn finish_preparation(
        &self,
        root: &Path,
        session_id: &str,
        mutation: GitMutation,
        suggestion: Option<BranchSuggestion>,
        start_point: Option<String>,
        start_point_sha: Option<String>,
        info: Option<BranchInfo>,
        refusal: Option<String>,
        dirty_paths: Vec<String>,
        unmerged_subjects: Vec<String>,
    ) -> Result<BranchPreparation> {
        let prepare_id = create_id("branch");
        let can_apply = refusal.is_none();
        let unmerged_commits = match &mutation {
            GitMutation::DeleteBranch {
                unmerged_commits, ..
            } => *unmerged_commits,
            _ => None,
        };
        if can_apply {
            let Some(record_info) = info.clone() else {
                return Err(ClientError::InvalidInput(
                    "A branch preparation without the repository's branch state cannot be \
                     applied."
                        .to_string(),
                ));
            };
            let record = BranchRecord {
                prepare_id: prepare_id.clone(),
                repository_root: root.to_string_lossy().to_string(),
                session_id: session_id.to_string(),
                task_id: None,
                mutation: mutation.clone(),
                start_point: start_point.clone(),
                resolved_start_sha: start_point_sha.clone(),
                expected_head: info.as_ref().and_then(|info| info.head_oid.clone()),
                expected_ref: mutation.reference(),
                expected_ref_oid: match &mutation {
                    GitMutation::CreateBranch { .. } => None,
                    GitMutation::SwitchBranch { .. } | GitMutation::DeleteBranch { .. } => {
                        start_point_sha.clone()
                    }
                },
                info: record_info,
                state: "prepared".to_string(),
                created_ref: None,
                unmerged_subjects: unmerged_subjects.clone(),
                applied_at_ms: None,
                git_commands: Vec::new(),
            };
            self.save_record(&record)?;
        }
        Ok(BranchPreparation {
            prepare_id,
            repository_root: root.to_string_lossy().to_string(),
            session_id: session_id.to_string(),
            mutation,
            suggestion,
            start_point,
            start_point_sha,
            info,
            refusal,
            can_apply,
            dirty_paths,
            unmerged_commits,
            unmerged_subjects,
        })
    }

    fn refused_preparation(
        &self,
        root: &Path,
        session_id: &str,
        mutation: GitMutation,
        suggestion: Option<BranchSuggestion>,
        reason: &str,
    ) -> Result<BranchPreparation> {
        self.audit_refusal(session_id, mutation.action(), mutation.name(), reason)?;
        self.finish_preparation(
            root,
            session_id,
            mutation,
            suggestion,
            None,
            None,
            None,
            Some(reason.to_string()),
            Vec::new(),
            Vec::new(),
        )
    }

    /// The capability and mode gate, checked at preparation and again at
    /// apply. A denied capability is a refusal with a reason.
    fn policy_refusal(&self, session_id: &str) -> Result<Option<String>> {
        if self.session_store.read_session(session_id)?.is_none() {
            return Ok(Some(
                "Unknown session. Open a session before changing branches.".to_string(),
            ));
        }
        let mode = self.session_store.session_mode(session_id);
        if mode != SessionMode::Code {
            return Ok(Some(format!(
                "Branch changes are available in Code mode only. This session is in {} mode.",
                mode.label()
            )));
        }
        if !self.config.profile_capabilities().allow_git_branches {
            return Ok(Some(
                "The permission profile does not allow Git branch changes \
                 (allow_git_branches=false). Switching mode will not allow it."
                    .to_string(),
            ));
        }
        Ok(None)
    }

    fn suggestion_for(&self, root: &Path, name: &str) -> Result<BranchSuggestion> {
        if let Err(reason) = validate_branch_name_text(name) {
            return Ok(BranchSuggestion {
                name: name.to_string(),
                valid: false,
                refusal: Some(reason),
                exists: false,
                alternative: None,
            });
        }
        checked_suggestion(&self.git, root, name.to_string())
    }

    fn creation_refusal(
        &self,
        info: &BranchInfo,
        name: &str,
        start_point: Option<&str>,
        suggestion: Option<&BranchSuggestion>,
    ) -> Option<String> {
        if let Some(operation) = &info.in_progress_operation {
            return Some(format!(
                "A Git operation is in progress ({operation}). Finish or abort it before \
                 creating a branch."
            ));
        }
        if info.head_oid.is_none() {
            return Some(
                "This repository has no commits yet. Commit something before creating a branch."
                    .to_string(),
            );
        }
        if info.detached && start_point.is_none() {
            return Some(
                "HEAD is detached. Give an explicit start point to create a branch from."
                    .to_string(),
            );
        }
        if let Some(suggestion) = suggestion {
            if !suggestion.valid {
                return Some(
                    suggestion
                        .refusal
                        .clone()
                        .unwrap_or_else(|| format!("`{name}` is not a valid branch name.")),
                );
            }
            if suggestion.exists {
                let alternative = suggestion
                    .alternative
                    .as_deref()
                    .map(|alternative| format!(" Try `{alternative}` instead."))
                    .unwrap_or_default();
                return Some(format!(
                    "A branch named `{name}` already exists; it was not changed.{alternative}"
                ));
            }
        }
        None
    }

    fn resolve_start_point(&self, root: &Path, start_point: &str) -> Result<String> {
        validate_start_point(start_point)?;
        self.resolve_ref_commit(root, &format!("{start_point}^{{commit}}"))
    }

    fn resolve_ref_commit(&self, root: &Path, spec: &str) -> Result<String> {
        let output = run_user_git(
            root,
            &["rev-parse", "--verify", "-q", "--end-of-options", spec],
        )
        .map_err(|error| ClientError::InvalidInput(format!("`{spec}` did not resolve: {error}")))?;
        let sha = String::from_utf8_lossy(&output).trim().to_string();
        if sha.is_empty() {
            return Err(ClientError::InvalidInput(format!(
                "`{spec}` did not resolve to a commit"
            )));
        }
        Ok(sha)
    }

    /// Paths that may leave the engine: restricted paths are omitted, as in
    /// `BranchInfo` and the switch's dirty-path report. Sorted and de-duplicated
    /// so a report is stable across runs.
    fn visible_paths(&self, paths: impl IntoIterator<Item = String>) -> Vec<String> {
        paths
            .into_iter()
            .filter(|path| !self.path_policy.is_restricted(path, false))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// `<abbrev> <subject>` per commit in `range`, newest first. Commit bodies
    /// are never read, so they cannot reach a report or the audit.
    fn commit_summaries(&self, root: &Path, range: &str) -> Result<Vec<String>> {
        let output = run_user_git(root, &["log", "--format=%h %s", range])?;
        Ok(String::from_utf8_lossy(&output)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// The number of commits in `range`, for the ahead/behind counts that keep
    /// the merge report useful when conflict prediction is unavailable.
    fn commit_count(&self, root: &Path, range: &str) -> Result<u32> {
        let output = run_user_git(root, &["rev-list", "--count", range])?;
        let raw = String::from_utf8_lossy(&output).trim().to_string();
        raw.parse::<u32>().map_err(|_| {
            ClientError::Git(format!(
                "git rev-list returned an unreadable commit count: {raw}"
            ))
        })
    }

    /// Whether `ancestor` is reachable from `descendant`, i.e. a merge would
    /// fast-forward. Git exits 1 for "no", which is a false answer rather than
    /// an error here.
    fn is_ancestor(&self, root: &Path, ancestor: &str, descendant: &str) -> bool {
        run_user_git(root, &["merge-base", "--is-ancestor", ancestor, descendant]).is_ok()
    }

    /// The merge report's audit: base, merge base, three counts, and whether
    /// conflict prediction ran. Never the incoming subjects (D12).
    fn audit_merge_preparation(
        &self,
        preparation: &MergePreparation,
        incoming: usize,
        changed: usize,
        conflicts: usize,
    ) -> Result<()> {
        let outcome = if preparation.conflict_prediction_available {
            "prepared"
        } else {
            "unavailable"
        };
        self.audit_log.record(
            "git_merge_prepared",
            &[
                ("actor", "user".to_string()),
                ("sessionId", preparation.session_id.clone()),
                ("base", preparation.base.clone()),
                (
                    "mergeBase",
                    preparation.merge_base.clone().unwrap_or_default(),
                ),
                ("incomingCommits", incoming.to_string()),
                ("ahead", preparation.ahead.to_string()),
                ("behind", preparation.behind.to_string()),
                ("changedFiles", changed.to_string()),
                ("conflictedPaths", conflicts.to_string()),
                (
                    "conflictPredictionAvailable",
                    preparation.conflict_prediction_available.to_string(),
                ),
                ("outcome", outcome.to_string()),
            ],
        )?;
        Ok(())
    }

    /// The rebase report's audit: onto, merge base, how many commits would be
    /// replayed, and that conflict prediction did not run. Never subjects.
    fn audit_rebase_preparation(&self, preparation: &RebasePreparation) -> Result<()> {
        let outcome = if preparation.problem.is_some() {
            "unavailable"
        } else {
            "prepared"
        };
        self.audit_log.record(
            "git_rebase_prepared",
            &[
                ("actor", "user".to_string()),
                ("sessionId", preparation.session_id.clone()),
                ("onto", preparation.onto.clone()),
                ("ontoOid", preparation.onto_oid.clone().unwrap_or_default()),
                (
                    "mergeBase",
                    preparation.merge_base.clone().unwrap_or_default(),
                ),
                (
                    "commitsToReplay",
                    preparation.commits_to_replay.len().to_string(),
                ),
                ("conflictPredictionAvailable", "false".to_string()),
                ("outcome", outcome.to_string()),
            ],
        )?;
        Ok(())
    }

    /// The commits on `name` that are not on any remote and not on the
    /// repository's default branch, and their subject lines. The count is
    /// `None` when decision D11's resolution order finds no default branch;
    /// deletion then always needs the separate unmerged confirmation.
    ///
    /// Git treats a second `--not` as toggling back to positive revisions, so
    /// the exclusions share one `--not`: `<branch> --not --remotes <default>`.
    fn unmerged_report(&self, root: &Path, name: &str) -> Result<(Option<u32>, Vec<String>)> {
        let branch = format!("refs/heads/{name}");
        let default = self.default_branch_rev(root, name)?;
        let mut exclusions = vec!["--not".to_string(), "--remotes".to_string()];
        if let Some(default) = &default {
            exclusions.push(default.clone());
        }
        let mut count_args = vec![
            "rev-list".to_string(),
            "--count".to_string(),
            branch.clone(),
        ];
        count_args.extend(exclusions.iter().cloned());
        let count_refs = count_args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_user_git(root, &count_refs)?;
        let raw = String::from_utf8_lossy(&output).trim().to_string();
        let count = if default.is_some() {
            Some(raw.parse::<u32>().map_err(|_| {
                ClientError::Git(format!(
                    "git rev-list returned an unreadable commit count: {raw}"
                ))
            })?)
        } else {
            None
        };
        let mut subject_args = vec!["log".to_string(), "--format=%s".to_string(), branch];
        subject_args.extend(exclusions);
        let subject_refs = subject_args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_user_git(root, &subject_refs)?;
        let subjects = String::from_utf8_lossy(&output)
            .lines()
            .map(|line| {
                let subject = line.trim();
                if subject.is_empty() {
                    "(no subject)".to_string()
                } else {
                    subject.to_string()
                }
            })
            .collect();
        Ok((count, subjects))
    }

    /// Decision D11: `refs/remotes/origin/HEAD`'s symbolic target, then local
    /// `main`, then local `master`, then the branch's merge base with `HEAD`.
    /// `None` means no default branch could be resolved, not an error.
    fn default_branch_rev(&self, root: &Path, branch: &str) -> Result<Option<String>> {
        if let Some(target) = self.symbolic_ref(root, "refs/remotes/origin/HEAD")?
            && self.ref_oid(root, &target)?.is_some()
        {
            return Ok(Some(target));
        }
        for candidate in ["refs/heads/main", "refs/heads/master"] {
            if self.ref_oid(root, candidate)?.is_some() {
                return Ok(Some(candidate.to_string()));
            }
        }
        match self.git.merge_base(root, &format!("refs/heads/{branch}")) {
            Ok(sha) => Ok(Some(sha)),
            Err(ClientError::Git(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn symbolic_ref(&self, root: &Path, reference: &str) -> Result<Option<String>> {
        let Ok(output) = run_user_git(root, &["symbolic-ref", "-q", reference]) else {
            return Ok(None);
        };
        let target = String::from_utf8_lossy(&output).trim().to_string();
        Ok((!target.is_empty()).then_some(target))
    }

    /// The worktree, if any, that has `name` checked out. Git refuses to delete
    /// such a branch; refusing here names the worktree instead of surfacing
    /// that error, and is checked again at apply time.
    fn worktree_with_branch(&self, root: &Path, name: &str) -> Result<Option<PathBuf>> {
        Ok(self
            .git
            .worktrees(root)?
            .into_iter()
            .find(|entry| entry.branch.as_deref() == Some(name))
            .map(|entry| entry.path))
    }

    /// `HEAD..target`'s changed paths intersected with the working tree's
    /// changed and untracked paths. This is the earlier, clearer version of
    /// the refusal `git switch` itself produces (decision D15); Git's own
    /// check remains the backstop.
    fn dirty_switch_paths(&self, root: &Path, target: &str) -> Result<Vec<String>> {
        let output = run_user_git(
            root,
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "HEAD",
                target,
            ],
        )?;
        let mut local: HashSet<String> = self
            .git
            .changes(root)?
            .into_iter()
            .map(|change| change.path)
            .collect();
        if local.is_empty() {
            return Ok(Vec::new());
        }
        let mut intersecting = BTreeSet::new();
        for path in output
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = String::from_utf8_lossy(path).to_string();
            if self.path_policy.is_restricted(&path, false) {
                continue;
            }
            if local.remove(&path) {
                intersecting.insert(path);
            }
        }
        Ok(intersecting.into_iter().collect())
    }

    fn expected_state_refusal(
        &self,
        root: &Path,
        record: &BranchRecord,
        info: &BranchInfo,
    ) -> Result<Option<String>> {
        if info.head_oid != record.expected_head {
            return Ok(Some(format!(
                "HEAD moved from {} to {} since you reviewed this. Nothing ran; prepare again.",
                record.expected_head.as_deref().unwrap_or("no commit"),
                info.head_oid.as_deref().unwrap_or("no commit"),
            )));
        }
        let actual = self.ref_oid(root, &record.expected_ref)?;
        match &record.mutation {
            GitMutation::CreateBranch { .. } => {
                if actual.is_some() {
                    return Ok(Some(format!(
                        "{} now exists; nothing ran. Prepare again with the free name.",
                        record.expected_ref
                    )));
                }
            }
            GitMutation::SwitchBranch { .. } => {
                if actual != record.expected_ref_oid {
                    return Ok(Some(format!(
                        "{} moved since you reviewed this switch. Nothing ran; prepare again.",
                        record.expected_ref
                    )));
                }
            }
            GitMutation::DeleteBranch { .. } => {
                if actual != record.expected_ref_oid {
                    return Ok(Some(match (&record.expected_ref_oid, &actual) {
                        (_, None) => format!(
                            "{} no longer exists. Nothing ran; if the deletion already happened, \
                             inspect the record.",
                            record.expected_ref
                        ),
                        _ => format!(
                            "{} moved since you reviewed this deletion. Nothing ran; prepare \
                             again.",
                            record.expected_ref
                        ),
                    }));
                }
            }
        }
        Ok(None)
    }

    fn ref_oid(&self, root: &Path, reference: &str) -> Result<Option<String>> {
        match run_user_git(root, &["show-ref", "--verify", reference]) {
            Ok(output) => Ok(String::from_utf8_lossy(&output)
                .split_whitespace()
                .next()
                .map(str::to_string)),
            Err(_) => Ok(None),
        }
    }

    fn refuse_apply(
        &self,
        record: &BranchRecord,
        code: &'static str,
        message: impl Into<String>,
    ) -> BranchError {
        let message = message.into();
        let _ = self.audit_log.record(
            "git_branch_refused",
            &[
                ("actor", "user".to_string()),
                ("sessionId", record.session_id.clone()),
                ("prepareId", record.prepare_id.clone()),
                ("operation", record.mutation.action().to_string()),
                ("branch", record.mutation.name().to_string()),
                ("reason", code.to_string()),
                ("outcome", "refused".to_string()),
            ],
        );
        BranchError::refused(code, message)
    }

    fn audit_refusal(
        &self,
        session_id: &str,
        operation: &str,
        name: &str,
        reason: &str,
    ) -> Result<()> {
        self.audit_log.record(
            "git_branch_refused",
            &[
                ("actor", "user".to_string()),
                ("sessionId", session_id.to_string()),
                ("operation", operation.to_string()),
                ("branch", name.to_string()),
                ("reason", "refused".to_string()),
                ("detail", reason.to_string()),
                ("outcome", "refused".to_string()),
            ],
        )?;
        Ok(())
    }

    fn audit_outcome(
        &self,
        record: &BranchRecord,
        approved_by: &str,
        outcome: &str,
        head_after: Option<&str>,
        confirmed_unmerged: bool,
    ) -> Result<()> {
        let mut fields = vec![
            ("actor", "user".to_string()),
            ("sessionId", record.session_id.clone()),
            ("prepareId", record.prepare_id.clone()),
            ("approvedBy", approved_by.to_string()),
            ("branch", record.mutation.name().to_string()),
            ("reference", record.expected_ref.clone()),
            ("startPoint", record.start_point.clone().unwrap_or_default()),
            (
                "startPointSha",
                record.resolved_start_sha.clone().unwrap_or_default(),
            ),
            (
                "headBefore",
                record.expected_head.clone().unwrap_or_default(),
            ),
            ("headAfter", head_after.unwrap_or_default().to_string()),
            ("outcome", outcome.to_string()),
        ];
        // A deletion records how many commits the default branch did not
        // reach, and whether the separate confirmation was actually used.
        // Never the commit bodies: subjects are what the dialog names, and
        // they are not repeated here.
        if let GitMutation::DeleteBranch {
            unmerged_commits, ..
        } = &record.mutation
        {
            fields.push((
                "unmergedCommits",
                unmerged_commits
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
            ));
            fields.push(("confirmedUnmerged", confirmed_unmerged.to_string()));
        }
        self.audit_log
            .record(record.mutation.audited_verb(), &fields)?;
        Ok(())
    }

    fn record_path(&self, prepare_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("branches")
            .join(format!("{prepare_id}.json"))
    }

    fn save_record(&self, record: &BranchRecord) -> Result<()> {
        let path = self.record_path(&record.prepare_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(record)
            .map_err(|error| ClientError::Io(format!("branch record serialization: {error}")))?;
        fs::write(path, content)?;
        Ok(())
    }
}

/// The exact argv for a mutation. Creation uses `git branch`, which refuses an
/// existing ref; switching uses `git switch --no-guess` so the index and
/// working tree move with `HEAD` (decision D6); deletion uses `git branch -d`
/// unless `force_delete` is set, which only the separately confirmed unmerged
/// path in `apply` does. No branch here constructs `-f` or `--force`.
fn mutation_args(mutation: &GitMutation, force_delete: bool) -> Vec<String> {
    match mutation {
        GitMutation::CreateBranch { name, start_point } => {
            vec!["branch".to_string(), name.clone(), start_point.clone()]
        }
        GitMutation::SwitchBranch { name } => {
            vec!["switch".to_string(), "--no-guess".to_string(), name.clone()]
        }
        GitMutation::DeleteBranch { name, .. } => vec![
            "branch".to_string(),
            if force_delete { "-D" } else { "-d" }.to_string(),
            name.clone(),
        ],
    }
}

/// The commit subjects a deletion would lose, as the confirmation names them.
fn unmerged_subjects_text(subjects: &[String]) -> String {
    if subjects.is_empty() {
        return "no commit subjects could be read".to_string();
    }
    subjects
        .iter()
        .map(|subject| format!("`{subject}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The interpreted output of `merge-tree --write-tree --name-only`, where the
/// first line is the resulting tree id, the following lines up to the first
/// blank line are conflicted paths, and everything after that is informational
/// (decision D12).
#[derive(Debug, Clone, PartialEq, Eq)]
struct MergeTreePrediction {
    /// The tree id, when the output shape was recognised. `None` whenever the
    /// prediction is unavailable, so no changed-file diff is taken from an
    /// output this version does not understand.
    tree: Option<String>,
    conflicted_paths: Vec<String>,
    unavailable_reason: Option<String>,
}

fn interpret_merge_tree(exit_code: Option<i32>, stdout: &str, stderr: &str) -> MergeTreePrediction {
    let mut lines = stdout.lines();
    let tree = lines.next().map(str::trim).unwrap_or_default();
    if !is_object_id(tree) {
        return unavailable_prediction(
            "git merge-tree did not print a tree id, so conflict prediction is unavailable",
            stderr,
        );
    }
    let mut conflicted_paths = Vec::new();
    for line in lines {
        let path = line.trim();
        if path.is_empty() {
            // The rest is informational text about the merge, not paths.
            break;
        }
        conflicted_paths.push(path.to_string());
    }
    match exit_code {
        Some(0) if conflicted_paths.is_empty() => MergeTreePrediction {
            tree: Some(tree.to_string()),
            conflicted_paths,
            unavailable_reason: None,
        },
        Some(1) if !conflicted_paths.is_empty() => MergeTreePrediction {
            tree: Some(tree.to_string()),
            conflicted_paths,
            unavailable_reason: None,
        },
        Some(status) => unavailable_prediction(
            &format!(
                "git merge-tree exited with status {status} for an output shape this version does \
                 not recognise"
            ),
            stderr,
        ),
        None => unavailable_prediction(
            "git merge-tree reported no exit status, so conflict prediction is unavailable",
            stderr,
        ),
    }
}

fn unavailable_prediction(reason: &str, stderr: &str) -> MergeTreePrediction {
    let stderr = stderr.trim();
    let reason = if stderr.is_empty() {
        reason.to_string()
    } else {
        format!("{reason}: {stderr}")
    };
    MergeTreePrediction {
        tree: None,
        conflicted_paths: Vec::new(),
        unavailable_reason: Some(reason),
    }
}

/// A Git object id is the only thing the tree line may contain. Accepting only
/// hex of the two hash lengths keeps the value safe to pass to `git diff`
/// without it ever looking like an option.
fn is_object_id(line: &str) -> bool {
    matches!(line.len(), 40 | 64) && line.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Guard the shape before Git sees an argument that it would read as an option
/// or expand as a previous-branch expression. Git remains authoritative about
/// the rest through `check-ref-format --branch`.
fn validate_branch_name_text(name: &str) -> std::result::Result<(), String> {
    let invalid = name.is_empty()
        || name.starts_with('-')
        || name.contains("@{")
        || name.chars().any(char::is_control);
    if invalid {
        return Err(format!(
            "`{name}` is not a branch name Git would accept as text"
        ));
    }
    Ok(())
}

/// Refuse an option-shaped or empty start point before any branch command.
fn validate_start_point(start_point: &str) -> Result<()> {
    if start_point.is_empty() || start_point.starts_with('-') {
        return Err(ClientError::InvalidInput(
            "A start point cannot be empty or start with `-`.".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{interpret_merge_tree, is_object_id};

    /// SHA-1 length, so the parser accepts it without the test depending on
    /// the fixture repository's object format.
    const TREE: &str = "4341faceb9b742bb1b5b0345ff991e671581ae19";

    #[test]
    fn the_first_line_is_the_tree_and_the_next_lines_are_conflicted_paths() {
        let stdout = format!("{TREE}\nconflict.txt\n\nAuto-merging conflict.txt\n");
        let prediction = interpret_merge_tree(Some(1), &stdout, "");
        assert_eq!(prediction.tree.as_deref(), Some(TREE));
        assert_eq!(
            prediction.conflicted_paths,
            vec!["conflict.txt".to_string()]
        );
        assert_eq!(prediction.unavailable_reason, None);

        let clean = interpret_merge_tree(Some(0), &format!("{TREE}\n"), "");
        assert_eq!(clean.tree.as_deref(), Some(TREE));
        assert!(clean.conflicted_paths.is_empty());
        assert_eq!(clean.unavailable_reason, None);
    }

    #[test]
    fn an_unsupported_write_tree_reports_unavailable_with_gits_reason() {
        // This is what Git prints when it has no `--write-tree`; the test is
        // version-independent because the decision is the parser's.
        let prediction = interpret_merge_tree(
            Some(129),
            "",
            "error: unknown option `write-tree'\nusage: git merge-tree ...",
        );
        assert_eq!(prediction.tree, None);
        assert!(prediction.conflicted_paths.is_empty());
        let reason = prediction.unavailable_reason.expect("a reason");
        assert!(reason.contains("write-tree"), "{reason}");
    }

    #[test]
    fn an_output_shape_that_contradicts_the_exit_status_is_unavailable() {
        // Exit 0 promises a clean merge; a named path contradicts that.
        let clean_but_listed = interpret_merge_tree(Some(0), &format!("{TREE}\npath.txt\n"), "");
        assert_eq!(clean_but_listed.tree, None);
        assert!(clean_but_listed.conflicted_paths.is_empty());
        assert!(clean_but_listed.unavailable_reason.is_some());

        // Exit 1 promises conflicts; naming none contradicts that.
        let conflict_but_empty = interpret_merge_tree(Some(1), &format!("{TREE}\n"), "");
        assert_eq!(conflict_but_empty.tree, None);
        assert!(conflict_but_empty.unavailable_reason.is_some());

        // A first line that is not an object id is not a tree to diff.
        let not_a_tree = interpret_merge_tree(Some(0), "not-a-tree\n", "");
        assert_eq!(not_a_tree.tree, None);
        assert!(not_a_tree.unavailable_reason.is_some());
    }

    #[test]
    fn only_full_object_ids_are_accepted_as_the_tree_line() {
        assert!(is_object_id(TREE));
        assert!(is_object_id(&"a".repeat(64)));
        assert!(!is_object_id("--output=/tmp/x"));
        assert!(!is_object_id(&"a".repeat(39)));
        assert!(!is_object_id(&"g".repeat(40)));
    }
}
