//! Classifying what a crash left behind, per
//! `docs/specs/17_durable_task_state_and_crash_recovery/proposal.md` §5.4.
//!
//! Classification is a pure function of a replayed session log: for every task
//! whose latest status is not terminal, a dangling `action_started` with
//! `sideEffecting: true` means the outcome is unknown, and anything else means
//! the task was merely interrupted.
//!
//! The classifier makes one write, and only one: spec 19 §5.5 requires a model
//! call lost to the crash to be accounted for, and the sweep is the only place
//! that knows one was lost. It is an append, it is idempotent across launches,
//! and it changes no classification — see `account_for_lost_model_calls`.
//!
//! The module's reason for existing is requirement 5 — no action whose outcome
//! is unknown is ever automatically repeated. That decision is made *here*
//! rather than by a caller, because spec 45 will present it from a webview and
//! a guarantee enforced only in a webview is not enforced.

use crate::audit::AuditLog;
use crate::chat::PausedTurns;
use crate::clarification::PendingDecision;
use crate::edit::PatchStore;
use crate::error::{ClientError, Result};
use crate::model::TokenUsage;
use crate::patch_engine::ProposedPatch;
use crate::plan::TaskPlan;
use crate::session::{DanglingAction, PendingApprovalRef, SessionStore, TaskStatus};
use crate::validation::{CommandProposal, CommandStore};
use crate::verification::{VerificationEvent, fold_verification};

/// A task that was in flight when the process stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredTask {
    pub task_id: String,
    pub session_id: String,
    /// Always [`TaskStatus::Interrupted`] or
    /// [`TaskStatus::UnknownExternalOutcome`] — the classifier's two outputs.
    pub classification: TaskStatus,
    /// The action that started and never finished, when there was one. This is
    /// the evidence: spec 45 renders "a patch application was in progress" from
    /// it rather than re-deriving anything.
    pub dangling: Option<DanglingAction>,
    /// The status the task was left in, before classification.
    pub previous_status: String,
    /// Whether this task may be resumed without asking. Decided here, so a
    /// caller cannot widen it.
    pub auto_resume_permitted: bool,
}

