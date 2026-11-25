//! The launch sweep and the recovery decision endpoint, per
//! `docs/specs/45_crash_recovery_prompt.md` §5.3.
//!
//! Nothing here decides what may be resumed. That is spec 17's classifier, and
//! the one rule this module must not break is that a decision reaching the
//! engine is built from a *re-classification* rather than from anything the
//! webview posted — see [`decide`].

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use workspace_engine::{
    AgentFetchProposal, CommandProposal, PausedTurns, PendingVerification, ProposedPatch,
    QUESTION_WAITING_HEADLINE, ReattachedApproval, RecoveredTask, TaskStatus, WorkspaceEngine,
    allow_always_eligible, classify_session, command_approval_prompt, headline,
    pending_verifications, read_pending_approval, reattach_pending_approvals, resume_allowed,
    resume_blocked_reason,
};

use crate::{escape_json, fetch_proposal_json, patch_payload_json, plan_json};

/// What an interrupted commit can be inspected against: the expected old
/// `HEAD`, the commit object that may have been created, and where `HEAD` is
/// now. `landed` is `Some(true)` only when `HEAD` is the prepared commit,
/// `Some(false)` only when it is still the expected old one, and `None` when it
/// moved somewhere else.
struct CommitRecovery {
    expected_old: Option<String>,
    prepared_commit: Option<String>,
    head_now: Option<String>,
    landed: Option<bool>,
    selected_paths: Vec<String>,
}

/// One recovered task, with everything the card needs already resolved.
struct Recovered {
    session_id: String,
    task_id: String,
    headline: String,
    classification: String,
    previous_status: String,
    dangling_action: Option<String>,
    dangling_reference: Option<String>,
    dangling_seq: Option<u64>,
    resume_allowed: bool,
    resume_blocked_reason: Option<String>,
    /// Whether the sweep authorized this task automatically (§5.5). The card
    /// reports it; it does not mean the turn has run.
    auto_resumed: bool,
    prompt: String,
    /// Files the dangling action may have touched. Empty is not "none touched"
    /// for a command — the card says which of the two it is.
    files: Vec<String>,
    checkpoint_id: Option<String>,
    /// The interrupted commit's durable record, when the dangling action is
    /// `commit`. Rendered by `Inspect`; the comparison is information, not an
    /// automatic resolution (spec 35 §5.7).
    commit: Option<CommitRecovery>,
    /// What this task spent, as `task_usage_json` fields spliced into the
    /// entry — the same shape a turn's own response sends, so the frontend
    /// reads both with one piece of code. Empty when nothing was recorded,
    /// which is not the same as zero (spec 19).
    usage_fields: String,
    /// Whether that total includes a model call lost to the crash. Read back
    /// from what was recorded rather than inferred from the dangling marker,
    /// which can name a model call whose cost was never estimable.
    includes_lost_call: bool,
}

/// A reattached approval, as a payload the existing approval cards can render.
struct Approval {
    session_id: String,
    task_id: String,
    kind: &'static str,
    payload: String,
    unavailable_reason: Option<String>,
}

struct Sweep {
    recovered: Vec<Recovered>,
    approvals: Vec<Approval>,
    verifications: Vec<(String, PendingVerification)>,
}

/// Everything the sweep needs from one session log, read once.
///
/// Each of these is a full replay of the log, and the sweep asks four questions
/// per recovered task. Asked per task they would make the launch cost quadratic
/// in tasks-per-session — the same shape spec 17 measured and removed from
/// `append_session_event`, which is reason enough not to reintroduce it one
/// caller later.
struct SessionFacts {
    statuses: HashMap<String, String>,
    tasks: HashMap<String, String>,
    usage: HashMap<String, workspace_engine::TaskUsage>,
    superseded: std::collections::HashSet<String>,
    lost_to_crash: std::collections::HashSet<String>,
}

impl SessionFacts {
    fn read(engine: &WorkspaceEngine, session_id: &str) -> Result<Self, String> {
        let store = &engine.session_store;
        Ok(Self {
            statuses: store
                .read_task_statuses(session_id)
                .map_err(|error| error.to_string())?,
            tasks: store
                .read_tasks(session_id)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|task| (task.id, task.user_prompt))
                .collect(),
            usage: store
                .read_task_usage(session_id)
                .map_err(|error| error.to_string())?,
            superseded: store
                .superseded_task_ids(session_id)
                .map_err(|error| error.to_string())?,
            lost_to_crash: store
                .lost_to_crash_task_ids(session_id)
                .map_err(|error| error.to_string())?,
        })
    }
}

/// The sweep runs once per process. Once is correct rather than merely cheap: a
/// crash ends the process, so no new crash can appear while one is running, and
/// classification appends a `task_recovered` audit event per task — re-running
/// it on every session switch would fill the audit log with re-derivations of a
/// single fact.
static SWEEP: OnceLock<Mutex<Sweep>> = OnceLock::new();

/// The sweep result as JSON, with tasks the user has since dealt with filtered
/// out.
pub fn sweep_json() -> Result<String, String> {
    let engine = crate::default_engine()?;
    let sweep = sweep_once(&engine)?;
    let sweep = sweep.lock().map_err(|error| error.to_string())?;
    sweep_payload(&engine, &sweep)
}

/// Split from [`sweep_json`] so it can be tested against an engine of the
/// test's own making: the memoized sweep is process-wide and the default engine
/// reads the user's real data directory, neither of which a test may touch.
fn sweep_payload(engine: &WorkspaceEngine, sweep: &Sweep) -> Result<String, String> {
    // A task that has become terminal since the sweep — decided in an earlier
    // run of the UI, or completed by a resumed turn — is no longer a question.
    // Re-read the statuses rather than trusting the snapshot, so reloading the
    // webview does not bring a settled card back.
    let mut facts: HashMap<String, SessionFacts> = HashMap::new();
    let mut open = Vec::new();
    for task in &sweep.recovered {
        if !facts.contains_key(&task.session_id) {
            facts.insert(
                task.session_id.clone(),
                SessionFacts::read(engine, &task.session_id)?,
            );
        }
        let session = &facts[&task.session_id];
        let settled = session
            .statuses
            .get(&task.task_id)
            .and_then(|status| TaskStatus::parse(status))
            .is_some_and(|status| status.is_terminal())
            || session.superseded.contains(&task.task_id);
        if !settled {
            open.push(recovered_json(task));
        }
    }

    // A reattached approval is a one-time sweep result too, but unlike a
    // recovered task it was never rechecked here: once captured, it was
    // offered on every poll for the rest of the process's life, even after
    // the task it belongs to finished, was cancelled, or was superseded by a
    // later turn (observation 103's follow-up, 2026-11-14) — the exact same
    // staleness the `recovered` loop above already guards against.
    //
    // "unavailable" is excluded: it carries no Approve/Reject of its own (the
    // card is pure explanation — "a command from before the crash could not
    // be restored"), and resume()'s own classification above can finalize
    // such a task as terminal in this same sweep, before the card is ever
    // shown once. Checking it the same way as a decidable approval would
    // drop the one card whose entire job is to explain that outcome.
    let mut approvals_open = Vec::new();
    for approval in &sweep.approvals {
        if approval.kind == "unavailable" {
            approvals_open.push(approval_json(approval));
            continue;
        }
        if !facts.contains_key(&approval.session_id) {
            facts.insert(
                approval.session_id.clone(),
                SessionFacts::read(engine, &approval.session_id)?,
            );
        }
        let session = &facts[&approval.session_id];
        let settled = session
            .statuses
            .get(&approval.task_id)
            .and_then(|status| TaskStatus::parse(status))
            .is_some_and(|status| status.is_terminal())
            || session.superseded.contains(&approval.task_id);
        if !settled {
            approvals_open.push(approval_json(approval));
        }
    }

    Ok(format!(
        "{{\"tasks\":[{}],\"approvals\":[{}],\"verifications\":[{}]}}",
        open.join(","),
        approvals_open.join(","),
        sweep
            .verifications
            .iter()
            .map(|(session_id, due)| format!(
                "{{\"sessionId\":\"{}\",\"taskId\":\"{}\",\"patchId\":\"{}\"}}",
                escape_json(session_id),
                escape_json(&due.task_id),
                escape_json(&due.patch_id)
            ))
            .collect::<Vec<_>>()
            .join(",")
    ))
}

/// Applies one recovery decision.
///
/// The posted `session_id` and `task_id` are an **address, not evidence**: the
/// task is re-classified here and the engine's own [`RecoveredTask`] is what
/// reaches `recovery::resume`. Rebuilding one from posted fields would let a
/// webview claim `interrupted` for a task whose outcome is unknown and walk
/// straight through requirement 5's enforcement point — the arrangement spec 17
/// §5.4 refused when it put the decision in the engine.
///
/// A `resume` returns the prompt to re-send rather than running the turn here.
/// The engine authorizes first, so a refused resume yields no prompt and the
/// turn cannot start; re-sending it is then an ordinary turn on the existing
/// streaming path, which is better than a second copy of the turn machinery.
pub fn decide(form: &HashMap<String, String>) -> Result<String, String> {
    let engine = crate::default_engine()?;
    apply_decision(&engine, form)
}

