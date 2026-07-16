use crate::audit::AuditLog;
use crate::command_runner::CommandExecution;
use crate::config::Config;
use crate::context_manager::ContextManager;
use crate::error::{ClientError, Result};
use crate::hash::create_id;
use crate::indexer::ProjectIndexer;
use crate::model::{ModelAdapter, ModelMessage, ModelRequest, ModelRun, ToolCall, ToolDefinition};
use crate::secret_scanner::SecretScanner;
use crate::session::{ChatMessage, Session, SessionStore, Task, TaskStatus};
use crate::validation::{
    CommandProposal, CommandStore, ValidationOrchestrator, command_approval_prompt,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommandProposal {
    pub id: String,
    pub command: String,
    pub prompt: String,
    pub risk: String,
    pub requires_approval: bool,
    pub blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurnResult {
    pub session: Session,
    pub task: Task,
    pub model_run: ModelRun,
    pub context_files: Vec<String>,
    pub response: String,
    pub command_proposal: Option<AgentCommandProposal>,
}

#[derive(Debug, Clone)]
pub struct ChatOrchestrator {
    config: Config,
    scanner: SecretScanner,
    audit_log: AuditLog,
    indexer: ProjectIndexer,
    context_manager: ContextManager,
    session_store: SessionStore,
    validation_orchestrator: ValidationOrchestrator,
    command_store: CommandStore,
    pending_commands: PendingCommandStore,
}

impl ChatOrchestrator {
    pub fn new(
        config: Config,
        scanner: SecretScanner,
        audit_log: AuditLog,
        indexer: ProjectIndexer,
        context_manager: ContextManager,
        session_store: SessionStore,
        validation_orchestrator: ValidationOrchestrator,
        command_store: CommandStore,
    ) -> Self {
        let pending_commands = PendingCommandStore::new(&config.data_dir);
        Self {
            config,
            scanner,
            audit_log,
            indexer,
            context_manager,
            session_store,
            validation_orchestrator,
            command_store,
            pending_commands,
        }
    }

    pub fn ask(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        model_adapter: &mut dyn ModelAdapter,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ChatTurnResult> {
        self.ask_with_session(
            repository_root,
            prompt,
            explicit_paths,
            None,
            model_adapter,
            on_token,
        )
    }

    pub fn ask_with_session(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        session_id: Option<&str>,
        model_adapter: &mut dyn ModelAdapter,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ChatTurnResult> {
        let repository_root = repository_root.as_ref();
        let index = crate::index_cache::IndexCache::get_or_build(&self.indexer, repository_root)?;
        let (session, prior_messages) = if let Some(session_id) = session_id {
            let Some(session) = self.session_store.read_session(session_id)? else {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown session: {session_id}"
                )));
            };
            if session.repository_id != index.repository_id {
                return Err(ClientError::AccessDenied(
                    "Session belongs to a different repository".to_string(),
                ));
            }
            let messages = self.session_store.read_messages(&session.id)?;
            (session, messages)
        } else {
            (
                self.session_store
                    .create_session(&index.repository_id, &session_title(prompt))?,
                Vec::new(),
            )
        };
        let mut task = self.session_store.create_task(
            &session.id,
            prompt,
            &self.config.model_provider,
            &self.config.model_name,
        )?;
        task = self
            .session_store
            .update_task_status(&task, TaskStatus::Running, None)?;
        self.session_store
            .append_message(&session.id, Some(&task.id), "user", prompt)?;

        let context = self.context_manager.build_context(
            repository_root,
            &index.repository_id,
            &task.id,
            prompt,
            Some(&index),
            explicit_paths,
            16_000,
        );
        let model_prompt = build_model_prompt(prompt, &context.items, &prior_messages, None);
        let messages = vec![
            ModelMessage::system(system_prompt()),
            ModelMessage::user(model_prompt),
        ];

        self.run_agentic_turn(
            repository_root,
            session,
            task,
            context.files,
            messages,
            0,
            model_adapter,
            on_token,
        )
    }

    /// Continues a chat turn that stopped to ask the user whether a proposed
    /// command may run. Executes (or rejects) the command, feeds the result
    /// back to the model, and lets the agentic loop keep going from there —
    /// previously approving a risky command just ran it in isolation and the
    /// model never got to use the result to answer the user's question.
    pub fn resume_after_command_decision(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(proposal_id)?;
        let repository_root = PathBuf::from(&pending.repository_root);
        let mut messages = pending.messages;
        let proposal = self.command_store.load_proposal(proposal_id)?;
        let command_request = CommandRequest {
            command: proposal.command.clone(),
            reason: proposal.reason.clone(),
        };

        let tool_result_content = if approved {
            let record = self
                .validation_orchestrator
                .run_proposal(proposal_id, true, approved_by)?;
            sandbox_command_context(&record.execution)
        } else {
            self.validation_orchestrator
                .reject_proposal(proposal_id, approved_by)?;
            format!(
                "The user declined to run `{}`. Do not request it again; answer using what you already know, noting the limitation if it matters.",
                command_request.command
            )
        };

        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "assistant",
            &tool_call_summary(&command_request),
        )?;
        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "tool",
            &tool_result_content,
        )?;

        if let Some(call) = &pending.matched_tool_call {
            messages.push(ModelMessage::assistant_with_tool_calls(
                pending.last_content.clone(),
                vec![call.clone()],
            ));
            messages.push(ModelMessage::tool(call.id.clone(), tool_result_content));
        } else {
            messages.push(ModelMessage::assistant(pending.last_content.clone()));
            messages.push(ModelMessage::user(format!(
                "Command result:\n{tool_result_content}"
            )));
        }

        let task =
            self.session_store
                .update_task_status(&pending.task, TaskStatus::Running, None)?;

        self.run_agentic_turn(
            &repository_root,
            pending.session,
            task,
            pending.context_files,
            messages,
            pending.round + 1,
            model_adapter,
            on_token,
        )
    }

    /// Whether `proposal_id` was raised by a chat turn (and so should be
    /// resumed via [`Self::resume_after_command_decision`]) as opposed to a
    /// standalone command proposal from outside the chat flow.
    pub fn has_pending_chat_command(&self, proposal_id: &str) -> bool {
        self.pending_commands.has(proposal_id)
    }

    /// Runs the model in a loop, letting it request sandboxed commands
    /// across multiple rounds (e.g. `git log` followed by `git show <sha>`)
    /// instead of stopping after one. Bounded by `MAX_TOOL_ROUNDS` so a
    /// provider that never stops requesting commands can't run forever; the
    /// final round always drops `tools`, forcing a plain answer. If the
    /// model proposes a command that needs human approval, the in-flight
    /// conversation state is persisted so the turn can be resumed later via
    /// [`Self::resume_after_command_decision`].
    fn run_agentic_turn(
        &self,
        repository_root: &Path,
        session: Session,
        mut task: Task,
        context_files: Vec<String>,
        mut messages: Vec<ModelMessage>,
        mut round: u32,
        model_adapter: &mut dyn ModelAdapter,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ChatTurnResult> {
        let native_tools = self
            .config
            .supports_native_tools()
            .then(|| vec![run_command_tool_definition()]);

        let (final_run, response, command_proposal) = loop {
            let force_final = round >= MAX_TOOL_ROUNDS;
            let tools = if force_final {
                None
            } else {
                native_tools.clone()
            };
            let request = ModelRequest {
                provider: self.config.model_provider.clone(),
                model: self.config.model_name.clone(),
                messages: messages.clone(),
                temperature: Some("0".to_string()),
                reasoning_level: Some(self.config.model_reasoning_level.clone()),
                stream: true,
                tools,
            };

            let token_estimate: usize = messages
                .iter()
                .map(|message| message.content.len())
                .sum::<usize>()
                .div_ceil(4);
            self.audit_log.record(
                "model_request_prepared",
                &[
                    ("actor", "system".to_string()),
                    ("sessionId", session.id.clone()),
                    ("taskId", task.id.clone()),
                    ("repositoryId", session.repository_id.clone()),
                    ("contextFiles", context_files.join(",")),
                    ("tokenEstimate", token_estimate.to_string()),
                    ("toolRound", round.to_string()),
                ],
            )?;

            let model_run = match model_adapter.stream_response(&request, on_token) {
                Ok(model_run) => model_run,
                Err(error) => {
                    let _ = self.session_store.update_task_status(
                        &task,
                        TaskStatus::Failed,
                        Some(&error.to_string()),
                    );
                    return Err(error);
                }
            };
            let redacted = self.scanner.redact(&model_run.content).text;

            if force_final {
                break (model_run, redacted, None);
            }

            let matched_tool_call: Option<ToolCall> = model_run
                .tool_calls
                .iter()
                .find(|call| command_request_from_tool_call(call).is_some())
                .cloned();
            let command_request = matched_tool_call
                .as_ref()
                .and_then(|call| command_request_from_tool_call(call))
                .or_else(|| parse_command_request(&redacted));

            let Some(command_request) = command_request else {
                break (model_run, redacted, None);
            };

            let proposal = self.validation_orchestrator.propose_command(
                repository_root,
                &command_request.command,
                &command_request.reason,
            )?;

            if proposal.requires_approval || proposal.blocked {
                let response = command_proposal_response(&proposal);
                self.pending_commands.save(&PendingChatTurn {
                    proposal_id: proposal.id.clone(),
                    session: session.clone(),
                    task: task.clone(),
                    repository_root: repository_root.to_string_lossy().to_string(),
                    context_files: context_files.clone(),
                    round,
                    messages: messages.clone(),
                    matched_tool_call: matched_tool_call.clone(),
                    last_content: redacted.clone(),
                })?;
                let mut proposal_run = model_run;
                proposal_run.content = response.clone();
                break (
                    proposal_run,
                    response,
                    Some(agent_command_proposal(&proposal)),
                );
            }

            let record = self
                .validation_orchestrator
                .run_proposal(&proposal.id, false, "sandbox")?;
            let command_context = sandbox_command_context(&record.execution);

            // Persist the tool call and its result so later turns in this
            // session can still see that a command ran (previously this
            // context was discarded once the turn finished).
            self.session_store.append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                &tool_call_summary(&command_request),
            )?;
            self.session_store.append_message(
                &session.id,
                Some(&task.id),
                "tool",
                &command_context,
            )?;

            if let Some(call) = &matched_tool_call {
                messages.push(ModelMessage::assistant_with_tool_calls(
                    redacted.clone(),
                    vec![call.clone()],
                ));
                messages.push(ModelMessage::tool(call.id.clone(), command_context));
            } else {
                messages.push(ModelMessage::assistant(redacted.clone()));
                messages.push(ModelMessage::user(format!(
                    "Command result:\n{command_context}"
                )));
            }

            round += 1;
        };

        self.session_store
            .append_message(&session.id, Some(&task.id), "assistant", &response)?;
        let final_status = if command_proposal.is_some() {
            TaskStatus::WaitingForApproval
        } else {
            TaskStatus::Complete
        };
        task = self
            .session_store
            .update_task_status(&task, final_status, None)?;
        self.audit_log.record(
            "model_response_completed",
            &[
                ("actor", "model".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("provider", final_run.provider.clone()),
                ("model", final_run.model.clone()),
                (
                    "status",
                    if command_proposal.is_some() {
                        "command_approval_required".to_string()
                    } else if round > 0 {
                        "complete_with_sandbox_command".to_string()
                    } else {
                        "complete".to_string()
                    },
                ),
            ],
        )?;
        Ok(ChatTurnResult {
            session,
            task,
            model_run: final_run,
            context_files,
            response,
            command_proposal,
        })
    }
}