/// Classifies every non-terminal task in one session.
///
/// A torn log tail is audited rather than swallowed: a discarded line means a
/// write was interrupted, which is evidence of the crash being classified.
/// `SessionStore` has no `AuditLog` of its own, so the audit happens here — the
/// caller that actually cares.
pub fn classify_session(
    store: &SessionStore,
    audit: &AuditLog,
    session_id: &str,
) -> Result<Vec<RecoveredTask>> {
    // Spec 57 correction C12: a live worker's tasks are running in that
    // process, not left by a crash. Checked before anything is audited or
    // written, so the sweep records nothing about them — not even a lost model
    // call, which is still in flight.
    if store.has_live_worker(session_id)? {
        return Ok(Vec::new());
    }
    let unreadable = store.unreadable_event_count(session_id)?;
    if unreadable > 0 {
        audit.record(
            "session_log_truncated_tail",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("discardedLines", unreadable.to_string()),
            ],
        )?;
    }

    let statuses = store.read_task_statuses(session_id)?;
    let dangling = store.dangling_actions(session_id)?;

    account_for_lost_model_calls(store, audit, session_id, &dangling)?;
    let is_worker = store.worker_record(session_id)?.is_some();

    let mut recovered = Vec::new();

    for (task_id, raw_status) in statuses {
        let status = TaskStatus::parse(&raw_status);
        // An unrecognised status is not assumed benign. It is treated as
        // non-terminal and not auto-resumable, because a status this version
        // does not understand is precisely the case where guessing is unsafe.
        let is_terminal = status.as_ref().is_some_and(TaskStatus::is_terminal);
        if is_terminal {
            continue;
        }
        // §5.4 rule 3: a task waiting for a human was not interrupted
        // mid-action. It keeps its status and its proposal is reattached
        // (§5.5), so it is not a recovered task.
        if status == Some(TaskStatus::WaitingForApproval) {
            continue;
        }

        let task_dangling = dangling
            .iter()
            .find(|action| action.task_id == task_id)
            .cloned();

        // §5.4 rules 1 and 2.
        let classification = match &task_dangling {
            Some(action) if action.side_effecting => TaskStatus::UnknownExternalOutcome,
            _ => TaskStatus::Interrupted,
        };

        // Requirement 6 permits auto-resume only for read-only work. Five
        // conditions, each blocking on its own:
        //
        // 1. The classification must be `Interrupted` — an unknown outcome is
        //    never resumed automatically.
        // 2. The *status* must not itself say a side effect could be in flight.
        //    This is a backstop for a marker that never reached the log at all:
        //    the marker is the primary signal, but a task left in `running_tool`
        //    with no marker is not evidence of safety, it is absence of
        //    evidence.
        // 3. A legacy `running` task is never auto-resumed. §5.6: it carries
        //    exactly the information this work exists to eliminate — something
        //    was in flight and nothing recorded what — so "resumable" cannot be
        //    concluded from it, even though it classifies as `Interrupted`.
        // 4. A task already *recorded* as `unknown_external_outcome` is never
        //    auto-resumed. §5.1's table says "never auto-retry" for that state,
        //    and today that rests entirely on the classification being
        //    recomputed from a marker which, being append-only, is still
        //    dangling. That holds only while nothing persists the
        //    classification — the moment something does (spec 45 keeping a
        //    recovery list across a second crash), a status that says the
        //    outcome is unknown would otherwise come back off disk and
        //    re-derive as resumable if its marker were out of reach.
        // 5. A worker's task is never auto-resumed in the shell (spec 57
        //    correction C5). Its repository is a worktree and its host process
        //    is gone; the user restarts the worker instead.
        let status_allows = status
            .as_ref()
            .is_some_and(|status| !status.may_have_side_effect_in_flight());
        let auto_resume_permitted = classification == TaskStatus::Interrupted
            && status_allows
            && raw_status != "running"
            && status != Some(TaskStatus::UnknownExternalOutcome)
            && !is_worker;

        let task = RecoveredTask {
            task_id: task_id.clone(),
            session_id: session_id.to_string(),
            classification: classification.clone(),
            dangling: task_dangling.clone(),
            previous_status: raw_status.clone(),
            auto_resume_permitted,
        };

        // Recorded so a recovery decision is auditable rather than a conclusion
        // something reached once and forgot (§5.4).
        audit.record(
            "task_recovered",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("taskId", task_id.clone()),
                ("previousStatus", raw_status.clone()),
                ("classification", classification.as_str().to_string()),
                (
                    "danglingAction",
                    task_dangling
                        .as_ref()
                        .map(|action| action.action.clone())
                        .unwrap_or_default(),
                ),
                (
                    "danglingRef",
                    task_dangling
                        .as_ref()
                        .map(|action| action.reference.clone())
                        .unwrap_or_default(),
                ),
                (
                    "danglingSeq",
                    task_dangling
                        .as_ref()
                        .map(|action| action.seq.to_string())
                        .unwrap_or_default(),
                ),
                ("autoResumePermitted", auto_resume_permitted.to_string()),
            ],
        )?;

        recovered.push(task);
    }

    recovered.sort_by(|left, right| left.task_id.cmp(&right.task_id));
    Ok(recovered)
}