/// Split from [`decide`] for the same reason as [`sweep_payload`]: a test needs
/// an engine pointed at its own data directory.
fn apply_decision(
    engine: &WorkspaceEngine,
    form: &HashMap<String, String>,
) -> Result<String, String> {
    let session_id = crate::required_form(form, "session_id")?;
    let task_id = crate::required_form(form, "task_id")?;
    let decision = crate::required_form(form, "decision")?;

    let recovered = classify_session(&engine.session_store, &engine.audit_log, &session_id)
        .map_err(|error| error.to_string())?;
    let task = recovered
        .into_iter()
        .find(|candidate| candidate.task_id == task_id)
        .ok_or_else(|| format!("{task_id} is not a recovered task in {session_id}"))?;
    // A card from a stale webview must not resume the same turn twice.
    if engine
        .session_store
        .superseded_task_ids(&session_id)
        .map_err(|error| error.to_string())?
        .contains(&task_id)
    {
        return Err(format!(
            "{task_id} was already resumed; its work was handed to a later turn"
        ));
    }

    let prompt = match decision.as_str() {
        "resume" => {
            workspace_engine::resume(&engine.session_store, &engine.audit_log, &task)
                .map_err(|error| error.to_string())?;
            let prompt = task_prompt(engine, &session_id, &task_id)?;
            // The resumed work runs as a *new* turn with a task of its own, so
            // without this the original stays non-terminal and every later
            // launch offers the same card again. Recorded before the prompt is
            // handed back, so a client that dies on the way to sending it still
            // leaves the question settled.
            engine
                .session_store
                .mark_task_superseded(
                    &session_id,
                    &task_id,
                    "Resumed after a crash: its request was re-sent as a new turn",
                )
                .map_err(|error| error.to_string())?;
            prompt
        }
        "mark_failed" => {
            workspace_engine::mark_failed(&engine.session_store, &engine.audit_log, &task)
                .map_err(|error| error.to_string())?;
            String::new()
        }
        "abandon" => {
            workspace_engine::abandon(&engine.session_store, &engine.audit_log, &task)
                .map_err(|error| error.to_string())?;
            String::new()
        }
        other => return Err(format!("Unknown recovery decision: {other}")),
    };

    forget(&session_id, &task_id)?;

    Ok(format!(
        "{{\"sessionId\":\"{}\",\"taskId\":\"{}\",\"decision\":\"{}\",\"prompt\":\"{}\"}}",
        escape_json(&session_id),
        escape_json(&task_id),
        escape_json(&decision),
        escape_json(&prompt)
    ))
}

/// Closes out a task whose reattached approval the user has just answered.
///
/// Without this the task stays `waiting_for_approval` in the log, so the next
/// launch reattaches the same proposal and offers to run a command that has
/// already run. That is the one way this surface could cause a side effect
/// twice, which is the failure the whole feature exists to prevent.
///
/// The turn is `cancelled` rather than `complete`: the command ran, but the
/// conversation that asked for it died with the process and produced no answer.
/// This writes a status and records it — it authorizes nothing, so it does not
/// go through the engine's recovery operations, none of which fit a task the
/// classifier deliberately skips.
pub fn resolve_reattached_approval(form: &HashMap<String, String>) -> Result<String, String> {
    let engine = crate::default_engine()?;
    resolve_reattached_approval_with(&engine, form)
}

fn resolve_reattached_approval_with(
    engine: &WorkspaceEngine,
    form: &HashMap<String, String>,
) -> Result<String, String> {
    let session_id = crate::required_form(form, "session_id")?;
    let task_id = crate::required_form(form, "task_id")?;
    let outcome = crate::required_form(form, "outcome")?;
    let reason = format!(
        "The proposal it was waiting on was {outcome} after a crash; the turn did not continue"
    );

    let task = workspace_engine::Task {
        id: task_id.clone(),
        session_id: session_id.clone(),
        status: TaskStatus::WaitingForApproval,
        user_prompt: String::new(),
        model_provider: String::new(),
        model_name: String::new(),
        created_at_ms: 0,
        completed_at_ms: None,
    };
    engine
        .session_store
        .update_task_status(&task, TaskStatus::Cancelled, Some(&reason))
        .map_err(|error| error.to_string())?;
    engine
        .audit_log
        .record(
            "reattached_approval_resolved",
            &[
                ("actor", "user".to_string()),
                ("sessionId", session_id.clone()),
                ("taskId", task_id.clone()),
                ("outcome", outcome.clone()),
            ],
        )
        .map_err(|error| error.to_string())?;
    forget(&session_id, &task_id)?;

    Ok(format!(
        "{{\"sessionId\":\"{}\",\"taskId\":\"{}\",\"outcome\":\"{}\"}}",
        escape_json(&session_id),
        escape_json(&task_id),
        escape_json(&outcome)
    ))
}

/// Drops a decided task from the snapshot, so it does not come back when the
/// webview reloads. A resumed task is still non-terminal, so the status filter
/// in [`sweep_json`] cannot do this on its own.
fn forget(session_id: &str, task_id: &str) -> Result<(), String> {
    let Some(sweep) = SWEEP.get() else {
        return Ok(());
    };
    let mut sweep = sweep.lock().map_err(|error| error.to_string())?;
    sweep
        .recovered
        .retain(|task| task.session_id != session_id || task.task_id != task_id);
    sweep
        .approvals
        .retain(|approval| approval.session_id != session_id || approval.task_id != task_id);
    Ok(())
}

/// The card payload of the decision one task is waiting on, for redrawing the
/// card when a session is reloaded (spec 107 requirement 1, note 2026-11-20).
///
/// `{"card":null}` when there is nothing to draw. Unlike the sweep this runs on
/// every reload, so it only reads: an unrestorable proposal draws no card and is
/// failed by the next launch's sweep, not here. The payload is the same one the
/// reattached card gets, which for a command, patch, plan, fetch or context
/// review is the live turn's shape, so the shell draws the live card and
/// answers through the live resume route.
pub fn pending_card_json(session_id: &str, task_id: &str) -> Result<String, String> {
    let engine = crate::default_engine()?;
    pending_card_payload(&engine, session_id, task_id)
}

fn pending_card_payload(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
) -> Result<String, String> {
    let approval = read_pending_approval(
        &engine.session_store,
        &engine.command_store,
        &engine.patch_store,
        &PausedTurns::new(&engine.config.data_dir),
        session_id,
        task_id,
    )
    .map_err(|error| error.to_string())?;
    Ok(format!(
        "{{\"card\":{}}}",
        approval.map_or_else(
            || "null".to_string(),
            |approval| approval_json(&describe_approval(engine, session_id, approval))
        )
    ))
}

fn sweep_once(engine: &WorkspaceEngine) -> Result<&'static Mutex<Sweep>, String> {
    if let Some(sweep) = SWEEP.get() {
        return Ok(sweep);
    }
    let sweep = run_sweep(engine)?;
    // A second caller that lost the race keeps its own result rather than
    // overwriting the stored one; both ran the same classification.
    Ok(SWEEP.get_or_init(|| Mutex::new(sweep)))
}

fn run_sweep(engine: &WorkspaceEngine) -> Result<Sweep, String> {
    // Spec 110: the shell runs turns while it serves other requests, so the
    // first sweep can come while a session works. Its tasks are running in
    // this process, not left by a crash, so it is skipped before anything is
    // classified or written (the same rule spec 57 C12 gives a live worker).
    let mut classified = Vec::new();
    for session in engine
        .session_store
        .list_sessions(None)
        .map_err(|error| error.to_string())?
    {
        if crate::lanes::working_label(&session.id).is_some() {
            continue;
        }
        classified.extend(
            classify_session(&engine.session_store, &engine.audit_log, &session.id)
                .map_err(|error| error.to_string())?,
        );
    }

    let sessions = engine
        .session_store
        .list_sessions(None)
        .map_err(|error| error.to_string())?;
    let repository_ids: HashMap<String, String> = sessions
        .iter()
        .map(|session| (session.id.clone(), session.repository_id.clone()))
        .collect();

    let recovered = recovered_tasks(engine, &classified, &repository_ids)?;

    let mut approvals = Vec::new();
    for session in &sessions {
        let reattached = reattach_pending_approvals(
            &engine.session_store,
            &engine.audit_log,
            &engine.command_store,
            &engine.patch_store,
            &PausedTurns::new(&engine.config.data_dir),
            &session.id,
        )
        .map_err(|error| error.to_string())?;
        for approval in reattached {
            approvals.push(describe_approval(engine, &session.id, approval));
        }
    }

    let mut verifications = Vec::new();
    for session in &sessions {
        for due in pending_verifications(&engine.session_store, &session.id)
            .map_err(|error| error.to_string())?
        {
            verifications.push((session.id.clone(), due));
        }
    }

    Ok(Sweep {
        recovered,
        approvals,
        verifications,
    })
}

/// The sweep's middle step for classified tasks: each one's facts read once
/// per session, a task already handed to a later turn left out, and an
/// automatic resume (§5.5) applied before it is described.
fn recovered_tasks(
    engine: &WorkspaceEngine,
    classified: &[RecoveredTask],
    repository_ids: &HashMap<String, String>,
) -> Result<Vec<Recovered>, String> {
    let mut facts: HashMap<String, SessionFacts> = HashMap::new();
    let mut recovered = Vec::new();
    for task in classified {
        if !facts.contains_key(&task.session_id) {
            facts.insert(
                task.session_id.clone(),
                SessionFacts::read(engine, &task.session_id)?,
            );
        }
        // Already handed to a later turn, so it is not a question any more —
        // and must not be auto-resumed a second time.
        if facts[&task.session_id].superseded.contains(&task.task_id) {
            continue;
        }
        // §5.5: authorized automatically, so the status change is the engine's
        // and is audited — but the turn does not re-run until the user asks.
        // Otherwise opening the app would fire a billed model call for every
        // `waiting_for_model` task before the user had read anything.
        let auto_resumed = if task.auto_resume_permitted {
            workspace_engine::resume(&engine.session_store, &engine.audit_log, task)
                .map_err(|error| error.to_string())?;
            true
        } else {
            false
        };
        recovered.push(describe(
            engine,
            task,
            &facts[&task.session_id],
            repository_ids,
            auto_resumed,
        )?);
    }
    Ok(recovered)
}

