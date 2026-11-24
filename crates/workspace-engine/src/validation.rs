use crate::audit::AuditLog;
use crate::cancel::CancelToken;
use crate::command_policy::{
    ApprovalCause, CommandClassification, CommandPolicy, CommandRisk, DecidingRule,
    allow_always_eligible, home_directory, relative_location, root_qualified_allowlist_entry,
    session_approval_refusal,
};
use crate::command_purpose::{CommandPurpose, command_purpose};
use crate::command_rules::{CommandRule, RuleEffect, RuleSource};
use crate::command_runner::{CommandExecution, CommandRunOptions, CommandRunner};
use crate::config::ConfigOverlay;
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis, repository_id_for_root};
use crate::path_policy::PathPolicy;
use crate::repository_map::{RepositoryMap, RootOverride};
use crate::session::{CommandSessionApproval, SessionStore};
use crate::session_approval::RepositoryState;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandProposal {
    pub id: String,
    pub command: String,
    pub working_directory: String,
    /// The repository the command belongs to. Equal to `working_directory`
    /// unless the command runs in a sub-root (spec 24 `context.md` §1).
    pub repository_root: String,
    pub reason: String,
    pub risk: CommandRisk,
    pub requires_approval: bool,
    pub blocked: bool,
    pub expected_effects: String,
    pub may_use_network: bool,
    pub reasons: Vec<String>,
    /// Why the policy asked or blocked, as values in the classifier's order
    /// (spec 62 §5.4). Stored with the proposal so reattach and *Allow Always*
    /// read the same causes the audit record names.
    pub causes: Vec<ApprovalCause>,
    /// The command rule or built-in default that decided (spec 111 D7), for
    /// the card. `None` for a command Damaian's other defaults decided, and
    /// for a proposal stored before command rules decided.
    pub rule: Option<DecidingRule>,
    pub created_at_ms: u128,
    pub status: String,
    /// What the command is for (spec 63 §5.1), decided from the
    /// classification after the policy has judged it. It describes the run's
    /// findings and evidence and never feeds back into approval.
    pub purpose: CommandPurpose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRunRecord {
    pub proposal_id: String,
    pub execution: CommandExecution,
    pub stdout_ref: PathBuf,
    pub stderr_ref: PathBuf,
    pub summary_ref: PathBuf,
    /// The run was covered by an **Approve for this session** and changed
    /// the repository, so the approval was revoked (spec 109 D5): the paths
    /// it changed. `None` for every other run.
    pub session_approval_revoked: Option<Vec<String>>,
}

/// Resolves a model-requested, repository-relative working directory into the
/// absolute directory a command may run in (spec 24 Task 7).
///
/// An empty request is the repository root, exactly as before the tool could
/// name a directory. Otherwise the path must resolve inside the repository,
/// name a directory, and not be restricted by policy. A refusal is the
/// `ClientError` the chat loop hands back to the model as the tool result, so
/// nothing is proposed or run.
pub fn resolve_command_directory(
    path_policy: &PathPolicy,
    repository_root: &Path,
    requested: &str,
) -> Result<PathBuf> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Ok(repository_root.to_path_buf());
    }
    let resolved = path_policy.resolve_existing(repository_root, requested, false)?;
    if !resolved.absolute_path.is_dir() {
        return Err(ClientError::InvalidInput(format!(
            "Working directory is not a directory: {requested}"
        )));
    }
    if path_policy.is_restricted(&resolved.relative_path, true) {
        return Err(ClientError::AccessDenied(format!(
            "Working directory is restricted by policy: {}",
            resolved.relative_path
        )));
    }
    // Kept relative to the caller's `repository_root` rather than the
    // canonical one, so the pair `classify_at` compares stays consistent.
    Ok(repository_root.join(&resolved.relative_path))
}

#[derive(Debug, Clone)]
pub struct CommandStore {
    data_dir: PathBuf,
}

impl CommandStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    pub fn save_proposal(&self, proposal: &CommandProposal) -> Result<PathBuf> {
        let path = self.proposal_path(&proposal.id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serialize_proposal(proposal)?)?;
        Ok(path)
    }

    pub fn load_proposal(&self, proposal_id: &str) -> Result<CommandProposal> {
        let content = fs::read_to_string(self.proposal_path(proposal_id))?;
        deserialize_proposal(&content)
    }

    pub fn save_execution(
        &self,
        proposal: &CommandProposal,
        execution: &CommandExecution,
    ) -> Result<CommandRunRecord> {
        let output_dir = self
            .data_dir
            .join("commands")
            .join("output")
            .join(&execution.id);
        fs::create_dir_all(&output_dir)?;
        let stdout_ref = output_dir.join("stdout.log");
        let stderr_ref = output_dir.join("stderr.log");
        let summary_ref = output_dir.join("summary.dcmd");
        fs::write(&stdout_ref, &execution.stdout)?;
        fs::write(&stderr_ref, &execution.stderr)?;
        fs::write(
            &summary_ref,
            serialize_execution_summary(proposal, execution, &stdout_ref, &stderr_ref),
        )?;
        Ok(CommandRunRecord {
            proposal_id: proposal.id.clone(),
            execution: execution.clone(),
            stdout_ref,
            stderr_ref,
            summary_ref,
            session_approval_revoked: None,
        })
    }

    pub fn mark_rejected(&self, proposal_id: &str, rejected_by: &str) -> Result<PathBuf> {
        let proposal = self.load_proposal(proposal_id)?;
        let path = self
            .data_dir
            .join("commands")
            .join("rejected")
            .join(format!("{proposal_id}.dcmd"));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut content = serialize_proposal(&proposal)?;
        write_field(&mut content, "REJECTED_BY", rejected_by);
        write_field(&mut content, "REJECTED_AT_MS", &now_millis().to_string());
        fs::write(&path, content)?;
        Ok(path)
    }

    fn proposal_path(&self, proposal_id: &str) -> PathBuf {
        self.data_dir
            .join("commands")
            .join("pending")
            .join(format!("{proposal_id}.dcmd"))
    }
}

