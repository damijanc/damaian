use super::*;

impl ChatOrchestrator {
    /// Parks a turn after assembly and before its first model call so the user
    /// can review and edit what it is about to send (spec 27 §5.1).
    ///
    /// Nothing has been sent, so this is a pause and not a failure. It is the
    /// only pause that may be resumed by
    /// [`Self::resume_after_context_decision`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn pause_for_context_review(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        plan: ContextPlan,
        conversation: PromptConversation,
        summary: Option<ConversationSummary>,
        recalled_memory: Vec<RecalledMemory>,
        messages: Vec<ModelMessage>,
        turn_notes: TurnNotesState,
        turn_options: ChatTurnOptions,
    ) -> Result<ChatTurnResult> {
        let proposal_id = create_id("contextreview");
        let prepared = PreparedContext::from_plan(&plan);
        let manifest = prepared.manifest.clone();
        let context_files = prepared.files();
        let response =
            "Review the context before this turn is sent. Nothing has been sent yet.".to_string();
        self.pending_commands.save(&PendingChatTurn {
            proposal_id: proposal_id.clone(),
            session: session.clone(),
            task: task.clone(),
            repository_root: repository_root.to_string_lossy().to_string(),
            context_files: context_files.clone(),
            round: 0,
            exploration_rounds: 0,
            turn_notes: turn_notes.text.clone(),
            turn_notes_carried: turn_notes.carried,
            calls: turn_notes.calls.clone(),
            completed_tool_rounds: Vec::new(),
            step_evidence: Vec::new(),
            plan_reminder_sent: false,
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
            round_budget: None,
            segment: None,
            question: None,
            check_batch: None,
            failure_baseline: None,
            fetch_call: None,
            context_review: Some(PendingContextReview {
                prepared,
                conversation,
                summary,
                summary_removed: false,
                recalled_memory,
            }),
        })?;
        self.note_pending_approvals(
            &session,
            &task,
            vec![PendingApproval {
                kind: "context_review".to_string(),
                proposal_id: proposal_id.clone(),
            }],
        );
        let task = self.session_store.await_approval(
            &task,
            &crate::session::PendingApprovalRef {
                kind: "context_review".to_string(),
                proposal_id: proposal_id.clone(),
            },
        )?;
        self.audit_log.record(
            "context_review_paused",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("proposalId", proposal_id.clone()),
                ("tokenEstimate", manifest.total_tokens.to_string()),
                ("limit", manifest.limit.tokens.to_string()),
                (
                    "limitSource",
                    format!("{:?}", manifest.limit.source).to_lowercase(),
                ),
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
            round_budget_proposal: None,
            check_batch: None,
            verification_report: None,
            turn_notes: turn_notes.view(),
            cancelled: false,
            usage,
            estimated_cost,
            context_review: Some(AgentContextReview {
                id: proposal_id,
                manifest,
            }),
            task_control,
            turn_presentation,
        })
    }

    /// Continues or declines a paused pre-send review (spec 27 §5.1).
    ///
    /// `Send` re-renders the user message from the amended prepared plan and
    /// enters the same `run_agentic_turn` a non-paused turn would have entered.
    /// `Decline` sends nothing at all: no model call, and the task is
    /// `Cancelled` with a transcript line saying so.
    pub fn resume_after_context_decision(
        &self,
        proposal_id: &str,
        decision: ContextDecision,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(proposal_id)?;
        let turn_notes = TurnNotesState::from_pending(&pending);
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::ContextReview)?;
        let Some(review) = pending.context_review.clone() else {
            // `refuse_unless_paused_for` just proved the pause is a review.
            unreachable!("a context review pause must carry its review");
        };
        self.audit_log.record(
            "context_review_decision",
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", proposal_id.to_string()),
                (
                    "decision",
                    match decision {
                        ContextDecision::Send => "send",
                        ContextDecision::Decline => "decline",
                    }
                    .to_string(),
                ),
            ],
        )?;

        if decision == ContextDecision::Decline {
            let message = "No request was sent: you declined the context review.".to_string();
            self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
            self.session_store.append_message(
                &pending.session.id,
                Some(&pending.task.id),
                "assistant",
                &message,
            )?;
            let task = self.session_store.update_task_status_with_kind(
                &pending.task,
                TaskStatus::Cancelled,
                Some(&message),
                Some("context_review_declined"),
            )?;
            let usage = self
                .session_store
                .read_task_usage(&pending.session.id)
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
            let mut model_run = ModelRun::cancelled_before_start(
                &self.config.model_provider,
                &self.config.model_name,
            );
            model_run.content = message.clone();
            let task_control = self.task_control_for(&task);
            let turn_presentation = self.turn_presentation_for(&task, false, true, None);
            return Ok(ChatTurnResult {
                session: pending.session,
                task,
                model_run,
                context_files: pending.context_files,
                response: message,
                command_proposal: None,
                patch_proposal: None,
                plan_proposal: None,
                question_proposal: None,
                fetch_proposal: None,
                fallback_proposal: None,
                round_budget_proposal: None,
                check_batch: None,
                verification_report: None,
                turn_notes: turn_notes.view(),
                cancelled: true,
                usage,
                estimated_cost,
                context_review: None,
                task_control,
                turn_presentation,
            });
        }

        let repository_root = PathBuf::from(&pending.repository_root);
        let mut plan = review.prepared.into_plan();
        // Spec 30 §5.6: the review may have sat open long enough for an entry
        // to be deleted, expired or withdrawn. This is the same turn, so it
        // re-checks the entries the plan carries and never recalls again: a
        // second recall could rank differently and put in the request an entry
        // the user never reviewed.
        let recalled_memory = if plan.manifest.memory.is_empty() {
            review.recalled_memory
        } else {
            let identity = ProjectIdentity::resolve_for_root(&repository_root, &self.git)?;
            self.recheck_memory_before_send(
                &repository_root,
                &identity,
                &pending.session.id,
                &mut plan,
                review.recalled_memory,
            )?
        };
        let conversation = if review.summary_removed {
            let covered = review
                .summary
                .as_ref()
                .map(|summary| summary.message_count)
                .unwrap_or(0);
            review.conversation.without_summary(covered)
        } else {
            review.conversation
        };
        let mut messages = pending.messages;
        // The request is rebuilt from the amended plan; the system prompt is
        // unchanged and stays at index 0.
        if messages.len() >= 2 {
            messages[1] = ModelMessage::user(build_model_prompt_with_conversation(
                &plan,
                &conversation,
                None,
            ));
        }
        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        let task = self.session_store.update_task_status(
            &pending.task,
            TaskStatus::PreparingContext,
            None,
        )?;
        // The turn is now committed to sending, so this is its one
        // `context_assembled` record, whether or not the user amended the plan.
        // The newest record wins in the reader, so an amended manifest is what
        // the post-send view shows, and an unchanged send writes it once.
        let mut context_fields = vec![
            ("actor", "system".to_string()),
            ("sessionId", pending.session.id.clone()),
        ];
        context_fields.extend(plan.manifest.audit_fields());
        self.audit_log
            .record("context_assembled", &context_fields)?;
        sink.context(&plan.manifest);
        // Spec 30 C9: one recall record per entry this send actually carries.
        // A removal in the review and a send-time rejection both dropped their
        // manifest rows, so neither is recorded.
        self.record_memory_recall(&plan.manifest, &recalled_memory)?;

        let verification_turn = pending.turn_options.verification_turn;
        let segment_pause = pending.segment.clone();
        let result = self.run_agentic_turn(
            &repository_root,
            pending.session,
            task,
            plan.files,
            messages,
            pending.round,
            pending.exploration_rounds,
            turn_notes,
            CompletedToolTracker::from(pending.completed_tool_rounds.clone()),
            pending.step_evidence,
            pending.plan_reminder_sent,
            model_adapter,
            sink,
            pending.turn_options,
            segment_pause.as_ref(),
        )?;
        // A paused repair turn resumes here rather than in `run_repair_turn`,
        // so the attempt has to be closed by the same rule the other resume
        // paths use: an answer with no patch is `no_repair_applied`.
        if verification_turn {
            return self.close_verification_resume(&repository_root, result);
        }
        Ok(result)
    }

    /// Removes one optional item from a paused review and returns the amended
    /// manifest (spec 27 §5.4).
    ///
    /// Refuses a turn that is not a context review, and refuses `Required`
    /// below the UI. The pending turn is **not** taken: the review is still
    /// open, and the user may remove another item or decline.
    pub fn remove_context_item(&self, proposal_id: &str, index: usize) -> Result<ContextManifest> {
        let pending = self.pending_commands.load(proposal_id)?;
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::ContextReview)?;
        let mut review = pending.context_review.clone().ok_or_else(|| {
            ClientError::InvalidInput(format!("{proposal_id} is not a context review"))
        })?;
        let record = review.prepared.remove_optional(index)?;
        self.audit_log.record(
            "context_item_removed",
            &[
                ("actor", "user".to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", proposal_id.to_string()),
                ("category", record.category.as_str().to_string()),
                (
                    "path",
                    record.path.clone().unwrap_or_else(|| "(none)".to_string()),
                ),
                (
                    "range",
                    record
                        .range
                        .map(|range| format!("{}-{}", range.start, range.end))
                        .unwrap_or_else(|| "(whole)".to_string()),
                ),
            ],
        )?;
        self.save_context_review(&pending, review.clone())?;
        Ok(review.prepared.manifest)
    }

    /// Removes the #55 conversation summary from a paused review
    /// (spec 27 §5.4). The summary is not an item, so it has its own call.
    pub fn remove_context_summary(&self, proposal_id: &str) -> Result<ContextManifest> {
        let pending = self.pending_commands.load(proposal_id)?;
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::ContextReview)?;
        let mut review = pending.context_review.clone().ok_or_else(|| {
            ClientError::InvalidInput(format!("{proposal_id} is not a context review"))
        })?;
        let record = review.prepared.remove_summary()?;
        review.summary_removed = true;
        self.audit_log.record(
            "context_summary_removed",
            &[
                ("actor", "user".to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", proposal_id.to_string()),
                ("category", record.category.as_str().to_string()),
            ],
        )?;
        self.save_context_review(&pending, review.clone())?;
        Ok(review.prepared.manifest)
    }

    /// Resolves the manifest's memory rows against the store for the inspector
    /// (spec 30 §5.4, C3, C17).
    ///
    /// The manifest is content-free and is written to the audit log, so this is
    /// the only place a recalled statement is reconstructed for display. It
    /// reads exactly the scopes recall reads — this project, this session, and
    /// user scope only while that scope is enabled — so an id owned by another
    /// project cannot resolve and is reported as `deleted` rather than exposed.
    /// Every resolved statement passes the secret scanner, because a memory
    /// file edited outside Damaian may hold a credential even though the store
    /// refuses one at write time (C13.2). Nothing here is written or audited.
    pub fn resolve_memory_display(
        &self,
        repository_root: &Path,
        session_id: &str,
        references: &[MemoryRecallRef],
    ) -> Result<Vec<MemoryDisplayEntry>> {
        if references.is_empty() {
            return Ok(Vec::new());
        }
        let identity = ProjectIdentity::resolve_for_root(repository_root, &self.git)?;
        let project_entries = self.memory_store.project_entries(&identity)?;
        let session_entries = self.memory_store.session_entries(session_id)?;
        let user_entries = self.memory_store.user_entries()?;
        let mut entries: HashMap<&str, &MemoryEntry> = HashMap::new();
        for entry in project_entries
            .iter()
            .chain(session_entries.iter())
            .chain(user_entries.iter())
        {
            entries.insert(entry.id.as_str(), entry);
        }
        Ok(references
            .iter()
            .map(|reference| {
                let entry = entries
                    .get(reference.entry_id.as_str())
                    .copied()
                    .filter(|entry| entry.state != MemoryState::Deleted);
                let (statement, redacted) = match entry {
                    Some(entry) => {
                        let redaction = self.scanner.redact(&entry.statement);
                        (Some(redaction.text), !redaction.findings.is_empty())
                    }
                    None => (None, false),
                };
                MemoryDisplayEntry {
                    entry_id: reference.entry_id.clone(),
                    statement,
                    scope: entry.map(|entry| entry.scope),
                    category: entry.map(|entry| entry.category),
                    provenance: entry.map(|entry| entry.provenance.clone()),
                    created_at_ms: entry.map(|entry| entry.created_at_ms),
                    verification: entry
                        .map(|entry| entry.verification)
                        .unwrap_or(reference.verification),
                    match_reason: reference.match_reason.clone(),
                    redacted,
                    deleted: entry.is_none(),
                }
            })
            .collect())
    }

    fn save_context_review(
        &self,
        pending: &PendingChatTurn,
        review: PendingContextReview,
    ) -> Result<()> {
        let mut updated = pending.clone();
        updated.context_review = Some(review);
        self.pending_commands.save(&updated)
    }
}
