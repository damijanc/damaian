//! The engine-owned outcome and action model for a task (spec 101 Task 3).
//!
//! A desktop shell, worker host, CLI or web UI may transport these values and
//! draw them, but must not decide what a task status means or which action is
//! permitted. This module owns that mapping for an ordinary session and for a
//! worker, including the stale-progress sentence, the completion reasons and
//! waivers, and the one primary action a surface may offer.
//!
//! The continuation itself is still authorized at the execution boundary, not
//! here: [`crate::chat::ChatOrchestrator::continue_incomplete_task`] re-reads
//! the newest durable task and its report before creating a run, so a stale
//! browser control or a second click is refused by the engine.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::completion::{CompletionGateReport, CompletionReason, CompletionWaiver};
use crate::error::{ClientError, Result};
use crate::plan::{StepStatus, TaskPlan};
use crate::session::{SessionStore, Task, TaskStatus};

/// A running worker with no progress for this long says so. Presentation only;
/// the engine never calls a worker dead or stuck from the absence of output.
pub const WORKER_STALE_PROGRESS_AFTER_MS: u128 = 30_000;

/// The visual kind of an outcome. The names are stable across adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcomeKind {
    Starting,
    Working,
    Paused,
    NeedsDecision,
    /// A worker is waiting on an answer to a model question (spec 104). It is
    /// a decision-shaped pause but not an approval, and its label says so.
    NeedsAnswer,
    Complete,
    Incomplete,
    Stopped,
    Failed,
    Interrupted,
    UnknownOutcome,
    BudgetExhausted,
    /// The next request could not fit the configured window share (spec 103
    /// Task 6).
    CannotFit,
    /// The engine-owned segment progress state repeated (spec 103 Task 6).
    NoProgress,
    /// A worker spent its pre-plan exploration budget and did not propose a
    /// plan when asked (spec 103 R17).
    PrePlanExplorationExhausted,
    Ended,
}

impl TaskOutcomeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::Paused => "paused",
            Self::NeedsDecision => "needs_decision",
            Self::NeedsAnswer => "needs_answer",
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::UnknownOutcome => "unknown_outcome",
            Self::BudgetExhausted => "budget_exhausted",
            Self::CannotFit => "cannot_fit",
            Self::NoProgress => "no_progress",
            Self::PrePlanExplorationExhausted => "preplan_exploration_exhausted",
            Self::Ended => "ended",
        }
    }
}

/// The five worker dots, chosen by the engine rather than by a browser map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerDot {
    Running,
    Paused,
    NeedsYou,
    Finished,
    Failed,
}

impl WorkerDot {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::NeedsYou => "needs-you",
            Self::Finished => "finished",
            Self::Failed => "failed",
        }
    }
}

/// The Workers board's three groups (spec 57, 95).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerBoardGroup {
    NeedsYou,
    Running,
    Finished,
}

impl WorkerBoardGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs_you",
            Self::Running => "running",
            Self::Finished => "finished",
        }
    }
}

/// Where a request for an action goes. The engine decides this from the
/// surface; the browser posts the command and never picks a worker command from
/// a lifecycle string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskActionTransport {
    /// A streamed ordinary-session turn.
    Stream,
    /// The worker host's control socket.
    Worker,
    /// A browser navigation, with no mutation.
    Navigate,
}

/// What a user must supply with an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskActionInput {
    None,
    /// The action works with an empty input; a note may still be appended.
    OptionalNote,
    /// The action needs text before it can run.
    Message,
}

/// One action a surface may offer, already labelled by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAction {
    /// Stable command id. The transport is part of the model, so a browser does
    /// not infer it from a status.
    pub command: TaskCommand,
    pub label: String,
    pub input: TaskActionInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    pub transport: TaskActionTransport,
    /// The exact durable task this action targets, when it targets one. A
    /// continuation carries the id it was rendered for; the execution boundary
    /// re-reads it rather than resolving "the latest task" at click time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The worker-host wire name, filled from [`TaskCommand::worker_command`] for
    /// worker actions. This is the one semantic-action-to-wire map; a browser
    /// posts the returned value and keeps no map of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_command: Option<String>,
}

impl TaskAction {
    fn new(
        command: TaskCommand,
        label: impl Into<String>,
        input: TaskActionInput,
        placeholder: Option<String>,
        transport: TaskActionTransport,
    ) -> Self {
        let wire_command = if transport == TaskActionTransport::Worker {
            command.worker_command().map(str::to_string)
        } else {
            None
        };
        Self {
            command,
            label: label.into(),
            input,
            placeholder,
            transport,
            task_id: None,
            wire_command,
        }
    }

    /// Attaches the exact durable task this action was rendered for.
    fn for_task(mut self, task_id: Option<&str>) -> Self {
        self.task_id = task_id.map(str::to_string);
        self
    }
}

/// The engine-authorized task actions (spec 101 Task 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCommand {
    /// The one explicit continuation that carries an incomplete task's plan and
    /// completion reasons. Requires no typed "continue".
    ContinueTask,
    /// A new user message that starts another turn; used for a completed task
    /// or a stopped one the engine cannot resume under the completion gate.
    SendMessage,
    /// A note that joins the running turn at its next round boundary.
    SendNote,
    /// An ordinary session's completed task starts another task.
    SendFollowUp,
    /// The user must review a pending approval, question, patch or fallback.
    ReviewDecision,
    /// Re-run the stopped task's own request through the engine-built
    /// `ask_again_task_id` route rather than retyping a continuation.
    AskAgain,
    PauseWorker,
    ResumeWorker,
    StopWorker,
    RestartWorker,
    OpenWorker,
}

impl TaskCommand {
    /// The worker-host wire name, or `None` for an action the host does not
    /// take. The serialized `wireCommand` carries this value to the browser, so
    /// there is exactly one semantic-action-to-wire mapping.
    pub fn worker_command(self) -> Option<&'static str> {
        match self {
            Self::ContinueTask => Some("continue-task"),
            Self::SendMessage | Self::SendFollowUp => Some("message"),
            Self::SendNote => Some("note"),
            Self::PauseWorker => Some("pause"),
            Self::ResumeWorker => Some("resume"),
            Self::StopWorker => Some("stop"),
            Self::ReviewDecision | Self::AskAgain | Self::RestartWorker | Self::OpenWorker => None,
        }
    }
}

/// The visual state of a turn's indicator and of any stop row under it.
///
/// Only [`Self::Complete`] is success. Every task that is still running or
/// waiting for the user is [`Self::Pending`], so a paused turn can never be
/// labelled as finished by a fallback arm or by a browser string comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnIndicatorState {
    /// The task closed through the completion gate.
    Complete,
    /// The task is resumable but was not closed.
    Incomplete,
    /// The user stopped the turn.
    Stopped,
    /// The turn failed or its outcome is unknown.
    Failed,
    /// The turn is still running, paused, or waiting for a user decision.
    Pending,
}

/// How a live turn's indicator and any stop row should read.
///
/// The engine decides this from the settled task and the model run, so the UI
/// only draws `indicatorState` and `stopNotice`. A row that says "Tool budget
/// exhausted" and offers *Ask again* is presentation data, not a browser map
/// from a status string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPresentation {
    pub indicator_state: TurnIndicatorState,
    /// Whether a reload groups this task with turns that stopped for the user.
    /// A duration row is suppressed for those turns, and the fact comes from
    /// the engine rather than from the browser reading `task.status`.
    pub paused: bool,
    /// Whether the live turn should draw the engine's task-control card (for
    /// example an incomplete task with its reasons and **Continue task**).
    pub show_task_control: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_notice: Option<TurnStopNotice>,
    /// The typed terminal outcome for a bounded stop (spec 103 Task 6), so a
    /// live turn and a reload carry the same engine-owned facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_outcome: Option<crate::task_outcomes::TaskTerminalOutcome>,
}

/// One engine-labelled row under a stopped turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStopNotice {
    pub state: TurnIndicatorState,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<TaskAction>,
}

/// The composer's next command, or `None` when the composer takes no input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskComposer {
    pub action: TaskAction,
    pub placeholder: String,
}