#[derive(Debug, Clone)]
pub struct ValidationOrchestrator {
    command_policy: CommandPolicy,
    command_runner: CommandRunner,
    command_store: CommandStore,
    audit_log: AuditLog,
}

impl ValidationOrchestrator {
    pub fn new(
        command_policy: CommandPolicy,
        command_runner: CommandRunner,
        command_store: CommandStore,
        audit_log: AuditLog,
    ) -> Self {
        Self {
            command_policy,
            command_runner,
            command_store,
            audit_log,
        }
    }

    /// Whether this command would stop for a human, answered without storing
    /// a proposal or writing an audit entry.
    ///
    /// Runs the same classifier [`Self::propose_command`] does, so the two
    /// cannot disagree about a command. It exists separately because spec 21's
    /// plan review gate has to ask "would this step change anything?" *before*
    /// the turn commits to proposing it — a proposal stored for a step that
    /// then never runs would leave a pending command nobody asked about.
    pub fn command_needs_approval(
        &self,
        working_directory: impl AsRef<Path>,
        command: &str,
    ) -> bool {
        let classification = self.classify_command(working_directory, command);
        classification.requires_approval || classification.blocked
    }

    /// The classification [`Self::propose_command`] would store, without
    /// storing it. Spec 20's Layer 3 asks the session mode about a command
    /// before the turn commits to proposing it, for the same reason
    /// [`Self::command_needs_approval`] exists: a refused command must leave
    /// no pending proposal behind.
    pub fn classify_command(
        &self,
        working_directory: impl AsRef<Path>,
        command: &str,
    ) -> CommandClassification {
        self.classify_command_at(&working_directory, &working_directory, command)
    }

    /// [`Self::classify_command`] for a command run in `working_directory`
    /// inside the repository at `repository_root`.
    pub fn classify_command_at(
        &self,
        repository_root: impl AsRef<Path>,
        working_directory: impl AsRef<Path>,
        command: &str,
    ) -> CommandClassification {
        self.command_policy.classify_at(
            command,
            repository_root.as_ref(),
            working_directory.as_ref(),
        )
    }

    pub fn propose_command(
        &self,
        working_directory: impl AsRef<Path>,
        command: &str,
        reason: &str,
    ) -> Result<CommandProposal> {
        self.propose_command_at(&working_directory, &working_directory, command, reason)
    }

    /// Stores a proposal to run `command` in `working_directory`, inside the
    /// repository at `repository_root`.
    pub fn propose_command_at(
        &self,
        repository_root: impl AsRef<Path>,
        working_directory: impl AsRef<Path>,
        command: &str,
        reason: &str,
    ) -> Result<CommandProposal> {
        self.store_proposal(
            repository_root,
            working_directory,
            command,
            reason,
            false,
            Vec::new(),
        )
    }

    /// [`Self::propose_command_at`] with a cause from outside the classifier:
    /// a `before_tool` hook that asked for approval (spec 32 `context.md` §4).
    /// The cause is stored with the proposal and written to the same
    /// `command_proposal_stored` record the eval harness reads, so a prompt a
    /// hook forced is attributed to the hook rather than to a missing cause.
    pub fn propose_command_at_with_causes(
        &self,
        repository_root: impl AsRef<Path>,
        working_directory: impl AsRef<Path>,
        command: &str,
        reason: &str,
        extra_causes: Vec<ApprovalCause>,
    ) -> Result<CommandProposal> {
        self.store_proposal(
            repository_root,
            working_directory,
            command,
            reason,
            false,
            extra_causes,
        )
    }

    /// Proposes a plan-declared required check for the verification loop to
    /// rerun (spec 106 Task 6). The command is classified by the ordinary
    /// policy exactly like any other proposal, so a rerun never needs less
    /// approval than the model's own run of it did. Its purpose is `Check`,
    /// as a detected validation's is: it is the check the plan declared.
    pub fn propose_plan_check(
        &self,
        repository_root: impl AsRef<Path>,
        working_directory: impl AsRef<Path>,
        command: &str,
    ) -> Result<CommandProposal> {
        self.store_proposal(
            repository_root,
            working_directory,
            command,
            "Required check from the plan, run again because the files changed after it last ran",
            true,
            Vec::new(),
        )
    }

    /// Proposes the plan's scoped required check as a change's per-change
    /// check, in place of the detected checks of the root it covers (spec 109
    /// D4). The same policy path as every check; only the stored reason differs.
    pub fn propose_scoped_plan_check(
        &self,
        repository_root: impl AsRef<Path>,
        working_directory: impl AsRef<Path>,
        command: &str,
    ) -> Result<CommandProposal> {
        self.store_proposal(
            repository_root,
            working_directory,
            command,
            "Required check from the plan, run for this change in place of the detected check",
            true,
            Vec::new(),
        )
    }

