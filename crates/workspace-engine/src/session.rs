use crate::audit::escape_json;
use crate::error::Result;
use crate::hash::{create_id, now_millis};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub repository_id: String,
    pub title: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Created,
    Running,
    WaitingForApproval,
    Failed,
    Complete,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::WaitingForApproval => "waiting_for_approval",
            Self::Failed => "failed",
            Self::Complete => "complete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub session_id: String,
    pub status: TaskStatus,
    pub user_prompt: String,
    pub model_provider: String,
    pub model_name: String,
    pub created_at_ms: u128,
    pub completed_at_ms: Option<u128>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at_ms: u128,
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    data_dir: PathBuf,
}

impl SessionStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    pub fn create_session(&self, repository_id: &str, title: &str) -> Result<Session> {
        let now = now_millis();
        let session = Session {
            id: create_id("session"),
            repository_id: repository_id.to_string(),
            title: title.to_string(),
            created_at_ms: now,
            updated_at_ms: now,
            summary: String::new(),
        };
        self.append_session_event(&session.id, "session_created", &session_json(&session))?;
        Ok(session)
    }

    pub fn create_task(
        &self,
        session_id: &str,
        user_prompt: &str,
        model_provider: &str,
        model_name: &str,
    ) -> Result<Task> {
        let task = Task {
            id: create_id("task"),
            session_id: session_id.to_string(),
            status: TaskStatus::Created,
            user_prompt: user_prompt.to_string(),
            model_provider: model_provider.to_string(),
            model_name: model_name.to_string(),
            created_at_ms: now_millis(),
            completed_at_ms: None,
        };
        self.append_session_event(session_id, "task_created", &task_json(&task))?;
        Ok(task)
    }

    pub fn update_task_status(
        &self,
        task: &Task,
        status: TaskStatus,
        error: Option<&str>,
    ) -> Result<Task> {
        let mut updated = task.clone();
        updated.status = status;
        if matches!(updated.status, TaskStatus::Complete | TaskStatus::Failed) {
            updated.completed_at_ms = Some(now_millis());
        }
        let mut payload = task_json(&updated);
        if let Some(error) = error {
            payload = format!(
                "{{\"task\":{},\"error\":\"{}\"}}",
                payload,
                escape_json(error)
            );
        }
        self.append_session_event(&task.session_id, "task_status_updated", &payload)?;
        Ok(updated)
    }

    pub fn append_message(
        &self,
        session_id: &str,
        task_id: Option<&str>,
        role: &str,
        content: &str,
    ) -> Result<ChatMessage> {
        let message = ChatMessage {
            id: create_id("msg"),
            session_id: session_id.to_string(),
            task_id: task_id.map(|value| value.to_string()),
            role: role.to_string(),
            content: content.to_string(),
            created_at_ms: now_millis(),
        };
        self.append_session_event(session_id, "message_appended", &message_json(&message))?;
        Ok(message)
    }

    pub fn read_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(Vec::new());
        };
        Ok(content
            .lines()
            .filter(|line| line.contains("\"eventType\":\"message_appended\""))
            .filter_map(parse_message_event)
            .collect())
    }

    fn append_session_event(
        &self,
        session_id: &str,
        event_type: &str,
        payload: &str,
    ) -> Result<()> {
        let path = self.session_log_path(session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(
            file,
            "{{\"eventId\":\"{}\",\"timestampMs\":{},\"eventType\":\"{}\",\"payload\":{}}}",
            create_id("evt"),
            now_millis(),
            escape_json(event_type),
            payload
        )?;
        Ok(())
    }

    fn session_log_path(&self, session_id: &str) -> PathBuf {
        self.data_dir
            .join("sessions")
            .join(format!("{session_id}.jsonl"))
    }
}

fn session_json(session: &Session) -> String {
    format!(
        "{{\"id\":\"{}\",\"repositoryId\":\"{}\",\"title\":\"{}\",\"createdAtMs\":{},\"updatedAtMs\":{},\"summary\":\"{}\"}}",
        escape_json(&session.id),
        escape_json(&session.repository_id),
        escape_json(&session.title),
        session.created_at_ms,
        session.updated_at_ms,
        escape_json(&session.summary)
    )
}

fn task_json(task: &Task) -> String {
    format!(
        "{{\"id\":\"{}\",\"sessionId\":\"{}\",\"status\":\"{}\",\"userPrompt\":\"{}\",\"modelProvider\":\"{}\",\"modelName\":\"{}\",\"createdAtMs\":{},\"completedAtMs\":{}}}",
        escape_json(&task.id),
        escape_json(&task.session_id),
        task.status.as_str(),
        escape_json(&task.user_prompt),
        escape_json(&task.model_provider),
        escape_json(&task.model_name),
        task.created_at_ms,
        task.completed_at_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string())
    )
}

fn message_json(message: &ChatMessage) -> String {
    format!(
        "{{\"id\":\"{}\",\"sessionId\":\"{}\",\"taskId\":{},\"role\":\"{}\",\"content\":\"{}\",\"createdAtMs\":{}}}",
        escape_json(&message.id),
        escape_json(&message.session_id),
        message
            .task_id
            .as_ref()
            .map(|value| format!("\"{}\"", escape_json(value)))
            .unwrap_or_else(|| "null".to_string()),
        escape_json(&message.role),
        escape_json(&message.content),
        message.created_at_ms
    )
}

fn parse_message_event(line: &str) -> Option<ChatMessage> {
    let payload_start = line.find("\"payload\":")? + "\"payload\":".len();
    let payload = &line[payload_start..line.len().checked_sub(1)?];
    Some(ChatMessage {
        id: json_string_field(payload, "id")?,
        session_id: json_string_field(payload, "sessionId")?,
        task_id: json_nullable_string_field(payload, "taskId"),
        role: json_string_field(payload, "role")?,
        content: json_string_field(payload, "content")?,
        created_at_ms: json_number_field(payload, "createdAtMs")?,
    })
}

fn json_string_field(raw: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\":\"");
    let start = raw.find(&needle)? + needle.len();
    parse_json_string_at(raw, start)
}

fn json_nullable_string_field(raw: &str, field: &str) -> Option<String> {
    let string_needle = format!("\"{field}\":\"");
    if let Some(start) = raw
        .find(&string_needle)
        .map(|index| index + string_needle.len())
    {
        return parse_json_string_at(raw, start);
    }
    None
}

fn json_number_field(raw: &str, field: &str) -> Option<u128> {
    let needle = format!("\"{field}\":");
    let start = raw.find(&needle)? + needle.len();
    let end = raw[start..]
        .find(|character: char| !character.is_ascii_digit())
        .map(|offset| start + offset)
        .unwrap_or(raw.len());
    raw[start..end].parse().ok()
}

fn parse_json_string_at(raw: &str, start: usize) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut output = String::new();
    let mut index = start;
    let mut segment_start = index;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                output.push_str(raw.get(segment_start..index)?);
                return Some(output);
            }
            b'\\' => {
                output.push_str(raw.get(segment_start..index)?);
                index += 1;
                let escaped = *bytes.get(index)?;
                match escaped {
                    b'"' => output.push('"'),
                    b'\\' => output.push('\\'),
                    b'n' => output.push('\n'),
                    b'r' => output.push('\r'),
                    b't' => output.push('\t'),
                    other => output.push(other as char),
                }
                index += 1;
                segment_start = index;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    None
}
