//! Failure-time check baselines (spec 102, made lazy by spec 109 D1).
//!
//! A baseline answers whether a failing check already failed before the task
//! changed anything. Spec 102 took every baseline before a task's first patch;
//! spec 109 takes one only when a per-change or completion check fails and the
//! task chain has none for that check yet. A passing check never needs one.
//!
//! The baseline runs the same command, through the same command policy, in a
//! temporary task-start tree ([`crate::pre_task_tree`]), never in the user's
//! checkout. A command the policy lets run does so at once; one that needs
//! approval goes on one card that says why. A tree that cannot be rebuilt
//! exactly is an unavailable baseline.
//!
//! A Rust test failure (`cargo test`, `cargo nextest run`) whose findings
//! name the failing tests reruns only those tests (spec 109 D3): the same
//! command with `-- --exact <names>`, which narrows what runs and nothing
//! else. The baseline records the names it covered, so a later failure of
//! another test takes a new baseline rather than reading this one. Any other
//! failure reruns the whole check.
//!
//! It deliberately does not classify a result. It records [`CheckBaseline`]
//! facts; spec 102 Task 3's `provenance::classify` compares them with the
//! failed result, with the meaning it always had.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audit::AuditLog;
use crate::cancel::CancelToken;
use crate::checkpoint::CheckpointStore;
use crate::command_policy::allow_always_eligible;
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::finding::{
    FindingSource, Severity, default_parsers, failed_test_name, findings_from_execution,
    is_nextest_command, is_rust_test_command,
};
use crate::pre_task_tree::{PreTaskSources, TreeBuild, build_pre_task_tree};
use crate::secret_scanner::SecretScanner;
use crate::session::SessionStore;
use crate::validation::{CommandProposal, ValidationOrchestrator};
use crate::verification::{
    CheckBaseline, CheckId, CheckTermination, FailureFingerprint, FindingEvidence,
    VerificationEvent, baseline_result_word, environment_termination,
};

/// Why a failure-time baseline is proposed. Stored with the proposal, so the
/// command store and the audit say what the run was for.
const FAILURE_BASELINE_REASON: &str =
    "Baseline for a failed check: run on the files as they were before this task's changes";

/// The one-line question a failure-time baseline asks (spec 109 Task 3).
pub const FAILURE_BASELINE_PROMPT: &str = "This check failed; run it on the files as they were \
     before this task's changes to tell whether the failure is new?";

/// One command the user must decide about before the failure can be
/// classified.
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
    /// The failing tests `command` reruns by name (spec 109 D3); empty when
    /// it reruns the whole check. Defaulted so a card persisted before the
    /// field existed still loads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<String>,
}

/// A check whose latest run failed with a code result, with that run's
/// findings: what a failure-time baseline is taken for (spec 109 D1). The
/// findings decide whether only the failing tests rerun (D3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedCheck {
    pub check: CheckId,
    pub findings: Vec<FindingEvidence>,
}

/// One baseline to run: the check it is for, its stored proposal, and the
/// failing tests that proposal reruns by name (empty: the whole check).
#[derive(Debug, Clone)]
struct BaselineJob {
    check: CheckId,
    proposal_id: String,
    tests: Vec<String>,
}

/// The engine-owned card for baselines that need separate approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineBatch {
    pub id: String,
    /// What the card belongs to: the verification task whose failed checks it
    /// baselines. Spec 102 cards named a patch here, under `patchId`.
    #[serde(alias = "patchId")]
    pub source_id: String,
    pub task_id: String,
    pub title: String,
    pub note: String,
    pub run_label: String,
    pub skip_label: String,
    pub run_all_label: String,
    /// The control that stops a running baseline (spec 107 Task 4). Defaulted
    /// so a card persisted before the field existed still loads.
    #[serde(default = "default_cancel_label")]
    pub cancel_label: String,
    pub checks: Vec<BaselineCheck>,
}

fn default_cancel_label() -> String {
    "Cancel".to_string()
}

