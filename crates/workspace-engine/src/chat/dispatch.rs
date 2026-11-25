use super::*;
use crate::session::ActionMarker;

/// What a sequentially dispatched tool call reads from and writes back to the
/// round it runs in. `run_agentic_turn` builds one per call, after the mode,
/// review and hook gates, so a handler sees exactly the turn state the call
/// was admitted with.
pub(super) struct CallContext<'a, 's> {
    /// The mode captured at the top of the turn (spec 20 §5.4).
    pub(super) mode: SessionMode,
    pub(super) repository_root: &'a Path,
    pub(super) session: &'a Session,
    pub(super) task: &'a Task,
    pub(super) context_files: &'a Vec<String>,
    pub(super) turn_options: ChatTurnOptions,
    pub(super) action_rounds: u32,
    pub(super) exploration_rounds: u32,
    pub(super) max_rounds: u32,
    pub(super) max_exploration_rounds: u32,
    pub(super) plan_reminder_sent: bool,
    /// Index of this round's assistant message, once a call has joined it.
    pub(super) round_message: Option<usize>,
    pub(super) turn_notes: &'a mut TurnNotesState,
    pub(super) completed_tool_rounds: &'a mut CompletedToolTracker,
    pub(super) round_completed: &'a mut Vec<TrackedCompletedToolCall>,
    pub(super) step_evidence: &'a mut Vec<crate::plan::Evidence>,
    pub(super) plan: &'a mut Option<TaskPlan>,
    pub(super) plan_approved: &'a mut bool,
    pub(super) messages: &'a Vec<ModelMessage>,
    pub(super) matched_tool_call: &'a Option<ToolCall>,
    /// The model's redacted prose for this round.
    pub(super) redacted: &'a String,
    pub(super) reasoning_content: &'a Option<String>,
    pub(super) segment_tracker: &'a Option<SegmentRunState>,
    pub(super) sink: &'a mut TurnSink<'s>,
    pub(super) mcp: &'a mut McpRuntime,
    pub(super) failed_browser_calls: &'a mut HashMap<String, u32>,
    /// A `before_tool` hook asked for approval even where policy would not.
    pub(super) force_approval: bool,
    pub(super) force_approval_cause: &'a Option<(String, String)>,
    /// The call as dispatched, for `after_tool`: the handlers consume their
    /// own copy of the action's fields.
    pub(super) after_tool_action: &'a ToolAction,
    // A sandbox command's facts leave its handler here rather than through
    // `ActionOutcome` (spec 22 `context.md` §14): `CommandExecution` is not
    // persisted, so nothing downstream could recover them.
    pub(super) command_findings: Option<crate::plan::Evidence>,
    pub(super) command_termination: Option<CommandTermination>,
    /// The directory the command actually ran in, repository-relative for
    /// the evidence (spec 105's check identity).
    pub(super) command_directory: Option<String>,
}

impl CallContext<'_, '_> {
    /// The turn as it stands at this call, ready to persist for a pause. The
    /// round's completed calls are published first, so a resumed turn
    /// compacts from the same typed facts. Every pause-kind field is `None`;
    /// the caller sets the one it is pausing for.
    pub(super) fn paused_turn(
        &mut self,
        proposal_id: String,
        matched_tool_call: Option<ToolCall>,
    ) -> PendingChatTurn {
        self.completed_tool_rounds
            .push_round(std::mem::take(self.round_completed));
        PendingChatTurn {
            proposal_id,
            session: self.session.clone(),
            task: self.task.clone(),
            repository_root: self.repository_root.to_string_lossy().to_string(),
            context_files: self.context_files.clone(),
            round: self.action_rounds,
            exploration_rounds: self.exploration_rounds,
            turn_notes: self.turn_notes.text.clone(),
            turn_notes_carried: self.turn_notes.carried,
            calls: self.turn_notes.calls.clone(),
            completed_tool_rounds: self.completed_tool_rounds.rounds.clone(),
            // A pause carries the open step's evidence and the reminder
            // flag; neither is attached or reset here.
            step_evidence: self.step_evidence.clone(),
            plan_reminder_sent: self.plan_reminder_sent,
            messages: self.messages.clone(),
            matched_tool_call,
            last_content: self.redacted.clone(),
            turn_options: self.turn_options,
            reasoning_content: self.reasoning_content.clone(),
            round_calls_fed_back: self.round_message.is_some(),
            mcp_call: None,
            web_diagnostic_call: None,
            plan_review: None,
            provider_fallback: None,
            round_budget: None,
            segment: self
                .segment_tracker
                .as_ref()
                .map(|tracker| tracker.pause_state(self.action_rounds, self.exploration_rounds)),
            question: None,
            check_batch: None,
            failure_baseline: None,
            fetch_call: None,
            context_review: None,
        }
    }
}

/// A call that ends the turn for a human decision: the answer shown to the
/// user and the card that asks them.
pub(super) struct RoundStop {
    pub(super) response: String,
    pub(super) proposals: TurnProposals,
}

/// What a tool reported: the assistant summary, the tool result fed back to
/// the model, and the outcome the action's marker is finished with.
pub(super) type ToolResult = (String, String, ActionOutcome);

/// `Break` ends the turn; `Continue` hands back the still-open marker.
pub(super) type CallFlow = ControlFlow<RoundStop, (ActionMarker, ToolResult)>;

