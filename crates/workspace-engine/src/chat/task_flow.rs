use super::*;

impl ChatOrchestrator {
    /// The engine's control model for a settled task.
    ///
    /// Reading the durable report here is what makes a live `ChatTurnResult`
    /// carry the same reasons and waivers a reloaded session shows. A failure to
    /// read presentation data must not fail a turn that already settled, so the
    /// control falls back to the status with no report.
    pub(super) fn task_control_for(&self, task: &Task) -> TaskControl {
        let report = self
            .session_store
            .read_completion_report(&task.session_id, &task.id)
            .ok()
            .flatten();
        let outcome = self
            .session_store
            .read_task_outcome(&task.session_id, &task.id)
            .ok()
            .flatten();
        TaskControl::for_session_with_outcome(Some(task), report.as_ref(), outcome.as_ref())
    }

    /// The live turn's presentation, with the decision it is waiting on read
    /// back from the durable pause link (spec 107 Task 1) — the same reader
    /// `/api/session` uses, so a reload shows what the live stream showed.
    ///
    /// An unreadable link leaves `pendingDecision` absent rather than failing a
    /// turn that has already settled; the indicator still says `pending`.
    pub(super) fn turn_presentation_for(
        &self,
        task: &Task,
        model_incomplete: bool,
        cancelled: bool,
        outcome: Option<&crate::task_outcomes::TaskTerminalOutcome>,
    ) -> TurnPresentation {
        let mut presentation = turn_presentation_with_outcome(
            &task.status,
            model_incomplete,
            cancelled,
            Some(&task.id),
            outcome,
        );
        if !cancelled {
            presentation.pending_decision = TaskControlService::new(&self.session_store, None)
                .pending_decision(&task.session_id, &task.id, &task.status)
                .ok()
                .flatten();
            presentation.worker_suggestion = presentation
                .pending_decision
                .as_ref()
                .and_then(|decision| self.worker_suggestion_for(task, decision));
        }
        presentation
    }

    /// The worker suggestion recorded beside `decision`'s card, if one was.
    ///
    /// Read back from the durable offer rather than decided here, so asking
    /// twice for the same card answers the same way and no other card — a
    /// later patch of the same task — ever carries it.
    fn worker_suggestion_for(
        &self,
        task: &Task,
        decision: &crate::task_control::TurnPendingDecision,
    ) -> Option<crate::task_control::WorkerSuggestion> {
        if !matches!(
            decision.kind,
            crate::task_control::PendingDecisionKind::Plan
                | crate::task_control::PendingDecisionKind::Patch
        ) {
            return None;
        }
        let offered = self
            .session_store
            .worker_suggestion_offers(&task.session_id)
            .ok()?
            .into_iter()
            .any(|(offered_task, card)| offered_task == task.id && card == decision.card_id);
        if !offered {
            return None;
        }
        let prompt = if task.user_prompt.starts_with(VERIFICATION_TURN_PROMPT) {
            self.verification_origin_prompt(&task.session_id)
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            task.user_prompt.clone()
        };
        Some(crate::task_control::WorkerSuggestion::new(
            decision.card_id.clone(),
            decision.kind,
            prompt,
        ))
    }

