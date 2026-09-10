use crate::audit::escape_json;
use crate::error::Result;
use crate::hash::{create_id, now_millis};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub repository_id: String,
    pub title: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub summary: String,
}

/// What a task is doing, precisely enough that a crash in it is classifiable.
///
/// The old `Running` covered context preparation, the model call, tool
/// execution, patch application and validation indiscriminately, so a crash
/// while `Running` said nothing about whether anything had happened. See
/// `docs/specs/17_durable_task_state_and_crash_recovery/proposal.md` §5.1 for
/// the state table and what a crash in each one means.
///
/// `Interrupted` and `UnknownExternalOutcome` are never set during normal
/// operation — they are the recovery classifier's output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Created,
    PreparingContext,
    WaitingForModel,
    RunningTool,
    WaitingForApproval,
    ApplyingPatch,
    Validating,
    Complete,
    Failed,
    Cancelled,
    ToolBudgetExhausted,
    Interrupted,
    UnknownExternalOutcome,
}

impl TaskStatus {
    /// Every state, so callers can enumerate rather than list. A variant added
    /// later shows up here automatically, which is what makes the state tests
    /// fail until it has been given a terminality and a side-effect answer.
    pub fn all() -> Vec<Self> {
        vec![
            Self::Created,
            Self::PreparingContext,
            Self::WaitingForModel,
            Self::RunningTool,
            Self::WaitingForApproval,
            Self::ApplyingPatch,
            Self::Validating,
            Self::Complete,
            Self::Failed,
            Self::Cancelled,
            Self::ToolBudgetExhausted,
            Self::Interrupted,
            Self::UnknownExternalOutcome,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::PreparingContext => "preparing_context",
            Self::WaitingForModel => "waiting_for_model",
            Self::RunningTool => "running_tool",
            Self::WaitingForApproval => "waiting_for_approval",
            Self::ApplyingPatch => "applying_patch",
            Self::Validating => "validating",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::ToolBudgetExhausted => "tool_budget_exhausted",
            Self::Interrupted => "interrupted",
            Self::UnknownExternalOutcome => "unknown_external_outcome",
        }
    }

    /// Reads a stored status back.
    ///
    /// `"running"` is accepted and maps to [`Self::Interrupted`]: it is the
    /// legacy value written before this spec, and a task left in it carries
    /// exactly the information this work eliminates — something was in flight
    /// and nothing recorded what (§5.6). Nothing writes it any more.
    pub fn parse(value: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|status| status.as_str() == value)
            .or_else(|| (value == "running").then_some(Self::Interrupted))
    }

    /// Nothing further will happen to a task in this state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Complete | Self::Failed | Self::Cancelled | Self::ToolBudgetExhausted
        )
    }

    /// Whether a crash in this state may have left a side effect part-done, and
    /// so must never be automatically repeated (requirement 5).
    ///
    /// `Validating` is included even though §5.1 says validation "splits on what
    /// is being validated" — a sandbox-safe read-only command is resumable and
    /// anything else is not. That split needs the *command*, which the status
    /// alone does not carry, so the conservative answer lives here and the
    /// classifier refines it from the dangling marker's `sideEffecting` flag.
    /// Erring toward "may have a side effect" is the safe direction: the cost is
    /// a resume that was not offered, not an action silently repeated.
    pub fn may_have_side_effect_in_flight(&self) -> bool {
        matches!(
            self,
            Self::RunningTool | Self::ApplyingPatch | Self::Validating
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Which stored proposal a task is waiting on, recorded alongside the
/// `waiting_for_approval` status so the link survives a restart.
///
/// §5.5: the proposals themselves already persist; what was missing was the
/// link back to the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApprovalRef {
    /// `"command"` or `"patch"`.
    pub kind: String,
    pub proposal_id: String,
}

/// A started action, returned by [`SessionStore::start_action`] and consumed by
/// [`SessionStore::finish_action`].
///
/// There is deliberately **no `Drop` impl**. A dropped marker is exactly the
/// crash case this spec exists to detect, so finishing an action automatically
/// on drop would erase the signal. `finish_action` takes the marker by value, so
/// a forgotten finish shows up in review as a binding that goes out of scope
/// unused rather than as silence.
///
/// The marker does not carry `sideEffecting`: that is written durably onto the
/// `action_started` event, which is where the classifier reads it, and a second
/// in-memory copy would be state that could disagree with the log.
#[derive(Debug)]
pub struct ActionMarker {
    id: String,
    session_id: String,
    task_id: String,
    action: String,
    reference: String,
}

/// An action that started and never finished — the signature of a crash while
/// it was in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingAction {
    pub task_id: String,
    pub action: String,
    pub reference: String,
    /// Recorded on the *start* event, so the classifier does not have to
    /// re-derive the action's nature after the code that knew it is gone.
    pub side_effecting: bool,
    pub seq: u64,
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    data_dir: PathBuf,
    /// The last `seq` handed out per session, so an append does not have to
    /// re-read the log to find it.
    ///
    /// `Arc`, not a plain field: `SessionStore` derives `Clone` and is cloned
    /// into both the chat and the edit orchestrator
    /// (`workspace_engine.rs:97`, `:112`). Per-clone caches would each hand out
    /// the same next `seq`, and the log would carry duplicates — which rewind
    /// resolves by sequence number, so it would rewind to the wrong place.
    ///
    /// A miss falls back to reading the file, which is what keeps two
    /// independently constructed stores over one directory in agreement.
    ///
    /// Measured before this existed: appending re-read and re-scanned the whole
    /// log every time, so cost per append grew with the log — 2.3ms at 500
    /// events, 4.4ms at 1000, 8.7ms at 2000 (17.5s for the batch). Spec 17 adds
    /// two marker events per action across six action types, which is what made
    /// this worth fixing before those markers land.
    last_seq: Arc<Mutex<HashMap<String, u64>>>,
}

