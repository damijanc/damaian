use crate::audit::escape_json;
use crate::error::Result;
use crate::hash::{create_id, now_millis};
use crate::model::{TokenUsage, UsageSource};
use crate::secret_scanner::SecretScanner;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    /// Who opened the session: `"user"`, or the server-mode origin spec 52 will
    /// introduce. Stubbed now (always `"user"`) so search and the session list
    /// can filter on it without a second migration when 52 lands. See spec 53
    /// §5.6.
    pub origin: String,
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
    /// Stopped because the task reached `agent_max_task_tokens`.
    ///
    /// Beside `ToolBudgetExhausted` rather than reusing it: they are different
    /// facts with different remedies — one means the work needed more rounds,
    /// the other that it needed more money — and collapsing them would leave
    /// the eval harness unable to tell them apart. Spec 21 §5.4.
    TokenBudgetExhausted,
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
            Self::TokenBudgetExhausted,
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
            Self::TokenBudgetExhausted => "token_budget_exhausted",
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
            Self::Complete
                | Self::Failed
                | Self::Cancelled
                | Self::ToolBudgetExhausted
                | Self::TokenBudgetExhausted
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

/// One match from [`SessionStore::search_sessions`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchHit {
    pub session_id: String,
    pub session_title: String,
    /// The matching event's `seq`, the stable anchor to scroll to (§5.3).
    pub seq: u64,
    /// Surrounding text, redacted, with ellipses marking a cut.
    pub snippet: String,
    pub role: String,
    pub created_at_ms: u128,
    /// How many events in this session matched, so the ranking's second key is
    /// visible to the caller rather than hidden inside the sort.
    pub match_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResult {
    pub hits: Vec<SessionSearchHit>,
    /// `true` when the result cap cut the list — stated, never silently applied.
    pub capped: bool,
    /// Torn lines skipped across every session searched (§5.2).
    pub unreadable_lines: usize,
}

/// How [`SessionStore::search_sessions`] matches and caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// Match only on word boundaries rather than anywhere in the text.
    pub whole_word: bool,
    /// `true`: the query is one literal substring. `false`: the query is split
    /// on whitespace and every term must appear (an AND of substrings).
    pub literal_phrase: bool,
    pub max_results: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
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

impl ActionMarker {
    /// The marker's id, so a caller can tie other events to this action —
    /// spec 19 records a call's usage against the marker it started under, and
    /// recovery pairs the two to tell an accounted call from an unaccounted
    /// one. Read-only: only [`SessionStore`] may mint one.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// An action that started and never finished — the signature of a crash while
/// it was in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingAction {
    /// The `markerId` this action started under. Load-bearing for pairing:
    /// spec 19 records a call's usage against it, so recovery can tell an
    /// accounted call from an unaccounted one.
    pub marker_id: String,
    pub task_id: String,
    pub action: String,
    pub reference: String,
    /// Recorded on the *start* event, so the classifier does not have to
    /// re-derive the action's nature after the code that knew it is gone.
    pub side_effecting: bool,
    /// The input-token estimate written before a model call went out, so a
    /// call lost to a crash can still be accounted. `None` for every action
    /// that is not a model call, and for markers written before spec 19.
    pub estimated_input_tokens: Option<u64>,
    pub seq: u64,
}

