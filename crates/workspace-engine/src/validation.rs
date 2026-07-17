use crate::audit::AuditLog;
use crate::command_policy::{CommandClassification, CommandPolicy, CommandRisk};
use crate::command_runner::{CommandExecution, CommandRunner};
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandProposal {
    pub id: String,
    pub command: String,
    pub working_directory: String,
    pub reason: String,
    pub risk: CommandRisk,
    pub requires_approval: bool,
    pub blocked: bool,
    pub expected_effects: String,
    pub may_use_network: bool,
    pub reasons: Vec<String>,
    pub created_at_ms: u128,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRunRecord {
    pub proposal_id: String,
    pub execution: CommandExecution,
    pub stdout_ref: PathBuf,
    pub stderr_ref: PathBuf,
    pub summary_ref: PathBuf,
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
        fs::write(&path, serialize_proposal(proposal))?;
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
        let mut content = serialize_proposal(&proposal);
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

    pub fn propose_command(
        &self,
        working_directory: impl AsRef<Path>,
        command: &str,
        reason: &str,
    ) -> Result<CommandProposal> {
        let classification = self
            .command_policy
            .classify(command, working_directory.as_ref());
        let proposal =
            proposal_from_classification(working_directory.as_ref(), reason, classification);
        self.command_store.save_proposal(&proposal)?;
        self.audit_log.record(
            "command_proposal_stored",
            &[
                ("actor", "system".to_string()),
                ("proposalId", proposal.id.clone()),
                ("command", proposal.command.clone()),
                ("workingDirectory", proposal.working_directory.clone()),
                ("risk", proposal.risk.as_str().to_string()),
                ("requiresApproval", proposal.requires_approval.to_string()),
                ("blocked", proposal.blocked.to_string()),
            ],
        )?;
        Ok(proposal)
    }

    pub fn propose_detected_validations(
        &self,
        working_directory: impl AsRef<Path>,
    ) -> Result<Vec<CommandProposal>> {
        let commands = self
            .command_policy
            .detect_project_commands(&working_directory)?;
        commands
            .iter()
            .map(|command| {
                self.propose_command(
                    &working_directory,
                    &command.command,
                    &format!("Detected project validation command from {}", command.name),
                )
            })
            .collect()
    }

    pub fn run_proposal(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
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
        let execution = self.command_runner.run(
            &proposal.command,
            &proposal.working_directory,
            &proposal.reason,
            approved,
            Some(approved_by),
            None,
        )?;
        let record = self.command_store.save_execution(&proposal, &execution)?;
        self.audit_log.record(
            "stored_command_executed",
            &[
                ("actor", "command".to_string()),
                ("proposalId", proposal.id.clone()),
                ("commandId", execution.id.clone()),
                ("exitCode", execution.exit_code.unwrap_or(-1).to_string()),
                ("stdoutRef", record.stdout_ref.to_string_lossy().to_string()),
                ("stderrRef", record.stderr_ref.to_string_lossy().to_string()),
            ],
        )?;
        Ok(record)
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

fn proposal_from_classification(
    working_directory: &Path,
    reason: &str,
    classification: CommandClassification,
) -> CommandProposal {
    CommandProposal {
        id: create_id("cmdprop"),
        command: classification.command,
        working_directory: working_directory.to_string_lossy().to_string(),
        reason: reason.to_string(),
        risk: classification.risk,
        requires_approval: classification.requires_approval,
        blocked: classification.blocked,
        expected_effects: classification.expected_effects,
        may_use_network: classification.may_use_network,
        reasons: classification.reasons,
        created_at_ms: now_millis(),
        status: "pending".to_string(),
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

fn serialize_proposal(proposal: &CommandProposal) -> String {
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
    output.push_str("END_COMMAND_PROPOSAL\n");
    output
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
    Ok(CommandProposal {
        id,
        command,
        working_directory,
        reason,
        risk,
        requires_approval,
        blocked,
        expected_effects,
        may_use_network,
        reasons,
        created_at_ms,
        status,
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
    write_field(&mut output, "STDOUT_REF", &stdout_ref.to_string_lossy());
    write_field(&mut output, "STDERR_REF", &stderr_ref.to_string_lossy());
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