/// Appends an estimated usage event for every model call that started and
/// never finished, per spec 19 §5.5.
///
/// A call in flight when the process died was billed and its answer is gone.
/// Leaving it out would make a crash look free, which is the under-reporting
/// that spec's §5.5 exists to prevent. The figure comes from the estimate
/// written onto the marker *before* the request went out, because the request
/// itself no longer exists.
///
/// Guarded by marker id rather than by a flag: this runs on every launch, and
/// an unguarded append would grow the reported spend each time the app starts.
fn account_for_lost_model_calls(
    store: &SessionStore,
    audit: &AuditLog,
    session_id: &str,
    dangling: &[DanglingAction],
) -> Result<()> {
    let already_accounted = store.usage_marker_ids(session_id)?;
    for action in dangling
        .iter()
        .filter(|action| action.action == "model_call")
    {
        if already_accounted.contains(&action.marker_id) {
            continue;
        }
        // A marker written before spec 19 carries no estimate. Nothing
        // truthful can be said about it, and inventing a number would be
        // worse than the gap.
        let Some(estimate) = action.estimated_input_tokens else {
            continue;
        };
        store.record_task_usage_for_task_id(
            session_id,
            &action.task_id,
            &format!("lost_{}", action.marker_id),
            Some(&action.marker_id),
            TokenUsage::estimated(estimate, 0),
            None,
            Some("lost_to_crash"),
        )?;
        audit.record(
            "task_usage_recorded_for_lost_call",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("taskId", action.task_id.clone()),
                ("estimatedInputTokens", estimate.to_string()),
            ],
        )?;
    }
    Ok(())
}

/// Classifies every non-terminal task across every session in the data
/// directory. This is the launch-time sweep.
pub fn classify_all(store: &SessionStore, audit: &AuditLog) -> Result<Vec<RecoveredTask>> {
    let mut recovered = Vec::new();
    for session in store.list_sessions(None)? {
        recovered.extend(classify_session(store, audit, &session.id)?);
    }
    Ok(recovered)
}

/// A verification that is due and not being run (spec 23 §9).
///
/// Recovery reports it; it runs nothing. The shell offers it, and running it
/// after a restart without asking would be the same unrequested side effect
/// spec 17 refuses for commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingVerification {
    /// The task that proposed the patch, from the `Due` event.
    pub task_id: String,
    pub patch_id: String,
}

/// Every due verification in a session that nothing is currently running.
///
/// A task in an active state, or the verification turn parked on a check batch,
/// is already the thing the user sees; the due event behind it is not a second
/// offer.
pub fn pending_verifications(
    store: &SessionStore,
    session_id: &str,
) -> Result<Vec<PendingVerification>> {
    let events = store.read_verification_events(session_id, &[])?;
    let state = fold_verification(events.iter());
    let Some(due) = state.due else {
        return Ok(Vec::new());
    };
    let statuses = store.read_task_statuses(session_id)?;
    let busy = statuses
        .values()
        .filter_map(|raw| TaskStatus::parse(raw))
        .any(|status| {
            matches!(
                status,
                TaskStatus::PreparingContext
                    | TaskStatus::WaitingForModel
                    | TaskStatus::RunningTool
                    | TaskStatus::ApplyingPatch
                    | TaskStatus::Validating
            )
        });
    let latest_started = events.iter().rev().find_map(|event| match event {
        VerificationEvent::Started { task_id, .. } => Some(task_id.as_str()),
        _ => None,
    });
    let waiting_on_batch = latest_started
        .and_then(|task_id| statuses.get(task_id))
        .and_then(|raw| TaskStatus::parse(raw))
        == Some(TaskStatus::WaitingForApproval);
    if busy || waiting_on_batch {
        return Ok(Vec::new());
    }
    Ok(vec![PendingVerification {
        task_id: due.task_id,
        patch_id: due.patch_id,
    }])
}

