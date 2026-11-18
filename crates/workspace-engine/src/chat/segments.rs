use super::*;

impl ChatOrchestrator {
    /// Builds one durable terminal outcome with the engine state available now.
    ///
    /// `last_segment` is the live snapshot of the segment still open, not the
    /// durable `segment_started` record: a declined boundary leaves that record
    /// at its zeros and without a progress digest, and persisting those would
    /// describe a segment the turn never ran. Token totals come from the
    /// caller's durable usage entry; missing usage stays absent, so the record
    /// never invents a zero someone could read as "spent nothing".
    #[allow(clippy::too_many_arguments)]
    pub(super) fn task_terminal_outcome(
        &self,
        session: &Session,
        task: &Task,
        kind: TaskTerminalOutcomeKind,
        dimension: Option<TaskBudgetDimension>,
        figures: Option<TaskOutcomeFigures>,
        compaction: Option<TaskCompactionFacts>,
        usage: Option<&TaskUsage>,
        last_segment: Option<TaskSegmentSnapshot>,
    ) -> Result<TaskTerminalOutcome> {
        let mut outcome = TaskTerminalOutcome::new(&session.id, &task.id, kind)
            .with_dimension_opt(dimension)
            .with_figures_opt(figures)
            .with_last_segment(last_segment)
            .with_usage(usage.map(|usage| SegmentTokenTotals::from_usage(Some(usage))));
        if let Some(compaction) = compaction {
            outcome = outcome.with_compaction(compaction);
        }
        Ok(outcome)
    }

    /// Records one terminal outcome and returns the loop's stop value.
    ///
    /// The durable status/outcome pair is written later, at the single final
    /// settlement point; this helper only prepares the response the user sees.
    /// The audit fact is written there too, because a spec 102 repair stall can
    /// win the settlement race and then the newer outcome never happened.
    pub(super) fn task_terminal_stop(
        &self,
        outcome: TaskTerminalOutcome,
    ) -> Result<(ModelRun, String, StopReason)> {
        let response = outcome.render();
        let mut run =
            ModelRun::cancelled_before_start(&self.config.model_provider, &self.config.model_name);
        run.content = response.clone();
        Ok((run, response, StopReason::TaskTerminal(Box::new(outcome))))
    }

