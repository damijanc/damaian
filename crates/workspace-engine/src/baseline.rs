//! Pre-apply check baselines (spec 102 Task 2).
//!
//! A patch is reviewed against base hashes. Once it is approved, the checks its
//! approved paths select are established against those same bytes *before* the
//! patch writes. This module owns that decision: which checks are new, which
//! already have a task-local baseline, which run under the ordinary command
//! policy, and which need the user's separate approval.
//!
//! It deliberately does not classify a post-patch result. It records
//! [`CheckBaseline`] facts; spec 102 Task 3 compares them with later results.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audit::AuditLog;
use crate::cancel::CancelToken;
use crate::command_policy::allow_always_eligible;
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::finding::{default_parsers, findings_from_execution};
use crate::index_cache::IndexCache;
use crate::indexer::ProjectIndexer;
use crate::patch_engine::ProposedPatch;
use crate::repository_map::RepositoryMapStore;
use crate::secret_scanner::SecretScanner;
use crate::session::SessionStore;
use crate::validation::{CommandProposal, ValidationOrchestrator};
use crate::verification::{
    CheckBaseline, CheckId, CheckTermination, FailureFingerprint, FindingEvidence, Selection,
    VerificationEvent, environment_termination, propose_checks,
};

/// One command the user must decide about before the patch can apply.
///
/// The fields are the engine's own labels and facts. The desktop and CLI render
/// them and transport the decision; neither derives a risk or a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineCheck {
    pub proposal_id: String,
    pub check: CheckId,
    /// The same label the verification report uses.
    pub label: String,
    pub command: String,
    /// Repository-relative, empty at the root.
    pub directory: String,
    /// The engine's own label for where the command runs, including the
    /// repository-root case. Renderers must not invent that wording.
    pub location_label: String,
    pub risk: String,
    /// The engine's display label for the risk, derived from the same
    /// classification that produced `risk`. Never re-derived by a renderer.
    pub risk_label: String,
    /// The engine's one-line question for this command.
    pub prompt: String,
    pub cause: String,
    pub blocked: bool,
    pub requires_approval: bool,
    /// Whether the ordinary command-approval path may offer *Allow Always*.
    pub allow_always: bool,
}

/// The engine-owned card for baselines that need separate approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineBatch {
    pub id: String,
    pub patch_id: String,
    pub task_id: String,
    pub title: String,
    pub note: String,
    pub run_label: String,
    pub skip_label: String,
    pub run_all_label: String,
    pub checks: Vec<BaselineCheck>,
}

/// What preparing a patch's baseline came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselinePreparation {
    /// Every newly selected check now has a terminal record. `recorded` names
    /// the checks this call recorded; empty means the patch reused current
    /// task-local baselines.
    Complete { recorded: Vec<CheckId> },
    /// At least one baseline needs the user's ordinary command approval. No
    /// patch file has been written.
    Awaiting(BaselineBatch),
}

/// The persisted half of a paused baseline approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingBaseline {
    batch: BaselineBatch,
    repository_root: String,
    session_id: String,
    task_id: String,
}

/// Selects and records pre-apply check baselines.
#[derive(Debug, Clone)]
pub struct BaselineCoordinator {
    config: Config,
    scanner: SecretScanner,
    audit_log: AuditLog,
    indexer: ProjectIndexer,
    session_store: SessionStore,
    validation: ValidationOrchestrator,
}

impl BaselineCoordinator {
    pub fn new(
        config: Config,
        scanner: SecretScanner,
        audit_log: AuditLog,
        indexer: ProjectIndexer,
        session_store: SessionStore,
        validation: ValidationOrchestrator,
    ) -> Self {
        Self {
            config,
            scanner,
            audit_log,
            indexer,
            session_store,
            validation,
        }
    }

