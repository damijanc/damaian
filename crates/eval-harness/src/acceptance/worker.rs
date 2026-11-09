//! A [`Transport`] over a Damaian worker (spec 106 `context.md` D3, D4).
//!
//! Both tiers drive a worker the way the desktop shell does. Decisions travel as
//! the worker's own control-socket JSON lines, through a [`WorkerControl`]: the
//! deterministic tier hands each line to the in-process host's parser, the live
//! tier writes it to the real socket, so the text is identical. What is pending
//! is read the way the UI reads it: the worker's status, its progress file
//! (approval and question cards) and the session log (a question's options).
//! A patch is applied the way the shell's apply route applies it, followed by
//! the same `verify` the UI sends once the patch settles.
//!
//! [`WorkerControl`] is how a forbidden intervention would be sent, so this
//! module only ever builds the allowed commands: `status`, `approve`,
//! `reject`, `answer_question`, `checks` and `verify`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use workspace_engine::worker_progress::read_progress_after;
use workspace_engine::{
    BaselinePreparation, ClientError, Config, PatchApplyOutcome, Result, SessionStore,
    WorkspaceEngine,
};

use super::driver::{ApplyStep, Pending, PendingCheck, Transport};

/// Sends one control-socket request (a JSON object with `command` and its
/// arguments; the transport adds nothing else) and returns the worker's reply.
pub trait WorkerControl {
    fn request(&mut self, request: Value) -> Result<Value>;
}

/// How the worker transport waits.
#[derive(Debug, Clone, Copy)]
pub struct WorkerTiming {
    /// Between status polls while the worker is busy.
    pub poll: Duration,
    /// The longest one [`Transport::next`] may wait for the worker to settle.
    pub settle_timeout: Duration,
}

impl WorkerTiming {
    pub fn deterministic() -> Self {
        Self {
            poll: Duration::from_millis(10),
            settle_timeout: Duration::from_secs(120),
        }
    }

    pub fn live() -> Self {
        Self {
            poll: Duration::from_millis(500),
            settle_timeout: Duration::from_secs(30 * 60),
        }
    }
}

/// The actor recorded for patch decisions, as the desktop shell records them.
const PATCH_ACTOR: &str = "desktop_user";

pub struct WorkerTransport<C: WorkerControl> {
    control: C,
    engine: WorkspaceEngine,
    store: SessionStore,
    data_dir: PathBuf,
    session_id: String,
    repository_root: PathBuf,
    timing: WorkerTiming,
    progress_seq: u64,
    /// Approval cards seen in the progress file, by proposal id.
    approvals: BTreeMap<String, (String, Value)>,
    /// Patch ids in the order their cards appeared.
    patch_order: Vec<String>,
    /// Ids already answered, so a card the worker has not cleared yet is not
    /// decided twice.
    decided: HashSet<String>,
    last_task_status: Option<String>,
    /// Whether the worker has started any work. A freshly spawned worker is
    /// idle until its prompt arrives (the supervisor delivers it on a thread
    /// of its own), and that idle must not read as "the task ended".
    started: bool,
}

impl<C: WorkerControl> WorkerTransport<C> {
    /// `data_dir` and `repository_root` must be the worker's. The engine used
    /// to apply patches loads the same configuration the worker loads.
    pub fn new(
        control: C,
        data_dir: &Path,
        session_id: &str,
        repository_root: &Path,
        timing: WorkerTiming,
    ) -> Result<Self> {
        let config = worker_config(data_dir, repository_root)?;
        Ok(Self {
            control,
            engine: WorkspaceEngine::new(config),
            store: SessionStore::new(data_dir),
            data_dir: data_dir.to_path_buf(),
            session_id: session_id.to_string(),
            repository_root: repository_root.to_path_buf(),
            timing,
            progress_seq: 0,
            approvals: BTreeMap::new(),
            patch_order: Vec::new(),
            decided: HashSet::new(),
            last_task_status: None,
            started: false,
        })
    }

    pub fn control_mut(&mut self) -> &mut C {
        &mut self.control
    }

