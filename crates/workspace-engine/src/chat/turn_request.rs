use super::*;

impl ChatOrchestrator {
    /// Seeds a new turn with the previous turn's final notes (spec 65 §5.5).
    ///
    /// "Previous" is the turn before this one in the conversation as the active
    /// log stands, which is the same ordering `carry_plan_from_the_previous_turn`
    /// uses. That makes a rewind correct without a second rule: everything
    /// after the newest marker is already invisible to `read_tasks` and to
    /// `read_turn_notes`.
    ///
    /// The carry is written as a `turn_notes` event with `carried: true`
    /// before the turn's first request, and audited with its size only. With
    /// the bound at `0`, nothing is read, written or carried.
    pub(super) fn carry_notes_from_the_previous_turn(
        &self,
        session_id: &str,
        task: &Task,
    ) -> Result<TurnNotesState> {
        if self.config.agent_max_turn_notes_chars == 0 {
            return Ok(TurnNotesState::default());
        }
        let tasks = self.session_store.read_tasks(session_id)?;
        let Some(previous) = tasks.iter().rev().find(|candidate| candidate.id != task.id) else {
            return Ok(TurnNotesState::default());
        };
        let notes = self.session_store.read_turn_notes(session_id)?;
        let Some(previous_notes) = notes.get(&previous.id) else {
            return Ok(TurnNotesState::default());
        };
        if previous_notes.text.is_empty() {
            return Ok(TurnNotesState::default());
        }
        // Read back through the scanner as well: the log is trusted only as
        // far as every other boundary trusts it, and a note is model-written
        // text that may quote what was read (requirement 9).
        let text = self.scanner.redact(&previous_notes.text).text;
        self.session_store.record_turn_notes(
            session_id,
            &task.id,
            false,
            true,
            &text,
            text.chars().count(),
        )?;
        self.audit_log.record(
            "turn_notes_carried",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("taskId", task.id.clone()),
                ("sourceTaskId", previous.id.clone()),
                ("chars", text.chars().count().to_string()),
            ],
        )?;
        Ok(TurnNotesState {
            text,
            carried: true,
            calls: Vec::new(),
        })
    }

    /// Rebuilds a turn's call list from the tool calls in its saved messages
    /// (spec 65 §5.6). The pending file is the normal source; this is the
    /// fallback for a turn whose notes survived but whose call list did not,
    /// and the shape a task resumed from the log alone would use.
    pub(super) fn rebuild_call_list(
        &self,
        repository_root: &Path,
        messages: &[ModelMessage],
    ) -> Vec<RecordedCall> {
        let mut calls = Vec::new();
        let mut round = 0u32;
        for (message_index, message) in messages.iter().enumerate() {
            if message.role != "assistant" || message.tool_calls.is_empty() {
                continue;
            }
            round += 1;
            for call in &message.tool_calls {
                let Ok(Some(action)) = tool_action_from_call(call) else {
                    continue;
                };
                let line = self
                    .render_call_line(repository_root, &action)
                    .map(|line| {
                        truncate_for_prompt(
                            &single_line(&self.scanner.redact(&line).text),
                            CALL_LINE_MAX_CHARS,
                        )
                    })
                    .unwrap_or_default();
                calls.push(RecordedCall {
                    round,
                    message_index,
                    line,
                    key: self.call_key(repository_root, &action),
                    writes: matches!(action_effect(&action), ActionEffect::WritesRepository),
                    compacted: false,
                });
            }
        }
        calls
    }

    /// One request selected for the next model call, plus what Task 4 did to it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_turn_request(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        messages: &[ModelMessage],
        turn_notes: &TurnNotesState,
        completed_tool_rounds: &CompletedToolTracker,
        plan: Option<&TaskPlan>,
        step_evidence: &[Evidence],
        protected: &TaskProtectedPaths,
        tools: Option<Vec<ToolDefinition>>,
        max_tokens: Option<u32>,
    ) -> Result<TurnRequestDecision> {
        let notes_window = if self.config.agent_max_turn_notes_chars == 0 {
            // Requirement 3: zero restores today's request exactly.
            NotesWindow::none()
        } else {
            NotesWindow {
                notes: &turn_notes.text,
                carried: turn_notes.carried,
                calls: &turn_notes.calls,
                enabled: true,
                notes_limit: self.config.agent_max_turn_notes_chars,
                notice_cap: ELISION_NOTICE_LINE_CAP,
                warning_cap: PRE_CUT_LINE_CAP,
            }
        };
        // The legacy message-count window is exactly what today's request would
        // send. The count is only needed when it will actually cut a round: a
        // turn below it is never elided, so nothing can be dropped and the
        // request stays today's byte for byte.
        //
        // The count cut is the fallback for a model whose context window is
        // unknown. With a known window, the window share bounds the turn: the
        // compactor below turns old results into short manifests the model can
        // act on, where the count cut drops whole rounds. Dropping rounds well
        // below the window made a turn that had to read a large file forget
        // its own reads and read them again indefinitely (spec 33 Task 1,
        // 2026-11-10: 60 reads of one file in 64 rounds at 4% of the window).
        let context_window = self.config.context_window();
        let message_budget = if context_window.is_some() {
            usize::MAX
        } else {
            self.config.agent_max_turn_messages
        };
        let cut_needed = messages.len() > message_budget;
        let legacy_request = self.request_for(
            bounded_messages(messages, message_budget, &notes_window),
            tools.clone(),
            max_tokens,
        );
        let threshold = context_window
            .map(|window| request_threshold_tokens(window, self.config.conversation_window_share));
        let request_tokens_before =
            TokenEstimate::Prose.tokens(&model_request_json(&legacy_request));
        let crosses_threshold = threshold.is_some_and(|limit| request_tokens_before > limit);
        if !cut_needed && !crosses_threshold {
            return Ok(TurnRequestDecision::prepared(legacy_request));
        }

        // Either the legacy count cut would elide a round, or the request
        // crosses the window share. Both decisions obey the same pin set: the
        // count bound applies only to unpinned completed rounds and never drops
        // the approved plan and budget, an unresolved failure, or the latest
        // result for an open step. `within_turn_pins` is the single definition
        // of that set for this request path and the compactor.
        let mut pins = self.within_turn_pins(
            repository_root,
            &session.id,
            completed_tool_rounds,
            plan,
            step_evidence,
            protected,
        )?;
        // A native call the tracker cannot describe — a resumed dispatch from
        // an older pending turn, or a call whose typed metadata was lost — is
        // pinned rather than guessed at. Its message structure is read; its
        // prose is not.
        let tracked_ids = completed_tool_rounds.completed_call_ids();
        for message in messages {
            for call in &message.tool_calls {
                if !tracked_ids.contains(call.id.as_str()) {
                    pins.insert(call.id.clone());
                }
            }
        }
        let Some(context_window) = context_window else {
            return Ok(TurnRequestDecision::prepared(self.request_for(
                bounded_messages_preserving_safety(messages, message_budget, &notes_window, &pins),
                tools,
                max_tokens,
            )));
        };
        // With a known window the full history is tried first, and only a
        // request the compactor cannot fit falls back to the count cut, which
        // drops whole unpinned rounds. A very long turn in a small window
        // stays bounded rather than stopping `CannotFit` (the R7 command-heavy
        // case), while a turn below the share keeps every round.
        let fallback = self.config.agent_max_turn_messages;
        let budgets = if message_budget == fallback {
            vec![message_budget]
        } else {
            vec![message_budget, fallback]
        };
        for (index, budget) in budgets.iter().copied().enumerate() {
            let last = index + 1 == budgets.len();
            let safety_request = self.request_for(
                bounded_messages_preserving_safety(messages, budget, &notes_window, &pins),
                tools.clone(),
                max_tokens,
            );
            // `request_tokens_before` measured the more aggressively cut legacy
            // view. Keeping pinned rounds can push a count-cut turn over the
            // share even when that legacy view stayed under it, so the window
            // decision is made again on the request that will actually be sent.
            // `prepare_request` returns `Unchanged` below the share, which
            // sends exactly this selection and writes no attempt. Only
            // completed rounds still present in the selected request may be
            // described to the reducer: a round the safety selection elided has
            // no declaration or result to pair with.
            let rounds = present_completed_rounds(
                &safety_request.messages,
                &completed_tool_rounds.as_compaction_rounds(),
            );
            let outcome = prepare_request(&RequestPreparationInput {
                request: &safety_request,
                completed_rounds: &rounds,
                pinned_call_ids: &pins,
                context_window: Some(context_window),
                window_share_bps: self.config.conversation_window_share,
                scanner: &self.scanner,
                session_id: &session.id,
                task_id: &task.id,
            });
            match outcome {
                RequestPreparationOutcome::Unchanged => {
                    return Ok(TurnRequestDecision::prepared(safety_request));
                }
                RequestPreparationOutcome::Compacted { request, attempt } => {
                    self.record_within_turn_compaction_attempt(&attempt)?;
                    let compacted_call_ids = attempt
                        .facts
                        .iter()
                        .map(|fact| fact.call_id.clone())
                        .collect();
                    return Ok(TurnRequestDecision::Prepared(PreparedTurnRequest {
                        request,
                        compacted_call_ids,
                    }));
                }
                // The full history could not fit even compacted: try the count
                // cut before giving up. Nothing is recorded for the attempt
                // the fallback replaces, so the log shows only what was sent.
                RequestPreparationOutcome::FailedClosed { attempt }
                    if !last
                        && attempt.outcome
                            == crate::request_preparation::WithinTurnCompactionAttemptOutcome::CannotFit =>
                {
                    continue;
                }
                RequestPreparationOutcome::FailedClosed { attempt } => {
                    self.record_within_turn_compaction_attempt(&attempt)?;
                    return match attempt.outcome {
                        // A request that cannot fit is a typed terminal outcome,
                        // never a generic client error. It carries no request
                        // and the caller settles it without sending a model call
                        // or writing a segment boundary.
                        crate::request_preparation::WithinTurnCompactionAttemptOutcome::CannotFit => {
                            Ok(TurnRequestDecision::CannotFit(attempt))
                        }
                        // Invalid metadata stays fail-closed: a caller cannot
                        // act on it as if the window were too small.
                        _ => Err(ClientError::RequestPreparation(
                            "the completed tool metadata does not match the request messages"
                                .to_string(),
                        )),
                    };
                }
            }
        }
        unreachable!("the last budget always returns")
    }

    /// Adds one call dispatched by a resume path to the paused turn's tracker.
    ///
    /// The resume paths execute an approved command, answer a question or make
    /// an approved fetch outside the loop, so their typed result has to join the
    /// tracker before the resumed turn prepares its next request.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn tracker_with_resumed_call(
        &self,
        session_id: &str,
        task_id: &str,
        completed_tool_rounds: &[TrackedCompletedToolRound],
        matched_tool_call: Option<&ToolCall>,
        repository_root: &Path,
        action: &ToolAction,
        outcome: &ActionOutcome,
    ) -> Result<CompletedToolTracker> {
        let mut tracker = CompletedToolTracker::from(completed_tool_rounds.to_vec());
        let Some(call) = matched_tool_call else {
            return Ok(tracker);
        };
        let open_step_id = self
            .session_store
            .read_task_plan(session_id, task_id)?
            .and_then(|current| {
                current
                    .steps
                    .iter()
                    .find(|step| step.status == StepStatus::InProgress)
                    .map(|step| step.id.clone())
            });
        let read_key = self.call_key(repository_root, action);
        if let Some(tracked) = tracked_completed_call(call, action, outcome, open_step_id, read_key)
        {
            tracker.push_round(vec![tracked]);
        }
        Ok(tracker)
    }

    fn request_for(
        &self,
        messages: Vec<ModelMessage>,
        tools: Option<Vec<ToolDefinition>>,
        max_tokens: Option<u32>,
    ) -> ModelRequest {
        ModelRequest {
            provider: self.config.model_provider.clone(),
            model: self.config.model_name.clone(),
            messages,
            temperature: Some("0".to_string()),
            reasoning_level: Some(self.config.model_reasoning_level.clone()),
            stream: true,
            tools,
            // A budgeted caller supplies the already-approved cap; every other
            // caller keeps the configured provider/model ceiling.
            max_tokens: max_tokens.or_else(|| self.config.max_output_tokens()),
            request_usage: self.config.provider_reports_usage(),
            emit_cache_breakpoints: self.config.supports_explicit_cache_breakpoints(),
        }
    }

    /// Everything the next request must not lose, derived only from typed
    /// engine state: the latest result for every open plan step (any tool kind),
    /// an unresolved failed result, an unknown external outcome, protected paths
    /// and the paths whose file hashes a later patch or finding check rests on,
    /// and active failure findings.
    ///
    /// A successful completed round that is not one of these is bounded by the
    /// message count and eligible for a compaction manifest. Historical
    /// `FileRead` evidence deliberately does not pin its result body. The
    /// evidence keeps its path and hash for the durable plan, and the newest
    /// read for an open step stays pinned below, so an older completed read is
    /// still eligible for a compaction manifest in a planned task.
    fn within_turn_pins(
        &self,
        repository_root: &Path,
        session_id: &str,
        tracker: &CompletedToolTracker,
        plan: Option<&TaskPlan>,
        step_evidence: &[Evidence],
        protected: &TaskProtectedPaths,
    ) -> Result<BTreeSet<String>> {
        let mut pins = BTreeSet::new();
        let mut safety_paths: BTreeSet<String> = BTreeSet::new();

        if let Some(plan) = plan {
            for step in &plan.steps {
                for evidence in &step.evidence {
                    // Requirement 6: the paths whose file hashes a later patch
                    // or finding safety check rests on.
                    collect_safety_path(evidence, &mut safety_paths);
                }
                if !step.status.is_terminal()
                    && let Some(call_id) = tracker.latest_call_id_for_step(&step.id)
                {
                    // Requirement 6: the latest result for every open step,
                    // whatever its tool kind.
                    pins.insert(call_id.to_string());
                }
            }
        }
        for evidence in step_evidence {
            collect_safety_path(evidence, &mut safety_paths);
        }

        for finding in self
            .session_store
            .read_findings(session_id, repository_root)?
        {
            if finding.status() != FindingStatus::Open {
                continue;
            }
            if finding.severity() == Severity::Error
                && let Some(origin) = finding.origin_ref()
            {
                // Requirement 6/8: an active failure finding keeps its origin
                // result, which is the evidence the model can still reason
                // about.
                pins.insert(origin.to_string());
            }
            if finding.file_hash().is_some()
                && let Some(range) = finding.range()
            {
                // Requirement 6: findings with a file hash keep their path.
                safety_paths.insert(range.path.clone());
            }
        }

        let calls: Vec<&TrackedCompletedToolCall> = tracker.calls().collect();
        for (index, call) in calls.iter().enumerate() {
            // Requirement 8: an unknown external outcome may still have
            // happened, so it is never dropped.
            if call.call.outcome == ToolResultOutcome::UnknownExternalOutcome {
                pins.insert(call.call.call_id.clone());
                continue;
            }

            // Requirement 8: a failed result stays while its failure is
            // unresolved. An open Error finding pins its origin separately
            // above. Otherwise the failure is resolved once the same kind and
            // identity succeeds later, or once its step closes; a `Blocked` or
            // still-running step with no later success keeps it pinned. Without
            // a step there is no durable resolution signal, so keep it.
            if !call.call.outcome.is_eligible() {
                let step_closed = call
                    .plan_step_id
                    .as_deref()
                    .and_then(|step_id| {
                        plan.and_then(|plan| {
                            plan.steps
                                .iter()
                                .find(|step| step.id == step_id)
                                .map(|step| {
                                    matches!(
                                        step.status,
                                        StepStatus::Completed | StepStatus::Skipped
                                    )
                                })
                        })
                    })
                    .unwrap_or(false);
                let later_same_success = calls[index + 1..].iter().any(|later| {
                    later.call.tool_kind == call.call.tool_kind
                        && later.call.identity == call.call.identity
                        && later.call.outcome.is_eligible()
                });
                if !step_closed && !later_same_success {
                    pins.insert(call.call.call_id.clone());
                    continue;
                }
            }

            // Requirement 6: protected paths, and the paths whose file hashes a
            // later patch or finding safety check rests on. This covers a read
            // and any other result whose typed identity names a path.
            if call.call.identity.kind == crate::within_turn_compaction::ToolIdentityKind::Path
                && (protected.covering_path(&call.call.identity.value).is_some()
                    || safety_paths.contains(&call.call.identity.value))
            {
                pins.insert(call.call.call_id.clone());
                continue;
            }

            // Requirement 6: the latest result for every open step is pinned
            // above, whatever its kind. Every other completed round — including
            // a successful command, patch or `complete_step` — is bounded by the
            // message count and eligible for a manifest. A later patch proposal
            // hashes the current file when it is created; its base hashes come
            // from the patch store and durable `PatchApplied` evidence, not from
            // an old result body.
        }

        Ok(pins)
    }

    fn record_within_turn_compaction_attempt(
        &self,
        attempt: &WithinTurnCompactionAttempt,
    ) -> Result<()> {
        self.session_store
            .append_within_turn_compaction_attempt(&attempt.session_id, attempt)?;
        self.audit_log
            .record("within_turn_compaction", &attempt.audit_fields())?;
        Ok(())
    }
}