/// A task that was awaiting a human decision when the process stopped, with the
/// proposal it was waiting on — or the reason it could not be produced.
#[derive(Debug, Clone)]
pub enum ReattachedApproval {
    Command {
        task_id: String,
        proposal: CommandProposal,
    },
    Patch {
        task_id: String,
        patch: ProposedPatch,
    },
    /// A plan the turn had paused to have reviewed. Added when spec 21 made
    /// plan review a third kind of pending approval; before that, a crash
    /// during a review failed the task with "unknown pending approval kind
    /// plan" and destroyed a turn that was in fact recoverable.
    ///
    /// The plan comes from the session log and the deferred action from the
    /// paused turn, so both halves of the card are read back rather than
    /// rebuilt — and the paused turn's absence is what makes the review
    /// unrestorable, since approving the plan would have nothing to continue.
    Plan {
        task_id: String,
        proposal_id: String,
        plan: TaskPlan,
        deferred_action: String,
    },
    /// A provider-fallback offer the turn had paused on (spec 56). Like a
    /// plan review its paused turn is on disk, so the offer is made again
    /// rather than lost; a paused turn that is gone fails the task instead,
    /// because approving would have nothing to re-issue.
    ProviderFallback {
        task_id: String,
        proposal_id: String,
        fallback_provider: String,
    },
    /// A round-budget pause the turn had reached before its forced final call
    /// (spec 61). Like a provider-fallback offer its paused turn is on disk, so
    /// the pause is re-presented rather than lost; a paused turn that is gone
    /// fails the task instead, because resuming would have nothing to continue.
    RoundBudget {
        task_id: String,
        proposal_id: String,
        rounds_used: u32,
        rounds_granted: u32,
    },
    /// A verification check batch waiting for one card's decisions (spec 23).
    /// Its paused turn is on disk, so the batch is re-presented rather than
    /// lost; a paused turn that is gone, or one with no proposals, fails the
    /// task instead, because answering would have nothing to run.
    CheckBatch {
        task_id: String,
        proposal_id: String,
        /// The proposal ids in the batch, in card order.
        checks: Vec<String>,
    },
    /// A question the turn paused on (spec 50). Unlike the other kinds its
    /// state is not a proposal file: the question and its answer are events in
    /// the session log, so this is read back from `question_state`.
    ///
    /// It carries no proposal id. Answering it goes through the question route,
    /// never a command or patch endpoint, and nothing here can be approved.
    Question {
        task_id: String,
        question_id: String,
        question: String,
        options: Vec<String>,
        allow_free_text: bool,
        /// The answer and, when it was one of the options, which one. Present
        /// when the crash landed after the answer was recorded: the resumed
        /// turn then uses it instead of asking again (requirement 4).
        answered: Option<(String, Option<usize>)>,
    },
    /// The link or the proposal file is gone, so the task was failed.
    ///
    /// §5.5: never an approval card reconstructed from partial data. The user
    /// would be approving a command Damaian is guessing at, which is worse than
    /// losing the task.
    Unavailable { task_id: String, reason: String },
}