/// A typed worker lifecycle fact, not a browser condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerProcessState {
    Running,
    Paused,
    WaitingForApproval,
    Idle,
    ShuttingDown,
    Exited,
}

impl WorkerProcessState {
    /// Reads the progress file's `state` string. An unknown value is treated as
    /// `Idle`: saying "running" about a state this build cannot classify would
    /// be the wrong direction.
    pub fn parse(value: &str) -> Self {
        match value {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "waiting_for_approval" => Self::WaitingForApproval,
            "shutting_down" => Self::ShuttingDown,
            "exited" => Self::Exited,
            _ => Self::Idle,
        }
    }
}

/// Everything the engine needs to describe a worker's task outcome.
///
/// All lifecycle facts are typed inputs supplied by the worker host or shell.
/// The engine alone turns them, the task status and the completion report into
/// labels, dots, groups and actions.
#[derive(Debug, Clone)]
pub struct WorkerTaskFacts<'a> {
    pub task: Option<&'a Task>,
    /// The status the worker's own progress record reported, when the task
    /// log has not been read in this call. The engine still treats the durable
    /// task as authoritative; this keeps the literal reported value visible
    /// when a progress line arrived first.
    pub reported_task_status: Option<TaskStatus>,
    pub report: Option<&'a CompletionGateReport>,
    pub alive: bool,
    pub state: WorkerProcessState,
    /// The worker's progress record says the pause is a model question, not an
    /// approval (spec 104). Set by the host from the question id in its status
    /// line; the engine owns the label and group.
    pub pending_question: bool,
    pub has_run: bool,
    pub turn_error: Option<&'a str>,
    pub last_progress_at_ms: Option<u128>,
    pub now_ms: u128,
}

/// The engine's rendered model for one task surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskControl {
    pub task_id: Option<String>,
    /// The literal durable status (`incomplete` stays `incomplete`).
    pub status: Option<String>,
    pub outcome: TaskOutcomeKind,
    pub dot: WorkerDot,
    pub group: WorkerBoardGroup,
    pub headline: String,
    pub detail: String,
    pub needs_decision: bool,
    pub ended: bool,
    pub progress_current: bool,
    /// Whether the worker strip should show its round counter and bar.
    pub show_round: bool,
    pub reasons: Vec<CompletionReason>,
    pub waivers: Vec<CompletionWaiver>,
    /// The one action the composer or card should offer.
    pub primary_action: Option<TaskAction>,
    /// The Workers board's one visible action.
    pub board_action: Option<TaskAction>,
    /// Worker lifecycle controls (pause, resume, stop, restart).
    pub actions: Vec<TaskAction>,
    pub composer: Option<TaskComposer>,
    /// The typed terminal outcome when this task stopped at an approved-task
    /// bound (spec 103 Task 6). Carried for desktop/worker/return transports;
    /// no surface reconstructs it from `status` or a label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_outcome: Option<crate::task_outcomes::TaskTerminalOutcome>,
}

impl TaskControl {
    /// An ordinary session's view of its newest task.
    pub fn for_session(task: Option<&Task>, report: Option<&CompletionGateReport>) -> Self {
        Self::for_session_with_outcome(task, report, None)
    }

    /// An ordinary session's view with the typed terminal outcome, when the
    /// task stopped at an approved-task bound (spec 103 Task 6).
    pub fn for_session_with_outcome(
        task: Option<&Task>,
        report: Option<&CompletionGateReport>,
        outcome: Option<&crate::task_outcomes::TaskTerminalOutcome>,
    ) -> Self {
        let Some(task) = task else {
            return Self::empty();
        };
        let reasons = report
            .map(|report| report.reasons.clone())
            .unwrap_or_default();
        let waivers = report
            .map(|report| report.waivers.clone())
            .unwrap_or_default();
        let status = task.status.as_str().to_string();
        match &task.status {
            TaskStatus::TaskBudgetExhausted
            | TaskStatus::TaskCompactionCannotFit
            | TaskStatus::TaskNoProgress
            | TaskStatus::TaskPrePlanExplorationExhausted => {
                let (outcome_kind, headline, detail) =
                    bounded_stop_presentation(&task.status, outcome);
                Self::terminal(
                    task,
                    status,
                    outcome_kind,
                    WorkerDot::Failed,
                    &headline,
                    detail,
                    reasons,
                    waivers,
                    outcome.cloned(),
                )
            }
            TaskStatus::Failed if outcome.is_some_and(usage_unavailable) => Self::terminal(
                task,
                status,
                TaskOutcomeKind::Failed,
                WorkerDot::Failed,
                &outcome.expect("checked above").label(),
                outcome.expect("checked above").explanation(),
                reasons,
                waivers,
                outcome.cloned(),
            ),
            TaskStatus::Incomplete => Self {
                task_id: Some(task.id.clone()),
                status: Some(status),
                outcome: TaskOutcomeKind::Incomplete,
                dot: WorkerDot::Failed,
                group: WorkerBoardGroup::Finished,
                headline: "Task incomplete".to_string(),
                detail: completion_detail(
                    report,
                    "The task ended with work still open. Continuing it carries the same plan and completion reasons into a new run.",
                ),
                needs_decision: false,
                ended: true,
                progress_current: true,
                show_round: false,
                reasons,
                waivers,
                primary_action: Some(
                    TaskAction::new(
                        TaskCommand::ContinueTask,
                        "Continue task",
                        TaskActionInput::OptionalNote,
                        Some("Add a note for the continuation (optional)".to_string()),
                        TaskActionTransport::Stream,
                    )
                    .for_task(Some(&task.id)),
                ),
                board_action: None,
                actions: Vec::new(),
                composer: None,
                terminal_outcome: outcome.cloned(),
            },
            TaskStatus::Complete => Self::terminal(
                task,
                status,
                TaskOutcomeKind::Complete,
                WorkerDot::Finished,
                "Task complete",
                "The task finished its work.".to_string(),
                reasons,
                waivers,
                None,
            ),
            TaskStatus::Cancelled => Self::terminal(
                task,
                status,
                TaskOutcomeKind::Stopped,
                WorkerDot::Failed,
                "Task stopped",
                "The task stopped before completion.".to_string(),
                reasons,
                waivers,
                None,
            ),
            TaskStatus::Failed => Self::terminal(
                task,
                status,
                TaskOutcomeKind::Failed,
                WorkerDot::Failed,
                "Task failed",
                "The task did not complete.".to_string(),
                reasons,
                waivers,
                None,
            ),
            TaskStatus::ToolBudgetExhausted | TaskStatus::TokenBudgetExhausted => Self::terminal(
                task,
                status,
                TaskOutcomeKind::BudgetExhausted,
                WorkerDot::Failed,
                "Task needs continuation",
                "The task reached its budget before completion.".to_string(),
                reasons,
                waivers,
                None,
            ),
            TaskStatus::Interrupted => Self::terminal(
                task,
                status,
                TaskOutcomeKind::Interrupted,
                WorkerDot::Failed,
                "Task interrupted",
                "The task stopped before it completed.".to_string(),
                reasons,
                waivers,
                None,
            ),
            TaskStatus::UnknownExternalOutcome => Self::terminal(
                task,
                status,
                TaskOutcomeKind::UnknownOutcome,
                WorkerDot::Failed,
                "Outcome needs review",
                "The task stopped during an external action, so its outcome is unknown."
                    .to_string(),
                reasons,
                waivers,
                None,
            ),
            _ => Self {
                task_id: Some(task.id.clone()),
                status: Some(status),
                outcome: TaskOutcomeKind::Working,
                dot: WorkerDot::Running,
                group: WorkerBoardGroup::Running,
                headline: "Working".to_string(),
                detail: "The task is still running.".to_string(),
                needs_decision: false,
                ended: false,
                progress_current: true,
                show_round: false,
                reasons,
                waivers,
                primary_action: None,
                board_action: None,
                actions: Vec::new(),
                composer: None,
                terminal_outcome: outcome.cloned(),
            },
        }
    }

    /// A worker's view of the same durable verdict plus its lifecycle.
    pub fn for_worker(facts: WorkerTaskFacts<'_>) -> Self {
        Self::for_worker_with_outcome(facts, None)
    }

