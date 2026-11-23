use super::*;

impl ChatOrchestrator {
    /// The budget a plan review offers, or `None` when this is not a worker.
    ///
    /// Worker status comes from the durable worker record, not from prompt or
    /// repository text and not from a caller saying "hand this to a worker":
    /// only a session the engine itself spawned into a worktree may be offered
    /// a task budget. An inline review in the same repository stays
    /// budgetless.
    pub(super) fn worker_task_budget_review(
        &self,
        session_id: &str,
    ) -> Result<Option<TaskBudgetReview>> {
        if self.session_store.worker_record(session_id)?.is_none() {
            return Ok(None);
        }
        let policy = self.config.task_budget_policy();
        Ok(Some(TaskBudgetReview::from_policy(&policy)))
    }

    /// The prompt a worker gets for a turn paused on its plan review (spec 94):
    /// the request that started the turn and the plan as the user left it on
    /// the card. Read-only, so the pause survives a worker dialog the user
    /// cancels.
    pub fn plan_worker_prompt(
        &self,
        proposal_id: &str,
        revised: Option<Vec<PlanRevisionStep>>,
    ) -> Result<PlanWorkerPrompt> {
        let pending = self.pending_commands.load(proposal_id)?;
        if pending.plan_review.is_none() {
            return Err(ClientError::InvalidInput(format!(
                "{proposal_id} is not waiting for a plan review"
            )));
        }
        let plan = self
            .session_store
            .read_task_plan(&pending.session.id, &pending.task.id)?;
        let repository_root = PathBuf::from(&pending.repository_root);
        let steps: Vec<String> = match (revised, plan) {
            (Some(revision), Some(current)) => {
                apply_plan_revision(&repository_root, &current, &revision)?
                    .steps
                    .into_iter()
                    .filter(|step| !step.status.is_terminal())
                    .map(|step| step.title)
                    .collect()
            }
            (_, Some(current)) => current
                .steps
                .into_iter()
                .filter(|step| !step.status.is_terminal())
                .map(|step| step.title)
                .collect(),
            (_, None) => Vec::new(),
        };
        // The request that set the work out, not a typed "continue": the
        // latest prompt the user typed that says something.
        let request = self
            .session_store
            .read_tasks(&pending.session.id)?
            .into_iter()
            .rev()
            .map(|task| task.user_prompt)
            .find(|prompt| {
                !is_damaian_task_prompt(prompt) && prompt.split_whitespace().count() >= 4
            })
            .unwrap_or_else(|| pending.task.user_prompt.clone());
        let mut prompt = request.trim().to_string();
        if !steps.is_empty() {
            // Spec 108 D7: Run in a worker is offered only after Approve, so
            // the plan is handed over approved. The engine, not this text,
            // decides that: the spawn snapshots the approval, and a plan with
            // other steps is reviewed in the worker as usual.
            prompt.push_str(
                "\n\nThis work was handed over from another session, where the user approved this plan. \
                 Propose it as your plan with propose_plan, these steps with these exact titles in this order, then work through it:\n",
            );
            for (index, step) in steps.iter().enumerate() {
                prompt.push_str(&format!("{}. {step}\n", index + 1));
            }
        }
        Ok(PlanWorkerPrompt {
            prompt,
            task_id: pending.task.id,
        })
    }