impl ChatOrchestrator {
    /// Runs one call the concurrent batch does not handle. A call that ends
    /// the turn for a human decision returns `Break`; every other call
    /// returns its marker and the (assistant summary, tool result, outcome)
    /// triple to persist and feed back to the model.
    pub(super) fn dispatch_sequential_action(
        &self,
        cx: &mut CallContext<'_, '_>,
        tool_action: ToolAction,
        action_marker: ActionMarker,
    ) -> Result<CallFlow> {
        let result = match tool_action {
            ToolAction::Command(command_request) => {
                return self.handle_command(cx, command_request, action_marker);
            }
            ToolAction::ProposePatch(generated_edit) => {
                return self.handle_propose_patch(cx, generated_edit, action_marker);
            }
            ToolAction::ProposePlan(steps) => self.handle_propose_plan(cx, steps)?,
            ToolAction::CompleteStep => self.handle_complete_step(cx)?,
            ToolAction::EditFile { summary, edits } => {
                return self.handle_edit_file(cx, summary, edits, action_marker);
            }
            ToolAction::FetchUrl { url } => return self.handle_fetch_url(cx, url, action_marker),
            ToolAction::WebDiagnostic(call) => {
                return self.handle_web_diagnostic(cx, call, action_marker);
            }
            ToolAction::McpCall {
                server_id,
                tool_name,
                arguments_json,
            } => {
                return self.handle_mcp_call(
                    cx,
                    server_id,
                    tool_name,
                    arguments_json,
                    action_marker,
                );
            }
            ToolAction::McpToolSearch { query } => {
                let (content, outcome) =
                    self.search_mcp_tools(cx.mode, &cx.session.repository_id, cx.mcp, &query);
                ("Searched the MCP tools.".to_string(), content, outcome)
            }
            ToolAction::TurnNotes { text, replace } => self.handle_turn_notes(cx, text, replace)?,
            ToolAction::ProposeMemory {
                statement,
                category,
                scope,
                source,
                evidence_path,
            } => {
                self.handle_propose_memory(cx, statement, category, scope, source, evidence_path)?
            }
            ToolAction::AskUser(input) => return self.handle_ask_user(cx, input, action_marker),
            other => {
                unreachable!("read-only action reached the sequential match: {other:?}")
            }
        };
        Ok(ControlFlow::Continue((action_marker, result)))
    }

    fn handle_command(
        &self,
        cx: &mut CallContext<'_, '_>,
        command_request: CommandRequest,
        action_marker: ActionMarker,
    ) -> Result<CallFlow> {
        let result = {
            let working_directory =
                self.command_working_directory(cx.repository_root, &command_request);
            let extra_causes = cx
                .force_approval_cause
                .as_ref()
                .map(|(hook, reason)| {
                    vec![ApprovalCause::HookRequested {
                        hook: hook.clone(),
                        reason: reason.clone(),
                    }]
                })
                .unwrap_or_default();
            let proposal = self
                .validation_orchestrator
                .propose_command_at_with_causes(
                    cx.repository_root,
                    &working_directory,
                    &command_request.command,
                    &command_request.reason,
                    extra_causes,
                )?;

            if proposal.requires_approval || proposal.blocked {
                let response = command_proposal_response(&proposal);
                self.pending_commands
                    .save(&cx.paused_turn(proposal.id.clone(), cx.matched_tool_call.clone()))?;
                self.note_pending_approvals(
                    cx.session,
                    cx.task,
                    vec![PendingApproval {
                        kind: "command".to_string(),
                        proposal_id: proposal.id.clone(),
                    }],
                );
                // A clean stop for a human decision, not a crash: the
                // action is finished so the classifier does not read a
                // dangling marker as an unknown outcome.
                self.session_store
                    .finish_action(action_marker, "awaiting_approval")?;
                return Ok(ControlFlow::Break(RoundStop {
                    response,
                    proposals: TurnProposals {
                        command: Some(agent_command_proposal(&self.config, &proposal)),
                        ..Default::default()
                    },
                }));
            }

            cx.sink.phase(
                PhaseKind::Tool,
                running_command_label(&proposal.command),
                cx.action_rounds,
                cx.max_rounds,
                cx.exploration_rounds,
                cx.max_exploration_rounds,
                cx.turn_notes.text.chars().count(),
            );
            let cancel = cx.sink.cancel;
            // Borrow only the progress field, so `cancel` can be read
            // for the same call without two conflicting borrows of the
            // sink.
            let on_progress = &mut *cx.sink.on_progress;
            let mut on_output = |line: &str| {
                on_progress(TurnProgress::Phase(TurnPhase::new(
                    PhaseKind::Output,
                    line,
                    cx.action_rounds,
                    cx.max_rounds,
                    cx.exploration_rounds,
                    cx.max_exploration_rounds,
                    cx.turn_notes.text.chars().count(),
                )));
            };
            let record = self.validation_orchestrator.run_proposal(
                &proposal.id,
                false,
                "policy",
                Some(&cx.task.id),
                cancel,
                &mut on_output,
            )?;
            cx.command_findings = self.record_command_findings(
                cx.repository_root,
                cx.session,
                cx.task,
                &record.execution,
            )?;
            let command_context = command_context_with_revocation(&record);
            // The exit code is in hand right here, one statement before
            // the marker is finished. Nothing downstream can recover it
            // — `CommandExecution` is never persisted to the session log
            // — so it is carried out of the arm rather than looked up.
            let exit_code = record.execution.exit_code;
            cx.command_termination = Some(record.execution.termination);
            cx.command_directory = Some(
                relative_location(
                    cx.repository_root,
                    Path::new(&record.execution.working_directory),
                )
                .unwrap_or_else(|| record.execution.working_directory.clone()),
            );
            // Spec 109 D5: a run that needed no card because of a session
            // approval says so in its row.
            let summary = if proposal
                .rule
                .as_ref()
                .is_some_and(|rule| rule.source == "session")
            {
                session_approved_call_summary(&command_request)
            } else {
                tool_call_summary(&command_request)
            };
            (
                summary,
                command_context,
                ActionOutcome::CommandExit(exit_code, record.execution.purpose),
            )
        };
        Ok(ControlFlow::Continue((action_marker, result)))
    }