    /// Proposes the approved plan's required check as the completion floor of
    /// a project root Damaian detects no check for (spec 109 D9).
    pub fn propose_floor_plan_check(
        &self,
        repository_root: impl AsRef<Path>,
        working_directory: impl AsRef<Path>,
        command: &str,
    ) -> Result<CommandProposal> {
        self.store_proposal(
            repository_root,
            working_directory,
            command,
            "Required check from the plan, run at task completion because Damaian detects no check for this part of the project",
            true,
            Vec::new(),
        )
    }

    /// Proposes a check the user configured for this repository
    /// (`project_checks`, spec 109 D9), run at task completion.
    pub fn propose_project_check(
        &self,
        repository_root: impl AsRef<Path>,
        working_directory: impl AsRef<Path>,
        command: &str,
    ) -> Result<CommandProposal> {
        self.store_proposal(
            repository_root,
            working_directory,
            command,
            "Check you configured for this project, run at task completion",
            true,
            Vec::new(),
        )
    }

    /// Proposes a build check — the plan step's compile or type-check command,
    /// or the Cargo fast path — that a per-change verification runs before
    /// anything else (spec 109 D10). The same policy path as every check; it
    /// asks where the policy asks.
    pub fn propose_build_check(
        &self,
        repository_root: impl AsRef<Path>,
        working_directory: impl AsRef<Path>,
        command: &str,
    ) -> Result<CommandProposal> {
        self.store_proposal(
            repository_root,
            working_directory,
            command,
            "Build check after a change: compile only, so a change that does not build is caught at once",
            true,
            Vec::new(),
        )
    }