    /// A worker's view with the typed terminal outcome when the task stopped at
    /// an approved-task bound (spec 103 Task 6).
    pub fn for_worker_with_outcome(
        facts: WorkerTaskFacts<'_>,
        outcome: Option<&crate::task_outcomes::TaskTerminalOutcome>,
    ) -> Self {
        let task = facts.task;
        let reported_task_status = facts.reported_task_status.clone();
        let effective_status = task
            .map(|task| &task.status)
            .or(reported_task_status.as_ref());
        // Spec 103 Task 6: a bounded stop keeps the same engine-owned control
        // whether the worker is still alive, idle after the turn, or restarted.
        // It never becomes a generic Continue/Send message against the same
        // approved bound; only the ordinary Open lifecycle action remains.
        if let Some(status) = effective_status
            && (matches!(
                status,
                TaskStatus::TaskBudgetExhausted
                    | TaskStatus::TaskCompactionCannotFit
                    | TaskStatus::TaskNoProgress
                    | TaskStatus::TaskPrePlanExplorationExhausted
            ) || (matches!(status, TaskStatus::Failed)
                && outcome.is_some_and(usage_unavailable)))
        {
            return Self::bounded_worker(
                task.map(|task| task.id.as_str()),
                status.as_str(),
                status,
                outcome,
                facts.report,
            );
        }
        // The engine decides "needs a decision" from the typed process state and
        // the durable or reported status. The shell only transports those facts.
        let waiting_for_decision = facts.state == WorkerProcessState::WaitingForApproval
            || matches!(effective_status, Some(TaskStatus::WaitingForApproval))
            || matches!(
                reported_task_status.as_ref(),
                Some(TaskStatus::WaitingForApproval)
            );
        let reasons = facts
            .report
            .map(|report| report.reasons.clone())
            .unwrap_or_default();
        let waivers = facts
            .report
            .map(|report| report.waivers.clone())
            .unwrap_or_default();
        let task_status = task.map(|task| task.status.as_str().to_string());

        if !facts.alive {
            let (outcome, headline, detail) = match effective_status {
                Some(TaskStatus::Incomplete) => (
                    TaskOutcomeKind::Incomplete,
                    "Task incomplete",
                    completion_detail(facts.report, "The worker stopped with work still open."),
                ),
                Some(TaskStatus::Complete) => (
                    TaskOutcomeKind::Complete,
                    "Task complete",
                    "The worker finished this task.".to_string(),
                ),
                Some(TaskStatus::Cancelled) => (
                    TaskOutcomeKind::Stopped,
                    "Task stopped",
                    "The task stopped before completion.".to_string(),
                ),
                Some(TaskStatus::Failed) => (
                    TaskOutcomeKind::Failed,
                    "Task failed",
                    "The task did not complete.".to_string(),
                ),
                Some(TaskStatus::ToolBudgetExhausted | TaskStatus::TokenBudgetExhausted) => (
                    TaskOutcomeKind::BudgetExhausted,
                    "Task needs continuation",
                    "The worker reached its limit before completion.".to_string(),
                ),
                Some(TaskStatus::UnknownExternalOutcome) => (
                    TaskOutcomeKind::UnknownOutcome,
                    "Outcome needs review",
                    "The worker stopped during an external action, so its outcome is unknown."
                        .to_string(),
                ),
                Some(TaskStatus::Interrupted) => (
                    TaskOutcomeKind::Interrupted,
                    "Task interrupted",
                    "The worker process ended before completion.".to_string(),
                ),
                Some(_) | None if facts.has_run => (
                    TaskOutcomeKind::Interrupted,
                    "Task interrupted",
                    "The worker process ended before completion.".to_string(),
                ),
                _ => (
                    TaskOutcomeKind::Ended,
                    "Worker ended",
                    "Restart the worker to send another task.".to_string(),
                ),
            };
            let needs_decision = false;
            let ended = true;
            let primary = restart_action();
            return Self {
                task_id: task.map(|task| task.id.clone()),
                status: task_status,
                outcome,
                dot: if outcome == TaskOutcomeKind::Complete {
                    WorkerDot::Finished
                } else {
                    WorkerDot::Failed
                },
                group: WorkerBoardGroup::Finished,
                headline: headline.to_string(),
                detail: format!("{detail} Restart the worker to send another message."),
                needs_decision,
                ended,
                progress_current: true,
                show_round: false,
                reasons,
                waivers,
                primary_action: Some(primary.clone()),
                board_action: Some(open_action()),
                actions: vec![primary],
                composer: None,
                terminal_outcome: None,
            };
        }

        if facts.state == WorkerProcessState::Running {
            let current = progress_is_current(facts.last_progress_at_ms, facts.now_ms);
            return Self {
                task_id: task.map(|task| task.id.clone()),
                status: task_status,
                outcome: TaskOutcomeKind::Working,
                dot: WorkerDot::Running,
                group: WorkerBoardGroup::Running,
                headline: "Working".to_string(),
                detail: worker_activity_detail(facts.last_progress_at_ms, facts.now_ms),
                needs_decision: false,
                ended: false,
                progress_current: current,
                show_round: true,
                reasons,
                waivers,
                primary_action: None,
                board_action: Some(lifecycle_action(TaskCommand::PauseWorker, "Pause")),
                actions: vec![
                    lifecycle_action(TaskCommand::PauseWorker, "Pause"),
                    lifecycle_action(TaskCommand::StopWorker, "Stop"),
                ],
                composer: Some(TaskComposer {
                    action: TaskAction::new(
                        TaskCommand::SendNote,
                        "Send note",
                        TaskActionInput::Message,
                        Some(
                            "Steer the worker: a note reaches it before its next round".to_string(),
                        ),
                        TaskActionTransport::Worker,
                    ),
                    placeholder: "Steer the worker: a note reaches it before its next round"
                        .to_string(),
                }),
                terminal_outcome: outcome.cloned(),
            };
        }

        if facts.state == WorkerProcessState::Paused {
            return Self {
                task_id: task.map(|task| task.id.clone()),
                status: task_status,
                outcome: TaskOutcomeKind::Paused,
                dot: WorkerDot::Paused,
                group: WorkerBoardGroup::Running,
                headline: "Paused".to_string(),
                detail: "Resume it with a note when you are ready.".to_string(),
                needs_decision: false,
                ended: false,
                progress_current: true,
                show_round: false,
                reasons,
                waivers,
                primary_action: None,
                board_action: Some(open_action()),
                actions: vec![
                    lifecycle_action(TaskCommand::ResumeWorker, "Resume"),
                    lifecycle_action(TaskCommand::StopWorker, "Stop"),
                ],
                composer: Some(TaskComposer {
                    action: TaskAction::new(
                        TaskCommand::ResumeWorker,
                        "Resume with note",
                        TaskActionInput::Message,
                        Some("Add a note: the worker reads it as it resumes".to_string()),
                        TaskActionTransport::Worker,
                    ),
                    placeholder: "Add a note: the worker reads it as it resumes".to_string(),
                }),
                terminal_outcome: outcome.cloned(),
            };
        }

        if waiting_for_decision && facts.pending_question {
            return Self {
                task_id: task.map(|task| task.id.clone()),
                status: task_status,
                outcome: TaskOutcomeKind::NeedsAnswer,
                dot: WorkerDot::NeedsYou,
                group: WorkerBoardGroup::NeedsYou,
                headline: "Needs your answer".to_string(),
                detail: "Answer the question below before the task can continue.".to_string(),
                needs_decision: true,
                ended: false,
                progress_current: true,
                show_round: false,
                reasons,
                waivers,
                primary_action: Some(review_action()),
                board_action: Some(review_action()),
                actions: Vec::new(),
                composer: None,
                terminal_outcome: outcome.cloned(),
            };
        }

        if waiting_for_decision {
            return Self {
                task_id: task.map(|task| task.id.clone()),
                status: task_status,
                outcome: TaskOutcomeKind::NeedsDecision,
                dot: WorkerDot::NeedsYou,
                group: WorkerBoardGroup::NeedsYou,
                headline: "Needs your decision".to_string(),
                detail: "Review the proposal below before the task can continue.".to_string(),
                needs_decision: true,
                ended: false,
                progress_current: true,
                show_round: false,
                reasons,
                waivers,
                primary_action: Some(review_action()),
                board_action: Some(review_action()),
                actions: Vec::new(),
                composer: None,
                terminal_outcome: outcome.cloned(),
            };
        }

        if facts.state == WorkerProcessState::ShuttingDown {
            return Self {
                task_id: task.map(|task| task.id.clone()),
                status: task_status,
                outcome: TaskOutcomeKind::Stopped,
                dot: WorkerDot::Finished,
                group: WorkerBoardGroup::Finished,
                headline: "Stopping".to_string(),
                detail: "The worker is shutting down.".to_string(),
                needs_decision: false,
                ended: false,
                progress_current: true,
                show_round: false,
                reasons,
                waivers,
                primary_action: None,
                board_action: Some(open_action()),
                actions: Vec::new(),
                composer: None,
                terminal_outcome: outcome.cloned(),
            };
        }

        if let Some(error) = facts.turn_error {
            return Self {
                task_id: task.map(|task| task.id.clone()),
                status: task_status,
                outcome: TaskOutcomeKind::Failed,
                dot: WorkerDot::Failed,
                group: WorkerBoardGroup::Finished,
                headline: "Task failed".to_string(),
                detail: error.to_string(),
                needs_decision: false,
                ended: false,
                progress_current: true,
                show_round: false,
                reasons,
                waivers,
                primary_action: Some(continue_message_action()),
                board_action: Some(open_action()),
                actions: Vec::new(),
                composer: Some(composer_for(continue_message_action())),
                terminal_outcome: outcome.cloned(),
            };
        }

        if let Some(status) = effective_status
            && let Some(control) = Self::terminal_worker(
                status,
                task.map(|task| task.id.as_str()),
                task_status.clone(),
                facts.report,
                reasons.clone(),
                waivers.clone(),
            )
        {
            return control;
        }

        if facts.has_run {
            return Self {
                task_id: task.map(|task| task.id.clone()),
                status: task_status,
                outcome: TaskOutcomeKind::Failed,
                dot: WorkerDot::Failed,
                group: WorkerBoardGroup::Finished,
                headline: "Stopped without an outcome".to_string(),
                detail: "The worker ended its turn without recording whether the task completed. Send a message to continue or inspect the worktree.".to_string(),
                needs_decision: false,
                ended: false,
                progress_current: true,
            show_round: false,
                reasons,
                waivers,
                primary_action: Some(continue_message_action()),
                board_action: Some(open_action()),
                actions: Vec::new(),
                composer: Some(composer_for(continue_message_action())),
                terminal_outcome: outcome.cloned(),
            };
        }

        Self {
            task_id: None,
            status: task_status,
            outcome: TaskOutcomeKind::Starting,
            dot: WorkerDot::Running,
            group: WorkerBoardGroup::Running,
            headline: "Starting worker".to_string(),
            detail: "Waiting for the worker to begin its first task.".to_string(),
            needs_decision: false,
            ended: false,
            progress_current: true,
            show_round: false,
            reasons,
            waivers,
            primary_action: None,
            board_action: Some(open_action()),
            actions: Vec::new(),
            composer: Some(TaskComposer {
                action: TaskAction::new(
                    TaskCommand::SendMessage,
                    "Send message",
                    TaskActionInput::Message,
                    Some("Message the worker".to_string()),
                    TaskActionTransport::Worker,
                ),
                placeholder: "Message the worker".to_string(),
            }),
            terminal_outcome: outcome.cloned(),
        }
    }

