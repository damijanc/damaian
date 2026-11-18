use super::*;

impl ChatOrchestrator {
    /// The task's ceiling and recorded spend, or `None` when no ceiling is set.
    ///
    /// One reader for both the pre-call check and the reminder gate: the
    /// reminder must be judged on the spend *after* the answer round, and two
    /// copies of this read could disagree about which figure the ceiling is
    /// compared with (spec 66 `context.md` D12).
    pub(super) fn token_ceiling_spend(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<Option<(u64, u64)>> {
        let Some(ceiling) = self.config.agent_max_task_tokens else {
            return Ok(None);
        };
        let spent = self
            .session_store
            .read_task_usage(session_id)?
            .get(task_id)
            .map(|usage| usage.input_tokens + usage.output_tokens)
            .unwrap_or(0);
        Ok(Some((ceiling, spent)))
    }

    /// Audits and records a compaction that did not run, so the user is told
    /// on the next session load and the audit trail says why. Never fails the
    /// turn: a diagnostics write that itself fails is still not a turn error.
    fn note_compaction_failure(&self, session_id: &str, reason: &str) {
        let _ = self.audit_log.record(
            "conversation_compaction_failed",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("reason", reason.to_string()),
            ],
        );
        let _ = self
            .session_store
            .record_conversation_compaction_failure(session_id, &self.scanner.redact(reason).text);
    }

    /// The conversation window in tokens: `conversation_window_share` of the
    /// effective context budget (spec 55 §5.1). The same number is the
    /// automatic threshold, so a compaction that triggers leaves a conversation
    /// that fits (context.md D2).
    pub fn conversation_window_tokens(&self) -> usize {
        self.config.context_token_budget() * usize::from(self.config.conversation_window_share)
            / 10_000
    }

    /// Summarises the conversation outside the recency window, appending one
    /// `conversation_compacted` event (spec 55).
    ///
    /// Never fails a turn: a model failure produces a facts-only summary, and
    /// only an unreadable log or a failed append returns `Err`, which the
    /// turn-boundary caller treats as "compaction did not run". A failure is
    /// audited and recorded here, once, so the manual route reports it too. The
    /// summarisation call is recorded against a synthetic task id so it appears
    /// in the session's totals without reducing any turn's ceiling (§5.5).
    pub fn compact_conversation(
        &self,
        repository_root: &Path,
        session_id: &str,
        model_adapter: &mut dyn ModelAdapter,
        cancel: &CancelToken,
    ) -> Result<crate::compaction::CompactionOutcome> {
        let outcome = self.compact_conversation_at_boundary(
            repository_root,
            session_id,
            model_adapter,
            cancel,
        );
        // Recorded here rather than by each caller, so a failure is audited and
        // shown to the user whichever route asked: the automatic turn boundary
        // and the manual `POST /api/compact` both go through this method.
        match &outcome {
            Ok(crate::compaction::CompactionOutcome::Failed(reason)) => {
                self.note_compaction_failure(session_id, reason);
            }
            Err(error) => self.note_compaction_failure(session_id, &error.to_string()),
            Ok(
                crate::compaction::CompactionOutcome::Compacted(_)
                | crate::compaction::CompactionOutcome::NothingToCompact
                | crate::compaction::CompactionOutcome::Cancelled,
            ) => {}
        }
        outcome
    }