    /// Ends a turn paused on its plan review because the user started a
    /// worker for it (spec 94). Nothing has changed in this checkout, and
    /// nothing will: the open steps are skipped here, as for a declined plan,
    /// and the turn finishes with a note naming the worker. The model is not
    /// called.
    pub fn hand_plan_to_worker(
        &self,
        proposal_id: &str,
        worker_session_id: &str,
        handed_by: &str,
    ) -> Result<()> {
        let pending = self.pending_commands.take(proposal_id)?;
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::PlanReview)?;
        if let Some(mut plan) = self
            .session_store
            .read_task_plan(&pending.session.id, &pending.task.id)?
        {
            let now = now_millis();
            for step in plan
                .steps
                .iter_mut()
                .filter(|step| !step.status.is_terminal())
            {
                step.status = crate::plan::StepStatus::Skipped;
                step.completed_at_ms = Some(now);
                self.session_store.update_plan_step(&pending.task, step)?;
            }
        }
        self.audit_log.record(
            "plan_handed_to_worker",
            &[
                ("actor", handed_by.to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", proposal_id.to_string()),
                ("workerSessionId", worker_session_id.to_string()),
            ],
        )?;
        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "assistant",
            "Handed to a worker. It runs the approved plan in its own worktree; nothing changed here.",
        )?;
        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        // The review is resolved by the hand-off, so clear the durable pending
        // link it left on the status before the completion gate reads it. This
        // is the same transition every other resume writes; without it the
        // gate would correctly refuse to close a task that still names an
        // unanswered proposal.
        let task = self.session_store.update_task_status(
            &pending.task,
            TaskStatus::PreparingContext,
            None,
        )?;
        // Handing the plan to a worker settles this task's planned work here:
        // its steps are skipped and nothing changed in this checkout. It still
        // goes through the shared completion gate (spec 101), so this ordinary
        // turn cannot write `complete` by a path that bypasses the verdict.
        let repository_root = PathBuf::from(&pending.repository_root);
        self.completion_gate(&repository_root)
            .settle(&task, CompletionSettlement::Gate)?;
        Ok(())
    }

    /// The approved plan a worker started from a plan review is handed
    /// (spec 108 D7): the review's outstanding step titles, as the engine
    /// holds them. Read-only, like [`Self::plan_worker_prompt`], so a spawn
    /// that fails leaves the review answerable.
    ///
    /// The card offers **Run in a worker** only after **Approve**, so the spawn
    /// request that names the review is the user's approval of it, just as
    /// `approved=true` on the plan decision route is. It must name the
    /// session and task the review belongs to.
    pub fn approved_plan_handoff(
        &self,
        proposal_id: &str,
        parent_session_id: &str,
        parent_task_id: &str,
        approved_by: &str,
    ) -> Result<HandedPlan> {
        let pending = self.pending_commands.load(proposal_id)?;
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::PlanReview)?;
        if pending.session.id != parent_session_id || pending.task.id != parent_task_id {
            return Err(ClientError::InvalidInput(format!(
                "{proposal_id} is not the plan review of that session and task"
            )));
        }
        let steps: Vec<String> = self
            .session_store
            .read_task_plan(&pending.session.id, &pending.task.id)?
            .map(|plan| {
                plan.steps
                    .into_iter()
                    .filter(|step| !step.status.is_terminal())
                    .map(|step| step.title)
                    .collect()
            })
            .unwrap_or_default();
        if steps.is_empty() {
            return Err(ClientError::InvalidInput(format!(
                "{proposal_id} has no outstanding steps to hand over"
            )));
        }
        Ok(HandedPlan {
            parent_session_id: parent_session_id.to_string(),
            parent_task_id: parent_task_id.to_string(),
            proposal_id: proposal_id.to_string(),
            steps,
            approved_by: approved_by.to_string(),
        })
    }

    /// Approves a worker's freshly proposed plan when it is the plan the user
    /// approved before handing it over (spec 108 D7), and returns the note the
    /// model gets with the `propose_plan` result. `None` leaves the plan to the
    /// review gate as usual.
    ///
    /// Only a durable worker record and the spawn's [`HandedPlan`] snapshot
    /// count — never the prompt — and only for the worker's first task, with
    /// exactly the handed step titles in order. A worker plan is approved with
    /// the budget its review would have offered, which the card returns
    /// unchanged (spec 103), so the allowance is the one the review proposes.
    pub(super) fn approve_handed_plan(
        &self,
        session: &Session,
        task: &Task,
        plan: &crate::plan::TaskPlan,
    ) -> Result<Option<String>> {
        let Some(record) = self.session_store.worker_record(&session.id)? else {
            return Ok(None);
        };
        let Some(handed) = self.session_store.handed_plan(&session.id)? else {
            return Ok(None);
        };
        if record.parent_session_id.as_deref() != Some(handed.parent_session_id.as_str()) {
            return Ok(None);
        }
        let first_task = self
            .session_store
            .read_tasks(&session.id)?
            .first()
            .map(|first| first.id == task.id)
            .unwrap_or(false);
        let titles: Vec<&str> = plan.steps.iter().map(|step| step.title.as_str()).collect();
        if !first_task || titles != handed.steps {
            return Ok(None);
        }
        let budget = self
            .worker_task_budget_review(&session.id)?
            .map(|review| review.budget);
        match budget {
            Some(budget) => {
                self.session_store
                    .approve_plan_with_budget(task, &handed.approved_by, &budget)?
            }
            None => self.session_store.approve_plan(task, &handed.approved_by)?,
        }
        self.audit_log.record(
            "plan_review_decision",
            &[
                ("actor", handed.approved_by.clone()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("proposalId", handed.proposal_id.clone()),
                ("parentSessionId", handed.parent_session_id.clone()),
                ("decision", "approved_before_handoff".to_string()),
            ],
        )?;
        let mut note = "The user approved this plan before handing it to you; it does not need another review. Work through it."
            .to_string();
        if let Some(budget) = budget {
            note.push_str("\n\n");
            note.push_str(&budget.approval_note());
        }
        Ok(Some(note))
    }

    /// Continues a chat turn that stopped to have its plan reviewed before
    /// taking its first mutating step (spec 21 §5.5).
    ///
    /// `revised` is the user's edit of the step list: steps in the order they
    /// should run, each naming an existing step id, with whatever title the
    /// user wants it to carry. Omitting a step deletes it. `None` means the
    /// plan was approved as proposed.
    ///
    /// For an inline or historical review, a revision is recorded as
    /// `plan_revised` and the approval as `plan_approved`, in that order, so
    /// the log holds both what the model proposed and what the user decided to
    /// run — and `approved: false` records neither, because nothing was
    /// approved. A worker review with a budget records a revision inside its
    /// one `plan_approved` event instead (spec 103 Task 2).
    ///
    /// Nothing was dispatched when the turn paused, so there is no call to
    /// execute or discard here: the turn resumes with the decision put to the
    /// model as a message, and the model asks again for whatever it still
    /// needs. Replaying the deferred call instead would run the step the user
    /// may have just deleted.
    pub fn resume_after_plan_decision(
        &self,
        proposal_id: &str,
        approved: bool,
        revised: Option<Vec<PlanRevisionStep>>,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        self.resume_after_plan_decision_with_budget(
            proposal_id,
            approved,
            revised,
            None,
            approved_by,
            model_adapter,
            sink,
        )
    }

    /// [`Self::resume_after_plan_decision`] with the exact typed budget the
    /// user reviewed (spec 103 Task 2).
    ///
    /// A budgeted worker review cannot be approved through the budgetless
    /// entry point above: a decision that omits the allowance is not the
    /// decision the user made. A budgetless review cannot be approved with an
    /// injected one either. Both refusals happen before any plan revision or
    /// approval is written, so a malformed request leaves the pending turn
    /// answerable and the log unchanged.
    #[allow(clippy::too_many_arguments)]
    pub fn resume_after_plan_decision_with_budget(
        &self,
        proposal_id: &str,
        approved: bool,
        revised: Option<Vec<PlanRevisionStep>>,
        budget: Option<&TaskExecutionBudget>,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        // Load before taking: a refused budget must leave the review pending so
        // the user can answer it again, and must not consume the turn.
        let pending = self.pending_commands.load(proposal_id)?;
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::PlanReview)?;
        let Some(review) = pending.plan_review.clone() else {
            // `refuse_unless_paused_for` just proved the pause is a plan review.
            unreachable!("a plan review pause must carry its review");
        };
        validate_plan_budget_decision(
            &review,
            approved,
            budget,
            &self.config.task_budget_policy(),
        )?;
        // The plan half is validated here, still before `take`. An invalid
        // revision must not consume the pending turn: the review has to remain
        // answerable after a refusal, and nothing durable may be written for a
        // step list this turn will never run. `SessionStore` rechecks the same
        // rules at the atomic write below.
        let current_plan = self
            .session_store
            .read_task_plan(&pending.session.id, &pending.task.id)?;
        let repository_root = PathBuf::from(&pending.repository_root);
        let revised_plan = match (approved, revised, &current_plan) {
            (true, Some(revision), Some(current)) => {
                let applied = apply_plan_revision(&repository_root, current, &revision)?;
                validate_plan_approval(&pending.task, &applied)?;
                Some(applied)
            }
            // A revision with no plan to revise is not a rewrite; the existing
            // path below records the decision without inventing steps.
            _ => None,
        };
        let pending = self.pending_commands.take(proposal_id)?;

        let mut plan = current_plan;
        let note = if !approved {
            // A declined plan is settled by the user's decision, not the
            // model's. Every non-terminal step becomes `Skipped`, so the plan
            // is not open: the reminder never asks the model to complete work
            // the user refused, and the panel reports what happened. The step
            // keeps any evidence it already had (spec 66 `context.md` D5).
            if let Some(current) = plan.as_mut() {
                let now = now_millis();
                for step in current
                    .steps
                    .iter_mut()
                    .filter(|step| !step.status.is_terminal())
                {
                    step.status = crate::plan::StepStatus::Skipped;
                    step.completed_at_ms = Some(now);
                    let skipped = step.clone();
                    self.session_store
                        .update_plan_step(&pending.task, &skipped)?;
                }
                sink.plan(current);
            }
            // The same shape as a declined command: the work does not happen,
            // and the model is told plainly rather than left to infer it from
            // a tool result that never arrives.
            "The user reviewed this plan and declined it. Do not make any change to the repository. Explain briefly what you were going to do and what you would need from the user to proceed differently."
                .to_string()
        } else {
            let reviewed_budget = review.budget.as_ref().map(|review| review.budget);
            let applied = match revised_plan {
                Some(revised_plan) => {
                    match reviewed_budget {
                        // Spec 103 Task 2: a reviewed revision and its budget
                        // are one durable decision. Neither line can survive a
                        // partial write without the other.
                        Some(budget) => self.session_store.approve_plan_with_budget_and_plan(
                            &pending.task,
                            approved_by,
                            &budget,
                            &revised_plan,
                        )?,
                        // Inline and historical reviews keep the two-event shape
                        // that predates task budgets.
                        None => {
                            self.session_store
                                .revise_plan(&pending.task, &revised_plan)?;
                            self.session_store
                                .approve_plan(&pending.task, approved_by)?;
                        }
                    }
                    Some(revised_plan)
                }
                // No revision: there is nothing to rewrite, and approving is
                // still a decision worth recording.
                None => {
                    match reviewed_budget {
                        Some(budget) => self.session_store.approve_plan_with_budget(
                            &pending.task,
                            approved_by,
                            &budget,
                        )?,
                        None => self
                            .session_store
                            .approve_plan(&pending.task, approved_by)?,
                    }
                    plan
                }
            };
            let mut note = match applied {
                Some(plan) => format!(
                    "The user reviewed the plan and approved it. The plan is now:\n{}\nContinue with the current step. You were about to: {}.",
                    plan.steps
                        .iter()
                        .enumerate()
                        .map(|(index, step)| format!("{}. {}", index + 1, step.title))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    review.deferred_action
                ),
                None => format!(
                    "The user reviewed the plan and approved it. Continue with the current step. You were about to: {}.",
                    review.deferred_action
                ),
            };
            if let Some(budget) = reviewed_budget {
                // Spec 103 Task 4: the approved plan and its exact task budget
                // travel in the same user message the model sees, so the
                // allowance is pinned as approved user content rather than
                // inferred from the plan, repository text or a tool result. A
                // budgetless review never reaches this branch, which keeps the
                // historical note byte-for-byte.
                note.push_str("\n\n");
                note.push_str(&budget.approval_note());
            }
            note
        };

        self.audit_log.record(
            "plan_review_decision",
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", proposal_id.to_string()),
                (
                    "decision",
                    if approved { "approved" } else { "declined" }.to_string(),
                ),
            ],
        )?;

        self.continue_after_plan_review(pending, note, model_adapter, sink)
    }

    /// Answers a plan review with the user's request for changes (spec 108
    /// D6): neither an approval nor a decline.
    ///
    /// The steps stay outstanding, exactly as proposed; the decision is
    /// recorded as `plan_review_decision` `changes_requested` (the audit holds
    /// the text's length, never the text) and as a `plan_changes_requested`
    /// session event, which lets the model's next `propose_plan` replace the
    /// plan. The user's words reach the model as a user message telling it not
    /// to change the repository and to propose a revised plan. The answer
    /// authorises nothing: no plan is approved here, so the revised plan — or
    /// any mutating action the model tries instead — meets the plan-review
    /// gate again (AGENTS.md "Questions are not approvals").
    ///
    /// `budget` follows the decline rule of
    /// [`Self::resume_after_plan_decision_with_budget`]: it may be omitted, and
    /// when sent must be the reviewed one.
    pub fn request_plan_changes(
        &self,
        proposal_id: &str,
        changes: &str,
        budget: Option<&TaskExecutionBudget>,
        requested_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let changes = changes.trim();
        if changes.is_empty() {
            return Err(ClientError::InvalidInput(
                "Say how the plan should be changed, or decline it without changes.".to_string(),
            ));
        }
        // Validated before `take`, as for a decision: a refusal leaves the
        // review pending and answerable.
        let pending = self.pending_commands.load(proposal_id)?;
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::PlanReview)?;
        let Some(review) = pending.plan_review.clone() else {
            unreachable!("a plan review pause must carry its review");
        };
        validate_plan_budget_decision(&review, false, budget, &self.config.task_budget_policy())?;
        let pending = self.pending_commands.take(proposal_id)?;

        // User text bound for the model: redacted like an answer, because the
        // card's text field invites pasting.
        let changes = self.scanner.redact(changes).text;
        self.session_store.request_plan_changes(
            &pending.task,
            requested_by,
            changes.chars().count(),
        )?;
        self.audit_log.record(
            "plan_review_decision",
            &[
                ("actor", requested_by.to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", proposal_id.to_string()),
                ("decision", "changes_requested".to_string()),
                ("changesLength", changes.chars().count().to_string()),
            ],
        )?;
        let note = format!(
            "The user reviewed the plan and asked for changes:\n{changes}\nDo not change the repository. Propose a revised plan with propose_plan that addresses this, then continue with it: the user reviews the revised plan before any change is made."
        );
        self.continue_after_plan_review(pending, note, model_adapter, sink)
    }

    /// The common end of every plan-review answer that continues the turn:
    /// the decision goes to the model as a user message, and the turn runs on.
    fn continue_after_plan_review(
        &self,
        pending: PendingChatTurn,
        note: String,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let turn_notes = TurnNotesState::from_pending(&pending);
        let repository_root = PathBuf::from(&pending.repository_root);
        let mut messages = pending.messages;
        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "user",
            &note,
        )?;
        messages.push(
            ModelMessage::assistant(pending.last_content.clone())
                .with_reasoning_content(pending.reasoning_content.clone()),
        );
        messages.push(ModelMessage::user(note));

        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        let task = self.session_store.update_task_status(
            &pending.task,
            TaskStatus::PreparingContext,
            None,
        )?;

        let segment_pause = pending.segment.clone();
        self.run_agentic_turn(
            &repository_root,
            pending.session,
            task,
            pending.context_files,
            messages,
            pending.round + 1,
            pending.exploration_rounds,
            turn_notes,
            CompletedToolTracker::from(pending.completed_tool_rounds.clone()),
            pending.step_evidence,
            pending.plan_reminder_sent,
            model_adapter,
            sink,
            pending.turn_options,
            segment_pause.as_ref(),
        )
    }

    /// Runs the model in a loop, letting it request tools across multiple
    /// rounds (e.g. `read_git_status` followed by `read_git_diff`) instead of
    /// stopping after one. Configured round budgets keep a provider that never
    /// stops requesting tools from running forever; the final round always
    /// drops `tools`, forcing a plain answer. If a tool needs human approval,
    /// the in-flight conversation state is persisted so the turn can be
    /// resumed later via [`Self::resume_after_command_decision`].
    // Threads the full per-turn state (session, task, messages, round) plus the
    // model adapter and token sink through one recursive-ish loop.
    /// Carries a plan onto `task` when the previous task in the session handed
    /// its work to this one with steps still outstanding.
    ///
    /// Three ways that happens, and they are the only three: the previous turn
    /// hit the token ceiling and the user raised it (spec 21 §5.4), it stopped
    /// at its tool-round budget and the user asked again (spec 61 §5.7), or it
    /// was interrupted by a crash and the user pressed `Continue`
    /// (`docs/specs/45_crash_recovery_prompt.md` §5.4), which re-sends the same
    /// request as a new turn and records the old task as superseded.
    ///
    /// **The crash case is safe for the same reason the resume was offered at
    /// all.** A plan carries the steps already completed and the evidence
    /// behind them, so carrying one after a crash would be wrong if a step
    /// could have half-happened. It cannot: a resume is refused outright for
    /// `unknown_external_outcome`, so the only crashes that reach here had
    /// nothing side-effecting in flight. Without this the resumed turn
    /// re-plans from nothing and can redo steps whose work already landed —
    /// which is worse than the token-stop case it was already avoiding.
    ///
    /// Best-effort by design: a session whose log cannot be read should not
    /// stop the user asking a question, and the worst case of skipping it is a
    /// turn that starts a fresh plan.
    pub(super) fn carry_plan_from_the_previous_turn(
        &self,
        session_id: &str,
        task: &Task,
    ) -> Result<()> {
        let tasks = self.session_store.read_tasks(session_id)?;
        // The one before this turn's own, which was appended a moment ago.
        let Some(previous) = tasks.iter().rev().find(|candidate| candidate.id != task.id) else {
            return Ok(());
        };
        let handed_over = previous.status == TaskStatus::TokenBudgetExhausted
            || previous.status == TaskStatus::ToolBudgetExhausted
            || self
                .session_store
                .superseded_task_ids(session_id)?
                .contains(&previous.id);
        if !handed_over {
            return self.carry_the_open_plan(session_id, &tasks, task);
        }
        let Some(plan) = self
            .session_store
            .read_task_plan(session_id, &previous.id)?
        else {
            return Ok(());
        };
        if plan.open_steps().is_empty() {
            return Ok(());
        }
        self.session_store.resume_plan(task, &previous.id)
    }

    /// Picks up the session's latest plan for a turn the user typed
    /// (`docs/specs/83_typed_turns_pick_up_the_open_plan.md`).
    ///
    /// A turn that ends with steps open — an answer after spec 66's reminder,
    /// a patch the user still has to review — hands nothing over by the rule
    /// above, and the user's "continue" then started with no plan. The
    /// verification after its patch carried that empty plan, and spec 77
    /// found nothing of the task's own to continue.
    ///
    /// The latest plan, not the previous task's: a turn that carried none (in
    /// Ask mode, say) does not end the work. It is not picked up when its only
    /// open steps are the verification loop's own `verify_` steps, when its
    /// task ended in a crash (recovery decides that), outside Code mode, or
    /// when the turn that last carried it left it untouched — so an unrelated
    /// question costs one turn with the plan on it. The model may also replace
    /// a carried plan it has not started on (`ProposePlan` below).
    fn carry_the_open_plan(&self, session_id: &str, tasks: &[Task], task: &Task) -> Result<()> {
        if self.session_store.session_mode(session_id) != SessionMode::Code {
            return Ok(());
        }
        let plans = self.session_store.read_session_plans(session_id)?;
        let Some((source, plan)) = tasks
            .iter()
            .rev()
            .filter(|candidate| candidate.id != task.id)
            .find_map(|candidate| plans.get(&candidate.id).map(|plan| (candidate, plan)))
        else {
            return Ok(());
        };
        if matches!(
            source.status,
            TaskStatus::Interrupted | TaskStatus::UnknownExternalOutcome
        ) {
            return Ok(());
        }
        let work_left = plan
            .open_steps()
            .iter()
            .any(|step| !step.id.starts_with("verify_"));
        if !work_left
            || self
                .session_store
                .carried_plan_untouched(session_id, &source.id)?
        {
            return Ok(());
        }
        self.session_store.resume_plan(task, &source.id)
    }

    /// The reminder to send at a completion point, and how many steps are open,
    /// or `None` when the gate does not allow one (spec 66 §5.2).
    ///
    /// The budget condition is deliberately two rounds of room: the reminder is
    /// charged before the next call, and a call landing at `max_rounds` would
    /// have its tools withheld — the model could not call `complete_step` even
    /// though it was just asked to.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn plan_reminder_note(
        &self,
        session_id: &str,
        task_id: &str,
        plan: Option<&TaskPlan>,
        reminder_sent: bool,
        action_rounds: u32,
        max_rounds: u32,
        answer_now: bool,
    ) -> Result<Option<(String, usize)>> {
        let Some(plan) = plan else {
            return Ok(None);
        };
        let open = plan.open_steps();
        let ceiling = self.token_ceiling_spend(session_id, task_id)?;
        // *Answer now* is the user's decision to end the turn with no rounds
        // left; re-entering it would be unbounded for the reason the hook is
        // skipped there (spec 66 `context.md` D2).
        if !plan_reminder_allowed(
            reminder_sent,
            open.len(),
            action_rounds,
            max_rounds,
            answer_now,
            ceiling,
        ) {
            return Ok(None);
        }
        Ok(Some((plan_reminder_message(plan), open.len())))
    }

    /// Attaches the turn's accrued evidence to whichever step is still open,
    /// in the order it was observed.
    ///
    /// Attaching is not closing: the step keeps its status, and
    /// `append_step_evidence` never recomputes one. Best-effort writes — an
    /// unwritable log must not turn a finished turn into an error, the same rule
    /// `edit.rs` follows.
    ///
    /// Two kinds of stop attach through here. A true turn end does both this
    /// and the audit of the plan it leaves open. A stop that saves no paused
    /// turn — a patch proposal (`propose_patch` or `edit_file`), the only
    /// pending approval with no pending turn — attaches only: it is a pause,
    /// not an end, so it must not audit, remind or re-judge. The pending turn
    /// that the other approvals save already carries this buffer, which is why
    /// they must not call this too (spec 66 §5.3, R13).
    pub(super) fn attach_step_evidence(
        &self,
        session: &Session,
        task: &Task,
        plan: &mut Option<TaskPlan>,
        step_evidence: &mut Vec<crate::plan::Evidence>,
    ) {
        let accrued = std::mem::take(step_evidence);
        let Some(current) = plan.as_mut() else {
            return;
        };
        if !accrued.is_empty()
            && let Some(open) = current
                .steps
                .iter_mut()
                .find(|step| step.status == crate::plan::StepStatus::InProgress)
        {
            // Extend the in-memory plan so the final panel event carries the
            // same evidence the log now does.
            open.evidence.extend(accrued.iter().cloned());
            for item in accrued {
                let _ = self
                    .session_store
                    .append_step_evidence(&session.id, &task.id, item);
            }
        }
    }

    /// Attaches the turn's accrued evidence, then audits the plan left open
    /// (spec 66 §5.2, §5.3).
    ///
    /// Only a true end calls this.
    pub(super) fn settle_plan_at_turn_end(
        &self,
        session: &Session,
        task: &Task,
        plan: &mut Option<TaskPlan>,
        step_evidence: &mut Vec<crate::plan::Evidence>,
        plan_reminder_sent: bool,
        stop_reason: &str,
    ) -> Result<()> {
        self.attach_step_evidence(session, task, plan, step_evidence);
        let Some(current) = plan.as_mut() else {
            return Ok(());
        };
        let open_steps = current.open_steps().len();
        if open_steps > 0 {
            self.audit_log.record(
                "plan_left_open",
                &[
                    ("actor", "system".to_string()),
                    ("sessionId", session.id.clone()),
                    ("taskId", task.id.clone()),
                    ("openSteps", open_steps.to_string()),
                    ("reminderSent", plan_reminder_sent.to_string()),
                    ("stopReason", stop_reason.to_string()),
                ],
            )?;
        }
        Ok(())
    }

    /// Keeps the answer that triggered the reminder, then appends the reminder
    /// itself to the request and the session log (spec 66 §5.2).
    ///
    /// The pre-reminder answer is the turn's response on a true completion, so
    /// it must reach the log even if the post-reminder reply is just "Done.".
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_plan_reminder(
        &self,
        session: &Session,
        task: &Task,
        answer: &str,
        model_run: &ModelRun,
        reminder: &str,
        open_steps: usize,
        messages: &mut Vec<ModelMessage>,
        pre_reminder_answer: &mut Option<String>,
    ) -> Result<()> {
        if !answer.trim().is_empty() {
            self.session_store
                .append_message(&session.id, Some(&task.id), "assistant", answer)?;
            *pre_reminder_answer = Some(answer.to_string());
        }
        messages.push(
            ModelMessage::assistant(answer.to_string())
                .with_reasoning_content(model_run.reasoning_content.clone()),
        );
        // Tagged so the conversation view renders it as a system line rather
        // than as something the user typed, and so a later turn's seed skips it
        // (spec 66 `context.md` D10).
        self.session_store.append_message_with_kind(
            &session.id,
            Some(&task.id),
            "user",
            reminder,
            Some("plan_reminder"),
        )?;
        messages.push(ModelMessage::user(reminder.to_string()));
        self.audit_log.record(
            "plan_reminder_sent",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("openSteps", open_steps.to_string()),
            ],
        )?;
        Ok(())
    }
}

