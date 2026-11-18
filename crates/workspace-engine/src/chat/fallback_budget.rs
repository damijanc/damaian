use super::*;

impl ChatOrchestrator {
    /// Decides a provider-fallback offer (spec 56).
    ///
    /// Declining — the default — fails the task exactly as an unoffered
    /// refusal does (spec 48 §5.4): `failureKind`, the provider's message, and
    /// the refusal returned as the error. The marker and the measured zero
    /// were already written when the refusal arrived, so nothing is booked
    /// twice.
    pub fn resume_after_fallback_decision(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(proposal_id)?;
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::ProviderFallback)?;
        let Some(offer) = pending.provider_fallback.clone() else {
            // `refuse_unless_paused_for` just proved the pause is a fallback.
            unreachable!("a provider fallback pause must carry its offer");
        };

        if approved {
            // The request takes its provider, model, token ceiling and tool
            // contract from `self.config`, not from the adapter, so the caller
            // must resume on an engine whose config has the fallback active
            // (spec 56 `tasks.md` C1). Anything else would send the refused
            // provider's model name to the fallback's endpoint — refused, and
            // the offer put back so the right engine can still take it.
            if self.config.model_provider != offer.fallback_provider {
                self.pending_commands.save(&pending)?;
                return Err(ClientError::InvalidInput(format!(
                    "Proposal {proposal_id} must be resumed with {} as the active provider, not {}",
                    offer.fallback_provider, self.config.model_provider
                )));
            }
            return self.reissue_through_fallback(pending, offer, approved_by, model_adapter, sink);
        }