    fn handle_propose_patch(
        &self,
        cx: &mut CallContext<'_, '_>,
        generated_edit: GeneratedEdit,
        action_marker: ActionMarker,
    ) -> Result<CallFlow> {
        let result = {
            match self
                .refuse_edit_in_worktree(
                    cx.repository_root,
                    generated_edit
                        .changes
                        .iter()
                        .map(|change| change.path.as_str()),
                )
                .and_then(|()| self.without_unchanged_files(cx, generated_edit.changes.clone()))
                .and_then(|changes| {
                    self.patch_engine.create_patch(
                        cx.repository_root,
                        &changes,
                        Some(&cx.task.id),
                        &generated_edit.summary,
                    )
                })
                .and_then(|patch| {
                    self.ensure_patch_not_protected(&cx.session.id, &cx.task.id, &patch)
                        .map(|()| patch)
                }) {
                Ok(patch) => return self.stop_for_patch_review(cx, patch, action_marker),
                // Fed back as a tool result rather than aborting the
                // turn, so the model can see why (e.g. a restricted
                // or out-of-repo path) and correct itself within the
                // remaining rounds instead of the turn just failing.
                Err(error) => (
                    format!("Attempted to propose a patch: {}", generated_edit.summary),
                    format!("Cannot propose that patch: {error}"),
                    ActionOutcome::Failed,
                ),
            }
        };
        Ok(ControlFlow::Continue((action_marker, result)))
    }

    /// Observation 99: a proposal never raises a decision with nothing in it.
    /// A file that already holds the proposed content is left out, and a
    /// proposal that is only such files is refused back to the model as the
    /// tool result, so it moves on instead of the user approving an empty card.
    ///
    /// A verification repair turn is exempt: a repair that repeats the content
    /// already on disk is spec 102's evidence of a stall, which the repair
    /// loop detects from the patch and reports as **Repair stalled**.
    fn without_unchanged_files(
        &self,
        cx: &CallContext<'_, '_>,
        changes: Vec<ProposedChange>,
    ) -> Result<Vec<ProposedChange>> {
        if cx.turn_options.verification_turn {
            return Ok(changes);
        }
        let unchanged = self
            .patch_engine
            .unchanged_paths(cx.repository_root, &changes);
        if !changes.is_empty() && unchanged.len() == changes.len() {
            return Err(ClientError::InvalidInput(
                "the patch changes nothing: every file already has the proposed content. \
                 Do not propose it again; continue with the next step, or re-read the file \
                 if you expected it to differ."
                    .to_string(),
            ));
        }
        Ok(changes
            .into_iter()
            .filter(|change| !unchanged.contains(&change.path))
            .collect())
    }

    /// A patch that passed the worktree and protected-path checks stops the
    /// turn for review. Shared by `propose_patch` and `edit_file`, which differ
    /// only in how they build the changes.
    fn stop_for_patch_review(
        &self,
        cx: &mut CallContext<'_, '_>,
        mut patch: ProposedPatch,
        action_marker: ActionMarker,
    ) -> Result<CallFlow> {
        // See `ProposedPatch::session_id`: the engine does
        // not know the session, this orchestrator does.
        patch.session_id = cx.session.id.clone();
        // Spec 102 Task 5: the scope review is part of
        // the patch before its first save, so a crash
        // can never recover a repair card without it.
        let repair_scope = self.repair_scope_review_for_patch(
            cx.repository_root,
            cx.session,
            cx.task,
            cx.turn_options,
            &patch,
        )?;
        patch.repair_scope = repair_scope;
        self.patch_store.save(&patch)?;
        let response = patch_proposal_response(&patch);
        // A patch waiting for review is a clean stop, not
        // a crash — finish the marker before breaking.
        self.session_store
            .finish_action(action_marker, "awaiting_review")?;
        let after = self.dispatch_tool_event(
            HookEvent::AfterTool,
            cx.session,
            cx.task,
            cx.repository_root,
            tool_event_payload(
                &self.scanner,
                cx.after_tool_action,
                Some(&ActionOutcome::Ok),
                None,
            ),
        );
        self.record_after_refusal(&cx.session.id, &cx.task.id, HookEvent::AfterTool, &after);
        let proposal = agent_patch_proposal(&patch);
        Ok(ControlFlow::Break(RoundStop {
            response,
            proposals: TurnProposals {
                patch: Some(proposal),
                ..Default::default()
            },
        }))
    }