    /// A terminal worker task on a live, idle worker.
    fn terminal_worker(
        status: &TaskStatus,
        task_id: Option<&str>,
        status_text: Option<String>,
        report: Option<&CompletionGateReport>,
        reasons: Vec<CompletionReason>,
        waivers: Vec<CompletionWaiver>,
    ) -> Option<Self> {
        let (outcome, dot, headline, detail, action) = match status {
            TaskStatus::Incomplete => (
                TaskOutcomeKind::Incomplete,
                WorkerDot::Failed,
                "Task incomplete",
                completion_detail(
                    report,
                    "The task stopped with work still open. Continue the task to carry its plan into a new run.",
                ),
                TaskAction::new(
                    TaskCommand::ContinueTask,
                    "Continue task",
                    TaskActionInput::OptionalNote,
                    Some("Add a note for the continuation (optional)".to_string()),
                    TaskActionTransport::Worker,
                )
                .for_task(task_id),
            ),
            TaskStatus::Complete => (
                TaskOutcomeKind::Complete,
                WorkerDot::Finished,
                "Task complete",
                "The worker finished this task. Send a follow-up to start another one.".to_string(),
                TaskAction::new(
                    TaskCommand::SendFollowUp,
                    "Send follow-up",
                    TaskActionInput::Message,
                    Some("The task is complete. Send a follow-up to start another task".to_string()),
                    TaskActionTransport::Worker,
                ),
            ),
            TaskStatus::Cancelled => (
                TaskOutcomeKind::Stopped,
                WorkerDot::Failed,
                "Task stopped",
                "The task stopped before completion. Send a message with how to continue."
                    .to_string(),
                continue_message_action(),
            ),
            TaskStatus::Failed => (
                TaskOutcomeKind::Failed,
                WorkerDot::Failed,
                "Task failed",
                "The task did not complete. Send a message to repair or retry it.".to_string(),
                continue_message_action(),
            ),
            TaskStatus::ToolBudgetExhausted | TaskStatus::TokenBudgetExhausted => (
                TaskOutcomeKind::BudgetExhausted,
                WorkerDot::Failed,
                "Task needs continuation",
                "The worker reached its limit before completion. Send a message to continue it."
                    .to_string(),
                continue_message_action(),
            ),
            TaskStatus::UnknownExternalOutcome => (
                TaskOutcomeKind::UnknownOutcome,
                WorkerDot::Failed,
                "Outcome needs review",
                "The worker stopped during an external action, so its outcome is unknown. Inspect the worktree before continuing.".to_string(),
                continue_message_action(),
            ),
            TaskStatus::Interrupted => (
                TaskOutcomeKind::Interrupted,
                WorkerDot::Failed,
                "Task interrupted",
                "The worker stopped before the task completed. Send a message to continue."
                    .to_string(),
                continue_message_action(),
            ),
            _ => return None,
        };
        Some(Self {
            task_id: task_id.map(str::to_string),
            status: status_text,
            outcome,
            dot,
            group: WorkerBoardGroup::Finished,
            headline: headline.to_string(),
            detail,
            needs_decision: false,
            ended: false,
            progress_current: true,
            show_round: false,
            reasons,
            waivers,
            primary_action: Some(action.clone()),
            board_action: Some(open_action()),
            actions: Vec::new(),
            composer: Some(composer_for(action)),
            terminal_outcome: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal(
        task: &Task,
        status: String,
        outcome: TaskOutcomeKind,
        dot: WorkerDot,
        headline: &str,
        detail: String,
        reasons: Vec<CompletionReason>,
        waivers: Vec<CompletionWaiver>,
        terminal_outcome: Option<crate::task_outcomes::TaskTerminalOutcome>,
    ) -> Self {
        Self {
            task_id: Some(task.id.clone()),
            status: Some(status),
            outcome,
            dot,
            group: WorkerBoardGroup::Finished,
            headline: headline.to_string(),
            detail,
            needs_decision: false,
            ended: true,
            progress_current: true,
            show_round: false,
            reasons,
            waivers,
            primary_action: None,
            board_action: None,
            actions: Vec::new(),
            composer: None,
            terminal_outcome,
        }
    }

    fn empty() -> Self {
        Self {
            task_id: None,
            status: None,
            outcome: TaskOutcomeKind::Starting,
            dot: WorkerDot::Running,
            group: WorkerBoardGroup::Running,
            headline: "No task yet".to_string(),
            detail: "No task has run in this session.".to_string(),
            needs_decision: false,
            ended: false,
            progress_current: true,
            show_round: false,
            reasons: Vec::new(),
            waivers: Vec::new(),
            primary_action: None,
            board_action: None,
            actions: Vec::new(),
            composer: None,
            terminal_outcome: None,
        }
    }

    /// The completion reasons as one sentence for a prompt or a detail line.
    pub fn reason_sentences(&self) -> Vec<String> {
        self.reasons
            .iter()
            .map(|reason| reason.explanation.clone())
            .collect()
    }
}

/// A returned worker result's presentation (spec 100 + 101).
///
/// Only the headline and detail are used by the parent's result card; the
/// action remains #100's own **Continue with result**, which is a separate
/// operation and not an approval.
pub fn returned_task_control(
    task_status: &str,
    report: Option<&CompletionGateReport>,
) -> TaskControl {
    returned_task_control_with_outcome(task_status, report, None)
}

/// [`returned_task_control`] with the typed terminal outcome a bounded task
/// stopped with (spec 103 Task 6). Historical returns have no outcome and keep
/// the pre-existing control shape.
pub fn returned_task_control_with_outcome(
    task_status: &str,
    report: Option<&CompletionGateReport>,
    outcome: Option<&crate::task_outcomes::TaskTerminalOutcome>,
) -> TaskControl {
    let Some(status) = TaskStatus::parse(task_status) else {
        return TaskControl {
            task_id: None,
            status: Some(task_status.to_string()),
            outcome: TaskOutcomeKind::Ended,
            dot: WorkerDot::Finished,
            group: WorkerBoardGroup::Finished,
            headline: "Worker finished".to_string(),
            detail: "The worker returned a result.".to_string(),
            needs_decision: false,
            ended: true,
            progress_current: true,
            show_round: false,
            reasons: Vec::new(),
            waivers: Vec::new(),
            primary_action: None,
            board_action: None,
            actions: Vec::new(),
            composer: None,
            terminal_outcome: outcome.cloned(),
        };
    };
    let task = Task {
        id: String::new(),
        session_id: String::new(),
        status,
        user_prompt: String::new(),
        model_provider: String::new(),
        model_name: String::new(),
        created_at_ms: 0,
        completed_at_ms: None,
    };
    let mut control = TaskControl::for_session_with_outcome(Some(&task), report, outcome);
    control.task_id = None;
    control.primary_action = None;
    control
}

/// The engine-built prompt for an explicit **Continue task** (spec 101 Task 3).
///
/// The user does not have to type "continue"; the action itself authorizes a
/// new run. The prompt names the durable completion reasons and waivers, says
/// whether the task's plan is carried, and states that nothing is approved by
/// continuing. A source task that settled `incomplete` may have only an unmet
/// required check while every step is terminal, and may have a blocked step, so
/// the prompt never claims that no plan was carried merely because no step is
/// open (2026-11-04 review fix R2).
///
/// An optional user note is appended as data, never as an instruction that
/// widens the execution boundary.
pub fn continuation_prompt(
    report: &CompletionGateReport,
    plan: Option<&TaskPlan>,
    note: Option<&str>,
) -> String {
    let mut prompt = String::from(
        "Continue the task that ended incomplete. The completion gate recorded these facts:\n",
    );
    for reason in &report.reasons {
        prompt.push_str(&format!("- {}\n", reason.explanation));
    }
    for waiver in &report.waivers {
        prompt.push_str(&format!("- {}\n", waiver.explanation));
    }
    if report.reasons.is_empty() && report.waivers.is_empty() {
        prompt.push_str("- The task was not closed.\n");
    }
    if let Some(plan) = plan {
        let blocked = plan
            .steps
            .iter()
            .any(|step| step.status == StepStatus::Blocked);
        prompt.push_str(
            "\nThe task's plan and its completed-step evidence are carried into this run",
        );
        if blocked {
            prompt.push_str(", including its blocked step");
        }
        prompt.push_str(
            ". Continue from the plan, finish its remaining work, and do not redo steps that \
             are already terminal. This continuation is not an approval: every patch, command, \
             check, question and fallback still needs its own explicit approval.\n",
        );
    } else {
        prompt.push_str(
            "\nThis task carries no plan. Continue from the recorded completion facts above and \
             close the outstanding verification or required check before the task can finish. \
             This continuation is not an approval: every patch, command, check, question and \
             fallback still needs its own explicit approval.\n",
        );
    }
    if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
        prompt.push_str("\nThe user added this note for the continuation:\n");
        prompt.push_str(note.trim());
        prompt.push('\n');
    }
    prompt
}

/// The engine's presentation for a live or reloaded turn.
///
/// The task status and model-run flags are typed inputs from the engine. A
/// browser draws the returned fields; it does not compare `taskStatus` values or
/// translate an outcome into a stop row or an action.
pub fn turn_presentation(
    status: &TaskStatus,
    model_incomplete: bool,
    cancelled: bool,
    task_id: Option<&str>,
) -> TurnPresentation {
    turn_presentation_with_outcome(status, model_incomplete, cancelled, task_id, None)
}

/// [`turn_presentation`] with the typed terminal outcome a bounded task stopped
/// with (spec 103 Task 6).
pub fn turn_presentation_with_outcome(
    status: &TaskStatus,
    model_incomplete: bool,
    cancelled: bool,
    task_id: Option<&str>,
    outcome: Option<&crate::task_outcomes::TaskTerminalOutcome>,
) -> TurnPresentation {
    let terminal_outcome = outcome.cloned();
    if cancelled {
        return TurnPresentation {
            indicator_state: TurnIndicatorState::Stopped,
            paused: false,
            show_task_control: false,
            stop_notice: Some(TurnStopNotice {
                state: TurnIndicatorState::Stopped,
                label: "Stopped by you".to_string(),
                hint: None,
                action: None,
            }),
            terminal_outcome,
        };
    }
    match status {
        TaskStatus::Cancelled => TurnPresentation {
            indicator_state: TurnIndicatorState::Stopped,
            paused: false,
            show_task_control: false,
            stop_notice: Some(TurnStopNotice {
                state: TurnIndicatorState::Stopped,
                label: "Stopped by you".to_string(),
                hint: None,
                action: None,
            }),
            terminal_outcome,
        },
        TaskStatus::Incomplete => TurnPresentation {
            indicator_state: TurnIndicatorState::Incomplete,
            paused: false,
            show_task_control: true,
            stop_notice: None,
            terminal_outcome,
        },
        TaskStatus::TaskBudgetExhausted
        | TaskStatus::TaskCompactionCannotFit
        | TaskStatus::TaskNoProgress
        | TaskStatus::TaskPrePlanExplorationExhausted => {
            let (_, headline, detail) = bounded_stop_presentation(status, outcome);
            TurnPresentation {
                indicator_state: TurnIndicatorState::Incomplete,
                paused: false,
                show_task_control: true,
                stop_notice: Some(TurnStopNotice {
                    state: TurnIndicatorState::Incomplete,
                    label: headline,
                    hint: Some(detail),
                    // Deliberately no continuation action against the same
                    // approved bound. A worker's ordinary Open control lives
                    // on `TaskControl`.
                    action: None,
                }),
                terminal_outcome,
            }
        }
        TaskStatus::Failed if outcome.is_some_and(usage_unavailable) => {
            let outcome = outcome.expect("checked above");
            TurnPresentation {
                indicator_state: TurnIndicatorState::Failed,
                paused: false,
                show_task_control: false,
                stop_notice: Some(TurnStopNotice {
                    state: TurnIndicatorState::Failed,
                    label: outcome.label(),
                    hint: Some(outcome.explanation()),
                    action: None,
                }),
                terminal_outcome,
            }
        }
        TaskStatus::ToolBudgetExhausted => TurnPresentation {
            indicator_state: TurnIndicatorState::Incomplete,
            paused: false,
            show_task_control: false,
            stop_notice: Some(TurnStopNotice {
                state: TurnIndicatorState::Incomplete,
                label: "Tool budget exhausted".to_string(),
                hint: None,
                action: Some(
                    TaskAction::new(
                        TaskCommand::AskAgain,
                        "Ask again",
                        TaskActionInput::None,
                        None,
                        TaskActionTransport::Stream,
                    )
                    .for_task(task_id),
                ),
            }),
            terminal_outcome,
        },
        TaskStatus::TokenBudgetExhausted => TurnPresentation {
            indicator_state: TurnIndicatorState::Incomplete,
            paused: false,
            show_task_control: false,
            stop_notice: Some(TurnStopNotice {
                state: TurnIndicatorState::Incomplete,
                label: "Token budget exhausted".to_string(),
                // Continuing means raising `agent_max_task_tokens` first. A
                // one-click button beside the number that stopped the spending
                // would be the wrong shape of control.
                hint: Some("Raise agent_max_task_tokens to go further.".to_string()),
                action: None,
            }),
            terminal_outcome,
        },
        TaskStatus::Failed | TaskStatus::UnknownExternalOutcome => TurnPresentation {
            indicator_state: TurnIndicatorState::Failed,
            paused: false,
            show_task_control: false,
            stop_notice: None,
            terminal_outcome,
        },
        TaskStatus::Interrupted => TurnPresentation {
            indicator_state: TurnIndicatorState::Incomplete,
            paused: false,
            show_task_control: false,
            stop_notice: Some(TurnStopNotice {
                state: TurnIndicatorState::Incomplete,
                label: "Task interrupted".to_string(),
                hint: None,
                action: None,
            }),
            terminal_outcome,
        },
        TaskStatus::Complete if model_incomplete => TurnPresentation {
            indicator_state: TurnIndicatorState::Incomplete,
            paused: false,
            show_task_control: false,
            stop_notice: Some(TurnStopNotice {
                state: TurnIndicatorState::Incomplete,
                label: "Stopped before finishing".to_string(),
                hint: None,
                action: None,
            }),
            terminal_outcome,
        },
        TaskStatus::Complete => TurnPresentation {
            indicator_state: TurnIndicatorState::Complete,
            paused: false,
            show_task_control: false,
            stop_notice: None,
            terminal_outcome,
        },
        // Everything below is still in progress or waiting for the user. It is
        // explicitly not success, including `waiting_for_approval`, whose
        // proposal card is drawn from its own payload rather than from a
        // completion indicator.
        TaskStatus::Created
        | TaskStatus::PreparingContext
        | TaskStatus::WaitingForModel
        | TaskStatus::RunningTool
        | TaskStatus::WaitingForApproval
        | TaskStatus::Paused
        | TaskStatus::ApplyingPatch
        | TaskStatus::Validating => TurnPresentation {
            indicator_state: TurnIndicatorState::Pending,
            paused: matches!(status, TaskStatus::WaitingForApproval | TaskStatus::Paused),
            show_task_control: false,
            stop_notice: None,
            terminal_outcome,
        },
    }
}

/// Reads durable task facts for an ordinary session or a worker.
///
/// Presentation only: it validates that the session exists, but the
/// continuation operation re-validates the newest task and its report under the
/// session log's lock before writing anything.
#[derive(Debug, Clone, Copy)]
pub struct TaskControlService<'a> {
    session_store: &'a SessionStore,
    repository_root: Option<&'a Path>,
}

impl<'a> TaskControlService<'a> {
    pub fn new(session_store: &'a SessionStore, repository_root: Option<&'a Path>) -> Self {
        Self {
            session_store,
            repository_root,
        }
    }

