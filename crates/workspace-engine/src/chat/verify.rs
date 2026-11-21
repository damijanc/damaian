use super::*;

impl ChatOrchestrator {
    /// The completion gate for `repository_root`, with the user's
    /// `project_checks` in its floor (spec 109 D9). Every chat path that
    /// settles or reads the gate goes through this, so they read one floor.
    pub(super) fn completion_gate<'a>(
        &'a self,
        repository_root: &'a Path,
    ) -> CompletionGateService<'a> {
        CompletionGateService::new(&self.session_store, repository_root)
            .with_project_checks(&self.config.project_checks_for(repository_root))
    }

    /// Runs the verification that a settled Code-mode patch made due (spec 23).
    ///
    /// A continuation turn Damaian starts, not one the user typed. The apply
    /// path records `verification_due`; this runs in a new task carrying the
    /// plan, so the model never decides whether to verify. With every check
    /// auto-runnable it runs them and finishes with the report; with any
    /// requiring approval it pauses once on a check card and
    /// [`Self::resume_after_check_decision`] continues.
    ///
    /// A verification that does not fail, with the task's plan still open,
    /// continues the task in the same turn (spec 77).
    pub fn verify_due(
        &self,
        repository_root: impl AsRef<Path>,
        session_id: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let repository_root = repository_root.as_ref();
        let result = self.verify_due_report(repository_root, session_id, model_adapter, sink)?;
        self.continue_task_after_verification(repository_root, result, model_adapter, sink)
    }

    fn verify_due_report(
        &self,
        repository_root: &Path,
        session_id: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let Some(session) = self.session_store.read_session(session_id)? else {
            return Err(ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        }; // The loop is Code-only: in Ask, Plan and Review there is nothing
        // applied to verify. Checked again here because the mode can change
        // between the apply and this call.
        if self.session_store.session_mode(session_id) != SessionMode::Code {
            return Err(ClientError::AccessDenied(
                "Verification runs only in Code mode".to_string(),
            ));
        }
        let events = self
            .session_store
            .read_verification_events(session_id, &[])?;
        let state = fold_verification(events.iter());
        let Some(due) = state.due.clone() else {
            return Err(ClientError::InvalidInput(
                "No verification is due in this session".to_string(),
            ));
        };
        // A repair patch whose every file the user rejected left the failing
        // attempt open and nothing to verify. It closes as
        // `no_repair_applied`, and the attempt count does not increase: no
        // repair was attempted at all (spec 23 `context.md` §5).
        let decisions = self
            .session_store
            .read_patch_file_decisions(session_id, &due.task_id)?;
        let decisions: Vec<_> = decisions
            .into_iter()
            .filter(|decision| decision.patch_id == due.patch_id)
            .collect();
        if !decisions.is_empty() && decisions.iter().all(|decision| !decision.applied) {
            let mut task = self.session_store.create_task(
                session_id,
                VERIFICATION_TURN_PROMPT,
                &self.config.model_provider,
                &self.config.model_name,
            )?;
            task = self
                .session_store
                .update_task_status(&task, TaskStatus::Validating, None)?;
            self.session_store.resume_plan(&task, &due.task_id)?;
            let changed = self.verification_changed_files(session_id, &task.id, Some(&due))?;
            let file_hashes = hash_existing_files(repository_root, &changed);
            return self.finish_verification_with_outcome(
                repository_root,
                session,
                task,
                VerificationOutcome::NoRepairApplied,
                changed,
                file_hashes,
            );
        }
        // Attempt 0 is the first verification. A repair patch proposed by the
        // verification turn that was already running starts the next attempt;
        // a new patch after a finished verification does too. An interrupted
        // attempt (its due patch predates the running turn) is rerun at the
        // attempt it was already on (spec 23 `context.md` §5).
        // A command's `Due` names the task that ran it, as a patch names the
        // task that proposed it (spec 85).
        let due_patch_task = if due.from_command() {
            Some(due.task_id.clone())
        } else {
            self.patch_store
                .load(&due.patch_id)
                .ok()
                .and_then(|patch| patch.task_id)
        };
        let latest_started = events.iter().rev().find_map(|event| match event {
            VerificationEvent::Started { task_id, .. } => Some(task_id.as_str()),
            _ => None,
        });
        let repair_patch = latest_started.is_some() && due_patch_task.as_deref() == latest_started;
        let attempt = if state.selected.is_empty() {
            0
        } else if repair_patch || state.finished.is_some() {
            state.attempt.saturating_add(1)
        } else {
            state.attempt
        };
        let mut task = self.session_store.create_task(
            session_id,
            VERIFICATION_TURN_PROMPT,
            &self.config.model_provider,
            &self.config.model_name,
        )?;
        task = self
            .session_store
            .update_task_status(&task, TaskStatus::PreparingContext, None)?;
        self.session_store.resume_plan(&task, &due.task_id)?;
        task = self
            .session_store
            .update_task_status(&task, TaskStatus::Validating, None)?;
        sink.session(&session.id);
        self.verify_with_task(
            repository_root,
            session,
            task,
            due,
            attempt,
            Vec::new(),
            model_adapter,
            sink,
        )
    }

    /// Selects and runs a due verification's checks in its verification task:
    /// the build checks first for a per-change run (spec 109 D10), then the
    /// checks. `builds_done` are build checks already decided on a build card
    /// and run, so a resumed verification does not ask for them again.
    #[allow(clippy::too_many_arguments)]
    fn verify_with_task(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        due: crate::verification::VerificationDue,
        attempt: u32,
        builds_done: Vec<BuildRecord>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let session_id = session.id.as_str();
        let events = self
            .session_store
            .read_verification_events(session_id, &[])?;
        // Requirement 9's "once per batch" skip compares against checks from
        // this task's own continuation chain, never every task in the session:
        // two independent plans can both sit at generation 0, and a check an
        // unrelated task ran must not suppress this one.
        let chain = self
            .session_store
            .task_continuation_chain(session_id, &due.task_id)?;
        let scoped_state = fold_verification(
            events
                .iter()
                .filter(|event| chain.contains(event.task_id())),
        );
        let changed = self.verification_changed_files(session_id, &task.id, Some(&due))?;
        let file_hashes = hash_existing_files(repository_root, &changed);
        let index = crate::index_cache::IndexCache::get_or_build(&self.indexer, repository_root)?;
        let (map, _) = RepositoryMapStore::new(&self.config.data_dir, self.audit_log.clone())
            .load_or_build(&index, self.validation_orchestrator.command_policy())?;
        // Review fix R16: the per-change run selects the owning root only. At
        // task completion the enclosing roots must still run, because R15's
        // floor keeps them as completion obligations. The plan is the signal:
        // while primary work remains this is a per-change run; with no open or
        // blocked primary step it is the completion run.
        //
        // Spec 107 D3: a task with no plan continues after a verified patch
        // (`continue_task_after_verification`), so its patch is a per-change
        // run too, and the floor runs once when the model answers — that
        // answer-time due names its checks and is handled above. Before this a
        // planless task treated every patch as its end and ran the whole floor
        // after each one.
        let plan = self.session_store.read_task_plan(session_id, &task.id)?;
        let at_completion = match plan.as_ref().filter(|plan| has_primary_steps(plan)) {
            Some(plan) => plan
                .steps
                .iter()
                .filter(|step| !step.id.starts_with("verify_"))
                .all(|step| step.status.is_terminal() && step.status != StepStatus::Blocked),
            None => !self
                .session_store
                .planless_continues(session_id, &due.task_id)?,
        };
        // Spec 109 D6: a check whose latest run in this task chain covered
        // exactly the current tree is not proposed or run again; its recorded
        // run stands, passed or failed. A check the plan defers to an open step
        // is left to that step, as it would be if it were proposed.
        let generation = plan.as_ref().map_or(0, TaskPlan::mutation_generation);
        // Spec 109 D10: a per-change run compiles first, for each root the
        // change touched that has a build check (the current step's, else the
        // Cargo fast path), whatever D4 then defers or runs. Not at
        // completion, where the floor compiles anyway. A build that fails ends
        // the verification here: nothing else is proposed or run, and no
        // baseline is taken.
        let mut builds = Vec::new();
        if !at_completion && due.checks.is_empty() {
            let step_build = match plan.as_ref() {
                Some(plan)
                    if self
                        .session_store
                        .read_plan_approved(session_id, &task.id)? =>
                {
                    plan_current_step_build(repository_root, plan)
                }
                _ => None,
            };
            let build_checks = build_checks_for_change(&map, &changed, step_build.as_ref());
            match self.run_build_checks(
                repository_root,
                &session,
                &task,
                &due,
                attempt,
                &build_checks,
                builds_done,
                &scoped_state,
                &changed,
                &file_hashes,
                generation,
                sink,
            )? {
                BuildStage::Paused(result) => return Ok(*result),
                BuildStage::Done(records) => builds = records,
            }
            if builds.iter().any(BuildRecord::failed) {
                return self.finish_build_failed(
                    repository_root,
                    session,
                    task,
                    attempt,
                    &builds,
                    changed,
                    file_hashes,
                );
            }
        }
        let mut reuse = CheckReuse::from_state(&scoped_state, &file_hashes, generation);
        if let Some(plan) = plan.as_ref() {
            reuse.retain(|check| !plan_defers_check(repository_root, plan, check));
        }
        // Plan checks standing in for this change that their open step runs
        // instead, and detected checks a plan check displaces with nothing
        // standing in for them per change (spec 109 D4); the report names both.
        let mut displaced_deferred: Vec<CheckId> = Vec::new();
        let mut left_to_completion: Vec<CheckId> = Vec::new();
        let selection = if !due.checks.is_empty() {
            // A completion due carries the exact floor checks the answer-time
            // trigger found missing or stale. Run those and nothing else; the
            // finished steps' stale plan checks are added below, as for any
            // verification (spec 106 Task 6).
            propose_checks_exactly(
                &self.validation_orchestrator,
                repository_root,
                &map,
                &due.checks,
                &DeclaredFloor::project_check_ids(&self.config.project_checks_for(repository_root)),
                &mut reuse,
            )?
        } else if at_completion {
            // Spec 109 D9: a floor root with no detected check takes the
            // approved plan's required checks covering it, and the user's
            // `project_checks` join the floor wherever the task touched them.
            let plan_checks = match plan.as_ref() {
                Some(plan)
                    if self
                        .session_store
                        .read_plan_approved(session_id, &task.id)? =>
                {
                    plan_required_checks(repository_root, plan)
                }
                _ => Vec::new(),
            };
            let declared = DeclaredFloor {
                plan_checks,
                project_checks: DeclaredFloor::project_check_ids(
                    &self.config.project_checks_for(repository_root),
                ),
            };
            propose_completion_checks(
                &self.validation_orchestrator,
                repository_root,
                &map,
                &changed,
                &declared,
                &mut reuse,
            )?
        } else {
            // Spec 109 D4: any approved-plan required check covering a root
            // displaces that root's detected checks per change; they stay in
            // the completion floor and run once at the end. Only the check of
            // the step the change belongs to (the in-progress step) stands in
            // for them, and that check is normally deferred to its open step
            // (see `propose_scoped_checks`). A finished step's check is not a
            // per-change check: it is rerun once, when stale, at completion
            // (spec 106 Task 6, below). An unapproved plan narrows nothing, as
            // it gates nothing (spec 105).
            let (plan_checks, current_checks) = match plan.as_ref() {
                Some(plan)
                    if self
                        .session_store
                        .read_plan_approved(session_id, &task.id)? =>
                {
                    (
                        plan_required_checks(repository_root, plan),
                        plan_current_step_checks(repository_root, plan),
                    )
                }
                _ => (Vec::new(), Vec::new()),
            };
            let deferred = |check: &CheckId| {
                plan.as_ref()
                    .is_some_and(|plan| plan_defers_check(repository_root, plan, check))
            };
            let scoped = scoped_selection(&map, &changed, &plan_checks, &current_checks);
            displaced_deferred = scoped
                .plan_checks
                .iter()
                .filter(|check| deferred(check))
                .cloned()
                .collect();
            left_to_completion = detected_checks_of_roots(&map, &scoped.completion_roots);
            // Spec 109 D9: a covered root Damaian detects no check for has the
            // plan's covering checks as its floor, so those are what wait for
            // completion, not nothing.
            for check in plan_floor_of_roots(&map, &scoped.completion_roots, &plan_checks) {
                if !left_to_completion.contains(&check) {
                    left_to_completion.push(check);
                }
            }
            propose_scoped_checks(
                &self.validation_orchestrator,
                repository_root,
                &map,
                &changed,
                &scoped,
                &deferred,
                &mut reuse,
            )?
        };
        // Spec 106 Task 6: a finished step's required check that went stale
        // can only be refreshed by the engine; nothing the model runs on a
        // later step counts for it. It is rerun once, by the completion run
        // (spec 109 D4): mid-plan the gate leaves it to completion, so
        // rerunning it after every later patch would only repeat it.
        let plan_reruns = if at_completion || !due.checks.is_empty() {
            self.completion_gate(repository_root)
                .plan_checks_needing_rerun(session_id, &task.id)?
        } else {
            Vec::new()
        };
        let selection = with_plan_checks(
            &self.validation_orchestrator,
            repository_root,
            selection,
            &plan_reruns,
            &mut reuse,
        )?;
        let reused = reuse.reused().to_vec();
        let browser = self.browser_check(session_id, &session, &task, &changed)?;
        if browser.is_none() {
            return match selection {
                Selection::OnlyDocumentation => self.finish_verification_without_checks(
                    repository_root,
                    session,
                    task,
                    attempt,
                    VerificationOutcome::OnlyDocumentationChanged,
                    &builds,
                    changed,
                    file_hashes,
                ),
                Selection::NoChecksAvailable { .. } => self.finish_verification_without_checks(
                    repository_root,
                    session,
                    task,
                    attempt,
                    VerificationOutcome::NoChecksAvailable,
                    &builds,
                    changed,
                    file_hashes,
                ),
                Selection::Proposed(checks) => self.run_verification_checks(
                    repository_root,
                    session,
                    task,
                    due,
                    attempt,
                    checks,
                    &builds,
                    reused,
                    displaced_deferred,
                    left_to_completion,
                    browser,
                    changed,
                    file_hashes,
                    &scoped_state,
                    model_adapter,
                    sink,
                ),
            };
        }
        // A browser check can stand alone: a web-only change whose root detects
        // no command still has behaviour to verify.
        let checks = match selection {
            Selection::Proposed(checks) => checks,
            Selection::OnlyDocumentation | Selection::NoChecksAvailable { .. } => Vec::new(),
        };
        self.run_verification_checks(
            repository_root,
            session,
            task,
            due,
            attempt,
            checks,
            &builds,
            reused,
            displaced_deferred,
            left_to_completion,
            browser,
            changed,
            file_hashes,
            &scoped_state,
            model_adapter,
            sink,
        )
    }

    /// Builds the browser check for a web-asset change (proposal §5.4).
    ///
    /// `None` when no changed path is a web asset: then there is no browser
    /// check at all, not a skip. With a changed web asset the check is always
    /// selected; only the target and the session's diagnostic approval decide
    /// whether it runs.
    fn browser_check(
        &self,
        session_id: &str,
        session: &Session,
        task: &Task,
        changed: &[String],
    ) -> Result<Option<BrowserCheck>> {
        let web_changed = changed.iter().any(|path| {
            matches!(
                detect_language(path),
                "html" | "css" | "javascript" | "typescript"
            )
        });
        if !web_changed {
            return Ok(None);
        }
        let Some(record) = self
            .session_store
            .read_session_web_diagnostics(session_id)?
            .into_values()
            .flatten()
            .max_by_key(|record| record.recorded_at_ms)
        else {
            return Ok(Some(BrowserCheck::Skipped {
                check: CheckId::browser("the changed page"),
                reason: SkipReason::NoRunningTarget,
            }));
        };
        let check = CheckId::browser(&record.url);
        if !self
            .session_store
            .browser_diagnostics_allowed_for_session(session_id)?
        {
            return Ok(Some(BrowserCheck::Skipped {
                check,
                reason: SkipReason::Blocked {
                    reason: "browser diagnostics are not approved for this session".to_string(),
                },
            }));
        }
        Ok(Some(BrowserCheck::Ready {
            call: WebDiagnosticCall {
                kind: WebDiagnosticKind::Inspect,
                url: record.url.clone(),
                arguments_json: serde_json::json!({"url": record.url}).to_string(),
                session_id: Some(session.id.clone()),
                task_id: Some(task.id.clone()),
            },
            check,
        }))
    }

    /// Continues the verification turn parked on a check card (spec 23 §3).
    ///
    /// `decisions` must answer every proposal in the batch; a missing one is
    /// refused rather than read as a decline, and the pause is put back so the
    /// client can retry.
    pub fn resume_after_check_decision(
        &self,
        batch_id: &str,
        decisions: &[(String, bool)],
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.load(batch_id)?;
        // A failed check's baseline uses the same card and the same resume
        // route (spec 109 D1). It records baselines, then finishes the
        // verification that was waiting to classify its failures.
        if pending.failure_baseline.is_some() {
            return self.resume_failure_baseline(
                batch_id,
                decisions,
                approved_by,
                model_adapter,
                sink,
            );
        }
        let repository_root = PathBuf::from(&pending.repository_root);
        let result =
            self.check_decision_report(batch_id, decisions, approved_by, model_adapter, sink)?;
        self.continue_task_after_verification(&repository_root, result, model_adapter, sink)
    }

    /// Parks a verification on the card for its failed checks' baselines
    /// (spec 109 D1). The checks have run and failed; nothing is classified
    /// yet. The verification resumes once every baseline has a terminal
    /// record, from a decision or a cancel.
    #[allow(clippy::too_many_arguments)]
    fn pause_for_failure_baseline(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        attempt: u32,
        changed: Vec<String>,
        file_hashes: BTreeMap<String, String>,
        batch: BaselineBatch,
    ) -> Result<ChatTurnResult> {
        let batch_id = create_id("checkbatch");
        let mut entries = Vec::new();
        for check in &batch.checks {
            let proposal = self.command_store.load_proposal(&check.proposal_id)?;
            entries.push(agent_command_proposal(&self.config, &proposal));
        }
        let card = AgentCheckBatch {
            id: batch_id.clone(),
            checks: entries,
            title: Some(batch.title.clone()),
            note: Some(batch.note.clone()),
        };
        self.pending_commands.save(&PendingChatTurn {
            proposal_id: batch_id.clone(),
            session: session.clone(),
            task: task.clone(),
            repository_root: repository_root.to_string_lossy().to_string(),
            context_files: changed.clone(),
            round: 0,
            exploration_rounds: 0,
            turn_notes: String::new(),
            turn_notes_carried: false,
            calls: Vec::new(),
            completed_tool_rounds: Vec::new(),
            step_evidence: Vec::new(),
            plan_reminder_sent: false,
            messages: Vec::new(),
            matched_tool_call: None,
            last_content: String::new(),
            turn_options: ChatTurnOptions::default(),
            reasoning_content: None,
            round_calls_fed_back: false,
            mcp_call: None,
            web_diagnostic_call: None,
            plan_review: None,
            provider_fallback: None,
            round_budget: None,
            segment: None,
            question: None,
            check_batch: None,
            failure_baseline: Some(PendingFailureBaseline {
                batch,
                attempt,
                files_changed: changed.clone(),
                file_hashes,
            }),
            fetch_call: None,
            context_review: None,
        })?;
        self.note_pending_approvals(
            &session,
            &task,
            vec![PendingApproval {
                kind: "checks".to_string(),
                proposal_id: batch_id.clone(),
            }],
        );
        let task = self.await_approval(
            &task,
            &PendingApprovalRef {
                kind: "checks".to_string(),
                proposal_id: batch_id.clone(),
            },
        )?;
        self.audit_log.record(
            "failure_baseline_paused",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("proposalId", batch_id.clone()),
                ("checks", card.checks.len().to_string()),
            ],
        )?;
        self.verification_turn_result(
            session,
            task,
            FAILURE_BASELINE_PAUSED_RESPONSE.to_string(),
            Some(card),
            None,
            changed,
        )
    }

    /// Answers the card for a failed check's baseline, runs the approved ones
    /// in the task-start tree under the turn's cancel, and finishes the
    /// verification that was waiting on them.
    fn resume_failure_baseline(
        &self,
        batch_id: &str,
        decisions: &[(String, bool)],
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(batch_id)?;
        self.refuse_unless_paused_for(batch_id, &pending, PausedFor::FailureBaseline)?;
        let Some(failure) = pending.failure_baseline.clone() else {
            unreachable!("the pause was proved to be a failure baseline")
        };
        let repository_root = PathBuf::from(&pending.repository_root);
        // Validate the whole decision set before anything runs; a malformed
        // retry leaves the card intact.
        if let Err(error) =
            self.baselines
                .validate_resume(&repository_root, &failure.batch.id, decisions, &[])
        {
            self.pending_commands.save(&pending)?;
            return Err(error);
        }
        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        let task =
            self.session_store
                .update_task_status(&pending.task, TaskStatus::Validating, None)?;
        sink.session(&pending.session.id);
        let cancel = sink.cancel;
        let on_progress = &mut *sink.on_progress;
        self.baselines.resume_streamed(
            &repository_root,
            &failure.batch.id,
            decisions,
            &[],
            approved_by,
            cancel,
            &mut |progress| on_progress(baseline_turn_progress(progress)),
        )?;
        let result = self.finish_verification(
            &repository_root,
            pending.session,
            task,
            failure.attempt,
            failure.files_changed,
            failure.file_hashes,
            model_adapter,
            sink,
        )?;
        self.continue_task_after_verification(&repository_root, result, model_adapter, sink)
    }

    fn check_decision_report(
        &self,
        batch_id: &str,
        decisions: &[(String, bool)],
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(batch_id)?;
        self.refuse_unless_paused_for(batch_id, &pending, PausedFor::CheckBatch)?;
        if self.session_store.session_mode(&pending.session.id) != SessionMode::Code {
            self.pending_commands.save(&pending)?;
            return Err(ClientError::AccessDenied(
                "Verification runs only in Code mode".to_string(),
            ));
        }
        let Some(batch) = pending.check_batch.clone() else {
            self.pending_commands.save(&pending)?;
            return Err(ClientError::InvalidInput(format!(
                "Proposal {batch_id} is not a check batch"
            )));
        };
        let repository_root = PathBuf::from(&pending.repository_root);
        let session = pending.session.clone();
        let mut task =
            self.session_store
                .update_task_status(&pending.task, TaskStatus::Validating, None)?;
        let mut plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)?
            .unwrap_or_else(|| TaskPlan::new(&task.id, now_millis()));
        let generation = plan.mutation_generation();

        if let Some(stage) = batch.builds.clone() {
            return self.resume_build_stage(
                &pending,
                &batch,
                stage,
                decisions,
                approved_by,
                task,
                generation,
                model_adapter,
                sink,
            );
        }

        for pending_check in &batch.checks {
            let Some((_, approved)) = decisions
                .iter()
                .find(|(proposal_id, _)| proposal_id == &pending_check.proposal_id)
            else {
                self.pending_commands.save(&pending)?;
                return Err(ClientError::InvalidInput(format!(
                    "No decision was given for check {}",
                    pending_check.proposal_id
                )));
            };
            let check = pending_check.check();
            let proposal = self
                .command_store
                .load_proposal(&pending_check.proposal_id)?;
            let outcome = if *approved {
                self.run_verification_check(
                    &repository_root,
                    &session,
                    &task,
                    &check,
                    &proposal,
                    &pending_check.step_id,
                    &mut plan,
                    true,
                    approved_by,
                    &batch.file_hashes,
                    sink,
                )?
            } else {
                // Through the store as well as the log, so the command store
                // and the audit trail agree with the report (proposal §5.3).
                self.validation_orchestrator
                    .reject_proposal(&pending_check.proposal_id, approved_by)?;
                self.close_verification_step(
                    &task,
                    &mut plan,
                    &pending_check.step_id,
                    StepStatus::Skipped,
                )?;
                sink.progress(TurnProgress::Plan(plan.clone()));
                CheckOutcome::Skipped(SkipReason::DeclinedByUser)
            };
            self.session_store.append_verification_event(
                &session.id,
                &VerificationEvent::Check {
                    task_id: task.id.clone(),
                    check,
                    outcome,
                    file_hashes: batch.file_hashes.clone(),
                    generation,
                },
            )?;
        }
        task = self
            .session_store
            .update_task_status(&task, TaskStatus::Validating, None)?;
        self.finish_verification(
            &repository_root,
            session,
            task,
            batch.attempt,
            batch.files_changed,
            batch.file_hashes,
            model_adapter,
            sink,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_verification_checks(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        due: crate::verification::VerificationDue,
        attempt: u32,
        checks: Vec<(CheckId, CommandProposal)>,
        // Build checks that ran (or were reused) before this selection, all
        // of which built (spec 109 D10). Recorded after `Started`.
        builds: &[BuildRecord],
        // Checks not proposed because their recorded run covered exactly the
        // current tree (spec 109 D6). Recorded, never run.
        reused: Vec<CheckId>,
        // Checks standing in for this change that their open plan step runs
        // instead (spec 109 D4); recorded so the report can name them.
        displaced_deferred: Vec<CheckId>,
        // Detected checks a plan check displaces with nothing standing in for
        // them per change; the completion floor runs them (spec 109 D4).
        left_to_completion: Vec<CheckId>,
        browser: Option<BrowserCheck>,
        changed: Vec<String>,
        file_hashes: BTreeMap<String, String>,
        // Verification state scoped to this task's continuation chain, so the
        // "once per batch" filter never suppresses a check for another task.
        scoped_state: &crate::verification::VerificationState,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let mut plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)?
            .unwrap_or_else(|| TaskPlan::new(&task.id, now_millis()));
        let generation = plan.mutation_generation();
        // A required whole-suite check the plan carries on an open step belongs
        // to that plan step: running it after every applied patch is expensive
        // and makes the evidence describe the wrong point in the task. The
        // worker runs it once when it reaches the planned step through its
        // ordinary command and approval flow. Checks the plan did not schedule
        // — smaller, different, or optional ones — remain immediate repair
        // feedback (spec 105 §4).
        //
        // The second filter is requirement 9's "once per mutation batch": a
        // check that already passed at the current generation and whose files
        // still match is not run again merely because another patch landed in
        // the same primary step. A stale or failed run is re-run, so repair
        // feedback stays intact. A plan-less task has no batch to collapse, so
        // its per-patch feedback is unchanged.
        let batch_scoped = plan
            .steps
            .iter()
            .any(|step| !step.id.starts_with("verify_"));
        let mut deferred = displaced_deferred;
        let checks: Vec<_> = checks
            .into_iter()
            .filter(|(check, _)| {
                let defers = plan_defers_check(repository_root, &plan, check);
                if defers && !deferred.contains(check) {
                    deferred.push(check.clone());
                }
                !defers
            })
            .filter(|(check, _)| {
                if !batch_scoped {
                    return true;
                }
                !scoped_state.checks.get(check).is_some_and(|run| {
                    run.outcome.is_passed() && !run.is_stale(&file_hashes, generation)
                })
            })
            .collect();
        let mut selected: Vec<CheckId> = checks.iter().map(|(check, _)| check.clone()).collect();
        if let Some(browser) = &browser {
            selected.push(browser.check().clone());
        }
        self.session_store.append_verification_event(
            &session.id,
            &VerificationEvent::Started {
                task_id: task.id.clone(),
                attempt,
                checks: selected,
            },
        )?;
        self.record_builds(&session, &task, builds)?;
        for check in &reused {
            self.session_store.append_verification_event(
                &session.id,
                &VerificationEvent::Reused {
                    task_id: task.id.clone(),
                    check: check.clone(),
                },
            )?;
            self.audit_log.record(
                "verification_check_reused",
                &[
                    ("actor", "system".to_string()),
                    ("sessionId", session.id.clone()),
                    ("taskId", task.id.clone()),
                    ("command", check.command.clone()),
                    ("workingDirectory", check.working_directory.clone()),
                ],
            )?;
        }
        for check in &deferred {
            let Some(step) = plan_deferring_step(repository_root, &plan, check) else {
                continue;
            };
            self.session_store.append_verification_event(
                &session.id,
                &VerificationEvent::Deferred {
                    task_id: task.id.clone(),
                    check: check.clone(),
                    step: step.title.clone(),
                },
            )?;
        }
        for check in &left_to_completion {
            self.session_store.append_verification_event(
                &session.id,
                &VerificationEvent::DeferredToCompletion {
                    task_id: task.id.clone(),
                    check: check.clone(),
                },
            )?;
        }
        // A rerun (proposal §5.5) reuses the step of a check an earlier attempt
        // already has and adds one only for a check new in this attempt: the
        // step is closed by id, so a second `verify_1` would take the old
        // step's place and leave the new one open (spec 92).
        let mut step_ids = Vec::new();
        for (check, _) in &checks {
            let detail = format!("Verification check: {}", check.label());
            let step_id = if let Some(step) = plan.steps.iter().find(|step| {
                step.id.starts_with("verify_") && step.detail.as_deref() == Some(detail.as_str())
            }) {
                let id = step.id.clone();
                reset_verify_step(&mut plan, &id);
                id
            } else {
                let step_id = next_verify_step_id(&plan);
                plan.steps.push(PlanStep {
                    id: step_id.clone(),
                    title: format!(
                        "Run `{}` in `{}`",
                        check.command,
                        root_label(&check.working_directory)
                    ),
                    detail: Some(detail),
                    status: StepStatus::Pending,
                    depends_on: Vec::new(),
                    started_at_ms: None,
                    completed_at_ms: None,
                    evidence: Vec::new(),
                    check: None,
                    build: None,
                });
                step_id
            };
            step_ids.push(step_id);
        }
        let browser_step = browser.as_ref().map(|browser| {
            let step_id = "verify_browser".to_string();
            let title = format!("Check browser behaviour: {}", browser.check().command);
            match plan.steps.iter_mut().find(|step| step.id == step_id) {
                Some(step) => step.title = title,
                None => plan.steps.push(PlanStep {
                    id: step_id.clone(),
                    title,
                    detail: Some("Browser diagnostics for the changed web assets".to_string()),
                    status: StepStatus::Pending,
                    depends_on: Vec::new(),
                    started_at_ms: None,
                    completed_at_ms: None,
                    evidence: Vec::new(),
                    check: None,
                    build: None,
                }),
            }
            reset_verify_step(&mut plan, &step_id);
            step_id
        });
        self.session_store.revise_plan(&task, &plan)?;
        sink.progress(TurnProgress::Plan(plan.clone()));

        let mut awaiting = Vec::new();
        for ((check, proposal), step_id) in checks.iter().zip(step_ids.iter()) {
            if proposal.blocked {
                let reason = if proposal.reasons.is_empty() {
                    "blocked by command policy".to_string()
                } else {
                    proposal.reasons.join("; ")
                };
                self.close_verification_step(&task, &mut plan, step_id, StepStatus::Skipped)?;
                self.session_store.append_verification_event(
                    &session.id,
                    &VerificationEvent::Check {
                        task_id: task.id.clone(),
                        check: check.clone(),
                        outcome: CheckOutcome::Skipped(SkipReason::Blocked { reason }),
                        file_hashes: file_hashes.clone(),
                        generation,
                    },
                )?;
                sink.progress(TurnProgress::Plan(plan.clone()));
                continue;
            }
            if proposal.requires_approval {
                awaiting.push(PendingCheckDecision {
                    proposal_id: proposal.id.clone(),
                    step_id: step_id.clone(),
                    working_directory: check.working_directory.clone(),
                    command: check.command.clone(),
                });
                continue;
            }
            let outcome = self.run_verification_check(
                repository_root,
                &session,
                &task,
                check,
                proposal,
                step_id,
                &mut plan,
                false,
                "policy",
                &file_hashes,
                sink,
            )?;
            self.session_store.append_verification_event(
                &session.id,
                &VerificationEvent::Check {
                    task_id: task.id.clone(),
                    check: check.clone(),
                    outcome,
                    file_hashes: file_hashes.clone(),
                    generation,
                },
            )?;
        }

        if let (Some(browser), Some(step_id)) = (browser.as_ref(), browser_step.as_deref()) {
            let outcome = self.run_browser_check(
                repository_root,
                &session,
                &task,
                browser,
                step_id,
                &mut plan,
                &file_hashes,
                sink,
            )?;
            self.session_store.append_verification_event(
                &session.id,
                &VerificationEvent::Check {
                    task_id: task.id.clone(),
                    check: browser.check().clone(),
                    outcome,
                    file_hashes: file_hashes.clone(),
                    generation,
                },
            )?;
        }

        if !awaiting.is_empty() {
            return self.pause_for_check_batch(
                repository_root,
                session,
                task,
                due,
                attempt,
                awaiting,
                changed,
                file_hashes,
                None,
            );
        }
        self.finish_verification(
            repository_root,
            session,
            task,
            attempt,
            changed,
            file_hashes,
            model_adapter,
            sink,
        )
    }

    /// Runs one check and records its effect on its plan step.
    ///
    /// The exit is recorded as [`Evidence::CommandExit`] with the purpose the
    /// classification gave it (a detected check is always
    /// [`CommandPurpose::Check`]). Recording it as `Exploration` would leave the
    /// step completed and produce no findings, which is exactly the
    /// laundering spec 63 §5.3 exists to stop.
    #[allow(clippy::too_many_arguments)]
    fn run_verification_check(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        check: &CheckId,
        proposal: &CommandProposal,
        step_id: &str,
        plan: &mut TaskPlan,
        approved: bool,
        approved_by: &str,
        _file_hashes: &BTreeMap<String, String>,
        sink: &mut TurnSink<'_>,
    ) -> Result<CheckOutcome> {
        self.start_verification_step(task, plan, step_id, sink)?;
        sink.phase(
            PhaseKind::Tool,
            running_command_label(&proposal.command),
            0,
            0,
            0,
            0,
            0,
        );
        let cancel = sink.cancel;
        let on_progress = &mut *sink.on_progress;
        let mut on_output = |line: &str| {
            on_progress(TurnProgress::Phase(TurnPhase::new(
                PhaseKind::Output,
                line,
                0,
                0,
                0,
                0,
                0,
            )));
        };
        let record = match self.validation_orchestrator.run_proposal(
            &proposal.id,
            approved,
            approved_by,
            Some(&task.id),
            cancel,
            &mut on_output,
        ) {
            Ok(record) => record,
            Err(error) => {
                // Requirement 5: the command never reached a code result. Only
                // a policy refusal or the process spawn itself is an
                // environment fact for the report. A proposal-store,
                // execution-store, audit, cancellation or other internal
                // failure is not evidence about the environment, so it
                // propagates instead of becoming provenance.
                let Some(termination) = environment_termination(&error) else {
                    return Err(error);
                };
                self.audit_log.record(
                    "verification_check_unavailable",
                    &[
                        ("actor", "system".to_string()),
                        ("sessionId", session.id.clone()),
                        ("taskId", task.id.clone()),
                        ("check", check.label()),
                        ("termination", termination.as_str().to_string()),
                    ],
                )?;
                self.close_verification_step(task, plan, step_id, StepStatus::Blocked)?;
                sink.progress(TurnProgress::Plan(plan.clone()));
                return Ok(CheckOutcome::Environment { termination });
            }
        };
        // Findings and `after_check` both come from here, exactly as they do
        // for a check the model ran. Writing findings through another path
        // would make this the one check no hook sees (proposal §5.3).
        let findings =
            self.record_command_findings(repository_root, session, task, &record.execution)?;
        let exit_code = record.execution.exit_code;
        let marker_id = record.execution.id.clone();
        let termination = CheckTermination::from_command(record.execution.termination, exit_code);
        let mut evidence = vec![Evidence::CommandExit {
            marker_id: marker_id.clone(),
            exit_code,
            purpose: record.execution.purpose,
            command_hash: Some(command_hash(&record.execution.command)),
            working_directory: relative_location(
                repository_root,
                Path::new(&record.execution.working_directory),
            )
            .unwrap_or_else(|| record.execution.working_directory.clone()),
        }];
        if let Some(findings) = findings {
            evidence.push(findings);
        }
        if let Some(step) = plan.steps.iter_mut().find(|step| step.id == step_id) {
            step.evidence = evidence;
            step.status = status_from_evidence(&step.evidence);
            step.completed_at_ms = Some(now_millis());
            self.session_store.update_plan_step(task, step)?;
        }
        sink.progress(TurnProgress::Plan(plan.clone()));
        Ok(CheckOutcome::Ran {
            exit_code,
            marker_id,
            termination: Some(termination),
        })
    }

    /// Runs (or skips) the browser check and records what it saw.
    ///
    /// A diagnostic's verdict becomes the same pass/fail evidence a command's
    /// exit code does, so the plan and the report read one way. A page that
    /// cannot be reached at all is `no_running_target`, never a failure and
    /// never a pass: nothing was observed about the page.
    #[allow(clippy::too_many_arguments)]
    fn run_browser_check(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        browser: &BrowserCheck,
        step_id: &str,
        plan: &mut TaskPlan,
        _file_hashes: &BTreeMap<String, String>,
        sink: &mut TurnSink<'_>,
    ) -> Result<CheckOutcome> {
        self.start_verification_step(task, plan, step_id, sink)?;
        let BrowserCheck::Ready { call, .. } = browser else {
            let BrowserCheck::Skipped { reason, .. } = browser else {
                unreachable!("all browser checks are ready or skipped")
            };
            self.close_verification_step(task, plan, step_id, StepStatus::Skipped)?;
            sink.progress(TurnProgress::Plan(plan.clone()));
            return Ok(CheckOutcome::Skipped(reason.clone()));
        };
        let (_, failed) = self.run_and_record_web_diagnostic(repository_root, call, sink)?;
        if failed {
            self.close_verification_step(task, plan, step_id, StepStatus::Skipped)?;
            sink.progress(TurnProgress::Plan(plan.clone()));
            return Ok(CheckOutcome::Skipped(SkipReason::NoRunningTarget));
        }
        let Some(record) = self
            .session_store
            .read_session_web_diagnostics(&session.id)?
            .get(&task.id)
            .and_then(|records| records.last())
            .cloned()
        else {
            // The runner claimed success but recorded nothing: treat it as no
            // observation rather than inventing a pass.
            self.close_verification_step(task, plan, step_id, StepStatus::Skipped)?;
            sink.progress(TurnProgress::Plan(plan.clone()));
            return Ok(CheckOutcome::Skipped(SkipReason::NoRunningTarget));
        };
        // "Behaviour verified" means the diagnostic ran and recorded no Error
        // finding (proposal §5.4). A page error, a console error and a failed
        // request all become findings, so the findings are what decide rather
        // than the report's summary flag.
        let has_error = self
            .session_store
            .read_findings(&session.id, repository_root)?
            .iter()
            .any(|finding| {
                finding.origin_ref() == Some(record.id.as_str())
                    && finding.severity() == Severity::Error
            });
        let exit_code = if record.report.is_error || has_error {
            1
        } else {
            0
        };
        let evidence = Evidence::CommandExit {
            marker_id: record.id.clone(),
            exit_code: Some(exit_code),
            purpose: CommandPurpose::Check,
            // A browser check is not a command line; each run stands alone.
            command_hash: None,
            working_directory: String::new(),
        };
        if let Some(step) = plan.steps.iter_mut().find(|step| step.id == step_id) {
            step.evidence = vec![evidence];
            step.status = status_from_evidence(&step.evidence);
            step.completed_at_ms = Some(now_millis());
            self.session_store.update_plan_step(task, step)?;
        }
        sink.progress(TurnProgress::Plan(plan.clone()));
        Ok(CheckOutcome::Ran {
            exit_code: Some(exit_code),
            marker_id: record.id,
            termination: Some(CheckTermination::Exited { exit_code }),
        })
    }

    fn close_verification_step(
        &self,
        task: &Task,
        plan: &mut TaskPlan,
        step_id: &str,
        status: StepStatus,
    ) -> Result<()> {
        if let Some(step) = plan.steps.iter_mut().find(|step| step.id == step_id) {
            step.status = status;
            if step.started_at_ms.is_none() {
                step.started_at_ms = Some(now_millis());
            }
            step.completed_at_ms = Some(now_millis());
            self.session_store.update_plan_step(task, step)?;
        }
        Ok(())
    }

    /// Starts a verification substep without claiming two primary plan steps.
    ///
    /// A patch is applied after the proposing turn ended, so its original plan
    /// step can correctly still be open when the automatic verification turn
    /// starts. A newly added `verify_*` step is secondary to that work. It
    /// records its result below, but stays `Pending` while the primary step is
    /// active rather than producing an impossible two-`InProgress` log state.
    fn start_verification_step(
        &self,
        task: &Task,
        plan: &mut TaskPlan,
        step_id: &str,
        sink: &mut TurnSink<'_>,
    ) -> Result<()> {
        let another_primary_is_open = plan
            .steps
            .iter()
            .any(|step| step.id != step_id && step.status == StepStatus::InProgress);
        if let Some(step) = plan.steps.iter_mut().find(|step| step.id == step_id) {
            if !another_primary_is_open {
                step.status = StepStatus::InProgress;
            }
            if step.started_at_ms.is_none() {
                step.started_at_ms = Some(now_millis());
            }
            self.session_store.update_plan_step(task, step)?;
            sink.progress(TurnProgress::Plan(plan.clone()));
        }
        Ok(())
    }

    /// Runs a per-change verification's build checks before anything else
    /// (spec 109 D10).
    ///
    /// Each check in `checks` is taken from `done` (decided on a build card
    /// and run for this same tree), reused when its latest run in the task
    /// chain covered exactly this tree (D6), recorded as not run when the
    /// policy blocks it, or run now. One that needs approval parks the turn on
    /// a build card before anything else is proposed; the resume comes back
    /// here through [`Self::verify_with_task`]. The first failure stops the
    /// stage: nothing after it is proposed.
    #[allow(clippy::too_many_arguments)]
    fn run_build_checks(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        due: &crate::verification::VerificationDue,
        attempt: u32,
        checks: &[CheckId],
        mut done: Vec<BuildRecord>,
        scoped_state: &crate::verification::VerificationState,
        changed: &[String],
        file_hashes: &BTreeMap<String, String>,
        generation: u32,
        sink: &mut TurnSink<'_>,
    ) -> Result<BuildStage> {
        let mut records: Vec<BuildRecord> = Vec::new();
        let mut awaiting = Vec::new();
        for check in checks {
            if records.iter().any(BuildRecord::failed) {
                break;
            }
            // A decided build for a tree that has changed since is built again.
            if let Some(position) = done
                .iter()
                .position(|record| &record.check == check && &record.file_hashes == file_hashes)
            {
                records.push(done.remove(position));
                continue;
            }
            if let Some(previous) =
                crate::verification::reusable_build(scoped_state, check, file_hashes, generation)
            {
                let mut record = previous.clone();
                record.reused = true;
                self.audit_log.record(
                    "verification_build_reused",
                    &[
                        ("actor", "system".to_string()),
                        ("sessionId", session.id.clone()),
                        ("taskId", task.id.clone()),
                        ("command", check.command.clone()),
                        ("workingDirectory", check.working_directory.clone()),
                    ],
                )?;
                records.push(record);
                continue;
            }
            let working_directory = if check.working_directory.is_empty() {
                repository_root.to_path_buf()
            } else {
                repository_root.join(&check.working_directory)
            };
            let proposal = self.validation_orchestrator.propose_build_check(
                repository_root,
                &working_directory,
                &check.command,
            )?;
            if proposal.blocked {
                let reason = if proposal.reasons.is_empty() {
                    "blocked by command policy".to_string()
                } else {
                    proposal.reasons.join("; ")
                };
                records.push(BuildRecord {
                    check: check.clone(),
                    outcome: CheckOutcome::Skipped(SkipReason::Blocked { reason }),
                    file_hashes: file_hashes.clone(),
                    generation,
                    errors: None,
                    reused: false,
                });
                continue;
            }
            if proposal.requires_approval {
                awaiting.push(PendingCheckDecision {
                    proposal_id: proposal.id.clone(),
                    // A build check owns no plan step: it is never evidence.
                    step_id: String::new(),
                    working_directory: check.working_directory.clone(),
                    command: check.command.clone(),
                });
                continue;
            }
            records.push(self.run_build_check(
                repository_root,
                task,
                check,
                &proposal,
                false,
                "policy",
                changed,
                file_hashes,
                generation,
                sink,
            )?);
        }
        if !awaiting.is_empty() && !records.iter().any(BuildRecord::failed) {
            let result = self.pause_for_check_batch(
                repository_root,
                session.clone(),
                task.clone(),
                due.clone(),
                attempt,
                awaiting,
                changed.to_vec(),
                file_hashes.clone(),
                Some(PendingBuildStage { done: records }),
            )?;
            return Ok(BuildStage::Paused(Box::new(result)));
        }
        Ok(BuildStage::Done(records))
    }

    /// Runs one build check and reads the compiler's errors from its output
    /// (spec 109 D10). No plan step, no findings in the log, no `CheckRun`: a
    /// build is feedback for the model, never evidence. The errors come from
    /// the same parsers a check's findings do, so they are secret-redacted
    /// and bounded the same way.
    #[allow(clippy::too_many_arguments)]
    fn run_build_check(
        &self,
        repository_root: &Path,
        task: &Task,
        check: &CheckId,
        proposal: &CommandProposal,
        approved: bool,
        approved_by: &str,
        changed: &[String],
        file_hashes: &BTreeMap<String, String>,
        generation: u32,
        sink: &mut TurnSink<'_>,
    ) -> Result<BuildRecord> {
        sink.phase(
            PhaseKind::Tool,
            running_command_label(&proposal.command),
            0,
            0,
            0,
            0,
            0,
        );
        let record = |outcome: CheckOutcome, errors: Option<BuildErrors>| BuildRecord {
            check: check.clone(),
            outcome,
            file_hashes: file_hashes.clone(),
            generation,
            errors,
            reused: false,
        };
        let cancel = sink.cancel;
        let on_progress = &mut *sink.on_progress;
        let mut on_output = |line: &str| {
            on_progress(TurnProgress::Phase(TurnPhase::new(
                PhaseKind::Output,
                line,
                0,
                0,
                0,
                0,
                0,
            )));
        };
        let run = match self.validation_orchestrator.run_proposal(
            &proposal.id,
            approved,
            approved_by,
            Some(&task.id),
            cancel,
            &mut on_output,
        ) {
            Ok(run) => run,
            Err(error) => {
                let Some(termination) = environment_termination(&error) else {
                    return Err(error);
                };
                return Ok(record(CheckOutcome::Environment { termination }, None));
            }
        };
        let execution = &run.execution;
        let exit_code = execution.exit_code;
        let outcome = CheckOutcome::Ran {
            exit_code,
            marker_id: execution.id.clone(),
            termination: Some(CheckTermination::from_command(
                execution.termination,
                exit_code,
            )),
        };
        let errors = (!outcome.is_passed()).then(|| {
            let findings = findings_from_execution(execution, &default_parsers(), &self.scanner);
            BuildErrors::from_findings(
                repository_root,
                Path::new(&execution.working_directory),
                &findings,
                changed,
            )
        });
        Ok(record(outcome, errors))
    }

    /// Appends a `verification_build` event for each build, after the
    /// verification's `Started` (spec 109 D10).
    fn record_builds(&self, session: &Session, task: &Task, builds: &[BuildRecord]) -> Result<()> {
        for build in builds {
            self.session_store.append_verification_event(
                &session.id,
                &VerificationEvent::build(&task.id, build),
            )?;
            self.audit_log.record(
                "verification_build_recorded",
                &[
                    ("actor", "system".to_string()),
                    ("sessionId", session.id.clone()),
                    ("taskId", task.id.clone()),
                    ("check", build.check.label()),
                    ("result", build.label()),
                    ("reused", build.reused.to_string()),
                ],
            )?;
        }
        Ok(())
    }

    /// Ends a per-change verification whose change does not build (spec 109
    /// D10): nothing else was selected or run, no baseline is taken, and the
    /// outcome is `build_failed`. The continuation turn receives the errors
    /// (`continue_task_after_verification`); no card is shown.
    #[allow(clippy::too_many_arguments)]
    fn finish_build_failed(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        attempt: u32,
        builds: &[BuildRecord],
        changed: Vec<String>,
        file_hashes: BTreeMap<String, String>,
    ) -> Result<ChatTurnResult> {
        self.finish_verification_without_checks(
            repository_root,
            session,
            task,
            attempt,
            VerificationOutcome::BuildFailed,
            builds,
            changed,
            file_hashes,
        )
    }

    /// Answers a build card (spec 109 D10): runs the approved build checks,
    /// records the declined ones as not run, and carries on with the same
    /// verification, which now selects its checks.
    #[allow(clippy::too_many_arguments)]
    fn resume_build_stage(
        &self,
        pending: &PendingChatTurn,
        batch: &PendingCheckBatch,
        stage: PendingBuildStage,
        decisions: &[(String, bool)],
        approved_by: &str,
        task: Task,
        generation: u32,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        // Every build needs an answer before any of them runs.
        for pending_check in &batch.checks {
            if !decisions
                .iter()
                .any(|(proposal_id, _)| proposal_id == &pending_check.proposal_id)
            {
                self.pending_commands.save(pending)?;
                return Err(ClientError::InvalidInput(format!(
                    "No decision was given for check {}",
                    pending_check.proposal_id
                )));
            }
        }
        let repository_root = PathBuf::from(&pending.repository_root);
        let mut done = stage.done;
        for pending_check in &batch.checks {
            let approved = decisions
                .iter()
                .find(|(proposal_id, _)| proposal_id == &pending_check.proposal_id)
                .is_some_and(|(_, approved)| *approved);
            let check = pending_check.check();
            if approved {
                let proposal = self
                    .command_store
                    .load_proposal(&pending_check.proposal_id)?;
                let record = self.run_build_check(
                    &repository_root,
                    &task,
                    &check,
                    &proposal,
                    true,
                    approved_by,
                    &batch.files_changed,
                    &batch.file_hashes,
                    generation,
                    sink,
                )?;
                let failed = record.failed();
                done.push(record);
                if failed {
                    // Nothing after a failed build runs; the rest stay
                    // undecided proposals, as a stopped card's would.
                    break;
                }
            } else {
                self.validation_orchestrator
                    .reject_proposal(&pending_check.proposal_id, approved_by)?;
                done.push(BuildRecord {
                    check,
                    outcome: CheckOutcome::Skipped(SkipReason::DeclinedByUser),
                    file_hashes: batch.file_hashes.clone(),
                    generation,
                    errors: None,
                    reused: false,
                });
            }
        }
        let state = fold_verification(
            self.session_store
                .read_verification_events(&pending.session.id, &[])?
                .iter(),
        );
        let Some(due) = state.due else {
            return Err(ClientError::InvalidInput(
                "No verification is due in this session".to_string(),
            ));
        };
        self.verify_with_task(
            &repository_root,
            pending.session.clone(),
            task,
            due,
            batch.attempt,
            done,
            model_adapter,
            sink,
        )
    }

    /// Parks the verification turn on one card for every check that needs
    /// approval (proposal §5.3).
    #[allow(clippy::too_many_arguments)]
    fn pause_for_check_batch(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        due: crate::verification::VerificationDue,
        attempt: u32,
        awaiting: Vec<PendingCheckDecision>,
        changed: Vec<String>,
        file_hashes: BTreeMap<String, String>,
        build_stage: Option<PendingBuildStage>,
    ) -> Result<ChatTurnResult> {
        let batch_id = create_id("checkbatch");
        let mut entries = Vec::new();
        for pending in &awaiting {
            let proposal = self.command_store.load_proposal(&pending.proposal_id)?;
            entries.push(agent_command_proposal(&self.config, &proposal));
        }
        let is_build = build_stage.is_some();
        let batch = AgentCheckBatch {
            id: batch_id.clone(),
            checks: entries,
            title: is_build.then(|| BUILD_CHECK_CARD_TITLE.to_string()),
            note: is_build.then(|| BUILD_CHECK_CARD_NOTE.to_string()),
        };
        self.pending_commands.save(&PendingChatTurn {
            proposal_id: batch_id.clone(),
            session: session.clone(),
            task: task.clone(),
            repository_root: repository_root.to_string_lossy().to_string(),
            context_files: changed.clone(),
            round: 0,
            exploration_rounds: 0,
            turn_notes: String::new(),
            turn_notes_carried: false,
            calls: Vec::new(),
            completed_tool_rounds: Vec::new(),
            step_evidence: Vec::new(),
            plan_reminder_sent: false,
            messages: Vec::new(),
            matched_tool_call: None,
            last_content: String::new(),
            turn_options: ChatTurnOptions::default(),
            reasoning_content: None,
            round_calls_fed_back: false,
            mcp_call: None,
            web_diagnostic_call: None,
            plan_review: None,
            provider_fallback: None,
            round_budget: None,
            segment: None,
            question: None,
            check_batch: Some(PendingCheckBatch {
                checks: awaiting,
                due_task_id: due.task_id,
                patch_id: due.patch_id,
                attempt,
                files_changed: changed.clone(),
                file_hashes,
                builds: build_stage,
            }),
            failure_baseline: None,
            fetch_call: None,
            context_review: None,
        })?;
        self.note_pending_approvals(
            &session,
            &task,
            vec![PendingApproval {
                kind: "checks".to_string(),
                proposal_id: batch_id.clone(),
            }],
        );
        let task = self.await_approval(
            &task,
            &PendingApprovalRef {
                kind: "checks".to_string(),
                proposal_id: batch_id.clone(),
            },
        )?;
        self.audit_log.record(
            "verification_check_batch_paused",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("proposalId", batch_id.clone()),
                ("checks", batch.checks.len().to_string()),
            ],
        )?;
        self.verification_turn_result(
            session,
            task,
            if is_build {
                BUILD_CHECK_PAUSED_RESPONSE.to_string()
            } else {
                CHECK_BATCH_PAUSED_RESPONSE.to_string()
            },
            Some(batch),
            None,
            changed,
        )
    }

    // The signature mirrors the verification turn's durable inputs. Grouping
    // them into a one-use struct would only move the same fields to every call.
    #[allow(clippy::too_many_arguments)]
    fn finish_verification_without_checks(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        attempt: u32,
        outcome: VerificationOutcome,
        builds: &[BuildRecord],
        changed: Vec<String>,
        file_hashes: BTreeMap<String, String>,
    ) -> Result<ChatTurnResult> {
        self.session_store.append_verification_event(
            &session.id,
            &VerificationEvent::Started {
                task_id: task.id.clone(),
                attempt,
                checks: Vec::new(),
            },
        )?;
        self.record_builds(&session, &task, builds)?;
        self.finish_verification_with_outcome(
            repository_root,
            session,
            task,
            outcome,
            changed,
            file_hashes,
        )
    }

    /// Fingerprint every live post-patch result and record its classification
    /// against the task-local baseline (spec 102 Task 3).
    ///
    /// The classification is durable, so a reload, a worker restart or the
    /// explicit repair continuation reads the same provenance instead of
    /// recomputing it from a baseline it might no longer find. Findings are
    /// taken only by the run's marker id and only while open; the environment
    /// and unknown classes deliberately carry no repair evidence.
    fn record_post_patch_provenance(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        state: &crate::verification::VerificationState,
        plan: &TaskPlan,
    ) -> Result<()> {
        let baselines = self
            .session_store
            .task_verification_baselines(&session.id, &task.id)?;
        let previous = self
            .session_store
            .task_verification_provenance(&session.id, &task.id)?;
        // A failed check takes its baseline just before this classification
        // (spec 109 D1), so a required check reaches here without one only
        // when none was taken: its run was cancelled, or the log predates
        // baselining. It stays `uncertain` rather than guessed. A declined
        // baseline is still `Some(Declined)` and stays unknown, never
        // uncertain.
        let required = plan
            .steps
            .iter()
            .filter(|step| !step.id.starts_with("verify_"))
            .filter_map(|step| step.check.as_ref())
            .filter(|planned| planned.required)
            .map(|planned| {
                CheckId::new(planned.working_directory.as_str(), planned.command.as_str())
            })
            .collect::<std::collections::BTreeSet<_>>();
        let findings = self
            .session_store
            .read_findings(&session.id, repository_root)?;
        for check in &state.selected {
            let Some(run) = state.checks.get(check) else {
                continue;
            };
            if run.outcome.is_passed() {
                continue;
            }
            let Some(termination) = run.outcome.termination() else {
                // A declined check or a missing browser target is not a
                // failure with a provenance.
                continue;
            };
            let evidence = run_evidence(&findings, &run.outcome);
            let result = PostPatchResult::new(check.clone(), termination, evidence);
            let classification = if !baselines.contains_key(check) && required.contains(check) {
                let marker = crate::verification::CheckBaseline::Uncertain;
                classify(&result, Some(&marker), previous.get(check))
            } else {
                classify(&result, baselines.get(check), previous.get(check))
            };
            self.session_store.append_verification_event(
                &session.id,
                &VerificationEvent::Provenance {
                    task_id: task.id.clone(),
                    record: classification.record(),
                },
            )?;
        }
        Ok(())
    }

    /// Decides whether the verification is done, may be repaired, or has hit
    /// the repair limit (spec 23 proposal §5.5).
    ///
    /// A failing check below `agent_tool_retry_limit` starts a repair turn whose
    /// request is the failing check's findings. A repair is a patch, so the
    /// turn ends awaiting review as any patch does; when it settles, Task 3's
    /// `verification_due` makes attempt *n + 1* due. At the limit the task is
    /// `failed` and the report leads with what still fails.
    #[allow(clippy::too_many_arguments)]
    fn finish_verification(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        attempt: u32,
        changed: Vec<String>,
        file_hashes: BTreeMap<String, String>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let events = self
            .session_store
            .read_verification_events(&session.id, &[])?;
        let state = fold_verification(events.iter());
        // Spec 109 D1: a check that failed and has no baseline in this task
        // chain takes one now, on the task-start tree, before anything
        // classifies it. A passing check never does. One that needs approval
        // parks the verification on the baseline card.
        let failed = checks_needing_failure_baseline(
            &state,
            &self
                .session_store
                .read_findings(&session.id, repository_root)?,
        );
        if !failed.is_empty() {
            let cancel = sink.cancel;
            let on_progress = &mut *sink.on_progress;
            let preparation = self.baselines.prepare_for_failures(
                repository_root,
                &session.id,
                &task.id,
                &failed,
                cancel,
                &mut |progress| on_progress(baseline_turn_progress(progress)),
            )?;
            if let BaselinePreparation::Awaiting(batch) = preparation {
                return self.pause_for_failure_baseline(
                    repository_root,
                    session,
                    task,
                    attempt,
                    changed,
                    file_hashes,
                    batch,
                );
            }
        }
        let plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)?
            .unwrap_or_else(|| TaskPlan::new(&task.id, now_millis()));
        // Spec 102 Task 3: fingerprint and classify every live result against
        // the task-local baseline before anything decides whether to repair.
        self.record_post_patch_provenance(repository_root, &session, &task, &state, &plan)?;
        let events = self
            .session_store
            .read_verification_events(&session.id, &[])?;
        let state = fold_verification(events.iter());
        let preliminary = VerificationReport::build(&state, &plan, &changed, &file_hashes);
        if preliminary.failed.is_empty() && preliminary.stale.is_empty() {
            return self.finish_verification_with_outcome(
                repository_root,
                session,
                task,
                // Every check skipped is not a pass (spec 78).
                if preliminary.passed.is_empty() {
                    VerificationOutcome::NothingVerified
                } else {
                    VerificationOutcome::Passed
                },
                changed,
                file_hashes,
            );
        }
        let has_uncertain = preliminary
            .provenance
            .iter()
            .any(|partition| partition.provenance == FailureProvenance::Uncertain);
        if preliminary.repair_targets.is_empty() {
            // Spec 102 D2: an uncertain failure is never silently pre-existing
            // and never an automatic repair. A worker asks the user; the
            // answer decides whether one ordinary repair turn runs. A
            // non-worker session keeps the report and starts no repair.
            if has_uncertain
                && attempt < self.config.agent_tool_retry_limit
                && self.session_store.worker_record(&session.id)?.is_some()
            {
                return self.pause_for_uncertain_repair(
                    repository_root,
                    session,
                    task,
                    &preliminary,
                    &plan,
                    changed,
                );
            }
            // Pre-existing, unchanged, environment and unknown failures are
            // reported, but no code-repair turn is started from evidence the
            // patch did not introduce. When every failure is one the task
            // started with and did not change, the run is not a failure of
            // this task: it settles through the completion gate, which names
            // each such check instead of passing it (spec 106 requirement 11).
            // An environment, unknown or uncertain failure still fails.
            // Staleness is not decided here: this outcome settles through the
            // gate, which reports a stale required check as such.
            let only_pre_existing = !preliminary.failed.is_empty()
                && preliminary.failed.iter().all(|failed| {
                    crate::completion::accepted_pre_existing_failure(&failed.check, &state)
                        .is_some()
                });
            return self.finish_verification_with_outcome(
                repository_root,
                session,
                task,
                if only_pre_existing {
                    VerificationOutcome::OnlyPreExistingFailures
                } else {
                    VerificationOutcome::Failed
                },
                changed,
                file_hashes,
            );
        }
        if attempt >= self.config.agent_tool_retry_limit {
            // The limit is a stop, for every session kind. A worker has no
            // user typing "continue", so spec 96 gives both kinds the same
            // explicit *Continue repair* action instead of one of them
            // silently bypassing the bound. Nothing here starts another
            // attempt on its own.
            return self.finish_verification_with_outcome(
                repository_root,
                session,
                task,
                VerificationOutcome::RepairLimitExhausted,
                changed,
                file_hashes,
            );
        }
        self.run_repair_turn(
            repository_root,
            session,
            task,
            &preliminary,
            &plan,
            changed,
            file_hashes,
            model_adapter,
            sink,
        )
    }

    /// Appends `Finished`, builds the report and closes the task through the
    /// completion gate.
    ///
    /// A successful outcome still has to close the whole task; while plan work
    /// remains the settlement is `incomplete`, never a transient `complete`. A
    /// failed outcome keeps `failed` but carries the gate's ineligible report
    /// with it.
    // The signature mirrors the verification turn's durable inputs. Grouping
    // them into a one-use struct would only move the same fields to every call.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_verification_with_outcome(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        outcome: VerificationOutcome,
        changed: Vec<String>,
        file_hashes: BTreeMap<String, String>,
    ) -> Result<ChatTurnResult> {
        self.session_store.append_verification_event(
            &session.id,
            &VerificationEvent::Finished {
                task_id: task.id.clone(),
                outcome,
            },
        )?;
        let events = self
            .session_store
            .read_verification_events(&session.id, &[])?;
        let state = fold_verification(events.iter());
        let plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)?
            .unwrap_or_else(|| TaskPlan::new(&task.id, now_millis()));
        let report = VerificationReport::build(&state, &plan, &changed, &file_hashes);
        let response = format!("{}\n\n{}", report.summary(&plan.report()), report.render());
        let settlement = match outcome {
            VerificationOutcome::Passed
            | VerificationOutcome::NothingVerified
            | VerificationOutcome::NoChecksAvailable
            | VerificationOutcome::OnlyDocumentationChanged
            | VerificationOutcome::OnlyPreExistingFailures
            // A change that does not build goes back to the model (spec 109
            // D10). The gate cannot pass it: nothing verified this tree.
            | VerificationOutcome::BuildFailed => CompletionSettlement::Gate,
            VerificationOutcome::Failed
            | VerificationOutcome::RepairLimitExhausted
            | VerificationOutcome::RepairStalled
            | VerificationOutcome::NoRepairApplied => CompletionSettlement::VerificationFailed,
        };
        let task = self
            .completion_gate(repository_root)
            .settle(&task, settlement)?
            .task;
        self.verification_turn_result(session, task, response, None, Some(report), changed)
    }

    /// The required check `step` declares, when nothing has answered it yet.
    ///
    /// Answered means a run or a decline of exactly that command in exactly
    /// that directory on the step (including what this turn observed and has
    /// not attached yet), or a verification-loop run of the same identity in
    /// the task's chain. Stale evidence still answers it: a rerun is the
    /// engine's job once the step has closed (spec 106 Task 6).
    pub(super) fn unrun_required_check(
        &self,
        session_id: &str,
        task_id: &str,
        step: &PlanStep,
        pending_evidence: &[crate::plan::Evidence],
    ) -> Result<Option<CheckId>> {
        let Some(planned) = step.check.as_ref().filter(|planned| planned.required) else {
            return Ok(None);
        };
        let check = CheckId::new(planned.working_directory.as_str(), planned.command.as_str());
        if step
            .evidence
            .iter()
            .chain(pending_evidence)
            .any(|evidence| crate::completion::evidence_answers_check(evidence, &check))
        {
            return Ok(None);
        }
        let chain = self
            .session_store
            .task_continuation_chain(session_id, task_id)?;
        let ids = chain.iter().map(String::as_str).collect::<Vec<_>>();
        let verified = self
            .session_store
            .read_verification_events(session_id, &ids)?
            .iter()
            .any(|event| {
                matches!(event, VerificationEvent::Check { check: ran, .. } if ran == &check)
            });
        Ok((!verified).then_some(check))
    }

    /// Continues the task a verification interrupted (spec 77).
    ///
    /// A patch ends the turn that proposed it, and the verification after the
    /// apply used to end the task with its report, so a plan of nine steps
    /// stopped after the first patch. When the verification did not fail and
    /// the carried plan still has open steps of the task's own, the model is
    /// asked to carry on in the same turn. Its next patch ends that turn the
    /// usual way, and the next verification comes back here. A failed
    /// verification is not continued: repair and the report own that case.
    fn continue_task_after_verification(
        &self,
        repository_root: &Path,
        result: ChatTurnResult,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let Some(report) = result.verification_report.clone() else {
            return Ok(result);
        };
        if result.cancelled {
            return Ok(result);
        }
        // A successful verification settles the task, but only a whole closed
        // task becomes `complete` (spec 101). An `incomplete` settlement may
        // still be continued when the sole remaining blocker is the task's own
        // open non-verification plan work. Any failed, stale, missing, pending
        // or policy-skipped evidence keeps it stopped, which is why the gate's
        // exact reasons are checked rather than just the status.
        let session = result.session.clone();
        let plan = self
            .session_store
            .read_task_plan(&session.id, &result.task.id)?;
        let reason_codes = match result.task.status {
            TaskStatus::Complete => Some(Vec::new()),
            TaskStatus::Incomplete => self
                .session_store
                .read_completion_report(&session.id, &result.task.id)?
                .map(|settled| settled.reason_codes()),
            _ => None,
        };
        let Some(reason_codes) = reason_codes else {
            return Ok(result);
        };
        let original_request = self
            .verification_origin_prompt(&session.id)?
            .map(|prompt| format!("The request was:\n\n{prompt}\n\n"))
            .unwrap_or_default();
        let prompt = match plan.as_ref().filter(|plan| has_primary_steps(plan)) {
            Some(plan) => {
                let may_continue = reason_codes.is_empty()
                    || reason_codes == vec![CompletionReasonCode::OpenPlanSteps];
                if !may_continue {
                    return Ok(result);
                }
                let open: Vec<&PlanStep> = plan
                    .steps
                    .iter()
                    .filter(|step| !step.id.starts_with("verify_") && !step.status.is_terminal())
                    .collect();
                if open.is_empty() {
                    return Ok(result);
                }
                let steps = open
                    .iter()
                    .map(|step| format!("- {} ({})", step.title, step.id))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "{CONTINUE_AFTER_VERIFICATION_PROMPT}\n\n{original_request}Verification: {}\n\nOpen plan steps:\n{steps}",
                    report.summary(&plan.report()),
                )
            }
            // Spec 107 D3: a planless task does not end at its verified patch.
            // Its per-change run cannot satisfy the completion floor's
            // enclosing roots, so a missing or stale floor check is expected
            // here and is what the answer-time run settles; anything else
            // (a failed, skipped or pending check) keeps the task stopped. A
            // completion run — the answer-time floor — is the end and is never
            // continued, or the task would loop on its own answer.
            None => {
                let only_floor_left = reason_codes.iter().all(|code| {
                    matches!(
                        code,
                        CompletionReasonCode::RequiredCheckMissing
                            | CompletionReasonCode::RequiredCheckStale
                    )
                });
                let verified = !matches!(
                    report.outcome,
                    Some(
                        VerificationOutcome::Failed
                            | VerificationOutcome::RepairLimitExhausted
                            | VerificationOutcome::RepairStalled
                            | VerificationOutcome::NoRepairApplied
                    )
                );
                if !only_floor_left
                    || !verified
                    || !self
                        .session_store
                        .planless_continues(&session.id, &result.task.id)?
                    || self.verification_was_completion_run(&session.id, &result.task.id)?
                {
                    return Ok(result);
                }
                let empty = TaskPlan::new(&result.task.id, 0);
                format!(
                    "{CONTINUE_PLANLESS_AFTER_VERIFICATION_PROMPT}\n\n{original_request}Verification: {}",
                    report.summary(&empty.report()),
                )
            }
        };
        // Spec 109 D10: a change that does not build comes back to the model
        // in this same turn with the compiler's errors, redacted and bounded.
        let prompt = match report.build_failure_prompt() {
            Some(errors) => format!("{prompt}\n\n{errors}"),
            None => prompt,
        };
        let task = result.task.clone();
        let changed = report.files_changed.clone();
        let index = crate::index_cache::IndexCache::get_or_build(&self.indexer, repository_root)?;
        let conversation = self.session_store.read_conversation(&session.id)?;
        let context_pins = self
            .session_store
            .context_pins(&session.id)
            .unwrap_or_default();
        let context_restriction = self.session_store.context_restriction(&session.id)?;
        let mut context = self.context_manager.build_context(ContextRequest {
            repository_root,
            repository_id: &index.repository_id,
            task_id: &task.id,
            prompt: &prompt,
            index: Some(&index),
            explicit_paths: &changed,
            limit: self.config.context_token_budget(),
            window: self.config.context_window(),
            hook_context: &[],
            plan: plan.as_ref(),
            pinned: &context_pins,
            restriction: context_restriction.as_deref(),
            conversation_summary: conversation.summary.as_ref(),
            // Like a repair continuation: the original turn's references and
            // recalled memory were sent with that request (spec 51 D5,
            // spec 30 Task 4).
            external_references: &[],
            memory: &[],
        })?;
        let turn_notes = self.carry_notes_from_the_previous_turn(&session.id, &task)?;
        let prompt_conversation = build_conversation(
            &conversation.messages,
            conversation.summary.as_ref(),
            self.conversation_window_tokens(),
            &self.scanner,
        );
        let offer_handoffs = self.config.enable_worker_sessions
            && self.session_store.worker_record(&session.id)?.is_none();
        let options = engine_started_turn_options(&self.session_store, &session.id);
        let messages = vec![
            ModelMessage::system(system_prompt_for_session(
                SessionMode::Code,
                &options,
                offer_handoffs,
                &self.config.shell,
            )),
            ModelMessage::user(build_model_prompt_with_conversation(
                &context,
                &prompt_conversation,
                None,
            )),
        ];
        note_turn_overhead(&mut context.manifest, &prompt_conversation, &messages);
        if self.config.pause_for_context_review {
            return self.pause_for_context_review(
                repository_root,
                session,
                task,
                context,
                prompt_conversation,
                conversation.summary.clone(),
                Vec::new(),
                messages,
                turn_notes,
                options,
            );
        }
        let mut fields = vec![
            ("actor", "system".to_string()),
            ("sessionId", session.id.clone()),
        ];
        fields.extend(context.manifest.audit_fields());
        self.audit_log.record("context_assembled", &fields)?;
        sink.context(&context.manifest);
        let task =
            self.session_store
                .update_task_status(&task, TaskStatus::PreparingContext, None)?;
        let mut continued = self.run_agentic_turn(
            repository_root,
            session,
            task,
            context.files,
            messages,
            0,
            0,
            turn_notes,
            CompletedToolTracker::default(),
            Vec::new(),
            false,
            model_adapter,
            sink,
            options,
            None,
        )?;
        // The report stays with the turn, so the card the user would have seen
        // is still drawn above the continuation's answer.
        continued.verification_report = Some(report);
        Ok(continued)
    }

    /// Whether `task_id`'s verification was the answer-time completion run:
    /// the `verification_due` that started it named its checks (review
    /// follow-up to R16). Read from the log so a resumed check card answers
    /// the same way as a run that never paused.
    fn verification_was_completion_run(&self, session_id: &str, task_id: &str) -> Result<bool> {
        let events = self
            .session_store
            .read_verification_events(session_id, &[])?;
        let mut due_named_checks = false;
        let mut completion = false;
        for event in &events {
            match event {
                VerificationEvent::Due { checks, .. } => due_named_checks = !checks.is_empty(),
                VerificationEvent::Started {
                    task_id: started, ..
                } if started == task_id => completion = due_named_checks,
                _ => {}
            }
        }
        Ok(completion)
    }

    /// The latest request the user typed in this session. A continuation runs
    /// inside the verification task, whose prompt is Damaian's, so the user's
    /// own words are found by skipping those.
    pub(super) fn verification_origin_prompt(&self, session_id: &str) -> Result<Option<String>> {
        Ok(self
            .session_store
            .read_tasks(session_id)?
            .into_iter()
            .rev()
            .find(|task| {
                // A repair's prompt starts with the verification prompt.
                !task.user_prompt.starts_with(VERIFICATION_TURN_PROMPT)
            })
            .map(|task| task.user_prompt))
    }

    /// Closes a repair turn that paused and later ended with a model answer
    /// (spec 23 proposal §5.5).
    ///
    /// The pause's resume path re-enters `run_agentic_turn` outside
    /// [`Self::run_repair_turn`], so this is where its answer is judged: a
    /// patch ends the turn as any patch does, another pause waits, and an
    /// answer with no patch is `no_repair_applied` rather than a success.
    pub(super) fn close_verification_resume(
        &self,
        repository_root: &Path,
        result: ChatTurnResult,
    ) -> Result<ChatTurnResult> {
        let paused = result.patch_proposal.is_some()
            || result.command_proposal.is_some()
            || result.plan_proposal.is_some()
            || result.question_proposal.is_some()
            || result.fallback_proposal.is_some()
            || result.round_budget_proposal.is_some()
            || result.check_batch.is_some()
            || result.cancelled;
        let events = self
            .session_store
            .read_verification_events(&result.session.id, &[])?;
        let state = fold_verification(events.iter());
        // A paused repair turn resumes outside `run_repair_turn`, so its patch
        // proposal reaches Task 4's progress check here instead. The same
        // durable decision and the same typed report are required on this path.
        if result.patch_proposal.is_some() {
            let changed = self.verification_changed_files(
                &result.session.id,
                &result.task.id,
                state.due.as_ref(),
            )?;
            let file_hashes = hash_existing_files(repository_root, &changed);
            let plan = self
                .session_store
                .read_task_plan(&result.session.id, &result.task.id)?
                .unwrap_or_else(|| TaskPlan::new(&result.task.id, now_millis()));
            let report = VerificationReport::build(&state, &plan, &changed, &file_hashes);
            // The paused repair proposal already carries the same engine-owned
            // scope review that `run_agentic_turn` attached before its first
            // save; only the progress decision remains for this path.
            let proposal = result
                .patch_proposal
                .clone()
                .expect("a proposal was checked above");
            if self.record_repair_state_or_stall(
                repository_root,
                &result.session.id,
                &result.task.id,
                &report,
                &changed,
                &proposal,
            )? {
                return self.finish_verification_stalled(
                    repository_root,
                    result.session,
                    result.task,
                    changed,
                    file_hashes,
                );
            }
            return Ok(result);
        }
        if paused {
            return Ok(result);
        }
        // Spec 103 Task 6: a bounded stop already settled a typed terminal
        // outcome. It is not a "no repair applied" verification result, and the
        // later settlement must not overwrite its status or outcome.
        if result.task_control.terminal_outcome.is_some() {
            return Ok(result);
        }
        let Some(due) = state.due.clone() else {
            return Ok(result);
        };
        if state.selected.is_empty() || state.finished.is_some() {
            return Ok(result);
        }
        let changed =
            self.verification_changed_files(&result.session.id, &result.task.id, Some(&due))?;
        let file_hashes = hash_existing_files(repository_root, &changed);
        self.finish_verification_with_outcome(
            repository_root,
            result.session,
            result.task,
            VerificationOutcome::NoRepairApplied,
            changed,
            file_hashes,
        )
    }

    /// The files the task changed, from the plan's applied patches and the due
    /// patch's applied decisions, falling back to the stored patch's files.
    pub(super) fn verification_changed_files(
        &self,
        session_id: &str,
        task_id: &str,
        due: Option<&crate::verification::VerificationDue>,
    ) -> Result<Vec<String>> {
        let mut changed = BTreeSet::new();
        if let Some(plan) = self.session_store.read_task_plan(session_id, task_id)? {
            for step in &plan.steps {
                for evidence in &step.evidence {
                    if let Evidence::PatchApplied { files, .. } = evidence {
                        for file in files {
                            changed.insert(file.path.clone());
                        }
                    }
                }
            }
        }
        // Every applied patch in the conversation, not only the one that made
        // this attempt due: a repair changes files, and the rerun set is every
        // check selected so far plus whatever the repair's files select
        // (spec 23 proposal §5.5). Reading only `due.task_id` would lose the
        // files an earlier patch applied once the plan step that carried the
        // evidence had already closed.
        for decision in self
            .session_store
            .read_all_patch_file_decisions(session_id)?
        {
            if decision.applied {
                for path in decision.paths {
                    changed.insert(path);
                }
            }
        }
        // Files approved commands changed (spec 85), for the same reason every
        // applied patch counts: the run checks the conversation's changes.
        for event in self
            .session_store
            .read_verification_events(session_id, &[])?
        {
            if let VerificationEvent::Due { paths, .. } = event {
                changed.extend(paths);
            }
        }
        if changed.is_empty()
            && let Some(due) = due
            && let Ok(patch) = self.patch_store.load(&due.patch_id)
        {
            for file in patch.files {
                changed.insert(file.path);
            }
        }
        Ok(changed.into_iter().collect())
    }

    pub(super) fn verification_turn_result(
        &self,
        session: Session,
        task: Task,
        response: String,
        check_batch: Option<AgentCheckBatch>,
        report: Option<VerificationReport>,
        context_files: Vec<String>,
    ) -> Result<ChatTurnResult> {
        if !response.is_empty() {
            self.session_store.append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                &response,
            )?;
        }
        let usage = self
            .session_store
            .read_task_usage(&session.id)
            .ok()
            .and_then(|usage| usage.get(&task.id).cloned());
        let estimated_cost = usage.as_ref().and_then(|usage| {
            self.config.estimated_cost(&TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                source: usage.source,
            })
        });
        let mut model_run =
            ModelRun::cancelled_before_start(&self.config.model_provider, &self.config.model_name);
        model_run.content = response.clone();
        let task_control = self.task_control_for(&task);
        let turn_presentation = self.turn_presentation_for(&task, false, false, None);
        Ok(ChatTurnResult {
            session,
            task,
            model_run,
            context_files,
            response,
            command_proposal: None,
            patch_proposal: None,
            plan_proposal: None,
            question_proposal: None,
            fetch_proposal: None,
            fallback_proposal: None,
            round_budget_proposal: None,
            check_batch,
            verification_report: report,
            turn_notes: None,
            cancelled: false,
            usage,
            estimated_cost,
            context_review: None,
            task_control,
            turn_presentation,
        })
    }
}

