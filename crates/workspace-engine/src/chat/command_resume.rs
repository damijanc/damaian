use super::*;

impl ChatOrchestrator {
    /// Continues a chat turn that stopped to ask the user whether a proposed
    /// command may run. Executes (or rejects) the command, feeds the result
    /// back to the model, and lets the agentic loop keep going from there —
    /// previously approving a risky command just ran it in isolation and the
    /// model never got to use the result to answer the user's question.
    pub fn resume_after_command_decision(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        self.resume_after_command_decision_with_options(
            proposal_id,
            approved,
            approved_by,
            model_adapter,
            sink,
            ResumeDecisionOptions::default(),
        )
    }

    pub fn resume_after_command_decision_with_options(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
        decision_options: ResumeDecisionOptions,
    ) -> Result<ChatTurnResult> {
        // Spec 109 D5: the paused turn's session, for the approvals the
        // command policy adds and the backstop around a covered run.
        let Some(session_id) = PausedTurns::new(&self.config.data_dir).session_id(proposal_id)
        else {
            return self.resume_after_command_decision_in_scope(
                proposal_id,
                approved,
                approved_by,
                model_adapter,
                sink,
                decision_options,
            );
        };
        in_session_scope(&session_id, || {
            self.resume_after_command_decision_in_scope(
                proposal_id,
                approved,
                approved_by,
                model_adapter,
                sink,
                decision_options,
            )
        })
    }

    fn resume_after_command_decision_in_scope(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
        decision_options: ResumeDecisionOptions,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(proposal_id)?;
        let turn_notes = TurnNotesState::from_pending(&pending);
        // Every pause shares the pending-turn file, and each resume loads
        // whatever file its id names. A plan review, a provider fallback, a
        // round-budget pause and a question each have their own resume;
        // falling through to the shell-command branch here would run
        // `last_content` as a command or fail with a misleading error after
        // deleting the pause. Refused loudly instead, with the file put back
        // so the right resume can still find it (spec 50 `context.md` §2).
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::Action)?;
        let repository_root = PathBuf::from(&pending.repository_root);
        let mut pending = pending;
        let mut messages = std::mem::take(&mut pending.messages);
        // Layer 3's resume-path points (`context.md` §8): approving a paused
        // action is a new decision, so it answers to the session's mode now,
        // not the mode it was requested under — the user may have switched
        // since. Read once, for whichever branch below runs; a decline needs
        // no check, since nothing is about to happen either way.
        let mode = self.session_store.session_mode(&pending.session.id);
        // The profile is captured the same way: this engine loaded config for
        // this request, so it is the selection now (spec 31 `context.md` §6).
        let capabilities = self.config.profile_capabilities();

