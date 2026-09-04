//! Session checkpoints, per `docs/specs/16_session_checkpoints_and_rewind.md`.
//!
//! Checkpoint content lives in a Damaian-owned Git object store, the way
//! `git stash create` builds a commit without moving anything: `GIT_DIR` points
//! at the store and `--work-tree` at the user's repository, so blobs land here
//! and the user's `.git` is never written to, read for state, or locked.
//! Content addressing, deduplication, packing, and `git gc` come with it.
//!
//! The store holds the user's bytes as they are — see the spec's §5.2. That is
//! not a weakening of secret redaction, which covers what leaves the user's
//! control: a checkpoint is a local copy made so a file can be put back, and
//! redacting it would destroy the file on restore. The obligations that follow
//! are that the store is never read into model context, never included in a
//! patch or diagnostic bundle, and never uploaded, and that manifests carry
//! paths, hashes, and counts but never content.

use crate::audit::AuditLog;
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis, repository_id_for_root, sha256};
use crate::path_policy::PathPolicy;
use crate::session::SessionStore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Why a path is in a checkpoint. Requirement 3's attribution is auditable
/// because the manifest says which agent action put the path in scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointOrigin {
    /// A file an accepted patch changed.
    Patch,
    /// A file an approved command created, modified, or deleted.
    Command,
}

/// A path the caller has attributed to an agent action, before policy is
/// applied to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointPath {
    pub path: String,
    pub origin: CheckpointOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointFile {
    pub path: String,
    /// SHA-256 of the content at capture time, or `None` when the file did not
    /// exist. Restore compares against this before writing anything.
    pub hash: Option<String>,
    pub existed: bool,
    /// The blob in the shadow store holding this file's bytes. `None` when
    /// there were no bytes to hold.
    pub oid: Option<String>,
    pub origin: CheckpointOrigin,
    pub mode: u32,
    /// What Damaian last left at this path, recorded when the checkpoint is
    /// sealed after its turn finishes: the hash of the content it wrote, and
    /// whether the file existed then.
    ///
    /// This is the comparison restore makes, for the same reason
    /// `RollbackSnapshot::applied_hash` exists: after the turn the file holds
    /// the agent's content, not the pre-turn content, so comparing against
    /// `hash` would report every agent change as a conflict. `None` means the
    /// checkpoint was never sealed — the turn did not run, or crashed — and
    /// restore then expects the captured state and refuses anything else.
    #[serde(default)]
    pub expected_hash: Option<String>,
    #[serde(default)]
    pub expected_existed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointExclusion {
    pub path: String,
    pub reason: String,
}

/// Where the conversation stood when the checkpoint was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointConversation {
    pub last_event_seq: u64,
    pub task_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    pub kind: String,
    pub proposal_id: String,
}

/// Which halves of a checkpoint to put back. Files and conversation are two
/// independent switches; `only_path` narrows the file half to one path.
#[derive(Debug, Clone)]
pub struct CheckpointRestoreOptions<'a> {
    pub files: bool,
    pub conversation: bool,
    pub only_path: Option<&'a str>,
}

/// Shaped like `PatchRollbackResult`, with skipped and conflicted split apart:
/// the older type folds both into `warnings`, which makes "the file changed
/// under you" indistinguishable from "there was nothing to do".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRestoreResult {
    pub checkpoint_id: String,
    pub restored_files: Vec<String>,
    pub deleted_files: Vec<String>,
    pub skipped_files: Vec<String>,
    pub conflicted_files: Vec<String>,
    pub conversation_restored: bool,
    pub warnings: Vec<String>,
}

/// What the caller knows when a checkpoint is taken: the turn it precedes, the
/// conversation position, and the paths it attributes to agent actions.
#[derive(Debug, Clone)]
pub struct CheckpointRequest<'a> {
    pub session_id: &'a str,
    pub task_id: Option<&'a str>,
    pub user_message_id: Option<&'a str>,
    pub summary: &'a str,
    pub conversation: CheckpointConversation,
    pub pending_approvals: Vec<PendingApproval>,
    pub paths: Vec<CheckpointPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointManifest {
    pub checkpoint_id: String,
    pub repository_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub created_at_ms: u128,
    pub user_message_id: Option<String>,
    pub summary: String,
    pub conversation: CheckpointConversation,
    pub pending_approvals: Vec<PendingApproval>,
    pub tree_oid: String,
    pub files: Vec<CheckpointFile>,
    /// Paths policy declined to cover, recorded rather than dropped so a user
    /// who wonders why `.env` was not restored gets an answer.
    pub excluded: Vec<CheckpointExclusion>,
    /// When this checkpoint was last restored from, for the list view.
    pub restored_at_ms: Option<u128>,
    /// False when the working-tree census exceeded its ceiling, so this
    /// checkpoint does not cover what approved commands changed. The UI says
    /// so rather than implying coverage it does not have.
    #[serde(default = "covered_by_default")]
    pub command_effects_covered: bool,
}

fn covered_by_default() -> bool {
    true
}

/// A read of the working tree: every path git reports as changed or untracked,
/// with the hash of what is there now, plus the pre-command content of those
/// paths in the shadow store.
///
/// Taken immediately before an approved command runs. The census records paths
/// and hashes; content is only kept for the paths that turn out to have
/// changed, and the rest is collected by the next cleanup.
#[derive(Debug, Clone)]
pub struct CommandCensus {
    /// Every path git reported, mapped to its content snapshot — `None` when
    /// the path is reported but not there, which is how a deletion is seen.
    entries: BTreeMap<String, Option<CensusEntry>>,
    /// True when the repository's census exceeded
    /// `checkpoint_census_max_paths`, so command effects are not covered.
    truncated: bool,
}

impl CommandCensus {
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn path_count(&self) -> usize {
        self.entries.len()
    }

    /// A census that could not be taken — the repository is not a git
    /// checkout, or git failed. Command effects are then not covered, and the
    /// checkpoint says so rather than the turn failing.
    pub fn unavailable() -> Self {
        Self {
            entries: BTreeMap::new(),
            truncated: true,
        }
    }