/// A task's total token usage, summed from its `task_usage_recorded` events.
///
/// Not a stored record, for the same reason [`Task`] is not: the event log is
/// where task facts live, so a crash mid-task loses at most the run that was
/// in flight while every completed run stays correctly accounted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TaskUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// [`UsageSource::Estimated`] when *any* contributing run was estimated: a
    /// total is only as trustworthy as its weakest term.
    pub source: UsageSource,
    /// `Some` only when every contributing run reported a cost. A partial sum
    /// would understate the bill while looking authoritative.
    pub reported_cost: Option<f64>,
    pub run_count: u32,
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
            origin: "user".to_string(),
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
        self.update_task_status_with_kind(task, status, error, None)
    }

    /// [`Self::update_task_status`] with an optional typed failure reason, so a
    /// failure can be *named* rather than only described. Spec 48 §5.4: a task
    /// refused by the provider fails with `failureKind: "provider_rate_limited"`
    /// and a UI can branch on that code instead of parsing a sentence.
    ///
    /// `failure_kind` travels as a sibling of the wrapped `task` object —
    /// `{"task":…,"error":…,"failureKind":…}` — and is absent on every event
    /// written before this spec, so the read side defaults it to `None`.
    pub fn update_task_status_with_kind(
        &self,
        task: &Task,
        status: TaskStatus,
        error: Option<&str>,
        failure_kind: Option<&str>,
    ) -> Result<Task> {
        let mut updated = task.clone();
        updated.status = status;
        // Asks the status rather than re-listing the terminal ones here. The
        // hand-written list this replaces was the one place `TaskStatus::all()`
        // could not reach: a new terminal status omitted from it silently got
        // no completion timestamp, and nothing failed. Spec 21 `context.md`
        // §3.2.
        if updated.status.is_terminal() {
            updated.completed_at_ms = Some(now_millis());
        }
        let mut extra = Vec::new();
        if let Some(error) = error {
            extra.push(format!("\"error\":\"{}\"", escape_json(error)));
        }
        if let Some(kind) = failure_kind {
            extra.push(format!("\"failureKind\":\"{}\"", escape_json(kind)));
        }
        let payload = if extra.is_empty() {
            task_json(&updated)
        } else {
            format!("{{\"task\":{},{}}}", task_json(&updated), extra.join(","))
        };
        self.append_session_event(&task.session_id, "task_status_updated", &payload)?;
        Ok(updated)
    }

    /// The latest `failureKind` of every task in the session, keyed by task id.
    ///
    /// Mirrors [`Self::read_task_statuses`], reading only the sibling the
    /// wrapped `task_status_updated` event carries when a failure was named.
    /// A task without one is simply absent, so the two maps stay independent.
    pub fn read_task_failure_kinds(&self, session_id: &str) -> Result<HashMap<String, String>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(HashMap::new());
        };
        let mut kinds = HashMap::new();
        for event in active_events(&content) {
            if event.event_type != "task_status_updated" {
                continue;
            }
            let Some(task_id) = event
                .payload
                .get("task")
                .and_then(|value| value.get("id"))
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            let Some(kind) = event
                .payload
                .get("failureKind")
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            kinds.insert(task_id.to_string(), kind.to_string());
        }
        Ok(kinds)
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

    /// [`Self::read_messages`] with each message's event `seq`, the stable anchor
    /// a search hit scrolls to (§5.3). `seq` survives rewinds, where a rendered
    /// list index does not.
    pub fn read_messages_with_seq(&self, session_id: &str) -> Result<Vec<(u64, ChatMessage)>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(Vec::new());
        };
        Ok(messages_with_seq(&content))
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

    /// Appends one run's token usage. One event per call that reached the
    /// provider, never rewritten — spec 19 §5.4 follows spec 17's append-only
    /// rule, so a partial total is correct rather than absent.
    ///
    /// `marker_id` ties the event to the `action_started` marker for that
    /// call, so recovery can tell a call it has already accounted for from one
    /// it has not. `reason` explains a non-obvious estimate — a stopped
    /// stream, a lost call — and is always a fixed literal chosen here, never
    /// free text from a model, a file, or a provider.
    ///
    /// Requirement 7 is satisfied by construction: every field written is a
    /// number or an id.
    pub fn record_task_usage(
        &self,
        task: &Task,
        run_id: &str,
        marker_id: Option<&str>,
        usage: TokenUsage,
        reported_cost: Option<f64>,
        reason: Option<&str>,
    ) -> Result<()> {
        self.record_task_usage_for_task_id(
            &task.session_id,
            &task.id,
            run_id,
            marker_id,
            usage,
            reported_cost,
            reason,
        )
    }

    /// [`Self::record_task_usage`] for a caller that holds ids rather than a
    /// [`Task`] — recovery replays from the log and never rebuilds one.
    #[allow(clippy::too_many_arguments)] // Every argument is one field of the event being written.
    pub fn record_task_usage_for_task_id(
        &self,
        session_id: &str,
        task_id: &str,
        run_id: &str,
        marker_id: Option<&str>,
        usage: TokenUsage,
        reported_cost: Option<f64>,
        reason: Option<&str>,
    ) -> Result<()> {
        let cost = match reported_cost {
            Some(cost) => format!("{cost}"),
            None => "null".to_string(),
        };
        let marker = match marker_id {
            Some(marker_id) => format!(",\"markerId\":\"{}\"", escape_json(marker_id)),
            None => String::new(),
        };
        let reason = match reason {
            Some(reason) => format!(",\"reason\":\"{}\"", escape_json(reason)),
            None => String::new(),
        };
        self.append_session_event(
            session_id,
            "task_usage_recorded",
            &format!(
                "{{\"taskId\":\"{}\",\"runId\":\"{}\"{},\"inputTokens\":{},\"outputTokens\":{},\"source\":\"{}\",\"reportedCost\":{}{}}}",
                escape_json(task_id),
                escape_json(run_id),
                marker,
                usage.input_tokens,
                usage.output_tokens,
                usage.source.as_str(),
                cost,
                reason
            ),
        )
    }

    /// Every task's summed usage, keyed by task id.
    ///
    /// A task with no usage events is **absent** from the map rather than
    /// present with a zero. That is the difference between "not recorded" and
    /// "used nothing", and it is what lets a session written before usage
    /// existed report honestly instead of claiming a free turn.
    ///
    /// Reads **all** events rather than only the active conversation, for the
    /// same reason [`Self::dangling_actions`] does: a rewind moves the
    /// conversation back, but what was billed was billed.
    pub fn read_task_usage(&self, session_id: &str) -> Result<HashMap<String, TaskUsage>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(HashMap::new());
        };
        let mut totals: HashMap<String, TaskUsage> = HashMap::new();
        let mut every_run_costed: HashMap<String, bool> = HashMap::new();
        for event in parsed_events(&content).0 {
            if event.event_type != "task_usage_recorded" {
                continue;
            }
            let Some(task_id) = event.text("taskId") else {
                continue;
            };
            let input = event
                .payload
                .get("inputTokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let output = event
                .payload
                .get("outputTokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            // An absent or unrecognised source is treated as an estimate: the
            // weaker claim is the safe default, and a version that does not
            // understand a future source must not upgrade it to measured.
            let source = event
                .text("source")
                .and_then(|value| UsageSource::parse(&value))
                .unwrap_or(UsageSource::Estimated);
            let cost = event
                .payload
                .get("reportedCost")
                .and_then(|value| value.as_f64());

            let costed = every_run_costed.entry(task_id.clone()).or_insert(true);
            *costed = *costed && cost.is_some();

            let total = totals.entry(task_id).or_insert(TaskUsage {
                input_tokens: 0,
                output_tokens: 0,
                source: UsageSource::Measured,
                reported_cost: None,
                run_count: 0,
            });
            total.input_tokens += input;
            total.output_tokens += output;
            total.run_count += 1;
            if source == UsageSource::Estimated {
                total.source = UsageSource::Estimated;
            }
            total.reported_cost = Some(total.reported_cost.unwrap_or(0.0) + cost.unwrap_or(0.0));
        }
        for (task_id, total) in totals.iter_mut() {
            if !every_run_costed.get(task_id).copied().unwrap_or(false) {
                total.reported_cost = None;
            }
        }
        Ok(totals)
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
        self.start_action_with_estimate(task, action, reference, side_effecting, None)
    }

    /// [`Self::start_action`], additionally recording the input-token estimate
    /// for a model call.
    ///
    /// The estimate has to be durable *before* the call, because after a crash
    /// the request object is gone: recovery has the marker and nothing else,
    /// and would otherwise have only a zero to account a billed call with.
    /// `None` for every action that is not a model call. Spec 19 §5.5.
    pub fn start_action_with_estimate(
        &self,
        task: &Task,
        action: &str,
        reference: &str,
        side_effecting: bool,
        estimated_input_tokens: Option<u64>,
    ) -> Result<ActionMarker> {
        let marker = ActionMarker {
            id: create_id("action"),
            session_id: task.session_id.clone(),
            task_id: task.id.clone(),
            action: action.to_string(),
            reference: reference.to_string(),
        };
        let estimate = match estimated_input_tokens {
            Some(tokens) => format!(",\"estimatedInputTokens\":{tokens}"),
            None => String::new(),
        };
        self.append_session_event(
            &marker.session_id,
            "action_started",
            &format!(
                "{{\"markerId\":\"{}\",\"taskId\":\"{}\",\"action\":\"{}\",\"ref\":\"{}\",\"sideEffecting\":{}{}}}",
                escape_json(&marker.id),
                escape_json(&marker.task_id),
                escape_json(&marker.action),
                escape_json(&marker.reference),
                side_effecting,
                estimate
            ),
        )?;
        Ok(marker)
    }

    /// The `markerId` of every action that already has a usage event.
    ///
    /// Recovery runs at every launch, so "has this call been accounted for"
    /// has to be answerable from the log rather than from a flag held in
    /// memory. Without it, one crash would inflate the reported spend on
    /// every subsequent start.
    pub fn usage_marker_ids(&self, session_id: &str) -> Result<HashSet<String>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(HashSet::new());
        };
        Ok(parsed_events(&content)
            .0
            .into_iter()
            .filter(|event| event.event_type == "task_usage_recorded")
            .filter_map(|event| event.text("markerId"))
            .collect())
    }

    /// Records `action_finished`, consuming the marker.
    pub fn finish_action(&self, marker: ActionMarker, outcome: &str) -> Result<()> {
        self.write_action_finished(&marker, outcome, None)
    }

    /// [`Self::finish_action`] for a command, recording the exit code and
    /// deriving the outcome from it.
    ///
    /// Split out rather than folded into `finish_action` because the outcome is
    /// a *function* of the exit code and must not be passed in beside it: two
    /// callers could then disagree, and the log would carry a `"failed"` next
    /// to an exit code of zero with nothing to say which was right.
    ///
    /// `None` maps to `"unknown"`, never to `"ok"`. A killed or signalled
    /// command reports no code (`command_runner.rs`, `output.status.code()`),
    /// and mapping absence to success is how "never represent an unrun check as
    /// passed" gets violated by an `unwrap_or(0)`. See
    /// `docs/specs/21_task_plan_progress_and_budget/proposal.md` §5.3.
    pub fn finish_command_action(
        &self,
        marker: ActionMarker,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let outcome = match exit_code {
            Some(0) => "ok",
            Some(_) => "failed",
            None => "unknown",
        };
        self.write_action_finished(&marker, outcome, exit_code)
    }

    fn write_action_finished(
        &self,
        marker: &ActionMarker,
        outcome: &str,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let code = match exit_code {
            Some(code) => format!(",\"exitCode\":{code}"),
            None => String::new(),
        };
        self.append_session_event(
            &marker.session_id,
            "action_finished",
            &format!(
                "{{\"markerId\":\"{}\",\"taskId\":\"{}\",\"action\":\"{}\",\"ref\":\"{}\",\"outcome\":\"{}\"{}}}",
                escape_json(&marker.id),
                escape_json(&marker.task_id),
                escape_json(&marker.action),
                escape_json(&marker.reference),
                escape_json(outcome),
                code
            ),
        )
    }

    /// Appends the plan a turn will work through.
    ///
    /// See `docs/specs/21_task_plan_progress_and_budget/proposal.md` §5.2. Plan
    /// state goes in the session log rather than a second store, so
    /// requirement 5's "recovered after restart" is the same replay that
    /// already recovers task status — and a crash mid-step loses nothing
    /// already recorded.
    pub fn create_plan(&self, task: &Task, plan: &crate::plan::TaskPlan) -> Result<()> {
        self.append_plan(task, "plan_created", plan)
    }

    /// Appends the user's revision of a plan, keeping the original in the log.
    ///
    /// §5.5: both the plan Damaian proposed and the plan the user approved are
    /// history, and a revision that overwrote the original would leave the log
    /// unable to say what was suggested.
    pub fn revise_plan(&self, task: &Task, plan: &crate::plan::TaskPlan) -> Result<()> {
        self.append_plan(task, "plan_revised", plan)
    }

    fn append_plan(
        &self,
        task: &Task,
        event_type: &str,
        plan: &crate::plan::TaskPlan,
    ) -> Result<()> {
        // The plan's own `task_id` is authoritative and is not re-derived from
        // `task` here: `resume_plan` writes a plan whose `task_id` it has
        // deliberately rewritten, and silently overwriting that would send the
        // carried plan back to the task it came from.
        let payload = serde_json::to_string(plan).map_err(|error| {
            crate::error::ClientError::Io(format!("plan serialization: {error}"))
        })?;
        self.append_session_event(&task.session_id, event_type, &payload)
    }

    /// Carries a plan forward onto a new task, keeping the original readable
    /// under the task that made it.
    ///
    /// `context.md` §3.1: a task is one turn, so the resumed turn is a *new*
    /// task with a new id and [`Self::read_task_plan`] is keyed on that id.
    /// Without this, §5.4's "the plan survives, so the user can raise the
    /// ceiling and resume" is true of the log and false of the user.
    ///
    /// Step state comes across verbatim, evidence included. A step that was
    /// `InProgress` stays open — work resumes on it — and a completed step
    /// keeps what confirmed it, or the resumed plan would re-run work it can
    /// already show was done and downgrade a verified step to an unverified one
    /// on the way through.
    ///
    /// A task with no plan carries nothing rather than an empty plan: `None`
    /// and a zero-step plan are different facts.
    pub fn resume_plan(&self, task: &Task, from_task_id: &str) -> Result<()> {
        let Some(mut plan) = self.read_task_plan(&task.session_id, from_task_id)? else {
            return Ok(());
        };
        plan.task_id = task.id.clone();
        let mut payload = serde_json::to_value(&plan).map_err(|error| {
            crate::error::ClientError::Io(format!("plan serialization: {error}"))
        })?;
        if let Some(object) = payload.as_object_mut() {
            // Alongside the plan's own fields rather than wrapping it, so the
            // reader needs no special case: `TaskPlan` ignores the extra key.
            object.insert(
                "resumedFrom".to_string(),
                serde_json::Value::String(from_task_id.to_string()),
            );
        }
        self.append_session_event(&task.session_id, "plan_resumed", &payload.to_string())
    }

    /// Appends one more piece of evidence to whichever step is open.
    ///
    /// For work that lands *outside* the turn that planned it: a patch is
    /// proposed in one turn and applied later, after that turn has ended, so
    /// the apply path has evidence and no in-memory plan to put it on.
    ///
    /// Appends rather than replaces, and does nothing when no step is open. A
    /// step that has already closed reached its status from its own evidence,
    /// and adding to it afterwards would change the answer to a question that
    /// was already settled.
    pub fn append_step_evidence(
        &self,
        session_id: &str,
        task_id: &str,
        evidence: crate::plan::Evidence,
    ) -> Result<()> {
        let Some(plan) = self.read_task_plan(session_id, task_id)? else {
            return Ok(());
        };
        let Some(mut open) = plan
            .steps
            .into_iter()
            .find(|step| step.status == crate::plan::StepStatus::InProgress)
        else {
            return Ok(());
        };
        open.evidence.push(evidence);
        let step_json = serde_json::to_string(&open).map_err(|error| {
            crate::error::ClientError::Io(format!("plan step serialization: {error}"))
        })?;
        self.append_session_event(
            session_id,
            "plan_step_updated",
            &format!(
                "{{\"taskId\":\"{}\",\"step\":{}}}",
                escape_json(task_id),
                step_json
            ),
        )
    }

    /// Appends one step's new state.
    ///
    /// The step is written **whole** rather than as a delta. A partial update
    /// would need the reader to know which absent field means "unchanged" and
    /// which means "cleared" — and spec 17's rule is that the log says what is
    /// true, not what changed, precisely so a reader never has to guess.
    pub fn update_plan_step(&self, task: &Task, step: &crate::plan::PlanStep) -> Result<()> {
        let step_json = serde_json::to_string(step).map_err(|error| {
            crate::error::ClientError::Io(format!("plan step serialization: {error}"))
        })?;
        self.append_session_event(
            &task.session_id,
            "plan_step_updated",
            &format!(
                "{{\"taskId\":\"{}\",\"step\":{}}}",
                escape_json(&task.id),
                step_json
            ),
        )
    }

    /// The task's plan as of the newest event for each step, or `None` when the
    /// task has no plan at all.
    ///
    /// `None` and an empty plan are different facts: a trivial turn gets no
    /// plan (§5.1), which is not the same as a plan that proposed no steps.
    ///
    /// Reads **active** events, unlike [`Self::read_task_usage`]. A rewind
    /// moves the conversation back, and a plan is part of the conversation —
    /// whereas what was billed was billed regardless of where the conversation
    /// now sits.
    pub fn read_task_plan(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<Option<crate::plan::TaskPlan>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(None);
        };
        let mut plan: Option<crate::plan::TaskPlan> = None;
        for event in active_events(&content) {
            match event.event_type.as_str() {
                "plan_created" | "plan_revised" | "plan_resumed" => {
                    let Ok(created) =
                        serde_json::from_value::<crate::plan::TaskPlan>(event.payload.clone())
                    else {
                        continue;
                    };
                    // A session holds every task's plan, so the reader selects.
                    if created.task_id == task_id {
                        plan = Some(created);
                    }
                }
                "plan_step_updated" => {
                    let Some(current) = plan.as_mut() else {
                        continue;
                    };
                    if event.text("taskId").as_deref() != Some(task_id) {
                        continue;
                    }
                    let Some(updated) = event.payload.get("step").cloned().and_then(|value| {
                        serde_json::from_value::<crate::plan::PlanStep>(value).ok()
                    }) else {
                        continue;
                    };
                    // A step id the plan does not contain is ignored rather
                    // than appended. The step list is set by `plan_created` and
                    // `plan_revised`; letting an update introduce a step would
                    // let the log grow a plan nobody wrote.
                    if let Some(existing) =
                        current.steps.iter_mut().find(|step| step.id == updated.id)
                    {
                        *existing = updated;
                    }
                }
                _ => {}
            }
        }
        Ok(plan)
    }

    /// Every task's plan in the session, in one pass.
    ///
    /// [`Self::read_task_plan`] answers for one task and re-reads the log to do
    /// it, which is right for a turn that only has its own plan in hand. A
    /// session view needs all of them, and calling the single-task reader once
    /// per task would read the whole log once per task — quadratic in exactly
    /// the sessions that are already the longest.
    pub fn read_session_plans(
        &self,
        session_id: &str,
    ) -> Result<HashMap<String, crate::plan::TaskPlan>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(HashMap::new());
        };
        let mut plans: HashMap<String, crate::plan::TaskPlan> = HashMap::new();
        for event in active_events(&content) {
            match event.event_type.as_str() {
                "plan_created" | "plan_revised" | "plan_resumed" => {
                    if let Ok(created) =
                        serde_json::from_value::<crate::plan::TaskPlan>(event.payload.clone())
                    {
                        plans.insert(created.task_id.clone(), created);
                    }
                }
                "plan_step_updated" => {
                    let Some(task_id) = event.text("taskId") else {
                        continue;
                    };
                    let Some(current) = plans.get_mut(&task_id) else {
                        continue;
                    };
                    let Some(updated) = event.payload.get("step").cloned().and_then(|value| {
                        serde_json::from_value::<crate::plan::PlanStep>(value).ok()
                    }) else {
                        continue;
                    };
                    // Same rule as the single-task reader: an update may not
                    // introduce a step the plan does not have.
                    if let Some(existing) =
                        current.steps.iter_mut().find(|step| step.id == updated.id)
                    {
                        *existing = updated;
                    }
                }
                _ => {}
            }
        }
        Ok(plans)
    }

    /// Records that the user reviewed this task's plan and let the work go
    /// ahead. Spec 21 §5.5.
    ///
    /// A separate event rather than a field on the plan, for the same reason
    /// the log is append-only everywhere else: the approval is a decision
    /// taken at a moment, and rewriting the plan to carry it would lose which
    /// version of the steps the user was actually looking at. A revision
    /// writes `plan_revised` and then this, so there is one event to read
    /// whether the user approved as-proposed or edited first.
    pub fn approve_plan(&self, task: &Task, approved_by: &str) -> Result<()> {
        self.append_session_event(
            &task.session_id,
            "plan_approved",
            &format!(
                "{{\"taskId\":\"{}\",\"approvedBy\":\"{}\"}}",
                escape_json(&task.id),
                escape_json(approved_by)
            ),
        )
    }

    /// Whether this task's plan has been approved.
    ///
    /// Reads *active* events, like [`Self::read_task_plan`] and unlike
    /// [`Self::read_task_usage`]: an approval is part of the conversation, so
    /// rewinding to before the user saw the plan must take the approval with
    /// it. The alternative — a rewind that keeps the clearance but drops the
    /// plan it was granted for — would let a rewind quietly widen what the
    /// agent may do without asking again.
    ///
    /// Absence is `false`, never "unknown": the gate reads this to decide
    /// whether to pause, and anything that fell through to `true` would let a
    /// mutating step run unreviewed.
    pub fn read_plan_approved(&self, session_id: &str, task_id: &str) -> Result<bool> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(false);
        };
        Ok(active_events(&content).iter().any(|event| {
            event.event_type == "plan_approved" && event.text("taskId").as_deref() == Some(task_id)
        }))
    }

    /// Tasks carrying a usage entry for a model call lost to a crash, written
    /// by `recovery::account_for_lost_model_calls` (spec 19 §5.5).
    ///
    /// The totals from [`Self::read_task_usage`] already include that call, but
    /// they cannot say so, and a recovery prompt that claimed "this includes
    /// the interrupted call" from the dangling marker alone would be wrong
    /// whenever the marker predates spec 19 and carried no estimate to record.
    /// So the claim is read back from what was actually written.
    pub fn lost_to_crash_task_ids(&self, session_id: &str) -> Result<HashSet<String>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(HashSet::new());
        };
        Ok(parsed_events(&content)
            .0
            .iter()
            .filter(|event| {
                event.event_type == "task_usage_recorded"
                    && event.text("reason").as_deref() == Some("lost_to_crash")
            })
            .filter_map(|event| event.text("taskId"))
            .collect())
    }

    /// Records that this task's work was handed to another turn, so recovery
    /// stops asking about it.
    ///
    /// The status is deliberately **left alone**. A task resumed from the crash
    /// prompt (`docs/specs/45_crash_recovery_prompt.md` §5.4) is re-sent as a
    /// fresh turn with a task of its own, which leaves the original
    /// non-terminal forever — so every later launch classifies it as
    /// interrupted and offers the same card again. Writing a terminal status
    /// instead would mean choosing one that lies: `complete` (it never
    /// finished), `failed` (nothing failed), or `cancelled` (it *was* retried).
    /// What is true is that it was superseded, so that is what is recorded, and
    /// the prompt filters on it.
    pub fn mark_task_superseded(
        &self,
        session_id: &str,
        task_id: &str,
        reason: &str,
    ) -> Result<()> {
        self.append_session_event(
            session_id,
            "task_superseded",
            &format!(
                "{{\"taskId\":\"{}\",\"reason\":\"{}\"}}",
                escape_json(task_id),
                escape_json(reason)
            ),
        )
    }

    /// Tasks whose work was handed to another turn, by [`Self::mark_task_superseded`].
    ///
    /// Reads *active* events, so a rewind past the resume takes the supersession
    /// with it and the task becomes a live question again — which is correct:
    /// the turn that superseded it is no longer in the conversation either.
    pub fn superseded_task_ids(&self, session_id: &str) -> Result<HashSet<String>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(HashSet::new());
        };
        Ok(active_events(&content)
            .iter()
            .filter(|event| event.event_type == "task_superseded")
            .filter_map(|event| event.text("taskId"))
            .collect())
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
                        marker_id.clone(),
                        DanglingAction {
                            marker_id,
                            task_id,
                            action: event.text("action").unwrap_or_default(),
                            reference: event.text("ref").unwrap_or_default(),
                            side_effecting: event
                                .payload
                                .get("sideEffecting")
                                .and_then(|value| value.as_bool())
                                .unwrap_or(true),
                            estimated_input_tokens: event
                                .payload
                                .get("estimatedInputTokens")
                                .and_then(|value| value.as_u64()),
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

    /// A bounded scan over the sessions of `repository_id` (or every session
    /// when `None`), matching `message_appended` content and task titles through
    /// spec 17's parse-first reader. Snippets are redacted by `scanner` before
    /// they are returned, because storage is deliberately unredacted (§5.4).
    pub fn search_sessions(
        &self,
        repository_id: Option<&str>,
        query: &str,
        options: SearchOptions,
        scanner: &SecretScanner,
    ) -> Result<SessionSearchResult> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(SessionSearchResult {
                hits: Vec::new(),
                capped: false,
                unreadable_lines: 0,
            });
        }
        let needles = search_needles(query, options.literal_phrase);
        let mut grouped: Vec<(Session, Vec<SessionSearchHit>)> = Vec::new();
        let mut unreadable_lines = 0;
        // `list_sessions` is already recency-descending, so the scan reads the
        // sessions a user most likely wants first — but the sort below re-ranks
        // on match count, so the order here is only a hint.
        for session in self.list_sessions(repository_id)? {
            if session.origin != "user" {
                continue;
            }
            let Ok(content) = fs::read_to_string(self.session_log_path(&session.id)) else {
                continue;
            };
            let (events, discarded) = parsed_events(&content);
            unreadable_lines += discarded;
            let mut session_hits = Vec::new();
            for event in &events {
                let (haystack, role) = match event.event_type.as_str() {
                    "message_appended" => (event.text("content"), event.text("role")),
                    "task_created" => (event.text("userPrompt"), Some("task".to_string())),
                    _ => continue,
                };
                let Some(haystack) = haystack else { continue };
                let Some(match_start) = first_match(&haystack, &needles, options.whole_word) else {
                    continue;
                };
                session_hits.push(SessionSearchHit {
                    session_id: session.id.clone(),
                    session_title: session.title.clone(),
                    seq: event.seq,
                    snippet: snippet_around(&haystack, match_start, scanner),
                    role: role.unwrap_or_default(),
                    created_at_ms: event.number("createdAtMs").unwrap_or(0),
                    match_count: 0,
                });
            }
            if !session_hits.is_empty() {
                grouped.push((session, session_hits));
            }
        }
        // Ranking is recency first, then match count, then title, so two
        // identical searches over an unchanged corpus return the same order.
        grouped.sort_by(|left, right| {
            right
                .0
                .updated_at_ms
                .cmp(&left.0.updated_at_ms)
                .then(right.1.len().cmp(&left.1.len()))
                .then(left.0.title.cmp(&right.0.title))
        });
        let total = grouped.iter().map(|(_, hits)| hits.len()).sum::<usize>();
        let mut hits = Vec::with_capacity(total.min(options.max_results));
        for (_session, mut session_hits) in grouped {
            session_hits.sort_by_key(|hit| hit.seq);
            let count = session_hits.len();
            for mut hit in session_hits {
                hit.match_count = count;
                hits.push(hit);
            }
        }
        let capped = hits.len() > options.max_results;
        hits.truncate(options.max_results);
        Ok(SessionSearchResult {
            hits,
            capped,
            unreadable_lines,
        })
    }

    /// Renders a session to Markdown or JSON, redacted on the way out with the
    /// redaction count stated rather than silently applied (§5.4, §5.5).
    pub fn export_session(
        &self,
        session_id: &str,
        format: ExportFormat,
        scanner: &SecretScanner,
    ) -> Result<String> {
        let Some(session) = self.read_session(session_id)? else {
            return Err(crate::error::ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        };
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Err(crate::error::ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        };
        let messages = messages_with_seq(&content);
        let tasks = self.read_tasks(session_id)?;
        let usage = self.read_task_usage(session_id)?;
        let plans = self.read_session_plans(session_id)?;

        let mut redaction_count = 0usize;
        let mut redact = |text: &str| -> String {
            let redaction = scanner.redact(text);
            redaction_count += redaction.findings.len();
            redaction.text
        };

        let rendered = match format {
            ExportFormat::Markdown => {
                let mut out = String::new();
                out.push_str(&format!("# {}\n\n", redact(&session.title)));
                out.push_str(&format!("- Repository: `{}`\n", session.repository_id));
                out.push_str(&format!(
                    "- Time range: {} ms → {} ms (epoch)\n",
                    session.created_at_ms, session.updated_at_ms
                ));
                out.push_str("- Origin: user\n\n");
                out.push_str("## Conversation\n\n");
                for (_, message) in &messages {
                    out.push_str(&format!(
                        "### {}\n\n{}\n\n",
                        message.role,
                        redact(&message.content)
                    ));
                }
                out.push_str("## Tasks\n\n");
                for task in &tasks {
                    out.push_str(&format!(
                        "- {} — {}",
                        task.status.as_str(),
                        redact(&task.user_prompt)
                    ));
                    if let Some(total) = usage.get(&task.id) {
                        out.push_str(&format!(
                            " ({} in / {} out tokens{})",
                            total.input_tokens,
                            total.output_tokens,
                            total
                                .reported_cost
                                .map(|cost| format!(", reported cost {cost}"))
                                .unwrap_or_default()
                        ));
                    }
                    out.push('\n');
                    if let Some(plan) = plans.get(&task.id) {
                        for step in &plan.steps {
                            out.push_str(&format!(
                                "  - [{}] {}\n",
                                format!("{:?}", step.status).to_lowercase(),
                                redact(&step.title)
                            ));
                            for evidence in &step.evidence {
                                out.push_str(&format!(
                                    "    - {}\n",
                                    serde_json::to_string(evidence).unwrap_or_default()
                                ));
                            }
                        }
                    }
                }
                out
            }
            ExportFormat::Json => {
                let messages: Vec<serde_json::Value> = messages
                    .iter()
                    .map(|(seq, message)| {
                        serde_json::json!({
                            "seq": seq,
                            "role": message.role,
                            "content": redact(&message.content),
                            "taskId": message.task_id,
                            "createdAtMs": message.created_at_ms,
                        })
                    })
                    .collect();
                let tasks: Vec<serde_json::Value> = tasks
                    .iter()
                    .map(|task| {
                        serde_json::json!({
                            "id": task.id,
                            "status": task.status.as_str(),
                            "userPrompt": redact(&task.user_prompt),
                            "modelProvider": task.model_provider,
                            "modelName": task.model_name,
                            "usage": usage.get(&task.id).map(|total| serde_json::json!({
                                "inputTokens": total.input_tokens,
                                "outputTokens": total.output_tokens,
                                "source": total.source,
                                "reportedCost": total.reported_cost,
                                "runCount": total.run_count,
                            })),
                            "plan": plans.get(&task.id).map(|plan| serde_json::json!({
                                "createdAtMs": plan.created_at_ms,
                                "steps": plan.steps.iter().map(|step| serde_json::json!({
                                    "id": step.id,
                                    "title": redact(&step.title),
                                    "detail": step.detail.as_ref().map(|d| redact(d)),
                                    "status": step.status,
                                    "evidence": step.evidence,
                                })).collect::<Vec<_>>(),
                            })),
                        })
                    })
                    .collect();
                serde_json::to_string_pretty(&serde_json::json!({
                    "redacted": true,
                    "redactionCount": redaction_count,
                    "session": {
                        "id": session.id,
                        "repositoryId": session.repository_id,
                        "title": session.title,
                        "createdAtMs": session.created_at_ms,
                        "updatedAtMs": session.updated_at_ms,
                        "origin": session.origin,
                    },
                    "messages": messages,
                    "tasks": tasks,
                }))
                .unwrap_or_default()
            }
        };

        // The notice goes last in Markdown (header carries no count) and is a
        // field in JSON, so both state the count. Reuse `redact` is done; read
        // the tally once.
        match format {
            ExportFormat::Markdown => Ok(format!(
                "{rendered}\n---\nThis export was redacted: {redaction_count} secret(s) removed.\n"
            )),
            ExportFormat::Json => Ok(rendered),
        }
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
        "{{\"id\":\"{}\",\"repositoryId\":\"{}\",\"title\":\"{}\",\"createdAtMs\":{},\"updatedAtMs\":{},\"summary\":\"{}\",\"origin\":\"{}\"}}",
        escape_json(&session.id),
        escape_json(&session.repository_id),
        escape_json(&session.title),
        session.created_at_ms,
        session.updated_at_ms,
        escape_json(&session.summary),
        escape_json(&session.origin)
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
        origin: event.text("origin").unwrap_or_else(|| "user".to_string()),
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

/// The active conversation's messages with their event `seq`, for export's JSON
/// and for a search hit's anchor. Mirrors [`SessionStore::read_messages`] but
/// keeps the `seq` that method drops.
fn messages_with_seq(content: &str) -> Vec<(u64, ChatMessage)> {
    active_events(content)
        .iter()
        .filter(|event| event.event_type == "message_appended")
        .filter_map(|event| parse_message_event(event).map(|message| (event.seq, message)))
        .collect()
}

/// The case-insensitive substrings a query is split into: one literal phrase, or
/// every whitespace-separated term (an AND of substrings).
fn search_needles(query: &str, literal_phrase: bool) -> Vec<String> {
    if literal_phrase {
        vec![query.to_ascii_lowercase()]
    } else {
        query
            .split_whitespace()
            .map(|term| term.to_ascii_lowercase())
            .collect()
    }
}

/// The byte offset of the earliest match, or `None` when any needle is absent.
///
/// ASCII case-folding keeps byte offsets identical between `text` and its
/// lowercase copy, so the returned offset indexes `text` directly.
fn first_match(text: &str, needles: &[String], whole_word: bool) -> Option<usize> {
    if needles.is_empty() {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    let mut earliest: Option<usize> = None;
    for needle in needles {
        let mut from = 0;
        let mut found = false;
        while let Some(relative) = lower[from..].find(needle.as_str()) {
            let start = from + relative;
            if whole_word && !on_word_boundaries(&lower, start, needle.len()) {
                from = start + 1;
                continue;
            }
            earliest = Some(earliest.map_or(start, |current| current.min(start)));
            found = true;
            break;
        }
        if !found {
            return None;
        }
    }
    earliest
}

fn on_word_boundaries(text: &str, start: usize, len: usize) -> bool {
    let before = start == 0 || {
        text[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric() && character != '_')
    };
    let after = start + len >= text.len() || {
        text[start + len..]
            .chars()
            .next()
            .is_none_or(|character| !character.is_alphanumeric() && character != '_')
    };
    before && after
}

/// A redacted window of text around `match_start`, with ellipses marking a cut.
fn snippet_around(text: &str, match_start: usize, scanner: &SecretScanner) -> String {
    const WINDOW_CHARS: usize = 60;
    let char_index = text[..match_start].chars().count();
    let start = char_index.saturating_sub(WINDOW_CHARS);
    let end = (char_index + WINDOW_CHARS).min(text.chars().count());
    let mut snippet: String = text.chars().skip(start).take(end - start).collect();
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < text.chars().count() {
        snippet.push('…');
    }
    scanner.redact(&snippet).text
}