    fn handle_propose_plan(
        &self,
        cx: &mut CallContext<'_, '_>,
        steps: Vec<ProposedStep>,
    ) -> Result<ToolResult> {
        let result = {
            // A plan an earlier turn made, carried in and not yet
            // started on, is the one exception (spec 83 §5.4): the
            // user may have moved on to other work, and nothing
            // this turn observed is tied to it yet. It stays
            // recorded under the task that made it.
            //
            // A continuation is different (spec 101 review fix
            // R9). The user asked to continue this exact
            // incomplete task, and its carried plan is what
            // still keeps it from completing. Replacing it here
            // would drop those open or blocked steps, or its
            // required check, without the user ever seeing the
            // replacement, so the proposal is refused while any
            // remain. A carried plan with nothing outstanding has
            // no such work to lose and may still be replaced,
            // which the ordinary review gate covers before its
            // first mutating action.
            let continue_protected = cx.turn_options.plan_already_carried
                && cx
                    .plan
                    .as_ref()
                    .is_some_and(carried_plan_has_outstanding_work);
            // Spec 108 D6: the user reviewed this plan and asked
            // for changes, so its replacement is exactly what
            // they asked for, carried or not. It still meets the
            // review gate before any change.
            let changes_requested = cx.plan.is_some()
                && self
                    .session_store
                    .plan_changes_requested(&cx.session.id, &cx.task.id)
                    .unwrap_or(false);
            let replaceable = cx.plan.is_some()
                && (changes_requested
                    || (!continue_protected
                        && self
                            .session_store
                            .carried_plan_untouched(&cx.session.id, &cx.task.id)
                            .unwrap_or(false)));
            if cx.plan.is_some() && !replaceable {
                // Refused rather than replaced. Steps already carry
                // evidence tied to a state of the repository, and
                // rewriting the plan underneath that evidence produces
                // a history that no longer describes what happened —
                // the same reason §5.5 rules out mid-execution edits.
                let guidance = if continue_protected {
                    "This task is continuing a plan carried from the task you were asked to continue, and that plan still has open or blocked work or a required check. Work through the carried steps with complete_step and satisfy the check. Only the user can change which carried steps survive, through plan review. This proposal was not recorded.".to_string()
                } else {
                    "This turn already has a plan. Work through its remaining steps with complete_step, or stop and start a new turn if the plan is wrong.".to_string()
                };
                (
                    "Attempted to propose a second plan.".to_string(),
                    guidance,
                    ActionOutcome::Failed,
                )
            } else {
                let now = now_millis();
                let mut proposed = crate::plan::TaskPlan::new(&cx.task.id, now);
                for (index, step) in steps.iter().enumerate() {
                    proposed.steps.push(crate::plan::PlanStep {
                        id: format!("step_{}", index + 1),
                        // Model-authored text, redacted like any other:
                        // a title is rendered in the panel and written
                        // to the log, so a secret echoed into one must
                        // not survive there.
                        title: self.scanner.redact(&step.title).text,
                        detail: step
                            .detail
                            .as_ref()
                            .map(|detail| self.scanner.redact(detail).text),
                        // The engine's to set, not the model's.
                        status: if index == 0 {
                            crate::plan::StepStatus::InProgress
                        } else {
                            crate::plan::StepStatus::Pending
                        },
                        depends_on: Vec::new(),
                        started_at_ms: (index == 0).then_some(now),
                        completed_at_ms: None,
                        evidence: Vec::new(),
                        check: normalized_proposed_check(cx.repository_root, step.check.clone()),
                        build: normalized_proposed_check(cx.repository_root, step.build.clone()),
                    });
                }
                self.session_store.create_plan(cx.task, &proposed)?;
                // A replaced plan's approval was for the old steps.
                *cx.plan_approved = false;
                // Spec 108 D7: the plan the user approved before
                // handing it to this worker is not reviewed twice.
                let handed_note = self.approve_handed_plan(cx.session, cx.task, &proposed)?;
                if handed_note.is_some() {
                    *cx.plan_approved = true;
                }
                cx.sink.plan(&proposed);
                let summary = format!("Planned {} steps.", proposed.steps.len());
                let first = proposed.steps[0].title.clone();
                *cx.plan = Some(proposed);
                let mut result = format!(
                    "Plan recorded. The current step is: {first}. Call complete_step when its work is done."
                );
                if let Some(note) = handed_note {
                    result.push_str("\n\n");
                    result.push_str(&note);
                }
                (summary, result, ActionOutcome::Ok)
            }
        };
        Ok(result)
    }