    /// `detected` marks a proposal from [`Self::propose_detected_validations`],
    /// whose purpose is `Check` whatever its text.
    fn store_proposal(
        &self,
        repository_root: impl AsRef<Path>,
        working_directory: impl AsRef<Path>,
        command: &str,
        reason: &str,
        detected: bool,
        extra_causes: Vec<ApprovalCause>,
    ) -> Result<CommandProposal> {
        let classification =
            self.classify_command_at(&repository_root, &working_directory, command);
        let mut proposal = proposal_from_classification(
            repository_root.as_ref(),
            working_directory.as_ref(),
            reason,
            classification,
            detected,
        );
        for cause in extra_causes {
            if proposal.causes.contains(&cause) {
                continue;
            }
            if !proposal.blocked {
                proposal.requires_approval = true;
            }
            proposal.reasons.push(cause.plain_words());
            proposal.causes.push(cause);
        }
        self.command_store.save_proposal(&proposal)?;
        self.audit_log.record(
            "command_proposal_stored",
            &[
                ("actor", "system".to_string()),
                ("proposalId", proposal.id.clone()),
                ("command", proposal.command.clone()),
                ("workingDirectory", proposal.working_directory.clone()),
                ("repositoryRoot", proposal.repository_root.clone()),
                ("risk", proposal.risk.as_str().to_string()),
                ("requiresApproval", proposal.requires_approval.to_string()),
                ("blocked", proposal.blocked.to_string()),
                // Names only, in classifier order, and empty for a command that
                // ran without a cause (spec 62 §5.4). `AuditLog::record` runs
                // every value through the secret scanner like the fields above.
                (
                    "causes",
                    proposal
                        .causes
                        .iter()
                        .map(ApprovalCause::as_str)
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                // The deciding command rule as written, empty when none
                // decided (spec 111 D7).
                (
                    "rule",
                    proposal
                        .rule
                        .as_ref()
                        .filter(|rule| rule.source != "default")
                        .map(|rule| rule.rule.clone())
                        .unwrap_or_default(),
                ),
            ],
        )?;
        // A rule of the user's lifted one of Damaian's built-in blocks (D7):
        // the rule text and the default's name, never the command's output.
        if let Some(rule) = &proposal.rule
            && let Some(overridden) = &rule.overrides
        {
            self.audit_log.record(
                "command_rule_override",
                &[
                    ("actor", "system".to_string()),
                    ("proposalId", proposal.id.clone()),
                    ("rule", rule.rule.clone()),
                    ("ruleSource", rule.source.clone()),
                    ("builtInDefault", overridden.name.clone()),
                    ("workingDirectory", proposal.working_directory.clone()),
                ],
            )?;
        }
        Ok(proposal)
    }

    /// Proposes every root's detected commands, each at its own root's
    /// directory, so a command found in `packages/api` runs there (spec 24
    /// `context.md` §9). A removed root proposes nothing. The map's stored
    /// risk is not used: each proposal is classified again where it will run.
    pub fn propose_detected_validations(
        &self,
        repository_root: impl AsRef<Path>,
        map: &RepositoryMap,
    ) -> Result<Vec<CommandProposal>> {
        let repository_root = repository_root.as_ref();
        let mut proposals = Vec::new();
        for root in &map.roots {
            if root.user_override == Some(RootOverride::Removed) {
                continue;
            }
            for command in &root.commands {
                let working_directory = if command.working_directory.is_empty() {
                    repository_root.to_path_buf()
                } else {
                    repository_root.join(&command.working_directory)
                };
                proposals.push(self.store_proposal(
                    repository_root,
                    &working_directory,
                    &command.command,
                    &format!("Detected project validation command from {}", command.name),
                    true,
                    Vec::new(),
                )?);
            }
        }
        Ok(proposals)
    }

    pub fn run_proposal(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
        task_id: Option<&str>,
        cancel: &CancelToken,
        on_output: &mut dyn FnMut(&str),
    ) -> Result<CommandRunRecord> {
        let proposal = self.command_store.load_proposal(proposal_id)?;
        if proposal.blocked {
            return Err(ClientError::PolicyBlocked(
                "Command proposal is blocked by policy".to_string(),
            ));
        }
        if proposal.requires_approval && !approved {
            return Err(ClientError::ApprovalRequired(
                "Command proposal requires explicit approval before execution".to_string(),
            ));
        }
        // Spec 109 D5: a run a session approval covers — one that runs
        // without a card because of it, or the run the user approved for
        // the session — is checked before and after.
        let covered = self.covering_session_approval(&proposal);
        let approved_by = match &covered {
            Some(_) if !approved => "session_approval",
            _ => approved_by,
        };
        let data_dir = self.command_policy.config().data_dir.clone();
        let capture =
            || RepositoryState::capture(Path::new(&proposal.repository_root), Some(&data_dir));
        let before = covered.as_ref().and_then(|_| capture());
        let execution = self.command_runner.run_at(
            &proposal.command,
            &proposal.repository_root,
            &proposal.working_directory,
            &proposal.reason,
            CommandRunOptions {
                approved,
                approved_by: Some(approved_by),
                task_id,
                cancel,
                on_output,
                purpose: proposal.purpose,
            },
        )?;
        let mut record = self.command_store.save_execution(&proposal, &execution)?;
        self.audit_log.record(
            "stored_command_executed",
            &[
                ("actor", "command".to_string()),
                ("proposalId", proposal.id.clone()),
                ("commandId", execution.id.clone()),
                ("exitCode", execution.exit_code.unwrap_or(-1).to_string()),
                ("purpose", execution.purpose.as_str().to_string()),
                ("stdoutRef", record.stdout_ref.to_string_lossy().to_string()),
                ("stderrRef", record.stderr_ref.to_string_lossy().to_string()),
                ("approvedBy", approved_by.to_string()),
            ],
        )?;
        if let Some((session_id, approval)) = covered {
            let after = capture();
            let changed = match (&before, &after) {
                (Some(before), Some(after)) => before.changed_since(after),
                // A state Git could not read is not evidence the run kept
                // the repository unchanged.
                _ => vec!["(the repository state could not be read)".to_string()],
            };
            if !changed.is_empty() {
                self.revoke_session_approval(&session_id, &approval, &proposal, &changed)?;
                record.session_approval_revoked = Some(changed);
            }
        }
        Ok(record)
    }

    /// The session approval covering `proposal` in the session whose turn
    /// runs on this thread, if one does: the policy, with that session's
    /// rules, lets it run because of a session rule.
    fn covering_session_approval(
        &self,
        proposal: &CommandProposal,
    ) -> Option<(String, CommandSessionApproval)> {
        let session_id = crate::session_approval::current_session()?;
        let approvals = self
            .session_store()
            .command_session_approvals(&session_id)
            .ok()?;
        if approvals.is_empty() {
            return None;
        }
        let classification = self.command_policy.classify_at(
            &proposal.command,
            Path::new(&proposal.repository_root),
            Path::new(&proposal.working_directory),
        );
        let rule = classification
            .rule
            .filter(|rule| rule.source == "session")?;
        let config = self.command_policy.config();
        approvals
            .into_iter()
            .find(|approval| {
                CommandRule::session(
                    &approval.command,
                    Path::new(&approval.directory),
                    home_directory().as_deref(),
                    config.shell_parameters(),
                )
                .is_ok_and(|candidate| candidate.text() == rule.rule)
            })
            .map(|approval| (session_id, approval))
    }

    fn session_store(&self) -> SessionStore {
        SessionStore::new(&self.command_policy.config().data_dir)
    }

    /// Records **Approve for this session** for a stored proposal (spec 109
    /// D5), after checking it qualifies: the exact command in the exact
    /// directory, for `session_id` only. The caller runs the command next,
    /// as for an ordinary approval.
    pub fn approve_for_session(
        &self,
        session_id: &str,
        proposal_id: &str,
        approved_by: &str,
    ) -> Result<CommandSessionApproval> {
        let proposal = self.command_store.load_proposal(proposal_id)?;
        if let Some(refusal) = self.session_approval_refusal(&proposal) {
            return Err(ClientError::PolicyBlocked(format!(
                "Cannot approve `{}` for this session: {refusal}",
                proposal.command
            )));
        }
        let directory =
            crate::command_policy::normalize_lexically(Path::new(&proposal.working_directory))
                .to_string_lossy()
                .into_owned();
        let approval = CommandSessionApproval {
            command: proposal.command.trim().to_string(),
            directory,
        };
        self.session_store().approve_command_for_session(
            session_id,
            &approval.command,
            &approval.directory,
            approved_by,
        )?;
        self.audit_log.record(
            "command_approved_for_session",
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", session_id.to_string()),
                ("proposalId", proposal.id.clone()),
                ("command", approval.command.clone()),
                ("workingDirectory", approval.directory.clone()),
            ],
        )?;
        Ok(approval)
    }

    /// Why a stored proposal is not offered **Approve for this session**, or
    /// `None` when it is; judged from the causes it was stored with.
    pub fn session_approval_refusal(&self, proposal: &CommandProposal) -> Option<String> {
        session_approval_refusal(
            self.command_policy.config(),
            &proposal.command,
            &proposal.causes,
            proposal.blocked,
            proposal.requires_approval,
            Path::new(&proposal.repository_root),
            Path::new(&proposal.working_directory),
        )
    }