impl SessionStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
            last_seq: Arc::new(Mutex::new(HashMap::new())),
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
        if matches!(
            updated.status,
            TaskStatus::Complete
                | TaskStatus::Failed
                | TaskStatus::Cancelled
                | TaskStatus::ToolBudgetExhausted
        ) {
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

    pub fn list_sessions(&self, repository_id: Option<&str>) -> Result<Vec<Session>> {
        let sessions_dir = self.data_dir.join("sessions");
        let Ok(entries) = fs::read_dir(sessions_dir) else {
            return Ok(Vec::new());
        };
        let mut sessions = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let content = fs::read_to_string(entry.path())?;
            if let Some(session) = parse_session_log(&content)
                && repository_id
                    .map(|id| id == session.repository_id.as_str())
                    .unwrap_or(true)
            {
                sessions.push(session);
            }
        }
        sessions.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then(left.title.cmp(&right.title))
        });
        Ok(sessions)
    }

    pub fn read_session(&self, session_id: &str) -> Result<Option<Session>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(None);
        };
        Ok(parse_session_log(&content))
    }

    pub fn rename_session(&self, session_id: &str, title: &str) -> Result<Session> {
        let Some(mut session) = self.read_session(session_id)? else {
            return Err(crate::error::ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        };
        session.title = title.to_string();
        session.updated_at_ms = now_millis();
        self.append_session_event(session_id, "session_renamed", &session_json(&session))?;
        Ok(session)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let path = self.session_log_path(session_id);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn read_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(Vec::new());
        };
        // Rewound events stay in the log but leave the conversation, so a
        // reloaded session shows the position the user rewound to.
        Ok(active_events(&content)
            .iter()
            .filter(|event| event.event_type == "message_appended")
            .filter_map(parse_message_event)
            .collect())
    }

    /// The latest recorded status of every task in the session, keyed by task id.
    ///
    /// Tasks are not stored as records but replayed from `task_created` and
    /// `task_status_updated` events, so a later event simply overwrites an
    /// earlier one. Lets a reloaded conversation tell a stopped turn from a
    /// completed one, which the message log alone cannot express.
    pub fn read_task_statuses(&self, session_id: &str) -> Result<HashMap<String, String>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(HashMap::new());
        };
        let mut statuses = HashMap::new();
        for event in active_events(&content) {
            if event.event_type != "task_created" && event.event_type != "task_status_updated" {
                continue;
            }
            // `task_status_updated` may wrap the task as `{"task":…,"error":…}`,
            // so look inside that wrapper when it is there and at the payload
            // itself when it is not. The old reader found these fields by
            // substring across the whole line, which reached into the payload
            // without descending into it — and matched a torn line just as
            // readily as a complete one.
            let task = event.payload.get("task").unwrap_or(&event.payload);
            if let (Some(id), Some(status)) = (
                task.get("id").and_then(|value| value.as_str()),
                task.get("status").and_then(|value| value.as_str()),
            ) {
                statuses.insert(id.to_string(), status.to_string());
            }
        }
        Ok(statuses)
    }

    /// Every task in the session as a full record: the latest status, with the
    /// metadata from whichever event recorded it.
    ///
    /// [`Self::read_task_statuses`] stays the cheap path for the conversation
    /// view, which wants nothing but the status. This exists because resuming a
    /// recovered task means re-sending the prompt the user typed
    /// (`docs/specs/45_crash_recovery_prompt.md` §5.4), and that is in the log
    /// but not in a status map.
    ///
    /// **A status event is not required to carry full metadata**, which is why
    /// a later event is merged rather than substituted. Tasks are replayed from
    /// events rather than stored as records, so a writer that only wants to
    /// move the status legitimately fills the rest with blanks —
    /// `recovery::set_status` and `recovery::fail_task` both do, with only `id`
    /// and `session_id` load-bearing. Substituting wholesale would let a
    /// resumed task lose the prompt that a resume exists to re-send.
    pub fn read_tasks(&self, session_id: &str) -> Result<Vec<Task>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(Vec::new());
        };
        // Insertion-ordered by first appearance, so the caller sees tasks in
        // the order the session created them.
        let mut order: Vec<String> = Vec::new();
        let mut tasks: HashMap<String, Task> = HashMap::new();
        for event in active_events(&content) {
            if event.event_type != "task_created" && event.event_type != "task_status_updated" {
                continue;
            }
            // Both shapes of `task_status_updated` — flat, and wrapped as
            // `{"task":…,"error":…}` by `await_approval`.
            let payload = event.payload.get("task").unwrap_or(&event.payload);
            let Some(task) = parse_task_payload(payload) else {
                continue;
            };
            match tasks.get_mut(&task.id) {
                Some(known) => {
                    known.status = task.status;
                    known.completed_at_ms = task.completed_at_ms.or(known.completed_at_ms);
                    // Nothing ever *changes* these, so a blank means "not
                    // recorded on this event" rather than "cleared".
                    if !task.user_prompt.is_empty() {
                        known.user_prompt = task.user_prompt;
                    }
                    if !task.model_provider.is_empty() {
                        known.model_provider = task.model_provider;
                    }
                    if !task.model_name.is_empty() {
                        known.model_name = task.model_name;
                    }
                    if task.created_at_ms > 0 {
                        known.created_at_ms = task.created_at_ms;
                    }
                }
                None => {
                    order.push(task.id.clone());
                    tasks.insert(task.id.clone(), task);
                }
            }
        }
        Ok(order
            .into_iter()
            .filter_map(|id| tasks.remove(&id))
            .collect())
    }

    pub fn allow_browser_diagnostics_for_session(
        &self,
        session_id: &str,
        approved_by: &str,
    ) -> Result<()> {
        self.append_session_event(
            session_id,
            "browser_diagnostics_approval_updated",
            &format!(
                "{{\"sessionId\":\"{}\",\"allowed\":true,\"approvedBy\":\"{}\"}}",
                escape_json(session_id),
                escape_json(approved_by)
            ),
        )
    }

    pub fn browser_diagnostics_allowed_for_session(&self, session_id: &str) -> Result<bool> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(false);
        };
        let mut allowed = false;
        for event in parsed_events(&content).0 {
            if event.event_type != "browser_diagnostics_approval_updated" {
                continue;
            }
            if let Some(value) = event.payload.get("allowed").and_then(|v| v.as_bool()) {
                allowed = value;
            }
        }
        Ok(allowed)
    }

    /// Sets [`TaskStatus::WaitingForApproval`] and records which stored
    /// proposal the task is waiting on, so recovery can reattach it (§5.5).
    pub fn await_approval(&self, task: &Task, pending: &PendingApprovalRef) -> Result<Task> {
        let mut updated = task.clone();
        updated.status = TaskStatus::WaitingForApproval;
        let payload = format!(
            "{{\"task\":{},\"pendingApproval\":{{\"kind\":\"{}\",\"proposalId\":\"{}\"}}}}",
            task_json(&updated),
            escape_json(&pending.kind),
            escape_json(&pending.proposal_id)
        );
        self.append_session_event(&task.session_id, "task_status_updated", &payload)?;
        Ok(updated)
    }

    /// The proposal a task is waiting on, from the most recent status event
    /// that recorded one. `None` when the task is not awaiting approval, or
    /// when it was set by a version that did not record the link.
    pub fn pending_approval_for(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<Option<PendingApprovalRef>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(None);
        };
        let mut found = None;
        for event in active_events(&content) {
            if event.event_type != "task_status_updated" {
                continue;
            }
            let task = event.payload.get("task").unwrap_or(&event.payload);
            if task.get("id").and_then(|value| value.as_str()) != Some(task_id) {
                continue;
            }
            // A later status event without a pending approval clears an earlier
            // one: the task has moved on, and a stale link would reattach a
            // proposal the user already decided about.
            found = event.payload.get("pendingApproval").and_then(|pending| {
                Some(PendingApprovalRef {
                    kind: pending.get("kind")?.as_str()?.to_string(),
                    proposal_id: pending.get("proposalId")?.as_str()?.to_string(),
                })
            });
        }
        Ok(found)
    }

    /// Records `action_started` and returns the marker that must be finished.
    ///
    /// Requirement 2: a durable marker before a consequential action starts and
    /// another after it finishes, so a crash between the two is detectable as a
    /// specific action with an unknown outcome.
    pub fn start_action(
        &self,
        task: &Task,
        action: &str,
        reference: &str,
        side_effecting: bool,
    ) -> Result<ActionMarker> {
        let marker = ActionMarker {
            id: create_id("action"),
            session_id: task.session_id.clone(),
            task_id: task.id.clone(),
            action: action.to_string(),
            reference: reference.to_string(),
        };
        self.append_session_event(
            &marker.session_id,
            "action_started",
            &format!(
                "{{\"markerId\":\"{}\",\"taskId\":\"{}\",\"action\":\"{}\",\"ref\":\"{}\",\"sideEffecting\":{}}}",
                escape_json(&marker.id),
                escape_json(&marker.task_id),
                escape_json(&marker.action),
                escape_json(&marker.reference),
                side_effecting
            ),
        )?;
        Ok(marker)
    }

    /// Records `action_finished`, consuming the marker.
    pub fn finish_action(&self, marker: ActionMarker, outcome: &str) -> Result<()> {
        self.append_session_event(
            &marker.session_id,
            "action_finished",
            &format!(
                "{{\"markerId\":\"{}\",\"taskId\":\"{}\",\"action\":\"{}\",\"ref\":\"{}\",\"outcome\":\"{}\"}}",
                escape_json(&marker.id),
                escape_json(&marker.task_id),
                escape_json(&marker.action),
                escape_json(&marker.reference),
                escape_json(outcome)
            ),
        )
    }

    /// Every action that started and never finished, in log order.
    ///
    /// Paired by `markerId` rather than by action name: the same action can run
    /// twice in one turn, and matching by name would let the second start cancel
    /// out the first.
    ///
    /// Reads **all** events rather than only the active conversation. A rewind
    /// moves the conversation back; whether an action completed is a fact about
    /// the world, not about the conversation. Filtering by active events here
    /// would let a rewind conceal a dangling side-effecting action, which is
    /// precisely what requirement 5 exists to prevent.
    pub fn dangling_actions(&self, session_id: &str) -> Result<Vec<DanglingAction>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(Vec::new());
        };
        let (events, _) = parsed_events(&content);
        let mut started: Vec<(String, DanglingAction)> = Vec::new();
        let mut finished: Vec<String> = Vec::new();
        for event in &events {
            let Some(marker_id) = event.text("markerId") else {
                continue;
            };
            match event.event_type.as_str() {
                "action_started" => {
                    let Some(task_id) = event.text("taskId") else {
                        continue;
                    };
                    started.push((
                        marker_id,
                        DanglingAction {
                            task_id,
                            action: event.text("action").unwrap_or_default(),
                            reference: event.text("ref").unwrap_or_default(),
                            side_effecting: event
                                .payload
                                .get("sideEffecting")
                                .and_then(|value| value.as_bool())
                                .unwrap_or(true),
                            seq: event.seq,
                        },
                    ));
                }
                "action_finished" => finished.push(marker_id),
                _ => {}
            }
        }
        Ok(started
            .into_iter()
            .filter(|(id, _)| !finished.contains(id))
            .map(|(_, action)| action)
            .collect())
    }

    /// How many lines of the session log did not parse as a complete event.
    ///
    /// A non-zero count means a write was interrupted — the tail is torn — and
    /// is therefore evidence of a crash rather than a mere formatting problem.
    /// Exposed rather than audited here on purpose: `SessionStore` has no
    /// `AuditLog`, and threading one through its fifteen construction sites (of
    /// which thirteen are tests, each of which would then need a
    /// `SecretScanner`) is a large amount of churn for a diagnostic. The
    /// recovery classifier is the caller that cares, and audits it —
    /// spec 17 §5.2.
    pub fn unreadable_event_count(&self, session_id: &str) -> Result<usize> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(0);
        };
        Ok(parsed_events(&content).1)
    }

    /// The sequence number of the newest event in the session, or 0 for a
    /// session with no log yet. This is the conversation position a checkpoint
    /// records, and the point a rewind returns to.
    pub fn latest_event_seq(&self, session_id: &str) -> Result<u64> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(0);
        };
        Ok(latest_seq(&content))
    }

    /// Moves the active conversation back to `through_event_seq` by appending a
    /// marker, never by rewriting the log: tasks are replayed from these events
    /// and the log is the audit trail. Events after the marker stay on disk,
    /// stay readable, and stop counting as part of the conversation.
    pub fn rewind_conversation(&self, session_id: &str, through_event_seq: u64) -> Result<()> {
        self.append_session_event(
            session_id,
            "conversation_rewound",
            &format!(
                "{{\"throughEventSeq\":{},\"restoredAtMs\":{}}}",
                through_event_seq,
                now_millis()
            ),
        )
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
        let seq = self.next_seq(session_id)?;
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(
            file,
            "{{\"eventId\":\"{}\",\"seq\":{},\"timestampMs\":{},\"eventType\":\"{}\",\"payload\":{}}}",
            create_id("evt"),
            seq,
            now_millis(),
            escape_json(event_type),
            payload
        )?;
        Ok(())
    }

    /// The `seq` for the next event, taken from the cache when present and read
    /// back from the log when not.
    ///
    /// Holding the lock across the read is deliberate: two appends to one
    /// session must not both compute the same next value. The lock is per
    /// store-family rather than per session, which is coarse — but appends are
    /// short and the alternative is a lock per session id, which buys nothing
    /// until sessions are appended to concurrently, and nothing does that yet.
    fn next_seq(&self, session_id: &str) -> Result<u64> {
        let mut cache = self
            .last_seq
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = match cache.get(session_id) {
            Some(last) => last + 1,
            None => self.latest_event_seq(session_id)? + 1,
        };
        cache.insert(session_id.to_string(), next);
        Ok(next)
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

/// One parsed log line.
///
/// Every read path goes through this rather than matching substrings, so a
/// partially written line is structurally unreadable rather than a coincidence
/// of what text happens to be present. Requirement 3 of
/// `docs/specs/17_durable_task_state_and_crash_recovery/`.
///
/// This replaced a substring reader that did not merely mis-handle a torn line
/// but *fabricated* records from one: with all six message fields present in a
/// truncated tail, `read_messages` returned a phantom chat message.
struct SessionEvent {
    seq: u64,
    event_type: String,
    payload: serde_json::Value,
}

impl SessionEvent {
    /// A payload string field, or `None` when absent or not a string.
    fn text(&self, field: &str) -> Option<String> {
        self.payload
            .get(field)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    }

    fn number(&self, field: &str) -> Option<u128> {
        self.payload
            .get(field)
            .and_then(|value| value.as_u64())
            .map(u128::from)
    }
}

/// `None` for a line that is not a complete JSON object carrying an
/// `eventType`. A torn final line is the expected cause; `seq` falls back to
/// the caller's line ordering for events written before spec 16 added it, which
/// is their append order.
fn parse_event(line: &str, fallback_seq: u64) -> Option<SessionEvent> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let object = value.as_object()?;
    let event_type = object.get("eventType")?.as_str()?.to_string();
    let seq = object
        .get("seq")
        .and_then(|seq| seq.as_u64())
        .unwrap_or(fallback_seq);
    let payload = object
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Some(SessionEvent {
        seq,
        event_type,
        payload,
    })
}