/// Whether a plan still carries work or a completion obligation that the user
/// has not discharged: an open (`Pending` or `InProgress`) step, a `Blocked`
/// one, or a required check.
///
/// This is the completion gate's outstanding set (spec 101), extended with the
/// plan's required checks because carrying them is what keeps an `incomplete`
/// task's unmet check visible to the next run (review fix R2). The
/// `propose_plan` refusal for a continuation uses it so a replacement cannot
/// discard either before the user has reviewed the change. A plan with no open
/// or blocked step and no required check has nothing outstanding and may still
/// be replaced (spec 101 review fix R9).
pub(super) fn carried_plan_has_outstanding_work(plan: &TaskPlan) -> bool {
    !plan.open_steps().is_empty()
        || plan.steps.iter().any(|step| {
            step.status == crate::plan::StepStatus::Blocked
                || step.check.as_ref().is_some_and(|check| check.required)
        })
}

/// Whether a plan holds any of the task's own steps, rather than only the
/// `verify_` steps the verification loop adds. A task whose plan has none is
/// planless for spec 107 D3: it verifies per change and continues after a
/// verified patch.
pub(super) fn has_primary_steps(plan: &TaskPlan) -> bool {
    plan.steps
        .iter()
        .any(|step| !step.id.starts_with("verify_"))
}