    /// What the census says about a path: not reported at all (clean and
    /// committed), reported but absent (deleted), or reported with this hash.
    /// Two censuses that disagree on a path are the command's effect on it.
    fn state(&self, path: &str) -> Option<Option<&str>> {
        self.entries
            .get(path)
            .map(|entry| entry.as_ref().map(|entry| entry.hash.as_str()))
    }

    fn snapshot(&self, path: &str) -> Option<&CensusEntry> {
        self.entries.get(path).and_then(Option::as_ref)
    }
}

#[derive(Debug, Clone)]
struct CensusEntry {
    hash: String,
    oid: String,
    mode: u32,
}

/// Git's mode for a regular file, and for one with the executable bit set.
/// Restoring a script without its executable bit leaves the user with a file
/// that no longer runs, so the mode is part of the snapshot.
pub(crate) const FILE_MODE: u32 = 0o100644;
pub(crate) const EXECUTABLE_FILE_MODE: u32 = 0o100755;

#[derive(Debug, Clone)]
pub struct CheckpointStore {
    config: Config,
    audit_log: AuditLog,
    path_policy: PathPolicy,
}

impl CheckpointStore {
    pub fn new(config: Config, audit_log: AuditLog, path_policy: PathPolicy) -> Self {
        Self {
            config,
            audit_log,
            path_policy,
        }
    }