    fn handle_complete_step(&self, cx: &mut CallContext<'_, '_>) -> Result<ToolResult> {
        let result = {
            match cx.plan.as_mut() {
                None => (
                    "Attempted to complete a step.".to_string(),
                    "There is no plan for this turn, so there is no step to complete.".to_string(),
                    ActionOutcome::Failed,
                ),
                Some(current) => {
                    // A step can only be closed while it is the one
                    // running, or re-judged while it is the one
                    // blocked. A second `complete_step` after a block
                    // used to find no open step, leave `status` at its
                    // default `Completed`, and start the next pending
                    // step — laundering the block into a handoff
                    // (found in a session on 2026-10-26). It then
                    // became a refusal with no way out, so a block
                    // stood for the rest of the turn even after the
                    // model fixed it (2026-10-29). A blocked step is
                    // now judged again on everything observed since,
                    // by the same rule: it clears only when the
                    // command that failed has since passed.
                    let running = current
                        .steps
                        .iter()
                        .position(|step| step.status == crate::plan::StepStatus::InProgress);
                    let blocked = current
                        .steps
                        .iter()
                        .position(|step| step.status == crate::plan::StepStatus::Blocked);
                    match running.or(blocked) {
                    None => (
                        "Attempted to complete a step.".to_string(),
                        "No step is in progress, so there is nothing to complete. Do not call complete_step again.".to_string(),
                        ActionOutcome::Failed,
                    ),
                    Some(open_index) => {
                        let was_blocked = current.steps[open_index].status
                            == crate::plan::StepStatus::Blocked;
                        // Spec 106 Task 6 (live run 3): a step that
                        // declares a required check does not close
                        // before that check has run on it, or been
                        // declined, or been run by the verification
                        // loop. Closing it anyway left the gate a
                        // missing check nothing would ever run (it is
                        // the model's job, review follow-up to R16),
                        // and the worker stopped. Refusing here is the
                        // one point where the model can still act.
                        let unrun_check = if was_blocked {
                            None
                        } else {
                            self.unrun_required_check(
                                &cx.session.id,
                                &cx.task.id,
                                &current.steps[open_index],
                                cx.step_evidence,
                            )?
                        };
                        if let Some(check) = unrun_check {
                            let title = current.steps[open_index].title.clone();
                            (
                                "Attempted to complete a step.".to_string(),
                                format!(
                                    "Step \"{title}\" declares the required check `{}` ({}), and it has not run on this step. Run exactly that command in that directory with run_command, then call complete_step again. The step stays in progress.",
                                    check.command,
                                    if check.working_directory.is_empty() {
                                        "repository root".to_string()
                                    } else {
                                        check.working_directory.clone()
                                    }
                                ),
                                ActionOutcome::Failed,
                            )
                        } else {
                        // The model asked to move on; it does not get
                        // to say how the step ended. §5.3: the status
                        // is a function of the evidence, and this is
                        // the only place a step reaches a terminal
                        // status.
                        let accrued = std::mem::take(&mut *cx.step_evidence);
                        let observed_more = !accrued.is_empty();
                        let now = now_millis();
                        let open = &mut current.steps[open_index];
                        // Extends rather than replaces. A step can
                        // already carry evidence this turn never saw:
                        // a patch applied after the turn that proposed
                        // it appends through the log (`edit.rs`), and
                        // a resumed plan arrives with everything its
                        // earlier turns recorded. Assigning here would
                        // silently drop both, and the step would then
                        // be judged on a fraction of what is known
                        // about it.
                        open.evidence.extend(accrued);
                        let status =
                            crate::plan::status_from_evidence(&open.evidence);
                        let finished_title = open.title.clone();
                        if was_blocked && status == crate::plan::StepStatus::Blocked {
                            // Still blocked: nothing hands off, and the
                            // block keeps its time. What was observed
                            // since is kept, so the next re-check
                            // weighs it too.
                            if observed_more {
                                let kept = open.clone();
                                self.session_store.update_plan_step(cx.task, &kept)?;
                                cx.sink.plan(current);
                            }
                            (
                                "Attempted to complete a step.".to_string(),
                                format!(
                                    "No step is in progress. Step \"{finished_title}\" is still blocked: a command it ran failed, and that same command has not succeeded since. Fix the cause, run exactly the same command again (a different command does not clear it), then call complete_step. The remaining steps are still pending."
                                ),
                                ActionOutcome::Failed,
                            )
                        } else {
                            open.status = status;
                            open.completed_at_ms = Some(now);
                            let closed = open.clone();
                            self.session_store.update_plan_step(cx.task, &closed)?;

                            // A blocked step does not hand off: the next
                            // step's prerequisite failed, and starting it
                            // anyway would build on work that did not
                            // happen.
                            let mut next_title = None;
                            if status == crate::plan::StepStatus::Completed
                                && let Some(next) =
                                    current.steps.iter_mut().find(|step| {
                                        step.status
                                            == crate::plan::StepStatus::Pending
                                    })
                            {
                                next.status = crate::plan::StepStatus::InProgress;
                                next.started_at_ms = Some(now);
                                next_title = Some(next.title.clone());
                                let started = next.clone();
                                self.session_store
                                    .update_plan_step(cx.task, &started)?;
                            }
                            // Once, after the handoff rather than after
                            // each of its two writes: between them the
                            // closing step is already terminal and the
                            // next has not opened, so a panel updated
                            // mid-handoff would blink through a state
                            // with no current step. The log keeps both
                            // writes; the panel does not need them.
                            cx.sink.plan(current);

                            let done = if was_blocked {
                                format!(
                                    "Step \"{finished_title}\" is no longer blocked: the command that failed has since succeeded."
                                )
                            } else {
                                format!("Step \"{finished_title}\" is complete.")
                            };
                            let result = match (status, &next_title) {
                                (crate::plan::StepStatus::Blocked, _) => format!(
                                    "Step \"{finished_title}\" is blocked: a command it ran did not succeed. Fix the cause and run that same command again; complete_step then checks the step again. The remaining steps are still pending."
                                ),
                                (_, Some(next)) => {
                                    format!("{done} The current step is now: {next}.")
                                }
                                (_, None) => format!("{done} That was the last step."),
                            };
                            (
                                format!("Finished: {finished_title}"),
                                result,
                                ActionOutcome::Ok,
                            )
                        }
                        }
                    }
                }
                }
            }
        };
        Ok(result)
    }

    fn handle_edit_file(
        &self,
        cx: &mut CallContext<'_, '_>,
        summary: String,
        edits: Vec<RegionEdit>,
        action_marker: ActionMarker,
    ) -> Result<CallFlow> {
        let result = {
            match self
                .refuse_edit_in_worktree(
                    cx.repository_root,
                    edits.iter().map(|edit| edit.path.as_str()),
                )
                .and_then(|()| {
                    region_edits_to_changes(cx.repository_root, &self.path_policy, &edits)
                })
                .and_then(|changes| self.without_unchanged_files(cx, changes))
                .and_then(|changes| {
                    self.patch_engine.create_patch(
                        cx.repository_root,
                        &changes,
                        Some(&cx.task.id),
                        &summary,
                    )
                })
                .and_then(|patch| {
                    self.ensure_patch_not_protected(&cx.session.id, &cx.task.id, &patch)
                        .map(|()| patch)
                }) {
                Ok(patch) => return self.stop_for_patch_review(cx, patch, action_marker),
                // Fed back as a tool result rather than aborting the
                // turn, so the model can see why (a stale anchor, a
                // restricted path) and correct itself within the
                // remaining rounds.
                Err(error) => (
                    format!("Attempted to edit files: {summary}"),
                    format!("Cannot apply that edit: {error}"),
                    ActionOutcome::Failed,
                ),
            }
        };
        Ok(ControlFlow::Continue((action_marker, result)))
    }