        self.audit_log.record(
            "provider_fallback_decision",
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", proposal_id.to_string()),
                ("refusedProvider", offer.refused_provider.clone()),
                ("fallbackProvider", offer.fallback_provider.clone()),
                ("decision", "declined".to_string()),
            ],
        )?;
        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        // Declining the fallback fails the task without re-entering the loop,
        // so this is the one place outside it that must settle the plan from
        // the evidence the pause carried (spec 66 `context.md` D6).
        let mut plan = self
            .session_store
            .read_task_plan(&pending.session.id, &pending.task.id)?;
        let mut step_evidence = pending.step_evidence.clone();
        let _ = self.settle_plan_at_turn_end(
            &pending.session,
            &pending.task,
            &mut plan,
            &mut step_evidence,
            pending.plan_reminder_sent,
            "failed",
        );
        self.session_store.update_task_status_with_kind(
            &pending.task,
            TaskStatus::Failed,
            Some(offer.message.as_str()),
            Some(offer.refusal_code.as_str()),
        )?;
        if let Some(current) = plan.as_ref() {
            sink.plan_finished(current);
        }
        Err(ClientError::Provider(
            ProviderRefusal::from_code(&offer.refusal_code).unwrap_or(ProviderRefusal::Unknown),
            offer.message,
        ))
    }

    /// The approved half of [`Self::resume_after_fallback_decision`]: records
    /// the switch, then re-issues the refused round through this engine's
    /// provider — which the caller has already made the fallback.
    ///
    /// The round is re-entered, not advanced: it never completed, so the
    /// fallback gets the same messages the refused provider was sent and the
    /// round budget is not charged twice. The plan is left exactly where the
    /// refusal left it, which is what "carried across unchanged" means here.
    fn reissue_through_fallback(
        &self,
        pending: PendingChatTurn,
        offer: PendingProviderFallback,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let turn_notes = TurnNotesState::from_pending(&pending);
        self.audit_log.record(
            "provider_fallback_decision",
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", pending.proposal_id.clone()),
                ("refusedProvider", offer.refused_provider.clone()),
                ("fallbackProvider", offer.fallback_provider.clone()),
                ("fallbackModel", self.config.model_name.clone()),
                ("decision", "approved".to_string()),
            ],
        )?;
        self.session_store.record_provider_fallback(
            &pending.task,
            &offer.refused_provider,
            &offer.fallback_provider,
            approved_by,
        )?;
        // The transcript marker (requirement 6). Worded as what happened, not
        // as an instruction: earlier turns reach the model as plain
        // `role: content` lines, so this is read as a fact about the
        // conversation. Written before the call, so it says "switched to"
        // rather than claiming an answer the fallback may yet refuse.
        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "system",
            &format!(
                "Switched to {} ({}) for this turn after {} refused it ({}).",
                offer.fallback_provider,
                self.config.model_name,
                offer.refused_provider,
                offer.refusal_code
            ),
        )?;
        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        let task = self.session_store.update_task_status(
            &pending.task,
            TaskStatus::PreparingContext,
            None,
        )?;
        let turn_options = ChatTurnOptions {
            provider_fallback_used: true,
            ..pending.turn_options
        };
        let repository_root = PathBuf::from(&pending.repository_root);
        let segment_pause = pending.segment.clone();
        let result = self.run_agentic_turn(
            &repository_root,
            pending.session,
            task,
            pending.context_files,
            pending.messages,
            pending.round,
            pending.exploration_rounds,
            turn_notes,
            CompletedToolTracker::from(pending.completed_tool_rounds.clone()),
            pending.step_evidence,
            pending.plan_reminder_sent,
            model_adapter,
            sink,
            turn_options,
            segment_pause.as_ref(),
        )?;
        if pending.turn_options.verification_turn {
            return self.close_verification_resume(&repository_root, result);
        }
        Ok(result)
    }

    /// Decides a round-budget pause (spec 61 §5.2).
    ///
    /// *Continue* grants the turn another segment of tool rounds and re-enters
    /// with the saved messages and round, seeding `round_budget_extension` from
    /// the running `rounds_granted` total so the limit continues where it
    /// stopped. *Answer now* re-enters with the forced final call and no extra
    /// rounds; the turn ends answered or `ToolBudgetExhausted`, exactly as the
    /// forced call would have before this spec.
    ///
    /// Refuses any other kind of pause, and the other resume entry points
    /// refuse a round-budget one (C5), so a pause can only be answered by its
    /// own entry point.
    pub fn resume_round_budget(
        &self,
        proposal_id: &str,
        decision: RoundBudgetDecision,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(proposal_id)?;
        let turn_notes = TurnNotesState::from_pending(&pending);
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::RoundBudget)?;
        let Some(pause) = pending.round_budget.clone() else {
            // `refuse_unless_paused_for` just proved the pause is a budget stop.
            unreachable!("a round-budget pause must carry its rounds");
        };
        let repository_root = PathBuf::from(&pending.repository_root);

        match decision {
            RoundBudgetDecision::Continue => {
                // The turn has spent every segment the cap allows; only *Answer
                // now* remains (spec 61 §5.3). The pause stays on disk for it.
                if pause.rounds_used >= ABSOLUTE_TURN_ROUND_CAP {
                    self.pending_commands.save(&pending)?;
                    return Err(ClientError::InvalidInput(format!(
                        "Proposal {proposal_id} has reached the turn round cap \
                         ({ABSOLUTE_TURN_ROUND_CAP}); only Answer now is available"
                    )));
                }
                // The segment granted is the same one a token-ceiling
                // continuation adds (`default_tool_round_limit`), and the
                // running total is what the resumed turn seeds its extension
                // from — the two must agree or the limit would reset.
                let granted = self.default_tool_round_limit();
                let rounds_granted = pause.rounds_granted + granted;
                let note = format!("You have {granted} more tool rounds. Continue the task.");

                self.audit_log.record(
                    "round_budget_decision",
                    &[
                        ("actor", approved_by.to_string()),
                        ("sessionId", pending.session.id.clone()),
                        ("taskId", pending.task.id.clone()),
                        ("proposalId", proposal_id.to_string()),
                        ("decision", "continue".to_string()),
                    ],
                )?;
                // The audit record Task 3's tests assert (§5.4): who granted
                // it, the round, the delta, and the running total.
                self.audit_log.record(
                    "turn_budget_extended",
                    &[
                        ("actor", approved_by.to_string()),
                        ("sessionId", pending.session.id.clone()),
                        ("taskId", pending.task.id.clone()),
                        ("round", pending.round.to_string()),
                        ("roundsGranted", granted.to_string()),
                        ("roundsTotal", rounds_granted.to_string()),
                    ],
                )?;
                self.session_store.append_message(
                    &pending.session.id,
                    Some(&pending.task.id),
                    "user",
                    &note,
                )?;
                self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
                let task = self.session_store.update_task_status(
                    &pending.task,
                    TaskStatus::PreparingContext,
                    None,
                )?;
                let mut messages = pending.messages;
                messages.push(ModelMessage::user(note));
                let turn_options = ChatTurnOptions {
                    round_budget_extension: rounds_granted,
                    ..pending.turn_options
                };
                let segment_pause = pending.segment.clone();
                let result = self.run_agentic_turn(
                    &repository_root,
                    pending.session,
                    task,
                    pending.context_files,
                    messages,
                    pending.round,
                    pending.exploration_rounds,
                    turn_notes,
                    CompletedToolTracker::from(pending.completed_tool_rounds.clone()),
                    pending.step_evidence,
                    pending.plan_reminder_sent,
                    model_adapter,
                    sink,
                    turn_options,
                    segment_pause.as_ref(),
                )?;
                if pending.turn_options.verification_turn {
                    return self.close_verification_resume(&repository_root, result);
                }
                Ok(result)
            }
            RoundBudgetDecision::AnswerNow => {
                self.audit_log.record(
                    "round_budget_decision",
                    &[
                        ("actor", approved_by.to_string()),
                        ("sessionId", pending.session.id.clone()),
                        ("taskId", pending.task.id.clone()),
                        ("proposalId", proposal_id.to_string()),
                        ("decision", "answer_now".to_string()),
                    ],
                )?;
                self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
                let task = self.session_store.update_task_status(
                    &pending.task,
                    TaskStatus::PreparingContext,
                    None,
                )?;
                let turn_options = ChatTurnOptions {
                    round_budget_answer_now: true,
                    ..pending.turn_options
                };
                let segment_pause = pending.segment.clone();
                let result = self.run_agentic_turn(
                    &repository_root,
                    pending.session,
                    task,
                    pending.context_files,
                    pending.messages,
                    pending.round,
                    pending.exploration_rounds,
                    turn_notes,
                    CompletedToolTracker::from(pending.completed_tool_rounds.clone()),
                    pending.step_evidence,
                    pending.plan_reminder_sent,
                    model_adapter,
                    sink,
                    turn_options,
                    segment_pause.as_ref(),
                )?;
                if pending.turn_options.verification_turn {
                    return self.close_verification_resume(&repository_root, result);
                }
                Ok(result)
            }
        }
    }

    /// Builds the prompt that continues a stored `ToolBudgetExhausted` turn
    /// (spec 61 §5.7). The original request is read from the task record, not
    /// reconstructed by the client: a turn stopped after many messages has long
    /// since pushed its request out of the window a client could re-read, and
    /// only `task.user_prompt` still holds it verbatim.
    pub fn tool_budget_ask_again_prompt(&self, session_id: &str, task_id: &str) -> Result<String> {
        let task = self
            .session_store
            .read_tasks(session_id)?
            .into_iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| ClientError::InvalidInput(format!("Unknown task: {task_id}")))?;
        if task.status != TaskStatus::ToolBudgetExhausted {
            return Err(ClientError::InvalidInput(format!(
                "Task {task_id} is not waiting on its tool budget"
            )));
        }
        Ok(format!(
            "Continue this earlier request: {}",
            task.user_prompt
        ))
    }

    /// Parks a refused turn on a provider-fallback offer (spec 56 §5.1).
    ///
    /// Returned from the refusal arm directly rather than through the loop's
    /// `terminal` value: there is no model run to finish and no response to
    /// persist, and skipping the seal is right — like a plan review, nothing
    /// has happened yet and the turn may still continue.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn pause_for_provider_fallback(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        context_files: Vec<String>,
        messages: Vec<ModelMessage>,
        action_rounds: u32,
        exploration_rounds: u32,
        turn_notes: TurnNotesState,
        completed_tool_rounds: CompletedToolTracker,
        step_evidence: Vec<crate::plan::Evidence>,
        plan_reminder_sent: bool,
        turn_options: ChatTurnOptions,
        segment: Option<&SegmentRunState>,
        offer: PendingProviderFallback,
    ) -> Result<ChatTurnResult> {
        let proposal_id = create_id("fallback");
        let response = provider_fallback_response(&offer);
        self.pending_commands.save(&PendingChatTurn {
            proposal_id: proposal_id.clone(),
            session: session.clone(),
            task: task.clone(),
            repository_root: repository_root.to_string_lossy().to_string(),
            context_files: context_files.clone(),
            round: action_rounds,
            exploration_rounds,
            turn_notes: turn_notes.text.clone(),
            turn_notes_carried: turn_notes.carried,
            calls: turn_notes.calls.clone(),
            completed_tool_rounds: completed_tool_rounds.rounds.clone(),
            step_evidence,
            plan_reminder_sent,
            // The request as the provider refused it: approving re-issues this
            // round, so nothing is appended for a call that never answered.
            messages,
            matched_tool_call: None,
            last_content: String::new(),
            turn_options,
            reasoning_content: None,
            round_calls_fed_back: false,
            mcp_call: None,
            web_diagnostic_call: None,
            plan_review: None,
            provider_fallback: Some(offer.clone()),
            round_budget: None,
            segment: segment.map(|tracker| tracker.pause_state(action_rounds, exploration_rounds)),
            question: None,
            check_batch: None,
            failure_baseline: None,
            fetch_call: None,
            context_review: None,
        })?;
        self.note_pending_approvals(
            &session,
            &task,
            vec![PendingApproval {
                kind: "provider_fallback".to_string(),
                proposal_id: proposal_id.clone(),
            }],
        );
        let task = self.session_store.await_approval(
            &task,
            &crate::session::PendingApprovalRef {
                kind: "provider_fallback".to_string(),
                proposal_id: proposal_id.clone(),
            },
        )?;
        self.audit_log.record(
            "provider_fallback_offered",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("proposalId", proposal_id.clone()),
                ("refusedProvider", offer.refused_provider.clone()),
                ("fallbackProvider", offer.fallback_provider.clone()),
                ("classification", offer.refusal_code.clone()),
            ],
        )?;
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
            ModelRun::cancelled_before_start(&offer.refused_provider, &offer.refused_model);
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
            fallback_proposal: Some(AgentFallbackProposal {
                id: proposal_id,
                refused_provider: offer.refused_provider,
                refused_model: offer.refused_model,
                fallback_provider: offer.fallback_provider,
                fallback_model: offer.fallback_model,
                refusal_code: offer.refusal_code,
                message: offer.message,
            }),
            round_budget_proposal: None,
            check_batch: None,
            context_review: None,
            verification_report: None,
            turn_notes: turn_notes.view(),
            cancelled: false,
            usage,
            estimated_cost,
            task_control,
            turn_presentation,
        })
    }

    /// Parks a turn that hit its round budget (spec 61 §5.1).
    ///
    /// Returned from the loop directly, before the forced final call is made:
    /// nothing is in flight, so there is no model run to finish and no response
    /// to persist, and skipping the seal is right — like a plan review, nothing
    /// has happened yet and the turn may still continue. The forced call is made
    /// only when the user answers *Answer now* (Task 2).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn pause_for_round_budget(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        context_files: Vec<String>,
        messages: Vec<ModelMessage>,
        action_rounds: u32,
        exploration_rounds: u32,
        turn_notes: TurnNotesState,
        completed_tool_rounds: CompletedToolTracker,
        step_evidence: Vec<crate::plan::Evidence>,
        plan_reminder_sent: bool,
        max_rounds: u32,
        rounds_granted: u32,
        turn_options: ChatTurnOptions,
        segment: Option<&SegmentRunState>,
    ) -> Result<ChatTurnResult> {
        let proposal_id = create_id("roundbudget");
        let response = round_budget_pause_response(action_rounds, max_rounds, exploration_rounds);
        // The turn has spent its last segment: only *Answer now* is left.
        let at_hard_cap = action_rounds >= ABSOLUTE_TURN_ROUND_CAP;
        let pause = RoundBudgetPause {
            rounds_used: action_rounds,
            rounds_granted,
        };
        self.pending_commands.save(&PendingChatTurn {
            proposal_id: proposal_id.clone(),
            session: session.clone(),
            task: task.clone(),
            repository_root: repository_root.to_string_lossy().to_string(),
            context_files: context_files.clone(),
            round: action_rounds,
            exploration_rounds,
            turn_notes: turn_notes.text.clone(),
            turn_notes_carried: turn_notes.carried,
            calls: turn_notes.calls.clone(),
            completed_tool_rounds: completed_tool_rounds.rounds.clone(),
            step_evidence,
            plan_reminder_sent,
            // The conversation so far, unchanged: nothing was dispatched, so
            // resuming replays it and the model simply asks for its tool again.
            messages,
            matched_tool_call: None,
            last_content: String::new(),
            turn_options,
            reasoning_content: None,
            round_calls_fed_back: false,
            mcp_call: None,
            web_diagnostic_call: None,
            plan_review: None,
            provider_fallback: None,
            round_budget: Some(pause),
            segment: segment.map(|tracker| tracker.pause_state(action_rounds, exploration_rounds)),
            question: None,
            check_batch: None,
            failure_baseline: None,
            fetch_call: None,
            context_review: None,
        })?;
        self.note_pending_approvals(
            &session,
            &task,
            vec![PendingApproval {
                kind: "round_budget".to_string(),
                proposal_id: proposal_id.clone(),
            }],
        );
        let task = self.session_store.await_approval(
            &task,
            &crate::session::PendingApprovalRef {
                kind: "round_budget".to_string(),
                proposal_id: proposal_id.clone(),
            },
        )?;
        self.audit_log.record(
            "round_budget_paused",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("proposalId", proposal_id.clone()),
                ("roundsUsed", action_rounds.to_string()),
                ("roundsTotal", max_rounds.to_string()),
                ("roundsGranted", rounds_granted.to_string()),
                // C10: the pause record names both budgets. `roundsUsed` alone
                // would read as the whole turn when most of it was reads.
                ("explorationRounds", exploration_rounds.to_string()),
                (
                    "explorationLimit",
                    self.default_exploration_round_limit().to_string(),
                ),
                ("notesChars", turn_notes.text.chars().count().to_string()),
            ],
        )?;
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
            round_budget_proposal: Some(AgentRoundBudgetProposal {
                id: proposal_id,
                rounds_used: action_rounds,
                rounds_total: max_rounds,
                rounds_granted,
                rounds_more: self.default_tool_round_limit(),
                at_hard_cap,
            }),
            check_batch: None,
            context_review: None,
            verification_report: None,
            turn_notes: turn_notes.view(),
            cancelled: false,
            usage,
            estimated_cost,
            task_control,
            turn_presentation,
        })
    }
}