    /// Ends every session approval of `session_id` (**Revoke session
    /// approvals**); the number ended.
    pub fn revoke_session_approvals(&self, session_id: &str, revoked_by: &str) -> Result<usize> {
        let count = self
            .session_store()
            .revoke_all_command_session_approvals(session_id, "revoked_by_user")?;
        if count > 0 {
            self.audit_log.record(
                "command_session_approval_revoked",
                &[
                    ("actor", revoked_by.to_string()),
                    ("sessionId", session_id.to_string()),
                    ("reason", "revoked_by_user".to_string()),
                    ("count", count.to_string()),
                ],
            )?;
        }
        Ok(count)
    }

    fn revoke_session_approval(
        &self,
        session_id: &str,
        approval: &CommandSessionApproval,
        proposal: &CommandProposal,
        changed: &[String],
    ) -> Result<()> {
        self.session_store().revoke_command_session_approval(
            session_id,
            approval,
            "changed_repository",
            changed,
        )?;
        self.audit_log.record(
            "command_session_approval_revoked",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("proposalId", proposal.id.clone()),
                ("command", approval.command.clone()),
                ("workingDirectory", approval.directory.clone()),
                ("reason", "changed_repository".to_string()),
                ("changed", changed.join(",")),
            ],
        )?;
        crate::session_approval::push_notice(session_approval_revoked_note(
            &approval.command,
            changed,
        ));
        Ok(())
    }

    /// Runs a stored proposal in a copy of its repository instead of the
    /// repository itself (spec 109 D2: the failure-time baseline's task-start
    /// tree).
    ///
    /// The approval decision is the stored proposal's, made against the real
    /// repository: a blocked proposal never runs and one that needs approval
    /// runs only with `approved`. The command and its directory relative to the
    /// repository root are unchanged; only the root moves to `tree_root`. The
    /// runner still classifies the command where it runs, so a policy that
    /// refuses it there refuses it here too. `env` is added to the child.
    ///
    /// The recorded output names the tree's paths as the repository's, so a
    /// finding parsed from it locates the same file as the live run's would.
    #[allow(clippy::too_many_arguments)]
    pub fn run_proposal_in_tree(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
        task_id: Option<&str>,
        tree_root: &Path,
        env: &[(&str, String)],
        cancel: &CancelToken,
        on_output: &mut dyn FnMut(&str),
    ) -> Result<CommandRunRecord> {
        let proposal = self.command_store.load_proposal(proposal_id)?;
        if proposal.blocked {
            return Err(ClientError::PolicyBlocked(
                "Command proposal is blocked by policy".to_string(),
            ));
        }
        if proposal.requires_approval && !approved {
            return Err(ClientError::ApprovalRequired(
                "Command proposal requires explicit approval before execution".to_string(),
            ));
        }
        let repository_root = Path::new(&proposal.repository_root);
        let relative = Path::new(&proposal.working_directory)
            .strip_prefix(repository_root)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let working_directory = tree_root.join(&relative);
        let mut execution = self.command_runner.run_at_with_env(
            &proposal.command,
            tree_root,
            &working_directory,
            &proposal.reason,
            CommandRunOptions {
                // The stored decision stands: the runner reclassifies against
                // the tree, which is not the repository a grant was made for.
                approved: approved || !proposal.requires_approval,
                approved_by: Some(approved_by),
                task_id,
                cancel,
                on_output,
                purpose: proposal.purpose,
            },
            env,
        )?;
        let repository = repository_root.to_string_lossy().to_string();
        let mut tree_forms = vec![tree_root.to_string_lossy().to_string()];
        if let Ok(canonical) = std::fs::canonicalize(tree_root) {
            tree_forms.push(canonical.to_string_lossy().to_string());
        }
        // Longest first, so `/private/var/...` is replaced before `/var/...`.
        tree_forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
        for form in &tree_forms {
            execution.stdout = execution.stdout.replace(form.as_str(), &repository);
            execution.stderr = execution.stderr.replace(form.as_str(), &repository);
        }
        execution.working_directory = proposal.working_directory.clone();
        let record = self.command_store.save_execution(&proposal, &execution)?;
        self.audit_log.record(
            "stored_command_executed",
            &[
                ("actor", "command".to_string()),
                ("proposalId", proposal.id.clone()),
                ("commandId", execution.id.clone()),
                ("exitCode", execution.exit_code.unwrap_or(-1).to_string()),
                ("purpose", execution.purpose.as_str().to_string()),
                ("stdoutRef", record.stdout_ref.to_string_lossy().to_string()),
                ("stderrRef", record.stderr_ref.to_string_lossy().to_string()),
                ("tree", "task_start".to_string()),
            ],
        )?;
        Ok(record)
    }

    /// Permanently allows a proposal's command, so future proposals for the
    /// exact same command, in the same directory, run without asking.
    ///
    /// Writes a user command rule (spec 111 requirement 9): `command_rule =
    /// allow <command> in <working directory>` in the *user's* config, never
    /// in `<repo>/.damaian/config.conf`: the grant is the user's decision, and
    /// a repository config entry would be indistinguishable from one that
    /// arrived with a clone (spec 34 §5.4). The exact command in the exact
    /// directory, so the grant covers what was approved and nothing beside
    /// it; a directory names one checkout, so it does not transfer to
    /// another clone. A directory a rule line cannot name (a space or `*` in
    /// its path) keeps the old per-repository `command_allowlist.<id>` entry,
    /// which is read as the same rule (D10). Returns the path written.
    ///
    /// The caller is expected to run the command afterwards: this only records
    /// the decision.
    pub fn allow_command_always(&self, proposal_id: &str, approved_by: &str) -> Result<PathBuf> {
        let proposal = self.command_store.load_proposal(proposal_id)?;
        let repository_root = PathBuf::from(&proposal.repository_root);

        // Deliberately the config this orchestrator was built with, not a
        // fresh load from disk: the answer here must match the policy that
        // classified the proposal in the first place, and it keeps this method
        // a pure function of its injected dependencies. Callers are
        // responsible for constructing the orchestrator with the config of the
        // repository the proposal belongs to.
        let config = self.command_policy.config();
        if !allow_always_eligible(config, &proposal.command, proposal.blocked) {
            return Err(ClientError::PolicyBlocked(format!(
                "Command cannot be permanently allowed: {}",
                proposal.command
            )));
        }
        let Some(relative) =
            relative_location(&repository_root, Path::new(&proposal.working_directory))
        else {
            return Err(ClientError::PolicyBlocked(format!(
                "Command runs outside its repository and cannot be permanently allowed: {}",
                proposal.command
            )));
        };

        let repository_id = repository_id_for_root(&repository_root);
        let path = config.user_config_path();
        let mut overlay = ConfigOverlay::load_or_default(&path)?;
        let command = proposal.command.trim();
        let directory =
            crate::command_policy::normalize_lexically(Path::new(&proposal.working_directory))
                .to_string_lossy()
                .into_owned();
        let (granted, already_allowed) = match exact_user_rule(command, &directory) {
            Some(rule) => {
                let text = rule.text();
                let already_allowed = overlay
                    .command_rules
                    .iter()
                    .any(|existing| existing.text() == text);
                if !already_allowed {
                    overlay.command_rules.push(rule);
                }
                (text, already_allowed)
            }
            None => {
                // The per-repository key holds only what was granted here.
                let mut allowlist = overlay
                    .command_allowlist_by_repository
                    .remove(&repository_id)
                    .unwrap_or_default();
                let entry = if relative.is_empty() {
                    command.to_string()
                } else {
                    root_qualified_allowlist_entry(&relative, command)
                };
                let already_allowed = allowlist.iter().any(|existing| existing.trim() == entry);
                if !already_allowed {
                    allowlist.push(entry.clone());
                }
                overlay
                    .command_allowlist_by_repository
                    .insert(repository_id.clone(), allowlist);
                (entry, already_allowed)
            }
        };
        overlay.save(&path)?;

        self.audit_log.record(
            "command_allowlisted",
            &[
                ("actor", "user".to_string()),
                ("proposalId", proposal.id.clone()),
                ("command", command.to_string()),
                // The rule line written (`allow cargo test in /repo`), or the
                // old allowlist entry for a directory a rule cannot name.
                ("rule", granted),
                ("approvedBy", approved_by.to_string()),
                ("alreadyAllowed", already_allowed.to_string()),
                ("repositoryId", repository_id),
                ("resourcePath", path.to_string_lossy().to_string()),
            ],
        )?;
        Ok(path)
    }

    /// Reads back a stored proposal. Callers that need to act on a proposal's
    /// repository before building a repository-scoped engine (the CLI, which
    /// is handed only a proposal id) need this to find the working directory.
    pub fn load_proposal(&self, proposal_id: &str) -> Result<CommandProposal> {
        self.command_store.load_proposal(proposal_id)
    }

    /// The policy this orchestrator classifies with. The verification loop
    /// builds a repository map from the same policy, so the map's per-root
    /// commands and the proposals the loop stores cannot disagree about a risk
    /// or an allowlist entry (spec 23 Task 4).
    pub(crate) fn command_policy(&self) -> &CommandPolicy {
        &self.command_policy
    }

    /// The store the baseline path reads a proposal back from before running
    /// it, so a resumed batch executes exactly the command the user saw.
    pub(crate) fn command_store(&self) -> &CommandStore {
        &self.command_store
    }

    pub fn reject_proposal(&self, proposal_id: &str, rejected_by: &str) -> Result<PathBuf> {
        let path = self.command_store.mark_rejected(proposal_id, rejected_by)?;
        self.audit_log.record(
            "stored_command_rejected",
            &[
                ("actor", "user".to_string()),
                ("proposalId", proposal_id.to_string()),
                ("rejectedBy", rejected_by.to_string()),
                ("resourcePath", path.to_string_lossy().to_string()),
            ],
        )?;
        Ok(path)
    }
}