/// The selected checks of the latest verification that failed with a code
/// result: what a failure-time baseline is for (spec 109 D1). A passed,
/// skipped or declined check, an environment failure, a run the user
/// cancelled and a browser check (which has no command to rerun) take none.
/// Each comes with its run's open findings, which decide whether only the
/// failing tests rerun (spec 109 D3).
fn checks_needing_failure_baseline(
    state: &crate::verification::VerificationState,
    findings: &[crate::finding::Finding],
) -> Vec<crate::baseline::FailedCheck> {
    state
        .selected
        .iter()
        .filter(|check| !check.is_browser())
        .filter_map(|check| {
            let run = state.checks.get(check).filter(|run| {
                matches!(run.outcome, CheckOutcome::Ran { .. })
                    && !run.outcome.is_passed()
                    && run.outcome.termination().is_some_and(|termination| {
                        !termination.is_environment_failure()
                            && termination != CheckTermination::Cancelled
                    })
            })?;
            Some(crate::baseline::FailedCheck {
                check: check.clone(),
                findings: run_evidence(findings, &run.outcome),
            })
        })
        .collect()
}

/// The open findings a check run produced, as provenance compares them.
fn run_evidence(
    findings: &[crate::finding::Finding],
    outcome: &CheckOutcome,
) -> Vec<FindingEvidence> {
    match outcome {
        CheckOutcome::Ran { marker_id, .. } => findings
            .iter()
            .filter(|finding| {
                finding.origin_ref() == Some(marker_id.as_str())
                    && finding.status() == FindingStatus::Open
            })
            .map(FindingEvidence::from_finding)
            .collect(),
        _ => Vec::new(),
    }
}

