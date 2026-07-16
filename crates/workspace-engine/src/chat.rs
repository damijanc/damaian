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
use crate::validation::{CommandProposal, ValidationOrchestrator, command_approval_prompt};
use std::path::Path;

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
    ) -> Self {
        Self {
            config,
            scanner,
            audit_log,
            indexer,
            context_manager,
            session_store,
            validation_orchestrator,
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
        let request = ModelRequest {
            provider: self.config.model_provider.clone(),
            model: self.config.model_name.clone(),
            messages: vec![
                ModelMessage::system(system_prompt()),
                ModelMessage::user(model_prompt),
            ],
            temperature: Some("0".to_string()),
            reasoning_level: Some(self.config.model_reasoning_level.clone()),
            stream: true,
            tools: self
                .config
                .supports_native_tools()
                .then(|| vec![run_command_tool_definition()]),
        };

        self.audit_log.record(
            "model_request_prepared",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("repositoryId", index.repository_id.clone()),
                ("contextFiles", context.files.join(",")),
                ("tokenEstimate", context.token_estimate.to_string()),
            ],
        )?;

        let run_result = model_adapter.stream_response(&request, on_token);
        match run_result {
            Ok(model_run) => {
                let first_response = self.scanner.redact(&model_run.content).text;
                let command_request = model_run
                    .tool_calls
                    .iter()
                    .find_map(command_request_from_tool_call)
                    .or_else(|| parse_command_request(&first_response));
                if let Some(command_request) = command_request {
                    let proposal = self.validation_orchestrator.propose_command(
                        repository_root,
                        &command_request.command,
                        &command_request.reason,
                    )?;
                    if proposal.requires_approval || proposal.blocked {
                        let response = command_proposal_response(&proposal);
                        self.session_store.append_message(
                            &session.id,
                            Some(&task.id),
                            "assistant",
                            &response,
                        )?;
                        task = self.session_store.update_task_status(
                            &task,
                            TaskStatus::Complete,
                            None,
                        )?;
                        let mut proposal_run = model_run;
                        proposal_run.content = response.clone();
                        self.audit_log.record(
                            "model_response_completed",
                            &[
                                ("actor", "model".to_string()),
                                ("sessionId", session.id.clone()),
                                ("taskId", task.id.clone()),
                                ("provider", proposal_run.provider.clone()),
                                ("model", proposal_run.model.clone()),
                                ("status", "command_approval_required".to_string()),
                            ],
                        )?;
                        return Ok(ChatTurnResult {
                            session,
                            task,
                            model_run: proposal_run,
                            context_files: context.files,
                            response,
                            command_proposal: Some(agent_command_proposal(&proposal)),
                        });
                    }

                    let record = self.validation_orchestrator.run_proposal(
                        &proposal.id,
                        false,
                        "sandbox",
                    )?;
                    let command_context = sandbox_command_context(&record.execution);
                    let follow_up_prompt = build_model_prompt(
                        prompt,
                        &context.items,
                        &prior_messages,
                        Some(&command_context),
                    );
                    let follow_up_request = ModelRequest {
                        provider: self.config.model_provider.clone(),
                        model: self.config.model_name.clone(),
                        messages: vec![
                            ModelMessage::system(system_prompt_after_command()),
                            ModelMessage::user(follow_up_prompt),
                        ],
                        temperature: Some("0".to_string()),
                        reasoning_level: Some(self.config.model_reasoning_level.clone()),
                        stream: true,
                        tools: None,
                    };
                    let model_run = model_adapter.stream_response(&follow_up_request, on_token)?;
                    let response = self.scanner.redact(&model_run.content).text;
                    self.session_store.append_message(
                        &session.id,
                        Some(&task.id),
                        "assistant",
                        &response,
                    )?;
                    task =
                        self.session_store
                            .update_task_status(&task, TaskStatus::Complete, None)?;
                    self.audit_log.record(
                        "model_response_completed",
                        &[
                            ("actor", "model".to_string()),
                            ("sessionId", session.id.clone()),
                            ("taskId", task.id.clone()),
                            ("provider", model_run.provider.clone()),
                            ("model", model_run.model.clone()),
                            ("status", "complete_with_sandbox_command".to_string()),
                        ],
                    )?;
                    return Ok(ChatTurnResult {
                        session,
                        task,
                        model_run,
                        context_files: context.files,
                        response,
                        command_proposal: None,
                    });
                }

                let response = first_response;
                self.session_store.append_message(
                    &session.id,
                    Some(&task.id),
                    "assistant",
                    &response,
                )?;
                task = self
                    .session_store
                    .update_task_status(&task, TaskStatus::Complete, None)?;
                self.audit_log.record(
                    "model_response_completed",
                    &[
                        ("actor", "model".to_string()),
                        ("sessionId", session.id.clone()),
                        ("taskId", task.id.clone()),
                        ("provider", model_run.provider.clone()),
                        ("model", model_run.model.clone()),
                        ("status", "complete".to_string()),
                    ],
                )?;
                Ok(ChatTurnResult {
                    session,
                    task,
                    model_run,
                    context_files: context.files,
                    response,
                    command_proposal: None,
                })
            }
            Err(error) => {
                let _ = self.session_store.update_task_status(
                    &task,
                    TaskStatus::Failed,
                    Some(&error.to_string()),
                );
                Err(error)
            }
        }
    }
}

fn system_prompt() -> String {
    "You are a local-first coding assistant. Answer using only the provided repository context when possible. Cite relevant file paths. Do not request or expose secrets.\n\nIf the user asks about current Git state, recent commits, latest changes, uncommitted changes, repository history, or another fact that requires a local command, your entire response must be exactly one command request envelope. Do not add prose before or after the envelope:\nDAMAIAN_COMMAND_V1\nCOMMAND: git log -1 --stat --oneline\nREASON: Inspect the latest commit for the user's question.\nEND_COMMAND\n\nPrefer read-only commands such as git status, git log, git show, git diff, ls, and pwd. The app will run sandbox-safe commands automatically. Commands outside the sandbox require user approval."
        .to_string()
}

fn system_prompt_after_command() -> String {
    "You are a local-first coding assistant. A sandboxed command result is included in the prompt. Answer the user's request using that result and the repository context. Do not request another command. Do not expose secrets."
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