/// Whether taking this action would change something outside the session log,
/// and so must wait for the plan to be reviewed (spec 21 §5.5).
///
/// Every variant is decided here by name, with no fallback. It used to defer
/// to [`tool_action_marker`]'s crash-recovery side-effect flag for everything
/// but a patch and a command, and that flag is the wrong question: proposing
/// an edit applies nothing, so `edit_file` was not side-effecting and a plan
/// whose first change was an `edit_file` call was never reviewed (2026-11-11).
/// A new `ToolAction` variant now fails to compile until it is decided here.
///
/// * `propose_patch` and `edit_file` write nothing to the repository and are
///   correctly *not* side-effecting for crash recovery — but each is the front
///   door to an edit, and §5.5 exists to put the plan in front of the user
///   before edits start.
/// * `run_command` is answered by `command_needs_approval`, not by the action
///   alone. Whether a command mutates depends on the command, and the command
///   policy is the only thing that knows: a sandbox-safe `ls` is a read, and
///   gating on it would put a plan up for approval for a turn that only looks
///   at things — the noise §5.5 warns the user learns to click through.
pub(super) fn action_awaits_plan_review(
    action: &ToolAction,
    command_needs_approval: impl FnOnce(&str) -> bool,
) -> bool {
    match action {
        // Proposals of a repository change.
        ToolAction::ProposePatch(_) | ToolAction::EditFile { .. } => true,
        ToolAction::Command(request) => command_needs_approval(&request.command),
        // Reach outside the machine or into another process: an MCP tool or a
        // web diagnostic can act, and a fetch is an external outcome.
        ToolAction::McpCall { .. } | ToolAction::WebDiagnostic(_) | ToolAction::FetchUrl { .. } => {
            true
        }
        // Shape the turn or write only the session log; an answer or a memory
        // suggestion authorises nothing and changes no file.
        ToolAction::ProposePlan(_)
        | ToolAction::CompleteStep
        | ToolAction::AskUser(_)
        | ToolAction::TurnNotes { .. }
        | ToolAction::ProposeMemory { .. } => false,
        // Reads.
        ToolAction::ReadFile { .. }
        | ToolAction::ListDirectory { .. }
        | ToolAction::SearchContent { .. }
        | ToolAction::SearchCodebase { .. }
        | ToolAction::ReadGitStatus { .. }
        | ToolAction::ReadGitDiff { .. }
        | ToolAction::ListWorktrees { .. }
        | ToolAction::ReadGitLog { .. }
        // A search calls no tool; the call it leads to meets the row above.
        | ToolAction::McpToolSearch { .. } => false,
    }
}