    /// Establish every check the approved paths select that does not already
    /// have a current task-local baseline.
    ///
    /// Runs under the ordinary policy: a safe detected check runs as it would
    /// during verification, while one that needs approval is returned as a
    /// batch and nothing is written.
    pub fn prepare(
        &self,
        repository_root: &Path,
        patch: &ProposedPatch,
        approved_paths: Option<&[String]>,
    ) -> Result<BaselinePreparation> {
        let session_id = patch.session_id.as_str();
        let Some(task_id) = patch.task_id.as_deref() else {
            // A legacy patch has no session log to record a task-local baseline
            // in. Its apply behaviour is unchanged.
            return Ok(BaselinePreparation::Complete {
                recorded: Vec::new(),
            });
        };
        if session_id.is_empty() {
            // A task id alone is not a session. `create_patch` leaves the field
            // empty for callers that do not own a session, including legacy and
            // directly-created patches, and there is nowhere to write a
            // task-local baseline for them.
            return Ok(BaselinePreparation::Complete {
                recorded: Vec::new(),
            });
        }
        let selected_paths = self.selected_paths(patch, approved_paths)?;
        if selected_paths.is_empty() {
            return Ok(BaselinePreparation::Complete {
                recorded: Vec::new(),
            });
        }
        let index = IndexCache::get_or_build(&self.indexer, repository_root)?;
        let map_store = RepositoryMapStore::new(&self.config.data_dir, self.audit_log.clone());
        let (map, _) = map_store.load_or_build(&index, self.validation.command_policy())?;
        let selection = propose_checks(&self.validation, repository_root, &map, &selected_paths)?;
        let Selection::Proposed(checks) = selection else {
            return Ok(BaselinePreparation::Complete {
                recorded: Vec::new(),
            });
        };
        // A baseline belongs to the patch's durable task. Look through its
        // continuation chain so a repair or worker continuation reuses the
        // source task's baseline instead of taking a post-failure one.
        let current = self
            .session_store
            .task_verification_baselines(session_id, task_id)?;
        let mut recorded = Vec::new();
        let mut awaiting = Vec::new();
        for (check, proposal) in checks {
            if current.contains_key(&check) {
                continue;
            }
            if proposal.blocked {
                self.record(
                    session_id,
                    task_id,
                    &check,
                    unavailable(&check, CheckTermination::PolicyRefused),
                )?;
                recorded.push(check);
                continue;
            }
            if proposal.requires_approval {
                awaiting.push(self.batch_check(&proposal, &check));
                continue;
            }
            let baseline =
                self.run_baseline(session_id, task_id, &check, &proposal, false, "policy")?;
            self.record(session_id, task_id, &check, baseline)?;
            recorded.push(check);
        }
        if awaiting.is_empty() {
            // Every check now has a terminal baseline, so a batch left by an
            // earlier reload or a competing apply would only re-offer work the
            // engine has already settled.
            self.remove_pending_for_patch(&patch.id)?;
            return Ok(BaselinePreparation::Complete { recorded });
        }
        let batch = BaselineBatch {
            // Deterministic in the patch and the checks it still needs, so a
            // reload or a repeated apply re-offers the same batch instead of
            // orphaning a duplicate file with a fresh proposal id set.
            id: batch_id_for(&patch.id, &awaiting),
            patch_id: patch.id.clone(),
            task_id: task_id.to_string(),
            title: "Checks need a baseline".to_string(),
            note: "This patch has not been applied. Run these checks against the current, \
                   pre-patch files, or skip them. Skipping a baseline does not skip the patch."
                .to_string(),
            run_label: "Run selected baselines".to_string(),
            skip_label: "Skip baselines".to_string(),
            run_all_label: "Run all".to_string(),
            checks: awaiting,
        };
        // A different selection for the same patch must not leave the previous
        // card resumable beside this one. The new card is written after the
        // old one is removed, so the store never holds two pending decisions
        // for the same patch.
        self.remove_pending_for_patch(&patch.id)?;
        self.save_pending(PendingBaseline {
            batch: batch.clone(),
            repository_root: canonical_root(repository_root)
                .to_string_lossy()
                .to_string(),
            session_id: session_id.to_string(),
            task_id: task_id.to_string(),
        })?;
        self.audit_log.record(
            "verification_baseline_paused",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("taskId", task_id.to_string()),
                ("patchId", patch.id.clone()),
                ("proposalId", batch.id.clone()),
                ("checks", batch.checks.len().to_string()),
            ],
        )?;
        Ok(BaselinePreparation::Awaiting(batch))
    }

    /// Apply one card's decisions and record a terminal result for each check.
    ///
    /// The whole decision set is validated against the pending batch before a
    /// single command runs or a single baseline is recorded. A malformed retry
    /// therefore has no side effects and can be retried after the client
    /// refreshes its card.
    pub fn resume(
        &self,
        repository_root: &Path,
        batch_id: &str,
        decisions: &[(String, bool)],
        always: &[String],
        approved_by: &str,
    ) -> Result<BaselinePreparation> {
        let pending = self.load_pending(batch_id)?;
        self.validate_resume_against(&pending, repository_root, decisions, always)?;
        // Every *Allow Always* request was validated against this exact pending
        // batch above, so no grant can be written for an unknown, unapproved or
        // ineligible proposal. The grant is written before the first baseline
        // command runs, exactly as the ordinary command card does.
        for proposal_id in always {
            self.validation
                .allow_command_always(proposal_id, approved_by)?;
        }
        let mut recorded = Vec::new();
        for check in &pending.batch.checks {
            // Validated above, so every check has exactly one decision.
            let Some((_, approved)) = decisions
                .iter()
                .find(|(proposal_id, _)| proposal_id == &check.proposal_id)
            else {
                unreachable!("resume validated every decision before running")
            };
            if *approved {
                let proposal = self
                    .validation
                    .command_store()
                    .load_proposal(&check.proposal_id)?;
                let baseline = self.run_baseline(
                    &pending.session_id,
                    &pending.task_id,
                    &check.check,
                    &proposal,
                    true,
                    approved_by,
                )?;
                self.record(
                    &pending.session_id,
                    &pending.task_id,
                    &check.check,
                    baseline,
                )?;
            } else {
                // Through the store as well as the log, so declining a
                // baseline leaves the same command-store record an ordinary
                // declined check does.
                let _ = self
                    .validation
                    .reject_proposal(&check.proposal_id, approved_by);
                self.record(
                    &pending.session_id,
                    &pending.task_id,
                    &check.check,
                    CheckBaseline::Declined,
                )?;
            }
            recorded.push(check.check.clone());
        }
        self.remove_pending(batch_id)?;
        self.audit_log.record(
            "verification_baseline_resumed",
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", pending.session_id.clone()),
                ("taskId", pending.task_id.clone()),
                ("patchId", pending.batch.patch_id.clone()),
                ("proposalId", batch_id.to_string()),
                ("checks", recorded.len().to_string()),
            ],
        )?;
        Ok(BaselinePreparation::Complete { recorded })
    }

    /// Read-only validation of a retry before the engine writes an *Allow
    /// Always* grant or runs any baseline command. `resume` repeats this
    /// validation, so a transport that skips this call still cannot perform a
    /// partial grant.
    pub fn validate_resume(
        &self,
        repository_root: &Path,
        batch_id: &str,
        decisions: &[(String, bool)],
        always: &[String],
    ) -> Result<()> {
        let pending = self.load_pending(batch_id)?;
        self.validate_resume_against(&pending, repository_root, decisions, always)
    }

    fn validate_resume_against(
        &self,
        pending: &PendingBaseline,
        repository_root: &Path,
        decisions: &[(String, bool)],
        always: &[String],
    ) -> Result<()> {
        if canonical_root(Path::new(&pending.repository_root)) != canonical_root(repository_root) {
            return Err(ClientError::InvalidInput(
                "This baseline batch belongs to a different repository".to_string(),
            ));
        }
        let mut given = BTreeMap::new();
        for (proposal_id, approved) in decisions {
            if !pending
                .batch
                .checks
                .iter()
                .any(|check| &check.proposal_id == proposal_id)
            {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown baseline proposal {proposal_id}"
                )));
            }
            if given.insert(proposal_id.as_str(), *approved).is_some() {
                return Err(ClientError::InvalidInput(format!(
                    "Duplicate decision for baseline proposal {proposal_id}"
                )));
            }
        }
        for check in &pending.batch.checks {
            if !given.contains_key(check.proposal_id.as_str()) {
                return Err(ClientError::InvalidInput(format!(
                    "No decision was given for baseline check {}",
                    check.proposal_id
                )));
            }
        }
        // *Allow Always* is a separate grant. It is only meaningful for a check
        // on this exact card that the user approved and that the engine marked
        // eligible when it built the card. Validate the whole set before the
        // caller writes any grant or runs any command, so a malformed retry has
        // no side effects and can be corrected.
        let mut grants = BTreeSet::new();
        for proposal_id in always {
            if !grants.insert(proposal_id.as_str()) {
                return Err(ClientError::InvalidInput(format!(
                    "Duplicate always decision for baseline proposal {proposal_id}"
                )));
            }
            let Some(check) = pending
                .batch
                .checks
                .iter()
                .find(|check| &check.proposal_id == proposal_id)
            else {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown baseline proposal {proposal_id}"
                )));
            };
            if given.get(proposal_id.as_str()) != Some(&true) {
                return Err(ClientError::InvalidInput(format!(
                    "Baseline proposal {proposal_id} cannot be allowed always without approval"
                )));
            }
            if !check.allow_always {
                return Err(ClientError::InvalidInput(format!(
                    "Baseline proposal {proposal_id} cannot be permanently allowed"
                )));
            }
            // Recompute eligibility from the stored proposal so a config change
            // between rendering the card and answering it cannot leave a grant
            // the current policy would refuse.
            let proposal = self
                .validation
                .command_store()
                .load_proposal(proposal_id)
                .map_err(|_| {
                    ClientError::InvalidInput(format!(
                        "Baseline proposal {proposal_id} is no longer available"
                    ))
                })?;
            if !allow_always_eligible(&self.config, &proposal.command, proposal.blocked) {
                return Err(ClientError::InvalidInput(format!(
                    "Baseline proposal {proposal_id} cannot be permanently allowed"
                )));
            }
        }
        Ok(())
    }

    /// The pending batch, for a caller that recovered a patch card and needs
    /// the same engine-owned labels.
    pub fn pending(&self, batch_id: &str) -> Result<Option<BaselineBatch>> {
        let path = self.pending_path(batch_id)?;
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(self.load_pending(batch_id)?.batch))
    }

    fn selected_paths(
        &self,
        patch: &ProposedPatch,
        approved_paths: Option<&[String]>,
    ) -> Result<Vec<String>> {
        let selected = approved_paths
            .map(|paths| paths.to_vec())
            .unwrap_or_else(|| patch.files.iter().map(|file| file.path.clone()).collect());
        if let Some(path) = selected
            .iter()
            .find(|path| !patch.files.iter().any(|file| &file.path == *path))
        {
            return Err(ClientError::InvalidInput(format!(
                "Selected patch file was not found: {path}"
            )));
        }
        Ok(selected)
    }

    fn batch_check(&self, proposal: &CommandProposal, check: &CheckId) -> BaselineCheck {
        let location_label = if check.working_directory.is_empty() {
            "repository root".to_string()
        } else {
            check.working_directory.clone()
        };
        BaselineCheck {
            proposal_id: proposal.id.clone(),
            check: check.clone(),
            label: check.label(),
            command: check.command.clone(),
            directory: check.working_directory.clone(),
            location_label,
            risk: proposal.risk.as_str().to_string(),
            risk_label: proposal.risk.as_str().to_string(),
            prompt: "Run this check against the current, pre-patch files?".to_string(),
            cause: proposal
                .causes
                .first()
                .map(|cause| cause.plain_words())
                .or_else(|| proposal.reasons.first().cloned())
                .unwrap_or_default(),
            blocked: proposal.blocked,
            requires_approval: proposal.requires_approval,
            allow_always: allow_always_eligible(&self.config, &proposal.command, proposal.blocked),
        }
    }

    fn run_baseline(
        &self,
        session_id: &str,
        task_id: &str,
        check: &CheckId,
        proposal: &CommandProposal,
        approved: bool,
        approved_by: &str,
    ) -> Result<CheckBaseline> {
        let cancel = CancelToken::new();
        let mut on_output = |_line: &str| {};
        let record = match self.validation.run_proposal(
            &proposal.id,
            approved,
            approved_by,
            Some(task_id),
            &cancel,
            &mut on_output,
        ) {
            Ok(record) => record,
            Err(error) => {
                // Only a policy refusal or a real process-spawn failure is an
                // unavailable baseline. An execution-store, audit or other
                // internal error is not evidence about the environment, so it
                // propagates rather than being recorded as one.
                let Some(termination) = environment_termination(&error) else {
                    return Err(error);
                };
                self.audit_log.record(
                    "verification_baseline_unavailable",
                    &[
                        ("actor", "system".to_string()),
                        ("sessionId", session_id.to_string()),
                        ("taskId", task_id.to_string()),
                        ("check", check.label()),
                        ("termination", termination.as_str().to_string()),
                    ],
                )?;
                return Ok(unavailable(check, termination));
            }
        };
        let execution = record.execution;
        if execution.termination == crate::command_runner::CommandTermination::Exited
            && execution.exit_code == Some(0)
        {
            self.audit_log.record(
                "verification_baseline_recorded",
                &[
                    ("actor", "system".to_string()),
                    ("sessionId", session_id.to_string()),
                    ("taskId", task_id.to_string()),
                    ("check", check.label()),
                    ("result", "passed".to_string()),
                ],
            )?;
            return Ok(CheckBaseline::Passed);
        }
        let termination =
            CheckTermination::from_command(execution.termination, execution.exit_code);
        if matches!(&termination, CheckTermination::MissingExecutable) {
            return Ok(unavailable(check, termination));
        }
        let findings = findings_from_execution(&execution, &default_parsers(), &self.scanner)
            .iter()
            .map(FindingEvidence::from_finding)
            .collect::<Vec<_>>();
        let fingerprint = FailureFingerprint::compute(check, &findings, &termination);
        self.audit_log.record(
            "verification_baseline_recorded",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("taskId", task_id.to_string()),
                ("check", check.label()),
                ("result", "failed".to_string()),
                ("termination", termination.as_str().to_string()),
                ("fingerprint", fingerprint.as_str().to_string()),
            ],
        )?;
        Ok(CheckBaseline::Failed {
            termination,
            fingerprint,
            findings,
        })
    }

    fn record(
        &self,
        session_id: &str,
        task_id: &str,
        check: &CheckId,
        baseline: CheckBaseline,
    ) -> Result<()> {
        self.session_store.append_verification_event(
            session_id,
            &VerificationEvent::Baseline {
                task_id: task_id.to_string(),
                check: check.clone(),
                baseline,
            },
        )
    }

    fn save_pending(&self, pending: PendingBaseline) -> Result<()> {
        let path = self.pending_path(&pending.batch.id)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(&pending).map_err(|error| {
            ClientError::Io(format!("Failed to serialise a baseline batch: {error}"))
        })?;
        fs::write(path, json)?;
        Ok(())
    }

    fn load_pending(&self, batch_id: &str) -> Result<PendingBaseline> {
        let path = self.pending_path(batch_id)?;
        let content = fs::read_to_string(&path).map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                ClientError::InvalidInput(format!("No pending baseline batch {batch_id}"))
            } else {
                error.into()
            }
        })?;
        serde_json::from_str(&content).map_err(|error| {
            ClientError::InvalidInput(format!("Invalid baseline batch {batch_id}: {error}"))
        })
    }

    fn remove_pending(&self, batch_id: &str) -> Result<()> {
        match fs::remove_file(self.pending_path(batch_id)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Drops every pending card for `patch_id`. Called when the engine settles
    /// the patch's baselines and before it writes a replacement card, so a
    /// reload never finds two competing decisions for one apply.
    fn remove_pending_for_patch(&self, patch_id: &str) -> Result<()> {
        let directory = self.pending_dir();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(content) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(pending) = serde_json::from_str::<PendingBaseline>(&content) else {
                continue;
            };
            if pending.batch.patch_id == patch_id {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    fn pending_dir(&self) -> PathBuf {
        self.config.data_dir.join("baselines").join("pending")
    }

    /// A batch id reaches the filesystem, so it is not trusted as a path
    /// component. The engine only ever generates `baseline_` ids made of
    /// alphanumerics, `_` and `-`; anything else is refused before a path is
    /// built, which makes `..` and separators impossible.
    fn pending_path(&self, batch_id: &str) -> Result<PathBuf> {
        validate_batch_id(batch_id)?;
        Ok(self.pending_dir().join(format!("{batch_id}.json")))
    }
}

const BATCH_ID_PREFIX: &str = "baseline_";
const MAX_BATCH_ID_CHARS: usize = 96;

fn validate_batch_id(batch_id: &str) -> Result<()> {
    let valid = batch_id.starts_with(BATCH_ID_PREFIX)
        && batch_id.len() <= MAX_BATCH_ID_CHARS
        && batch_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        });
    if valid {
        Ok(())
    } else {
        Err(ClientError::InvalidInput(format!(
            "Invalid baseline batch id {batch_id}"
        )))
    }
}

/// Stable for one patch and one set of still-undecided checks, so reload and
/// repeated apply requests re-offer the same card instead of leaving orphaned
/// pending batches behind.
fn batch_id_for(patch_id: &str, checks: &[BaselineCheck]) -> String {
    let mut selected = checks
        .iter()
        .map(|check| {
            (
                check.check.working_directory.as_str(),
                check.check.command.as_str(),
            )
        })
        .collect::<Vec<_>>();
    selected.sort_unstable();
    let identity = serde_json::json!({
        "patchId": patch_id,
        "checks": selected,
    });
    let digest = crate::hash::sha256_hex(
        &serde_json::to_vec(&identity).expect("baseline batch identity is serializable"),
    );
    format!("{BATCH_ID_PREFIX}{}", &digest[..32])
}

fn canonical_root(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn unavailable(check: &CheckId, termination: CheckTermination) -> CheckBaseline {
    CheckBaseline::Unavailable {
        fingerprint: FailureFingerprint::compute(check, &[], &termination),
        termination,
    }
}

/// The task-local baselines a patch task currently has, keyed by check.
///
/// Exposed as a value rather than a second lookup rule so spec 102 Task 3
/// classifies against exactly the state Task 2 recorded.
pub type TaskBaselines = BTreeMap<CheckId, CheckBaseline>;