/// Every parseable event, in order. Unparsable lines are dropped — see
/// [`parse_event`] — and the count of them is returned so a caller can audit
/// the discard rather than swallow it.
fn parsed_events(content: &str) -> (Vec<SessionEvent>, usize) {
    let mut events = Vec::new();
    let mut discarded = 0;
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_event(line, index as u64 + 1) {
            Some(event) => events.push(event),
            None => discarded += 1,
        }
    }
    (events, discarded)
}

fn latest_seq(content: &str) -> u64 {
    parsed_events(content)
        .0
        .last()
        .map(|event| event.seq)
        .unwrap_or(0)
}

/// The events that are part of the active conversation: everything up to the
/// newest `conversation_rewound` marker's `throughEventSeq`. A later rewind to
/// an earlier point supersedes an earlier one, so the newest marker wins even
/// when it points further back.
fn active_events(content: &str) -> Vec<SessionEvent> {
    let (events, _) = parsed_events(content);
    let limit = events
        .iter()
        .filter(|event| event.event_type == "conversation_rewound")
        .filter_map(|event| event.number("throughEventSeq"))
        .next_back()
        .and_then(|value| u64::try_from(value).ok());
    events
        .into_iter()
        .filter(|event| limit.is_none_or(|limit| event.seq <= limit))
        .collect()
}