/// A baseline's progress as the turn's status line shows it: the running
/// check, its output, and how it ended.
fn baseline_turn_progress(progress: crate::baseline::BaselineProgress) -> TurnProgress {
    let (kind, label) = match progress {
        crate::baseline::BaselineProgress::Started { label, .. } => (PhaseKind::Tool, label),
        crate::baseline::BaselineProgress::Output { line } => (PhaseKind::Output, line),
        crate::baseline::BaselineProgress::Finished { label, .. } => (PhaseKind::Tool, label),
    };
    TurnProgress::Phase(TurnPhase::new(kind, label, 0, 0, 0, 0, 0))
}

/// A check's root as the report names it.
/// The first `verify_N` the plan does not use yet, so a check new in a rerun
/// never shares an id with an earlier attempt's step.
fn next_verify_step_id(plan: &TaskPlan) -> String {
    let used = plan
        .steps
        .iter()
        .filter_map(|step| step.id.strip_prefix("verify_")?.parse::<usize>().ok())
        .max()
        .unwrap_or(0);
    format!("verify_{}", used + 1)
}

/// Opens a reused verification step for this attempt. Its evidence goes too:
/// the repair request reads findings from every `verify_` step, and an earlier
/// attempt's are what the repair already answered.
fn reset_verify_step(plan: &mut TaskPlan, step_id: &str) {
    if let Some(step) = plan.steps.iter_mut().find(|step| step.id == step_id) {
        step.status = StepStatus::Pending;
        step.started_at_ms = None;
        step.completed_at_ms = None;
        step.evidence.clear();
    }
}