    fn handle_fetch_url(
        &self,
        cx: &mut CallContext<'_, '_>,
        url: String,
        action_marker: ActionMarker,
    ) -> Result<CallFlow> {
        let result = {
            let cancel = cx.sink.cancel;
            match self.dispatch_fetch_url(
                cx.repository_root,
                cx.session,
                cx.task,
                &url,
                &cx.turn_notes.calls,
                cx.messages,
                cancel,
            )? {
                FetchDispatch::Result(summary, content, outcome) => (summary, content, outcome),
                // Spec 51 §5.3 constraint 1: an unapproved host
                // pauses before the request is built. The turn
                // ends cleanly, not as a failure — nothing has
                // left the machine and the user has a decision
                // to make.
                FetchDispatch::NeedsApproval(inspection) => {
                    let host = inspection.url.host.clone();
                    let response = format!(
                        "Fetching from {host} needs your approval before \
                                         anything is requested."
                    );
                    let proposal_id = create_id("fetch");
                    let proposal = AgentFetchProposal {
                        id: proposal_id.clone(),
                        host: host.clone(),
                        url: inspection.url.raw.clone(),
                        approval_key: inspection.url.approval_key(),
                        long_components: inspection
                            .flags
                            .long_components
                            .iter()
                            .map(|flag| FetchComponentFlag {
                                component: flag.component.as_str().to_string(),
                                bytes: flag.bytes,
                            })
                            .collect(),
                        query: inspection.url.query.clone(),
                        fragment: inspection.url.fragment.clone(),
                    };
                    self.pending_commands.save(&PendingChatTurn {
                        fetch_call: Some(PendingFetchCall {
                            host: host.clone(),
                            url: proposal.url.clone(),
                            approval_key: proposal.approval_key.clone(),
                            long_components: proposal.long_components.clone(),
                            query: proposal.query.clone(),
                            fragment: proposal.fragment.clone(),
                        }),
                        ..cx.paused_turn(proposal_id.clone(), cx.matched_tool_call.clone())
                    })?;
                    self.note_pending_approvals(
                        cx.session,
                        cx.task,
                        vec![PendingApproval {
                            kind: "fetch".to_string(),
                            proposal_id: proposal_id.clone(),
                        }],
                    );
                    // A clean stop for a human decision, not a
                    // crash: nothing was dispatched, so the
                    // marker is finished rather than left
                    // dangling.
                    self.session_store
                        .finish_action(action_marker, "awaiting_approval")?;
                    return Ok(ControlFlow::Break(RoundStop {
                        response,
                        proposals: TurnProposals {
                            fetch: Some(proposal),
                            ..Default::default()
                        },
                    }));
                }
            }
        };
        Ok(ControlFlow::Continue((action_marker, result)))
    }

    fn handle_web_diagnostic(
        &self,
        cx: &mut CallContext<'_, '_>,
        call: WebDiagnosticCall,
        action_marker: ActionMarker,
    ) -> Result<CallFlow> {
        let result = {
            let call = call.with_context(&cx.session.id, &cx.task.id);
            // Spec 12 §5.4: the session grant never reaches a
            // remote URL.
            let session_approved = if call.is_low_risk() || !call.targets_loopback() {
                false
            } else {
                self.session_store
                    .browser_diagnostics_allowed_for_session(&cx.session.id)?
            };
            if cx.force_approval || (!call.is_low_risk() && !session_approved) {
                let proposal_id = create_id("webdiag");
                let proposal = web_diagnostic_approval_proposal(
                    &proposal_id,
                    &call,
                    cx.force_approval_cause.as_ref(),
                );
                let response = proposal.prompt.clone();
                self.pending_commands.save(&PendingChatTurn {
                    web_diagnostic_call: Some(PendingWebDiagnosticCall { call }),
                    ..cx.paused_turn(proposal_id, cx.matched_tool_call.clone())
                })?;
                self.note_pending_approvals(
                    cx.session,
                    cx.task,
                    vec![PendingApproval {
                        kind: "browser_diagnostic".to_string(),
                        proposal_id: proposal.id.clone(),
                    }],
                );
                // A clean stop for a human decision, not a crash: the
                // action is finished so the classifier does not read a
                // dangling marker as an unknown outcome.
                self.session_store
                    .finish_action(action_marker, "awaiting_approval")?;
                return Ok(ControlFlow::Break(RoundStop {
                    response,
                    proposals: TurnProposals {
                        command: Some(proposal),
                        ..Default::default()
                    },
                }));
            }
            if session_approved {
                self.audit_log.record(
                    "browser_diagnostic_session_approval_used",
                    &[
                        ("actor", "system".to_string()),
                        ("sessionId", cx.session.id.clone()),
                        ("taskId", cx.task.id.clone()),
                        ("tool", call.name().to_string()),
                        ("url", call.url.clone()),
                    ],
                )?;
            }

            let signature = web_diagnostic_signature(&call);
            let retry_limit = self.config.agent_tool_retry_limit;
            let (content, outcome) = if cx
                .failed_browser_calls
                .get(&signature)
                .copied()
                .unwrap_or_default()
                >= retry_limit
            {
                // Refused rather than attempted, because the same call
                // has already failed its retry limit. Still a failure:
                // the tool produced no diagnostic.
                (browser_retry_limit_note(retry_limit), ActionOutcome::Failed)
            } else {
                // A page that throws is not a failed call
                // (spec 12 `context.md` §3.2).
                let (content, failed) =
                    self.run_and_record_web_diagnostic(cx.repository_root, &call, cx.sink)?;
                if failed {
                    *cx.failed_browser_calls.entry(signature).or_insert(0) += 1;
                }
                let outcome = if failed {
                    ActionOutcome::Failed
                } else {
                    ActionOutcome::Ok
                };
                (content, outcome)
            };
            (web_diagnostic_summary(&call), content, outcome)
        };
        Ok(ControlFlow::Continue((action_marker, result)))
    }

