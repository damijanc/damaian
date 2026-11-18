use super::*;

impl ChatOrchestrator {
    /// Answers the question a turn paused on (spec 50 `context.md` §7).
    ///
    /// The answer goes back as a **tool result**, not a user message: the
    /// paused call is replayed and its result is the user's answer, so the
    /// model sees the exchange as a tool round and the question consumes one
    /// action round, exactly as an uninterrupted `ask_user` would.
    ///
    /// An answer authorises nothing. This method only ever appends a tool
    /// result; every side-effecting tool the model asks for next still passes
    /// through its own approval or review.
    pub fn resume_after_answer(
        &self,
        question_id: &str,
        answer: &str,
        selected_option: Option<usize>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(question_id)?;
        let turn_notes = TurnNotesState::from_pending(&pending);
        self.refuse_unless_paused_for(question_id, &pending, PausedFor::Question)?;
        let Some(question) = pending.question.clone() else {
            // `refuse_unless_paused_for` just proved the pause is a question.
            unreachable!("a question pause must carry its question id");
        };
        let Some(state) = self
            .session_store
            .question_state(&pending.session.id, &question.question_id)?
        else {
            // The question was rewound out of the active conversation, so
            // there is nothing left to answer. Put the pause back rather than
            // consume it.
            self.pending_commands.save(&pending)?;
            return Err(ClientError::InvalidInput(format!(
                "Unknown question: {}",
                question.question_id
            )));
        };

        // The answer is user text, but it becomes a tool result that leaves
        // the protected boundary, so it is redacted on the way in. The turn's
        // own prompt is stored and sent as written; this path is stricter
        // because the result is engine-authored text like any other tool
        // result (spec 50 `context.md` §7).
        let answer = self.scanner.redact(answer).text;

        // An already-answered question is never asked again (requirement 4):
        // the recorded answer wins and a new one is ignored, even if it would
        // not have been valid. That is the state a crash between recording the
        // answer and resuming the turn leaves behind.
        let (answer, selected_option) = match state.answer.clone() {
            Some(recorded) => recorded,
            None => {
                if let Err(error) = validate_answer(&state.input(), &answer, selected_option) {
                    self.pending_commands.save(&pending)?;
                    return Err(ClientError::InvalidInput(error));
                }
                match self.session_store.record_question_answered(
                    &pending.session.id,
                    &question.question_id,
                    &answer,
                    selected_option,
                ) {
                    Ok(updated) => updated.answer.unwrap_or((answer, selected_option)),
                    Err(error) => {
                        // Another route may have answered it between the read
                        // and the write. Use what is recorded; otherwise the
                        // pause is put back so the answer can be tried again.
                        match self
                            .session_store
                            .question_state(&pending.session.id, &question.question_id)?
                            .and_then(|state| state.answer)
                        {
                            Some(recorded) => recorded,
                            None => {
                                self.pending_commands.save(&pending)?;
                                return Err(error);
                            }
                        }
                    }
                }
            }
        };

        let tool_result = answered_question_result(&answer, selected_option, &state.options);
        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "assistant",
            &format!("Asked: \"{}\"", state.question),
        )?;
        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "tool",
            &tool_result,
        )?;

        // The engine's own uncertain-repair question is answered through the
        // same durable route as a model question, but its answer starts one
        // ordinary repair turn (or leaves the failure reported) instead of
        // continuing the paused model turn. The answer itself authorises
        // nothing: the repair patch it produces still needs its own approval.
        if question.kind == PendingQuestionKind::RepairDecision {
            return self.resume_repair_decision(
                &pending,
                &answer,
                selected_option,
                model_adapter,
                sink,
            );
        }

        let repository_root = PathBuf::from(&pending.repository_root);
        let mut messages = pending.messages;
        if let Some(call) = &pending.matched_tool_call {
            // Same round shape as the command resume: the paused call joins
            // the round's one assistant message when earlier calls ran, and
            // its answer is the tool result that follows it.
            let round_message = pending
                .round_calls_fed_back
                .then(|| {
                    messages
                        .iter()
                        .rposition(|message| !message.tool_calls.is_empty())
                })
                .flatten();
            match round_message {
                Some(index) => messages[index].tool_calls.push(call.clone()),
                None => messages.push(ModelMessage::assistant_with_tool_calls(
                    pending.last_content.clone(),
                    vec![call.clone()],
                    pending.reasoning_content.clone(),
                )),
            }
            messages.push(ModelMessage::tool(call.id.clone(), tool_result));
        } else {
            // `ask_user` exists only as a native call, so this is unreachable.
            // It mirrors the command resume's text fallback rather than
            // inventing a second rule.
            messages.push(
                ModelMessage::assistant(pending.last_content.clone())
                    .with_reasoning_content(pending.reasoning_content.clone()),
            );
            messages.push(ModelMessage::user(tool_result));
        }

        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        let task = self.session_store.update_task_status(
            &pending.task,
            TaskStatus::PreparingContext,
            None,
        )?;

        let completed_tool_rounds = match pending
            .matched_tool_call
            .as_ref()
            .and_then(|call| tool_action_from_call(call).ok().flatten())
        {
            Some(action) => self.tracker_with_resumed_call(
                &pending.session.id,
                &pending.task.id,
                &pending.completed_tool_rounds,
                pending.matched_tool_call.as_ref(),
                &repository_root,
                &action,
                &ActionOutcome::Ok,
            )?,
            None => CompletedToolTracker::from(pending.completed_tool_rounds.clone()),
        };

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
            completed_tool_rounds,
            pending.step_evidence,
            pending.plan_reminder_sent,
            model_adapter,
            sink,
            pending.turn_options,
            segment_pause.as_ref(),
        )
    }
}

/// The answer as the model receives it, in `context.md` §7's fixed format. The
/// option is named only when the user picked one; a free-text answer carries
/// its text alone. The answer is already redacted by the caller.
fn answered_question_result(
    answer: &str,
    selected_option: Option<usize>,
    options: &[String],
) -> String {
    match selected_option.and_then(|index| options.get(index).map(|label| (index, label))) {
        Some((index, label)) => {
            format!("The user answered: \"{answer}\" (option {index}: \"{label}\")")
        }
        None => format!("The user answered: \"{answer}\""),
    }
}