/// The user rule *Always allow* writes: `allow <command> in <directory>`,
/// when that line reads back as exactly this rule. `None` when it would not
/// (a space or `*` in the directory, which a rule line cannot say).
fn exact_user_rule(command: &str, directory: &str) -> Option<CommandRule> {
    if directory.contains('*') {
        return None;
    }
    let rule = CommandRule::new(
        RuleEffect::Allow,
        command,
        Some(directory.to_string()),
        RuleSource::User,
    )
    .ok()?;
    let read_back = CommandRule::parse(&rule.text(), RuleSource::User).ok()?;
    (read_back == rule).then_some(rule)
}

/// The line a turn reports when a covered run changed the repository.
pub fn session_approval_revoked_note(command: &str, changed: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut paths = changed
        .iter()
        .take(SHOWN)
        .map(|path| format!("`{path}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if changed.len() > SHOWN {
        paths.push_str(&format!(" and {} more", changed.len() - SHOWN));
    }
    format!(
        "`{command}` changed the repository ({paths}), so its approval for this session is \
         revoked; Damaian will ask before running it again."
    )
}

fn proposal_from_classification(
    repository_root: &Path,
    working_directory: &Path,
    reason: &str,
    classification: CommandClassification,
    detected: bool,
) -> CommandProposal {
    // From the classification's own command, after every policy decision
    // above is final: the purpose is a description, not an input.
    let purpose = command_purpose(&classification.command, detected);
    CommandProposal {
        id: create_id("cmdprop"),
        command: classification.command,
        working_directory: working_directory.to_string_lossy().to_string(),
        repository_root: repository_root.to_string_lossy().to_string(),
        reason: reason.to_string(),
        risk: classification.risk,
        requires_approval: classification.requires_approval,
        blocked: classification.blocked,
        expected_effects: classification.expected_effects,
        may_use_network: classification.may_use_network,
        reasons: classification.reasons,
        causes: classification.causes,
        rule: classification.rule,
        created_at_ms: now_millis(),
        status: "pending".to_string(),
        purpose,
    }
}

pub fn command_approval_prompt(proposal: &CommandProposal) -> String {
    format!(
        "Proposal: {}\nCommand: {}\nWorking directory: {}\nRisk: {}\nRequires approval: {}\nNetwork: {}\nExpected effects: {}\nReasons:\n- {}\n",
        proposal.id,
        proposal.command,
        proposal.working_directory,
        proposal.risk.as_str(),
        proposal.requires_approval,
        proposal.may_use_network,
        proposal.expected_effects,
        proposal.reasons.join("\n- ")
    )
}

fn serialize_proposal(proposal: &CommandProposal) -> Result<String> {
    let mut output = String::new();
    output.push_str("DAMAIAN_COMMAND_PROPOSAL_V1\n");
    write_field(&mut output, "ID", &proposal.id);
    write_field(&mut output, "COMMAND", &proposal.command);
    write_field(
        &mut output,
        "WORKING_DIRECTORY",
        &proposal.working_directory,
    );
    write_field(&mut output, "REASON", &proposal.reason);
    write_field(&mut output, "RISK", proposal.risk.as_str());
    write_field(
        &mut output,
        "REQUIRES_APPROVAL",
        &proposal.requires_approval.to_string(),
    );
    write_field(&mut output, "BLOCKED", &proposal.blocked.to_string());
    write_field(&mut output, "EXPECTED_EFFECTS", &proposal.expected_effects);
    write_field(
        &mut output,
        "MAY_USE_NETWORK",
        &proposal.may_use_network.to_string(),
    );
    write_field(&mut output, "REASONS", &proposal.reasons.join("\n"));
    write_field(
        &mut output,
        "CREATED_AT_MS",
        &proposal.created_at_ms.to_string(),
    );
    write_field(&mut output, "STATUS", &proposal.status);
    // Last, so a reader can treat it as optional: proposals stored before
    // spec 24 end at `STATUS`.
    write_field(&mut output, "REPOSITORY_ROOT", &proposal.repository_root);
    // After it, optional the same way: proposals stored before spec 63 have
    // no purpose and read as `Other`, which keeps their old treatment.
    write_field(&mut output, "PURPOSE", proposal.purpose.as_str());
    // Last, optional the same way: proposals stored before spec 62 have no
    // causes and read back with an empty list. The value is JSON so every
    // cause keeps its fields, not only its name.
    write_field(
        &mut output,
        "CAUSES",
        &serde_json::to_string(&proposal.causes).map_err(|error| {
            ClientError::InvalidInput(format!("Could not store approval causes: {error}"))
        })?,
    );
    // Last, optional the same way: proposals stored before spec 111 Task 4
    // have no deciding rule and read back with none.
    write_field(
        &mut output,
        "RULE",
        &serde_json::to_string(&proposal.rule).map_err(|error| {
            ClientError::InvalidInput(format!("Could not store the deciding rule: {error}"))
        })?,
    );
    output.push_str("END_COMMAND_PROPOSAL\n");
    Ok(output)
}

fn deserialize_proposal(raw: &str) -> Result<CommandProposal> {
    let mut cursor = Cursor::new(raw);
    cursor.expect_line("DAMAIAN_COMMAND_PROPOSAL_V1")?;
    let id = cursor.read_field("ID")?;
    let command = cursor.read_field("COMMAND")?;
    let working_directory = cursor.read_field("WORKING_DIRECTORY")?;
    let reason = cursor.read_field("REASON")?;
    let risk = risk_from_str(&cursor.read_field("RISK")?)?;
    let requires_approval = parse_bool(&cursor.read_field("REQUIRES_APPROVAL")?)?;
    let blocked = parse_bool(&cursor.read_field("BLOCKED")?)?;
    let expected_effects = cursor.read_field("EXPECTED_EFFECTS")?;
    let may_use_network = parse_bool(&cursor.read_field("MAY_USE_NETWORK")?)?;
    let reasons = cursor
        .read_field("REASONS")?
        .lines()
        .map(|line| line.to_string())
        .collect();
    let created_at_ms = cursor
        .read_field("CREATED_AT_MS")?
        .parse()
        .map_err(|_| ClientError::InvalidInput("Invalid command proposal timestamp".to_string()))?;
    let status = cursor.read_field("STATUS")?;
    // Every proposal stored before spec 24 ran at the repository root.
    let repository_root = if cursor.next_field_is("REPOSITORY_ROOT") {
        cursor.read_field("REPOSITORY_ROOT")?
    } else {
        working_directory.clone()
    };
    let purpose = if cursor.next_field_is("PURPOSE") {
        CommandPurpose::from_stored(&cursor.read_field("PURPOSE")?)
    } else {
        CommandPurpose::Other
    };
    let causes = if cursor.next_field_is("CAUSES") {
        serde_json::from_str(&cursor.read_field("CAUSES")?)
            .map_err(|_| ClientError::InvalidInput("Invalid command proposal causes".to_string()))?
    } else {
        Vec::new()
    };
    let rule = if cursor.next_field_is("RULE") {
        serde_json::from_str(&cursor.read_field("RULE")?)
            .map_err(|_| ClientError::InvalidInput("Invalid command proposal rule".to_string()))?
    } else {
        None
    };
    Ok(CommandProposal {
        id,
        command,
        working_directory,
        repository_root,
        reason,
        risk,
        requires_approval,
        blocked,
        expected_effects,
        may_use_network,
        reasons,
        causes,
        rule,
        created_at_ms,
        status,
        purpose,
    })
}

fn serialize_execution_summary(
    proposal: &CommandProposal,
    execution: &CommandExecution,
    stdout_ref: &Path,
    stderr_ref: &Path,
) -> String {
    let mut output = String::new();
    output.push_str("DAMAIAN_COMMAND_EXECUTION_V1\n");
    write_field(&mut output, "PROPOSAL_ID", &proposal.id);
    write_field(&mut output, "COMMAND_ID", &execution.id);
    write_field(&mut output, "COMMAND", &execution.command);
    write_field(
        &mut output,
        "WORKING_DIRECTORY",
        &execution.working_directory,
    );
    write_field(&mut output, "RISK", execution.risk.as_str());
    write_field(
        &mut output,
        "APPROVED_BY",
        execution.approved_by.as_deref().unwrap_or_default(),
    );
    write_field(
        &mut output,
        "STARTED_AT_MS",
        &execution.started_at_ms.to_string(),
    );
    write_field(
        &mut output,
        "COMPLETED_AT_MS",
        &execution.completed_at_ms.to_string(),
    );
    write_field(
        &mut output,
        "EXIT_CODE",
        &execution.exit_code.unwrap_or(-1).to_string(),
    );
    write_field(&mut output, "TERMINATION", execution.termination.as_str());
    write_field(&mut output, "STDOUT_REF", &stdout_ref.to_string_lossy());
    write_field(&mut output, "STDERR_REF", &stderr_ref.to_string_lossy());
    // Last, as on the proposal, so a summary written before spec 63 differs
    // only by lacking it, and its absence means `Other`.
    write_field(&mut output, "PURPOSE", execution.purpose.as_str());
    output.push_str("END_COMMAND_EXECUTION\n");
    output
}

fn write_field(output: &mut String, name: &str, value: &str) {
    output.push_str(&format!("{name} {}\n", value.len()));
    output.push_str(value);
    output.push('\n');
}

fn risk_from_str(value: &str) -> Result<CommandRisk> {
    match value {
        "low" => Ok(CommandRisk::Low),
        "medium" => Ok(CommandRisk::Medium),
        "high" => Ok(CommandRisk::High),
        "blocked" => Ok(CommandRisk::Blocked),
        _ => Err(ClientError::InvalidInput(format!(
            "Invalid command risk: {value}"
        ))),
    }
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ClientError::InvalidInput(format!(
            "Invalid boolean value: {value}"
        ))),
    }
}