    fn handle_mcp_call(
        &self,
        cx: &mut CallContext<'_, '_>,
        server_id: String,
        tool_name: String,
        arguments_json: String,
        action_marker: ActionMarker,
    ) -> Result<CallFlow> {
        let result = {
            // Spec 33 §5.2: a disabled tool is absent from the offer. A model
            // that names one anyway is told so; nothing is connected or sent.
            if !self.mcp_tool_callable(&server_id, &tool_name) {
                return Ok(ControlFlow::Continue((
                    action_marker,
                    (
                        mcp_call_summary(&server_id, &tool_name),
                        format!(
                            "The MCP tool `{tool_name}` on server `{server_id}` is not available \
                             in this session."
                        ),
                        ActionOutcome::Failed,
                    ),
                )));
            }
            // Spec 33 §5.3: a `Failed` server is not retried automatically,
            // so its call is answered with the reason before any approval
            // card — approving it could change nothing.
            if let Some(reason) = cx.mcp.failed_reason(&server_id) {
                return Ok(ControlFlow::Continue((
                    action_marker,
                    (
                        mcp_call_summary(&server_id, &tool_name),
                        reason,
                        ActionOutcome::Failed,
                    ),
                )));
            }
            // MCP tools reach an external service and can have side
            // effects, so unless the user waived approval for this exact
            // tool (or its server, when the tool has no setting of its
            // own) we pause the turn exactly like a command needing
            // approval: persist state keyed by a fresh proposal id and
            // hand the user a proposal to accept or decline. The server's
            // read-only claim is shown, never obeyed (spec 33 D1).
            if cx.force_approval
                || self
                    .config
                    .mcp_tool_requires_approval(&server_id, &tool_name)
            {
                let proposal_id = create_id("mcp");
                let proposal = mcp_approval_proposal(
                    &proposal_id,
                    &server_id,
                    &tool_name,
                    &arguments_json,
                    cx.mcp.tool_read_only_hint(&server_id, &tool_name),
                    cx.force_approval_cause.as_ref(),
                );
                let response = proposal.prompt.clone();
                self.pending_commands.save(&PendingChatTurn {
                    mcp_call: Some(PendingMcpCall {
                        server_id,
                        tool_name,
                        arguments_json,
                    }),
                    ..cx.paused_turn(proposal_id, cx.matched_tool_call.clone())
                })?;
                self.note_pending_approvals(
                    cx.session,
                    cx.task,
                    vec![PendingApproval {
                        kind: "mcp_tool".to_string(),
                        proposal_id: proposal.id.clone(),
                    }],
                );
                // A clean stop for a human decision, not a crash: nothing was
                // sent to the server, so the marker is finished rather than
                // left for recovery to read as a remote call of unknown outcome.
                self.session_store
                    .finish_action(action_marker, "awaiting_approval")?;
                return Ok(ControlFlow::Break(RoundStop {
                    response,
                    proposals: TurnProposals {
                        command: Some(proposal),
                        ..Default::default()
                    },
                }));
            }

            // No approval required: run it now and feed the result back.
            let summary = mcp_call_summary(&server_id, &tool_name);
            let (content, outcome) = match cx.mcp.call_tool(
                &server_id,
                &tool_name,
                &arguments_json,
                crate::mcp::McpApprovalDecision::NotRequired,
            ) {
                Ok(result) => {
                    let text = self.scanner.redact(&result.text).text;
                    // `is_error` is the server's own verdict on its
                    // call. Reaching the server is not the same as
                    // the call working, and only the server knows
                    // which happened.
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
                    "The user stopped this MCP call; its outcome is \
                                         unknown."
                        .to_string(),
                    ActionOutcome::Cancelled,
                ),
                Err(error) => (
                    format!("MCP tool call failed: {error}"),
                    ActionOutcome::Failed,
                ),
            };
            (summary, content, outcome)
        };
        Ok(ControlFlow::Continue((action_marker, result)))
    }

    /// Spec 65 §5.1: notes mutate turn state that only this
    /// loop owns, so they are dispatched here rather than in
    /// the concurrent batch. The text is redacted before it
    /// is measured, stored, logged, sent or counted.
    fn handle_turn_notes(
        &self,
        cx: &mut CallContext<'_, '_>,
        text: String,
        replace: bool,
    ) -> Result<ToolResult> {
        let result = {
            let limit = self.config.agent_max_turn_notes_chars;
            if limit == 0 {
                (
                    "Attempted to write notes.".to_string(),
                    "turn_notes is not available in this session.".to_string(),
                    ActionOutcome::Failed,
                )
            } else {
                let redacted = self.scanner.redact(&text).text;
                let chars = redacted.chars().count();
                let current = cx.turn_notes.text.chars().count();
                let separator = if cx.turn_notes.text.is_empty() { 0 } else { 2 };
                let fits = if replace {
                    chars <= limit
                } else {
                    current + separator + chars <= limit
                };
                if !fits {
                    let detail = if replace {
                        format!(
                            "The replacement is {chars} characters, over the \
                                             {limit}-character limit; shorten it."
                        )
                    } else {
                        format!(
                            "Adding {chars} more characters to {current} would \
                                             exceed the {limit}-character limit; rewrite the \
                                             notes shorter with replace."
                        )
                    };
                    (
                        "Attempted to write notes.".to_string(),
                        format!("notes: {current} / {limit} characters. {detail}"),
                        ActionOutcome::Failed,
                    )
                } else {
                    if replace {
                        cx.turn_notes.text = redacted;
                        cx.turn_notes.carried = false;
                    } else {
                        if !cx.turn_notes.text.is_empty() {
                            cx.turn_notes.text.push_str("\n\n");
                        }
                        cx.turn_notes.text.push_str(&redacted);
                    }
                    let total = cx.turn_notes.text.chars().count();
                    // The per-change log event carries the text
                    // (already redacted); the audit carries only
                    // sizes, never text (requirement 11).
                    self.session_store.record_turn_notes(
                        &cx.session.id,
                        &cx.task.id,
                        replace,
                        cx.turn_notes.carried,
                        &cx.turn_notes.text,
                        chars,
                    )?;
                    self.audit_log.record(
                        "turn_notes_recorded",
                        &[
                            ("actor", "model".to_string()),
                            ("sessionId", cx.session.id.clone()),
                            ("taskId", cx.task.id.clone()),
                            ("replace", replace.to_string()),
                            ("carried", cx.turn_notes.carried.to_string()),
                            ("chars", chars.to_string()),
                            ("totalChars", total.to_string()),
                        ],
                    )?;
                    (
                        if replace {
                            "Updated notes.".to_string()
                        } else {
                            "Wrote notes.".to_string()
                        },
                        format!("notes: {total} / {limit} characters."),
                        ActionOutcome::Ok,
                    )
                }
            }
        };
        Ok(result)
    }

