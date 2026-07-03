use crate::audit::AuditLog;
use crate::config::Config;
use crate::context_manager::{ContextItem, ContextManager};
use crate::error::{ClientError, Result};
use crate::hash::create_id;
use crate::indexer::ProjectIndexer;
use crate::model::{ModelAdapter, ModelMessage, ModelRequest, ModelRun};
use crate::patch_engine::{PatchApplyResult, PatchEngine, ProposedChange, ProposedPatch};
use crate::secret_scanner::SecretScanner;
use crate::session::{Session, SessionStore, Task, TaskStatus};
use std::fs;
use std::path::{Path, PathBuf};

const EDIT_FORMAT_HEADER: &str = "DAMAIAN_EDIT_V1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedEdit {
    pub summary: String,
    pub changes: Vec<ProposedChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditProposalResult {
    pub session: Session,
    pub task: Task,
    pub model_run: ModelRun,
    pub patch: ProposedPatch,
    pub context_files: Vec<String>,
    pub raw_model_output: String,
}

#[derive(Debug, Clone)]
pub struct PatchStore {
    data_dir: PathBuf,
}

impl PatchStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    pub fn save(&self, patch: &ProposedPatch) -> Result<PathBuf> {
        let path = self.path_for(&patch.id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serialize_patch(patch))?;
        Ok(path)
    }

    pub fn load(&self, patch_id: &str) -> Result<ProposedPatch> {
        let path = self.path_for(patch_id);
        let content = fs::read_to_string(path)?;
        deserialize_patch(&content)
    }

    pub fn mark_rejected(&self, patch_id: &str, rejected_by: &str) -> Result<PathBuf> {
        let source = self.path_for(patch_id);
        let rejected = self
            .data_dir
            .join("patches")
            .join("rejected")
            .join(format!("{patch_id}.dpatch"));
        if let Some(parent) = rejected.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut content = fs::read_to_string(&source)?;
        content.push_str(&format!(
            "\nREJECTED_BY {}\n{}\n",
            rejected_by.len(),
            rejected_by
        ));
        fs::write(&rejected, content)?;
        Ok(rejected)
    }

    fn path_for(&self, patch_id: &str) -> PathBuf {
        self.data_dir
            .join("patches")
            .join("pending")
            .join(format!("{patch_id}.dpatch"))
    }
}

#[derive(Debug, Clone)]
pub struct EditOrchestrator {
    config: Config,
    scanner: SecretScanner,
    audit_log: AuditLog,
    indexer: ProjectIndexer,
    context_manager: ContextManager,
    session_store: SessionStore,
    patch_engine: PatchEngine,
    patch_store: PatchStore,
}

impl EditOrchestrator {
    pub fn new(
        config: Config,
        scanner: SecretScanner,
        audit_log: AuditLog,
        indexer: ProjectIndexer,
        context_manager: ContextManager,
        session_store: SessionStore,
        patch_engine: PatchEngine,
        patch_store: PatchStore,
    ) -> Self {
        Self {
            config,
            scanner,
            audit_log,
            indexer,
            context_manager,
            session_store,
            patch_engine,
            patch_store,
        }
    }

    pub fn propose_edit(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        model_adapter: &mut dyn ModelAdapter,
    ) -> Result<EditProposalResult> {
        let index = self.indexer.index_repository(&repository_root)?;
        let session = self
            .session_store
            .create_session(&index.repository_id, &edit_session_title(prompt))?;
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
        let request = ModelRequest {
            provider: self.config.model_provider.clone(),
            model: self.config.model_name.clone(),
            messages: vec![
                ModelMessage::system(edit_system_prompt()),
                ModelMessage::user(build_edit_prompt(prompt, &context.items)),
            ],
            temperature: Some("0".to_string()),
            stream: true,
        };

        self.audit_log.record(
            "edit_model_request_prepared",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("repositoryId", index.repository_id.clone()),
                ("contextFiles", context.files.join(",")),
                ("tokenEstimate", context.token_estimate.to_string()),
            ],
        )?;