/// The text a paused turn shows while its plan is under review.
fn provider_fallback_response(offer: &PendingProviderFallback) -> String {
    format!(
        "{} refused this request ({}): {}\n\n{} is configured and could answer this turn instead. Nothing has been switched; declining leaves the task failed.",
        offer.refused_provider, offer.refusal_code, offer.message, offer.fallback_provider
    )
}

/// Names the ceiling, what was actually spent, and what is left undone.
///
/// §5.4 requires the usage consumed and the remaining steps. Without the
/// figures the user cannot tell a ceiling set too low from a turn that
/// genuinely ran away, which are opposite problems with opposite fixes.
pub(super) fn token_budget_exhausted_response(
    ceiling: u64,
    spent: u64,
    plan: Option<&crate::plan::TaskPlan>,
) -> String {
    let remaining: Vec<&str> = plan
        .map(|plan| {
            plan.open_steps()
                .into_iter()
                .map(|step| step.title.as_str())
                .collect()
        })
        .unwrap_or_default();
    let mut message = format!(
        "This task reached its token ceiling of {ceiling} after spending {spent}. I stopped at a step boundary rather than continue, so nothing is half-done."
    );
    if !remaining.is_empty() {
        message.push_str(&format!("\n\nStill to do: {}.", remaining.join("; ")));
    }
    message.push_str(
        "\n\nRaise `agent_max_task_tokens` and ask again to carry on, or narrow the request.",
    );
    message
}