    /// Spec 29 §5.11: a suggestion is a proposal at most.
    /// The gate screens it; the user confirms through a
    /// separate route. A refusal here is not an error the
    /// model can fix by rephrasing a credential.
    fn handle_propose_memory(
        &self,
        cx: &mut CallContext<'_, '_>,
        statement: String,
        category: MemoryCategory,
        scope: Option<MemoryScope>,
        source: Option<String>,
        evidence_path: Option<String>,
    ) -> Result<ToolResult> {
        let result = {
            let origin = if evidence_path.is_some() {
                ProposalOrigin::RepositoryObservation {
                    session_id: cx.session.id.clone(),
                }
            } else if let Some(source) = source {
                ProposalOrigin::ExternalContent {
                    session_id: cx.session.id.clone(),
                    source,
                }
            } else {
                ProposalOrigin::ModelSuggestion {
                    session_id: cx.session.id.clone(),
                    task_id: cx.task.id.clone(),
                }
            };
            let evidence = evidence_path
                .as_deref()
                .map(|path| vec![MemoryEvidence::new(path, String::new())])
                .unwrap_or_default();
            let proposal = MemoryProposal::new(
                scope.unwrap_or(MemoryScope::Project),
                category,
                statement,
                origin,
                evidence,
                Some(cx.task.id.clone()),
            );
            match self
                .memory_store
                .propose_with_evidence(cx.repository_root, proposal)
            {
                Ok(Some(_)) => (
                    "Suggested a memory.".to_string(),
                    "A memory proposal was created for the user to confirm. It \
                                     is not stored until they confirm it."
                        .to_string(),
                    ActionOutcome::Ok,
                ),
                Ok(None) => (
                    "Attempted to suggest a memory.".to_string(),
                    "Damaian did not propose that memory: the memory policy \
                                     refused it, so the user sees no card."
                        .to_string(),
                    ActionOutcome::Failed,
                ),
                Err(error) => (
                    "Attempted to suggest a memory.".to_string(),
                    format!("The proposal could not be recorded: {error}"),
                    ActionOutcome::Failed,
                ),
            }
        };
        Ok(result)
    }

    fn handle_ask_user(
        &self,
        cx: &mut CallContext<'_, '_>,
        input: AskUserInput,
        action_marker: ActionMarker,
    ) -> Result<CallFlow> {
        let result = {
            // Requirement 5: the cap is read from the log, so a
            // resumed turn counts what it already asked and an
            // in-memory counter cannot reset it.
            let asked = self
                .session_store
                .questions_asked_in_task(&cx.session.id, &cx.task.id)?;
            let cap = self.config.agent_max_questions_per_turn as usize;
            if asked >= cap {
                (
                    "Attempted to ask the user a question.".to_string(),
                    format!(
                        "A question was not asked: this turn has already asked \
                                         {asked} of {cap} questions. Proceed without asking more, \
                                         and say in your answer what you assumed and why."
                    ),
                    ActionOutcome::Failed,
                )
            } else {
                // Requirement 7: the question is model-authored
                // and may be quoting repository content, so it
                // is redacted before it is stored, shown or
                // sent anywhere.
                let redacted_input = AskUserInput {
                    question: self.scanner.redact(&input.question).text,
                    options: input
                        .options
                        .iter()
                        .map(|option| self.scanner.redact(option).text)
                        .collect(),
                    allow_free_text: input.allow_free_text,
                };
                if let Err(error) = redacted_input.validate() {
                    // Redaction can lengthen a question enough
                    // to break a bound. That is still the
                    // model's to fix, not a reason to fail the
                    // turn.
                    (
                        "Attempted to ask the user a question.".to_string(),
                        format!("The question was not asked: {error}"),
                        ActionOutcome::Failed,
                    )
                } else {
                    let question = PendingQuestionRef::new();
                    self.session_store.record_question_asked(
                        &cx.session.id,
                        &cx.task.id,
                        &question,
                        &redacted_input,
                    )?;
                    self.pending_commands.save(&PendingChatTurn {
                        question: Some(PendingQuestion {
                            question_id: question.question_id().to_string(),
                            kind: PendingQuestionKind::Agent,
                        }),
                        ..cx.paused_turn(
                            question.question_id().to_string(),
                            cx.matched_tool_call.clone(),
                        )
                    })?;
                    // A clean stop for a human decision, not a
                    // crash: the marker says what it waits on.
                    self.session_store
                        .finish_action(action_marker, "awaiting_answer")?;
                    let response = QUESTION_PAUSED_RESPONSE.to_string();
                    return Ok(ControlFlow::Break(RoundStop {
                        response,
                        proposals: TurnProposals {
                            question: Some(AgentQuestionProposal {
                                question_id: question.question_id().to_string(),
                                question: redacted_input.question,
                                options: redacted_input.options,
                                allow_free_text: redacted_input.allow_free_text,
                            }),
                            ..Default::default()
                        },
                    }));
                }
            }
        };
        Ok(ControlFlow::Continue((action_marker, result)))
    }
}