    /// The terminal outcome for a declined segment continuation.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn segment_decline_outcome(
        &self,
        session: &Session,
        task: &Task,
        reason: crate::task_segments::SegmentDeclineReason,
        budget: &TaskExecutionBudget,
        totals: SegmentTotals,
        usage: Option<&TaskUsage>,
        last_segment: Option<TaskSegmentSnapshot>,
    ) -> Result<TaskTerminalOutcome> {
        use crate::task_segments::SegmentDeclineReason;
        match reason {
            SegmentDeclineReason::ActionRoundsExhausted => {
                let approved = approved_action_round_cap(budget);
                let used = u64::from(totals.action_rounds);
                self.task_terminal_outcome(
                    session,
                    task,
                    TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
                    Some(TaskBudgetDimension::ActionRounds),
                    Some(TaskOutcomeFigures {
                        approved,
                        used,
                        required: None,
                        remaining: Some(approved.saturating_sub(used)),
                    }),
                    None,
                    usage,
                    last_segment,
                )
            }
            SegmentDeclineReason::ExplorationRoundsExhausted => {
                let approved = approved_exploration_round_cap(budget);
                let used = u64::from(totals.exploration_rounds);
                self.task_terminal_outcome(
                    session,
                    task,
                    TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
                    Some(TaskBudgetDimension::ExplorationRounds),
                    Some(TaskOutcomeFigures {
                        approved,
                        used,
                        required: None,
                        remaining: Some(approved.saturating_sub(used)),
                    }),
                    None,
                    usage,
                    last_segment,
                )
            }
            SegmentDeclineReason::InputTokensExhausted => {
                let used = usage.map_or(0, |usage| usage.input_tokens);
                self.task_terminal_outcome(
                    session,
                    task,
                    TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
                    Some(TaskBudgetDimension::InputTokens),
                    Some(TaskOutcomeFigures {
                        approved: budget.input_tokens,
                        used,
                        required: None,
                        remaining: Some(budget.input_tokens.saturating_sub(used)),
                    }),
                    None,
                    usage,
                    last_segment,
                )
            }
            SegmentDeclineReason::OutputTokensExhausted => {
                let used = usage.map_or(0, |usage| usage.output_tokens);
                self.task_terminal_outcome(
                    session,
                    task,
                    TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
                    Some(TaskBudgetDimension::OutputTokens),
                    Some(TaskOutcomeFigures {
                        approved: budget.output_tokens,
                        used,
                        required: None,
                        remaining: Some(budget.output_tokens.saturating_sub(used)),
                    }),
                    None,
                    usage,
                    last_segment,
                )
            }
            SegmentDeclineReason::NoProgress => self.task_terminal_outcome(
                session,
                task,
                TaskTerminalOutcomeKind::NoProgress,
                None,
                None,
                None,
                usage,
                last_segment,
            ),
            // Missing usage is not an exhausted total. It is a named failure
            // that never offers a continuation against the approved bound.
            SegmentDeclineReason::UsageUnavailable => self.task_terminal_outcome(
                session,
                task,
                TaskTerminalOutcomeKind::UsageUnavailable,
                None,
                None,
                None,
                None,
                last_segment,
            ),
        }
    }

    /// The terminal outcome for a prepared request the approved tokens refuse.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn task_authority_outcome(
        &self,
        session: &Session,
        task: &Task,
        budget: &TaskExecutionBudget,
        usage: Option<&TaskUsage>,
        failure: &TaskRequestTokenAuthority,
        last_segment: Option<TaskSegmentSnapshot>,
    ) -> Result<TaskTerminalOutcome> {
        match failure {
            TaskRequestTokenAuthority::InputExceeded {
                required_input_tokens,
                remaining_input_tokens,
            } => self.task_terminal_outcome(
                session,
                task,
                TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
                Some(TaskBudgetDimension::InputTokens),
                Some(TaskOutcomeFigures {
                    approved: budget.input_tokens,
                    used: usage.map_or(0, |usage| usage.input_tokens),
                    required: Some(*required_input_tokens),
                    remaining: Some(*remaining_input_tokens),
                }),
                None,
                usage,
                last_segment,
            ),
            TaskRequestTokenAuthority::OutputExhausted {
                remaining_output_tokens,
            } => self.task_terminal_outcome(
                session,
                task,
                TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
                Some(TaskBudgetDimension::OutputTokens),
                Some(TaskOutcomeFigures {
                    approved: budget.output_tokens,
                    used: usage.map_or(0, |usage| usage.output_tokens),
                    required: None,
                    remaining: Some(*remaining_output_tokens),
                }),
                None,
                usage,
                last_segment,
            ),
            TaskRequestTokenAuthority::UsageUnavailable => self.task_terminal_outcome(
                session,
                task,
                TaskTerminalOutcomeKind::UsageUnavailable,
                None,
                None,
                None,
                None,
                last_segment,
            ),
        }
    }

    /// The terminal outcome for a pre-plan exploration budget that was spent
    /// without the worker proposing a plan when asked (spec 103 R17).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn preplan_exhausted_outcome(
        &self,
        session: &Session,
        task: &Task,
        budget: &TaskExecutionBudget,
        totals: SegmentTotals,
        usage: Option<&TaskUsage>,
        last_segment: Option<TaskSegmentSnapshot>,
    ) -> Result<TaskTerminalOutcome> {
        let approved = approved_exploration_round_cap(budget);
        let used = u64::from(totals.exploration_rounds);
        self.task_terminal_outcome(
            session,
            task,
            TaskTerminalOutcomeKind::PrePlanExplorationExhausted,
            Some(TaskBudgetDimension::ExplorationRounds),
            Some(TaskOutcomeFigures {
                approved,
                used,
                required: None,
                remaining: Some(approved.saturating_sub(used)),
            }),
            None,
            usage,
            last_segment,
        )
    }

    /// The terminal outcome for a request whose pinned/fixed minimum cannot
    /// fit the configured window share.
    pub(super) fn cannot_fit_outcome(
        &self,
        session: &Session,
        task: &Task,
        attempt: &WithinTurnCompactionAttempt,
        usage: Option<&TaskUsage>,
        last_segment: Option<TaskSegmentSnapshot>,
    ) -> Result<TaskTerminalOutcome> {
        self.task_terminal_outcome(
            session,
            task,
            TaskTerminalOutcomeKind::CompactionCannotFit,
            None,
            None,
            Some(TaskCompactionFacts::from_attempt(attempt)),
            usage,
            last_segment,
        )
    }

    /// The deterministic, content-safe digest of the task state a segment
    /// boundary may treat as advancement (spec 103 Task 5).
    pub(super) fn segment_progress_digest(
        &self,
        plan: Option<&TaskPlan>,
        step_evidence: &[Evidence],
        completed_tool_rounds: &CompletedToolTracker,
        turn_notes: &TurnNotesState,
    ) -> String {
        let rounds = completed_tool_rounds.as_compaction_rounds();
        progress_digest(&SegmentProgressInput {
            plan,
            step_evidence,
            completed_rounds: &rounds,
            notes_text: &turn_notes.text,
            notes_carried: turn_notes.carried,
            scanner: &self.scanner,
        })
    }

    /// The live state of the segment that is still open when a task stops.
    ///
    /// A declined boundary leaves its `segment_started` record at zeros and
    /// without a progress digest, so the durable record cannot describe the
    /// segment the turn actually ran. This builds the snapshot from the
    /// counters, durable token totals and digest the engine holds now.
    pub(super) fn live_segment_snapshot(
        &self,
        tracker: &SegmentRunState,
        action_rounds: u32,
        exploration_rounds: u32,
        usage: Option<&TaskUsage>,
        progress_digest: String,
    ) -> TaskSegmentSnapshot {
        tracker.snapshot(
            action_rounds,
            exploration_rounds,
            SegmentTokenTotals::from_usage(usage),
            progress_digest,
        )
    }

    /// Starts the first automatic segment, or resumes the one a pause left open.
    ///
    /// `None` is the whole budgetless path: an inline task, a worker without an
    /// approved budget and every historical pending turn keep today's explicit
    /// round-budget behaviour. Worker status is read from the durable worker
    /// record, never from a prompt, label or UI state.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn task_segment_run_state(
        &self,
        session: &Session,
        task: &Task,
        action_rounds: u32,
        exploration_rounds: u32,
        plan: Option<&TaskPlan>,
        step_evidence: &[Evidence],
        completed_tool_rounds: &CompletedToolTracker,
        turn_notes: &TurnNotesState,
        normal_action_rounds: u32,
        normal_exploration_rounds: u32,
        resume_state: Option<&TaskSegmentPauseState>,
    ) -> Result<Option<(SegmentRunState, bool)>> {
        let approved_budget = self.session_store.read_task_budget(&session.id, &task.id)?;
        if self.session_store.worker_record(&session.id)?.is_none() {
            return Ok(None);
        }
        let Some(budget) = approved_budget else {
            // Spec 103 R17: before a plan exists a worker gets the bounded
            // pre-plan phase. It reuses the segment machinery, but there is no
            // approved task budget yet, so it only resumes a durable segment a
            // paused pre-plan turn left open. A new pre-plan segment starts
            // lazily at its first boundary, keeping a worker that proposes a
            // plan immediately on today's plan-review path with no extra
            // records.
            if plan.is_some() {
                return Ok(None);
            }
            let Some(resume) = resume_state else {
                return Ok(None);
            };
            let budget = self.preplan_budget();
            let segments = self
                .session_store
                .read_task_segments(&session.id, &task.id)?;
            let Some(crate::task_segments::StoredTaskSegment {
                event: crate::task_segments::TaskSegmentEvent::Started(started),
                ..
            }) = segments.last()
            else {
                return Err(ClientError::InvalidInput(
                    "a paused task segment has no durable segment_started record".to_string(),
                ));
            };
            validate_segment_resume(resume, started, action_rounds, exploration_rounds)?;
            return Ok(Some((
                SegmentRunState::resumed(
                    &budget,
                    started,
                    normal_action_rounds,
                    normal_exploration_rounds,
                    true,
                ),
                false,
            )));
        };
        let mut usage_by_task = self.session_store.read_task_usage(&session.id)?;
        let usage = usage_by_task.remove(&task.id);
        let tokens = SegmentTokenTotals::from_usage(usage.as_ref());
        let segments = self
            .session_store
            .read_task_segments(&session.id, &task.id)?;

        if let Some(crate::task_segments::StoredTaskSegment {
            event: crate::task_segments::TaskSegmentEvent::Started(started),
            ..
        }) = segments.last()
        {
            // The pending turn's `segment` field is the resume half: it must
            // describe exactly the open durable segment. A mismatch means the
            // log and the paused turn disagree (a hand-edited or partially
            // written pair), so fail closed rather than continue under totals
            // neither record can prove.
            if let Some(resume) = resume_state {
                validate_segment_resume(resume, started, action_rounds, exploration_rounds)?;
            }
            return Ok(Some((
                SegmentRunState::resumed(
                    &budget,
                    started,
                    normal_action_rounds,
                    normal_exploration_rounds,
                    false,
                ),
                false,
            )));
        }
        if resume_state.is_some() {
            return Err(ClientError::InvalidInput(
                "a paused task segment has no durable segment_started record".to_string(),
            ));
        }

        // The budget is approved at the plan review, after the planning rounds
        // that produced and reviewed the plan. Those rounds are part of the
        // task total, so they become the new segment's baseline and the
        // per-segment counters restart at zero. Otherwise the first segment
        // would begin already at its checkpoint and could never make progress.
        let (prior_action, prior_exploration) = segments
            .last()
            .map(|segment| segment.totals())
            .unwrap_or((0, 0));
        let total_action_rounds = prior_action.saturating_add(action_rounds);
        let total_exploration_rounds = prior_exploration.saturating_add(exploration_rounds);
        let digest =
            self.segment_progress_digest(plan, step_evidence, completed_tool_rounds, turn_notes);
        let started = TaskSegmentStarted {
            format_version: TASK_SEGMENT_FORMAT_VERSION,
            session_id: session.id.clone(),
            task_id: task.id.clone(),
            segment: segments
                .last()
                .map(|segment| segment.segment() + 1)
                .unwrap_or(1),
            segment_action_rounds: 0,
            segment_exploration_rounds: 0,
            total_action_rounds,
            total_exploration_rounds,
            tokens,
            start_digest: digest.clone(),
        };
        self.session_store
            .append_task_segment_started(&session.id, &started)?;
        self.audit_log
            .record("segment_started", &started.audit_fields())?;
        Ok(Some((
            SegmentRunState::resumed(
                &budget,
                &started,
                normal_action_rounds,
                normal_exploration_rounds,
                false,
            ),
            true,
        )))
    }

    /// The bounded pre-plan allowance a worker gets before it proposes a plan.
    ///
    /// The exploration total is the configured proposal bounded by the worker
    /// task-exploration ceiling and the product ceiling. The action total
    /// covers the read rounds that stop counting as exploration once a
    /// segment's allowance is spent; it is never smaller than one ordinary
    /// action segment. Token totals use the product ceilings because there is
    /// no approved task budget yet, while `agent_max_task_tokens` still bounds
    /// the turn exactly as it does today.
    fn preplan_budget(&self) -> TaskExecutionBudget {
        let exploration_rounds = self.config.preplan_exploration_rounds();
        TaskExecutionBudget {
            action_rounds: exploration_rounds.max(self.tool_round_limit(false)),
            exploration_rounds,
            input_tokens: crate::task_budget::ABSOLUTE_TASK_INPUT_TOKEN_CEILING,
            output_tokens: crate::task_budget::ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING,
        }
    }

    /// Starts the first pre-plan segment at its first round boundary.
    ///
    /// The rounds already spent are the new segment's baseline, exactly as the
    /// post-approval first segment treats the planning rounds. That keeps the
    /// durable totals monotonic without inventing a start digest for rounds
    /// whose observation state was never recorded.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_preplan_segment(
        &self,
        session: &Session,
        task: &Task,
        action_rounds: u32,
        exploration_rounds: u32,
        plan: Option<&TaskPlan>,
        step_evidence: &[Evidence],
        completed_tool_rounds: &CompletedToolTracker,
        turn_notes: &TurnNotesState,
        normal_action_rounds: u32,
        normal_exploration_rounds: u32,
    ) -> Result<SegmentRunState> {
        let budget = self.preplan_budget();
        let mut usage_by_task = self.session_store.read_task_usage(&session.id)?;
        let usage = usage_by_task.remove(&task.id);
        let tokens = SegmentTokenTotals::from_usage(usage.as_ref());
        let segments = self
            .session_store
            .read_task_segments(&session.id, &task.id)?;
        let (prior_action, prior_exploration) = segments
            .last()
            .map(|segment| segment.totals())
            .unwrap_or((0, 0));
        let total_action_rounds = prior_action.saturating_add(action_rounds);
        let total_exploration_rounds = prior_exploration.saturating_add(exploration_rounds);
        let digest =
            self.segment_progress_digest(plan, step_evidence, completed_tool_rounds, turn_notes);
        let started = TaskSegmentStarted {
            format_version: TASK_SEGMENT_FORMAT_VERSION,
            session_id: session.id.clone(),
            task_id: task.id.clone(),
            segment: segments
                .last()
                .map(|segment| segment.segment() + 1)
                .unwrap_or(1),
            segment_action_rounds: 0,
            segment_exploration_rounds: 0,
            total_action_rounds,
            total_exploration_rounds,
            tokens,
            start_digest: digest.clone(),
        };
        self.session_store
            .append_task_segment_started(&session.id, &started)?;
        self.audit_log
            .record("segment_started", &started.audit_fields())?;
        Ok(SegmentRunState::resumed(
            &budget,
            &started,
            normal_action_rounds,
            normal_exploration_rounds,
            true,
        ))
    }
}