/// Reattaches the stored proposal to every task left awaiting approval (§5.5).
///
/// A task whose proposal cannot be loaded is marked [`TaskStatus::Failed`] with
/// the reason, rather than being left waiting on something that no longer
/// exists.
pub fn reattach_pending_approvals(
    store: &SessionStore,
    audit: &AuditLog,
    commands: &CommandStore,
    patches: &PatchStore,
    paused: &PausedTurns,
    session_id: &str,
) -> Result<Vec<ReattachedApproval>> {
    // A live worker is waiting on its own approval (spec 57 correction C12).
    // Reattaching it would let the shell answer it behind the worker's back.
    if store.has_live_worker(session_id)? {
        return Ok(Vec::new());
    }
    let mut reattached = Vec::new();
    for (task_id, raw_status) in store.read_task_statuses(session_id)? {
        if TaskStatus::parse(&raw_status) != Some(TaskStatus::WaitingForApproval) {
            continue;
        }

        let pending = store.pending_decision_for(session_id, &task_id)?;
        let outcome = match &pending {
            // Written by a version that recorded the status but not the link.
            // There is nothing to reattach and nothing to guess from.
            None => Err("the task records no pending proposal".to_string()),
            Some(PendingDecision::Approval(PendingApprovalRef { kind, proposal_id }))
                if kind == "command" =>
            {
                commands
                    .load_proposal(proposal_id)
                    .map(|proposal| ReattachedApproval::Command {
                        task_id: task_id.clone(),
                        proposal,
                    })
                    .map_err(|error| {
                        format!("command proposal {proposal_id} is unreadable: {error:?}")
                    })
            }
            Some(PendingDecision::Approval(PendingApprovalRef { kind, proposal_id }))
                if kind == "patch" =>
            {
                patches
                    .load(proposal_id)
                    .map(|patch| ReattachedApproval::Patch {
                        task_id: task_id.clone(),
                        patch,
                    })
                    .map_err(|error| format!("patch {proposal_id} is unreadable: {error:?}"))
            }
            Some(PendingDecision::Approval(PendingApprovalRef { kind, proposal_id }))
                if kind == "plan" =>
            {
                reattach_plan(store, paused, session_id, &task_id, proposal_id)
            }
            Some(PendingDecision::Approval(PendingApprovalRef { kind, proposal_id }))
                if kind == "provider_fallback" =>
            {
                paused
                    .provider_fallback_target(proposal_id)
                    .map(|fallback_provider| ReattachedApproval::ProviderFallback {
                        task_id: task_id.clone(),
                        proposal_id: proposal_id.clone(),
                        fallback_provider,
                    })
                    .ok_or_else(|| {
                        format!("the paused turn behind fallback offer {proposal_id} is gone")
                    })
            }
            Some(PendingDecision::Approval(PendingApprovalRef { kind, proposal_id }))
                if kind == "round_budget" =>
            {
                paused
                    .round_budget_pause(proposal_id)
                    .map(|pause| ReattachedApproval::RoundBudget {
                        task_id: task_id.clone(),
                        proposal_id: proposal_id.clone(),
                        rounds_used: pause.rounds_used,
                        rounds_granted: pause.rounds_granted,
                    })
                    .ok_or_else(|| {
                        format!("the paused turn behind round-budget pause {proposal_id} is gone")
                    })
            }
            Some(PendingDecision::Approval(PendingApprovalRef { kind, proposal_id }))
                if kind == "checks" =>
            {
                paused
                    .check_batch_proposals(proposal_id)
                    .filter(|checks| !checks.is_empty())
                    .map(|checks| ReattachedApproval::CheckBatch {
                        task_id: task_id.clone(),
                        proposal_id: proposal_id.clone(),
                        checks,
                    })
                    .ok_or_else(|| {
                        format!("the paused turn behind check batch {proposal_id} is gone or empty")
                    })
            }
            Some(PendingDecision::Approval(PendingApprovalRef { kind, .. })) => {
                Err(format!("unknown pending approval kind {kind}"))
            }
            // The question and its answer live in the log, not in a paused-turn
            // file: a question link with no `question_asked` event has nothing
            // to show and nothing to answer, so it is failed like any other
            // unrecoverable link (§5.5).
            Some(PendingDecision::Question(question)) => {
                match store.question_state(session_id, question.question_id()) {
                    Ok(Some(state)) => Ok(ReattachedApproval::Question {
                        task_id: task_id.clone(),
                        question_id: state.question_id,
                        question: state.question,
                        options: state.options,
                        allow_free_text: state.allow_free_text,
                        answered: state.answer,
                    }),
                    Ok(None) => Err(format!(
                        "question {} has no recorded state",
                        question.question_id()
                    )),
                    Err(error) => Err(format!(
                        "question {} is unreadable: {error:?}",
                        question.question_id()
                    )),
                }
            }
        };

        match outcome {
            Ok(value) => reattached.push(value),
            Err(reason) => {
                fail_task(store, audit, session_id, &task_id, &reason)?;
                reattached.push(ReattachedApproval::Unavailable { task_id, reason });
            }
        }
    }
    Ok(reattached)
}