        let mut sink = |_token: &str| {};
        let run = match model_adapter.stream_response(&request, &mut sink) {
            Ok(run) => run,
            Err(error) => {
                let _ = self.session_store.update_task_status(
                    &task,
                    TaskStatus::Failed,
                    Some(&error.to_string()),
                );
                return Err(error);
            }
        };
        let raw_output = self.scanner.redact(&run.content).text;
        let generated = parse_generated_edit(&raw_output)?;
        let patch = self.patch_engine.create_patch(
            repository_root,
            &generated.changes,
            Some(&task.id),
            &generated.summary,
        )?;
        self.patch_store.save(&patch)?;
        self.session_store
            .append_message(&session.id, Some(&task.id), "assistant", &raw_output)?;
        task =
            self.session_store
                .update_task_status(&task, TaskStatus::WaitingForApproval, None)?;
        self.audit_log.record(
            "edit_patch_ready_for_approval",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("patchId", patch.id.clone()),
                (
                    "files",
                    patch
                        .files
                        .iter()
                        .map(|file| file.path.clone())
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                ("summary", patch.summary.clone()),
            ],
        )?;

        Ok(EditProposalResult {
            session,
            task,
            model_run: run,
            patch,
            context_files: context.files,
            raw_model_output: raw_output,
        })
    }

    pub fn apply_stored_patch(
        &self,
        repository_root: impl AsRef<Path>,
        patch_id: &str,
        approved_paths: Option<&[String]>,
        approved_by: &str,
    ) -> Result<PatchApplyResult> {
        let patch = self.patch_store.load(patch_id)?;
        let result = self.patch_engine.apply_patch(
            repository_root,
            &patch,
            approved_paths,
            approved_by,
            false,
        )?;
        self.audit_log.record(
            "stored_patch_applied",
            &[
                ("actor", "system".to_string()),
                ("patchId", patch_id.to_string()),
                ("approvedBy", approved_by.to_string()),
                ("files", result.applied_files.join(",")),
            ],
        )?;
        Ok(result)
    }

    pub fn reject_stored_patch(&self, patch_id: &str, rejected_by: &str) -> Result<PathBuf> {
        let path = self.patch_store.mark_rejected(patch_id, rejected_by)?;
        self.audit_log.record(
            "stored_patch_rejected",
            &[
                ("actor", "user".to_string()),
                ("patchId", patch_id.to_string()),
                ("rejectedBy", rejected_by.to_string()),
                ("resourcePath", path.to_string_lossy().to_string()),
            ],
        )?;
        Ok(path)
    }
}

pub fn parse_generated_edit(raw: &str) -> Result<GeneratedEdit> {
    let mut lines = raw.lines().peekable();
    let mut saw_header = false;
    let mut summary = String::new();
    let mut changes = Vec::new();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == EDIT_FORMAT_HEADER {
            saw_header = true;
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("SUMMARY:") {
            summary = value.trim().to_string();
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("FILE:") {
            let path = value.trim().to_string();
            if path.is_empty() {
                return Err(ClientError::InvalidInput(
                    "Generated edit contains an empty file path".to_string(),
                ));
            }
            let mut status: Option<String> = None;
            let mut content = String::new();
            let mut in_content = false;

            for file_line in lines.by_ref() {
                let file_trimmed = file_line.trim();
                if file_trimmed == "END_FILE" {
                    break;
                }
                if in_content {
                    content.push_str(file_line);
                    content.push('\n');
                    continue;
                }
                if let Some(value) = file_trimmed.strip_prefix("STATUS:") {
                    status = Some(value.trim().to_string());
                    continue;
                }
                if file_trimmed == "CONTENT:" {
                    in_content = true;
                    continue;
                }
            }

            changes.push(ProposedChange {
                path,
                new_content: content,
                status,
                allow_restricted: false,
            });
            continue;
        }
        if trimmed == "END_PATCH" {
            break;
        }
    }

    if !saw_header {
        return Err(ClientError::InvalidInput(format!(
            "Generated edit must start with {EDIT_FORMAT_HEADER}"
        )));
    }
    if changes.is_empty() {
        return Err(ClientError::InvalidInput(
            "Generated edit did not contain any file changes".to_string(),
        ));
    }
    if summary.is_empty() {
        summary = "Proposed model edit".to_string();
    }
    Ok(GeneratedEdit { summary, changes })
}

pub fn patch_diff_text(patch: &ProposedPatch) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "Patch: {}\nSummary: {}\n\n",
        patch.id, patch.summary
    ));
    for file in &patch.files {
        output.push_str(&format!("File: {} ({})\n", file.path, file.status));
        output.push_str(&file.diff);
        if !file.diff.ends_with('\n') {
            output.push('\n');
        }
        output.push('\n');
    }
    output
}