    /// Snapshots the in-scope paths as they are now, before the turn changes
    /// them. Reads the working tree and writes only to Damaian's own store.
    pub fn create_checkpoint(
        &self,
        repository_root: impl AsRef<Path>,
        request: CheckpointRequest<'_>,
    ) -> Result<CheckpointManifest> {
        let root = self.path_policy.canonical_root(&repository_root)?;
        let repository_id = repository_id_for_root(&root);
        let store = ObjectStore::open(&self.config.data_dir, &repository_id)?;

        let mut files = Vec::new();
        let mut excluded = Vec::new();
        for requested in &request.paths {
            match self.snapshot_path(&store, &root, requested)? {
                PathOutcome::Snapshotted(file) => files.push(file),
                PathOutcome::Excluded(exclusion) => excluded.push(exclusion),
            }
        }

        let checkpoint_id = create_id("checkpoint");
        let mut manifest = CheckpointManifest {
            checkpoint_id,
            repository_id,
            session_id: request.session_id.to_string(),
            task_id: request.task_id.map(str::to_string),
            created_at_ms: now_millis(),
            user_message_id: request.user_message_id.map(str::to_string),
            summary: request.summary.to_string(),
            conversation: request.conversation,
            pending_approvals: request.pending_approvals,
            files,
            excluded,
            restored_at_ms: None,
            command_effects_covered: true,
            tree_oid: String::new(),
        };
        // The ref is what keeps these blobs alive: cleanup deletes the ref and
        // lets `git gc` collect whatever nothing else reaches.
        self.reference_content(&store, &mut manifest)?;
        self.write_manifest(&manifest)?;

        self.audit_log.record(
            "checkpoint_created",
            &[
                ("actor", "system".to_string()),
                ("sessionId", manifest.session_id.clone()),
                ("taskId", manifest.task_id.clone().unwrap_or_default()),
                ("checkpointId", manifest.checkpoint_id.clone()),
                ("fileCount", manifest.files.len().to_string()),
                ("excludedCount", manifest.excluded.len().to_string()),
                (
                    "files",
                    manifest
                        .files
                        .iter()
                        .map(|file| file.path.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                ),
            ],
        )?;
        Ok(manifest)
    }

    /// Puts the checkpoint back: agent-managed files, conversation position, or
    /// both — requirement 5's four operations are these two switches, plus
    /// `only_path` for a single file.
    ///
    /// A conflict is a reported outcome rather than an error, and it stops every
    /// write in the invocation: a partial restore with a conflict in the middle
    /// leaves a tree that is neither the checkpoint nor what the user had.
    pub fn restore(
        &self,
        repository_root: impl AsRef<Path>,
        manifest: &CheckpointManifest,
        options: CheckpointRestoreOptions<'_>,
        approved_by: &str,
    ) -> Result<CheckpointRestoreResult> {
        let root = self.path_policy.canonical_root(&repository_root)?;
        let mut result = CheckpointRestoreResult {
            checkpoint_id: manifest.checkpoint_id.clone(),
            restored_files: Vec::new(),
            deleted_files: Vec::new(),
            skipped_files: Vec::new(),
            conflicted_files: Vec::new(),
            conversation_restored: false,
            warnings: Vec::new(),
        };

        if options.files {
            self.restore_files(&root, manifest, &options, approved_by, &mut result)?;
        }
        if options.conversation && result.conflicted_files.is_empty() {
            SessionStore::new(&self.config.data_dir)
                .rewind_conversation(&manifest.session_id, manifest.conversation.last_event_seq)?;
            result.conversation_restored = true;
        }

        if result.conflicted_files.is_empty() {
            // Restore has just changed these files itself, so what Damaian last
            // left at those paths is now the checkpoint's own content. Without
            // this, rewinding one file and then the turn would report the first
            // file as a conflict with a change Damaian made.
            self.mark_restored(manifest, &result)?;
        }
        self.audit_log.record(
            "checkpoint_restored",
            &[
                ("actor", "user".to_string()),
                ("approvedBy", approved_by.to_string()),
                ("sessionId", manifest.session_id.clone()),
                ("checkpointId", manifest.checkpoint_id.clone()),
                ("restoredFiles", result.restored_files.join(",")),
                ("deletedFiles", result.deleted_files.join(",")),
                ("skippedFiles", result.skipped_files.join(",")),
                ("conflictedFiles", result.conflicted_files.join(",")),
                (
                    "conversationRestored",
                    result.conversation_restored.to_string(),
                ),
            ],
        )?;
        Ok(result)
    }

    fn restore_files(
        &self,
        root: &Path,
        manifest: &CheckpointManifest,
        options: &CheckpointRestoreOptions<'_>,
        approved_by: &str,
        result: &mut CheckpointRestoreResult,
    ) -> Result<()> {
        if let Some(path) = options.only_path
            && !manifest.files.iter().any(|file| file.path == path)
        {
            return Err(ClientError::InvalidInput(format!(
                "Selected file is not part of checkpoint {}: {path}",
                manifest.checkpoint_id
            )));
        }

        let selected = manifest
            .files
            .iter()
            .filter(|file| options.only_path.is_none_or(|path| file.path == path));

        // Decide everything before writing anything, so conflicts are reported
        // as a set and a refusal leaves the tree exactly as it was.
        let mut planned = Vec::new();
        for file in selected {
            let resolved = match self.path_policy.resolve_for_write(root, &file.path) {
                Ok(resolved) => resolved,
                Err(ClientError::AccessDenied(reason)) => {
                    result.skipped_files.push(file.path.clone());
                    result.warnings.push(format!("{}: {reason}", file.path));
                    continue;
                }
                Err(error) => return Err(error),
            };
            if self
                .path_policy
                .is_restricted(&resolved.relative_path, false)
            {
                result.skipped_files.push(file.path.clone());
                result.warnings.push(format!(
                    "{}: restricted by policy at restore time, skipped",
                    file.path
                ));
                continue;
            }

            let current = match std::fs::read(&resolved.absolute_path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(ClientError::from(error)),
            };
            match plan_for(file, current.as_deref()) {
                PlannedRestore::Write => planned.push((file, resolved.absolute_path, true)),
                PlannedRestore::Delete => planned.push((file, resolved.absolute_path, false)),
                PlannedRestore::Skip => result.skipped_files.push(file.path.clone()),
                PlannedRestore::Conflict => {
                    result.conflicted_files.push(file.path.clone());
                    result.warnings.push(format!(
                        "{}: changed since the checkpoint was taken, not restored",
                        file.path
                    ));
                }
            }
        }

        if !result.conflicted_files.is_empty() {
            self.audit_log.record(
                "checkpoint_conflicted",
                &[
                    ("actor", "user".to_string()),
                    ("approvedBy", approved_by.to_string()),
                    ("checkpointId", manifest.checkpoint_id.clone()),
                    ("conflictedFiles", result.conflicted_files.join(",")),
                ],
            )?;
            return Ok(());
        }

        let store = ObjectStore::open(&self.config.data_dir, &manifest.repository_id)?;
        for (file, absolute_path, write) in planned {
            if write {
                let oid = file.oid.as_deref().ok_or_else(|| {
                    ClientError::Io(format!(
                        "Checkpoint {} has no stored content for {}",
                        manifest.checkpoint_id, file.path
                    ))
                })?;
                let bytes = store.read_blob(oid)?;
                if let Some(parent) = absolute_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let temp_path = temp_path_for(&absolute_path);
                std::fs::write(&temp_path, &bytes)?;
                std::fs::rename(&temp_path, &absolute_path)?;
                apply_mode(&absolute_path, file.mode)?;
                result.restored_files.push(file.path.clone());
            } else {
                std::fs::remove_file(&absolute_path)?;
                result.deleted_files.push(file.path.clone());
            }
        }

        if !result.skipped_files.is_empty() {
            self.audit_log.record(
                "checkpoint_files_skipped",
                &[
                    ("actor", "user".to_string()),
                    ("approvedBy", approved_by.to_string()),
                    ("checkpointId", manifest.checkpoint_id.clone()),
                    ("skippedFiles", result.skipped_files.join(",")),
                ],
            )?;
        }
        Ok(())
    }

    /// Records what the turn actually left at each in-scope path, and returns
    /// the sealed manifest. Call this when the turn the checkpoint precedes has
    /// finished: without it, restore cannot tell the agent's own changes from
    /// somebody else's, and conservatively refuses to overwrite either.
    pub fn seal_checkpoint(
        &self,
        repository_root: impl AsRef<Path>,
        manifest: &CheckpointManifest,
    ) -> Result<CheckpointManifest> {
        let root = self.path_policy.canonical_root(&repository_root)?;
        let mut sealed = manifest.clone();
        for file in &mut sealed.files {
            let Ok(resolved) = self.path_policy.resolve_for_write(&root, &file.path) else {
                continue;
            };
            match std::fs::read(&resolved.absolute_path) {
                Ok(bytes) => {
                    file.expected_hash = Some(sha256(&bytes));
                    file.expected_existed = Some(true);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    file.expected_hash = None;
                    file.expected_existed = Some(false);
                }
                Err(error) => return Err(ClientError::from(error)),
            }
        }
        self.write_manifest(&sealed)?;
        Ok(sealed)
    }

    fn mark_restored(
        &self,
        manifest: &CheckpointManifest,
        result: &CheckpointRestoreResult,
    ) -> Result<()> {
        let mut updated = manifest.clone();
        updated.restored_at_ms = Some(now_millis());
        for file in &mut updated.files {
            if result.restored_files.contains(&file.path)
                || result.deleted_files.contains(&file.path)
            {
                file.expected_hash = file.hash.clone();
                file.expected_existed = Some(file.existed);
            }
        }
        self.write_manifest(&updated)
    }

    pub fn read_checkpoint(
        &self,
        repository_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CheckpointManifest>> {
        let path = self
            .manifests_dir(repository_id)
            .join(format!("{checkpoint_id}.json"));
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        Ok(Some(parse_manifest(&raw, &path)?))
    }

    /// The newest checkpoint taken for a task, which is the one the turn in
    /// flight is adding to. The turn's own state is spread over several
    /// requests when it pauses for approval, so the task id is what connects
    /// them rather than a handle held in memory.
    pub fn read_checkpoint_for_task(
        &self,
        repository_id: &str,
        task_id: &str,
    ) -> Result<Option<CheckpointManifest>> {
        Ok(self
            .list_checkpoints(repository_id)?
            .into_iter()
            .find(|manifest| manifest.task_id.as_deref() == Some(task_id)))
    }

    /// Newest first, which is the order the list view shows and the order a
    /// user thinks in when they want to go back one turn.
    pub fn list_checkpoints(&self, repository_id: &str) -> Result<Vec<CheckpointManifest>> {
        let dir = self.manifests_dir(repository_id);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };
        let mut manifests = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            manifests.push(parse_manifest(&std::fs::read_to_string(&path)?, &path)?);
        }
        manifests.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then(right.checkpoint_id.cmp(&left.checkpoint_id))
        });
        Ok(manifests)
    }

    /// Adds paths to an existing checkpoint, snapshotting each as it is now.
    ///
    /// A checkpoint is taken before the turn, when nothing knows yet which
    /// files the turn will touch, so the paths arrive as the turn accepts a
    /// patch or runs a command. A path already in the checkpoint keeps its
    /// first snapshot: that is the older content, and the one rewind wants.
    pub fn add_paths(
        &self,
        repository_root: impl AsRef<Path>,
        manifest: &CheckpointManifest,
        paths: Vec<CheckpointPath>,
    ) -> Result<CheckpointManifest> {
        let root = self.path_policy.canonical_root(&repository_root)?;
        let store = ObjectStore::open(&self.config.data_dir, &manifest.repository_id)?;
        let mut updated = manifest.clone();
        let mut added = Vec::new();
        for requested in &paths {
            if updated.files.iter().any(|file| file.path == requested.path)
                || updated
                    .excluded
                    .iter()
                    .any(|exclusion| exclusion.path == requested.path)
            {
                continue;
            }
            match self.snapshot_path(&store, &root, requested)? {
                PathOutcome::Snapshotted(file) => {
                    added.push(file.path.clone());
                    updated.files.push(file);
                }
                PathOutcome::Excluded(exclusion) => updated.excluded.push(exclusion),
            }
        }
        if added.is_empty() && updated.files.len() == manifest.files.len() {
            self.write_manifest(&updated)?;
            return Ok(updated);
        }

        self.reference_content(&store, &mut updated)?;
        self.write_manifest(&updated)?;
        self.audit_log.record(
            "checkpoint_paths_added",
            &[
                ("actor", "system".to_string()),
                ("sessionId", updated.session_id.clone()),
                ("checkpointId", updated.checkpoint_id.clone()),
                ("files", added.join(",")),
            ],
        )?;
        Ok(updated)
    }

    /// Rebuilds the checkpoint's tree and ref so every blob it now holds is
    /// reachable. Until a ref reaches them, new blobs are collectable.
    fn reference_content(
        &self,
        store: &ObjectStore,
        manifest: &mut CheckpointManifest,
    ) -> Result<()> {
        let entries = manifest
            .files
            .iter()
            .filter_map(|file| {
                file.oid.as_ref().map(|oid| TreeEntry {
                    path: file.path.clone(),
                    oid: oid.clone(),
                    mode: file.mode,
                })
            })
            .collect::<Vec<_>>();
        manifest.tree_oid = store.write_tree(&entries)?;
        let commit_oid = store.commit_tree(&manifest.tree_oid, &manifest.checkpoint_id)?;
        store.set_checkpoint_ref(&manifest.checkpoint_id, &commit_oid)
    }

    /// Records what the turn is waiting on, replacing whatever was there:
    /// pass an empty list once the approval is decided, so the checkpoint never
    /// describes a pause that has already ended.
    pub fn set_pending_approvals(
        &self,
        manifest: &CheckpointManifest,
        pending_approvals: Vec<PendingApproval>,
    ) -> Result<CheckpointManifest> {
        let mut updated = manifest.clone();
        updated.pending_approvals = pending_approvals;
        self.write_manifest(&updated)?;
        Ok(updated)
    }

    /// Reads the working tree immediately before an approved command runs, and
    /// snapshots the content of every path it covers, so what the command
    /// changes can be put back afterwards.
    ///
    /// Only paths git reports as changed or untracked are covered, which is a
    /// handful on a normal repository rather than the whole tree.
    pub fn begin_command_census(&self, repository_root: impl AsRef<Path>) -> Result<CommandCensus> {
        let root = self.path_policy.canonical_root(&repository_root)?;
        let repository_id = repository_id_for_root(&root);
        let store = ObjectStore::open(&self.config.data_dir, &repository_id)?;
        let paths = self.censused_paths(&root)?;

        let ceiling =
            usize::try_from(self.config.checkpoint_census_max_paths).unwrap_or(usize::MAX);
        if paths.len() > ceiling {
            // Covering some of a repository's command effects while implying
            // all of them is the failure this ceiling exists to avoid.
            return Ok(CommandCensus {
                entries: BTreeMap::new(),
                truncated: true,
            });
        }

        let mut entries = BTreeMap::new();
        let mut present = Vec::new();
        for path in paths {
            let absolute = root.join(&path);
            match std::fs::read(&absolute) {
                Ok(bytes) => present.push((path, absolute, bytes)),
                // Reported but not there: git is telling us the path is
                // deleted, which is a state worth recording rather than
                // dropping.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    entries.insert(path, None);
                }
                Err(error) => return Err(ClientError::from(error)),
            }
        }
        let oids = store.write_blobs(
            &present
                .iter()
                .map(|(_, absolute, _)| absolute.clone())
                .collect::<Vec<_>>(),
        )?;
        for ((path, absolute, bytes), oid) in present.into_iter().zip(oids) {
            entries.insert(
                path,
                Some(CensusEntry {
                    hash: sha256(&bytes),
                    oid,
                    mode: file_mode(&absolute),
                }),
            );
        }
        Ok(CommandCensus {
            entries,
            truncated: false,
        })
    }

    /// Diffs a second census against the first and adds what the command
    /// changed to the checkpoint, with `origin: command`.
    ///
    /// A path that was clean before the command has no snapshot in the census,
    /// so its pre-command content comes from the user's own Git objects — read
    /// with `--no-optional-locks`, never written to and never locked. Without
    /// that, the most common case (a formatter rewriting committed files) would
    /// be uncoverable.
    pub fn record_command_effects(
        &self,
        repository_root: impl AsRef<Path>,
        manifest: &CheckpointManifest,
        before: &CommandCensus,
    ) -> Result<CheckpointManifest> {
        let root = self.path_policy.canonical_root(&repository_root)?;
        let mut updated = manifest.clone();
        if before.truncated {
            updated.command_effects_covered = false;
            self.write_manifest(&updated)?;
            return Ok(updated);
        }

        let after = self.begin_command_census(&root)?;
        if after.truncated {
            updated.command_effects_covered = false;
            self.write_manifest(&updated)?;
            return Ok(updated);
        }

        let store = ObjectStore::open(&self.config.data_dir, &manifest.repository_id)?;
        let mut changed = before
            .entries
            .keys()
            .chain(after.entries.keys())
            .cloned()
            .collect::<Vec<_>>();
        changed.sort();
        changed.dedup();

        for path in changed {
            if before.state(&path) == after.state(&path) {
                continue;
            }
            // A patch captured this path before the turn started, and that
            // snapshot is the older, more useful one.
            if updated.files.iter().any(|file| file.path == path) {
                continue;
            }

            let file = match before.snapshot(&path) {
                Some(entry) => CheckpointFile {
                    path: path.clone(),
                    hash: Some(entry.hash.clone()),
                    existed: true,
                    oid: Some(entry.oid.clone()),
                    origin: CheckpointOrigin::Command,
                    mode: entry.mode,
                    expected_hash: None,
                    expected_existed: None,
                },
                // Not in the before census: either committed and clean, or it
                // did not exist at all.
                None => match self.committed_content(&root, &path)? {
                    Some(bytes) => CheckpointFile {
                        hash: Some(sha256(&bytes)),
                        existed: true,
                        oid: Some(store.write_blob(&bytes)?),
                        origin: CheckpointOrigin::Command,
                        mode: FILE_MODE,
                        path: path.clone(),
                        expected_hash: None,
                        expected_existed: None,
                    },
                    None => CheckpointFile {
                        path: path.clone(),
                        hash: None,
                        existed: false,
                        oid: None,
                        origin: CheckpointOrigin::Command,
                        mode: FILE_MODE,
                        expected_hash: None,
                        expected_existed: None,
                    },
                },
            };
            updated.files.push(file);
        }

        // The tree and its ref have to be rebuilt: the new blobs are only
        // protected from collection once something reaches them.
        self.reference_content(&store, &mut updated)?;
        self.write_manifest(&updated)?;
        Ok(updated)
    }

    /// The paths git reports as changed or untracked, filtered by policy.
    fn censused_paths(&self, root: &Path) -> Result<Vec<String>> {
        let output = run_user_git(
            root,
            &["status", "--porcelain=v1", "--untracked-files=all", "-z"],
        )?;
        let mut paths = Vec::new();
        // `-z` output is NUL-separated and unquoted, so a path with a space or
        // a quote in it survives. A rename entry carries the original path in a
        // second field, and both sides are the command's effect.
        let mut fields = output
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty());
        while let Some(field) = fields.next() {
            let text = String::from_utf8_lossy(field);
            let Some((status, path)) = text.split_at_checked(3) else {
                continue;
            };
            if (status.starts_with('R') || status.starts_with('C'))
                && let Some(original) = fields.next()
            {
                self.push_censused(root, &mut paths, &String::from_utf8_lossy(original));
            }
            self.push_censused(root, &mut paths, path);
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    fn push_censused(&self, root: &Path, paths: &mut Vec<String>, path: &str) {
        let path = path.trim();
        if path.is_empty()
            || self.path_policy.is_restricted(path, false)
            || self.is_own_data_path(root, path)
        {
            return;
        }
        paths.push(path.to_string());
    }

    /// True when a repository-relative path is inside Damaian's own data
    /// directory. The documented development setup puts it there
    /// (`DAMAIAN_DATA_DIR=.damaian`), and snapshotting it would put the audit
    /// log, the sessions, and the checkpoint store itself into a checkpoint —
    /// so a rewind would roll back Damaian's own state along with the user's.
    fn is_own_data_path(&self, root: &Path, relative_path: &str) -> bool {
        let data_dir = std::fs::canonicalize(&self.config.data_dir)
            .unwrap_or_else(|_| self.config.data_dir.clone());
        let Ok(prefix) = data_dir.strip_prefix(root) else {
            return false;
        };
        let prefix = prefix.to_string_lossy().replace('\\', "/");
        if prefix.is_empty() {
            return true;
        }
        relative_path == prefix || relative_path.starts_with(&format!("{prefix}/"))
    }

    /// The content of a tracked path as `HEAD` has it, or `None` when `HEAD`
    /// does not have it. Read-only: no lock, no index refresh, no ref update.
    fn committed_content(&self, root: &Path, path: &str) -> Result<Option<Vec<u8>>> {
        let spec = format!("HEAD:{path}");
        match run_user_git(root, &["cat-file", "blob", &spec]) {
            Ok(bytes) => Ok(Some(bytes)),
            // Not in HEAD, or no HEAD yet: the path did not exist before.
            Err(_) => Ok(None),
        }
    }

    /// Drops checkpoints past the retention window, then the oldest ones still
    /// over the size ceiling, and collects what nothing references any more.
    /// Returns the checkpoint ids it removed.
    ///
    /// The newest checkpoint of the session the user is still in is never
    /// dropped, however old: during a long session that is exactly the one they
    /// reach for, and exactly the one a time-based rule would take. Pass the
    /// open session as `active_session_id`; `None` means no session is open.
    pub fn cleanup(
        &self,
        repository_id: &str,
        active_session_id: Option<&str>,
    ) -> Result<Vec<String>> {
        let manifests = self.list_checkpoints(repository_id)?;
        // `list_checkpoints` is newest first, so the first match is the one to
        // keep.
        let protected_id = active_session_id.and_then(|session_id| {
            manifests
                .iter()
                .find(|manifest| manifest.session_id == session_id)
                .map(|manifest| manifest.checkpoint_id.clone())
        });

        let retention_ms = u128::from(self.config.checkpoint_retention_days) * 24 * 60 * 60 * 1000;
        let cutoff = now_millis().saturating_sub(retention_ms);
        let protected = |manifest: &CheckpointManifest| {
            protected_id.as_deref() == Some(manifest.checkpoint_id.as_str())
        };

        let mut expired = manifests
            .iter()
            .filter(|manifest| manifest.created_at_ms < cutoff && !protected(manifest))
            .map(|manifest| manifest.checkpoint_id.clone())
            .collect::<Vec<_>>();

        // Oldest first for the size pass: the ceiling is a disk guard, so the
        // least useful checkpoints go first.
        let mut over_ceiling = manifests
            .iter()
            .rev()
            .filter(|manifest| !expired.contains(&manifest.checkpoint_id) && !protected(manifest))
            .map(|manifest| manifest.checkpoint_id.clone())
            .collect::<Vec<_>>()
            .into_iter();
        while self.store_bytes(repository_id)? > self.config.checkpoint_max_total_bytes {
            let Some(next) = over_ceiling.next() else {
                break;
            };
            self.remove_checkpoint(repository_id, &next)?;
            expired.push(next);
        }

        for checkpoint_id in &expired {
            self.remove_checkpoint(repository_id, checkpoint_id)?;
        }

        if !expired.is_empty() {
            // Only worth the cost when something was actually dropped: this
            // runs on every turn, and `git gc` is not free.
            ObjectStore::open(&self.config.data_dir, repository_id)?.collect_garbage()?;
            self.audit_log.record(
                "checkpoint_cleanup",
                &[
                    ("actor", "system".to_string()),
                    ("repositoryId", repository_id.to_string()),
                    ("removedCount", expired.len().to_string()),
                    ("checkpointIds", expired.join(",")),
                    (
                        "retentionDays",
                        self.config.checkpoint_retention_days.to_string(),
                    ),
                ],
            )?;
        }
        Ok(expired)
    }

    /// Removing a checkpoint is deleting its manifest and its ref. The blobs go
    /// when `collect_garbage` finds nothing reaching them.
    fn remove_checkpoint(&self, repository_id: &str, checkpoint_id: &str) -> Result<()> {
        let manifest_path = self
            .manifests_dir(repository_id)
            .join(format!("{checkpoint_id}.json"));
        if manifest_path.exists() {
            std::fs::remove_file(&manifest_path)?;
        }
        let store = ObjectStore::open(&self.config.data_dir, repository_id)?;
        store.delete_checkpoint_ref(checkpoint_id)
    }

    fn store_bytes(&self, repository_id: &str) -> Result<u64> {
        let root = self.config.data_dir.join("checkpoints").join(repository_id);
        Ok(directory_bytes(&root))
    }

    fn snapshot_path(
        &self,
        store: &ObjectStore,
        root: &Path,
        requested: &CheckpointPath,
    ) -> Result<PathOutcome> {
        // `resolve_for_write` is the same guard patch apply uses: it refuses a
        // path outside the repository and a symlink that resolves outside it.
        let resolved = match self.path_policy.resolve_for_write(root, &requested.path) {
            Ok(resolved) => resolved,
            Err(ClientError::AccessDenied(_)) => {
                return Ok(PathOutcome::Excluded(CheckpointExclusion {
                    path: requested.path.clone(),
                    reason: "outside_repository".to_string(),
                }));
            }
            Err(error) => return Err(error),
        };
        if self
            .path_policy
            .is_restricted(&resolved.relative_path, false)
        {
            return Ok(PathOutcome::Excluded(CheckpointExclusion {
                path: resolved.relative_path,
                reason: "restricted_pattern".to_string(),
            }));
        }
        if self.is_own_data_path(root, &resolved.relative_path) {
            return Ok(PathOutcome::Excluded(CheckpointExclusion {
                path: resolved.relative_path,
                reason: "damaian_data_directory".to_string(),
            }));
        }

        match std::fs::read(&resolved.absolute_path) {
            Ok(bytes) => Ok(PathOutcome::Snapshotted(CheckpointFile {
                hash: Some(sha256(&bytes)),
                existed: true,
                oid: Some(store.write_blob(&bytes)?),
                mode: file_mode(&resolved.absolute_path),
                path: resolved.relative_path,
                origin: requested.origin,
                expected_hash: None,
                expected_existed: None,
            })),
            // Absent now means the turn is about to create it, and restore
            // deletes it rather than inferring intent from absence.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(PathOutcome::Snapshotted(CheckpointFile {
                    path: resolved.relative_path,
                    hash: None,
                    existed: false,
                    oid: None,
                    origin: requested.origin,
                    mode: FILE_MODE,
                    expected_hash: None,
                    expected_existed: None,
                }))
            }
            Err(error) => Err(ClientError::from(error)),
        }
    }

    fn write_manifest(&self, manifest: &CheckpointManifest) -> Result<()> {
        let dir = self.manifests_dir(&manifest.repository_id);
        std::fs::create_dir_all(&dir)?;
        let serialized = serde_json::to_string_pretty(manifest).map_err(|error| {
            ClientError::Io(format!("Failed to write checkpoint manifest: {error}"))
        })?;
        std::fs::write(
            dir.join(format!("{}.json", manifest.checkpoint_id)),
            serialized,
        )?;
        Ok(())
    }

    fn manifests_dir(&self, repository_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("checkpoints")
            .join(repository_id)
            .join("manifests")
    }
}