    pub fn session_control(&self, session_id: &str) -> Result<Option<TaskControl>> {
        let session = self.session_store.read_session(session_id)?;
        let Some(session) = session else {
            return Err(ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        };
        if let Some(root) = self.repository_root
            && crate::hash::repository_id_for_root(root) != session.repository_id
        {
            return Err(ClientError::AccessDenied(format!(
                "Session {session_id} does not belong to repository {}",
                root.display()
            )));
        }
        let Some(task) = self
            .session_store
            .read_tasks(session_id)?
            .into_iter()
            .last()
        else {
            return Ok(None);
        };
        let report = self
            .session_store
            .read_completion_report(session_id, &task.id)?;
        let outcome = self.session_store.read_task_outcome(session_id, &task.id)?;
        Ok(Some(TaskControl::for_session_with_outcome(
            Some(&task),
            report.as_ref(),
            outcome.as_ref(),
        )))
    }

    pub fn worker_control(
        &self,
        session_id: &str,
        facts: WorkerTaskFacts<'_>,
    ) -> Result<TaskControl> {
        if self.session_store.read_session(session_id)?.is_none() {
            return Err(ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        }
        let task = self
            .session_store
            .read_tasks(session_id)?
            .into_iter()
            .last();
        let report = match &task {
            Some(task) => self
                .session_store
                .read_completion_report(session_id, &task.id)?,
            None => None,
        };
        let outcome = match &task {
            Some(task) => self.session_store.read_task_outcome(session_id, &task.id)?,
            None => None,
        };
        Ok(TaskControl::for_worker_with_outcome(
            WorkerTaskFacts {
                task: task.as_ref(),
                reported_task_status: facts.reported_task_status,
                report: report.as_ref(),
                alive: facts.alive,
                state: facts.state,
                pending_question: facts.pending_question,
                has_run: facts.has_run,
                turn_error: facts.turn_error,
                last_progress_at_ms: facts.last_progress_at_ms,
                now_ms: facts.now_ms,
            },
            outcome.as_ref(),
        ))
    }
}

fn lifecycle_action(command: TaskCommand, label: &str) -> TaskAction {
    TaskAction::new(
        command,
        label,
        TaskActionInput::None,
        None,
        TaskActionTransport::Worker,
    )
}

fn restart_action() -> TaskAction {
    lifecycle_action(TaskCommand::RestartWorker, "Restart worker")
}

fn open_action() -> TaskAction {
    TaskAction::new(
        TaskCommand::OpenWorker,
        "Open",
        TaskActionInput::None,
        None,
        TaskActionTransport::Navigate,
    )
}

fn review_action() -> TaskAction {
    TaskAction::new(
        TaskCommand::ReviewDecision,
        "Review",
        TaskActionInput::None,
        None,
        TaskActionTransport::Navigate,
    )
}

fn continue_message_action() -> TaskAction {
    TaskAction::new(
        TaskCommand::SendMessage,
        "Continue task",
        TaskActionInput::Message,
        Some("Tell the worker how to continue this unfinished task".to_string()),
        TaskActionTransport::Worker,
    )
}

fn composer_for(action: TaskAction) -> TaskComposer {
    let placeholder = action
        .placeholder
        .clone()
        .unwrap_or_else(|| action.label.clone());
    TaskComposer {
        action,
        placeholder,
    }
}

/// The engine-owned headline and detail for a bounded task stop.
fn bounded_stop_presentation(
    status: &TaskStatus,
    outcome: Option<&crate::task_outcomes::TaskTerminalOutcome>,
) -> (TaskOutcomeKind, String, String) {
    let kind = match status {
        TaskStatus::TaskBudgetExhausted => TaskOutcomeKind::BudgetExhausted,
        TaskStatus::TaskCompactionCannotFit => TaskOutcomeKind::CannotFit,
        TaskStatus::TaskNoProgress => TaskOutcomeKind::NoProgress,
        TaskStatus::TaskPrePlanExplorationExhausted => TaskOutcomeKind::PrePlanExplorationExhausted,
        _ => TaskOutcomeKind::Failed,
    };
    match outcome {
        Some(outcome) => (kind, outcome.label(), outcome.explanation()),
        None => match status {
            TaskStatus::TaskBudgetExhausted => (
                kind,
                "Approved task budget exhausted".to_string(),
                "The worker reached the task budget the user approved before it could finish."
                    .to_string(),
            ),
            TaskStatus::TaskCompactionCannotFit => (
                kind,
                "Request cannot fit".to_string(),
                "The next request could not fit the configured request-window share.".to_string(),
            ),
            TaskStatus::TaskNoProgress => (
                kind,
                "Task made no progress".to_string(),
                "The task repeated its recent state without advancing, so the engine stopped it."
                    .to_string(),
            ),
            TaskStatus::TaskPrePlanExplorationExhausted => (
                kind,
                "Pre-plan exploration exhausted".to_string(),
                "The worker spent its pre-plan exploration budget and did not propose a plan when asked."
                    .to_string(),
            ),
            _ => (
                kind,
                "Task stopped".to_string(),
                "The task stopped before completion.".to_string(),
            ),
        },
    }
}

fn usage_unavailable(outcome: &crate::task_outcomes::TaskTerminalOutcome) -> bool {
    outcome.kind == crate::task_outcomes::TaskTerminalOutcomeKind::UsageUnavailable
}

impl TaskControl {
    /// The same bounded-stop control for a live, idle, or restarted worker.
    fn bounded_worker(
        task_id: Option<&str>,
        status_text: &str,
        status: &TaskStatus,
        outcome: Option<&crate::task_outcomes::TaskTerminalOutcome>,
        report: Option<&CompletionGateReport>,
    ) -> Self {
        let (outcome_kind, headline, detail) = bounded_stop_presentation(status, outcome);
        Self {
            task_id: task_id.map(str::to_string),
            status: Some(status_text.to_string()),
            outcome: outcome_kind,
            dot: WorkerDot::Failed,
            group: WorkerBoardGroup::Finished,
            headline,
            detail,
            needs_decision: false,
            ended: true,
            progress_current: true,
            show_round: false,
            reasons: report
                .map(|report| report.reasons.clone())
                .unwrap_or_default(),
            waivers: report
                .map(|report| report.waivers.clone())
                .unwrap_or_default(),
            // No continuation against the same approved bound. Opening the
            // worker is the engine's ordinary navigation lifecycle action.
            primary_action: None,
            board_action: Some(open_action()),
            actions: Vec::new(),
            composer: None,
            terminal_outcome: outcome.cloned(),
        }
    }
}

fn completion_detail(report: Option<&CompletionGateReport>, prefix: &str) -> String {
    let Some(report) = report else {
        return prefix.to_string();
    };
    let mut parts = Vec::new();
    for reason in &report.reasons {
        parts.push(reason.explanation.clone());
    }
    for waiver in &report.waivers {
        parts.push(waiver.explanation.clone());
    }
    if parts.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} {}", parts.join(" "))
    }
}

fn progress_is_current(last_progress_at_ms: Option<u128>, now_ms: u128) -> bool {
    last_progress_at_ms
        .is_some_and(|last| now_ms.saturating_sub(last) < WORKER_STALE_PROGRESS_AFTER_MS)
}

fn worker_activity_detail(last_progress_at_ms: Option<u128>, now_ms: u128) -> String {
    let Some(last) = last_progress_at_ms else {
        return "Waiting for the worker to report progress.".to_string();
    };
    let elapsed = now_ms.saturating_sub(last);
    if elapsed < WORKER_STALE_PROGRESS_AFTER_MS {
        "Worker progress is up to date.".to_string()
    } else {
        format!(
            "No worker progress for {}. It may still be waiting for the model.",
            format_elapsed(elapsed)
        )
    }
}

/// The same compact duration the worker strip showed before this moved into the
/// engine: `12s`, `1m 05s`, `1h 02m`.
fn format_elapsed(ms: u128) -> String {
    let seconds = ms / 1000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {:02}s", seconds % 60);
    }
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion::CompletionReasonCode;
    use crate::plan::PlanStep;