fn parse_session_log(content: &str) -> Option<Session> {
    let (events, _) = parsed_events(content);
    events
        .iter()
        .filter(|event| {
            event.event_type == "session_created" || event.event_type == "session_renamed"
        })
        .filter_map(parse_session_event)
        .next_back()
}

fn parse_session_event(event: &SessionEvent) -> Option<Session> {
    Some(Session {
        id: event.text("id")?,
        repository_id: event.text("repositoryId")?,
        title: event.text("title")?,
        created_at_ms: event.number("createdAtMs")?,
        updated_at_ms: event.number("updatedAtMs")?,
        summary: event.text("summary").unwrap_or_default(),
    })
}

/// A `Task` from a `task_created` or `task_status_updated` payload.
///
/// `None` for a status string this version does not recognise — from a later
/// version, say. The event is then skipped and the task keeps its last
/// known-good record, which loses a status change but never invents one.
/// Recovery classification does not depend on this: `read_task_statuses` keeps
/// raw status strings, and `recovery::classify_session` treats an unrecognised
/// one as neither terminal nor safe.
fn parse_task_payload(payload: &serde_json::Value) -> Option<Task> {
    let text = |field: &str| {
        payload
            .get(field)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    Some(Task {
        id: text("id")?,
        session_id: text("sessionId")?,
        status: TaskStatus::parse(&text("status")?)?,
        user_prompt: text("userPrompt").unwrap_or_default(),
        model_provider: text("modelProvider").unwrap_or_default(),
        model_name: text("modelName").unwrap_or_default(),
        created_at_ms: payload
            .get("createdAtMs")
            .and_then(|value| value.as_u64())
            .map(u128::from)
            .unwrap_or_default(),
        completed_at_ms: payload
            .get("completedAtMs")
            .and_then(|value| value.as_u64())
            .map(u128::from),
    })
}

fn parse_message_event(event: &SessionEvent) -> Option<ChatMessage> {
    Some(ChatMessage {
        id: event.text("id")?,
        session_id: event.text("sessionId")?,
        task_id: event.text("taskId"),
        role: event.text("role")?,
        content: event.text("content")?,
        created_at_ms: event.number("createdAtMs")?,
    })
}