/// What preparing the baselines of failed checks came to.
// One value per call, returned and matched at once, never held in a
// collection, so the size gap between the variants costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselinePreparation {
    /// Every failed check now has a terminal record. `recorded` names the
    /// checks this call recorded; empty means the task chain already had them.
    Complete { recorded: Vec<CheckId> },
    /// At least one baseline needs the user's ordinary command approval.
    /// Nothing on the card has run.
    Awaiting(BaselineBatch),
}

/// What a baseline run reports while it runs (spec 107 Task 4).
///
/// Every label is the engine's own text; a renderer shows it and never derives
/// a check name or a result word of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BaselineProgress {
    /// A check started running. `index` is 1-based among the checks that run.
    #[serde(rename_all = "camelCase")]
    Started {
        check: String,
        label: String,
        index: usize,
        total: usize,
    },
    /// The running check's latest output, already secret-redacted by the
    /// command runner; one chunk as the runner read it, without its trailing
    /// newline.
    Output { line: String },
    /// A check reached its terminal record. `result` is the recorded baseline
    /// (`passed`, `failed`, `unavailable`, `declined`), or `cancelled` for the
    /// check that was running when the user cancelled (recorded as declined).
    #[serde(rename_all = "camelCase")]
    Finished {
        check: String,
        result: String,
        label: String,
    },
}

/// What a streamed resume came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineResumeOutcome {
    /// Every check on the card, each now with a terminal record.
    pub recorded: Vec<CheckId>,
    /// The user cancelled. The check that was running and every approved check
    /// that had not started are recorded as declined (spec 102's existing
    /// provenance).
    pub cancelled: bool,
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

/// Selects, runs and records failure-time check baselines.
#[derive(Debug, Clone)]
pub struct BaselineCoordinator {
    config: Config,
    scanner: SecretScanner,
    audit_log: AuditLog,
    session_store: SessionStore,
    validation: ValidationOrchestrator,
    checkpoints: CheckpointStore,
}

impl BaselineCoordinator {
    pub fn new(
        config: Config,
        scanner: SecretScanner,
        audit_log: AuditLog,
        session_store: SessionStore,
        validation: ValidationOrchestrator,
        checkpoints: CheckpointStore,
    ) -> Self {
        Self {
            config,
            scanner,
            audit_log,
            session_store,
            validation,
            checkpoints,
        }
    }