    fn task(status: TaskStatus) -> Task {
        Task {
            id: "task_1".to_string(),
            session_id: "ses_1".to_string(),
            status,
            user_prompt: "Do it".to_string(),
            model_provider: "provider".to_string(),
            model_name: "model".to_string(),
            created_at_ms: 1,
            completed_at_ms: None,
        }
    }

    fn incomplete_report() -> CompletionGateReport {
        CompletionGateReport {
            task_id: "task_1".to_string(),
            eligible: false,
            reasons: vec![CompletionReason {
                code: CompletionReasonCode::OpenPlanSteps,
                explanation: "1 planned step is still open: \"Write it\".".to_string(),
            }],
            waivers: Vec::new(),
        }
    }

    #[test]
    fn an_ordinary_incomplete_task_offers_engine_continue_without_a_typed_word() {
        let task = task(TaskStatus::Incomplete);
        let report = incomplete_report();
        let control = TaskControl::for_session(Some(&task), Some(&report));
        assert_eq!(control.status.as_deref(), Some("incomplete"));
        assert_eq!(control.outcome, TaskOutcomeKind::Incomplete);
        let action = control
            .primary_action
            .as_ref()
            .expect("continue is offered");
        assert_eq!(action.command, TaskCommand::ContinueTask);
        assert_eq!(action.input, TaskActionInput::OptionalNote);
        assert_eq!(action.transport, TaskActionTransport::Stream);
        assert_eq!(
            control.reason_sentences(),
            vec![report.reasons[0].explanation.clone()]
        );
    }