/// Spec 110 D6: a session host that died while the shell runs left its
/// session as a crash leaves it. The session is classified now, exactly as
/// the launch sweep classifies it, and its tasks join the sweep's snapshot,
/// so `/api/recovery` offers them without a relaunch. Before the first sweep
/// there is nothing to join: that sweep finds them itself.
pub(crate) fn recover_session(session_id: &str) -> Result<(), String> {
    let Some(sweep) = SWEEP.get() else {
        return Ok(());
    };
    let engine = crate::default_engine()?;
    let classified = classify_session(&engine.session_store, &engine.audit_log, session_id)
        .map_err(|error| error.to_string())?;
    if classified.is_empty() {
        return Ok(());
    }
    let repository_ids = engine
        .session_store
        .list_sessions(None)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|session| (session.id, session.repository_id))
        .collect();
    let recovered = recovered_tasks(&engine, &classified, &repository_ids)?;
    let mut sweep = sweep.lock().map_err(|error| error.to_string())?;
    for task in recovered {
        if !sweep
            .recovered
            .iter()
            .any(|known| known.session_id == task.session_id && known.task_id == task.task_id)
        {
            sweep.recovered.push(task);
        }
    }
    Ok(())
}

fn describe(
    engine: &WorkspaceEngine,
    task: &RecoveredTask,
    facts: &SessionFacts,
    repository_ids: &HashMap<String, String>,
    auto_resumed: bool,
) -> Result<Recovered, String> {
    // Spec 19 §5.5 accounts for the model call a crash interrupted, during this
    // very sweep. Without surfacing it the spend is recorded and invisible,
    // which leaves the crash looking free on the one screen that exists to
    // explain it.
    let usage_fields = match facts.usage.get(&task.task_id) {
        Some(total) => {
            let estimated_cost = engine.config.estimated_cost(&workspace_engine::TokenUsage {
                input_tokens: total.input_tokens,
                output_tokens: total.output_tokens,
                cached_input_tokens: total.cached_input_tokens,
                source: total.source,
            });
            let body = crate::task_usage_json(Some(total), estimated_cost);
            // Spliced rather than nested, matching `task_states_json`.
            format!(",{}", body.trim_start_matches('{').trim_end_matches('}'))
        }
        None => String::new(),
    };
    let includes_lost_call = facts.lost_to_crash.contains(&task.task_id);
    let dangling = task.dangling.as_ref();
    let files = dangling
        .filter(|action| action.action == "apply_patch" || action.action == "propose_patch")
        .and_then(|action| engine.patch_store.load(&action.reference).ok())
        .map(|patch: ProposedPatch| {
            patch
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // The checkpoint spec 16 wrote for this task, when it wrote one. Looked up
    // by task id rather than reconstructed, so `Inspect` links to the real
    // thing or says there is none.
    let checkpoint_id = repository_ids
        .get(&task.session_id)
        .and_then(|repository_id| engine.checkpoint_store.list_checkpoints(repository_id).ok())
        .and_then(|manifests| {
            manifests
                .into_iter()
                .find(|manifest| manifest.task_id.as_deref() == Some(task.task_id.as_str()))
                .map(|manifest| manifest.checkpoint_id)
        });

    // A commit's record is its own evidence: the marker's reference names it.
    // The comparison is offered for inspection and changes nothing, so a crash
    // is never resolved automatically.
    let commit = dangling
        .filter(|action| action.action == "commit")
        .and_then(|action| engine.commit.load_record(&action.reference).ok())
        .map(|record| {
            let head_now = engine
                .git
                .head_commit(&record.repository_root)
                .ok()
                .flatten();
            let landed = match (&record.prepared_commit, &head_now) {
                (Some(prepared), Some(head)) if prepared == head => Some(true),
                (_, Some(head)) if record.expected_old.as_deref() == Some(head.as_str()) => {
                    Some(false)
                }
                _ => None,
            };
            let selected_paths = if record.selected_paths.is_empty() {
                record
                    .candidates
                    .iter()
                    .filter(|candidate| candidate.selected_by_default)
                    .map(|candidate| candidate.path.clone())
                    .collect()
            } else {
                record.selected_paths
            };
            CommitRecovery {
                expected_old: record.expected_old,
                prepared_commit: record.prepared_commit,
                head_now,
                landed,
                selected_paths,
            }
        });

    let prompt = facts.tasks.get(&task.task_id).cloned().unwrap_or_default();
    // Two separate ways for `Resume` to be unavailable, each with its own
    // reason. §5.6 leaves the card with no primary action either way, so the
    // reason is the only thing that tells the user which case they are in.
    let (offer_resume, blocked_reason) = if !resume_allowed(task) {
        (false, resume_blocked_reason(task))
    } else if prompt.is_empty() {
        (
            false,
            Some(
                "This task was recorded without the prompt that started it, so there is \
                 nothing to send again."
                    .to_string(),
            ),
        )
    } else {
        (true, None)
    };

    Ok(Recovered {
        session_id: task.session_id.clone(),
        task_id: task.task_id.clone(),
        headline: headline(task),
        classification: task.classification.as_str().to_string(),
        previous_status: task.previous_status.clone(),
        dangling_action: dangling.map(|action| action.action.clone()),
        dangling_reference: dangling.map(|action| action.reference.clone()),
        dangling_seq: dangling.map(|action| action.seq),
        resume_allowed: offer_resume,
        resume_blocked_reason: blocked_reason,
        auto_resumed,
        prompt,
        files: if files.is_empty() {
            commit
                .as_ref()
                .map(|commit| commit.selected_paths.clone())
                .unwrap_or_default()
        } else {
            files
        },
        checkpoint_id,
        commit,
        usage_fields,
        includes_lost_call,
    })
}

fn describe_approval(
    engine: &WorkspaceEngine,
    session_id: &str,
    approval: ReattachedApproval,
) -> Approval {
    match approval {
        ReattachedApproval::Command { task_id, proposal } => Approval {
            session_id: session_id.to_string(),
            task_id,
            kind: "command",
            payload: command_proposal_json(engine, &proposal),
            unavailable_reason: None,
        },
        ReattachedApproval::Patch { task_id, patch } => Approval {
            session_id: session_id.to_string(),
            task_id,
            kind: "patch",
            // The same payload the live turn and a worker produce, including
            // the engine-owned repair-scope review (spec 102 Task 5).
            payload: patch_payload_json(
                &patch.id,
                &patch.summary,
                &patch.files,
                patch.repair_scope.as_ref(),
            ),
            unavailable_reason: None,
        },
        // Shaped exactly like `plan_proposal_json`, so the reattached review is
        // the same `createPlanReview` card the live turn raises. It also resumes
        // through the same endpoint: unlike a command, whose turn died with the
        // process, a plan review's paused turn is on disk and picks up where it
        // stopped.
        ReattachedApproval::Plan {
            task_id,
            proposal_id,
            plan,
            deferred_action,
            budget_review,
        } => Approval {
            session_id: session_id.to_string(),
            task_id,
            kind: "plan",
            payload: format!(
                "{{\"proposalId\":\"{}\",\"deferredAction\":\"{}\",\"plan\":{}{}}}",
                escape_json(&proposal_id),
                escape_json(&deferred_action),
                // The reattached review is a paused turn, still waiting.
                plan_json(&plan, true),
                // Absent, not null, when the review offered no budget: an
                // inline or historical review keeps its pre-budget shape
                // (spec 103 Task 2).
                budget_review
                    .and_then(|review| serde_json::to_string(&review).ok())
                    .map(|review| format!(",\"budgetReview\":{review}"))
                    .unwrap_or_default()
            ),
            unavailable_reason: None,
        },
        // Spec 56. Like a plan review, the paused turn is on disk, so the card
        // answers through the same `/api/resume-fallback-stream` the live offer
        // uses and approving continues the turn. Only the fallback is named:
        // that is all `PausedTurns` exposes, and the offer text was never a
        // transcript message, so the card says less than the live one does.
        ReattachedApproval::ProviderFallback {
            task_id,
            proposal_id,
            fallback_provider,
        } => Approval {
            session_id: session_id.to_string(),
            task_id,
            kind: "provider_fallback",
            payload: format!(
                "{{\"proposalId\":\"{}\",\"fallbackProvider\":\"{}\"}}",
                escape_json(&proposal_id),
                escape_json(&fallback_provider)
            ),
            unavailable_reason: None,
        },
        // Spec 61. Like a plan review, the paused turn is on disk, so the card
        // answers through the same resume the live pause uses and continuing
        // resumes the turn. Only the rounds are named: that is all
        // `PausedTurns` exposes for a round-budget pause.
        ReattachedApproval::RoundBudget {
            task_id,
            proposal_id,
            rounds_used,
            rounds_granted,
        } => Approval {
            session_id: session_id.to_string(),
            task_id,
            kind: "round_budget",
            payload: format!(
                "{{\"proposalId\":\"{}\",\"roundsUsed\":{},\"roundsGranted\":{}}}",
                escape_json(&proposal_id),
                rounds_used,
                rounds_granted,
            ),
            unavailable_reason: None,
        },
        // Spec 50. A question is not an approval: this payload is for display
        // only. It carries no proposal id, command or patch, so nothing can
        // route it to a command or patch endpoint; answering it uses the
        // question route. The text was redacted before it was stored in the
        // session log; `answered` and `answer` are what a reattached card shows
        // instead of asking again.
        // Spec 51. Like a plan review, the paused turn is on disk, so the same
        // `fetchProposal` payload the live dialog uses is rebuilt and answering
        // it goes through `/api/fetch-decision`, which performs the request the
        // dialog showed.
        ReattachedApproval::Fetch {
            task_id,
            proposal_id,
            call,
        } => Approval {
            session_id: session_id.to_string(),
            task_id,
            kind: "fetch",
            payload: fetch_proposal_json(Some(&AgentFetchProposal {
                id: proposal_id,
                host: call.host,
                url: call.url,
                approval_key: call.approval_key,
                long_components: call.long_components,
                query: call.query,
                fragment: call.fragment,
            })),
            unavailable_reason: None,
        },
        ReattachedApproval::CheckBatch {
            task_id,
            proposal_id,
            checks,
        } => {
            let checks_json = checks
                .iter()
                .filter_map(|proposal_id| engine.command_store.load_proposal(proposal_id).ok())
                .map(|proposal| command_proposal_json(engine, &proposal))
                .collect::<Vec<_>>()
                .join(",");
            Approval {
                session_id: session_id.to_string(),
                task_id,
                kind: "checks",
                payload: format!(
                    "{{\"proposalId\":\"{}\",\"checks\":[{}]}}",
                    escape_json(&proposal_id),
                    checks_json
                ),
                unavailable_reason: None,
            }
        }
        ReattachedApproval::ContextReview {
            task_id,
            proposal_id,
            manifest,
        } => Approval {
            session_id: session_id.to_string(),
            task_id,
            kind: "context_review",
            payload: format!(
                "{{\"proposalId\":\"{}\",\"manifest\":{}}}",
                escape_json(&proposal_id),
                serde_json::to_string(&manifest).unwrap_or_else(|_| "null".to_string())
            ),
            unavailable_reason: None,
        },
        ReattachedApproval::Question {
            task_id,
            question_id,
            question,
            options,
            allow_free_text,
            answered,
        } => Approval {
            session_id: session_id.to_string(),
            task_id,
            kind: "question",
            payload: format!(
                "{{\"headline\":\"{}\",\"questionId\":\"{}\",\"question\":\"{}\",\"options\":[{}],\"allowFreeText\":{},\"answered\":{},\"answer\":{}}}",
                QUESTION_WAITING_HEADLINE,
                escape_json(&question_id),
                escape_json(&question),
                options
                    .iter()
                    .map(|option| format!("\"{}\"", escape_json(option)))
                    .collect::<Vec<_>>()
                    .join(","),
                allow_free_text,
                answered.is_some(),
                match &answered {
                    None => "null".to_string(),
                    Some((answer, selected)) => format!(
                        "{{\"answer\":\"{}\",\"selectedOption\":{}}}",
                        escape_json(answer),
                        selected
                            .map(|index| index.to_string())
                            .unwrap_or_else(|| "null".to_string())
                    ),
                }
            ),
            unavailable_reason: None,
        },
        ReattachedApproval::Unavailable { task_id, reason } => Approval {
            session_id: session_id.to_string(),
            task_id,
            kind: "unavailable",
            payload: "null".to_string(),
            unavailable_reason: Some(reason),
        },
    }
}

/// Shaped exactly like `/api/propose-command`'s response, so the reattached
/// card is the same component rather than a second one.
fn command_proposal_json(engine: &WorkspaceEngine, proposal: &CommandProposal) -> String {
    format!(
        "{{\"proposalId\":\"{}\",\"command\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{},\"cause\":\"{}\",\"reason\":\"{}\",\"allowAlways\":{},\"allowBrowserDiagnosticsForSession\":false}}",
        escape_json(&proposal.id),
        escape_json(&proposal.command),
        escape_json(&command_approval_prompt(proposal)),
        proposal.risk.as_str(),
        proposal.requires_approval,
        proposal.blocked,
        escape_json(proposal.reasons.first().map(String::as_str).unwrap_or("")),
        escape_json(&proposal.reason),
        allow_always_eligible(&engine.config, &proposal.command, proposal.blocked)
    )
}

/// The prompt the user typed, for a resume to re-send and for `Inspect` to
/// show. Empty when the task predates the field or the log no longer has it —
/// the card then offers no resume, since there would be nothing to send.
fn task_prompt(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
) -> Result<String, String> {
    Ok(engine
        .session_store
        .read_tasks(session_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|task| task.id == task_id)
        .map(|task| task.user_prompt)
        .unwrap_or_default())
}

fn commit_json(commit: Option<&CommitRecovery>) -> String {
    match commit {
        None => "null".to_string(),
        Some(commit) => format!(
            "{{\"expectedOld\":{},\"preparedCommit\":{},\"headNow\":{},\"landed\":{}}}",
            json_optional(commit.expected_old.as_deref()),
            json_optional(commit.prepared_commit.as_deref()),
            json_optional(commit.head_now.as_deref()),
            match commit.landed {
                Some(landed) => landed.to_string(),
                None => "null".to_string(),
            },
        ),
    }
}

fn recovered_json(task: &Recovered) -> String {
    format!(
        "{{\"sessionId\":\"{}\",\"taskId\":\"{}\",\"headline\":\"{}\",\"classification\":\"{}\",\"previousStatus\":\"{}\",\"danglingAction\":{},\"danglingRef\":{},\"danglingSeq\":{},\"resumeAllowed\":{},\"resumeBlockedReason\":{},\"autoResumed\":{},\"prompt\":\"{}\",\"files\":[{}],\"checkpointId\":{},\"includesLostCall\":{},\"commit\":{}{}}}",
        escape_json(&task.session_id),
        escape_json(&task.task_id),
        escape_json(&task.headline),
        escape_json(&task.classification),
        escape_json(&task.previous_status),
        json_optional(task.dangling_action.as_deref()),
        json_optional(task.dangling_reference.as_deref()),
        task.dangling_seq
            .map(|seq| seq.to_string())
            .unwrap_or_else(|| "null".to_string()),
        task.resume_allowed,
        json_optional(task.resume_blocked_reason.as_deref()),
        task.auto_resumed,
        escape_json(&task.prompt),
        task.files
            .iter()
            .map(|path| format!("\"{}\"", escape_json(path)))
            .collect::<Vec<_>>()
            .join(","),
        json_optional(task.checkpoint_id.as_deref()),
        task.includes_lost_call,
        commit_json(task.commit.as_ref()),
        task.usage_fields
    )
}

/// A JSON string or `null`. The distinction is load-bearing on every field
/// that uses it: no dangling action at all reads differently from one whose
/// name is empty, and the card renders different text for each.
fn json_optional(value: Option<&str>) -> String {
    match value {
        Some(text) => format!("\"{}\"", escape_json(text)),
        None => "null".to_string(),
    }
}

fn approval_json(approval: &Approval) -> String {
    format!(
        "{{\"sessionId\":\"{}\",\"taskId\":\"{}\",\"kind\":\"{}\",\"payload\":{},\"unavailableReason\":{}}}",
        escape_json(&approval.session_id),
        escape_json(&approval.task_id),
        approval.kind,
        approval.payload,
        json_optional(approval.unavailable_reason.as_deref())
    )
}

#[cfg(test)]
mod tests {
    use super::{Sweep, apply_decision, pending_card_payload, run_sweep, sweep_payload};
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use workspace_engine::{
        AskUserInput, Config, PendingApprovalRef, PendingQuestionRef, PlanStep, StepStatus, Task,
        TaskPlan, TaskStatus, WorkspaceEngine,
    };

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    /// An engine on a data directory of this test's own, built from an explicit
    /// `Config` rather than through `default_engine`. The sweep the endpoints
    /// use is memoized process-wide and reads the user's real data directory;
    /// neither belongs in a test, which is why `run_sweep`, `sweep_payload` and
    /// `apply_decision` all take an engine.
    fn engine(name: &str) -> (WorkspaceEngine, PathBuf) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!(
            "damaian-shell-recovery-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&data_dir).expect("temp data dir");
        let config = Config {
            data_dir: data_dir.clone(),
            ..Config::default()
        };
        (WorkspaceEngine::new(config), data_dir)
    }

    fn task_left_in(
        engine: &WorkspaceEngine,
        session_id: &str,
        prompt: &str,
        status: TaskStatus,
        dangling: Option<(&str, &str, bool)>,
    ) -> Task {
        let task = engine
            .session_store
            .create_task(session_id, prompt, "mock", "m")
            .unwrap();
        let task = engine
            .session_store
            .update_task_status(&task, status, None)
            .unwrap();
        if let Some((action, reference, side_effecting)) = dangling {
            let _marker = engine
                .session_store
                .start_action(&task, action, reference, side_effecting)
                .unwrap();
        }
        task
    }

    fn sweep(engine: &WorkspaceEngine) -> (Sweep, String) {
        let swept = run_sweep(engine).expect("sweep should run");
        let payload = sweep_payload(engine, &swept).expect("payload should render");
        (swept, payload)
    }

    fn decision(session_id: &str, task_id: &str, decision: &str) -> HashMap<String, String> {
        HashMap::from([
            ("session_id".to_string(), session_id.to_string()),
            ("task_id".to_string(), task_id.to_string()),
            ("decision".to_string(), decision.to_string()),
        ])
    }

    fn audit_text(data_dir: &Path) -> String {
        fs::read_to_string(data_dir.join("audit").join("events.jsonl")).unwrap_or_default()
    }

    fn git_repository(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        let repo = std::env::temp_dir().join(format!(
            "damaian-shell-recovery-repo-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .expect("git should run");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet", "--initial-branch=main"]);
        git(&["config", "user.email", "test@damaian.local"]);
        git(&["config", "user.name", "Damaian Test"]);
        git(&["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "--quiet", "-m", "initial"]);
        repo
    }

    /// Spec 35 §5.7: an interrupted commit is recoverable evidence. The card
    /// is offered the prepared commit and where HEAD is now, and nothing is
    /// resolved automatically.
    #[test]
    fn an_interrupted_commit_reports_the_prepared_commit_and_the_current_head() {
        let (engine, _dir) = engine("commit-dangling");
        let repo = git_repository("commit-dangling");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let preparation = engine
            .commit
            .prepare(&repo, &session.id)
            .expect("preparation reads the repository");
        task_left_in(
            &engine,
            &session.id,
            "Commit prepared changes",
            TaskStatus::RunningTool,
            Some(("commit", &preparation.prepare_id, true)),
        );

        let (_, payload) = sweep(&engine);
        assert!(
            payload.contains(r#""danglingAction":"commit""#),
            "{payload}"
        );
        assert!(payload.contains(r#""commit":{"expectedOld":"#), "{payload}");
        assert!(payload.contains(r#""preparedCommit":null"#), "{payload}");
        assert!(payload.contains(r#""landed":false"#), "{payload}");
        let _ = fs::remove_dir_all(&repo);
    }

    /// Requirements 1 and 2 as the card actually receives them: the specific
    /// action is named, no resume is offered, and the absence has a reason.
    #[test]
    fn an_unknown_outcome_is_named_and_offered_no_resume() {
        let (engine, _dir) = engine("unknown-outcome");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        task_left_in(
            &engine,
            &session.id,
            "run the migration",
            TaskStatus::RunningTool,
            Some(("run_command", "psql -f migrate.sql", true)),
        );

        let (_, payload) = sweep(&engine);

        assert!(
            payload.contains("A command was in progress and its outcome is unknown"),
            "got {payload}"
        );
        assert!(payload.contains("\"resumeAllowed\":false"), "got {payload}");
        assert!(
            payload.contains("there is no way to tell whether it finished"),
            "got {payload}"
        );
        assert!(payload.contains("\"autoResumed\":false"), "got {payload}");
    }

    /// The other classification: read-only work is offered a resume, and §5.5's
    /// automatic authorization is reported rather than hidden.
    #[test]
    fn read_only_work_is_authorized_and_reported() {
        let (engine, _dir) = engine("interrupted");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = task_left_in(
            &engine,
            &session.id,
            "explain the indexer",
            TaskStatus::PreparingContext,
            Some(("read_file", "src/lib.rs", false)),
        );

        let (_, payload) = sweep(&engine);

        assert!(payload.contains("\"resumeAllowed\":true"), "got {payload}");
        assert!(payload.contains("\"autoResumed\":true"), "got {payload}");
        assert!(payload.contains("explain the indexer"), "got {payload}");
        // Authorized means the status moved. It does not mean the turn ran:
        // nothing here calls a model adapter.
        assert_eq!(
            engine
                .session_store
                .read_task_statuses(&session.id)
                .unwrap()
                .get(&task.id)
                .map(String::as_str),
            Some("preparing_context")
        );
    }

    /// Requirement 5's enforcement point, exercised through the endpoint rather
    /// than predicted from the payload. A webview asking for a resume it was
    /// never offered is refused, because the decision is re-classified here.
    #[test]
    fn a_resume_the_card_never_offered_is_still_refused() {
        let (engine, _dir) = engine("refused");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = task_left_in(
            &engine,
            &session.id,
            "deploy it",
            TaskStatus::ApplyingPatch,
            Some(("apply_patch", "patch_1", true)),
        );

        let error = apply_decision(&engine, &decision(&session.id, &task.id, "resume"))
            .expect_err("an unknown outcome may never be resumed");

        assert!(error.contains("Cannot resume"), "got {error}");
    }

    #[test]
    fn marking_a_task_failed_drops_it_from_the_next_payload() {
        let (engine, _dir) = engine("decided");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = task_left_in(
            &engine,
            &session.id,
            "run the migration",
            TaskStatus::RunningTool,
            Some(("run_command", "psql", true)),
        );
        let (swept, before) = sweep(&engine);
        assert!(before.contains(&task.id), "got {before}");

        apply_decision(&engine, &decision(&session.id, &task.id, "mark_failed")).unwrap();

        // The same snapshot, re-rendered: the status filter is what keeps a
        // settled task off the screen when the webview reloads.
        let after = sweep_payload(&engine, &swept).unwrap();
        assert!(!after.contains(&task.id), "got {after}");
    }

    /// Requirement 5 of spec 45: the card comes back, and presenting it runs
    /// nothing. Asserted on the audit log, which is where an execution would
    /// have to appear.
    #[test]
    fn a_reattached_command_approval_runs_nothing() {
        let (engine, dir) = engine("reattached");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "list the files", "mock", "m")
            .unwrap();
        let proposal = engine
            .validation_orchestrator
            .propose_command(&dir, "ls -la", "Desktop command proposal")
            .unwrap();
        engine
            .session_store
            .await_approval(
                &task,
                &PendingApprovalRef {
                    kind: "command".to_string(),
                    proposal_id: proposal.id.clone(),
                },
            )
            .unwrap();

        let (_, payload) = sweep(&engine);

        assert!(payload.contains("\"kind\":\"command\""), "got {payload}");
        assert!(payload.contains(&proposal.id), "got {payload}");
        assert!(payload.contains("ls -la"), "got {payload}");
        assert!(
            !audit_text(&dir).contains("stored_command_executed"),
            "re-presenting an approval must not run the command"
        );
        // Still pending, so the user's decision is still the one that counts.
        assert_eq!(
            engine
                .command_store
                .load_proposal(&proposal.id)
                .unwrap()
                .status,
            "pending"
        );
    }

    /// Observation 103's follow-up (2026-11-14): unlike a recovered task, a
    /// reattached approval was never rechecked against the task's current
    /// status, so once offered it stayed in every sweep for the rest of the
    /// process's life even after the task it belonged to settled on its own
    /// (completed, cancelled, or superseded) without the user ever deciding
    /// this specific card.
    #[test]
    fn a_settled_tasks_reattached_approval_drops_from_the_next_payload() {
        let (engine, dir) = engine("approval-settled");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "list the files", "mock", "m")
            .unwrap();
        let proposal = engine
            .validation_orchestrator
            .propose_command(&dir, "ls -la", "Desktop command proposal")
            .unwrap();
        engine
            .session_store
            .await_approval(
                &task,
                &PendingApprovalRef {
                    kind: "command".to_string(),
                    proposal_id: proposal.id.clone(),
                },
            )
            .unwrap();

        let (swept, before) = sweep(&engine);
        assert!(before.contains(&proposal.id), "got {before}");

        // The task settles on its own — cancelled by the user, say, from a
        // later turn — without the reattached card itself ever being decided.
        engine
            .session_store
            .update_task_status(&task, TaskStatus::Cancelled, None)
            .unwrap();

        // The same snapshot, re-rendered: the status filter is what keeps the
        // now-settled approval off the screen when the webview reloads.
        let after = sweep_payload(&engine, &swept).unwrap();
        assert!(!after.contains(&proposal.id), "got {after}");
    }

    /// Spec 21 made plan review a third kind of pending approval. Before this,
    /// the sweep hit its catch-all, failed the task with "unknown pending
    /// approval kind plan", and told the user a recoverable turn could not be
    /// restored — while its paused turn sat on disk the whole time.
    #[test]
    fn a_reattached_plan_review_is_offered_for_review() {
        let (engine, dir) = engine("plan-review");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "add retry", "mock", "m")
            .unwrap();
        let mut plan = TaskPlan::new(&task.id, 1);
        plan.steps.push(PlanStep {
            id: "step_1".to_string(),
            title: "Add a retry helper".to_string(),
            detail: None,
            status: StepStatus::Pending,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
            build: None,
        });
        engine.session_store.create_plan(&task, &plan).unwrap();
        let pending = dir.join("chat").join("pending");
        fs::create_dir_all(&pending).unwrap();
        fs::write(
            pending.join("planprop_1.json"),
            r#"{"proposal_id":"planprop_1","plan_review":{"deferred_action":"apply a patch"}}"#,
        )
        .unwrap();
        engine
            .session_store
            .await_approval(
                &task,
                &PendingApprovalRef {
                    kind: "plan".to_string(),
                    proposal_id: "planprop_1".to_string(),
                },
            )
            .unwrap();

        let (_, payload) = sweep(&engine);

        assert!(payload.contains("\"kind\":\"plan\""), "got {payload}");
        assert!(payload.contains("planprop_1"), "got {payload}");
        // Both halves the card needs: the plan from the log, the deferred
        // action from the paused turn.
        assert!(payload.contains("Add a retry helper"), "got {payload}");
        assert!(
            payload.contains("\"deferredAction\":\"apply a patch\""),
            "got {payload}"
        );
        // Spec 103 Task 2: this historical review offered no budget, so the
        // reattached payload keeps the exact pre-budget key set.
        let sweep: serde_json::Value = serde_json::from_str(&payload).expect("the sweep is JSON");
        let approval = &sweep["approvals"][0]["payload"];
        let keys: Vec<&str> = approval
            .as_object()
            .expect("the payload is an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["deferredAction", "plan", "proposalId"]);
        assert_eq!(
            engine
                .session_store
                .read_task_statuses(&session.id)
                .unwrap()
                .get(&task.id)
                .map(String::as_str),
            Some("waiting_for_approval")
        );
    }

    /// Spec 103 Task 2: a worker restart reattaches the same engine-built
    /// budget review, not a value reconstructed from current configuration.
    #[test]
    fn a_reattached_plan_review_carries_its_budget_review() {
        let (engine, dir) = engine("plan-budget");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "add retry", "mock", "m")
            .unwrap();
        let mut plan = TaskPlan::new(&task.id, 1);
        plan.steps.push(PlanStep {
            id: "step_1".to_string(),
            title: "Add a retry helper".to_string(),
            detail: None,
            status: StepStatus::Pending,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
            build: None,
        });
        engine.session_store.create_plan(&task, &plan).unwrap();
        let pending = dir.join("chat").join("pending");
        fs::create_dir_all(&pending).unwrap();
        fs::write(
            pending.join("planprop_budget.json"),
            r#"{"proposal_id":"planprop_budget","plan_review":{"deferred_action":"apply a patch","budget":{"budget":{"actionRounds":96,"explorationRounds":144,"inputTokens":8000000,"outputTokens":512000},"title":"Task budget","note":"This worker may run up to these totals.","rows":[{"dimension":"actionRounds","label":"Action rounds","value":96,"unit":"rounds"},{"dimension":"explorationRounds","label":"Exploration rounds","value":144,"unit":"rounds"},{"dimension":"inputTokens","label":"Input tokens","value":8000000,"unit":"tokens"},{"dimension":"outputTokens","label":"Output tokens","value":512000,"unit":"tokens"}]}}}"#,
        )
        .unwrap();
        engine
            .session_store
            .await_approval(
                &task,
                &PendingApprovalRef {
                    kind: "plan".to_string(),
                    proposal_id: "planprop_budget".to_string(),
                },
            )
            .unwrap();

        let (_, payload) = sweep(&engine);
        let sweep: serde_json::Value = serde_json::from_str(&payload).expect("the sweep is JSON");
        let approval = &sweep["approvals"][0]["payload"];
        let keys: Vec<&str> = approval
            .as_object()
            .expect("the payload is an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec!["budgetReview", "deferredAction", "plan", "proposalId"]
        );
        assert_eq!(approval["budgetReview"]["budget"]["actionRounds"], 96);
        assert_eq!(
            approval["budgetReview"]["rows"][0]["label"],
            "Action rounds"
        );
        assert_eq!(approval["budgetReview"]["rows"][0]["unit"], "rounds");
    }

    /// Spec 51 §5.3: a fetch pause is saved with the verbatim URL, so a crash
    /// re-presents the same dialog. Before this, the sweep failed the task with
    /// "unknown pending approval kind fetch" while the paused turn sat on disk.
    #[test]
    fn a_reattached_fetch_pause_carries_the_dialog_payload() {
        let (engine, dir) = engine("fetch-pause");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "read the guide", "mock", "m")
            .unwrap();
        let pending = dir.join("chat").join("pending");
        fs::create_dir_all(&pending).unwrap();
        fs::write(
            pending.join("fetchprop_1.json"),
            concat!(
                r#"{"proposal_id":"fetchprop_1","fetch_call":{"#,
                r#""host":"docs.example.com","url":"https://docs.example.com/a?b=1","#,
                r#""approval_key":"docs.example.com","long_components":[],"#,
                r#""query":"b=1","fragment":null}}"#,
            ),
        )
        .unwrap();
        engine
            .session_store
            .await_approval(
                &task,
                &PendingApprovalRef {
                    kind: "fetch".to_string(),
                    proposal_id: "fetchprop_1".to_string(),
                },
            )
            .unwrap();

        let (_, payload) = sweep(&engine);

        assert!(payload.contains("\"kind\":\"fetch\""), "got {payload}");
        assert!(
            payload.contains("\"proposalId\":\"fetchprop_1\""),
            "got {payload}"
        );
        assert!(
            payload.contains("\"host\":\"docs.example.com\""),
            "got {payload}"
        );
        assert!(
            payload.contains("\"url\":\"https://docs.example.com/a?b=1\""),
            "the verbatim URL is what the dialog shows and the request uses: got {payload}"
        );
        assert!(payload.contains("\"query\":\"b=1\""), "got {payload}");
        // The card renders the headline in the webview; the payload is the same
        // `fetchProposal` shape the live dialog uses.
        assert!(
            include_str!("../static/app.js").contains("approval.kind === \"fetch\""),
            "the reattached card must have a fetch branch"
        );
    }

    /// Spec 50 Task 5: a question waiting when the process stopped comes back
    /// as a question, not as an approval, with what was asked and whether it
    /// has been answered. The payload carries no proposal id, command or
    /// patch, so no command or patch endpoint has anything to act on.
    /// Spec 27's pre-send review is saved like a plan review, so a crash while
    /// it is open must re-present it rather than fail the task with an unknown
    /// approval kind.
    #[test]
    fn a_reattached_context_review_carries_its_manifest() {
        let (engine, dir) = engine("context-review");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "read the code", "mock", "m")
            .unwrap();
        let pending = dir.join("chat").join("pending");
        fs::create_dir_all(&pending).unwrap();
        fs::write(
            pending.join("contextreview_1.json"),
            concat!(
                r#"{"proposal_id":"contextreview_1","context_review":{"prepared":{"#,
                r#""manifest":{"repositoryId":"repo_1","taskId":"task","assembledAtMs":0,"#,
                r#""limit":{"tokens":100,"source":"declared","safetyMarginBps":0,"window":null},"#,
                r#""totalTokens":3,"categories":[],"included":[],"excluded":[]},"#,
                r#""items":[]},"conversation":{"summary":null,"body":null},"#,
                r#""summary":null,"summary_removed":false}}"#,
            ),
        )
        .unwrap();
        engine
            .session_store
            .await_approval(
                &task,
                &PendingApprovalRef {
                    kind: "context_review".to_string(),
                    proposal_id: "contextreview_1".to_string(),
                },
            )
            .unwrap();

        let (_, payload) = sweep(&engine);

        assert!(
            payload.contains("\"kind\":\"context_review\""),
            "got {payload}"
        );
        assert!(payload.contains("contextreview_1"), "got {payload}");
        assert!(payload.contains("\"totalTokens\":3"), "got {payload}");
    }

    #[test]
    fn a_reattached_question_is_offered_for_an_answer_not_an_approval() {
        let (engine, _dir) = engine("question");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "ask before choosing", "mock", "m")
            .unwrap();
        let question = PendingQuestionRef::new();
        engine
            .session_store
            .record_question_asked(
                &session.id,
                &task.id,
                &question,
                &AskUserInput {
                    question: "Did you mean the CLI or the shell?".to_string(),
                    options: vec!["The CLI".to_string(), "The shell".to_string()],
                    allow_free_text: true,
                },
            )
            .unwrap();
        engine.session_store.await_answer(&task, &question).unwrap();

        let (_, payload) = sweep(&engine);

        assert!(payload.contains("\"kind\":\"question\""), "got {payload}");
        assert!(
            payload.contains(&format!("\"questionId\":\"{}\"", question.question_id())),
            "got {payload}"
        );
        assert!(
            payload.contains("Did you mean the CLI or the shell?"),
            "got {payload}"
        );
        assert!(
            payload.contains("\"headline\":\"A question was waiting for your answer\""),
            "the card must name it a question, not an approval: got {payload}"
        );
        assert!(
            payload.contains("\"options\":[\"The CLI\",\"The shell\"]"),
            "got {payload}"
        );
        assert!(payload.contains("\"allowFreeText\":true"), "got {payload}");
        assert!(payload.contains("\"answered\":false"), "got {payload}");
        assert!(payload.contains("\"answer\":null"), "got {payload}");
        // Not an approval. Each of these is what a command or patch card needs
        // before it can run anything.
        assert!(!payload.contains("\"proposalId\""), "got {payload}");
        assert!(!payload.contains("\"command\""), "got {payload}");
        assert!(!payload.contains("\"patchId\""), "got {payload}");
        assert!(!payload.contains("\"requiresApproval\""), "got {payload}");

        // The state a crash between recording the answer and resuming leaves:
        // the reattached card names the answer instead of asking again.
        engine
            .session_store
            .record_question_answered(&session.id, question.question_id(), "The CLI", Some(0))
            .unwrap();
        let (_, answered) = sweep(&engine);
        assert!(answered.contains("\"answered\":true"), "got {answered}");
        assert!(
            answered.contains("\"answer\":{\"answer\":\"The CLI\",\"selectedOption\":0}"),
            "got {answered}"
        );
        assert_eq!(
            engine
                .session_store
                .read_task_statuses(&session.id)
                .unwrap()
                .get(&task.id)
                .map(String::as_str),
            Some("waiting_for_approval")
        );
    }

    /// A resumed task's work is handed to a new turn, so the original never
    /// reaches a terminal status. Without a record of that, the next launch
    /// classifies it as interrupted again and offers the identical card —
    /// forever. The in-process snapshot hid this: only a *fresh* sweep shows it.
    #[test]
    fn a_resumed_task_is_not_offered_again_on_the_next_launch() {
        let (engine, _dir) = engine("superseded");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = task_left_in(
            &engine,
            &session.id,
            "explain the indexer",
            TaskStatus::PreparingContext,
            Some(("read_file", "src/lib.rs", false)),
        );
        let (_, before) = sweep(&engine);
        assert!(before.contains(&task.id), "got {before}");

        let resumed = apply_decision(&engine, &decision(&session.id, &task.id, "resume")).unwrap();
        assert!(resumed.contains("explain the indexer"), "got {resumed}");

        // A fresh sweep, standing in for the next launch: a new snapshot, not
        // the one `forget` pruned.
        let (_, after) = sweep(&engine);
        assert!(!after.contains(&task.id), "got {after}");
        // And a stale card cannot resume the same work twice.
        let repeat = apply_decision(&engine, &decision(&session.id, &task.id, "resume"))
            .expect_err("the second resume should be refused");
        assert!(repeat.contains("already resumed"), "got {repeat}");
    }

    /// Spec 19 §5.5 accounts for the model call a crash interrupted — during
    /// this sweep, in fact — but until now the figure went straight to the log
    /// and the card said nothing, leaving the crash looking free on the one
    /// screen meant to explain it.
    #[test]
    fn the_card_reports_what_the_interrupted_turn_spent() {
        let (engine, _dir) = engine("lost-call");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = task_left_in(
            &engine,
            &session.id,
            "summarise the diff",
            TaskStatus::WaitingForModel,
            None,
        );
        // What `account_for_lost_model_calls` writes for a call that was in
        // flight: an estimate, marked as lost to the crash.
        engine
            .session_store
            .record_task_usage_for_task_id(
                &session.id,
                &task.id,
                None,
                "lost_marker_1",
                Some("marker_1"),
                workspace_engine::TokenUsage::estimated(1200, 0),
                None,
                Some("lost_to_crash"),
            )
            .unwrap();

        let (_, payload) = sweep(&engine);

        assert!(payload.contains("\"inputTokens\":1200"), "got {payload}");
        assert!(
            payload.contains("\"usageSource\":\"estimated\""),
            "got {payload}"
        );
        assert!(
            payload.contains("\"includesLostCall\":true"),
            "got {payload}"
        );
    }

    /// The complement, and the one that keeps the claim honest: a task with no
    /// usage recorded carries no figures at all. Zero would read as a crash
    /// that cost nothing, and a session written before spec 19 has no numbers.
    #[test]
    fn a_task_with_no_recorded_usage_reports_no_figure() {
        let (engine, _dir) = engine("no-usage");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        task_left_in(
            &engine,
            &session.id,
            "summarise the diff",
            TaskStatus::WaitingForModel,
            None,
        );

        let (_, payload) = sweep(&engine);

        assert!(!payload.contains("inputTokens"), "got {payload}");
        assert!(
            payload.contains("\"includesLostCall\":false"),
            "got {payload}"
        );
    }

    /// The one way this surface could cause a side effect twice: an approval
    /// answered once must not be reattached on the next launch, because the
    /// command behind it has already run.
    #[test]
    fn an_answered_approval_is_not_reattached_again() {
        let (engine, dir) = engine("resolved");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "list the crate", "mock", "m")
            .unwrap();
        let proposal = engine
            .validation_orchestrator
            .propose_command(&dir, "ls -la", "Desktop command proposal")
            .unwrap();
        engine
            .session_store
            .await_approval(
                &task,
                &PendingApprovalRef {
                    kind: "command".to_string(),
                    proposal_id: proposal.id.clone(),
                },
            )
            .unwrap();
        let (_, first) = sweep(&engine);
        assert!(first.contains(&proposal.id), "got {first}");

        super::resolve_reattached_approval_with(
            &engine,
            &HashMap::from([
                ("session_id".to_string(), session.id.clone()),
                ("task_id".to_string(), task.id.clone()),
                ("outcome".to_string(), "approved".to_string()),
            ]),
        )
        .unwrap();

        // A fresh sweep, standing in for the next launch.
        let (_, second) = sweep(&engine);
        assert!(!second.contains(&proposal.id), "got {second}");
        assert_eq!(
            engine
                .session_store
                .read_task_statuses(&session.id)
                .unwrap()
                .get(&task.id)
                .map(String::as_str),
            Some("cancelled")
        );
        // The proposal itself is untouched: closing the turn is not the same as
        // discarding what it was waiting on.
        assert!(engine.command_store.load_proposal(&proposal.id).is_ok());
    }

    /// Spec 57 correction C12, as observed on 2026-10-16: the sweep ran while a
    /// worker was preparing context and offered *Resume* for its live task.
    #[test]
    fn a_live_worker_is_not_offered_for_recovery() {
        let (engine, dir) = engine("live-worker");
        let session = engine
            .session_store
            .create_session("repo_1", "Worker")
            .unwrap();
        engine
            .session_store
            .record_worker_spawned(
                &session.id,
                &workspace_engine::WorkerRecord {
                    parent_session_id: None,
                    parent_task_id: None,
                    inherited_protected_paths: Vec::new(),
                    source_repository: PathBuf::from("/code/app"),
                    worktree_path: PathBuf::from("/code/app/.damaian/worktrees/w"),
                    branch: "damaian/w".to_string(),
                    base_commit: "0123456".to_string(),
                    included_uncommitted: false,
                    git_commands: Vec::new(),
                },
            )
            .unwrap();
        let task = task_left_in(
            &engine,
            &session.id,
            "add retries",
            TaskStatus::PreparingContext,
            Some(("model_call", "m", false)),
        );
        // This test process stands in for the worker the shell registered.
        let _worker = workspace_engine::ProcessRegistry::open(&dir)
            .unwrap()
            .register(
                workspace_engine::ProcessKind::Worker,
                &session.id,
                std::process::id(),
            )
            .unwrap();

        let (_, payload) = sweep(&engine);

        assert!(!payload.contains(&task.id), "got {payload}");
        assert!(
            !audit_text(&dir).contains("task_recovered"),
            "a live worker's task is not crash debris"
        );
        let error = apply_decision(&engine, &decision(&session.id, &task.id, "mark_failed"))
            .expect_err("a live worker's task is not a recovered task");
        assert!(error.contains("is not a recovered task"), "got {error}");
    }

    /// Spec 110: the shell answers `/api/recovery` while a session works, so a
    /// first sweep that lands mid-turn must not take that turn for crash
    /// debris. Once the work ends, the same task is classified as before.
    #[test]
    fn a_working_session_is_not_swept_until_its_work_ends() {
        let (engine, dir) = engine("working-session");
        let session = engine
            .session_store
            .create_session("repo_1", "Working")
            .unwrap();
        let task = task_left_in(
            &engine,
            &session.id,
            "add retries",
            TaskStatus::PreparingContext,
            Some(("model_call", "m", false)),
        );
        let lane = crate::lanes::claim(&session.id, "Waiting for the model").unwrap();

        let (_, payload) = sweep(&engine);
        assert!(!payload.contains(&task.id), "got {payload}");
        assert!(
            !audit_text(&dir).contains("task_recovered"),
            "a working session's task is not crash debris"
        );

        drop(lane);
        let (_, payload) = sweep(&engine);
        assert!(payload.contains(&task.id), "got {payload}");
    }

    /// Spec 98: a reloaded session must not offer a patch card whose remaining
    /// file became protected after the preview. The engine refuses the stale
    /// card; the shell renders the ordinary unavailable outcome, never a patch
    /// card the user could approve.
    #[test]
    fn a_reloaded_patch_card_with_a_newly_protected_file_is_unavailable() {
        let (engine, data_dir) = engine("protected-reload");
        let repo = data_dir.join("repo");
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/a.rs"), "pub fn a() {}\n").unwrap();
        let session = engine
            .session_store
            .create_session(&workspace_engine::repository_id_for_root(&repo), "Reload")
            .unwrap();
        let task = task_left_in(
            &engine,
            &session.id,
            "edit a.rs",
            TaskStatus::WaitingForApproval,
            None,
        );
        let patch = workspace_engine::ProposedPatch {
            id: "patch_protected_reload".to_string(),
            session_id: session.id.clone(),
            origin_session_id: String::new(),
            task_id: Some(task.id.clone()),
            summary: "Waiting for review".to_string(),
            status: "pending".to_string(),
            created_at_ms: 1,
            repair_scope: None,
            files: vec![workspace_engine::ProposedFilePatch {
                path: "src/a.rs".to_string(),
                status: "modified".to_string(),
                base_hash: None,
                new_hash: String::new(),
                diff: String::new(),
                hunks: Vec::new(),
                new_content: "pub fn a() { let changed = true; }\n".to_string(),
            }],
        };
        engine.patch_store.save(&patch).unwrap();
        engine
            .session_store
            .await_approval(
                &task,
                &workspace_engine::PendingApprovalRef {
                    kind: "patch".to_string(),
                    proposal_id: patch.id.clone(),
                },
            )
            .unwrap();
        engine
            .add_task_protected_paths(&repo, &session.id, &task.id, &["src/a.rs".to_string()])
            .unwrap();

        let (_swept, payload) = sweep(&engine);

        assert!(
            !payload.contains("\"kind\":\"patch\""),
            "a pending patch with a newly protected file must not be offered: {payload}"
        );
        assert!(
            payload.contains("\"kind\":\"unavailable\""),
            "the stale card must become the ordinary unavailable outcome: {payload}"
        );
        assert!(payload.contains("protected task path"), "{payload}");
        assert!(payload.contains("src/a.rs"), "{payload}");

        let _ = fs::remove_dir_all(&data_dir);
    }

    /// §5.5 of spec 17: a task awaiting an approval nothing recorded is failed
    /// with a reason rather than rebuilt from partial data.
    #[test]
    fn an_approval_with_no_recorded_proposal_is_reported_as_unavailable() {
        let (engine, _dir) = engine("unavailable");
        let session = engine
            .session_store
            .create_session("repo_1", "Recovery")
            .unwrap();
        let task = task_left_in(
            &engine,
            &session.id,
            "do the thing",
            TaskStatus::WaitingForApproval,
            None,
        );

        let (_, payload) = sweep(&engine);

        assert!(
            payload.contains("\"kind\":\"unavailable\""),
            "got {payload}"
        );
        assert!(
            payload.contains("records no pending proposal"),
            "got {payload}"
        );
        assert_eq!(
            engine
                .session_store
                .read_task_statuses(&session.id)
                .unwrap()
                .get(&task.id)
                .map(String::as_str),
            Some("failed")
        );
    }

    /// A task paused on `kind` while the app runs, its paused turn written as
    /// `pending` (none for a command or patch, whose proposal is stored).
    fn paused_on(
        engine: &WorkspaceEngine,
        dir: &Path,
        kind: &str,
        proposal_id: &str,
        pending: Option<&str>,
    ) -> (String, Task) {
        let session = engine
            .session_store
            .create_session("repo_1", "Reload")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "do the work", "mock", "m")
            .unwrap();
        if let Some(pending) = pending {
            let folder = dir.join("chat").join("pending");
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join(format!("{proposal_id}.json")), pending).unwrap();
        }
        engine
            .session_store
            .await_approval(
                &task,
                &PendingApprovalRef {
                    kind: kind.to_string(),
                    proposal_id: proposal_id.to_string(),
                },
            )
            .unwrap();
        (session.id, task)
    }

    fn pending_card(
        engine: &WorkspaceEngine,
        session_id: &str,
        task_id: &str,
    ) -> serde_json::Value {
        let payload = pending_card_payload(engine, session_id, task_id).expect("the read succeeds");
        serde_json::from_str(&payload).expect("the card is JSON")
    }

    /// Spec 107 note 2026-11-20: a reload redraws the card of a decision
    /// raised while the app runs, for every kind the startup sweep can
    /// reattach, with the payload the card builders take and the proposal id
    /// the engine's `pendingDecision.cardId` names.
    #[test]
    fn a_reload_reads_the_pending_card_of_every_decision_kind() {
        let (engine, dir) = engine("pending-card-kinds");
        let command = engine
            .validation_orchestrator
            .propose_command(&dir, "cargo build", "Desktop command proposal")
            .unwrap();
        let check = engine
            .validation_orchestrator
            .propose_command(&dir, "cargo test", "Desktop command proposal")
            .unwrap();
        let patch = workspace_engine::ProposedPatch {
            id: "patch_live".to_string(),
            session_id: String::new(),
            origin_session_id: String::new(),
            task_id: None,
            summary: "Rename the helper".to_string(),
            status: "pending".to_string(),
            created_at_ms: 1,
            repair_scope: None,
            files: vec![workspace_engine::ProposedFilePatch {
                path: "src/a.rs".to_string(),
                status: "modified".to_string(),
                base_hash: None,
                new_hash: String::new(),
                diff: String::new(),
                hunks: Vec::new(),
                new_content: "pub fn b() {}\n".to_string(),
            }],
        };
        engine.patch_store.save(&patch).unwrap();
        let check_batch = format!(
            r#"{{"proposal_id":"checks_1","check_batch":{{"checks":[{{"proposal_id":"{}"}}]}}}}"#,
            check.id
        );
        let cases: Vec<(&str, &str, Option<&str>, &str, &str)> = vec![
            ("command", &command.id, None, "proposalId", "cargo build"),
            ("patch", "patch_live", None, "patchId", "Rename the helper"),
            (
                "provider_fallback",
                "fallback_1",
                Some(
                    r#"{"proposal_id":"fallback_1","provider_fallback":{"fallback_provider":"openai"}}"#,
                ),
                "proposalId",
                "openai",
            ),
            (
                "round_budget",
                "rounds_1",
                Some(
                    r#"{"proposal_id":"rounds_1","round_budget":{"rounds_used":16,"rounds_granted":16}}"#,
                ),
                "proposalId",
                "\"roundsUsed\":16",
            ),
            (
                "fetch",
                "fetch_1",
                Some(concat!(
                    r#"{"proposal_id":"fetch_1","fetch_call":{"host":"docs.example.com","#,
                    r#""url":"https://docs.example.com/a","approval_key":"docs.example.com","#,
                    r#""long_components":[],"query":null,"fragment":null}}"#,
                )),
                "proposalId",
                "https://docs.example.com/a",
            ),
            (
                "checks",
                "checks_1",
                Some(&check_batch),
                "proposalId",
                "cargo test",
            ),
        ];
        for (kind, proposal_id, pending, id_field, shown) in cases {
            let (session_id, task) = paused_on(&engine, &dir, kind, proposal_id, pending);
            let card = pending_card(&engine, &session_id, &task.id)["card"].clone();
            assert_eq!(card["kind"], kind, "{kind}: {card}");
            assert_eq!(card["taskId"], task.id.as_str(), "{kind}: {card}");
            assert_eq!(card["payload"][id_field], proposal_id, "{kind}: {card}");
            assert!(card.to_string().contains(shown), "{kind}: {card}");
        }
    }

    /// The plan review is the kind the owner met (2026-11-20): the card needs
    /// the plan from the log and the deferred action from the paused turn, in
    /// `plan_proposal_json`'s shape.
    #[test]
    fn a_reload_reads_a_pending_plan_review_in_the_live_shape() {
        let (engine, dir) = engine("pending-card-plan");
        let (session_id, task) = paused_on(
            &engine,
            &dir,
            "plan",
            "planprop_live",
            Some(
                r#"{"proposal_id":"planprop_live","plan_review":{"deferred_action":"apply a patch"}}"#,
            ),
        );
        let mut plan = TaskPlan::new(&task.id, 1);
        plan.steps.push(PlanStep {
            id: "step_1".to_string(),
            title: "Add a retry helper".to_string(),
            detail: None,
            status: StepStatus::Pending,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
            build: None,
        });
        engine.session_store.create_plan(&task, &plan).unwrap();

        let card = pending_card(&engine, &session_id, &task.id)["card"].clone();

        assert_eq!(card["kind"], "plan", "{card}");
        assert_eq!(card["payload"]["proposalId"], "planprop_live", "{card}");
        assert_eq!(card["payload"]["deferredAction"], "apply a patch", "{card}");
        assert!(card.to_string().contains("Add a retry helper"), "{card}");
    }

    /// The read runs on every reload, so unlike the sweep it must never act on
    /// what it finds: an unrestorable pause draws no card and stays waiting,
    /// with nothing audited, for the next launch's sweep to close.
    #[test]
    fn reading_a_pending_card_writes_nothing() {
        let (engine, dir) = engine("pending-card-pure");
        let (session_id, task) = paused_on(&engine, &dir, "plan", "planprop_gone", None);
        let command = engine
            .validation_orchestrator
            .propose_command(&dir, "ls -la", "Desktop command proposal")
            .unwrap();
        let (command_session, command_task) =
            paused_on(&engine, &dir, "command", &command.id, None);
        let audit_before = audit_text(&dir);

        for _ in 0..2 {
            assert_eq!(
                pending_card(&engine, &session_id, &task.id)["card"],
                serde_json::Value::Null
            );
            assert_eq!(
                pending_card(&engine, &command_session, &command_task.id)["card"]["kind"],
                "command"
            );
        }

        assert_eq!(audit_text(&dir), audit_before, "the read audited something");
        assert_eq!(
            engine
                .session_store
                .read_task_statuses(&session_id)
                .unwrap()
                .get(&task.id)
                .map(String::as_str),
            Some("waiting_for_approval"),
            "the read failed the task"
        );
        assert_eq!(
            engine
                .command_store
                .load_proposal(&command.id)
                .unwrap()
                .status,
            "pending",
            "the read decided the command"
        );
    }

    /// Owner report 2026-11-25: two sessions of one project paused on command
    /// approvals at once, each in its own spec 110 session host. After the
    /// first was approved, the second, reloaded, had no card left to answer:
    /// the read took the session's live host for a spec 57 worker that owns
    /// its approvals. A session host owns none — the card's answer is
    /// forwarded to it — so each session keeps its card while its host runs.
    #[test]
    fn a_session_with_a_live_session_host_keeps_its_pending_card() {
        let (engine, dir) = engine("pending-card-hosts");
        let first = engine
            .validation_orchestrator
            .propose_command(&dir, "ls -la", "Desktop command proposal")
            .unwrap();
        let second = engine
            .validation_orchestrator
            .propose_command(&dir, "git status", "Desktop command proposal")
            .unwrap();
        let (first_session, first_task) = paused_on(&engine, &dir, "command", &first.id, None);
        let (second_session, second_task) = paused_on(&engine, &dir, "command", &second.id, None);
        // Two live processes stand in for the two hosts: the registry names an
        // entry by pid, so one pid cannot register both sessions.
        let registry = workspace_engine::ProcessRegistry::open(&dir).unwrap();
        let mut stand_in = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("a stand-in host process");
        let _first_host = registry
            .register(
                workspace_engine::ProcessKind::SessionHost,
                &first_session,
                std::process::id(),
            )
            .unwrap();
        let _second_host = registry
            .register(
                workspace_engine::ProcessKind::SessionHost,
                &second_session,
                stand_in.id(),
            )
            .unwrap();
        assert!(
            engine.session_store.has_live_host(&second_session).unwrap(),
            "the stand-in host is not registered as live"
        );

        // The first session's approval resumed its turn.
        engine
            .session_store
            .update_task_status(&first_task, TaskStatus::Complete, None)
            .unwrap();
        let first_card = pending_card(&engine, &first_session, &first_task.id)["card"].clone();
        let second_card = pending_card(&engine, &second_session, &second_task.id)["card"].clone();
        let _ = stand_in.kill();
        let _ = stand_in.wait();

        assert_eq!(first_card, serde_json::Value::Null);
        assert_eq!(second_card["kind"], "command", "{second_card}");
        assert_eq!(
            second_card["payload"]["proposalId"],
            second.id.as_str(),
            "{second_card}"
        );
    }

    /// A task that is not waiting has no card, even when its old pause link is
    /// still in the log.
    #[test]
    fn a_settled_task_has_no_pending_card() {
        let (engine, dir) = engine("pending-card-settled");
        let command = engine
            .validation_orchestrator
            .propose_command(&dir, "ls -la", "Desktop command proposal")
            .unwrap();
        let (session_id, task) = paused_on(&engine, &dir, "command", &command.id, None);
        engine
            .session_store
            .update_task_status(&task, TaskStatus::Complete, None)
            .unwrap();

        assert_eq!(
            pending_card(&engine, &session_id, &task.id)["card"],
            serde_json::Value::Null
        );
    }
}