/// Whether an open (non-terminal, non-`verify_`) plan step carries a `required`
/// structured check whose identity is `check` (spec 105 §4).
///
/// The match is the full identity — command *and* working directory — in the
/// same normalized directory form a discovered `CheckId` uses. A plan stored
/// before the input normalization, or written by a caller that did not go
/// through it, still compares correctly. Only a `required: true` check is a
/// deferral target; an optional check is not. Historical prose-only plans carry
/// no `PlannedCheck`, so they defer nothing (requirement 10): a command named in
/// a step's text is no longer a signal.
pub(super) fn plan_defers_check(repository_root: &Path, plan: &TaskPlan, check: &CheckId) -> bool {
    plan_deferring_step(repository_root, plan, check).is_some()
}

/// The open plan step [`plan_defers_check`] defers `check` to.
fn plan_deferring_step<'a>(
    repository_root: &Path,
    plan: &'a TaskPlan,
    check: &CheckId,
) -> Option<&'a PlanStep> {
    plan.steps.iter().find(|step| {
        if step.id.starts_with("verify_") || step.status.is_terminal() {
            return false;
        }
        let Some(planned) = step.check.as_ref() else {
            return false;
        };
        if !planned.required {
            return false;
        }
        let Some(directory) =
            normalize_planned_check_directory(repository_root, &planned.working_directory)
        else {
            return false;
        };
        CheckId::new(directory, planned.command.as_str()) == *check
    })
}

