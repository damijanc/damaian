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
    CommandProposal, ProposedPatch, ReattachedApproval, RecoveredTask, TaskStatus, WorkspaceEngine,
    allow_always_eligible, classify_all, classify_session, command_approval_prompt, headline,
    reattach_pending_approvals, resume_allowed, resume_blocked_reason,
};

use crate::{escape_json, patch_files_json};

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
    let mut statuses: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut open = Vec::new();
    for task in &sweep.recovered {
        if !statuses.contains_key(&task.session_id) {
            let read = engine
                .session_store
                .read_task_statuses(&task.session_id)
                .map_err(|error| error.to_string())?;
            statuses.insert(task.session_id.clone(), read);
        }
        let settled = statuses[&task.session_id]
            .get(&task.task_id)
            .and_then(|status| TaskStatus::parse(status))
            .is_some_and(|status| status.is_terminal());
        if !settled {
            open.push(recovered_json(task));
        }
    }

    Ok(format!(
        "{{\"tasks\":[{}],\"approvals\":[{}]}}",
        open.join(","),
        sweep
            .approvals
            .iter()
            .map(approval_json)
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

    let prompt = match decision.as_str() {
        "resume" => {
            workspace_engine::resume(&engine.session_store, &engine.audit_log, &task)
                .map_err(|error| error.to_string())?;
            task_prompt(engine, &session_id, &task_id)?
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
    let classified = classify_all(&engine.session_store, &engine.audit_log)
        .map_err(|error| error.to_string())?;

    let sessions = engine
        .session_store
        .list_sessions(None)
        .map_err(|error| error.to_string())?;
    let repository_ids: HashMap<String, String> = sessions
        .iter()
        .map(|session| (session.id.clone(), session.repository_id.clone()))
        .collect();

    let mut recovered = Vec::new();
    for task in &classified {
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
        recovered.push(describe(engine, task, &repository_ids, auto_resumed)?);
    }

    let mut approvals = Vec::new();
    for session in &sessions {
        let reattached = reattach_pending_approvals(
            &engine.session_store,
            &engine.audit_log,
            &engine.command_store,
            &engine.patch_store,
            &session.id,
        )
        .map_err(|error| error.to_string())?;
        for approval in reattached {
            approvals.push(describe_approval(engine, &session.id, approval));
        }
    }

    Ok(Sweep {
        recovered,
        approvals,
    })
}

fn describe(
    engine: &WorkspaceEngine,
    task: &RecoveredTask,
    repository_ids: &HashMap<String, String>,
    auto_resumed: bool,
) -> Result<Recovered, String> {
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

    let prompt = task_prompt(engine, &task.session_id, &task.task_id)?;
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
        files,
        checkpoint_id,
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
            payload: format!(
                "{{\"patchId\":\"{}\",\"summary\":\"{}\",\"files\":[{}]}}",
                escape_json(&patch.id),
                escape_json(&patch.summary),
                patch_files_json(&patch.files)
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
        "{{\"proposalId\":\"{}\",\"command\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{},\"allowAlways\":{},\"allowBrowserDiagnosticsForSession\":false}}",
        escape_json(&proposal.id),
        escape_json(&proposal.command),
        escape_json(&command_approval_prompt(proposal)),
        proposal.risk.as_str(),
        proposal.requires_approval,
        proposal.blocked,
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

fn recovered_json(task: &Recovered) -> String {
    format!(
        "{{\"sessionId\":\"{}\",\"taskId\":\"{}\",\"headline\":\"{}\",\"classification\":\"{}\",\"previousStatus\":\"{}\",\"danglingAction\":{},\"danglingRef\":{},\"danglingSeq\":{},\"resumeAllowed\":{},\"resumeBlockedReason\":{},\"autoResumed\":{},\"prompt\":\"{}\",\"files\":[{}],\"checkpointId\":{}}}",
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
        json_optional(task.checkpoint_id.as_deref())
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
    use super::{Sweep, apply_decision, run_sweep, sweep_payload};
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use workspace_engine::{Config, PendingApprovalRef, Task, TaskStatus, WorkspaceEngine};

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
}