/// A model-proposed check with its directory in `CheckId` form, or `None` when
/// that directory cannot be repository-relative. Dropping it is the same
/// outcome as a check with no command: the step keeps its prose and no check
/// obligation is invented from a path the engine cannot trust.
pub(super) fn normalized_proposed_check(
    repository_root: &Path,
    check: Option<PlannedCheck>,
) -> Option<PlannedCheck> {
    let mut check = check?;
    check.working_directory =
        normalize_planned_check_directory(repository_root, &check.working_directory)?;
    Some(check)
}

/// Rebuilds a plan from the user's revision, in the order the user put the
/// steps in.
///
/// A step the user omits is dropped — **unless it has already reached a
/// terminal status**. A completed or blocked step carries evidence tied to a
/// state of the repository, and deleting it would leave a plan whose history
/// no longer describes what happened: the reason §5.5 rules out mid-execution
/// editing in the first place. Terminal steps keep their original order, ahead
/// of whatever the user chose to keep, so what already ran still reads as
/// having run first.
///
/// A revision naming a step the plan does not have is ignored, for the same
/// reason `read_task_plan` ignores an update for an unknown step id: the step
/// list is not something a revision may grow.
///
/// Everything but the title and the check is carried over from the existing
/// step. The user is reordering, retitling and shaping the structured check,
/// not asserting anything about its status or its evidence — those remain the
/// engine's to decide (§3). A `Preserve` check leaves the existing one in
/// place; `Remove` clears it; `Set` replaces it with what the user approved,
/// after its directory is normalized to the `CheckId` form. A directory that is
/// outside the repository is refused: the review stays answerable rather than
/// storing a check that can never match.
fn apply_plan_revision(
    repository_root: &Path,
    current: &TaskPlan,
    revision: &[PlanRevisionStep],
) -> Result<TaskPlan> {
    let mut revised = current.clone();
    let mut steps: Vec<crate::plan::PlanStep> = current
        .steps
        .iter()
        .filter(|step| step.status.is_terminal())
        .cloned()
        .collect();
    for edit in revision {
        let Some(existing) = current.steps.iter().find(|step| step.id == edit.id) else {
            continue;
        };
        if existing.status.is_terminal() {
            continue;
        }
        let mut step = existing.clone();
        step.title = edit.title.clone();
        match &edit.check {
            PlanRevisionCheck::Preserve => {}
            PlanRevisionCheck::Remove => step.check = None,
            PlanRevisionCheck::Set(check) => {
                let mut check = check.clone();
                check.working_directory = normalize_planned_check_directory(
                    repository_root,
                    &check.working_directory,
                )
                .ok_or_else(|| {
                    ClientError::InvalidInput(format!(
                        "The check directory `{}` is outside this repository. Use a repository-relative directory such as `crates/foo`, or the repository root.",
                        check.working_directory
                    ))
                })?;
                step.check = Some(check);
            }
        }
        steps.push(step);
    }
    revised.steps = steps;
    Ok(revised)
}