/// "1 read-only round" / "2 read-only rounds": the count is often zero or one,
/// and the label reads as a bare count in the progress stream too.
fn read_only_rounds(count: u32) -> String {
    if count == 1 {
        "1 read-only round".to_string()
    } else {
        format!("{count} read-only rounds")
    }
}

/// Names both budgets so the user can tell which one stopped the turn and what
/// each had used (spec 59 requirement 8). Deliberately says nothing about
/// debugging: a turn may have been reading, editing or running commands.
pub(super) fn tool_budget_exhausted_response(max_rounds: u32, exploration_rounds: u32) -> String {
    format!(
        "This turn reached its budget of {max_rounds} action rounds and had used {} while the model was still trying to call another tool. I stopped instead of treating the raw tool request as a completed answer. Narrow the next request to finish within the budget.",
        read_only_rounds(exploration_rounds)
    )
}

/// The pause message a round-budget stop leaves in the transcript (spec 61
/// §5.6). Names both counts (spec 59 requirement 8) so the user knows where the
/// turn is; the buttons the shell renders beside it are what carry the decision.
fn round_budget_pause_response(
    rounds_used: u32,
    rounds_total: u32,
    exploration_rounds: u32,
) -> String {
    format!(
        "Used {rounds_used} of {rounds_total} action rounds and {}.",
        read_only_rounds(exploration_rounds)
    )
}
