use crate::audit::escape_json;
use crate::clarification::{
    AskUserInput, PendingDecision, PendingQuestionRef, QuestionState, validate_answer,
};
use crate::error::{ClientError, Result};
use crate::external_reference::ExternalReference;
use crate::file_access::LineRange;
use crate::hash::{create_id, now_millis};
use crate::mode::SessionMode;
use crate::model::{TokenUsage, UsageSource};
use crate::secret_scanner::SecretScanner;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
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

/// A turn's notes as the session log holds them (spec 65 §5.6).
///
/// The text has already been through the secret scanner: the chat loop redacts
/// before it records, and the export redacts again through its own closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnNotes {
    pub text: String,
    /// Whether the record still holds text carried from an earlier turn.
    pub carried: bool,
    /// The size of the change that produced this event.
    pub chars: usize,
    /// The running size of the record after the change.
    pub total_chars: usize,
}

/// A file or range the user pinned for the session (spec 27 §5.4).
///
/// A whole-file pin has `range: None`; a ranged pin names the lines the user
/// chose. The pin is control state, not content: only the path and range are
/// stored, and the assembler reads the file through the same path policy and
/// redaction as every other source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPin {
    pub path: String,
    pub range: Option<LineRange>,
}

/// What makes a session a worker (spec 57 §5.1 step 7): where it came from,
/// where it runs, and the exact git commands that made its worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRecord {
    /// `None` for a worker started from the sidebar rather than from a block.
    pub parent_session_id: Option<String>,
    pub source_repository: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub base_commit: String,
    pub included_uncommitted: bool,
    pub git_commands: Vec<String>,
}

/// The user's decision about one or more files of a patch.
///
/// A session event rather than a field on the stored patch: neither the apply
/// nor the reject path rewrites the stored patch, and
/// `ProposedFilePatch.status` is the content's `added`/`modified`/`deleted`,
/// never the review's. Without this record "the patch is settled" cannot be
/// computed after a restart (spec 23 `context.md` §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchFileDecision {
    pub task_id: String,
    pub patch_id: String,
    pub paths: Vec<String>,
    /// `true` for applied, `false` for rejected. Stored as a boolean rather
    /// than an enum because a decision event describes exactly one action and a
    /// third state would be a different event.
    pub applied: bool,
    /// What Damaian wrote at each applied path, from
    /// `PatchApplyResult::applied`. Spec 35 uses it to tell an accepted file
    /// that is still exactly as Damaian left it from one the user edited
    /// afterwards; without it a commit can only guess.
    ///
    /// Empty for a rejection, and for a decision written before spec 35.
    #[serde(default)]
    pub applied_hashes: BTreeMap<String, String>,
    /// The hunk ids accepted for each applied path, for the commit's hunk
    /// count and for what the preparation shows. Empty means "not recorded",
    /// which a caller must not read as "no hunks".
    #[serde(default)]
    pub accepted_hunks: BTreeMap<String, Vec<String>>,
}

/// One `commit_created` event, as the change timeline reads it. The commit
/// SHA and prepare id are for display and inspection; `paths` is what removes
/// those files from the commit's candidate set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSessionEvent {
    pub task_id: String,
    pub prepare_id: String,
    pub commit_sha: String,
    pub paths: Vec<String>,
}