/// Checks the budget half of a plan-review decision before anything is written.
///
/// The reviewed budget is the engine's proposal: a client may send it back or
/// omit it on a decline, and may never change, raise or invent one. The
/// effective ceiling is read again here rather than trusted from the pending
/// turn, because configuration can change while a review waits — a budget that
/// was valid when offered must still fit the ceiling it is approved under.
/// Only an approval actually spends the allowance, so only an approval is
/// measured against that re-read ceiling: a lowered ceiling must never leave
/// the user unable to decline work they no longer want to fund.
///
/// A budgetless (inline or historical) review keeps its old behaviour, which
/// includes rejecting a budget a client injected into a decision that never
/// offered one.
fn validate_plan_budget_decision(
    review: &PendingPlanReview,
    approved: bool,
    supplied: Option<&TaskExecutionBudget>,
    policy: &TaskBudgetPolicy,
) -> Result<()> {
    let Some(reviewed) = &review.budget else {
        return match supplied {
            None => Ok(()),
            Some(_) => Err(ClientError::InvalidInput(
                "this plan review offers no task budget, so none can be supplied".to_string(),
            )),
        };
    };
    let Some(supplied) = supplied else {
        // Declining needs no allowance: the user is refusing the work, and the
        // worker control socket's reject may therefore omit it.
        return if approved {
            Err(ClientError::InvalidInput(
                "approving this worker plan requires the exact reviewed task budget".to_string(),
            ))
        } else {
            Ok(())
        };
    };
    // A client that does send the budget back must return the reviewed one
    // unchanged. That holds for a decline too: it records no allowance, so
    // accepting a different one would only hide a client bug.
    if supplied != &reviewed.budget {
        return Err(ClientError::InvalidInput(
            "the task budget does not match the one reviewed".to_string(),
        ));
    }
    supplied.validate()?;
    if approved {
        supplied.validate_against(&policy.ceiling)?;
    }
    Ok(())
}