/// The in-progress primary step's build check (spec 109 D10), its directory in
/// `CheckId` form; `None` when the step has none or its directory leaves the
/// repository. The caller passes only an approved plan, as for D4's checks.
fn plan_current_step_build(repository_root: &Path, plan: &TaskPlan) -> Option<CheckId> {
    let planned = plan
        .steps
        .iter()
        .filter(|step| !step.id.starts_with("verify_") && step.status == StepStatus::InProgress)
        .find_map(|step| step.build.as_ref())?;
    let directory = normalize_planned_check_directory(repository_root, &planned.working_directory)?;
    Some(CheckId::new(directory, planned.command.as_str()))
}

/// The `required` checks of the plan's in-progress primary steps — the step a
/// change belongs to — in `CheckId` form (spec 109 D4). Per change only these
/// stand in for a covered root's detected checks.
fn plan_current_step_checks(repository_root: &Path, plan: &TaskPlan) -> Vec<CheckId> {
    let mut checks = Vec::new();
    for planned in plan
        .steps
        .iter()
        .filter(|step| !step.id.starts_with("verify_") && step.status == StepStatus::InProgress)
        .filter_map(|step| step.check.as_ref())
        .filter(|planned| planned.required)
    {
        let Some(directory) =
            normalize_planned_check_directory(repository_root, &planned.working_directory)
        else {
            continue;
        };
        let check = CheckId::new(directory, planned.command.as_str());
        if !checks.contains(&check) {
            checks.push(check);
        }
    }
    checks
}

/// The plan's `required` structured checks (non-`verify_` steps, any status),
/// in plan order and de-duplicated, with their directories in `CheckId` form
/// (spec 109 D4). A check whose directory leaves the repository is dropped,
/// as `plan_defers_check` ignores it.
fn plan_required_checks(repository_root: &Path, plan: &TaskPlan) -> Vec<CheckId> {
    let mut checks = Vec::new();
    for planned in plan
        .steps
        .iter()
        .filter(|step| !step.id.starts_with("verify_"))
        .filter_map(|step| step.check.as_ref())
        .filter(|planned| planned.required)
    {
        let Some(directory) =
            normalize_planned_check_directory(repository_root, &planned.working_directory)
        else {
            continue;
        };
        let check = CheckId::new(directory, planned.command.as_str());
        if !checks.contains(&check) {
            checks.push(check);
        }
    }
    checks
}