        // Three kinds of paused action resume through here: a browser
        // diagnostic, an MCP tool call, or the original shell-command path.
        // A resumed call that actually dispatches is followed by `after_tool`;
        // a decline or a refusal dispatches nothing, so there is no event.
        let mut after_tool_action: Option<ToolAction> = None;
        let mut after_tool_outcome: Option<ActionOutcome> = None;
        let mut after_tool_termination: Option<CommandTermination> = None;
        // The evidence the paused turn was already carrying. The approved
        // command's own exit is added to it below (spec 66 §5.3); everything
        // else resumes with exactly what the pause saved.
        let mut step_evidence = pending.step_evidence.clone();
        let (assistant_summary, mut tool_result_content) = if let Some(web_call) =
            &pending.web_diagnostic_call
        {
            let call = web_call.call.clone();
            let summary = web_diagnostic_summary(&call);
            let permission = permits(
                mode,
                &capabilities,
                &ToolAction::WebDiagnostic(call.clone()),
                None,
                None,
            );
            let refused = approved && !permission.is_allowed();
            // A refused approval grants nothing, including session-wide
            // consent: that consent governs diagnostics within a mode and
            // profile that allow them, and this one does not.
            let decision = if refused {
                match permission {
                    Permission::RefusedByProfile { .. } => "refused_by_profile",
                    _ => "refused_by_mode",
                }
            } else if approved
                && decision_options.allow_browser_diagnostics_for_session
                && call.targets_loopback()
            {
                // A remote proposal never offers the session grant, so a
                // request for one here is approved once instead.
                self.session_store
                    .allow_browser_diagnostics_for_session(&pending.session.id, approved_by)?;
                "approved_for_session"
            } else if approved {
                "approved_once"
            } else {
                "denied"
            };
            self.audit_log.record(
                "browser_diagnostic_approval_decision",
                &[
                    ("actor", approved_by.to_string()),
                    ("sessionId", pending.session.id.clone()),
                    ("taskId", pending.task.id.clone()),
                    ("proposalId", proposal_id.to_string()),
                    ("decision", decision.to_string()),
                    ("tool", call.name().to_string()),
                    ("url", call.url.clone()),
                ],
            )?;
            let content = if refused {
                refusal_message(permission)
            } else if approved {
                let (content, failed) =
                    self.run_and_record_web_diagnostic(&repository_root, &call, sink)?;
                after_tool_action = Some(ToolAction::WebDiagnostic(call.clone()));
                after_tool_outcome = Some(if failed {
                    ActionOutcome::Failed
                } else {
                    ActionOutcome::Ok
                });
                content
            } else {
                format!(
                    "The user declined to run `{}` against `{}`. Do not request the same browser diagnostic again; answer using what you already know, noting the limitation if it matters.",
                    call.name(),
                    call.url
                )
            };
            (summary, content)
        } else if let Some(mcp_call) = &pending.mcp_call {
            let summary = mcp_call_summary(&mcp_call.server_id, &mcp_call.tool_name);
            let content = if approved {
                let mut mcp =
                    self.build_mcp_runtime(&pending.session.id, &pending.session.repository_id);
                // A fresh runtime has fetched no tool lists, and the read-only
                // hint lives in them; without this every hint reads as `None`.
                // Only the called tool's server is listed (spec 33 §5.1).
                mcp.list_server(&mcp_call.server_id);
                let permission = permits(
                    mode,
                    &capabilities,
                    &ToolAction::McpCall {
                        server_id: mcp_call.server_id.clone(),
                        tool_name: mcp_call.tool_name.clone(),
                        arguments_json: String::new(),
                    },
                    None,
                    mcp.tool_read_only_hint(&mcp_call.server_id, &mcp_call.tool_name),
                );
                if !permission.is_allowed() {
                    refusal_message(permission)
                } else {
                    let (content, outcome) = match mcp.call_tool(
                        &mcp_call.server_id,
                        &mcp_call.tool_name,
                        &mcp_call.arguments_json,
                    ) {
                        Ok(result) => {
                            let text = self.scanner.redact(&result.text).text;
                            if result.is_error {
                                (
                                    format!("MCP tool reported an error:\n{text}"),
                                    ActionOutcome::Failed,
                                )
                            } else {
                                (text, ActionOutcome::Ok)
                            }
                        }
                        // Spec 109 D8: stopped part-way, not failed.
                        Err(ClientError::Cancelled) => (
                            "The user stopped this MCP call; its outcome is unknown.".to_string(),
                            ActionOutcome::Cancelled,
                        ),
                        Err(error) => (
                            format!("MCP tool call failed: {error}"),
                            ActionOutcome::Failed,
                        ),
                    };
                    after_tool_action = Some(ToolAction::McpCall {
                        server_id: mcp_call.server_id.clone(),
                        tool_name: mcp_call.tool_name.clone(),
                        arguments_json: mcp_call.arguments_json.clone(),
                    });
                    after_tool_outcome = Some(outcome);
                    content
                }
            } else {
                format!(
                    "The user declined to run the MCP tool `{}` on server `{}`. Do not request it again; answer using what you already know, noting the limitation if it matters.",
                    mcp_call.tool_name, mcp_call.server_id
                )
            };
            (summary, content)
        } else {
            let proposal = self.command_store.load_proposal(proposal_id)?;
            let command_request = CommandRequest {
                command: proposal.command.clone(),
                reason: proposal.reason.clone(),
                working_directory: None,
            };
            // The stored command is what `run_proposal` would execute, classified
            // again under the current config, because the stored fields predate
            // any profile or config change since (spec 31 Task 5, deviation 5).
            // Classified at the proposal's own pair, so a command in a sub-root
            // is judged with the repository as its boundary (spec 24 Task 7).
            let classification = self.validation_orchestrator.classify_command_at(
                Path::new(&proposal.repository_root),
                Path::new(&proposal.working_directory),
                &proposal.command,
            );
            let permission = permits(
                mode,
                &capabilities,
                &ToolAction::Command(command_request.clone()),
                Some(&classification),
                None,
            );
            let content = if approved && !permission.is_allowed() {
                // Marked rejected like a decline, so a later run of this id by
                // `/api/run-command` shows up as a violation (`context.md` §9).
                // The audit event still says `actor: user`; `rejectedBy` does not.
                let rejected_by = match permission {
                    Permission::RefusedByProfile { .. } => "profile_policy",
                    _ => "mode_policy",
                };
                self.validation_orchestrator
                    .reject_proposal(proposal_id, rejected_by)?;
                refusal_message(permission)
            } else if approved {
                // The census has to be taken before the command runs: once it
                // has, the pre-command bytes are gone.
                let census = self
                    .checkpoint_store
                    .begin_command_census(&repository_root)
                    .unwrap_or_else(|_| CommandCensus::unavailable());
                let max_rounds = self.tool_round_limit(false);
                let max_exploration_rounds = self.default_exploration_round_limit();
                // Spec 107 requirement 3: the status line names the command
                // while it runs, not the proposal it was before approval.
                sink.phase(
                    PhaseKind::Tool,
                    running_command_label(&proposal.command),
                    pending.round,
                    max_rounds,
                    pending.exploration_rounds,
                    max_exploration_rounds,
                    pending.turn_notes.chars().count(),
                );
                let cancel = sink.cancel;
                // Borrow only the progress field, so `cancel` can be read for
                // the same call without two conflicting borrows of the sink.
                let on_progress = &mut *sink.on_progress;
                let mut on_output = |line: &str| {
                    on_progress(TurnProgress::Phase(TurnPhase::new(
                        PhaseKind::Output,
                        line,
                        pending.round,
                        max_rounds,
                        pending.exploration_rounds,
                        max_exploration_rounds,
                        pending.turn_notes.chars().count(),
                    )));
                };
                // Spec 109 D8: the approved run gets its own marker (the one
                // before the pause finished `awaiting_approval`), so a Stop
                // part-way — or a crash — leaves its outcome `unknown`, and
                // the stopped turn can name it.
                let (marker_action, marker_ref, marker_side_effecting) =
                    tool_action_marker(&ToolAction::Command(command_request.clone()));
                let run_marker = self.session_store.start_action(
                    &pending.task,
                    marker_action,
                    &marker_ref,
                    marker_side_effecting,
                )?;
                let record = match self.validation_orchestrator.run_proposal(
                    proposal_id,
                    true,
                    approved_by,
                    Some(&pending.task.id),
                    cancel,
                    &mut on_output,
                ) {
                    Ok(record) if record.execution.termination == CommandTermination::Cancelled => {
                        self.session_store
                            .finish_action_stopped_by_user(run_marker)?;
                        record
                    }
                    Ok(record) => {
                        self.session_store
                            .finish_command_action(run_marker, record.execution.exit_code)?;
                        record
                    }
                    Err(error) => {
                        self.session_store.finish_action(run_marker, "failed")?;
                        return Err(error);
                    }
                };
                // Recorded, but no evidence: this path attaches no
                // `CommandExit` either (spec 22 `context.md` §14).
                self.record_command_findings(
                    &repository_root,
                    &pending.session,
                    &pending.task,
                    &record.execution,
                )?;
                let changed = self.record_command_effects(
                    &repository_root,
                    &pending.session,
                    &pending.task,
                    &census,
                );
                self.record_command_verification_due(
                    &pending.session.id,
                    &pending.task.id,
                    changed,
                )?;
                // Spec 55 requirement 6: the summary's approval digest is read
                // from the log, and a shell-command decision is not otherwise
                // there — the audit log carries it with no session id.
                self.session_store.record_conversation_approval(
                    &pending.session.id,
                    "command",
                    proposal_id,
                    "approved",
                    Some(&command_request.command),
                )?;
                // The command the user approved is evidence for the step it
                // served. The execution's own id is the breadcrumb, as it was
                // before the run had a marker of its own — it is the
                // `commandId` on the `stored_command_executed`
                // record that carries the exit (spec 66 `context.md` D14).
                // The directory comes from the execution, not the proposal,
                // because that is where the runner actually ran it.
                let command_directory = relative_location(
                    &repository_root,
                    Path::new(&record.execution.working_directory),
                )
                .unwrap_or_else(|| record.execution.working_directory.clone());
                if let Some(evidence) = evidence_for(
                    &ActionOutcome::CommandExit(
                        record.execution.exit_code,
                        record.execution.purpose,
                    ),
                    &record.execution.id,
                    Some(&command_request.command),
                    Some(&command_directory),
                ) {
                    step_evidence.push(evidence);
                }
                after_tool_action = Some(ToolAction::Command(command_request.clone()));
                after_tool_outcome = Some(ActionOutcome::CommandExit(
                    record.execution.exit_code,
                    record.execution.purpose,
                ));
                after_tool_termination = Some(record.execution.termination);
                command_context_with_revocation(&record)
            } else {
                self.validation_orchestrator
                    .reject_proposal(proposal_id, approved_by)?;
                self.session_store.record_conversation_approval(
                    &pending.session.id,
                    "command",
                    proposal_id,
                    "rejected",
                    Some(&command_request.command),
                )?;
                // Spec 105 review fix R8: the explicit decline is durable plan
                // evidence so the completion gate can match it by `CheckId`.
                // Only this branch records it: a policy refusal above, a
                // timeout, or the model never requesting the command is not a
                // waiver. The directory comes from the proposal, which is where
                // the command would have run, normalized like a `CheckId`.
                let decline_directory =
                    relative_location(&repository_root, Path::new(&proposal.working_directory))
                        .unwrap_or_else(|| proposal.working_directory.clone());
                step_evidence.push(Evidence::CommandDeclined {
                    proposal_id: proposal.id.clone(),
                    command_hash: Some(command_hash(&proposal.command)),
                    working_directory: decline_directory,
                });
                format!(
                    "The user declined to run `{}`. Do not request it again; answer using what you already know, noting the limitation if it matters.",
                    command_request.command
                )
            };
            (tool_call_summary(&command_request), content)
        };