    #[test]
    fn a_complete_task_offers_no_continuation() {
        let task = task(TaskStatus::Complete);
        let report = CompletionGateReport {
            task_id: "task_1".to_string(),
            eligible: true,
            reasons: Vec::new(),
            waivers: Vec::new(),
        };
        let control = TaskControl::for_session(Some(&task), Some(&report));
        assert_eq!(control.outcome, TaskOutcomeKind::Complete);
        assert!(control.primary_action.is_none());
    }

    #[test]
    fn worker_complete_and_incomplete_offer_different_primary_actions() {
        let complete = task(TaskStatus::Complete);
        let complete_control = TaskControl::for_worker(WorkerTaskFacts {
            task: Some(&complete),
            reported_task_status: None,
            report: None,
            alive: true,
            state: WorkerProcessState::Idle,
            pending_question: false,
            has_run: true,
            turn_error: None,
            last_progress_at_ms: None,
            now_ms: 0,
        });
        let complete_action = complete_control.primary_action.expect("follow-up");
        assert_eq!(complete_action.command, TaskCommand::SendFollowUp);
        assert_eq!(complete_action.label, "Send follow-up");

        let incomplete = task(TaskStatus::Incomplete);
        let report = incomplete_report();
        let incomplete_control = TaskControl::for_worker(WorkerTaskFacts {
            task: Some(&incomplete),
            reported_task_status: None,
            report: Some(&report),
            alive: true,
            state: WorkerProcessState::Idle,
            pending_question: false,
            has_run: true,
            turn_error: None,
            last_progress_at_ms: None,
            now_ms: 0,
        });
        let incomplete_action = incomplete_control.primary_action.expect("continue");
        assert_eq!(incomplete_action.command, TaskCommand::ContinueTask);
        assert_eq!(incomplete_action.label, "Continue task");
        assert_eq!(incomplete_control.status.as_deref(), Some("incomplete"));
        assert!(incomplete_control.detail.contains("1 planned step"));
    }

    #[test]
    fn a_pending_decision_suppresses_worker_composer_actions() {
        let waiting = task(TaskStatus::WaitingForApproval);
        let control = TaskControl::for_worker(WorkerTaskFacts {
            task: Some(&waiting),
            reported_task_status: None,
            report: None,
            alive: true,
            state: WorkerProcessState::Idle,
            pending_question: false,
            has_run: true,
            turn_error: None,
            last_progress_at_ms: None,
            now_ms: 0,
        });
        assert!(control.needs_decision);
        assert_eq!(control.group, WorkerBoardGroup::NeedsYou);
        assert!(control.composer.is_none());
        let action = control.primary_action.expect("review");
        assert_eq!(action.command, TaskCommand::ReviewDecision);
    }

    #[test]
    fn a_waiting_question_reads_as_needs_your_answer_not_a_decision() {
        let waiting = task(TaskStatus::WaitingForApproval);
        let control = TaskControl::for_worker(WorkerTaskFacts {
            task: Some(&waiting),
            reported_task_status: None,
            report: None,
            alive: true,
            state: WorkerProcessState::WaitingForApproval,
            pending_question: true,
            has_run: true,
            turn_error: None,
            last_progress_at_ms: None,
            now_ms: 0,
        });
        assert!(control.needs_decision, "a waiting question still blocks");
        assert_eq!(control.outcome, TaskOutcomeKind::NeedsAnswer);
        assert_eq!(control.dot, WorkerDot::NeedsYou);
        assert_eq!(control.group, WorkerBoardGroup::NeedsYou);
        assert_eq!(control.headline, "Needs your answer");
        assert_ne!(control.headline, "Needs your decision");
        assert!(control.composer.is_none());
        let action = control.primary_action.expect("review");
        assert_eq!(action.command, TaskCommand::ReviewDecision);
        assert_eq!(
            control.board_action.map(|action| action.command),
            Some(TaskCommand::ReviewDecision)
        );
    }