enum PathOutcome {
    Snapshotted(CheckpointFile),
    Excluded(CheckpointExclusion),
}

enum PlannedRestore {
    Write,
    Delete,
    Skip,
    Conflict,
}

/// §5.6's table, in one place: what Damaian last left at the path decides
/// whether restore may touch it, and what the checkpoint captured decides what
/// it does.
fn plan_for(file: &CheckpointFile, current: Option<&[u8]>) -> PlannedRestore {
    let expected_existed = file.expected_existed.unwrap_or(file.existed);
    let expected_hash = file
        .expected_hash
        .as_deref()
        .or(file.hash.as_deref())
        .unwrap_or_default();
    let unchanged_since_the_turn = match (expected_existed, current) {
        (true, Some(bytes)) => expected_hash == sha256(bytes),
        // Nothing on disk to lose, so writing the checkpoint's content back is
        // safe rather than a conflict.
        (true, None) => true,
        (false, None) => true,
        // Damaian left no file here, so whatever is here now came from
        // somewhere else. Deleting it would destroy work it cannot attribute.
        (false, Some(_)) => false,
    };
    if !unchanged_since_the_turn {
        return PlannedRestore::Conflict;
    }

    match (file.existed, current) {
        (true, _) => PlannedRestore::Write,
        (false, Some(_)) => PlannedRestore::Delete,
        (false, None) => PlannedRestore::Skip,
    }
}