/// Conversation state saved when a chat turn pauses on a command that needs
/// human approval, so [`ChatOrchestrator::resume_after_command_decision`]
/// can pick the turn back up once the user decides.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingChatTurn {
    proposal_id: String,
    session: Session,
    task: Task,
    repository_root: String,
    context_files: Vec<String>,
    round: u32,
    messages: Vec<ModelMessage>,
    matched_tool_call: Option<ToolCall>,
    last_content: String,
}

#[derive(Debug, Clone)]
struct PendingCommandStore {
    data_dir: PathBuf,
}

impl PendingCommandStore {
    fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    fn path_for(&self, proposal_id: &str) -> PathBuf {
        self.data_dir
            .join("chat")
            .join("pending")
            .join(format!("{proposal_id}.json"))
    }

    fn has(&self, proposal_id: &str) -> bool {
        self.path_for(proposal_id).exists()
    }

    fn save(&self, pending: &PendingChatTurn) -> Result<()> {
        let path = self.path_for(&pending.proposal_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(pending).map_err(|error| {
            ClientError::InvalidInput(format!("Failed to serialize pending chat turn: {error}"))
        })?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Loads and removes the pending state — a command decision can only be
    /// resumed once.
    fn take(&self, proposal_id: &str) -> Result<PendingChatTurn> {
        let path = self.path_for(proposal_id);
        let content = fs::read_to_string(&path).map_err(|_| {
            ClientError::InvalidInput(format!(
                "No pending chat turn for proposal: {proposal_id}"
            ))
        })?;
        let pending: PendingChatTurn = serde_json::from_str(&content).map_err(|error| {
            ClientError::InvalidInput(format!("Failed to parse pending chat turn: {error}"))
        })?;
        let _ = fs::remove_file(&path);
        Ok(pending)
    }
}

/// Upper bound on how many sandboxed commands the model can chain within a
/// single user turn before it's forced to answer with what it has.
const MAX_TOOL_ROUNDS: u32 = 6;

fn system_prompt() -> String {
    "You are a local-first coding assistant. Answer using only the provided repository context when possible. Cite relevant file paths. Do not request or expose secrets.\n\nIf the user asks about current Git state, recent commits, latest changes, uncommitted changes, repository history, or another fact that requires a local command, your entire response must be exactly one command request envelope. Do not add prose before or after the envelope:\nDAMAIAN_COMMAND_V1\nCOMMAND: git log -1 --stat --oneline\nREASON: Inspect the latest commit for the user's question.\nEND_COMMAND\n\nPrefer read-only commands such as git status, git log, git show, git diff, ls, and pwd. The app will run sandbox-safe commands automatically. Commands outside the sandbox require user approval."
        .to_string()
}

fn build_model_prompt(
    prompt: &str,
    items: &[crate::context_manager::ContextItem],
    prior_messages: &[ChatMessage],
    command_context: Option<&str>,
) -> String {
    let mut output = String::new();
    let recent_messages = prior_messages
        .iter()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    if !recent_messages.is_empty() {
        output.push_str("Recent conversation:\n");
        for message in recent_messages {
            output.push_str(&message.role);
            output.push_str(": ");
            output.push_str(&truncate_for_prompt(&message.content, 2_000));
            output.push('\n');
        }
        output.push('\n');
    }
    output.push_str("User request:\n");
    output.push_str(prompt);
    output.push_str("\n\nRepository context:\n");
    for item in items {
        output.push_str("\n--- ");
        output.push_str(&item.kind);
        if let Some(path) = &item.path {
            output.push_str(": ");
            output.push_str(path);
        }
        output.push_str(" ---\n");
        output.push_str(&item.content);
        if !item.content.ends_with('\n') {
            output.push('\n');
        }
    }
    if let Some(command_context) = command_context {
        output.push_str("\n--- sandbox_command_result ---\n");
        output.push_str(command_context);
        if !command_context.ends_with('\n') {
            output.push('\n');
        }
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandRequest {
    command: String,
    reason: String,
}

/// The one tool offered to providers configured with
/// `supports_native_tools`. Mirrors the `DAMAIAN_COMMAND_V1` envelope's
/// capability (propose a sandboxed read-only command) through a real
/// `tools`/`tool_calls` contract instead of a text convention.
fn run_command_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "run_command".to_string(),
        description: "Run a read-only local shell command (e.g. git status, git log, git diff, ls, pwd) in the repository sandbox to help answer the user's question.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\",\"description\":\"The shell command to run\"},\"reason\":{\"type\":\"string\",\"description\":\"Why this command is needed\"}},\"required\":[\"command\"]}".to_string(),
    }
}

fn command_request_from_tool_call(call: &ToolCall) -> Option<CommandRequest> {
    if call.name != "run_command" {
        return None;
    }
    let arguments: serde_json::Value = serde_json::from_str(&call.arguments_json).ok()?;
    let command = arguments.get("command")?.as_str()?.trim().to_string();
    if command.is_empty() {
        return None;
    }
    let reason = arguments
        .get("reason")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Assistant requested a local command")
        .to_string();
    Some(CommandRequest { command, reason })
}

fn parse_command_request(value: &str) -> Option<CommandRequest> {
    let marker_start = value.find("DAMAIAN_COMMAND_V1")?;
    let envelope = &value[marker_start..];
    let envelope = if let Some(end_start) = envelope.find("END_COMMAND") {
        &envelope[..end_start + "END_COMMAND".len()]
    } else {
        envelope
    };
    let mut command = String::new();
    let mut reason = String::new();
    for raw_line in envelope.lines() {
        let line = raw_line.trim();
        if let Some(value) = line.strip_prefix("COMMAND:") {
            command = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("REASON:") {
            reason = value.trim().to_string();
        } else if line.trim() == "END_COMMAND" {
            break;
        }
    }
    if command.is_empty() {
        return None;
    }
    if reason.is_empty() {
        reason = "Assistant requested a local command".to_string();
    }
    Some(CommandRequest { command, reason })
}

fn command_proposal_response(proposal: &CommandProposal) -> String {
    if proposal.blocked {
        format!(
            "I cannot run `{}` in sandbox mode, and local policy blocks this command. Review or reject the command request below.",
            proposal.command
        )
    } else {
        format!(
            "I need your approval before running `{}` because it cannot run in sandbox mode.",
            proposal.command
        )
    }
}

fn agent_command_proposal(proposal: &CommandProposal) -> AgentCommandProposal {
    AgentCommandProposal {
        id: proposal.id.clone(),
        command: proposal.command.clone(),
        prompt: command_approval_prompt(proposal),
        risk: proposal.risk.as_str().to_string(),
        requires_approval: proposal.requires_approval,
        blocked: proposal.blocked,
    }
}

fn tool_call_summary(command_request: &CommandRequest) -> String {
    format!(
        "Ran `{}` — {}",
        command_request.command, command_request.reason
    )
}

fn sandbox_command_context(execution: &CommandExecution) -> String {
    let mut output = String::new();
    output.push_str("Command: ");
    output.push_str(&execution.command);
    output.push('\n');
    output.push_str("Working directory: ");
    output.push_str(&execution.working_directory);
    output.push('\n');
    output.push_str("Exit code: ");
    output.push_str(&execution.exit_code.unwrap_or(-1).to_string());
    output.push_str("\n\nSTDOUT:\n");
    output.push_str(&truncate_for_prompt(&execution.stdout, 8_000));
    output.push_str("\n\nSTDERR:\n");
    output.push_str(&truncate_for_prompt(&execution.stderr, 4_000));
    output
}

fn truncate_for_prompt(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("\n[truncated]");
    }
    output
}

fn session_title(prompt: &str) -> String {
    let title = prompt
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        format!("Chat {}", create_id("session_title"))
    } else {
        title
    }
}
