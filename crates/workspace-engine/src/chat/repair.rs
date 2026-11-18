use super::*;

impl ChatOrchestrator {
    /// Parks a worker verification on the engine's own question: the required
    /// check has no task-start baseline, so Damaian reports the failure and
    /// lets the user decide whether one repair turn should run (spec 102 D2).
    ///
    /// The question is a durable question, not an approval. The answer cannot
    /// approve a patch or command, and a repair patch it produces still goes
    /// through its ordinary approval.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn pause_for_uncertain_repair(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        report: &VerificationReport,
        plan: &TaskPlan,
        changed: Vec<String>,
    ) -> Result<ChatTurnResult> {
        let question = PendingQuestionRef::new();
        let checks = report
            .provenance
            .iter()
            .filter(|partition| partition.provenance == FailureProvenance::Uncertain)
            .flat_map(|partition| partition.checks.iter())
            .map(|check| check.label.clone())
            .collect::<Vec<_>>();
        let input = AskUserInput {
            question: format!(
                "The required {} became required after this task had already changed files, so there is no task-start baseline. Damaian cannot tell whether this task caused the failure. Do you want one repair turn for it?",
                checks.join(", ")
            ),
            options: vec!["Repair it".to_string(), "Leave it unrepaired".to_string()],
            allow_free_text: false,
        };
        self.session_store
            .record_question_asked(&session.id, &task.id, &question, &input)?;
        let response = format!("{}\n\n{}", report.summary(&plan.report()), report.render());
        self.pending_commands.save(&PendingChatTurn {
            proposal_id: question.question_id().to_string(),
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
            question: Some(PendingQuestion {
                question_id: question.question_id().to_string(),
                kind: PendingQuestionKind::RepairDecision,
            }),
            check_batch: None,
            failure_baseline: None,
            fetch_call: None,
            context_review: None,
        })?;
        self.note_pending_approvals(&session, &task, Vec::new());
        let task = self.session_store.await_answer(&task, &question)?;
        self.audit_log.record(
            "verification_uncertain_repair_question",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("questionId", question.question_id().to_string()),
            ],
        )?;
        let mut result = self.verification_turn_result(
            session,
            task,
            response,
            None,
            Some(report.clone()),
            changed,
        )?;
        result.question_proposal = Some(AgentQuestionProposal {
            question_id: question.question_id().to_string(),
            question: input.question,
            options: input.options,
            allow_free_text: input.allow_free_text,
        });
        Ok(result)
    }

    /// Answers the engine's uncertain-repair question: one ordinary repair
    /// turn for the uncertain evidence, or the same failure reported without a
    /// repair. Neither choice approves a patch or a command.
    pub(super) fn resume_repair_decision(
        &self,
        pending: &PendingChatTurn,
        answer: &str,
        selected_option: Option<usize>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let repository_root = PathBuf::from(&pending.repository_root);
        let session = pending.session.clone();
        let task = pending.task.clone();
        let events = self
            .session_store
            .read_verification_events(&session.id, &[])?;
        let state = fold_verification(events.iter());
        let plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)?
            .unwrap_or_else(|| TaskPlan::new(&task.id, now_millis()));
        let changed = self.verification_changed_files(&session.id, &task.id, state.due.as_ref())?;
        let file_hashes = hash_existing_files(&repository_root, &changed);
        let mut report = VerificationReport::build(&state, &plan, &changed, &file_hashes);
        let repair = selected_option == Some(0) || answer == "Repair it";
        if !repair {
            return self.finish_verification_with_outcome(
                &repository_root,
                session,
                task,
                VerificationOutcome::Failed,
                changed,
                file_hashes,
            );
        }
        let targets = uncertain_repair_targets(&report);
        if targets.is_empty() {
            return Err(ClientError::InvalidInput(
                "The uncertain failure is no longer available to repair".to_string(),
            ));
        }
        report.repair_targets = targets;
        self.audit_log.record(
            "verification_uncertain_repair_approved",
            &[
                ("actor", "user".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
            ],
        )?;
        self.run_repair_turn(
            &repository_root,
            session,
            task,
            &report,
            &plan,
            changed,
            file_hashes,
            model_adapter,
            sink,
        )
    }

    /// Starts exactly one more repair attempt after the repair limit, as an
    /// explicit user decision (spec 96).
    ///
    /// The verification that reached the limit is the latest task, and its
    /// `repair_limit_exhausted` report is still in the log. This creates a new
    /// task carrying that plan and its failure evidence, then runs the ordinary
    /// repair turn once. It never loops: a repair that proposes no patch closes
    /// as `no_repair_applied`, and a repair that is patch-applied leads to the
    /// next verification, which stops at the limit again if it still fails.
    ///
    /// A second call while that repair is pending is refused: the limit's
    /// task is no longer the latest one. So is a call when the last
    /// verification did not reach the limit.
    pub fn continue_repair(
        &self,
        repository_root: impl AsRef<Path>,
        session_id: &str,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let repository_root = repository_root.as_ref();
        let Some(session) = self.session_store.read_session(session_id)? else {
            return Err(ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        };
        if self.session_store.session_mode(session_id) != SessionMode::Code {
            return Err(ClientError::AccessDenied(
                "Repair continuation runs only in Code mode".to_string(),
            ));
        }
        let events = self
            .session_store
            .read_verification_events(session_id, &[])?;
        let state = fold_verification(events.iter());
        if state.finished != Some(VerificationOutcome::RepairLimitExhausted) || state.due.is_some()
        {
            return Err(ClientError::InvalidInput(
                "The last verification did not reach the repair limit, or another verification is due"
                    .to_string(),
            ));
        }
        let Some(limited_task_id) = events.iter().rev().find_map(|event| match event {
            VerificationEvent::Finished { task_id, .. } => Some(task_id.clone()),
            _ => None,
        }) else {
            return Err(ClientError::InvalidInput(
                "The last verification did not reach the repair limit".to_string(),
            ));
        };
        // One explicit action, one attempt. Once the new task exists it is the
        // latest task, so another click is refused until a verification closes
        // this one. This is the guard that keeps the action from becoming an
        // unbounded retry loop.
        let latest = self
            .session_store
            .read_tasks(session_id)?
            .into_iter()
            .last();
        if !latest
            .is_some_and(|task| task.id == limited_task_id && task.status == TaskStatus::Failed)
        {
            return Err(ClientError::InvalidInput(
                "The repair limit's verification is not the latest task; a repair is already in progress".to_string(),
            ));
        }
        let Some(limited_plan) = self
            .session_store
            .read_task_plan(session_id, &limited_task_id)?
        else {
            return Err(ClientError::InvalidInput(
                "The verification that reached the repair limit carries no plan".to_string(),
            ));
        };
        let changed = self.verification_changed_files(session_id, &limited_task_id, None)?;
        let file_hashes = hash_existing_files(repository_root, &changed);
        let report = VerificationReport::build(&state, &limited_plan, &changed, &file_hashes);
        if report.repair_targets.is_empty() {
            return Err(ClientError::InvalidInput(
                "The verification has no introduced failure evidence to repair".to_string(),
            ));
        }

        let mut task = self.session_store.create_task(
            session_id,
            VERIFICATION_TURN_PROMPT,
            &self.config.model_provider,
            &self.config.model_name,
        )?;
        task = self
            .session_store
            .update_task_status(&task, TaskStatus::PreparingContext, None)?;
        // Step state and evidence come across verbatim, so the repair request
        // is built from the checks that actually failed.
        self.session_store.resume_plan(&task, &limited_task_id)?;
        task = self
            .session_store
            .update_task_status(&task, TaskStatus::Validating, None)?;
        let plan = self
            .session_store
            .read_task_plan(session_id, &task.id)?
            .unwrap_or(limited_plan);
        self.audit_log.record(
            "verification_repair_continued",
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", session.id.clone()),
                ("fromTaskId", limited_task_id.clone()),
                ("taskId", task.id.clone()),
            ],
        )?;
        self.run_repair_turn(
            repository_root,
            session,
            task,
            &report,
            &plan,
            changed,
            file_hashes,
            model_adapter,
            sink,
        )
    }

    /// Asks the model to fix only the introduced failures, and either returns
    /// its patch proposal or closes the attempt as `no_repair_applied`.
    ///
    /// The request is [`VerificationReport::repair_request`]: normalized
    /// introduced evidence, never the full report and never raw log output. It
    /// enters as the turn's user request through context assembly so
    /// `bounded_messages` can never elide it (spec 23 `context.md` §13).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_repair_turn(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        report: &VerificationReport,
        plan: &TaskPlan,
        changed: Vec<String>,
        file_hashes: BTreeMap<String, String>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        // Spec 102 Task 3: the repair request is the engine's normalized
        // introduced evidence, never the whole report and never raw output.
        // Pre-existing, environment and unknown failures stay visible in the
        // user's report but cannot reach the model here.
        let rendered = report.repair_request().unwrap_or_else(|| {
            let lines = report
                .repair_targets
                .iter()
                .map(|target| format!("- {} — {}", target.label, target.reason))
                .collect::<Vec<_>>()
                .join("\n");
            format!("Repair only the failures this patch introduced.\n\n{lines}")
        });
        let target_count = report.repair_targets.len();
        let prompt = format!(
            "{VERIFICATION_TURN_PROMPT} {} introduced {} to repair. Fix only {} and propose a patch with `propose_patch`.\n\n{rendered}",
            target_count,
            if target_count == 1 {
                "failure"
            } else {
                "failures"
            },
            if target_count == 1 { "it" } else { "them" },
        );
        let index = crate::index_cache::IndexCache::get_or_build(&self.indexer, repository_root)?;
        // The repair continuation sees the same conversation the turn does,
        // including any active summary (spec 55 §5.3), and the session's
        // context controls (spec 27 §5.4).
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
            plan: Some(plan),
            pinned: &context_pins,
            restriction: context_restriction.as_deref(),
            conversation_summary: conversation.summary.as_ref(),
            // A repair continuation carries no fetched pages of its own; the
            // ordinary turn's assembly is where the session's references enter
            // (spec 51 D5).
            external_references: &[],
            // A repair continuation is a new request built from check findings,
            // not the paused turn: it recalls nothing. Whatever the original
            // turn recalled was sent with that request (spec 30 Task 4).
            memory: &[],
        })?;
        let turn_notes = self.carry_notes_from_the_previous_turn(&session.id, &task)?;
        let prompt_conversation = build_conversation(
            &conversation.messages,
            conversation.summary.as_ref(),
            self.conversation_window_tokens(),
            &self.scanner,
        );
        let options = ChatTurnOptions {
            verification_turn: true,
            ..engine_started_turn_options(&self.session_store, &session.id)
        };
        let messages = vec![
            ModelMessage::system(system_prompt_for_session(
                SessionMode::Code,
                &options,
                false,
                &self.config.shell,
            )),
            ModelMessage::user(build_model_prompt_with_conversation(
                &context,
                &prompt_conversation,
                None,
            )),
        ];
        note_turn_overhead(&mut context.manifest, &prompt_conversation, &messages);
        // Spec 27 §5.1: a repair turn is a turn, so the pre-send review holds it
        // too. `resume_after_context_decision` carries `verification_turn` and
        // closes a resumed repair that answers without a patch through
        // `close_verification_resume`.
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
        let result = self.run_agentic_turn(
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
        // A pause is not the end of the attempt: a command approval or a round
        // budget resumes the same turn and still counts as this attempt.
        let paused = result.command_proposal.is_some()
            || result.plan_proposal.is_some()
            || result.question_proposal.is_some()
            || result.fallback_proposal.is_some()
            || result.round_budget_proposal.is_some()
            || result.check_batch.is_some();
        // Spec 102 Task 4: a repair patch proposal is the first point all
        // three comparable values exist. Record that state; when it proves no
        // progress, stop here rather than expose the redundant patch or spend
        // another automatic attempt.
        if let Some(proposal) = result.patch_proposal.clone() {
            if self.record_repair_state_or_stall(
                repository_root,
                &result.session.id,
                &result.task.id,
                report,
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
        // Spec 103 Task 6: a bounded task stop is already terminal and typed.
        // The verification's ordinary "no repair applied" settlement must not
        // relabel it as a generic failed verification.
        if result.task_control.terminal_outcome.is_some() {
            return Ok(result);
        }
        // The model answered without proposing a patch. Asking it the same
        // question again would spend the remaining attempts for nothing, so the
        // loop stops here (proposal §5.5).
        self.finish_verification_with_outcome(
            repository_root,
            result.session,
            result.task,
            VerificationOutcome::NoRepairApplied,
            changed,
            file_hashes,
        )
    }

    /// The engine's repair-scope review for a patch proposed in this turn.
    ///
    /// Only a verification turn is a repair, and its scope comes from the
    /// durable active verification state rather than a caller-supplied report.
    /// A direct repair and a resumed repair therefore compute the same fact by
    /// the same rule, before the patch is first stored. An ordinary turn never
    /// attaches one.
    pub(super) fn repair_scope_review_for_patch(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        turn_options: ChatTurnOptions,
        patch: &ProposedPatch,
    ) -> Result<Option<RepairScopeReview>> {
        if !turn_options.verification_turn {
            return Ok(None);
        }
        let events = self
            .session_store
            .read_verification_events(&session.id, &[])?;
        let state = fold_verification(events.iter());
        let changed = self.verification_changed_files(&session.id, &task.id, state.due.as_ref())?;
        let file_hashes = hash_existing_files(repository_root, &changed);
        let plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)?
            .unwrap_or_else(|| TaskPlan::new(&task.id, now_millis()));
        let report = VerificationReport::build(&state, &plan, &changed, &file_hashes);
        let finding_paths = repair_scope_finding_paths(&report);
        let proposed = patch
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        Ok(RepairScopeReview::for_patch(
            &changed,
            &finding_paths,
            &proposed,
        ))
    }

    /// Record one repair attempt's comparable state, or its terminal stall.
    ///
    /// Returns `true` when the state repeats an earlier attempt and the
    /// caller must settle the verification as stalled. The durable
    /// `verification_repair_stalled` event records the state and the reason in
    /// one append; a progressing attempt appends `verification_repair_state`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_repair_state_or_stall(
        &self,
        repository_root: &Path,
        session_id: &str,
        task_id: &str,
        report: &VerificationReport,
        changed: &[String],
        proposal: &AgentPatchProposal,
    ) -> Result<bool> {
        let entry = repair_state_entry(
            repository_root,
            changed,
            &report.repair_targets,
            &proposal.files,
            &self.scanner,
        );
        let history = self
            .session_store
            .task_verification_repair_history(session_id, task_id)?;
        let Some(reason) = detect_repair_stall(&history, &entry) else {
            self.session_store.append_verification_event(
                session_id,
                &VerificationEvent::RepairState {
                    task_id: task_id.to_string(),
                    entry,
                },
            )?;
            return Ok(false);
        };
        self.session_store.append_verification_event(
            session_id,
            &VerificationEvent::RepairStalled {
                task_id: task_id.to_string(),
                entry,
                stall: RepairStall::new(reason),
            },
        )?;
        Ok(true)
    }

    /// Settle a verification whose automatic repair stalled.
    ///
    /// The terminal event was appended by [`Self::record_repair_state_or_stall`]
    /// and already cleared the due/in-progress facts on replay. This rebuilds
    /// the report from the durable log, so a reload renders exactly what the
    /// live turn returns, and takes the failed completion settlement.
    /// The engine-owned presentation of the repair stall that already stopped
    /// this task, built from durable verification state (spec 102).
    ///
    /// A Task 6 stop that loses the settlement race must show the authoritative
    /// result, not the rendering it was about to persist. The report summary and
    /// rendering are the same ones [`Self::finish_verification_stalled`] uses.
    pub(super) fn verification_stall_presentation(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
    ) -> Result<(String, crate::verification::VerificationReport)> {
        let events = self
            .session_store
            .read_verification_events(&session.id, &[])?;
        let state = fold_verification(events.iter());
        let plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)?
            .unwrap_or_else(|| TaskPlan::new(&task.id, now_millis()));
        let changed = self.verification_changed_files(&session.id, &task.id, state.due.as_ref())?;
        let file_hashes = hash_existing_files(repository_root, &changed);
        let report = VerificationReport::build(&state, &plan, &changed, &file_hashes);
        let response = format!("{}\n\n{}", report.summary(&plan.report()), report.render());
        Ok((response, report))
    }

    pub(super) fn finish_verification_stalled(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        changed: Vec<String>,
        file_hashes: BTreeMap<String, String>,
    ) -> Result<ChatTurnResult> {
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
        let task = CompletionGateService::new(&self.session_store, repository_root)
            .settle(&task, CompletionSettlement::VerificationFailed)?
            .task;
        self.verification_turn_result(session, task, response, None, Some(report), changed)
    }
}