    /// Establish a baseline for every failed check the task chain has none
    /// for (spec 109 D1).
    ///
    /// Runs under the ordinary policy: a check the policy lets run is run now
    /// in the task-start tree, a blocked one is unavailable, and one that needs
    /// approval is returned on one card; nothing on the card has run. A cancel
    /// records the running check and every one not yet run as declined.
    pub fn prepare_for_failures(
        &self,
        repository_root: &Path,
        session_id: &str,
        task_id: &str,
        failed: &[FailedCheck],
        cancel: &CancelToken,
        on_progress: &mut dyn FnMut(BaselineProgress),
    ) -> Result<BaselinePreparation> {
        if session_id.is_empty() || task_id.is_empty() {
            return Ok(BaselinePreparation::Complete {
                recorded: Vec::new(),
            });
        }
        // A baseline belongs to the task chain: a repair or a continuation
        // reuses the one its source task took.
        let current = self
            .session_store
            .task_verification_baseline_scopes(session_id, task_id)?;
        let mut recorded = Vec::new();
        let mut awaiting = Vec::new();
        let mut runnable = Vec::new();
        let mut seen = BTreeSet::new();
        for failed in failed {
            let check = &failed.check;
            if !seen.insert(check.clone()) {
                continue;
            }
            let wanted = failing_tests(check, &failed.findings);
            let earlier = current.get(check);
            if earlier.is_some_and(|(baseline, covered)| {
                baseline_covers(baseline, covered, wanted.as_deref())
            }) {
                continue;
            }
            // A targeted baseline that missed a test now failing is
            // replaced by one covering both sets, so the earlier failure
            // stays covered too.
            let tests = match (wanted, earlier) {
                (Some(mut names), Some((_, covered))) => {
                    names.extend(covered.iter().cloned());
                    names.sort();
                    names.dedup();
                    names
                }
                (Some(names), None) => names,
                (None, _) => Vec::new(),
            };
            let directory = if check.working_directory.is_empty() {
                repository_root.to_path_buf()
            } else {
                repository_root.join(&check.working_directory)
            };
            let (command, tests) = self.baseline_command(repository_root, &directory, check, tests);
            let proposal = self.validation.propose_command_at(
                repository_root,
                &directory,
                &command,
                FAILURE_BASELINE_REASON,
            )?;
            if proposal.blocked {
                self.record(
                    session_id,
                    task_id,
                    check,
                    unavailable(check, CheckTermination::PolicyRefused),
                    tests,
                )?;
                recorded.push(check.clone());
            } else if proposal.requires_approval {
                awaiting.push(self.batch_check(&proposal, check, tests));
            } else {
                runnable.push(BaselineJob {
                    check: check.clone(),
                    proposal_id: proposal.id,
                    tests,
                });
            }
        }
        if !runnable.is_empty() {
            let outcome = self.run_in_task_start_tree(
                repository_root,
                session_id,
                task_id,
                &runnable,
                false,
                "policy",
                cancel,
                on_progress,
            )?;
            recorded.extend(outcome.recorded);
        }
        if awaiting.is_empty() {
            // Every check now has a terminal baseline, so a card left by an
            // earlier pause would only re-offer settled work.
            self.remove_pending_for_source(task_id)?;
            return Ok(BaselinePreparation::Complete { recorded });
        }
        let batch = BaselineBatch {
            // Deterministic in the source and the checks it still needs, so a
            // repeated request re-offers the same card instead of orphaning a
            // duplicate file with a fresh proposal id set.
            id: batch_id_for(task_id, &awaiting),
            source_id: task_id.to_string(),
            task_id: task_id.to_string(),
            title: if awaiting.len() == 1 {
                "A check failed: is the failure new?".to_string()
            } else {
                "Checks failed: are the failures new?".to_string()
            },
            note: "This check failed. Run it on the files as they were before this task's \
                   changes to tell whether the failure is new. Skipping leaves the cause \
                   unknown; it changes no file."
                .to_string(),
            run_label: "Run selected baselines".to_string(),
            skip_label: "Skip baselines".to_string(),
            run_all_label: "Run all".to_string(),
            cancel_label: default_cancel_label(),
            checks: awaiting,
        };
        self.remove_pending_for_source(task_id)?;
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
                ("proposalId", batch.id.clone()),
                ("checks", batch.checks.len().to_string()),
            ],
        )?;
        Ok(BaselinePreparation::Awaiting(batch))
    }

    /// Apply one card's decisions and record a terminal result for each check
    /// (spec 107 Task 4's streamed run, with its cancel).
    ///
    /// The whole decision set is validated against the pending batch before a
    /// single command runs or a single baseline is recorded, so a malformed
    /// retry has no side effects. A cancel stops the running check (the command
    /// runner kills its process group) and records it, and every approved
    /// check that had not started, as declined — exactly the record *Skip
    /// baselines* leaves. The pending card is removed either way.
    #[allow(clippy::too_many_arguments)]
    pub fn resume_streamed(
        &self,
        repository_root: &Path,
        batch_id: &str,
        decisions: &[(String, bool)],
        always: &[String],
        approved_by: &str,
        cancel: &CancelToken,
        on_progress: &mut dyn FnMut(BaselineProgress),
    ) -> Result<BaselineResumeOutcome> {
        let pending = self.load_pending(batch_id)?;
        self.validate_resume_against(&pending, repository_root, decisions, always)?;
        // Every *Allow Always* request was validated against this exact pending
        // batch above, so no grant can be written for an unknown, unapproved or
        // ineligible proposal.
        for proposal_id in always {
            self.validation
                .allow_command_always(proposal_id, approved_by)?;
        }
        let approved_of = |check: &BaselineCheck| {
            decisions
                .iter()
                .find(|(proposal_id, _)| proposal_id == &check.proposal_id)
                .is_some_and(|(_, approved)| *approved)
        };
        let mut recorded = Vec::new();
        let mut approved = Vec::new();
        for check in &pending.batch.checks {
            if approved_of(check) {
                approved.push(BaselineJob {
                    check: check.check.clone(),
                    proposal_id: check.proposal_id.clone(),
                    tests: check.tests.clone(),
                });
                continue;
            }
            // Through the store as well as the log, so declining a baseline
            // leaves the same command-store record an ordinary declined check
            // does.
            let _ = self
                .validation
                .reject_proposal(&check.proposal_id, approved_by);
            self.record(
                &pending.session_id,
                &pending.task_id,
                &check.check,
                CheckBaseline::Declined,
                check.tests.clone(),
            )?;
            recorded.push(check.check.clone());
        }
        let outcome = self.run_in_task_start_tree(
            repository_root,
            &pending.session_id,
            &pending.task_id,
            &approved,
            true,
            approved_by,
            cancel,
            on_progress,
        )?;
        recorded.extend(outcome.recorded);
        self.remove_pending(batch_id)?;
        self.audit_log.record(
            if outcome.cancelled {
                "verification_baseline_cancelled"
            } else {
                "verification_baseline_resumed"
            },
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", pending.session_id.clone()),
                ("taskId", pending.task_id.clone()),
                ("proposalId", batch_id.to_string()),
                ("checks", recorded.len().to_string()),
            ],
        )?;
        Ok(BaselineResumeOutcome {
            recorded,
            cancelled: outcome.cancelled,
        })
    }

    /// Read-only validation of a retry before the engine writes an *Allow
    /// Always* grant or runs any baseline command. `resume_streamed` repeats
    /// this validation, so a transport that skips this call still cannot
    /// perform a partial grant.
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
        // eligible when it built the card.
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

    /// Run `checks` (each with its stored proposal) in one task-start tree and
    /// record each one's baseline. The tree is built once and removed when
    /// this returns, whatever happened.
    #[allow(clippy::too_many_arguments)]
    fn run_in_task_start_tree(
        &self,
        repository_root: &Path,
        session_id: &str,
        task_id: &str,
        checks: &[BaselineJob],
        approved: bool,
        approved_by: &str,
        cancel: &CancelToken,
        on_progress: &mut dyn FnMut(BaselineProgress),
    ) -> Result<BaselineResumeOutcome> {
        let mut recorded = Vec::new();
        if checks.is_empty() {
            return Ok(BaselineResumeOutcome {
                recorded,
                cancelled: false,
            });
        }
        let build = if cancel.is_cancelled() {
            None
        } else {
            Some(build_pre_task_tree(
                repository_root,
                &PreTaskSources {
                    data_dir: &self.config.data_dir,
                    session_store: &self.session_store,
                    checkpoints: &self.checkpoints,
                },
                session_id,
                task_id,
            )?)
        };
        let tree = match build {
            Some(TreeBuild::Ready(tree)) => Some(tree),
            Some(TreeBuild::Unavailable(reason)) => {
                // D2: a task-start tree that cannot be rebuilt exactly is no
                // baseline at all, never a guess. Unknown provenance follows.
                for job in checks {
                    let check = &job.check;
                    self.audit_log.record(
                        "verification_baseline_unavailable",
                        &[
                            ("actor", "system".to_string()),
                            ("sessionId", session_id.to_string()),
                            ("taskId", task_id.to_string()),
                            ("check", check.label()),
                            ("termination", "task_start_tree_unavailable".to_string()),
                            ("reason", reason.clone()),
                        ],
                    )?;
                    self.record(
                        session_id,
                        task_id,
                        check,
                        unavailable(check, CheckTermination::UnavailableInfrastructure),
                        job.tests.clone(),
                    )?;
                    let name = check.label();
                    on_progress(BaselineProgress::Finished {
                        label: finished_label("unavailable", &name),
                        check: name,
                        result: "unavailable".to_string(),
                    });
                    recorded.push(check.clone());
                }
                return Ok(BaselineResumeOutcome {
                    recorded,
                    cancelled: false,
                });
            }
            None => None,
        };
        let env = vec![("CARGO_TARGET_DIR", self.cargo_target_dir(repository_root))];
        let total = checks.len();
        let mut cancelled = false;
        for (index, job) in checks.iter().enumerate() {
            let (check, proposal_id) = (&job.check, &job.proposal_id);
            let name = check.label();
            let result = match (&tree, cancel.is_cancelled()) {
                (Some(tree), false) => {
                    on_progress(BaselineProgress::Started {
                        check: name.clone(),
                        label: running_label(&name, &job.tests),
                        index: index + 1,
                        total,
                    });
                    let mut on_output = |line: &str| {
                        let line = line.trim_end_matches(['\n', '\r']);
                        if !line.is_empty() {
                            on_progress(BaselineProgress::Output {
                                line: line.to_string(),
                            });
                        }
                    };
                    self.run_baseline(
                        session_id,
                        task_id,
                        check,
                        proposal_id,
                        tree.root(),
                        &env,
                        approved,
                        approved_by,
                        cancel,
                        &mut on_output,
                    )?
                }
                // Cancelled before this one started: declined, the same
                // record as one left unticked.
                _ => BaselineRun::NotStarted,
            };
            let (baseline, word) = match result {
                BaselineRun::Recorded(baseline) => {
                    let word = baseline_result_word(&baseline);
                    (baseline, word)
                }
                BaselineRun::Cancelled => {
                    cancelled = true;
                    (CheckBaseline::Declined, "cancelled")
                }
                BaselineRun::NotStarted => {
                    cancelled = true;
                    let _ = self.validation.reject_proposal(proposal_id, approved_by);
                    (CheckBaseline::Declined, "declined")
                }
            };
            self.record(session_id, task_id, check, baseline, job.tests.clone())?;
            on_progress(BaselineProgress::Finished {
                label: finished_label(word, &name),
                check: name,
                result: word.to_string(),
            });
            recorded.push(check.clone());
        }
        Ok(BaselineResumeOutcome {
            recorded,
            cancelled,
        })
    }

    /// A Damaian-owned build cache for baselines of this repository, so a
    /// Rust baseline in a fresh tree reuses its compiled dependencies instead
    /// of building everything again, without writing into the user's own
    /// `target`. Ignored by every other tool.
    ///
    /// The directory name is hex only: cargo joins the target directory into
    /// `DYLD_FALLBACK_LIBRARY_PATH`, where a `:` (as in a repository id's
    /// `sha256:` prefix) is a separator and fails the build.
    fn cargo_target_dir(&self, repository_root: &Path) -> String {
        let root = canonical_root(repository_root);
        let digest = crate::hash::sha256_hex(root.to_string_lossy().as_bytes());
        self.config
            .data_dir
            .join("baselines")
            .join("cargo-target")
            .join(&digest[..16])
            .to_string_lossy()
            .to_string()
    }

    /// The command a failure-time baseline runs, and the tests it reruns by
    /// name (spec 109 D3). With `tests`, the check's own command narrowed to
    /// them, as long as the policy treats that no worse than the whole check:
    /// when the narrowed command would need a card or is blocked but the
    /// whole check runs without one (the user allowed it always, say), the
    /// whole check runs instead, so narrowing never adds a card.
    fn baseline_command(
        &self,
        repository_root: &Path,
        directory: &Path,
        check: &CheckId,
        tests: Vec<String>,
    ) -> (String, Vec<String>) {
        if tests.is_empty() {
            return (check.command.clone(), tests);
        }
        let targeted = targeted_command(&check.command, &tests);
        let narrowed = self
            .validation
            .classify_command_at(repository_root, directory, &targeted);
        if !narrowed.blocked && !narrowed.requires_approval {
            return (targeted, tests);
        }
        let whole = self
            .validation
            .classify_command_at(repository_root, directory, &check.command);
        if narrowed.blocked || !(whole.blocked || whole.requires_approval) {
            return (check.command.clone(), Vec::new());
        }
        (targeted, tests)
    }

    fn batch_check(
        &self,
        proposal: &CommandProposal,
        check: &CheckId,
        tests: Vec<String>,
    ) -> BaselineCheck {
        let location_label = if check.working_directory.is_empty() {
            "repository root".to_string()
        } else {
            check.working_directory.clone()
        };
        BaselineCheck {
            proposal_id: proposal.id.clone(),
            check: check.clone(),
            label: check.label(),
            // What runs, which is narrower than the check when it reruns
            // only the failing tests.
            command: proposal.command.clone(),
            directory: check.working_directory.clone(),
            location_label,
            risk: proposal.risk.as_str().to_string(),
            risk_label: proposal.risk.as_str().to_string(),
            prompt: FAILURE_BASELINE_PROMPT.to_string(),
            cause: proposal
                .causes
                .first()
                .map(|cause| cause.plain_words())
                .or_else(|| proposal.reasons.first().cloned())
                .unwrap_or_default(),
            blocked: proposal.blocked,
            requires_approval: proposal.requires_approval,
            allow_always: allow_always_eligible(&self.config, &proposal.command, proposal.blocked),
            tests,
        }
    }

    /// Run one baseline in the task-start tree under the caller's cancel
    /// token, streaming its (redacted) output to `on_output`.
    #[allow(clippy::too_many_arguments)]
    fn run_baseline(
        &self,
        session_id: &str,
        task_id: &str,
        check: &CheckId,
        proposal_id: &str,
        tree_root: &Path,
        env: &[(&str, String)],
        approved: bool,
        approved_by: &str,
        cancel: &CancelToken,
        on_output: &mut dyn FnMut(&str),
    ) -> Result<BaselineRun> {
        let record = match self.validation.run_proposal_in_tree(
            proposal_id,
            approved,
            approved_by,
            Some(task_id),
            tree_root,
            env,
            cancel,
            on_output,
        ) {
            Ok(record) => record,
            Err(ClientError::Cancelled) => return Ok(BaselineRun::Cancelled),
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
                return Ok(BaselineRun::Recorded(unavailable(check, termination)));
            }
        };
        let execution = record.execution;
        if execution.termination == crate::command_runner::CommandTermination::Cancelled {
            // The user cancelled while it ran. Not evidence about the code or
            // the environment: the caller records the baseline as declined.
            self.audit_log.record(
                "verification_baseline_cancelled",
                &[
                    ("actor", "system".to_string()),
                    ("sessionId", session_id.to_string()),
                    ("taskId", task_id.to_string()),
                    ("check", check.label()),
                ],
            )?;
            return Ok(BaselineRun::Cancelled);
        }
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
            return Ok(BaselineRun::Recorded(CheckBaseline::Passed));
        }
        let termination =
            CheckTermination::from_command(execution.termination, execution.exit_code);
        if matches!(&termination, CheckTermination::MissingExecutable) {
            return Ok(BaselineRun::Recorded(unavailable(check, termination)));
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
        Ok(BaselineRun::Recorded(CheckBaseline::Failed {
            termination,
            fingerprint,
            findings,
        }))
    }

    fn record(
        &self,
        session_id: &str,
        task_id: &str,
        check: &CheckId,
        baseline: CheckBaseline,
        tests: Vec<String>,
    ) -> Result<()> {
        self.session_store.append_verification_event(
            session_id,
            &VerificationEvent::Baseline {
                task_id: task_id.to_string(),
                check: check.clone(),
                baseline,
                tests,
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

    /// Drops every pending card for `source_id`, so a repeated request never
    /// finds two competing decisions for one verification.
    fn remove_pending_for_source(&self, source_id: &str) -> Result<()> {
        for (path, pending) in pending_baselines_in(&self.pending_dir())? {
            if pending.batch.source_id == source_id {
                fs::remove_file(path)?;
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

/// Every readable pending baseline card under `directory`. An unreadable or
/// foreign file is skipped.
fn pending_baselines_in(directory: &Path) -> Result<Vec<(PathBuf, PendingBaseline)>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut found = Vec::new();
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
        found.push((entry.path(), pending));
    }
    Ok(found)
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

/// Stable for one source and one set of still-undecided checks, so repeated
/// requests re-offer the same card instead of leaving orphaned pending batches
/// behind.
fn batch_id_for(source_id: &str, checks: &[BaselineCheck]) -> String {
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
        "sourceId": source_id,
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

/// How one baseline run ended.
enum BaselineRun {
    Recorded(CheckBaseline),
    /// The caller's token was cancelled while it ran.
    Cancelled,
    /// The caller's token was cancelled before it started.
    NotStarted,
}

/// The engine's line for a baseline that starts running. It says what the
/// run is for (spec 114 D5), not only that it runs.
fn running_label(check: &str, tests: &[String]) -> String {
    let base = format!("Checking whether {check} failed before this task's changes");
    match tests.len() {
        0 => base,
        1 => format!("{base}, only the failing test"),
        count => format!("{base}, only the {count} failing tests"),
    }
}

/// The failing tests a baseline of `check` can rerun by name, from the
/// findings of its failed run (spec 109 D3): `None` when the whole check must
/// rerun. Only a Rust test runner whose every error is a parsed test failure
/// with a plain path name qualifies. A compile error, an unparsed (generic)
/// failure, a doctest (its name is not an identifier path) or a command that
/// already passes arguments to the test binaries, or is more than one
/// command, reruns whole. Warnings are ignored: the narrowed run builds the
/// same targets and prints them again.
fn failing_tests(check: &CheckId, findings: &[FindingEvidence]) -> Option<Vec<String>> {
    let command = check.command.as_str();
    if !is_rust_test_command(command)
        || command
            .split_whitespace()
            .any(|token| token == "--" || token.starts_with("--no-tests"))
        || command.contains(['&', '|', ';', '<', '>', '$', '`', '(', ')', '\n', '\\'])
    {
        return None;
    }
    let mut names = BTreeSet::new();
    for finding in findings {
        if finding.severity() != Severity::Error {
            continue;
        }
        if finding.source() != FindingSource::Test {
            return None;
        }
        let name = failed_test_name(finding.summary()).filter(|name| is_plain_test_name(name))?;
        names.insert(name.to_string());
    }
    (!names.is_empty()).then(|| names.into_iter().collect())
}

/// A test path such as `tests::parses_a_header`: `::`-separated Rust
/// identifiers, so it is passed to the test binary as one plain argument.
fn is_plain_test_name(name: &str) -> bool {
    name.split("::").all(|segment| {
        segment
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && segment
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

/// `command` narrowed to the named tests. `--exact` makes each name match
/// only that test, never a test whose name merely contains it; libtest and
/// nextest both accept it after `--`.
///
/// A test the task added does not exist in the task-start tree, so the
/// narrowed run may match nothing. libtest then passes (exit 0), but nextest
/// fails with `error: no tests to run` (exit 4, nextest 0.9.144), which would
/// record the baseline as failed and turn a new regression into `worsened`.
/// `--no-tests=pass` makes nextest pass an empty run as libtest does.
fn targeted_command(command: &str, tests: &[String]) -> String {
    let command = command.trim_end();
    let no_tests = if is_nextest_command(command) {
        " --no-tests=pass"
    } else {
        ""
    };
    format!("{command}{no_tests} -- --exact {}", tests.join(" "))
}

/// Whether an earlier baseline already answers for a failure that needs
/// `wanted` (the failing tests, or `None` for the whole check). A declined,
/// unavailable or uncertain baseline answers the same way whatever fails; one
/// that ran answers only for the tests it ran.
fn baseline_covers(
    baseline: &CheckBaseline,
    covered: &[String],
    wanted: Option<&[String]>,
) -> bool {
    if !matches!(
        baseline,
        CheckBaseline::Passed | CheckBaseline::Failed { .. }
    ) || covered.is_empty()
    {
        return true;
    }
    wanted.is_some_and(|wanted| wanted.iter().all(|name| covered.contains(name)))
}

/// The engine's line for a finished baseline (spec 114 D5); the report's
/// `baselines` use the same words.
fn finished_label(result: &str, check: &str) -> String {
    crate::verification::baseline_finished_label(result, check)
}

fn unavailable(check: &CheckId, termination: CheckTermination) -> CheckBaseline {
    CheckBaseline::Unavailable {
        fingerprint: FailureFingerprint::compute(check, &[], &termination),
        termination,
    }
}

/// The task-local baselines a task currently has, keyed by check.
///
/// Exposed as a value rather than a second lookup rule so spec 102 Task 3
/// classifies against exactly the state that was recorded.
pub type TaskBaselines = BTreeMap<CheckId, CheckBaseline>;

#[cfg(test)]
mod targeted_rerun_tests {
    use super::*;
    use crate::finding::{Finding, FindingDraft};

    fn evidence(source: FindingSource, severity: Severity, summary: &str) -> FindingEvidence {
        FindingEvidence::from_finding(&Finding::new(
            FindingDraft {
                source,
                severity,
                summary: summary.to_string(),
                details: None,
                range: None,
                code: None,
            },
            &SecretScanner::default(),
        ))
    }

    fn test_failure(summary: &str) -> FindingEvidence {
        evidence(FindingSource::Test, Severity::Error, summary)
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn rust_test_failures_rerun_by_name_for_both_runners() {
        let findings = [
            test_failure("tests::b failed"),
            test_failure("tests::a: assertion `left == right` failed"),
            // A warning is printed again by the narrowed run; it does not
            // stop narrowing.
            evidence(
                FindingSource::Compiler,
                Severity::Warning,
                "unused variable",
            ),
        ];
        for command in [
            "cargo test",
            "cargo test -p demo --no-fail-fast",
            "cargo nextest run -p demo --test a",
        ] {
            assert_eq!(
                failing_tests(&CheckId::new("", command), &findings),
                Some(names(&["tests::a", "tests::b"])),
                "{command}"
            );
        }
        assert_eq!(
            targeted_command(
                "cargo nextest run -p demo",
                &names(&["tests::a", "tests::b"])
            ),
            "cargo nextest run -p demo --no-tests=pass -- --exact tests::a tests::b"
        );
        assert_eq!(
            targeted_command("cargo test -p demo", &names(&["tests::a"])),
            "cargo test -p demo -- --exact tests::a",
            "libtest already passes an empty run"
        );
    }

    #[test]
    fn anything_else_reruns_the_whole_check() {
        let a = test_failure("tests::a failed");
        let cases: [(&str, Vec<FindingEvidence>); 8] = [
            // Not a Rust test runner.
            ("npm test", vec![a.clone()]),
            // Already passes arguments to the test binaries.
            ("cargo test -- --nocapture", vec![a.clone()]),
            // Sets its own empty-run rule, which a narrowed run must not
            // inherit.
            ("cargo nextest run --no-tests=fail", vec![a.clone()]),
            // More than one command.
            ("cargo test && echo done", vec![a.clone()]),
            // A compile error is not a test failure.
            (
                "cargo test",
                vec![
                    a.clone(),
                    evidence(FindingSource::Compiler, Severity::Error, "mismatched types"),
                ],
            ),
            // An unparsed failure names no test.
            (
                "cargo test",
                vec![evidence(
                    FindingSource::Command,
                    Severity::Error,
                    "cargo test: Killed: 9",
                )],
            ),
            // A doctest's name is not a plain test path.
            (
                "cargo test",
                vec![test_failure("src/lib.rs - add (line 3) failed")],
            ),
            // Nothing failed by name.
            ("cargo test", Vec::new()),
        ];
        for (command, findings) in cases {
            assert_eq!(
                failing_tests(&CheckId::new("", command), &findings),
                None,
                "{command}: {findings:?}"
            );
        }
    }

    #[test]
    fn a_baseline_covers_only_the_tests_it_ran() {
        let ran = names(&["tests::a", "tests::b"]);
        let passed = CheckBaseline::Passed;
        assert!(baseline_covers(&passed, &ran, Some(&names(&["tests::a"]))));
        assert!(!baseline_covers(
            &passed,
            &ran,
            Some(&names(&["tests::a", "tests::c"]))
        ));
        assert!(!baseline_covers(&passed, &ran, None), "a whole-check need");
        assert!(
            baseline_covers(&passed, &[], None),
            "a whole-check baseline"
        );
        assert!(baseline_covers(&passed, &[], Some(&names(&["tests::c"]))));
        // A declined or unavailable baseline answers the same whatever fails.
        assert!(baseline_covers(&CheckBaseline::Declined, &ran, None));
    }
}