fn edit_system_prompt() -> String {
    format!(
        "You are a coding assistant that proposes edits only. Return exactly this format:\n{EDIT_FORMAT_HEADER}\nSUMMARY: short summary\nFILE: relative/path.ext\nSTATUS: modified\nCONTENT:\nfull replacement file content\nEND_FILE\nEND_PATCH\nDo not include Markdown fences. Do not include secrets. Use repository-relative paths only."
    )
}

fn build_edit_prompt(prompt: &str, items: &[ContextItem]) -> String {
    let mut output = String::new();
    output.push_str("User edit request:\n");
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

fn edit_session_title(prompt: &str) -> String {
    let title = prompt
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        format!("Edit {}", create_id("edit_title"))
    } else {
        title
    }
}

fn serialize_patch(patch: &ProposedPatch) -> String {
    let mut output = String::new();
    output.push_str("DAMAIAN_STORED_PATCH_V1\n");
    write_field(&mut output, "PATCH_ID", &patch.id);
    write_field(
        &mut output,
        "TASK_ID",
        patch.task_id.as_deref().unwrap_or_default(),
    );
    write_field(&mut output, "SUMMARY", &patch.summary);
    write_field(&mut output, "STATUS", &patch.status);
    write_field(
        &mut output,
        "CREATED_AT_MS",
        &patch.created_at_ms.to_string(),
    );
    write_field(&mut output, "FILE_COUNT", &patch.files.len().to_string());
    for file in &patch.files {
        output.push_str("FILE\n");
        write_field(&mut output, "PATH", &file.path);
        write_field(&mut output, "FILE_STATUS", &file.status);
        write_field(
            &mut output,
            "BASE_HASH",
            file.base_hash.as_deref().unwrap_or_default(),
        );
        write_field(&mut output, "NEW_HASH", &file.new_hash);
        write_field(&mut output, "NEW_CONTENT", &file.new_content);
        write_field(&mut output, "DIFF", &file.diff);
        output.push_str("END_FILE\n");
    }
    output.push_str("END_PATCH\n");
    output
}

fn deserialize_patch(raw: &str) -> Result<ProposedPatch> {
    let mut cursor = Cursor::new(raw);
    cursor.expect_line("DAMAIAN_STORED_PATCH_V1")?;
    let id = cursor.read_field("PATCH_ID")?;
    let task_id = empty_to_none(cursor.read_field("TASK_ID")?);
    let summary = cursor.read_field("SUMMARY")?;
    let status = cursor.read_field("STATUS")?;
    let created_at_ms = cursor
        .read_field("CREATED_AT_MS")?
        .parse()
        .map_err(|_| ClientError::InvalidInput("Stored patch has invalid timestamp".to_string()))?;
    let file_count: usize = cursor.read_field("FILE_COUNT")?.parse().map_err(|_| {
        ClientError::InvalidInput("Stored patch has invalid file count".to_string())
    })?;
    let mut files = Vec::new();
    for _ in 0..file_count {
        cursor.expect_line("FILE")?;
        let path = cursor.read_field("PATH")?;
        let status = cursor.read_field("FILE_STATUS")?;
        let base_hash = empty_to_none(cursor.read_field("BASE_HASH")?);
        let new_hash = cursor.read_field("NEW_HASH")?;
        let new_content = cursor.read_field("NEW_CONTENT")?;
        let diff = cursor.read_field("DIFF")?;
        cursor.expect_line("END_FILE")?;
        files.push(crate::patch_engine::ProposedFilePatch {
            path,
            status,
            base_hash,
            new_content,
            new_hash,
            diff,
        });
    }
    Ok(ProposedPatch {
        id,
        task_id,
        summary,
        status,
        created_at_ms,
        files,
    })
}

fn write_field(output: &mut String, name: &str, value: &str) {
    output.push_str(&format!("{name} {}\n", value.len()));
    output.push_str(value);
    output.push('\n');
}

fn empty_to_none(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
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
                "Stored field {name} is truncated"
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
                "Unexpected end of stored patch".to_string(),
            ));
        }
        let rest = &self.raw[self.position..];
        let Some(offset) = rest.find('\n') else {
            return Err(ClientError::InvalidInput(
                "Stored patch line is missing newline".to_string(),
            ));
        };
        let line = &rest[..offset];
        self.position += offset + 1;
        Ok(line.to_string())
    }
}