/// Runs git in the user's repository, read-only. `--no-optional-locks` is what
/// makes that true: without it `git status` may refresh and rewrite the user's
/// index, which §5.1 promises Damaian never does.
fn run_user_git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        return Err(ClientError::Git(format!(
            "git {} failed in {}: {}",
            args.first().copied().unwrap_or_default(),
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn directory_bytes(root: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.metadata() {
                Ok(metadata) if metadata.is_dir() => pending.push(entry.path()),
                Ok(metadata) => total += metadata.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn temp_path_for(path: &Path) -> PathBuf {
    let mut file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "checkpoint".to_string());
    file_name.push_str(".damaian-restore");
    path.with_file_name(file_name)
}

fn apply_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The snapshot's mode is a Git file mode; only its permission bits
        // matter on disk, and only the executable bit varies.
        let permissions = std::fs::Permissions::from_mode(if mode == EXECUTABLE_FILE_MODE {
            0o755
        } else {
            0o644
        });
        std::fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn parse_manifest(raw: &str, path: &Path) -> Result<CheckpointManifest> {
    serde_json::from_str(raw).map_err(|error| {
        ClientError::Io(format!(
            "Corrupt checkpoint manifest at {}: {error}",
            path.display()
        ))
    })
}

/// A script restored without its executable bit no longer runs, so the mode
/// travels with the snapshot.
fn file_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path)
            && metadata.permissions().mode() & 0o111 != 0
        {
            return EXECUTABLE_FILE_MODE;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    FILE_MODE
}

/// One path in a checkpoint's tree.
pub(crate) struct TreeEntry {
    pub(crate) path: String,
    pub(crate) oid: String,
    pub(crate) mode: u32,
}

/// The shadow object store for one repository:
/// `<data_dir>/checkpoints/<repository_id>/objects` as `GIT_DIR`.
pub(crate) struct ObjectStore {
    git_dir: PathBuf,
}

impl ObjectStore {
    pub(crate) fn open(data_dir: &Path, repository_id: &str) -> Result<Self> {
        let git_dir = data_dir
            .join("checkpoints")
            .join(repository_id)
            .join("objects");
        if !git_dir.join("HEAD").exists() {
            std::fs::create_dir_all(&git_dir)?;
            run_git(&git_dir, &["init", "--bare", "--quiet"], None)?;
        }
        Ok(Self { git_dir })
    }

    pub(crate) fn write_blob(&self, bytes: &[u8]) -> Result<String> {
        let output = run_git(
            &self.git_dir,
            &["hash-object", "-w", "--stdin"],
            Some(bytes),
        )?;
        Ok(String::from_utf8_lossy(&output).trim().to_string())
    }

    /// Writes many files' blobs in one git process. The census hashes every
    /// changed path before each approved command, and a spawn per file is most
    /// of that cost.
    ///
    /// `--no-filters` matches what [`Self::write_blob`] does with `--stdin`:
    /// without it git would apply the repository's `.gitattributes` clean
    /// filters — CRLF conversion and friends — and the stored bytes would no
    /// longer be the user's bytes, which is the one thing this store promises.
    pub(crate) fn write_blobs(&self, paths: &[PathBuf]) -> Result<Vec<String>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        // `--stdin-paths` is newline-separated, so a path containing one cannot
        // go through it. Pathological, but silently mis-hashing it would be
        // worse than the slow path.
        if paths
            .iter()
            .any(|path| path.to_string_lossy().contains('\n'))
        {
            return paths
                .iter()
                .map(|path| self.write_blob(&std::fs::read(path)?))
                .collect();
        }

        let mut stdin = String::new();
        for path in paths {
            stdin.push_str(&path.to_string_lossy());
            stdin.push('\n');
        }
        let output = run_git(
            &self.git_dir,
            &["hash-object", "-w", "--no-filters", "--stdin-paths"],
            Some(stdin.as_bytes()),
        )?;
        let oids = String::from_utf8_lossy(&output)
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if oids.len() != paths.len() {
            return Err(ClientError::Io(format!(
                "git hash-object returned {} object ids for {} paths",
                oids.len(),
                paths.len()
            )));
        }
        Ok(oids)
    }

    pub(crate) fn read_blob(&self, oid: &str) -> Result<Vec<u8>> {
        run_git(&self.git_dir, &["cat-file", "blob", oid], None)
    }

    /// Builds a tree for the checkpoint's paths through a store-local index, so
    /// the user's index is neither read nor written.
    pub(crate) fn write_tree(&self, entries: &[TreeEntry]) -> Result<String> {
        let index_path = self.git_dir.join("damaian-checkpoint-index");
        if index_path.exists() {
            std::fs::remove_file(&index_path)?;
        }
        let mut index_info = String::new();
        for entry in entries {
            // `update-index --index-info` takes one line per path, which keeps
            // this to a single git invocation however many files a turn touched.
            index_info.push_str(&format!("{:o} {}\t{}\n", entry.mode, entry.oid, entry.path));
        }
        run_git_with_index(
            &self.git_dir,
            &index_path,
            &["update-index", "--add", "--index-info"],
            Some(index_info.as_bytes()),
        )?;
        let output = run_git_with_index(&self.git_dir, &index_path, &["write-tree"], None)?;
        Ok(String::from_utf8_lossy(&output).trim().to_string())
    }

    pub(crate) fn commit_tree(&self, tree_oid: &str, message: &str) -> Result<String> {
        let output = run_git(
            &self.git_dir,
            &["commit-tree", tree_oid, "-m", message],
            None,
        )?;
        Ok(String::from_utf8_lossy(&output).trim().to_string())
    }

    /// Points a ref at the checkpoint's commit. Reachability is what protects a
    /// live checkpoint's blobs from [`Self::collect_garbage`].
    pub(crate) fn set_checkpoint_ref(&self, checkpoint_id: &str, commit_oid: &str) -> Result<()> {
        run_git(
            &self.git_dir,
            &["update-ref", &checkpoint_ref(checkpoint_id), commit_oid],
            None,
        )?;
        Ok(())
    }

    pub(crate) fn delete_checkpoint_ref(&self, checkpoint_id: &str) -> Result<()> {
        run_git(
            &self.git_dir,
            &["update-ref", "-d", &checkpoint_ref(checkpoint_id)],
            None,
        )?;
        Ok(())
    }

    /// Collects what no checkpoint ref reaches. `--prune=now` rather than the
    /// default grace period: the blobs of a just-expired checkpoint are exactly
    /// what this is meant to remove.
    pub(crate) fn collect_garbage(&self) -> Result<()> {
        run_git(&self.git_dir, &["gc", "--quiet", "--prune=now"], None)?;
        Ok(())
    }
}

fn checkpoint_ref(checkpoint_id: &str) -> String {
    format!("refs/damaian/checkpoints/{checkpoint_id}")
}

fn run_git(git_dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>> {
    run_git_inner(git_dir, None, args, stdin)
}

fn run_git_with_index(
    git_dir: &Path,
    index_path: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> Result<Vec<u8>> {
    run_git_inner(git_dir, Some(index_path), args, stdin)
}

fn run_git_inner(
    git_dir: &Path,
    index_path: Option<&Path>,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command.arg("--git-dir").arg(git_dir).args(args);
    if let Some(index_path) = index_path {
        command.env("GIT_INDEX_FILE", index_path);
    }
    // A commit needs an identity, and the user's own `user.name` may be unset
    // or scoped to their repository. These never appear in the user's history:
    // the commits exist only in Damaian's store.
    command
        .env("GIT_AUTHOR_NAME", "Damaian")
        .env("GIT_AUTHOR_EMAIL", "checkpoints@damaian.local")
        .env("GIT_COMMITTER_NAME", "Damaian")
        .env("GIT_COMMITTER_EMAIL", "checkpoints@damaian.local")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn()?;
    if let Some(bytes) = stdin {
        child
            .stdin
            .as_mut()
            .ok_or_else(|| ClientError::Io("git stdin was not available".to_string()))?
            .write_all(bytes)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(ClientError::Io(format!(
            "git {} failed in the checkpoint store: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_data_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "damaian-checkpoint-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    // Requirement 9's foundation: the store holds the user's bytes as they are.
    // A redacted snapshot does not protect the secret, it destroys the file on
    // restore.
    #[test]
    fn stores_and_reads_back_bytes_faithfully() {
        let data_dir = temp_data_dir("faithful");
        let store = ObjectStore::open(&data_dir, "repo_abc").expect("store should open");
        let bytes: &[u8] =
            b"AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n\x00\xff\xfe";

        let oid = store.write_blob(bytes).expect("blob should be written");

        assert_eq!(store.read_blob(&oid).expect("blob should read back"), bytes);
    }

    // Content addressing is what makes a 100-turn session affordable: the same
    // bytes stored twice are one object.
    #[test]
    fn identical_content_is_stored_once() {
        let data_dir = temp_data_dir("dedup");
        let store = ObjectStore::open(&data_dir, "repo_abc").unwrap();

        let first = store.write_blob(b"fn main() {}").unwrap();
        let second = store.write_blob(b"fn main() {}").unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn the_store_never_creates_a_repository_in_the_users_working_tree() {
        let data_dir = temp_data_dir("isolated");
        let repository = temp_data_dir("isolated-repo");
        std::fs::create_dir_all(&repository).unwrap();

        let store = ObjectStore::open(&data_dir, "repo_abc").unwrap();
        store.write_blob(b"content").unwrap();

        assert!(
            !repository.join(".git").exists(),
            "snapshots must not initialise or touch a repository in the working tree"
        );
        assert!(data_dir.join("checkpoints").join("repo_abc").exists());
    }

    // Retention deletes a checkpoint's ref and then collects what nothing else
    // references. If live checkpoints were not reachable from a ref, that
    // collection would take them too — which is the failure this test exists
    // to make impossible.
    #[test]
    fn garbage_collection_keeps_referenced_objects_and_drops_orphans() {
        let data_dir = temp_data_dir("gc");
        let store = ObjectStore::open(&data_dir, "repo_abc").unwrap();
        let kept = store.write_blob(b"still referenced").unwrap();
        let tree = store
            .write_tree(&[TreeEntry {
                path: "src/main.rs".to_string(),
                oid: kept.clone(),
                mode: FILE_MODE,
            }])
            .expect("tree should be written");
        let commit = store
            .commit_tree(&tree, "checkpoint_1")
            .expect("commit should be written");
        store.set_checkpoint_ref("checkpoint_1", &commit).unwrap();
        let orphan = store.write_blob(b"nothing points at me").unwrap();

        store.collect_garbage().expect("gc should run");

        assert_eq!(
            store
                .read_blob(&kept)
                .expect("referenced blob should survive"),
            b"still referenced"
        );
        assert!(
            store.read_blob(&orphan).is_err(),
            "an unreferenced blob should be collected"
        );
    }

    #[test]
    fn deleting_a_checkpoint_ref_makes_its_objects_collectable() {
        let data_dir = temp_data_dir("expire");
        let store = ObjectStore::open(&data_dir, "repo_abc").unwrap();
        let blob = store.write_blob(b"expired checkpoint").unwrap();
        let tree = store
            .write_tree(&[TreeEntry {
                path: "a.txt".to_string(),
                oid: blob.clone(),
                mode: FILE_MODE,
            }])
            .unwrap();
        let commit = store.commit_tree(&tree, "checkpoint_2").unwrap();
        store.set_checkpoint_ref("checkpoint_2", &commit).unwrap();

        store.delete_checkpoint_ref("checkpoint_2").unwrap();
        store.collect_garbage().unwrap();

        assert!(store.read_blob(&blob).is_err());
    }
}