    fn compact_conversation_at_boundary(
        &self,
        repository_root: &Path,
        session_id: &str,
        model_adapter: &mut dyn ModelAdapter,
        cancel: &CancelToken,
    ) -> Result<crate::compaction::CompactionOutcome> {
        if cancel.is_cancelled() {
            return Ok(crate::compaction::CompactionOutcome::Cancelled);
        }
        let view = self.session_store.read_conversation(session_id)?;
        let previous_through = view
            .summary
            .as_ref()
            .map(|summary| summary.through_seq)
            .unwrap_or(0);
        let messages = self.session_store.read_messages_with_seq(session_id)?;
        // The summary is part of the conversation block and shares its budget
        // (see `build_conversation`). So the span to compact is what does not
        // fit *beside* the summary: with the full window here, a message could
        // fit this check and still be omitted from the request, and the next
        // compaction would never pick it up because it already looks like part
        // of the window.
        let summary_tokens = view
            .summary
            .as_ref()
            .map(|summary| {
                TokenEstimate::Prose.tokens(&self.scanner.redact(&summary.render_for_model()).text)
            })
            .unwrap_or(0);
        let window = self
            .conversation_window_tokens()
            .saturating_sub(summary_tokens);
        let Some(outside) = crate::compaction::window_start(&messages, previous_through, window)
        else {
            return Ok(crate::compaction::CompactionOutcome::NothingToCompact);
        };
        // Requirement 5 and §5.4: never across a pending decision or an action
        // whose outcome is not known.
        let through_seq = outside.min(self.session_store.compaction_limit(session_id)?);
        if through_seq <= previous_through {
            return Ok(crate::compaction::CompactionOutcome::NothingToCompact);
        }

        let plans = self.session_store.read_session_plans(session_id)?;
        let patch_decisions = self
            .session_store
            .read_all_patch_file_decisions(session_id)?;
        let approvals = self
            .session_store
            .read_conversation_approvals_through(session_id, through_seq)?;
        let verification = self
            .session_store
            .read_verification_events(session_id, &[])?;
        let findings = self
            .session_store
            .read_findings(session_id, repository_root)?;
        // A second summary describes the whole conversation. The fact reader
        // below reads the active log up to the new boundary, which already
        // includes the first summary's span — the original objective, files,
        // steps, approvals and validation are re-derived there rather than
        // accumulated from the previous summary. That is what requirement 6
        // asks for; the previous summary still reaches the model, as the input
        // to the narrative half (`narrative_prompt`), which owns the part the
        // log cannot record.
        let Some(facts) = crate::compaction::facts_for_span(
            crate::compaction::FactInputs {
                messages: &messages,
                plans: &plans,
                patch_decisions: &patch_decisions,
                approvals: &approvals,
                verification: &verification,
                findings: &findings,
            },
            through_seq,
        ) else {
            return Ok(crate::compaction::CompactionOutcome::NothingToCompact);
        };

        // Only the covered span is material. The previous summary travels
        // separately as an input to the next one (proposal §5.3).
        let transcript_messages: Vec<(String, String)> = messages
            .iter()
            .filter(|(seq, _)| *seq > previous_through && *seq <= through_seq)
            .map(|(_, message)| (message.role.clone(), message.content.clone()))
            .collect();
        let transcript = crate::compaction::transcript_for_summary(&transcript_messages);
        // The transcript and the previous summary are stored verbatim; the
        // summarisation call is another boundary they must cross redacted.
        let prompt = self
            .scanner
            .redact(&crate::compaction::narrative_prompt(
                &transcript,
                view.summary.as_ref(),
            ))
            .text;
        if cancel.is_cancelled() {
            return Ok(crate::compaction::CompactionOutcome::Cancelled);
        }
        let mut on_token = |_token: &str| {};
        let mut on_wait = |_millis: u64| {};
        let request = ModelRequest {
            provider: self.config.model_provider.clone(),
            model: self.config.model_name.clone(),
            messages: vec![ModelMessage::user(prompt)],
            temperature: None,
            reasoning_level: None,
            stream: false,
            tools: None,
            max_tokens: self
                .config
                .model_provider_config(&self.config.model_provider)
                .and_then(|provider| provider.max_output_tokens),
            request_usage: true,
            emit_cache_breakpoints: false,
        };
        let run = model_adapter.stream_response(&request, cancel, &mut on_token, &mut on_wait);
        if cancel.is_cancelled() {
            return Ok(crate::compaction::CompactionOutcome::Cancelled);
        }
        // Review fix 4b: the previous summary already holds model-written
        // constraints, decisions, failed approaches and uncertainty. When this
        // call cannot produce a new narrative, those are still true unless the
        // conversation says otherwise, so they travel into the facts-only
        // summary rather than disappearing. `narrative_carried_forward` keeps
        // the failure visible even though the four lists are no longer empty.
        let previous_narrative = view.summary.as_ref().map(ConversationSummary::narrative);
        let carried_forward = previous_narrative
            .as_ref()
            .is_some_and(|narrative| !narrative.is_empty());
        let narrative = match &run {
            Ok(run) => match crate::compaction::parse_narrative(&run.content, &self.scanner) {
                Some(narrative) => narrative,
                None => {
                    let _ = self.audit_log.record(
                        "conversation_compaction_narrative_failed",
                        &[
                            ("actor", "system".to_string()),
                            ("sessionId", session_id.to_string()),
                            ("reason", "the reply was not a usable narrative".to_string()),
                            ("throughSeq", through_seq.to_string()),
                        ],
                    );
                    previous_narrative.unwrap_or_default()
                }
            },
            Err(error) => {
                let _ = self.audit_log.record(
                    "conversation_compaction_narrative_failed",
                    &[
                        ("actor", "system".to_string()),
                        ("sessionId", session_id.to_string()),
                        ("reason", error.to_string()),
                        ("throughSeq", through_seq.to_string()),
                    ],
                );
                previous_narrative.unwrap_or_default()
            }
        };
        let model_run_id = run.as_ref().ok().map(|run| run.run_id.clone());
        let summary = crate::compaction::ConversationSummary {
            id: create_id("summary"),
            through_seq,
            format_version: crate::compaction::SUMMARY_FORMAT_VERSION,
            objective: facts.objective,
            files_changed: facts.files_changed,
            plan_steps: facts.plan_steps,
            approvals: facts.approvals,
            validation: facts.validation,
            constraints: narrative.constraints,
            decisions: narrative.decisions,
            failed_approaches: narrative.failed_approaches,
            uncertainty: narrative.uncertainty,
            narrative_carried_forward: carried_forward,
            model_run_id: model_run_id.clone(),
            message_count: facts.message_count,
        };

        if let Ok(run) = &run {
            // A synthetic task id keeps the call in the session's totals while
            // keeping it out of every turn's ceiling: the mechanism for
            // continuing a long task must not make the ceiling arrive sooner
            // (proposal §5.5).
            let _ = self.session_store.record_task_usage_for_task_id(
                session_id,
                &format!("compaction_{}", summary.id),
                Some(CallModel::configured(&self.config)),
                &run.run_id,
                None,
                run.usage,
                run.reported_cost,
                Some("conversation_compaction"),
            );
        }
        if let Err(error) = self
            .session_store
            .append_conversation_compaction(session_id, &summary)
        {
            return Ok(crate::compaction::CompactionOutcome::Failed(
                error.to_string(),
            ));
        }
        let usage = run.as_ref().ok().map(|run| run.usage);
        self.audit_log.record(
            "conversation_compacted",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("summaryId", summary.id.clone()),
                ("throughSeq", through_seq.to_string()),
                ("formatVersion", summary.format_version.to_string()),
                ("messageCount", summary.message_count.to_string()),
                (
                    "modelRunId",
                    model_run_id.clone().unwrap_or_else(|| "none".to_string()),
                ),
                ("factsOnly", summary.is_facts_only().to_string()),
                (
                    "narrativeCarriedForward",
                    summary.narrative_carried_forward.to_string(),
                ),
                (
                    "inputTokens",
                    usage
                        .map(|usage| usage.input_tokens.to_string())
                        .unwrap_or_else(|| "0".to_string()),
                ),
                (
                    "outputTokens",
                    usage
                        .map(|usage| usage.output_tokens.to_string())
                        .unwrap_or_else(|| "0".to_string()),
                ),
            ],
        )?;
        Ok(crate::compaction::CompactionOutcome::Compacted(Box::new(
            summary,
        )))
    }
}