struct Cursor<'a> {
    raw: &'a str,
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(raw: &'a str) -> Self {
        Self { raw, position: 0 }
    }

    fn expect_line(&mut self, expected: &str) -> Result<()> {
        let line = self.read_line()?;
        if line == expected {
            Ok(())
        } else {
            Err(ClientError::InvalidInput(format!(
                "Expected {expected}, found {line}"
            )))
        }
    }

    fn read_field(&mut self, name: &str) -> Result<String> {
        let header = self.read_line()?;
        let expected_prefix = format!("{name} ");
        let Some(length_text) = header.strip_prefix(&expected_prefix) else {
            return Err(ClientError::InvalidInput(format!(
                "Expected field {name}, found {header}"
            )));
        };
        let length: usize = length_text
            .parse()
            .map_err(|_| ClientError::InvalidInput(format!("Invalid length for {name}")))?;
        if self.position + length > self.raw.len() {
            return Err(ClientError::InvalidInput(format!(
                "Stored command field {name} is truncated"
            )));
        }
        let value = self.raw[self.position..self.position + length].to_string();
        self.position += length;
        if self.raw.as_bytes().get(self.position) == Some(&b'\n') {
            self.position += 1;
        }
        Ok(value)
    }

    fn next_field_is(&self, name: &str) -> bool {
        self.raw[self.position..].starts_with(&format!("{name} "))
    }

    fn read_line(&mut self) -> Result<String> {
        if self.position >= self.raw.len() {
            return Err(ClientError::InvalidInput(
                "Unexpected end of stored command".to_string(),
            ));
        }
        let rest = &self.raw[self.position..];
        let Some(offset) = rest.find('\n') else {
            return Err(ClientError::InvalidInput(
                "Stored command line is missing newline".to_string(),
            ));
        };
        let line = &rest[..offset];
        self.position += offset + 1;
        Ok(line.to_string())
    }
}