        if let (Some(action), Some(outcome)) =
            (after_tool_action.as_ref(), after_tool_outcome.as_ref())
        {
            let payload =
                tool_event_payload(&self.scanner, action, Some(outcome), after_tool_termination);
            let after = self.dispatch_tool_event(
                HookEvent::AfterTool,
                &pending.session,
                &pending.task,
                &repository_root,
                payload,
            );
            if let Some(note) = hook_after_note(&after) {
                tool_result_content.push_str("\n\n");
                tool_result_content.push_str(&note);
            }
            self.record_after_refusal(
                &pending.session.id,
                &pending.task.id,
                HookEvent::AfterTool,
                &after,
            );
        }

        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "assistant",
            &assistant_summary,
        )?;
        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "tool",
            &tool_result_content,
        )?;

        if let Some(call) = &pending.matched_tool_call {
            // When earlier calls of the round ran, the round's assistant
            // message is the last one with `tool_calls` — only their results
            // follow it — and this call joins it, so the resumed request holds
            // the response once, as the uninterrupted round would have.
            let round_message = pending
                .round_calls_fed_back
                .then(|| messages.iter().rposition(|m| !m.tool_calls.is_empty()))
                .flatten();
            match round_message {
                Some(index) => messages[index].tool_calls.push(call.clone()),
                None => messages.push(ModelMessage::assistant_with_tool_calls(
                    pending.last_content.clone(),
                    vec![call.clone()],
                    pending.reasoning_content.clone(),
                )),
            }
            messages.push(ModelMessage::tool(call.id.clone(), tool_result_content));
        } else {
            messages.push(
                ModelMessage::assistant(pending.last_content.clone())
                    .with_reasoning_content(pending.reasoning_content.clone()),
            );
            messages.push(ModelMessage::user(format!(
                "Command result:\n{tool_result_content}"
            )));
        }

        // The pause is over either way, so the checkpoint stops claiming one.
        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        // The approved command has already run — its result is the `tool`
        // message appended above — so the side effect is behind us and what
        // follows is another model round. Verified by reading rather than
        // assumed: labelling a still-executing command as read-only would let
        // the classifier auto-resume it, which is exactly what requirement 5
        // forbids.
        let task = self.session_store.update_task_status(
            &pending.task,
            TaskStatus::PreparingContext,
            None,
        )?;

        // The approved dispatch happened here, outside the loop, so its typed
        // result joins the tracker now (spec 103 Task 4).
        let completed_tool_rounds = match (after_tool_action.as_ref(), after_tool_outcome.as_ref())
        {
            (Some(action), Some(outcome)) => self.tracker_with_resumed_call(
                &pending.session.id,
                &pending.task.id,
                &pending.completed_tool_rounds,
                pending.matched_tool_call.as_ref(),
                &repository_root,
                action,
                outcome,
            )?,
            _ => CompletedToolTracker::from(pending.completed_tool_rounds.clone()),
        };

        let segment_pause = pending.segment.clone();
        let result = self.run_agentic_turn(
            &repository_root,
            pending.session,
            task,
            pending.context_files,
            messages,
            pending.round + 1,
            pending.exploration_rounds,
            turn_notes,
            completed_tool_rounds,
            step_evidence,
            pending.plan_reminder_sent,
            model_adapter,
            sink,
            pending.turn_options,
            segment_pause.as_ref(),
        )?;
        self.verify_after_command_turn(
            &repository_root,
            result,
            pending.turn_options.verification_turn,
            model_adapter,
            sink,
        )
    }

    /// Ends a turn whose approved command changed files with the verification
    /// that change made due (spec 85), as a patch's card does once it is
    /// settled. Nothing else needs to ask: the shell, a worker and the CLI all
    /// resume through here.
    ///
    /// Only once the turn has finished. A turn that paused again keeps the run
    /// due, and whichever resume finishes the turn runs it; a patch proposed
    /// after the command makes it due on its own, and that run covers the
    /// command's files too. In a repair turn the command is the repair, so the
    /// next attempt runs instead of closing it as `no_repair_applied`.
    fn verify_after_command_turn(
        &self,
        repository_root: &Path,
        result: ChatTurnResult,
        verification_turn: bool,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        // Spec 101: an answered turn with a due command verification settles
        // `incomplete` for `verification_due`, not `complete`. The verification
        // the command made due is still the turn's work, so it runs for either
        // terminal settlement and its own task records the final verdict.
        let settled = matches!(
            result.task.status,
            TaskStatus::Complete | TaskStatus::Incomplete
        );
        let finished = !turn_is_paused(&result) && settled;
        let command_due = finished && {
            let events = self
                .session_store
                .read_verification_events(&result.session.id, &[])?;
            fold_verification(events.iter())
                .due
                .is_some_and(|due| due.from_command())
        };
        if !command_due {
            return if verification_turn {
                self.close_verification_resume(repository_root, result)
            } else {
                Ok(result)
            };
        }
        let answer = result.response.clone();
        let mut verified =
            self.verify_due(repository_root, &result.session.id, model_adapter, sink)?;
        // The turn's own answer was streamed into the same message; keeping it
        // in front means the final text does not replace it with the report.
        if !answer.trim().is_empty() {
            verified.response = format!("{answer}\n\n{}", verified.response);
        }
        Ok(verified)
    }
}
