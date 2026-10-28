//! Branch naming and branch delivery (spec 36).
//!
//! Naming does not change a ref and is kept here rather than in
//! `git_service.rs` because it is a Damaian convention built on Git's
//! validation, not another Git read. The delivery half is the prepared record
//! and the two mutations this spec owns: creating a branch and switching to
//! one. Creation runs `git branch` and never switches; switching runs
//! `git switch --no-guess`, so the index and working tree move with `HEAD`
//! (spec 36 `context.md` §3.17, decision D6).
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
/// another (spec 36 §5.5, decisions D3 and D14). Deletion arrives in Task 5.
///
/// `start_point` is the **resolved** commit object id, not the name the user
/// typed: the approval then names the exact commit that was reviewed, and a
/// ref that moves afterwards cannot silently become the start point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GitMutation {
    #[serde(rename_all = "camelCase")]
    CreateBranch { name: String, start_point: String },
    #[serde(rename_all = "camelCase")]
    SwitchBranch { name: String },
}

impl GitMutation {
    /// The branch this mutation names.
    pub fn name(&self) -> &str {
        match self {
            Self::CreateBranch { name, .. } | Self::SwitchBranch { name } => name,
        }
    }

    /// The action marker and audit verb for this operation.
    fn action(&self) -> &'static str {
        match self {
            Self::CreateBranch { .. } => "git_branch_create",
            Self::SwitchBranch { .. } => "git_branch_switch",
        }
    }

    fn audited_verb(&self) -> &'static str {
        match self {
            Self::CreateBranch { .. } => "git_branch_created",
            Self::SwitchBranch { .. } => "git_branch_switched",
        }
    }

    /// The full ref this mutation creates or moves `HEAD` onto.
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
        )
    }

    /// Apply an approved preparation exactly once. The approved mutation must
    /// equal the record's, the record must still be `prepared`, and the same
    /// Code-mode capability is checked again after the review. Nothing in this
    /// method constructs `-f` or `--force`.
    pub fn apply(
        &self,
        prepare_id: &str,
        approved_mutation: &GitMutation,
        approved_by: &str,
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
        let args = mutation_args(&record.mutation);
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
                    GitMutation::SwitchBranch { .. } => None,
                };
                record.expected_ref_oid = record.resolved_start_sha.clone();
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
                self.audit_outcome(&record, approved_by, "ok", head_after.as_deref())?;
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
                let _ = self.audit_outcome(&record, approved_by, "failed", None);
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
            GitMutation::SwitchBranch { .. } => record.expected_ref_oid.clone(),
        };
        let (outcome, message) = match (&expected_oid, &actual) {
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
    ) -> Result<BranchPreparation> {
        let prepare_id = create_id("branch");
        let can_apply = refusal.is_none();
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
                    GitMutation::SwitchBranch { .. } => start_point_sha.clone(),
                },
                info: record_info,
                state: "prepared".to_string(),
                created_ref: None,
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
    ) -> Result<()> {
        self.audit_log.record(
            record.mutation.audited_verb(),
            &[
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
            ],
        )?;
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
/// working tree move with `HEAD` (decision D6). Neither form can carry a force
/// flag: there is no branch in this function that appends one.
fn mutation_args(mutation: &GitMutation) -> Vec<String> {
    match mutation {
        GitMutation::CreateBranch { name, start_point } => {
            vec!["branch".to_string(), name.clone(), start_point.clone()]
        }
        GitMutation::SwitchBranch { name } => {
            vec!["switch".to_string(), "--no-guess".to_string(), name.clone()]
        }
    }
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