/// The plan a task paused to have reviewed, and the action that review is
/// holding back.
///
/// Two halves from two places, because that is where spec 21 put them: the plan
/// is replayed from the session log, and the deferred action lives in the
/// paused turn. Either one missing makes the review unrestorable — a plan with
/// no paused turn is a card whose buttons would have nothing to resume, which
/// is the dead-card case §5.5 rules out just as firmly as a guessed command.
fn reattach_plan(
    store: &SessionStore,
    paused: &PausedTurns,
    session_id: &str,
    task_id: &str,
    proposal_id: &str,
) -> std::result::Result<ReattachedApproval, String> {
    let plan = store
        .read_task_plan(session_id, task_id)
        .map_err(|error| format!("the plan for {proposal_id} is unreadable: {error:?}"))?
        .ok_or_else(|| format!("no plan was recorded for review {proposal_id}"))?;
    let deferred_action = paused
        .plan_review_deferred_action(proposal_id)
        .ok_or_else(|| format!("the paused turn behind plan review {proposal_id} is gone"))?;
    Ok(ReattachedApproval::Plan {
        task_id: task_id.to_string(),
        proposal_id: proposal_id.to_string(),
        plan,
        deferred_action,
    })
}

/// Moves a task to terminal `failed` with a stated reason, and records it.
fn fail_task(
    store: &SessionStore,
    audit: &AuditLog,
    session_id: &str,
    task_id: &str,
    reason: &str,
) -> Result<()> {
    // `update_task_status` needs a `Task`, and tasks are replayed from events
    // rather than stored as records, so the fields that do not affect the
    // status event are left empty. Only `id` and `session_id` are load-bearing.
    let task = crate::session::Task {
        id: task_id.to_string(),
        session_id: session_id.to_string(),
        status: TaskStatus::WaitingForApproval,
        user_prompt: String::new(),
        model_provider: String::new(),
        model_name: String::new(),
        created_at_ms: 0,
        completed_at_ms: None,
    };
    store.update_task_status(&task, TaskStatus::Failed, Some(reason))?;
    audit.record(
        "pending_approval_unavailable",
        &[
            ("actor", "system".to_string()),
            ("sessionId", session_id.to_string()),
            ("taskId", task_id.to_string()),
            ("reason", reason.to_string()),
        ],
    )?;
    Ok(())
}

/// The sentence naming what was in flight, for
/// `docs/specs/45_crash_recovery_prompt.md` §5.2.
///
/// It lives here rather than in the desktop shell for two reasons. The
/// frontend should never map action names to prose — the same reason
/// `chat::tool_action_label` is not in the UI — and the shell has no JS test
/// suite, so a sentence assembled in a webview could not be asserted against a
/// session log at all.
///
/// Requirement 1 forbids a generic "session interrupted", so every branch names
/// something specific: the action when a marker survived, the state the task
/// was left in when none did, and the raw action name when this version does
/// not recognise it.
pub fn headline(recovered: &RecoveredTask) -> String {
    let unknown = recovered.classification == TaskStatus::UnknownExternalOutcome;
    match &recovered.dangling {
        Some(action) => {
            let subject = action_subject(&action.action);
            if unknown {
                format!("{subject} was in progress and its outcome is unknown")
            } else {
                format!("{subject} was interrupted before it finished")
            }
        }
        // No marker reached the log. The action is unknowable, but the state is
        // not, so the sentence says what is actually known.
        None => format!(
            "This turn stopped while {}",
            state_phrase(&recovered.previous_status)
        ),
    }
}