/// The session's accepted-and-committed history, in log order. Spec 35 walks
/// it to answer "which accepted changes are not committed yet" without
/// rewriting any stored patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeEvent {
    Applied {
        paths: Vec<String>,
        applied_hashes: BTreeMap<String, String>,
        accepted_hunks: BTreeMap<String, Vec<String>>,
    },
    Committed {
        paths: Vec<String>,
    },
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
    /// Held at a round boundary by a worker's pause (spec 57 §5.4). Nothing
    /// is in flight: the previous round finished and the next has not
    /// started, so a crash here is as resumable as `PreparingContext`.
    Paused,
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
            Self::Paused,
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
            Self::Paused => "paused",
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

    /// Whether a turn is still going in this state, as the plan panel means it.
    ///
    /// Deliberately **not** `!is_terminal()`. `Interrupted` and
    /// `UnknownExternalOutcome` are non-terminal — recovery can still act on
    /// them — but no turn is running, and a crash-leftover plan must read "not
    /// finished" rather than "running". The match is exhaustive so a new state
    /// has to choose (spec 66 `context.md` D7).
    pub fn is_active(&self) -> bool {
        match self {
            Self::Created
            | Self::PreparingContext
            | Self::WaitingForModel
            | Self::RunningTool
            | Self::WaitingForApproval
            | Self::Paused
            | Self::ApplyingPatch
            | Self::Validating => true,
            Self::Complete
            | Self::Failed
            | Self::Cancelled
            | Self::ToolBudgetExhausted
            | Self::TokenBudgetExhausted
            | Self::Interrupted
            | Self::UnknownExternalOutcome => false,
        }
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
    /// What wrote this entry, when it was not an ordinary chat turn. The only
    /// value today is `"plan_reminder"` (spec 66 §5.5): the engine's nudge is a
    /// user-role message in the request but must render as a system line and
    /// must not seed the next turn as the user's own words. `None` for every
    /// message a person or the model wrote.
    pub kind: Option<String>,
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
    /// `"command"`, `"patch"`, `"plan"`, `"provider_fallback"`,
    /// `"round_budget"` or `"checks"`.
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
    /// Cached input tokens summed over the runs whose provider reported a
    /// split. `None` when no run did — never `Some(0)`, which would render as
    /// "caching is broken" when the truth is "we cannot see it".
    /// Spec 49 §5.6.
    pub cached_input_tokens: Option<u64>,
    /// The input tokens of those same runs, and therefore the denominator
    /// [`Self::cached_input_tokens`] is a rate *of*.
    ///
    /// Carried rather than reusing [`Self::input_tokens`] because a task can
    /// mix runs that reported a split with runs that did not: dividing a
    /// partial numerator by the whole task's input would silently dilute the
    /// rate with runs nobody can see into. The same failure #19's
    /// `reported_cost` rule guards against, one field over.
    pub cache_reported_input_tokens: u64,
    /// How many contributing runs reported no split, so a surface can say what
    /// the rate covers rather than implying it covers everything.
    pub runs_without_cache_report: u32,
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
        self.append_message_with_kind(session_id, task_id, role, content, None)
    }

    /// [`Self::append_message`] for an entry that is not an ordinary turn.
    ///
    /// `kind` is written as an extra field and read back as `None` by an older
    /// log, so a message written before this existed is indistinguishable from
    /// an ordinary one — which is what it was (spec 66 §5.5).
    pub fn append_message_with_kind(
        &self,
        session_id: &str,
        task_id: Option<&str>,
        role: &str,
        content: &str,
        kind: Option<&str>,
    ) -> Result<ChatMessage> {
        let message = ChatMessage {
            id: create_id("msg"),
            session_id: session_id.to_string(),
            task_id: task_id.map(|value| value.to_string()),
            role: role.to_string(),
            content: content.to_string(),
            kind: kind.map(|value| value.to_string()),
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
        let progress = crate::worker_progress::progress_path(&self.data_dir, session_id);
        if progress.exists() {
            fs::remove_file(progress)?;
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
            .filter_map(parse_transcript_event)
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

    /// Appends one conversation compaction (spec 55 §5.3). The full transcript
    /// stays exactly as it was: this is one more event, and [`Self::read_messages`]
    /// keeps returning every message.
    pub fn append_conversation_compaction(
        &self,
        session_id: &str,
        summary: &crate::compaction::ConversationSummary,
    ) -> Result<()> {
        let payload = serde_json::to_string(summary).map_err(|error| {
            crate::error::ClientError::Io(format!("conversation summary serialization: {error}"))
        })?;
        self.append_session_event(session_id, "conversation_compacted", &payload)
    }

    /// Every active compaction, oldest first.
    ///
    /// A payload written in a format version this build does not know is
    /// ignored rather than failing the read: the log is append-only, and an
    /// unreadable summary must degrade to the older one rather than take the
    /// conversation with it.
    pub fn read_conversation_compactions(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::compaction::ConversationSummary>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(Vec::new());
        };
        let mut summaries = Vec::new();
        for event in active_events(&content) {
            if event.event_type != "conversation_compacted" {
                continue;
            }
            let Ok(summary) = serde_json::from_value::<crate::compaction::ConversationSummary>(
                event.payload.clone(),
            ) else {
                continue;
            };
            if summary.format_version > crate::compaction::SUMMARY_FORMAT_VERSION {
                continue;
            }
            summaries.push(summary);
        }
        Ok(summaries)
    }

    /// The model's view of the conversation (spec 55 §5.3): the newest active
    /// summary plus every active message after its `through_seq`.
    ///
    /// [`Self::read_messages`] is deliberately unchanged. The pair is
    /// requirement 2: compaction reduces what is *sent*, never what is stored.
    pub fn read_conversation(
        &self,
        session_id: &str,
    ) -> Result<crate::compaction::ConversationView> {
        let messages = self.read_messages_with_seq(session_id)?;
        let summary = self.read_conversation_compactions(session_id)?.pop();
        let (summarised_messages, messages) = match &summary {
            Some(summary) => {
                let (covered, after): (Vec<_>, Vec<_>) = messages
                    .into_iter()
                    .partition(|(seq, _)| *seq <= summary.through_seq);
                (
                    covered.len(),
                    after.into_iter().map(|(_, message)| message).collect(),
                )
            }
            None => (
                0,
                messages.into_iter().map(|(_, message)| message).collect(),
            ),
        };
        Ok(crate::compaction::ConversationView {
            summary,
            messages,
            summarised_messages,
        })
    }

    /// Records that a compaction attempt did not run. A diagnostic, not a
    /// compaction: no summary is appended and the transcript is untouched. It
    /// is durable so a user reopening the session can be told why the model's
    /// view begins where it does (spec 55 §5.7).
    pub fn record_conversation_compaction_failure(
        &self,
        session_id: &str,
        reason: &str,
    ) -> Result<()> {
        self.append_session_event(
            session_id,
            "conversation_compaction_failed",
            &format!("{{\"reason\":\"{}\"}}", escape_json(reason)),
        )
    }

    /// The newest recorded compaction failure, if any *since the last
    /// successful compaction*. A failure before a later success is history; the
    /// marker must describe the state the user is looking at.
    pub fn read_conversation_compaction_failure(&self, session_id: &str) -> Result<Option<String>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(None);
        };
        let mut failure: Option<(u64, String)> = None;
        let mut last_compaction = 0u64;
        for event in active_events(&content) {
            match event.event_type.as_str() {
                "conversation_compacted" => last_compaction = event.seq,
                "conversation_compaction_failed" => {
                    if let Some(reason) = event.text("reason") {
                        failure = Some((event.seq, reason));
                    }
                }
                _ => {}
            }
        }
        Ok(failure
            .filter(|(seq, _)| *seq > last_compaction)
            .map(|(_, reason)| reason))
    }

    /// The highest event `seq` a new compaction may cover today.
    ///
    /// Requirement 5 and §5.4: never across a pending decision, and never across
    /// an action whose outcome is unknown. A turn boundary is normally clear of
    /// both, but a new prompt can arrive while an older task waits, and a crash
    /// can leave an action unfinished; the clamp is what makes the guarantee
    /// independent of those accidents.
    pub fn compaction_limit(&self, session_id: &str) -> Result<u64> {
        let messages = self.read_messages_with_seq(session_id)?;
        let mut limit = messages.last().map(|(seq, _)| *seq).unwrap_or(0);
        for action in self.dangling_actions(session_id)? {
            limit = limit.min(action.seq.saturating_sub(1));
        }
        let statuses = self.read_task_statuses(session_id)?;
        for (task_id, status) in statuses {
            if status != TaskStatus::WaitingForApproval.as_str() {
                continue;
            }
            if self.pending_decision_for(session_id, &task_id)?.is_some()
                && let Some((seq, _)) = messages
                    .iter()
                    .find(|(_, message)| message.task_id.as_deref() == Some(task_id.as_str()))
            {
                limit = limit.min(seq.saturating_sub(1));
            }
        }
        Ok(limit)
    }

    /// Approval decisions recorded at or before `through_seq`, for a summary's
    /// fact half (spec 55 requirement 6).
    ///
    /// Patches and plan approval already live in the session log. Shell-command
    /// decisions do not — they are audit-log events with no session id — so
    /// [`Self::record_conversation_approval`] adds one for each decision as it
    /// happens.
    pub fn read_conversation_approvals_through(
        &self,
        session_id: &str,
        through_seq: u64,
    ) -> Result<Vec<crate::compaction::ApprovalDigest>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(Vec::new());
        };
        let mut approvals = Vec::new();
        for event in active_events(&content) {
            if event.seq > through_seq {
                continue;
            }
            match event.event_type.as_str() {
                "conversation_approval_recorded" => {
                    let Some(kind) = event.text("kind") else {
                        continue;
                    };
                    let Some(id) = event.text("id") else {
                        continue;
                    };
                    let Some(decision) = event.text("decision") else {
                        continue;
                    };
                    approvals.push(crate::compaction::ApprovalDigest {
                        kind,
                        id,
                        decision,
                        subject: event.text("subject"),
                    });
                }
                "plan_approved" => {
                    let Some(task_id) = event.text("taskId") else {
                        continue;
                    };
                    approvals.push(crate::compaction::ApprovalDigest {
                        kind: "plan".to_string(),
                        id: task_id,
                        decision: "approved".to_string(),
                        subject: None,
                    });
                }
                "patch_file_decided" => {
                    let Ok(decision) =
                        serde_json::from_value::<PatchFileDecision>(event.payload.clone())
                    else {
                        continue;
                    };
                    approvals.push(crate::compaction::ApprovalDigest {
                        kind: "patch".to_string(),
                        id: decision.patch_id,
                        decision: if decision.applied {
                            "applied".to_string()
                        } else {
                            "rejected".to_string()
                        },
                        subject: (!decision.paths.is_empty()).then(|| decision.paths.join(", ")),
                    });
                }
                _ => {}
            }
        }
        Ok(approvals)
    }

    /// Records one approval decision in the conversation log, so a later
    /// summary can read it rather than ask a model to remember it (spec 55
    /// requirement 6).
    pub fn record_conversation_approval(
        &self,
        session_id: &str,
        kind: &str,
        id: &str,
        decision: &str,
        subject: Option<&str>,
    ) -> Result<()> {
        let subject = subject
            .map(|value| format!(",\"subject\":\"{}\"", escape_json(value)))
            .unwrap_or_default();
        self.append_session_event(
            session_id,
            "conversation_approval_recorded",
            &format!(
                "{{\"kind\":\"{}\",\"id\":\"{}\",\"decision\":\"{}\"{subject}}}",
                escape_json(kind),
                escape_json(id),
                escape_json(decision)
            ),
        )
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

    /// Appends a memory record to this session's log (spec 28 §5.3).
    ///
    /// Session-scoped memory lives in the session log rather than a memory
    /// file so it cannot outlive its session. The memory store is the only
    /// production caller: it runs the secret, evidence and scope checks before
    /// reaching here. `append_session_event` stays private so a second caller
    /// cannot write a memory around those checks.
    pub(crate) fn append_memory_record(&self, session_id: &str, record: &str) -> Result<()> {
        self.append_session_event(session_id, "memory_recorded", record)
    }

    /// The memory record payloads in a session log, in append order.
    pub(crate) fn memory_records(&self, session_id: &str) -> Result<Vec<String>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(Vec::new());
        };
        Ok(active_events(&content)
            .into_iter()
            .filter(|event| event.event_type == "memory_recorded")
            .map(|event| event.payload.to_string())
            .collect())
    }

    /// Appends a proposed memory (spec 29 §5.10).
    ///
    /// A proposal is not an entry and never reaches the memory store by being
    /// read back. It lives in the session log so it survives a reload and a
    /// restart, and dies with its session. The memory store is the only
    /// production caller and has already screened the candidate.
    pub(crate) fn append_memory_proposal(&self, session_id: &str, proposal: &str) -> Result<()> {
        self.append_session_event(session_id, "memory_proposal_proposed", proposal)
    }

    /// Appends a proposal decision: confirmed or dismissed.
    pub(crate) fn append_memory_proposal_decision(
        &self,
        session_id: &str,
        decision: &str,
    ) -> Result<()> {
        self.append_session_event(session_id, "memory_proposal_decided", decision)
    }

    /// Every proposal event in a session log, in append order. The caller
    /// separates proposed from decided so the pending list is one pass and a
    /// rewind drops a proposal with the turn it belongs to.
    pub(crate) fn memory_proposal_records(
        &self,
        session_id: &str,
        event_type: &str,
    ) -> Result<Vec<String>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(Vec::new());
        };
        Ok(active_events(&content)
            .into_iter()
            .filter(|event| event.event_type == event_type)
            .map(|event| event.payload.to_string())
            .collect())
    }

    /// Adds a file or range to the context the session carries into every
    /// turn (spec 27 §5.4). Appends an event; it never rewrites the log.
    pub fn add_context_pin(
        &self,
        session_id: &str,
        path: &str,
        range: Option<LineRange>,
    ) -> Result<()> {
        self.append_session_event(
            session_id,
            "context_pin_added",
            &serde_json::json!({
                "sessionId": session_id,
                "path": path,
                "range": range,
            })
            .to_string(),
        )
    }

    /// Removes one pin, identified by `(path, range)` so a second range of the
    /// same file survives. A pair that is not pinned is a no-op.
    pub fn remove_context_pin(
        &self,
        session_id: &str,
        path: &str,
        range: Option<LineRange>,
    ) -> Result<()> {
        self.append_session_event(
            session_id,
            "context_pin_removed",
            &serde_json::json!({
                "sessionId": session_id,
                "path": path,
                "range": range,
            })
            .to_string(),
        )
    }

    pub fn clear_context_pins(&self, session_id: &str) -> Result<()> {
        self.append_session_event(
            session_id,
            "context_pins_cleared",
            &serde_json::json!({ "sessionId": session_id }).to_string(),
        )
    }

    /// The session's pins, newest-wins, in pin order.
    ///
    /// Reads `parsed_events`, not `active_events`: a rewind discards
    /// conversation, not the user's control of what may be sent — the same
    /// reasoning `session_mode` records. A missing or unreadable log is an
    /// empty list, not an error, like the browser-diagnostics approval.
    pub fn context_pins(&self, session_id: &str) -> Result<Vec<ContextPin>> {
        let content = fs::read_to_string(self.session_log_path(session_id)).unwrap_or_default();
        let mut pins: Vec<ContextPin> = Vec::new();
        for event in parsed_events(&content).0 {
            match event.event_type.as_str() {
                "context_pin_added" => {
                    let Some(pin) = context_pin_from_event(&event) else {
                        continue;
                    };
                    if !pins.contains(&pin) {
                        pins.push(pin);
                    }
                }
                "context_pin_removed" => {
                    if let Some(pin) = context_pin_from_event(&event) {
                        pins.retain(|existing| existing != &pin);
                    }
                }
                "context_pins_cleared" => pins.clear(),
                _ => {}
            }
        }
        Ok(pins)
    }

    /// Restricts the session's file-bearing context to `paths` (spec 27 §5.4).
    /// An empty list is a restriction that admits no file content, not a clear.
    pub fn set_context_restriction(&self, session_id: &str, paths: &[String]) -> Result<()> {
        self.append_session_event(
            session_id,
            "context_restriction_set",
            &serde_json::json!({
                "sessionId": session_id,
                "paths": paths,
            })
            .to_string(),
        )
    }

    pub fn clear_context_restriction(&self, session_id: &str) -> Result<()> {
        self.append_session_event(
            session_id,
            "context_restriction_cleared",
            &serde_json::json!({ "sessionId": session_id }).to_string(),
        )
    }

    /// The newest restriction, or `None` when none is set. `Some([])` is an
    /// empty restriction and excludes every file-bearing source.
    pub fn context_restriction(&self, session_id: &str) -> Result<Option<Vec<String>>> {
        let content = fs::read_to_string(self.session_log_path(session_id)).unwrap_or_default();
        let mut restriction = None;
        for event in parsed_events(&content).0 {
            match event.event_type.as_str() {
                "context_restriction_set" => {
                    restriction = Some(
                        event
                            .payload
                            .get("paths")
                            .and_then(|paths| serde_json::from_value(paths.clone()).ok())
                            .unwrap_or_default(),
                    );
                }
                "context_restriction_cleared" => restriction = None,
                _ => {}
            }
        }
        Ok(restriction)
    }

    /// The pages this session has fetched, in fetch order (spec 51 §5.4, D5).
    ///
    /// The `fetch_url` tool appends an `external_reference_fetched` event whose
    /// payload is one [`ExternalReference`] — its converted, redacted, bounded
    /// text and its provenance, never the raw body. The next assembly reads
    /// them here and carries them as the last context category. The read is
    /// `active_events`, so a rewind drops the pages of the turns it discards.
    /// A malformed payload is skipped rather than failing the turn.
    pub fn read_external_references(&self, session_id: &str) -> Result<Vec<ExternalReference>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(Vec::new());
        };
        Ok(active_events(&content)
            .into_iter()
            .filter(|event| event.event_type == "external_reference_fetched")
            .filter_map(|event| serde_json::from_value(event.payload).ok())
            .collect())
    }

    /// Records one fetched page for the next assembly (spec 51 §5.4, D5).
    ///
    /// The event payload is exactly [`ExternalReference`]'s serialised form —
    /// the converted, redacted, bounded text and its provenance, never the raw
    /// body — plus `contentHash`, the SHA-256 of that text, so a reader can
    /// tell whether the stored page still matches what the tool returned.
    /// Unknown keys are ignored by [`serde_json::from_value`] in
    /// [`Self::read_external_references`].
    pub fn record_external_reference(
        &self,
        session_id: &str,
        reference: &ExternalReference,
    ) -> Result<()> {
        let mut payload = serde_json::to_value(reference)
            .map_err(|error| ClientError::InvalidInput(error.to_string()))?;
        payload["contentHash"] =
            serde_json::json!(crate::hash::sha256_hex(reference.text.as_bytes()));
        self.append_session_event(
            session_id,
            "external_reference_fetched",
            &payload.to_string(),
        )
    }

    /// The repository paths `read_file` returned during `task_id`, in the order
    /// they were read (spec 51 D9).
    ///
    /// The egress guard compares a URL's query and fragment against the content
    /// of these files. It uses `active_events`, so a rewind discards a read
    /// whose result is no longer in the conversation. Only `read_file` is
    /// recorded: files reached through `search_content`, `read_git_diff` or a
    /// command are deliberately outside this check (`context.md` §4).
    pub fn read_files_in_task(&self, session_id: &str, task_id: &str) -> Result<Vec<String>> {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(Vec::new());
        };
        Ok(active_events(&content)
            .into_iter()
            .filter(|event| event.event_type == "action_started")
            .filter(|event| event.text("action").as_deref() == Some("read_file"))
            .filter(|event| event.text("taskId").as_deref() == Some(task_id))
            .filter_map(|event| event.text("ref"))
            .collect())
    }

    /// The newest `context_assembled` manifest per task (spec 27 §5.3).
    ///
    /// The manifest is an **audit** record, not a session event, so this reads
    /// `<data_dir>/audit/events.jsonl` and keeps the entries whose
    /// `sessionId` matches. A removed item appends a second assembly before the
    /// resumed turn sends anything, so "newest wins" is what makes the
    /// post-send view show what was actually sent rather than what was first
    /// planned. The manifest is stored as one JSON string field by
    /// `audit_fields`; a malformed record is skipped rather than failing the
    /// session read. Audit retention can remove older events, in which case an
    /// old turn simply has no stored manifest — the same shape as a session
    /// written before #26.
    pub fn read_session_context_manifests(
        &self,
        session_id: &str,
    ) -> Result<HashMap<String, crate::context_manager::ContextManifest>> {
        let path = self.data_dir.join("audit").join("events.jsonl");
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(HashMap::new());
        };
        let mut manifests = HashMap::new();
        for value in content
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        {
            if value.get("eventType").and_then(|kind| kind.as_str()) != Some("context_assembled") {
                continue;
            }
            if value.get("sessionId").and_then(|id| id.as_str()) != Some(session_id) {
                continue;
            }
            let Some(task_id) = value.get("taskId").and_then(|id| id.as_str()) else {
                continue;
            };
            let Some(raw) = value.get("manifest").and_then(|raw| raw.as_str()) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str(raw) else {
                continue;
            };
            manifests.insert(task_id.to_string(), manifest);
        }
        Ok(manifests)
    }

    /// Sets the session's working mode by appending an event; the log is
    /// never rewritten. `set_by` is always `"user"` (`proposal.md` §5.4) —
    /// the field exists so a future non-user origin cannot be introduced
    /// without someone noticing it already asserts otherwise.
    pub fn set_session_mode(
        &self,
        session_id: &str,
        mode: SessionMode,
        set_by: &str,
    ) -> Result<()> {
        self.append_session_event(
            session_id,
            "session_mode_set",
            &format!(
                "{{\"sessionId\":\"{}\",\"mode\":\"{}\",\"setBy\":\"{}\"}}",
                escape_json(session_id),
                mode.as_str(),
                escape_json(set_by)
            ),
        )
    }

    /// The session's current working mode: the newest `session_mode_set`
    /// event's mode, or `Code` (requirement 7's default) when there is none,
    /// or when the session log is missing or unreadable.
    ///
    /// Reads with `parsed_events`, not `active_events`, deliberately: mode is
    /// a capability the user configured, not conversation content, so a
    /// rewind that discards messages must not silently reset what the
    /// session is allowed to do (`docs/specs/20_working_modes/tasks.md`
    /// Task 3).
    // Task 4 calls this once per turn, before tool-list construction; Task 6
    // calls it before the first Layer-3 check.
    pub fn session_mode(&self, session_id: &str) -> SessionMode {
        let path = self.session_log_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return SessionMode::Code;
        };
        let mut mode = SessionMode::Code;
        for event in parsed_events(&content).0 {
            if event.event_type != "session_mode_set" {
                continue;
            }
            if let Some(parsed) = event
                .payload
                .get("mode")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
            {
                mode = parsed;
            }
        }
        mode
    }

    /// Appends `worker_spawned` to the worker's own log: what makes a session a
    /// worker, and the only place its worktree path is kept (spec 57 §5.1 step
    /// 7). Sessions are keyed by repository id, so the worktree cannot be
    /// recovered from the session itself.
    pub fn record_worker_spawned(
        &self,
        worker_session_id: &str,
        record: &WorkerRecord,
    ) -> Result<()> {
        let mut payload = serde_json::to_value(record)
            .map_err(|error| crate::error::ClientError::InvalidInput(error.to_string()))?;
        payload["sessionId"] = serde_json::Value::from(worker_session_id);
        self.append_session_event(worker_session_id, "worker_spawned", &payload.to_string())
    }

    /// Appends `worker_spawned` to the parent's log, carrying only the worker's
    /// session id. `workerSessionId` is what tells it apart from the worker's
    /// own record, which has the same event type.
    pub fn record_worker_child(
        &self,
        parent_session_id: &str,
        worker_session_id: &str,
    ) -> Result<()> {
        let payload = serde_json::json!({
            "sessionId": parent_session_id,
            "workerSessionId": worker_session_id,
        });
        self.append_session_event(parent_session_id, "worker_spawned", &payload.to_string())
    }

    /// Whether `session_id` is a worker whose process is running now, judged
    /// by #46's registry (spec 57 correction C12). Crash recovery leaves such a
    /// session alone: its tasks are in flight in that process.
    pub fn has_live_worker(&self, session_id: &str) -> Result<bool> {
        Ok(self.worker_record(session_id)?.is_some()
            && crate::process_registry::live_worker_registered(&self.data_dir, session_id)?)
    }

    /// The session's own worker record, or `None` for an ordinary session.
    ///
    /// Reads with `parsed_events`, not `active_events`, for the reason
    /// [`Self::session_mode`] does: being a worker is what the session is, not
    /// conversation content, and a rewind must not turn a worker back into a
    /// session that is offered handoffs.
    pub fn worker_record(&self, session_id: &str) -> Result<Option<WorkerRecord>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(None);
        };
        Ok(parsed_events(&content)
            .0
            .iter()
            .filter(|event| event.event_type == "worker_spawned")
            .filter(|event| event.payload.get("workerSessionId").is_none())
            .filter_map(|event| serde_json::from_value(event.payload.clone()).ok())
            .next_back())
    }

    /// The workers started from this session, oldest first.
    pub fn worker_children(&self, parent_session_id: &str) -> Result<Vec<String>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(parent_session_id)) else {
            return Ok(Vec::new());
        };
        let mut children: Vec<String> = Vec::new();
        for event in parsed_events(&content).0 {
            if event.event_type != "worker_spawned" {
                continue;
            }
            if let Some(child) = event.text("workerSessionId")
                && !children.contains(&child)
            {
                children.push(child);
            }
        }
        Ok(children)
    }

    /// Every worker session whose record names `source_repository`, across all
    /// repository ids: each worktree is its own repository id, so the source
    /// is the only thing a project's workers share. Compared as given, so
    /// callers pass the same canonical form they recorded.
    pub fn worker_sessions_for_source(&self, source_repository: &Path) -> Result<Vec<Session>> {
        let mut workers = Vec::new();
        for session in self.list_sessions(None)? {
            if self
                .worker_record(&session.id)?
                .is_some_and(|record| record.source_repository == source_repository)
            {
                workers.push(session);
            }
        }
        Ok(workers)
    }

    /// Marks an ordinary user message as a steering note (spec 57 §5.4). A
    /// marker event rather than a field on `ChatMessage`: adding an origin
    /// field would change every message's serialized form for one flag.
    pub fn record_steering_note(&self, session_id: &str, message_id: &str) -> Result<()> {
        let payload = serde_json::json!({
            "sessionId": session_id,
            "messageId": message_id,
        });
        self.append_session_event(session_id, "steering_note_added", &payload.to_string())
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

    /// Sets [`TaskStatus::WaitingForApproval`] for a task waiting on the user's
    /// answer to `question` (spec 50 §5.2). The link goes under its own key,
    /// `pendingQuestion`, never `pendingApproval`, so [`Self::pending_approval_for`]
    /// can never return a question.
    pub fn await_answer(&self, task: &Task, question: &PendingQuestionRef) -> Result<Task> {
        if self
            .question_state(&task.session_id, question.question_id())?
            .is_none()
        {
            return Err(crate::error::ClientError::InvalidInput(format!(
                "Unknown question: {}",
                question.question_id()
            )));
        }
        let mut updated = task.clone();
        updated.status = TaskStatus::WaitingForApproval;
        let payload = format!(
            "{{\"task\":{},\"pendingQuestion\":{{\"questionId\":\"{}\"}}}}",
            task_json(&updated),
            escape_json(question.question_id())
        );
        self.append_session_event(&task.session_id, "task_status_updated", &payload)?;
        Ok(updated)
    }

    /// What the task is waiting on, from the newest status event that records
    /// it. `None` when it is waiting on nothing, or when the newest status
    /// event carries no link.
    pub fn pending_decision_for(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<Option<PendingDecision>> {
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
            found = if let Some(pending) = event.payload.get("pendingApproval") {
                (|| {
                    Some(PendingDecision::Approval(PendingApprovalRef {
                        kind: pending.get("kind")?.as_str()?.to_string(),
                        proposal_id: pending.get("proposalId")?.as_str()?.to_string(),
                    }))
                })()
            } else {
                event
                    .payload
                    .get("pendingQuestion")
                    .and_then(|pending| pending.get("questionId")?.as_str())
                    .map(|id| {
                        PendingDecision::Question(PendingQuestionRef::recorded(id.to_string()))
                    })
            };
        }
        Ok(found)
    }

    /// Records a question the model asked (spec 50 §5.6). The input must
    /// already be redacted: the chat loop runs the secret scanner before it
    /// gets here. An input that breaks a bound is refused, not stored.
    pub fn record_question_asked(
        &self,
        session_id: &str,
        task_id: &str,
        question: &PendingQuestionRef,
        input: &AskUserInput,
    ) -> Result<()> {
        input
            .validate()
            .map_err(crate::error::ClientError::InvalidInput)?;
        let payload = serde_json::json!({
            "questionId": question.question_id(),
            "sessionId": session_id,
            "taskId": task_id,
            "question": input.question,
            "options": input.options,
            "allowFreeText": input.allow_free_text,
            "createdAtMs": now_millis() as u64,
        });
        self.append_session_event(session_id, "question_asked", &payload.to_string())
    }

    /// Records one change to a turn's notes (spec 65 §5.6).
    ///
    /// The full text after the change is stored, already redacted by the chat
    /// loop; `chars` is the size of the change and `total_chars` the running
    /// record. The latest event for a task is that task's notes, and reading
    /// goes through `active_events`, so a rewind drops the notes with the turn
    /// they belong to.
    pub fn record_turn_notes(
        &self,
        session_id: &str,
        task_id: &str,
        replace: bool,
        carried: bool,
        text: &str,
        chars: usize,
    ) -> Result<()> {
        let payload = serde_json::json!({
            "taskId": task_id,
            "replace": replace,
            "carried": carried,
            "text": text,
            "chars": chars,
            "totalChars": text.chars().count(),
        });
        self.append_session_event(session_id, "turn_notes", &payload.to_string())
    }

    /// Every task's latest notes in a session (spec 65 §5.6), keyed by task id.
    ///
    /// Reads `active_events`, so after a rewind the notes of a rewound-away
    /// turn are gone and the turn before it is the previous turn — the same
    /// rule `read_tasks` and `read_messages` already follow.
    pub fn read_turn_notes(&self, session_id: &str) -> Result<BTreeMap<String, TurnNotes>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(BTreeMap::new());
        };
        let mut notes = BTreeMap::new();
        for event in active_events(&content) {
            if event.event_type != "turn_notes" {
                continue;
            }
            let (Some(task_id), Some(text)) = (event.text("taskId"), event.text("text")) else {
                continue;
            };
            notes.insert(
                task_id,
                TurnNotes {
                    text,
                    carried: event
                        .payload
                        .get("carried")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    chars: event
                        .number("chars")
                        .and_then(|value| usize::try_from(value).ok())
                        .unwrap_or(0),
                    total_chars: event
                        .number("totalChars")
                        .and_then(|value| usize::try_from(value).ok())
                        .unwrap_or(0),
                },
            );
        }
        Ok(notes)
    }

    /// Records the user's answer. Refused when the question is unknown,
    /// already answered, or the answer does not fit it. An answered question
    /// is never re-asked, and is never answered twice either (requirement 4).
    pub fn record_question_answered(
        &self,
        session_id: &str,
        question_id: &str,
        answer: &str,
        selected_option: Option<usize>,
    ) -> Result<QuestionState> {
        let Some(mut state) = self.question_state(session_id, question_id)? else {
            return Err(crate::error::ClientError::InvalidInput(format!(
                "Unknown question: {question_id}"
            )));
        };
        if state.answer.is_some() {
            return Err(crate::error::ClientError::InvalidInput(format!(
                "Question {question_id} is already answered"
            )));
        }
        validate_answer(&state.input(), answer, selected_option)
            .map_err(crate::error::ClientError::InvalidInput)?;
        let mut payload = serde_json::json!({
            "id": create_id("msg"),
            "questionId": question_id,
            "sessionId": session_id,
            "taskId": state.task_id,
            "answer": answer,
            "createdAtMs": now_millis() as u64,
        });
        if let Some(index) = selected_option {
            payload["selectedOption"] = serde_json::json!(index);
        }
        self.append_session_event(session_id, "question_answered", &payload.to_string())?;
        state.answer = Some((answer.to_string(), selected_option));
        Ok(state)
    }

    /// A question and its answer, replayed from the active conversation. A
    /// question rewound past is `None`.
    pub fn question_state(
        &self,
        session_id: &str,
        question_id: &str,
    ) -> Result<Option<QuestionState>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(None);
        };
        let mut state: Option<QuestionState> = None;
        for event in active_events(&content) {
            if event.text("questionId").as_deref() != Some(question_id) {
                continue;
            }
            match event.event_type.as_str() {
                "question_asked" => state = parse_question_asked(&event),
                "question_answered" => {
                    if let (Some(state), Some(answer)) = (state.as_mut(), event.text("answer")) {
                        let selected = event
                            .payload
                            .get("selectedOption")
                            .and_then(|value| value.as_u64())
                            .and_then(|value| usize::try_from(value).ok());
                        state.answer = Some((answer, selected));
                    }
                }
                _ => {}
            }
        }
        Ok(state)
    }

    /// How many questions a task has asked, for the per-turn cap (§5.3). Read
    /// from the log, so a resumed turn counts what it already asked.
    pub fn questions_asked_in_task(&self, session_id: &str, task_id: &str) -> Result<usize> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(0);
        };
        Ok(active_events(&content)
            .iter()
            .filter(|event| event.event_type == "question_asked")
            .filter(|event| event.text("taskId").as_deref() == Some(task_id))
            .count())
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
        // Omitted entirely when the provider reported no split, rather than
        // written as a zero. The reader defaults a missing key to "not
        // reported", so an event written today by a provider that says nothing
        // is indistinguishable from one written before the field existed —
        // which is correct, because both mean the same thing. Spec 49 §5.2.
        let cached = match usage.cached_input_tokens {
            Some(cached) => format!(",\"cachedInputTokens\":{cached}"),
            None => String::new(),
        };
        self.append_session_event(
            session_id,
            "task_usage_recorded",
            &format!(
                "{{\"taskId\":\"{}\",\"runId\":\"{}\"{},\"inputTokens\":{},\"outputTokens\":{}{},\"source\":\"{}\",\"reportedCost\":{}{}}}",
                escape_json(task_id),
                escape_json(run_id),
                marker,
                usage.input_tokens,
                usage.output_tokens,
                cached,
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
            // Absent means the provider reported no split. Spec 49's rule that
            // silence is not zero, at the read boundary: an event written
            // before the field existed and one written today by a provider
            // that says nothing are the same fact and read the same way.
            let cached = event
                .payload
                .get("cachedInputTokens")
                .and_then(|value| value.as_u64());

            let costed = every_run_costed.entry(task_id.clone()).or_insert(true);
            *costed = *costed && cost.is_some();

            let total = totals.entry(task_id).or_insert(TaskUsage {
                input_tokens: 0,
                output_tokens: 0,
                source: UsageSource::Measured,
                reported_cost: None,
                run_count: 0,
                cached_input_tokens: None,
                cache_reported_input_tokens: 0,
                runs_without_cache_report: 0,
            });
            total.input_tokens += input;
            total.output_tokens += output;
            total.run_count += 1;
            match cached {
                Some(cached) => {
                    total.cached_input_tokens =
                        Some(total.cached_input_tokens.unwrap_or(0) + cached);
                    total.cache_reported_input_tokens += input;
                }
                None => total.runs_without_cache_report += 1,
            }
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

    /// Appends one browser diagnostic run (spec 12 `context.md` §3.3). The
    /// record must already be redacted: this store writes what it is given.
    pub fn append_web_diagnostic(
        &self,
        session_id: &str,
        record: &crate::web_diagnostics::WebDiagnosticRecord,
    ) -> Result<()> {
        let payload = serde_json::to_string(record).map_err(|error| {
            crate::error::ClientError::Io(format!("web diagnostic serialization: {error}"))
        })?;
        self.append_session_event(session_id, "web_diagnostic_recorded", &payload)
    }

    /// Every task's diagnostic runs, keyed by task id, in log order. Reads
    /// `active_events`, so a rewound turn's diagnostics go with it, as its
    /// plan does.
    pub fn read_session_web_diagnostics(
        &self,
        session_id: &str,
    ) -> Result<HashMap<String, Vec<crate::web_diagnostics::WebDiagnosticRecord>>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(HashMap::new());
        };
        let mut records: HashMap<String, Vec<crate::web_diagnostics::WebDiagnosticRecord>> =
            HashMap::new();
        for event in active_events(&content) {
            if event.event_type != "web_diagnostic_recorded" {
                continue;
            }
            if let Ok(record) =
                serde_json::from_value::<crate::web_diagnostics::WebDiagnosticRecord>(event.payload)
            {
                records
                    .entry(record.task_id.clone())
                    .or_default()
                    .push(record);
            }
        }
        Ok(records)
    }

    /// Appends one patch-file decision (spec 23 `context.md` §1). The apply and
    /// reject paths call this even when the decision will not settle the patch,
    /// so a later decision can complete the set.
    pub fn append_patch_file_decision(
        &self,
        session_id: &str,
        decision: &PatchFileDecision,
    ) -> Result<()> {
        let payload = serde_json::to_string(decision).map_err(|error| {
            crate::error::ClientError::Io(format!("patch file decision serialization: {error}"))
        })?;
        self.append_session_event(session_id, "patch_file_decided", &payload)
    }

    /// Every decision this task recorded, in log order. The caller filters by
    /// patch to decide whether that patch is settled.
    pub fn read_patch_file_decisions(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<Vec<PatchFileDecision>> {
        Ok(self
            .read_all_patch_file_decisions(session_id)?
            .into_iter()
            .filter(|decision| decision.task_id == task_id)
            .collect())
    }

    /// Appends the record that a commit landed (spec 35 §5.7). The paths leave
    /// the session's uncommitted accepted set; the SHA is read back by
    /// inspection and the UI.
    pub fn append_commit_created(
        &self,
        session_id: &str,
        event: &CommitSessionEvent,
    ) -> Result<()> {
        let payload = serde_json::to_string(event).map_err(|error| {
            crate::error::ClientError::Io(format!("commit session event serialization: {error}"))
        })?;
        self.append_session_event(session_id, "commit_created", &payload)
    }

    /// The session's applied decisions and commits in log order, so a later
    /// applied change to a committed path is pending again while a commit
    /// removes its paths from the pending set. Read in one pass: interleaving
    /// two separate readers would lose the order that answers the question.
    pub fn read_change_timeline(&self, session_id: &str) -> Result<Vec<ChangeEvent>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(Vec::new());
        };
        let mut timeline = Vec::new();
        for event in active_events(&content) {
            match event.event_type.as_str() {
                "patch_file_decided" => {
                    let Ok(decision) =
                        serde_json::from_value::<PatchFileDecision>(event.payload.clone())
                    else {
                        continue;
                    };
                    if decision.applied {
                        timeline.push(ChangeEvent::Applied {
                            paths: decision.paths,
                            applied_hashes: decision.applied_hashes,
                            accepted_hunks: decision.accepted_hunks,
                        });
                    }
                }
                "commit_created" => {
                    let Ok(commit) =
                        serde_json::from_value::<CommitSessionEvent>(event.payload.clone())
                    else {
                        continue;
                    };
                    timeline.push(ChangeEvent::Committed {
                        paths: commit.paths,
                    });
                }
                _ => {}
            }
        }
        Ok(timeline)
    }

    /// Every patch-file decision in the session, in log order.
    ///
    /// The verification loop needs this rather than one task's decisions: a
    /// verification chain spans the task that proposed each patch, and the
    /// files a later attempt must re-check are the union of everything the
    /// conversation applied, not only the patch that made it due.
    pub fn read_all_patch_file_decisions(
        &self,
        session_id: &str,
    ) -> Result<Vec<PatchFileDecision>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(Vec::new());
        };
        let mut decisions = Vec::new();
        for event in active_events(&content) {
            if event.event_type != "patch_file_decided" {
                continue;
            }
            let Ok(decision) = serde_json::from_value::<PatchFileDecision>(event.payload.clone())
            else {
                continue;
            };
            decisions.push(decision);
        }
        Ok(decisions)
    }

    /// Appends one verification event (spec 23 §5.1). One session-event kind
    /// per variant, so a reader of the log can tell what appended it.
    pub fn append_verification_event(
        &self,
        session_id: &str,
        event: &crate::verification::VerificationEvent,
    ) -> Result<()> {
        use crate::verification::VerificationEvent;
        let event_type = match event {
            VerificationEvent::Due { .. } => "verification_due",
            VerificationEvent::Started { .. } => "verification_started",
            VerificationEvent::Check { .. } => "verification_check",
            VerificationEvent::Finished { .. } => "verification_finished",
        };
        let payload = serde_json::to_string(event).map_err(|error| {
            crate::error::ClientError::Io(format!("verification event serialization: {error}"))
        })?;
        self.append_session_event(session_id, event_type, &payload)
    }

    /// Every verification event in the session belonging to one of
    /// `task_ids`, in log order. An empty `task_ids` means every task, which is
    /// what a verification turn needs to find the due event before it knows
    /// which task proposed the patch.
    ///
    /// Takes a task chain because each verification turn is a new task carrying
    /// the plan: the `Due` names the task that proposed the patch, while the
    /// `Started`/`Check`/`Finished` name the verification turn's own task
    /// (spec 23 `context.md` §1). Unreadable payloads are skipped rather than
    /// failing the replay, matching the other readers here.
    pub fn read_verification_events(
        &self,
        session_id: &str,
        task_ids: &[&str],
    ) -> Result<Vec<crate::verification::VerificationEvent>> {
        use crate::verification::VerificationEvent;
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(Vec::new());
        };
        let mut events = Vec::new();
        for event in active_events(&content) {
            if !matches!(
                event.event_type.as_str(),
                "verification_due"
                    | "verification_started"
                    | "verification_check"
                    | "verification_finished"
            ) {
                continue;
            }
            let Ok(parsed) = serde_json::from_value::<VerificationEvent>(event.payload.clone())
            else {
                continue;
            };
            if task_ids.is_empty() || task_ids.iter().any(|id| *id == parsed.task_id()) {
                events.push(parsed);
            }
        }
        Ok(events)
    }

    /// Appends one finding (spec 22 §5.7). A `Finding` is redacted by
    /// construction, so this store writes it as given.
    pub fn record_finding(
        &self,
        session_id: &str,
        finding: &crate::finding::Finding,
    ) -> Result<()> {
        let payload = serde_json::to_string(finding).map_err(|error| {
            crate::error::ClientError::Io(format!("finding serialization: {error}"))
        })?;
        self.append_session_event(session_id, "finding_recorded", &payload)
    }

    /// Records the user's decision about a finding. `Stale` is derived on
    /// read and never stored, and an id this session never recorded is
    /// refused before anything is appended (spec 22 `context.md` §13.2).
    pub fn set_finding_status(
        &self,
        session_id: &str,
        finding_id: &str,
        status: crate::finding::FindingStatus,
    ) -> Result<()> {
        if status == crate::finding::FindingStatus::Stale {
            return Err(crate::error::ClientError::InvalidInput(
                "a finding becomes stale when its file changes; it cannot be set stale".to_string(),
            ));
        }
        let content = fs::read_to_string(self.session_log_path(session_id)).unwrap_or_default();
        if !replay_findings(&content)
            .iter()
            .any(|finding| finding.id() == finding_id)
        {
            return Err(crate::error::ClientError::InvalidInput(format!(
                "no finding {finding_id} in session {session_id}"
            )));
        }
        let payload = serde_json::json!({ "findingId": finding_id, "status": status }).to_string();
        self.append_session_event(session_id, "finding_status_changed", &payload)
    }

    /// Every finding in the session, in record order, with the newest status
    /// applied and `Stale` derived against `repository_root` (spec 22
    /// `context.md` §1, §13.2). Only an `Open` finding is checked.
    pub fn read_findings(
        &self,
        session_id: &str,
        repository_root: &Path,
    ) -> Result<Vec<crate::finding::Finding>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(Vec::new());
        };
        let mut findings = replay_findings(&content);
        for finding in &mut findings {
            if finding.status() == crate::finding::FindingStatus::Open
                && finding_is_stale(finding, repository_root)
            {
                finding.set_status(crate::finding::FindingStatus::Stale);
            }
        }
        Ok(findings)
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

    /// Records that the user let this task's refused model call be answered by
    /// a different provider (spec 56 §5.4). Its own event, like
    /// [`Self::approve_plan`]: a switch is a decision taken at a moment, and a
    /// turn whose second half was answered by another model must say so after
    /// a reload too.
    pub fn record_provider_fallback(
        &self,
        task: &Task,
        refused_provider: &str,
        fallback_provider: &str,
        approved_by: &str,
    ) -> Result<()> {
        self.append_session_event(
            &task.session_id,
            "provider_fallback",
            &format!(
                "{{\"taskId\":\"{}\",\"refusedProvider\":\"{}\",\"fallbackProvider\":\"{}\",\"approvedBy\":\"{}\"}}",
                escape_json(&task.id),
                escape_json(refused_provider),
                escape_json(fallback_provider),
                escape_json(approved_by)
            ),
        )
    }

    /// Every provider switch recorded on this session's tasks, as
    /// `(task_id, refused_provider, fallback_provider)` in log order.
    ///
    /// All events, not only active ones, like [`Self::read_task_usage`]: a
    /// rewind moves the conversation back, but it does not un-happen which
    /// model answered a call that was billed.
    pub fn read_provider_fallbacks(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, String, String)>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(Vec::new());
        };
        Ok(parsed_events(&content)
            .0
            .iter()
            .filter(|event| event.event_type == "provider_fallback")
            .filter_map(|event| {
                Some((
                    event.text("taskId")?,
                    event.text("refusedProvider")?,
                    event.text("fallbackProvider")?,
                ))
            })
            .collect())
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
        let notes = self.read_turn_notes(session_id)?;

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
                    if let Some(notes) = notes.get(&task.id) {
                        let carried = if notes.carried {
                            " (carried from the previous turn)"
                        } else {
                            ""
                        };
                        out.push_str(&format!("  - Notes{carried}:\n"));
                        for line in redact(&notes.text).lines() {
                            out.push_str(&format!("    {line}\n"));
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
                            "notes": notes.get(&task.id).map(|notes| serde_json::json!({
                                "text": redact(&notes.text),
                                "carried": notes.carried,
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
        "{{\"id\":\"{}\",\"sessionId\":\"{}\",\"taskId\":{},\"role\":\"{}\",\"content\":\"{}\",\"createdAtMs\":{}{}}}",
        escape_json(&message.id),
        escape_json(&message.session_id),
        message
            .task_id
            .as_ref()
            .map(|value| format!("\"{}\"", escape_json(value)))
            .unwrap_or_else(|| "null".to_string()),
        escape_json(&message.role),
        escape_json(&message.content),
        message.created_at_ms,
        message
            .kind
            .as_ref()
            .map(|value| format!(",\"kind\":\"{}\"", escape_json(value)))
            .unwrap_or_default()
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

/// The pin a `context_pin_added`/`context_pin_removed` payload names.
///
/// `None` for a payload with no path, so a malformed event is skipped rather
/// than becoming a pin on an empty path.
fn context_pin_from_event(event: &SessionEvent) -> Option<ContextPin> {
    let path = event.text("path")?;
    if path.trim().is_empty() {
        return None;
    }
    let range = event
        .payload
        .get("range")
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    Some(ContextPin { path, range })
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

/// Findings as recorded, with their newest status change applied. Staleness
/// is not derived here, so `set_finding_status` can use this to check that
/// an id exists.
fn replay_findings(content: &str) -> Vec<crate::finding::Finding> {
    let mut findings: Vec<crate::finding::Finding> = Vec::new();
    for event in active_events(content) {
        match event.event_type.as_str() {
            "finding_recorded" => {
                if let Ok(finding) =
                    serde_json::from_value::<crate::finding::Finding>(event.payload)
                    && !findings.iter().any(|known| known.id() == finding.id())
                {
                    findings.push(finding);
                }
            }
            "finding_status_changed" => {
                let id = event
                    .payload
                    .get("findingId")
                    .and_then(serde_json::Value::as_str);
                let status = event.payload.get("status").cloned().and_then(|status| {
                    serde_json::from_value::<crate::finding::FindingStatus>(status).ok()
                });
                if let (Some(id), Some(status)) = (id, status)
                    && let Some(finding) = findings.iter_mut().find(|finding| finding.id() == id)
                {
                    finding.set_status(status);
                }
            }
            _ => {}
        }
    }
    findings
}

/// Stale only when a hash was recorded and the file now differs or is
/// gone. No range or no hash means it cannot be judged (`context.md` §1).
fn finding_is_stale(finding: &crate::finding::Finding, repository_root: &Path) -> bool {
    let (Some(range), Some(recorded)) = (finding.range(), finding.file_hash()) else {
        return false;
    };
    match crate::hash::file_hash(repository_root.join(&range.path)) {
        Ok(current) => current != recorded,
        Err(_) => true,
    }
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
        kind: event.text("kind"),
        created_at_ms: event.number("createdAtMs")?,
    })
}

/// The active conversation's messages with their event `seq`, for export's JSON
/// and for a search hit's anchor. Mirrors [`SessionStore::read_messages`] but
/// keeps the `seq` that method drops.
fn messages_with_seq(content: &str) -> Vec<(u64, ChatMessage)> {
    active_events(content)
        .iter()
        .filter_map(|event| parse_transcript_event(event).map(|message| (event.seq, message)))
        .collect()
}

/// An event that belongs in the transcript, as a message. Besides ordinary
/// messages, a question the assistant asked reads as role `question` and the
/// user's answer as role `answer`, so the exchange shows as what it was
/// (spec 50 requirement 3, `context.md` §6).
fn parse_transcript_event(event: &SessionEvent) -> Option<ChatMessage> {
    match event.event_type.as_str() {
        "message_appended" => parse_message_event(event),
        "question_asked" => Some(ChatMessage {
            id: event.text("questionId")?,
            session_id: event.text("sessionId")?,
            task_id: event.text("taskId"),
            role: "question".to_string(),
            content: event.text("question")?,
            kind: None,
            created_at_ms: event.number("createdAtMs")?,
        }),
        "question_answered" => Some(ChatMessage {
            id: event.text("id")?,
            session_id: event.text("sessionId")?,
            task_id: event.text("taskId"),
            role: "answer".to_string(),
            content: event.text("answer")?,
            kind: None,
            created_at_ms: event.number("createdAtMs")?,
        }),
        _ => None,
    }
}

fn parse_question_asked(event: &SessionEvent) -> Option<QuestionState> {
    let options = event
        .payload
        .get("options")?
        .as_array()?
        .iter()
        .map(|option| option.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    Some(QuestionState {
        question_id: event.text("questionId")?,
        task_id: event.text("taskId")?,
        question: event.text("question")?,
        options,
        allow_free_text: event
            .payload
            .get("allowFreeText")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        answer: None,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::SessionMode;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_data_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "damaian-session-mode-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Requirement 7's migration criterion: a session with no
    /// `session_mode_set` event at all — the shape of every session that
    /// existed before this feature — reads as `Code`.
    #[test]
    fn a_session_with_no_mode_event_reads_as_code() {
        let store = SessionStore::new(temp_data_dir("no-event"));
        let session = store.create_session("repo_1", "Untouched").unwrap();

        assert_eq!(store.session_mode(&session.id), SessionMode::Code);
    }

    #[test]
    fn set_session_mode_round_trips() {
        let store = SessionStore::new(temp_data_dir("round-trip"));
        let session = store.create_session("repo_1", "Switched").unwrap();

        store
            .set_session_mode(&session.id, SessionMode::Ask, "user")
            .unwrap();

        assert_eq!(store.session_mode(&session.id), SessionMode::Ask);
    }

    #[test]
    fn the_newest_mode_event_wins() {
        let store = SessionStore::new(temp_data_dir("newest-wins"));
        let session = store.create_session("repo_1", "Toggled").unwrap();

        store
            .set_session_mode(&session.id, SessionMode::Plan, "user")
            .unwrap();
        store
            .set_session_mode(&session.id, SessionMode::Review, "user")
            .unwrap();

        assert_eq!(store.session_mode(&session.id), SessionMode::Review);
    }

    /// `proposal.md` §5.4: the field exists so a future non-user origin
    /// cannot be added without someone noticing it already asserts
    /// otherwise — assert what actually lands on disk, not just the
    /// argument passed in.
    #[test]
    fn set_by_is_always_user() {
        let store = SessionStore::new(temp_data_dir("set-by"));
        let session = store.create_session("repo_1", "Attributed").unwrap();

        store
            .set_session_mode(&session.id, SessionMode::Ask, "user")
            .unwrap();

        let log = fs::read_to_string(store.session_log_path(&session.id)).unwrap();
        assert!(log.contains("\"setBy\":\"user\""));
    }

    /// Spec 66 `context.md` D10: the tag is what tells the conversation view
    /// and the next turn's seed that the entry is the engine's nudge. A line
    /// written before the field existed must read as an ordinary message.
    #[test]
    fn a_message_with_a_kind_round_trips_and_an_old_one_reads_none() {
        let store = SessionStore::new(temp_data_dir("message-kind"));
        let session = store.create_session("repo_1", "Kind").unwrap();

        store
            .append_message_with_kind(
                &session.id,
                None,
                "user",
                "Your plan still has open steps.",
                Some("plan_reminder"),
            )
            .unwrap();
        store
            .append_message(&session.id, None, "user", "hello")
            .unwrap();

        let messages = store.read_messages(&session.id).unwrap();
        assert_eq!(
            messages[0].kind.as_deref(),
            Some("plan_reminder"),
            "the tag round-trips"
        );
        assert_eq!(messages[1].kind, None, "an ordinary message has no tag");

        // A message appended before the field existed: no `kind` key at all.
        let path = store.session_log_path(&session.id);
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(
            file,
            "{{\"seq\":99,\"eventType\":\"message_appended\",\"payload\":{{\"id\":\"msg_old\",\"sessionId\":\"{}\",\"taskId\":null,\"role\":\"user\",\"content\":\"old\",\"createdAtMs\":1}}}}",
            session.id
        )
        .unwrap();

        let messages = store.read_messages(&session.id).unwrap();
        let old = messages
            .iter()
            .find(|message| message.id == "msg_old")
            .expect("the old line parses");
        assert_eq!(old.kind, None);
    }

    /// Mode is a capability the user configured, not conversation content:
    /// a rewind that discards messages must not silently reset what the
    /// session is allowed to do. This is the test that would fail if a
    /// future edit switched `session_mode`'s reader to `active_events`.
    #[test]
    fn session_mode_survives_a_conversation_rewind() {
        let store = SessionStore::new(temp_data_dir("rewind"));
        let session = store.create_session("repo_1", "Rewound").unwrap();
        let rewind_to = store.latest_event_seq(&session.id).unwrap();

        store
            .set_session_mode(&session.id, SessionMode::Ask, "user")
            .unwrap();
        store.rewind_conversation(&session.id, rewind_to).unwrap();

        assert_eq!(store.session_mode(&session.id), SessionMode::Ask);
    }

    fn web_record(id: &str, task_id: &str, page_error: &str) -> crate::WebDiagnosticRecord {
        let json = serde_json::json!({
            "url": "http://localhost:5001/",
            "status": 200,
            "page_errors": [page_error],
        });
        crate::WebDiagnosticRecord {
            id: id.to_string(),
            task_id: task_id.to_string(),
            tool: "inspect_web_page".to_string(),
            url: "http://localhost:5001/".to_string(),
            recorded_at_ms: 1,
            report: crate::WebDiagnosticReport::from_text(json.to_string(), false),
        }
    }

    /// `context.md` §3.3: diagnostics replay per task in log order, and a
    /// rewound turn's diagnostics go with it, as its plan does.
    #[test]
    fn web_diagnostics_replay_per_task_in_order_and_drop_with_a_rewind() {
        let store = SessionStore::new(temp_data_dir("web-diagnostics"));
        let session = store.create_session("repo_1", "Diagnosed").unwrap();
        let task_a = store
            .create_task(&session.id, "why is the page blank?", "mock", "m")
            .unwrap();
        let task_b = store
            .create_task(&session.id, "and the login form?", "mock", "m")
            .unwrap();
        let r1 = web_record("webdiagrec_1", &task_a.id, "first");
        let r2 = web_record("webdiagrec_2", &task_b.id, "second");
        let r3 = web_record("webdiagrec_3", &task_a.id, "third");

        store.append_web_diagnostic(&session.id, &r1).unwrap();
        store.append_web_diagnostic(&session.id, &r2).unwrap();
        let before_r3 = store.latest_event_seq(&session.id).unwrap();
        store.append_web_diagnostic(&session.id, &r3).unwrap();

        let all = store.read_session_web_diagnostics(&session.id).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[&task_a.id], vec![r1.clone(), r3]);
        assert_eq!(all[&task_b.id], vec![r2]);

        store.rewind_conversation(&session.id, before_r3).unwrap();

        let rewound = store.read_session_web_diagnostics(&session.id).unwrap();
        assert_eq!(rewound[&task_a.id], vec![r1]);
    }

    #[test]
    fn a_session_with_no_log_has_no_web_diagnostics() {
        let store = SessionStore::new(temp_data_dir("web-diagnostics-missing"));

        assert!(
            store
                .read_session_web_diagnostics("never_created")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_unreadable_session_log_reads_as_code() {
        let store = SessionStore::new(temp_data_dir("missing"));

        assert_eq!(store.session_mode("never_created"), SessionMode::Code);
    }

    use crate::finding::{
        Finding, FindingDraft, FindingSource, FindingStatus, Severity, SourceRange,
    };

    fn repo_with(name: &str, file: &str, content: &str) -> PathBuf {
        let root = temp_data_dir(&format!("findings-repo-{name}"));
        fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        fs::write(root.join(file), content).unwrap();
        root
    }

    fn finding(summary: &str, range: Option<&str>) -> Finding {
        Finding::new(
            FindingDraft {
                source: FindingSource::Compiler,
                severity: Severity::Error,
                summary: summary.to_string(),
                details: None,
                range: range.map(|path| SourceRange {
                    path: path.to_string(),
                    start_line: 1,
                    start_column: None,
                    end_line: None,
                    end_column: None,
                }),
                code: None,
            },
            &SecretScanner::default(),
        )
    }

    /// A finding on `file` with the file's current hash, as Task 8 records it.
    fn hashed_finding(root: &Path, file: &str) -> Finding {
        finding("mismatched types", Some(file))
            .with_file_hash(crate::hash::file_hash(root.join(file)).unwrap())
    }

    fn store_and_session(name: &str) -> (SessionStore, Session) {
        let store = SessionStore::new(temp_data_dir(&format!("findings-{name}")));
        let session = store.create_session("repo_1", "Findings").unwrap();
        (store, session)
    }

    #[test]
    fn a_session_without_findings_reads_empty() {
        let (store, session) = store_and_session("empty");
        assert!(
            store
                .read_findings(&session.id, Path::new("/nonexistent"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_recorded_finding_reads_back_unchanged() {
        let (store, session) = store_and_session("round-trip");
        let recorded = finding("it broke", None)
            .with_task_id("task_1")
            .with_origin_ref("cmd_1");
        store.record_finding(&session.id, &recorded).unwrap();
        let read = store
            .read_findings(&session.id, Path::new("/nonexistent"))
            .unwrap();
        assert_eq!(read, vec![recorded]);
    }

    #[test]
    fn findings_read_back_in_record_order() {
        let (store, session) = store_and_session("order");
        for summary in ["first", "second", "third"] {
            store
                .record_finding(&session.id, &finding(summary, None))
                .unwrap();
        }
        let read = store
            .read_findings(&session.id, Path::new("/nonexistent"))
            .unwrap();
        let summaries: Vec<_> = read.iter().map(Finding::summary).collect();
        assert_eq!(summaries, ["first", "second", "third"]);
    }

    #[test]
    fn the_newest_status_change_wins() {
        let (store, session) = store_and_session("newest");
        let recorded = finding("x", None);
        store.record_finding(&session.id, &recorded).unwrap();
        let status = |store: &SessionStore| {
            store
                .read_findings(&session.id, Path::new("/nonexistent"))
                .unwrap()[0]
                .status()
        };
        store
            .set_finding_status(&session.id, recorded.id(), FindingStatus::Dismissed)
            .unwrap();
        assert_eq!(status(&store), FindingStatus::Dismissed);
        store
            .set_finding_status(&session.id, recorded.id(), FindingStatus::Fixed)
            .unwrap();
        assert_eq!(status(&store), FindingStatus::Fixed);
        store
            .set_finding_status(&session.id, recorded.id(), FindingStatus::Open)
            .unwrap();
        assert_eq!(status(&store), FindingStatus::Open);
    }

    /// Acceptance criterion: findings survive a restart with their statuses.
    #[test]
    fn findings_and_their_statuses_survive_a_new_store_over_the_same_data_dir() {
        let data_dir = temp_data_dir("findings-restart");
        let first = SessionStore::new(&data_dir);
        let session = first.create_session("repo_1", "Before restart").unwrap();
        let kept = finding("kept", None);
        let dismissed = finding("dismissed", None);
        first.record_finding(&session.id, &kept).unwrap();
        first.record_finding(&session.id, &dismissed).unwrap();
        first
            .set_finding_status(&session.id, dismissed.id(), FindingStatus::Dismissed)
            .unwrap();
        drop(first);

        let second = SessionStore::new(&data_dir);
        let read = second
            .read_findings(&session.id, Path::new("/nonexistent"))
            .unwrap();
        let statuses: Vec<_> = read.iter().map(|f| (f.summary(), f.status())).collect();
        assert_eq!(
            statuses,
            [
                ("kept", FindingStatus::Open),
                ("dismissed", FindingStatus::Dismissed)
            ]
        );
    }

    #[test]
    fn an_unchanged_file_keeps_its_finding_open() {
        let root = repo_with("unchanged", "src/lib.rs", "fn a() {}\n");
        let (store, session) = store_and_session("unchanged");
        store
            .record_finding(&session.id, &hashed_finding(&root, "src/lib.rs"))
            .unwrap();
        assert_eq!(
            store.read_findings(&session.id, &root).unwrap()[0].status(),
            FindingStatus::Open
        );
    }

    /// Acceptance criterion, and `context.md` §1.
    #[test]
    fn a_changed_file_makes_its_finding_stale() {
        let root = repo_with("changed", "src/lib.rs", "fn a() {}\n");
        let (store, session) = store_and_session("changed");
        store
            .record_finding(&session.id, &hashed_finding(&root, "src/lib.rs"))
            .unwrap();
        fs::write(root.join("src/lib.rs"), "fn a() { 1 }\n").unwrap();
        assert_eq!(
            store.read_findings(&session.id, &root).unwrap()[0].status(),
            FindingStatus::Stale
        );
    }

    #[test]
    fn a_deleted_file_makes_its_finding_stale() {
        let root = repo_with("deleted", "src/lib.rs", "fn a() {}\n");
        let (store, session) = store_and_session("deleted");
        store
            .record_finding(&session.id, &hashed_finding(&root, "src/lib.rs"))
            .unwrap();
        fs::remove_file(root.join("src/lib.rs")).unwrap();
        assert_eq!(
            store.read_findings(&session.id, &root).unwrap()[0].status(),
            FindingStatus::Stale
        );
    }

    /// §13.2: derived, not stored. Reverting the file brings the finding back.
    #[test]
    fn staleness_is_derived_on_read_so_a_reverted_file_reopens_its_finding() {
        let root = repo_with("reverted", "src/lib.rs", "fn a() {}\n");
        let (store, session) = store_and_session("reverted");
        store
            .record_finding(&session.id, &hashed_finding(&root, "src/lib.rs"))
            .unwrap();
        fs::write(root.join("src/lib.rs"), "changed\n").unwrap();
        assert_eq!(
            store.read_findings(&session.id, &root).unwrap()[0].status(),
            FindingStatus::Stale
        );
        fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
        assert_eq!(
            store.read_findings(&session.id, &root).unwrap()[0].status(),
            FindingStatus::Open
        );
    }

    /// `context.md` §1: no recorded hash means staleness cannot be judged.
    #[test]
    fn a_finding_without_a_recorded_hash_is_never_stale() {
        let root = repo_with("no-hash", "src/lib.rs", "fn a() {}\n");
        let (store, session) = store_and_session("no-hash");
        store
            .record_finding(&session.id, &finding("x", Some("src/lib.rs")))
            .unwrap();
        fs::write(root.join("src/lib.rs"), "changed\n").unwrap();
        assert_eq!(
            store.read_findings(&session.id, &root).unwrap()[0].status(),
            FindingStatus::Open
        );
    }

    /// §13.2: the user's decision is not overwritten by a hash.
    #[test]
    fn a_dismissed_finding_is_not_re_marked_stale() {
        let root = repo_with("dismissed", "src/lib.rs", "fn a() {}\n");
        let (store, session) = store_and_session("dismissed");
        let recorded = hashed_finding(&root, "src/lib.rs");
        store.record_finding(&session.id, &recorded).unwrap();
        store
            .set_finding_status(&session.id, recorded.id(), FindingStatus::Dismissed)
            .unwrap();
        fs::write(root.join("src/lib.rs"), "changed\n").unwrap();
        assert_eq!(
            store.read_findings(&session.id, &root).unwrap()[0].status(),
            FindingStatus::Dismissed
        );
    }

    #[test]
    fn setting_stale_directly_is_refused_and_appends_nothing() {
        let (store, session) = store_and_session("set-stale");
        let recorded = finding("x", None);
        store.record_finding(&session.id, &recorded).unwrap();
        let before = store.latest_event_seq(&session.id).unwrap();
        assert!(
            store
                .set_finding_status(&session.id, recorded.id(), FindingStatus::Stale)
                .is_err()
        );
        assert_eq!(store.latest_event_seq(&session.id).unwrap(), before);
    }

    #[test]
    fn a_status_change_for_an_unknown_finding_is_refused_and_appends_nothing() {
        let (store, session) = store_and_session("unknown");
        let before = store.latest_event_seq(&session.id).unwrap();
        assert!(
            store
                .set_finding_status(
                    &session.id,
                    "finding_never_recorded",
                    FindingStatus::Dismissed
                )
                .is_err()
        );
        assert_eq!(store.latest_event_seq(&session.id).unwrap(), before);
    }

    /// §13.2: findings follow `active_events`, as plans and diagnostics do.
    #[test]
    fn a_rewind_takes_the_findings_recorded_after_its_point() {
        let (store, session) = store_and_session("rewind");
        store
            .record_finding(&session.id, &finding("before", None))
            .unwrap();
        let point = store.latest_event_seq(&session.id).unwrap();
        store
            .record_finding(&session.id, &finding("after", None))
            .unwrap();
        store.rewind_conversation(&session.id, point).unwrap();
        let read = store
            .read_findings(&session.id, Path::new("/nonexistent"))
            .unwrap();
        let summaries: Vec<_> = read.iter().map(Finding::summary).collect();
        assert_eq!(summaries, ["before"]);
    }

    /// The §5.7 log shape, readable by hand.
    #[test]
    fn the_log_carries_both_event_kinds_in_their_documented_shape() {
        let (store, session) = store_and_session("shape");
        let recorded = finding("x", None);
        store.record_finding(&session.id, &recorded).unwrap();
        store
            .set_finding_status(&session.id, recorded.id(), FindingStatus::Dismissed)
            .unwrap();
        let log = fs::read_to_string(store.session_log_path(&session.id)).unwrap();
        let lines: Vec<serde_json::Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let recorded_event = lines
            .iter()
            .find(|event| event["eventType"] == "finding_recorded")
            .expect("finding_recorded");
        assert_eq!(recorded_event["payload"]["id"], recorded.id());
        assert_eq!(recorded_event["payload"]["source"], "compiler");
        let changed = lines
            .iter()
            .find(|event| event["eventType"] == "finding_status_changed")
            .expect("finding_status_changed");
        assert_eq!(changed["payload"]["findingId"], recorded.id());
        assert_eq!(changed["payload"]["status"], "dismissed");
    }

    fn worker_record(parent: Option<&str>, source: &str, worktree: &str) -> WorkerRecord {
        WorkerRecord {
            parent_session_id: parent.map(str::to_string),
            source_repository: PathBuf::from(source),
            worktree_path: PathBuf::from(worktree),
            branch: "damaian/split-the-parser".to_string(),
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            included_uncommitted: true,
            git_commands: vec![
                "git stash create".to_string(),
                format!("git worktree add -b damaian/split-the-parser {worktree} 0123456"),
            ],
        }
    }

    #[test]
    fn a_worker_record_round_trips() {
        let store = SessionStore::new(temp_data_dir("worker-round-trip"));
        let worker = store
            .create_session("repo_worktree", "Split the parser")
            .unwrap();
        let record = worker_record(
            Some("ses_parent"),
            "/code/app",
            "/code/app/.damaian/worktrees/split-the-parser",
        );

        store.record_worker_spawned(&worker.id, &record).unwrap();

        assert_eq!(store.worker_record(&worker.id).unwrap(), Some(record));
        // A rewind discards conversation, not what the session is.
        store.rewind_conversation(&worker.id, 0).unwrap();
        assert!(store.worker_record(&worker.id).unwrap().is_some());
    }

    #[test]
    fn deleting_a_session_deletes_its_progress_file() {
        use crate::worker_progress::{ProgressKind, ProgressWriter, progress_path};
        let data_dir = temp_data_dir("worker-progress-delete");
        let store = SessionStore::new(&data_dir);
        let worker = store.create_session("repo_worktree", "Worker").unwrap();
        let mut progress =
            ProgressWriter::create(&data_dir, &worker.id, SecretScanner::default()).unwrap();
        progress
            .append(
                ProgressKind::Status,
                serde_json::json!({"status": "running"}),
            )
            .unwrap();
        let path = progress_path(&data_dir, &worker.id);
        assert!(path.exists());

        store.delete_session(&worker.id).unwrap();

        assert!(!path.exists());
        assert!(!store.session_log_path(&worker.id).exists());
    }

    #[test]
    fn a_session_without_a_worker_record_has_none() {
        let store = SessionStore::new(temp_data_dir("worker-none"));
        let parent = store.create_session("repo_1", "Planner").unwrap();
        // A parent's `worker_spawned` names a child; it does not make the
        // parent a worker.
        store.record_worker_child(&parent.id, "ses_child").unwrap();

        assert_eq!(store.worker_record(&parent.id).unwrap(), None);
        assert_eq!(store.worker_record("ses_missing").unwrap(), None);
    }

    #[test]
    fn a_parent_lists_its_worker_children() {
        let store = SessionStore::new(temp_data_dir("worker-children"));
        let parent = store.create_session("repo_1", "Planner").unwrap();
        let other = store.create_session("repo_1", "Unrelated").unwrap();

        store.record_worker_child(&parent.id, "ses_first").unwrap();
        store.record_worker_child(&parent.id, "ses_second").unwrap();

        assert_eq!(
            store.worker_children(&parent.id).unwrap(),
            vec!["ses_first".to_string(), "ses_second".to_string()]
        );
        assert!(store.worker_children(&other.id).unwrap().is_empty());
    }

    #[test]
    fn worker_sessions_are_found_by_source_repository_across_repository_ids() {
        let store = SessionStore::new(temp_data_dir("worker-by-source"));
        let first = store.create_session("repo_worktree_a", "First").unwrap();
        let second = store.create_session("repo_worktree_b", "Second").unwrap();
        let elsewhere = store
            .create_session("repo_worktree_c", "Elsewhere")
            .unwrap();
        let ordinary = store.create_session("repo_source", "Planner").unwrap();
        store
            .record_worker_spawned(
                &first.id,
                &worker_record(
                    Some(&ordinary.id),
                    "/code/app",
                    "/code/app/.damaian/worktrees/a",
                ),
            )
            .unwrap();
        store
            .record_worker_spawned(
                &second.id,
                &worker_record(None, "/code/app", "/code/app/.damaian/worktrees/b"),
            )
            .unwrap();
        store
            .record_worker_spawned(
                &elsewhere.id,
                &worker_record(None, "/code/other", "/code/other/.damaian/worktrees/c"),
            )
            .unwrap();

        let mut found: Vec<String> = store
            .worker_sessions_for_source(Path::new("/code/app"))
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect();
        found.sort();
        let mut expected = vec![first.id, second.id];
        expected.sort();

        assert_eq!(found, expected);
    }

    #[test]
    fn pins_are_appended_events_and_replay_newest_wins() {
        let store = SessionStore::new(temp_data_dir("pins"));
        let session = store.create_session("repo_1", "Pinned").unwrap();
        let range = |start, end| LineRange { start, end };

        store
            .add_context_pin(&session.id, "src/upload.rs", None)
            .unwrap();
        store
            .add_context_pin(&session.id, "src/upload.rs", Some(range(40, 120)))
            .unwrap();
        store
            .add_context_pin(&session.id, "tests/upload.rs", Some(range(1, 20)))
            .unwrap();
        store
            .remove_context_pin(&session.id, "tests/upload.rs", Some(range(1, 20)))
            .unwrap();
        store
            .add_context_pin(&session.id, "tests/upload.rs", Some(range(30, 50)))
            .unwrap();

        assert_eq!(
            store.context_pins(&session.id).unwrap(),
            vec![
                ContextPin {
                    path: "src/upload.rs".to_string(),
                    range: None,
                },
                ContextPin {
                    path: "src/upload.rs".to_string(),
                    range: Some(range(40, 120)),
                },
                ContextPin {
                    path: "tests/upload.rs".to_string(),
                    range: Some(range(30, 50)),
                },
            ]
        );

        let log = fs::read_to_string(store.session_log_path(&session.id)).unwrap();
        assert!(log.contains("\"eventType\":\"context_pin_added\""), "{log}");
        assert!(
            log.contains("\"eventType\":\"context_pin_removed\""),
            "{log}"
        );
    }

    #[test]
    fn clearing_pins_removes_every_earlier_pin() {
        let store = SessionStore::new(temp_data_dir("pins-clear"));
        let session = store.create_session("repo_1", "Pinned").unwrap();

        store.add_context_pin(&session.id, "a.rs", None).unwrap();
        store.add_context_pin(&session.id, "b.rs", None).unwrap();
        store.clear_context_pins(&session.id).unwrap();
        assert!(store.context_pins(&session.id).unwrap().is_empty());

        store.add_context_pin(&session.id, "c.rs", None).unwrap();
        assert_eq!(
            store
                .context_pins(&session.id)
                .unwrap()
                .into_iter()
                .map(|pin| pin.path)
                .collect::<Vec<_>>(),
            vec!["c.rs".to_string()]
        );
    }

    #[test]
    fn a_pin_round_trips_through_a_second_store_and_survives_a_rewind() {
        let data_dir = temp_data_dir("pins-restart");
        let store = SessionStore::new(&data_dir);
        let session = store.create_session("repo_1", "Pinned").unwrap();
        let pin = ContextPin {
            path: "src/a.rs".to_string(),
            range: Some(LineRange { start: 3, end: 9 }),
        };
        store
            .add_context_pin(&session.id, &pin.path, pin.range)
            .unwrap();
        // A rewind discards conversation, not the user's control of context.
        store.rewind_conversation(&session.id, 0).unwrap();

        let reopened = SessionStore::new(&data_dir);
        assert_eq!(reopened.context_pins(&session.id).unwrap(), vec![pin]);
    }

    #[test]
    fn a_restriction_replaces_and_clears() {
        let store = SessionStore::new(temp_data_dir("restriction"));
        let session = store.create_session("repo_1", "Restricted").unwrap();

        assert_eq!(store.context_restriction(&session.id).unwrap(), None);

        store
            .set_context_restriction(&session.id, &["src/a.rs".to_string()])
            .unwrap();
        store
            .set_context_restriction(&session.id, &["src/b.rs".to_string()])
            .unwrap();
        assert_eq!(
            store.context_restriction(&session.id).unwrap(),
            Some(vec!["src/b.rs".to_string()])
        );

        store.clear_context_restriction(&session.id).unwrap();
        assert_eq!(store.context_restriction(&session.id).unwrap(), None);

        store.set_context_restriction(&session.id, &[]).unwrap();
        assert_eq!(
            store.context_restriction(&session.id).unwrap(),
            Some(Vec::new()),
            "an empty restriction is a restriction, not a clear"
        );
    }

    #[test]
    fn a_line_containing_a_pin_event_name_is_not_a_pin() {
        let store = SessionStore::new(temp_data_dir("pin-substring"));
        let session = store.create_session("repo_1", "Talk").unwrap();
        store
            .append_message(
                &session.id,
                None,
                "user",
                "we should add a context_pin_added event someday",
            )
            .unwrap();

        assert!(store.context_pins(&session.id).unwrap().is_empty());
    }
}