/// The one reminder a turn gets when it would end with steps still open (spec
/// 66 §5.2).
///
/// A user message rather than a system one, so it sits at the end of the
/// request where the model reads it and `bounded_messages` treats it like any
/// other round. It asks the model to close what it finished — it never closes
/// anything itself, and #21 requirement 6 still decides each status from the
/// step's own evidence.
/// The gate the reminder passes through (spec 66 §5.2).
///
/// Split from [`ChatOrchestrator::plan_reminder_note`] so the decision can be
/// tested without a turn built around it, the way the plan rules themselves
/// are. `ceiling` is `(ceiling, spent)` after the answer round, or `None` with
/// no ceiling configured.
pub(super) fn plan_reminder_allowed(
    reminder_sent: bool,
    open_steps: usize,
    action_rounds: u32,
    max_rounds: u32,
    answer_now: bool,
    ceiling: Option<(u64, u64)>,
) -> bool {
    if reminder_sent || answer_now || open_steps == 0 {
        return false;
    }
    // Two rounds of room, not one: the reminder is charged before the next
    // call, and a call landing at the limit has its tools withheld.
    if action_rounds + 1 >= max_rounds {
        return false;
    }
    !matches!(ceiling, Some((ceiling, spent)) if spent >= ceiling)
}

pub(super) fn plan_reminder_message(plan: &TaskPlan) -> String {
    let open = plan.open_steps();
    let mut text = String::from("Your plan still has open steps:\n");
    for (index, step) in open.iter().enumerate() {
        text.push_str(&format!("  {}. {}", index + 1, step.title));
        if step.status == crate::plan::StepStatus::InProgress {
            text.push_str(" (in progress)");
        }
        text.push('\n');
    }
    text.push_str(
        "Call complete_step once for each step whose work is done — Damaian decides its status from what the step ran. If a step was not done, answer again without calling it, and the turn ends with that step open.",
    );
    text
}

pub(super) fn plan_review_response(plan: &TaskPlan, deferred_action: &str) -> String {
    let mut text = String::from("Before going further, here is the plan:\n");
    for (index, step) in plan.steps.iter().enumerate() {
        text.push_str(&format!("{}. {}\n", index + 1, step.title));
    }
    text.push_str(&format!(
        "\nWaiting for your review before the first step that changes anything ({deferred_action}). You can approve the plan, or decline it and say what to change."
    ));
    text
}