/// The grammatical subject for an action marker: capitalised, because it opens
/// the sentence. An unrecognised name is returned verbatim rather than
/// generalised — a later version can add an action without silently turning its
/// recovery prompt vague.
fn action_subject(action: &str) -> &str {
    match action {
        "apply_patch" => "A patch application",
        "propose_patch" => "Preparing a patch",
        // Spec 21's two markers. Named here rather than left to the fallback:
        // the fallback exists so a *new* action still says something specific,
        // not as a resting place for actions this version knows about.
        "propose_plan" => "Drawing up a plan",
        "complete_step" => "Finishing a plan step",
        // Spec 50. A crash between starting the question and the pause leaves
        // this marker; the sentence names a question rather than the raw tool
        // name, and never calls it an approval.
        "ask_user" => "A question",
        "run_command" => "A command",
        "model_call" => "A model request",
        "mcp_call" => "An MCP tool call",
        "web_diagnostic" => "A browser diagnostic",
        "read_file" => "Reading a file",
        "search_codebase" => "A codebase search",
        "read_git_status" => "Reading git status",
        "read_git_diff" => "Reading the git diff",
        other => other,
    }
}

/// What the task was doing, from the status it was left in. The legacy
/// `running` string gets its own arm: it means only that something was in
/// flight, which is exactly what spec 17 §5.6 says cannot be narrowed further.
fn state_phrase(previous_status: &str) -> &str {
    match TaskStatus::parse(previous_status) {
        Some(TaskStatus::Created) => "starting up",
        Some(TaskStatus::PreparingContext) => "gathering context from your repository",
        Some(TaskStatus::WaitingForModel) => "waiting for the model",
        Some(TaskStatus::RunningTool) => "running a tool",
        Some(TaskStatus::ApplyingPatch) => "applying a patch",
        Some(TaskStatus::Validating) => "running validation",
        Some(TaskStatus::WaitingForApproval) => "waiting for your approval",
        Some(TaskStatus::UnknownExternalOutcome) => "in a state whose outcome was already unknown",
        // Terminal states are never classified, and `Interrupted` is the
        // classifier's own output rather than a state anything writes.
        Some(_) | None => "working, and nothing recorded on what",
    }
}

/// The sentence naming a task that stopped to ask the user a question (spec 50
/// §5.2). It is engine-owned for the same reason [`headline`] is: prose mapped
/// from a kind in a webview could not be asserted against a session log.
///
/// It cannot come from [`state_phrase`]: a waiting task is
/// `WaitingForApproval`, which classification skips (§5.4 rule 3), and the
/// status carries no question link. The reattached question carries it instead.
pub const QUESTION_WAITING_HEADLINE: &str = "A question was waiting for your answer";

/// Why `Resume` is not offered, or `None` when it is.
///
/// Requirement 2 asks for the absence to be explained rather than silent, and
/// the explanation belongs with the decision: an explanation composed in a
/// webview is the webview's guess at what the engine concluded.
pub fn resume_blocked_reason(recovered: &RecoveredTask) -> Option<String> {
    if resume_allowed(recovered) {
        return None;
    }
    let what = match &recovered.dangling {
        Some(action) => action_subject(&action.action).to_string(),
        // `UnknownExternalOutcome` without a marker cannot arise from the
        // classifier, which needs a side-effecting marker to reach it. It can
        // arise from a *stored* status of that name, so the sentence still has
        // to work.
        None => "A side-effecting action".to_string(),
    };
    Some(format!(
        "{what} was running when Damaian stopped, and there is no way to tell \
         whether it finished. Damaian will not run it again on its own."
    ))
}

/// Whether a human may choose to resume this task at all.
///
/// **This is a different question from [`RecoveredTask::auto_resume_permitted`]**,
/// and conflating them would be a mistake in either direction.
///
/// - `auto_resume_permitted` asks "may Damaian continue without asking?" It is
///   deliberately strict: it excludes a legacy `running` task and a
///   side-effecting status with no marker, because absence of evidence is not
///   evidence of safety.
/// - This asks "may the user, having been shown the evidence, choose to
///   continue?" Requirement 5 forbids repeating an unknown outcome
///   *automatically*; an informed human decision is not automatic.
///
/// So the only classification that can never be resumed is
/// [`TaskStatus::UnknownExternalOutcome`]: a side-effecting action was in
/// flight, and §4 rules out probing an external system to discover whether it
/// landed. Nothing a user is told can make that knowable.
pub fn resume_allowed(recovered: &RecoveredTask) -> bool {
    recovered.classification != TaskStatus::UnknownExternalOutcome
}

