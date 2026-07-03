use crate::audit::AuditLog;
use crate::config::Config;
use crate::context_manager::ContextManager;
use crate::error::Result;
use crate::hash::create_id;
use crate::indexer::ProjectIndexer;
use crate::model::{ModelAdapter, ModelMessage, ModelRequest, ModelRun};
use crate::secret_scanner::SecretScanner;
use crate::session::{Session, SessionStore, Task, TaskStatus};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurnResult {
    pub session: Session,
    pub task: Task,
    pub model_run: ModelRun,
    pub context_files: Vec<String>,
    pub response: String,
}

#[derive(Debug, Clone)]
pub struct ChatOrchestrator {
    config: Config,
    scanner: SecretScanner,
    audit_log: AuditLog,
    indexer: ProjectIndexer,
    context_manager: ContextManager,
    session_store: SessionStore,
}

impl ChatOrchestrator {
    pub fn new(
        config: Config,
        scanner: SecretScanner,
        audit_log: AuditLog,
        indexer: ProjectIndexer,
        context_manager: ContextManager,
        session_store: SessionStore,
    ) -> Self {
        Self {
            config,
            scanner,
            audit_log,
            indexer,
            context_manager,
            session_store,
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
        let index = self.indexer.index_repository(&repository_root)?;
        let session = self
            .session_store
            .create_session(&index.repository_id, &session_title(prompt))?;
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
            repository_root.as_ref(),
            &index.repository_id,
            &task.id,
            prompt,
            Some(&index),
            explicit_paths,
            16_000,
        );
        let model_prompt = build_model_prompt(prompt, &context.items);
        let request = ModelRequest {
            provider: self.config.model_provider.clone(),
            model: self.config.model_name.clone(),
            messages: vec![
                ModelMessage::system(system_prompt()),
                ModelMessage::user(model_prompt),
            ],
            temperature: Some("0".to_string()),
            stream: true,
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
                let response = self.scanner.redact(&model_run.content).text;
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
    "You are a local-first coding assistant. Answer using only the provided repository context when possible. Cite relevant file paths. Do not request or expose secrets.".to_string()
}

fn build_model_prompt(prompt: &str, items: &[crate::context_manager::ContextItem]) -> String {
    let mut output = String::new();
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