    /// Records the one worker suggestion a code-changing task gets (spec 107
    /// D3, option C), at its first change: a plan review whose deferred work
    /// changes code, or a patch touching more than documentation.
    ///
    /// Never in a worker, never outside Code mode, never with Worker sessions
    /// off, and never twice for the same task — its whole continuation chain,
    /// so a planless task's later patches do not ask again. The user's answer
    /// needs no record: either a worker takes the task, or the session simply
    /// continues, which is what it does without one.
    pub(super) fn offer_worker_once(
        &self,
        session: &Session,
        task: &Task,
        proposals: &TurnProposals,
    ) -> Result<()> {
        let (kind, card_id, changes_code) = if let Some(plan) = &proposals.plan {
            ("plan", plan.id.as_str(), proposals.plan_changes_code)
        } else if let Some(patch) = &proposals.patch {
            (
                "patch",
                patch.patch_id.as_str(),
                patch
                    .files
                    .iter()
                    .any(|file| !crate::verification::is_documentation(&file.path)),
            )
        } else {
            return Ok(());
        };
        if !changes_code
            || !self.config.enable_worker_sessions
            || self.session_store.session_mode(&session.id) != SessionMode::Code
            || self.session_store.worker_record(&session.id)?.is_some()
        {
            return Ok(());
        }
        let chain = self
            .session_store
            .task_continuation_chain(&session.id, &task.id)?;
        let already = self
            .session_store
            .worker_suggestion_offers(&session.id)?
            .iter()
            .any(|(offered_task, _)| chain.contains(offered_task));
        if already {
            return Ok(());
        }
        self.session_store.record_worker_suggestion_offered(
            &session.id,
            &task.id,
            kind,
            card_id,
        )?;
        self.audit_log.record(
            "worker_suggested",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("kind", kind.to_string()),
                ("cardId", card_id.to_string()),
            ],
        )?;
        Ok(())
    }

    /// Continues an incomplete task as an explicit engine-authorized action
    /// (spec 101 Task 3).
    ///
    /// The target must still be the newest task, still be `incomplete`, and
    /// still carry its completion report; the session must belong to the
    /// repository. The prompt is built here from the durable reasons and the
    /// optional user note, so a client never has to type "continue" or
    /// reconstruct the gate. The session-store transaction writes the new task
    /// and its carried plan in one locked batch; this method then runs that
    /// already-created task through the ordinary turn path. Continuing grants
    /// no patch, command, check, question or fallback approval.
    #[allow(clippy::too_many_arguments)]
    pub fn continue_incomplete_task(
        &self,
        repository_root: impl AsRef<Path>,
        session_id: &str,
        task_id: &str,
        note: Option<&str>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let repository_root = repository_root.as_ref();
        let index = crate::index_cache::IndexCache::get_or_build(&self.indexer, repository_root)?;
        let Some(session) = self.session_store.read_session(session_id)? else {
            return Err(ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        };
        if session.repository_id != index.repository_id {
            return Err(ClientError::AccessDenied(
                "Session belongs to a different repository".to_string(),
            ));
        }
        let latest = self
            .session_store
            .read_tasks(session_id)?
            .into_iter()
            .last();
        if !latest
            .as_ref()
            .is_some_and(|task| task.id == task_id && task.status == TaskStatus::Incomplete)
        {
            return Err(ClientError::InvalidInput(
                "The task is no longer the newest incomplete task".to_string(),
            ));
        }
        let Some(report) = self
            .session_store
            .read_completion_report(session_id, task_id)?
        else {
            return Err(ClientError::InvalidInput(
                "The incomplete task has no recorded completion report".to_string(),
            ));
        };
        let plan = self.session_store.read_task_plan(session_id, task_id)?;
        let prompt = continuation_prompt(&report, plan.as_ref(), note);
        let Some(continued) = self.session_store.continue_incomplete_task(
            session_id,
            task_id,
            &report,
            &prompt,
            &self.config.model_provider,
            &self.config.model_name,
        )?
        else {
            return Err(ClientError::InvalidInput(
                "The task is no longer the newest incomplete task".to_string(),
            ));
        };
        let reasons = report
            .reason_codes()
            .into_iter()
            .map(|code| code.as_str())
            .collect::<Vec<_>>()
            .join(",");
        self.audit_log.record(
            "task_continued",
            &[
                ("actor", "user".to_string()),
                ("sessionId", session_id.to_string()),
                ("fromTaskId", task_id.to_string()),
                ("taskId", continued.task.id.clone()),
                ("reasons", reasons),
            ],
        )?;
        let options = ChatTurnOptions {
            plan_already_carried: true,
            ..engine_started_turn_options(&self.session_store, session_id)
        };
        self.ask_with_session_using_task(
            repository_root,
            &prompt,
            &[],
            continued.task,
            continued.position,
            model_adapter,
            sink,
            options,
        )
    }

    /// Runs `before_task_complete` for a model answer that would end the turn
    /// (spec 32 `context.md` §3, §4).
    ///
    /// `Some(note)` means a hook denied: the caller feeds the note back to the
    /// model and continues the loop. `None` means the turn may complete. Only
    /// a completion calls this — a pause, a budget stop and a cancel are not
    /// completions, and a hook never sees them.
    ///
    /// `filesChanged` comes from the turn's checkpoint manifest: it is the
    /// durable per-task record of the paths this turn accepted a patch or
    /// changed through a command for. The session log carries findings and
    /// messages, not a path list.
    pub(super) fn before_task_complete(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        action_rounds: u32,
    ) -> Result<Option<String>> {
        // With nothing listening, this is today's completion path: no
        // checkpoint read, no dispatch and no audit.
        if crate::hooks::ordered(&self.config.hooks, HookEvent::BeforeTaskComplete).is_empty() {
            return Ok(None);
        }
        let ctx = HookContext::root();
        let files_changed = self
            .checkpoint_store
            .read_checkpoint_for_task(&session.repository_id, &task.id)
            .ok()
            .flatten()
            .map(|manifest| {
                manifest
                    .files
                    .into_iter()
                    .map(|file| file.path)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let parts = self.hooks.envelope_parts(
            Some(session),
            Some(&task.id),
            serde_json::json!({"rounds": action_rounds, "filesChanged": files_changed}),
        );
        let outcome =
            self.hooks
                .dispatch(HookEvent::BeforeTaskComplete, parts, repository_root, &ctx);
        for finding in &outcome.findings {
            let _ = self.session_store.record_finding(&session.id, finding);
        }
        if matches!(outcome.verdict, Verdict::Deny | Verdict::RequestApproval) {
            let note = format!(
                "A hook did not let this task complete: {}",
                hook_refusal(&outcome)
            );
            self.session_store
                .append_message(&session.id, Some(&task.id), "user", &note)?;
            return Ok(Some(note));
        }
        Ok(None)
    }
}