/// Authorizes continuing an interrupted task, or refuses.
///
/// Refusing here is requirement 5's enforcement point. Spec 45 will present
/// this decision from a webview, and a guarantee that lives only in a webview
/// is not a guarantee — so the refusal is in the engine, where a caller cannot
/// widen it.
///
/// This authorizes and records; actually re-running the turn is the
/// orchestrator's job, which is why nothing here touches a model adapter.
pub fn resume(store: &SessionStore, audit: &AuditLog, recovered: &RecoveredTask) -> Result<()> {
    if !resume_allowed(recovered) {
        let reason = match &recovered.dangling {
            Some(action) => format!("{} was in flight and its outcome is unknown", action.action),
            None => "a side-effecting action was in flight and its outcome is unknown".to_string(),
        };
        audit_decision(audit, recovered, "resume", "refused", &reason)?;
        return Err(ClientError::PolicyBlocked(format!(
            "Cannot resume {}: {reason}. Inspect it and decide, or mark it failed.",
            recovered.task_id
        )));
    }
    set_status(store, recovered, TaskStatus::PreparingContext, None)?;
    audit_decision(audit, recovered, "resume", "allowed", "")?;
    Ok(())
}

/// Ends the task as `failed`, noting that its outcome was unknown.
pub fn mark_failed(
    store: &SessionStore,
    audit: &AuditLog,
    recovered: &RecoveredTask,
) -> Result<()> {
    let note = match &recovered.dangling {
        Some(action) => format!(
            "Marked failed after a crash: {} was in flight and its outcome is unknown",
            action.action
        ),
        None => "Marked failed after a crash: the task was interrupted".to_string(),
    };
    set_status(store, recovered, TaskStatus::Failed, Some(&note))?;
    audit_decision(audit, recovered, "mark_failed", "applied", &note)?;
    Ok(())
}

/// Closes the task as `cancelled`. The turn is not retried.
pub fn abandon(store: &SessionStore, audit: &AuditLog, recovered: &RecoveredTask) -> Result<()> {
    set_status(store, recovered, TaskStatus::Cancelled, None)?;
    audit_decision(audit, recovered, "abandon", "applied", "")?;
    Ok(())
}

fn set_status(
    store: &SessionStore,
    recovered: &RecoveredTask,
    status: TaskStatus,
    error: Option<&str>,
) -> Result<()> {
    // Tasks are replayed from events rather than stored as records, so only
    // `id` and `session_id` are load-bearing here.
    let task = crate::session::Task {
        id: recovered.task_id.clone(),
        session_id: recovered.session_id.clone(),
        status: recovered.classification.clone(),
        user_prompt: String::new(),
        model_provider: String::new(),
        model_name: String::new(),
        created_at_ms: 0,
        completed_at_ms: None,
    };
    store.update_task_status(&task, status, error)?;
    Ok(())
}

/// Requirement 10: every recovery decision and its outcome, with the evidence
/// it was made on.
fn audit_decision(
    audit: &AuditLog,
    recovered: &RecoveredTask,
    decision: &str,
    outcome: &str,
    reason: &str,
) -> Result<()> {
    audit.record(
        "task_recovery_decision",
        &[
            ("actor", "user".to_string()),
            ("sessionId", recovered.session_id.clone()),
            ("taskId", recovered.task_id.clone()),
            ("decision", decision.to_string()),
            ("outcome", outcome.to_string()),
            (
                "classification",
                recovered.classification.as_str().to_string(),
            ),
            (
                "danglingAction",
                recovered
                    .dangling
                    .as_ref()
                    .map(|action| action.action.clone())
                    .unwrap_or_default(),
            ),
            ("reason", reason.to_string()),
        ],
    )?;
    Ok(())
}