    fn send(&mut self, request: Value) -> Result<Value> {
        let reply = self.control.request(request.clone())?;
        if reply.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(ClientError::InvalidInput(format!(
                "the worker refused {request}: {reply}"
            )));
        }
        Ok(reply)
    }

    fn read_progress(&mut self) -> Result<()> {
        for line in read_progress_after(&self.data_dir, &self.session_id, self.progress_seq)? {
            if let Some(seq) = line.get("seq").and_then(Value::as_u64) {
                self.progress_seq = self.progress_seq.max(seq);
            }
            let payload = line.get("payload").cloned().unwrap_or(Value::Null);
            match line.get("kind").and_then(Value::as_str) {
                Some("approval") => {
                    let kind = payload
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let proposal = payload.get("proposal").cloned().unwrap_or(Value::Null);
                    let id = proposal
                        .get("proposalId")
                        .or_else(|| proposal.get("patchId"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if !id.is_empty() {
                        if kind == "patch" && !self.patch_order.contains(&id) {
                            self.patch_order.push(id.clone());
                        }
                        self.approvals.insert(id, (kind, proposal));
                    }
                }
                Some("status") => {
                    if let Some(status) = payload.get("taskStatus").and_then(Value::as_str) {
                        self.last_task_status = Some(status.to_string());
                    }
                    if payload.get("state").and_then(Value::as_str) == Some("running") {
                        self.started = true;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn pending_for(&mut self, id: &str) -> Result<Option<Pending>> {
        let Some((kind, proposal)) = self.approvals.get(id).cloned() else {
            return Ok(None);
        };
        Ok(Some(match kind.as_str() {
            "plan" => Pending::PlanReview {
                proposal_id: id.to_string(),
                step_titles: proposal
                    .pointer("/plan/steps")
                    .and_then(Value::as_array)
                    .map(|steps| {
                        steps
                            .iter()
                            .filter_map(|step| step.get("title").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            "command" => Pending::Command {
                proposal_id: id.to_string(),
                command: string(&proposal, "command"),
                working_directory: string(&proposal, "directory"),
            },
            "checks" => Pending::Checks {
                batch_id: id.to_string(),
                checks: checks_of(&proposal),
            },
            "round_budget" => Pending::RoundBudget {
                proposal_id: id.to_string(),
            },
            other => Pending::Unrecognised {
                detail: format!("a `{other}` decision card ({id})"),
            },
        }))
    }
}

impl<C: WorkerControl> Transport for WorkerTransport<C> {
    fn repository_root(&self) -> &Path {
        &self.repository_root
    }

    fn next(&mut self) -> Result<Pending> {
        let deadline = Instant::now() + self.timing.settle_timeout;
        loop {
            // A freshly spawned worker opens its control socket a moment after
            // the process starts; until it has started work, no answer means
            // "not up yet", not a failure. Once it has started, it is one.
            let status = match self.send(json!({"command": "status"})) {
                Ok(status) => status,
                Err(error) if self.started || Instant::now() >= deadline => return Err(error),
                Err(_) => {
                    std::thread::sleep(self.timing.poll);
                    continue;
                }
            };
            let state = status
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.read_progress()?;
            if matches!(
                state.as_str(),
                "running" | "paused" | "waiting_for_approval"
            ) {
                self.started = true;
            }
            match state.as_str() {
                "idle" if !self.started => {}
                "waiting_for_approval" => {
                    if let Some(question_id) = status.get("questionId").and_then(Value::as_str)
                        && !self.decided.contains(question_id)
                    {
                        let question = self
                            .store
                            .question_state(&self.session_id, question_id)?
                            .ok_or_else(|| {
                                ClientError::InvalidInput(format!(
                                    "question {question_id} is not in the session log"
                                ))
                            })?;
                        return Ok(Pending::Question {
                            question_id: question_id.to_string(),
                            question: question.question,
                            options: question.options,
                            allow_free_text: question.allow_free_text,
                        });
                    }
                    if let Some(proposal_id) = status.get("proposalId").and_then(Value::as_str)
                        && !self.decided.contains(proposal_id)
                        && let Some(pending) = self.pending_for(proposal_id)?
                    {
                        return Ok(pending);
                    }
                }
                "idle" => {
                    if let Some(patch_id) = self
                        .patch_order
                        .iter()
                        .find(|id| !self.decided.contains(*id))
                        .cloned()
                    {
                        let proposal = self.approvals[&patch_id].1.clone();
                        return Ok(Pending::Patch {
                            patch_id,
                            paths: proposal
                                .get("files")
                                .and_then(Value::as_array)
                                .map(|files| {
                                    files
                                        .iter()
                                        .filter_map(|file| file.get("path").and_then(Value::as_str))
                                        .map(str::to_string)
                                        .collect()
                                })
                                .unwrap_or_default(),
                        });
                    }
                    return Ok(Pending::Ended {
                        status: self
                            .last_task_status
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                    });
                }
                "shutting_down" => {
                    return Ok(Pending::Ended {
                        status: "worker_shut_down".to_string(),
                    });
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(ClientError::InvalidInput(format!(
                    "the worker did not settle within {:?}: {status}",
                    self.timing.settle_timeout
                )));
            }
            std::thread::sleep(self.timing.poll);
        }
    }

    fn approve_plan(&mut self, proposal_id: &str) -> Result<()> {
        let budget = self
            .approvals
            .get(proposal_id)
            .and_then(|(_, proposal)| proposal.pointer("/budgetReview/budget").cloned())
            .map(|budget| budget.to_string());
        let mut request = json!({"command": "approve", "proposalId": proposal_id});
        if let Some(budget) = budget {
            request["budget"] = json!(budget);
        }
        self.send(request)?;
        self.decided.insert(proposal_id.to_string());
        Ok(())
    }

    fn decide_command(&mut self, proposal_id: &str, approve: bool) -> Result<()> {
        let command = if approve { "approve" } else { "reject" };
        self.send(json!({"command": command, "proposalId": proposal_id}))?;
        self.decided.insert(proposal_id.to_string());
        Ok(())
    }

    fn decide_checks(&mut self, batch_id: &str, decisions: &[(String, bool)]) -> Result<()> {
        self.send(json!({
            "command": "checks",
            "proposalId": batch_id,
            "decisions": decisions_json(decisions),
        }))?;
        self.decided.insert(batch_id.to_string());
        Ok(())
    }

    fn answer_question(
        &mut self,
        question_id: &str,
        answer: &str,
        option: Option<usize>,
    ) -> Result<()> {
        let mut request =
            json!({"command": "answer_question", "questionId": question_id, "answer": answer});
        if let Some(option) = option {
            request["selectedOption"] = json!(option);
        }
        self.send(request)?;
        self.decided.insert(question_id.to_string());
        Ok(())
    }

    fn apply_patch(&mut self, patch_id: &str) -> Result<ApplyStep> {
        let outcome = self
            .engine
            .edit_orchestrator
            .apply_stored_patch_with_baseline(
                &self.repository_root,
                patch_id,
                None,
                None,
                PATCH_ACTOR,
                false,
            )?;
        match outcome {
            PatchApplyOutcome::Applied(_) => {
                self.decided.insert(patch_id.to_string());
                self.send(json!({"command": "verify"}))?;
                Ok(ApplyStep::Applied)
            }
            PatchApplyOutcome::BaselineRequired(batch) => Ok(ApplyStep::Baselines {
                batch_id: batch.id.clone(),
                checks: batch
                    .checks
                    .iter()
                    .map(|check| PendingCheck {
                        id: check.proposal_id.clone(),
                        command: check.command.clone(),
                        working_directory: check.directory.clone(),
                    })
                    .collect(),
            }),
        }
    }

    fn decide_baselines(&mut self, batch_id: &str, decisions: &[(String, bool)]) -> Result<()> {
        match self.engine.edit_orchestrator.resume_stored_patch_baseline(
            &self.repository_root,
            batch_id,
            decisions,
            &[],
            PATCH_ACTOR,
        )? {
            BaselinePreparation::Complete { .. } | BaselinePreparation::Awaiting(_) => Ok(()),
        }
    }

    fn reject_patch(&mut self, patch_id: &str) -> Result<()> {
        self.engine
            .edit_orchestrator
            .reject_stored_patch(patch_id, PATCH_ACTOR)?;
        self.decided.insert(patch_id.to_string());
        Ok(())
    }
}

/// The configuration the worker loads (`desktop-shell` `load_worker_config`):
/// the data directory's user and admin config plus the repository's own.
pub fn worker_config(data_dir: &Path, repository_root: &Path) -> Result<Config> {
    let base = Config {
        data_dir: data_dir.to_path_buf(),
        ..Config::default()
    };
    let user_path = base.user_config_path();
    let admin_path = base.admin_config_path();
    let repo_path = Config::repository_config_path(repository_root);
    let (mut config, _report) = Config::load_scoped(
        base,
        Some(&user_path),
        Some(&repo_path),
        Some(&admin_path),
        Some(repository_root),
    )?;
    config.data_dir = data_dir.to_path_buf();
    Ok(config)
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn checks_of(proposal: &Value) -> Vec<PendingCheck> {
    proposal
        .get("checks")
        .and_then(Value::as_array)
        .map(|checks| {
            checks
                .iter()
                .map(|check| PendingCheck {
                    id: string(check, "proposalId"),
                    command: string(check, "command"),
                    working_directory: string(check, "directory"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `checks` command's `decisions` argument: a JSON string, as the shell's
/// check route and the worker socket take it.
fn decisions_json(decisions: &[(String, bool)]) -> String {
    Value::Array(
        decisions
            .iter()
            .map(|(id, approved)| json!({"proposalId": id, "approved": approved}))
            .collect(),
    )
    .to_string()
}