    #[test]
    fn a_reported_or_typed_waiting_state_needs_a_decision_without_a_shell_flag() {
        let running = task(TaskStatus::RunningTool);
        let reported = TaskControl::for_worker(WorkerTaskFacts {
            task: Some(&running),
            reported_task_status: Some(TaskStatus::WaitingForApproval),
            report: None,
            alive: true,
            state: WorkerProcessState::Idle,
            pending_question: false,
            has_run: true,
            turn_error: None,
            last_progress_at_ms: None,
            now_ms: 0,
        });
        assert!(reported.needs_decision);
        assert_eq!(reported.outcome, TaskOutcomeKind::NeedsDecision);

        let typed = TaskControl::for_worker(WorkerTaskFacts {
            task: Some(&running),
            reported_task_status: None,
            report: None,
            alive: true,
            state: WorkerProcessState::WaitingForApproval,
            pending_question: false,
            has_run: true,
            turn_error: None,
            last_progress_at_ms: None,
            now_ms: 0,
        });
        assert!(typed.needs_decision);
        assert_eq!(typed.outcome, TaskOutcomeKind::NeedsDecision);
    }

    #[test]
    fn only_a_complete_task_gets_a_complete_turn_indicator() {
        for status in TaskStatus::all() {
            let presentation = turn_presentation(&status, false, false, Some("task_1"));
            if matches!(status, TaskStatus::Complete) {
                assert_eq!(
                    presentation.indicator_state,
                    TurnIndicatorState::Complete,
                    "{status:?}"
                );
            } else {
                assert_ne!(
                    presentation.indicator_state,
                    TurnIndicatorState::Complete,
                    "{status:?}"
                );
            }
        }
        for status in TaskStatus::all().into_iter().filter(TaskStatus::is_active) {
            let presentation = turn_presentation(&status, false, false, Some("task_1"));
            assert_eq!(
                presentation.indicator_state,
                TurnIndicatorState::Pending,
                "{status:?}"
            );
        }

        let waiting = turn_presentation(
            &TaskStatus::WaitingForApproval,
            false,
            false,
            Some("task_1"),
        );
        assert_eq!(waiting.indicator_state, TurnIndicatorState::Pending);
        assert!(
            waiting.paused,
            "a reload groups a pending approval as paused"
        );
        assert!(waiting.stop_notice.is_none());

        let paused = turn_presentation(&TaskStatus::Paused, false, false, Some("task_1"));
        assert_eq!(paused.indicator_state, TurnIndicatorState::Pending);
        assert!(paused.paused);

        let complete = turn_presentation(&TaskStatus::Complete, false, false, Some("task_1"));
        assert_eq!(
            complete.indicator_state,
            TurnIndicatorState::Complete,
            "only the settled complete status is success"
        );
        assert!(!complete.paused);
    }

    #[test]
    fn stale_progress_is_an_engine_sentence() {
        let running = task(TaskStatus::RunningTool);
        let control = TaskControl::for_worker(WorkerTaskFacts {
            task: Some(&running),
            reported_task_status: None,
            report: None,
            alive: true,
            state: WorkerProcessState::Running,
            pending_question: false,
            has_run: true,
            turn_error: None,
            last_progress_at_ms: Some(1_000),
            now_ms: 46_000,
        });
        assert!(!control.progress_current);
        assert_eq!(
            control.detail,
            "No worker progress for 45s. It may still be waiting for the model."
        );
    }

    #[test]
    fn a_continuation_action_names_its_exact_task_and_wire_command() {
        let task = task(TaskStatus::Incomplete);
        let report = incomplete_report();
        let control = TaskControl::for_worker(WorkerTaskFacts {
            task: Some(&task),
            reported_task_status: None,
            report: Some(&report),
            alive: true,
            state: WorkerProcessState::Idle,
            pending_question: false,
            has_run: true,
            turn_error: None,
            last_progress_at_ms: None,
            now_ms: 0,
        });
        let action = control.primary_action.expect("continue");
        assert_eq!(action.task_id.as_deref(), Some("task_1"));
        assert_eq!(action.wire_command.as_deref(), Some("continue-task"));
        let composer = control.composer.expect("composer");
        assert_eq!(composer.action.task_id.as_deref(), Some("task_1"));

        let ordinary = TaskControl::for_session(Some(&task), Some(&report));
        let action = ordinary.primary_action.expect("continue");
        assert_eq!(action.task_id.as_deref(), Some("task_1"));
        assert!(
            action.wire_command.is_none(),
            "a streamed action has no worker wire name"
        );
    }

    #[test]
    fn a_budget_stop_notice_and_its_action_are_engine_presentation() {
        let tool = turn_presentation(
            &TaskStatus::ToolBudgetExhausted,
            false,
            false,
            Some("task_1"),
        );
        assert_eq!(tool.indicator_state, TurnIndicatorState::Incomplete);
        let notice = tool.stop_notice.expect("tool budget notice");
        assert_eq!(notice.state, TurnIndicatorState::Incomplete);
        assert_eq!(notice.label, "Tool budget exhausted");
        let action = notice.action.expect("ask again");
        assert_eq!(action.command, TaskCommand::AskAgain);
        assert_eq!(action.task_id.as_deref(), Some("task_1"));

        let token = turn_presentation(
            &TaskStatus::TokenBudgetExhausted,
            false,
            false,
            Some("task_1"),
        );
        let notice = token.stop_notice.expect("token budget notice");
        assert_eq!(notice.label, "Token budget exhausted");
        assert!(notice.hint.unwrap().contains("agent_max_task_tokens"));
        assert!(notice.action.is_none());
    }

    #[test]
    fn a_returned_incomplete_result_is_never_success() {
        let report = incomplete_report();
        let control = returned_task_control("incomplete", Some(&report));
        assert_eq!(control.outcome, TaskOutcomeKind::Incomplete);
        assert_eq!(control.headline, "Task incomplete");
        assert_ne!(control.outcome, TaskOutcomeKind::Complete);
    }

    #[test]
    fn a_continuation_prompt_names_the_carried_plan_even_when_no_step_is_open() {
        let planned = continuation_prompt(
            &incomplete_report(),
            Some(&TaskPlan::new("task_1", 0)),
            None,
        );
        assert!(
            planned.contains("plan and its completed-step evidence"),
            "{planned}"
        );
        assert!(planned.contains("not an approval"), "{planned}");

        let blocked_report = CompletionGateReport {
            task_id: "task_1".to_string(),
            eligible: false,
            reasons: vec![CompletionReason {
                code: CompletionReasonCode::BlockedPlanSteps,
                explanation: "1 planned step is blocked: \"Run tests\".".to_string(),
            }],
            waivers: Vec::new(),
        };
        let mut blocked_plan = TaskPlan::new("task_1", 0);
        blocked_plan.steps.push(PlanStep {
            id: "test".to_string(),
            title: "Run tests".to_string(),
            detail: None,
            status: StepStatus::Blocked,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
        });
        let prompt = continuation_prompt(
            &blocked_report,
            Some(&blocked_plan),
            Some("Run the checks."),
        );
        assert!(!prompt.contains("No plan step is open"), "{prompt}");
        assert!(prompt.contains("including its blocked step"), "{prompt}");
        assert!(prompt.contains("Run the checks."), "{prompt}");

        let verification_only = CompletionGateReport {
            task_id: "task_1".to_string(),
            eligible: false,
            reasons: vec![CompletionReason {
                code: CompletionReasonCode::VerificationDue,
                explanation: "Verification is due and has not run.".to_string(),
            }],
            waivers: Vec::new(),
        };
        let prompt = continuation_prompt(&verification_only, None, Some("Run the checks."));
        assert!(prompt.contains("carries no plan"), "{prompt}");
        assert!(!prompt.contains("No plan step is open"), "{prompt}");
        assert!(prompt.contains("Run the checks."), "{prompt}");
    }
}