/// The user-approved repair targets for an `uncertain` failure.
///
/// These are not automatic targets: `VerificationReport::build` deliberately
/// leaves them out. The answer to the engine's repair question is what turns
/// them into one repair attempt, and the patch that attempt proposes still
/// needs its ordinary approval.
fn uncertain_repair_targets(report: &VerificationReport) -> Vec<RepairTarget> {
    report
        .provenance
        .iter()
        .filter(|partition| partition.provenance == FailureProvenance::Uncertain)
        .flat_map(|partition| partition.checks.iter())
        .map(|check| RepairTarget {
            check: check.check.clone(),
            label: check.label.clone(),
            provenance: FailureProvenance::Uncertain,
            termination: check.termination.clone(),
            fingerprint: check.fingerprint.clone(),
            evidence: check.evidence.clone(),
            reason: check.reason.clone(),
        })
        .collect()
}

/// Every normalized finding location in the active verification report.
///
/// Both the full redacted evidence and a repair target's introduced delta
/// count: the existing repair scope is where the report says failures are, not
/// only the subset a repair turn may read.
fn repair_scope_finding_paths(report: &VerificationReport) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for partition in &report.provenance {
        for check in &partition.checks {
            for evidence in check.evidence.iter().chain(check.introduced.iter()) {
                if let Some(path) = evidence.path() {
                    paths.insert(path.to_string());
                }
            }
        }
    }
    for target in &report.repair_targets {
        for evidence in &target.evidence {
            if let Some(path) = evidence.path() {
                paths.insert(path.to_string());
            }
        }
    }
    paths.into_iter().collect()
}
