use super::*;

impl ChatOrchestrator {
    /// Signature deliberately unchanged: this is the entry point for
    /// `damaian-cli` and the engine's own tests, neither of which has anything
    /// to cancel or anywhere to show progress. Only the desktop shell needs
    /// [`Self::ask_with_session`]'s full sink.
    pub fn ask(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        model_adapter: &mut dyn ModelAdapter,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ChatTurnResult> {
        let never_cancelled = CancelToken::new();
        let mut discard_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token,
            on_progress: &mut discard_progress,
            cancel: &never_cancelled,
            steering: None,
        };
        self.ask_with_session(
            repository_root,
            prompt,
            explicit_paths,
            None,
            model_adapter,
            &mut sink,
        )
    }

    pub fn ask_with_session(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        session_id: Option<&str>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        self.ask_with_session_with_options(
            repository_root,
            prompt,
            explicit_paths,
            session_id,
            model_adapter,
            sink,
            ChatTurnOptions::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ask_with_session_with_options(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        session_id: Option<&str>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
        options: ChatTurnOptions,
    ) -> Result<ChatTurnResult> {
        self.ask_with_session_inner(
            repository_root,
            prompt,
            explicit_paths,
            session_id,
            None,
            model_adapter,
            sink,
            options,
        )
    }

    /// Runs an already-created task instead of creating one (spec 100).
    ///
    /// **Continue with result** creates the parent task under the session
    /// store's cross-process lock, together with the consumed marker, then
    /// calls this. A crash after that transaction leaves one ordinary
    /// `Created` task for recovery rather than a runnable result card.
    #[allow(clippy::too_many_arguments)]
    pub fn ask_with_session_using_task(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        task: Task,
        position: u64,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
        options: ChatTurnOptions,
    ) -> Result<ChatTurnResult> {
        let session_id = task.session_id.clone();
        self.ask_with_session_inner(
            repository_root,
            prompt,
            explicit_paths,
            Some(&session_id),
            Some((task, position)),
            model_adapter,
            sink,
            options,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn ask_with_session_inner(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        session_id: Option<&str>,
        existing_task: Option<(Task, u64)>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
        options: ChatTurnOptions,
    ) -> Result<ChatTurnResult> {
        let repository_root = repository_root.as_ref();
        let index = crate::index_cache::IndexCache::get_or_build(&self.indexer, repository_root)?;
        // Context a `session_start` or `before_context` hook returned, in the
        // order the events ran (spec 32 `context.md` §10). It is ordinary
        // context: `build_context_with_hooks` redacts it and charges it to the
        // turn's budget like every other item.
        let mut hook_context: Vec<(String, String)> = Vec::new();
        let session = if let Some(session_id) = session_id {
            let Some(session) = self.session_store.read_session(session_id)? else {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown session: {session_id}"
                )));
            };
            if session.repository_id != index.repository_id {
                return Err(ClientError::AccessDenied(
                    "Session belongs to a different repository".to_string(),
                ));
            }
            session
        } else {
            // Spec 32 `context.md` §3: the event runs before the session
            // exists, so the envelope carries no session id and there is no
            // task. `RequestApproval` has no approval step here, so it denies
            // like `deny` does (§4). A denial creates nothing: no session, no
            // task, and no model call.
            let ctx = HookContext::root();
            let parts = self.hooks.envelope_parts(None, None, serde_json::json!({}));
            let outcome =
                self.hooks
                    .dispatch(HookEvent::SessionStart, parts, repository_root, &ctx);
            if matches!(outcome.verdict, Verdict::Deny | Verdict::RequestApproval) {
                return Err(ClientError::PolicyBlocked(hook_refusal(&outcome)));
            }
            let session_start_findings = outcome.findings;
            hook_context.extend(outcome.context);
            let session = self
                .session_store
                .create_session(&index.repository_id, &session_title(prompt))?;
            // Warnings are findings too, and now there is a log to keep them
            // in. Requested commands are not run: `session_start` has no task,
            // and §9 refuses a command at an event with no task.
            for finding in &session_start_findings {
                let _ = self.session_store.record_finding(&session.id, finding);
            }
            session
        };
        // Captured before the turn's own events, so rewinding this checkpoint
        // takes the user's prompt with it rather than leaving a question the
        // conversation no longer answers. A continuation already has its task
        // and the position the store recorded just before creating it.
        //
        // A pre-created task or a worker's own session is constructed text, not
        // a direct user request, so only this ordinary path reads the prompt
        // for protected paths (spec 98 §2.1).
        let is_direct_user_task =
            existing_task.is_none() && self.session_store.worker_record(&session.id)?.is_none();
        let (position, mut task) = match existing_task {
            Some((task, position)) => {
                // This is a public entry point, so it must not be usable to
                // rerun a terminal task or to claim a task this turn did not
                // create. The caller builds the task and its prompt through the
                // same store transaction; these checks keep the boundary from
                // accepting anything else (spec 100).
                if task.status != TaskStatus::Created {
                    return Err(ClientError::InvalidInput(
                        "A pre-created task must still be in the created state".to_string(),
                    ));
                }
                if task.session_id != session.id {
                    return Err(ClientError::InvalidInput(
                        "The task belongs to a different session".to_string(),
                    ));
                }
                if task.user_prompt != prompt {
                    return Err(ClientError::InvalidInput(
                        "The task prompt does not match this turn".to_string(),
                    ));
                }
                if task.model_provider != self.config.model_provider
                    || task.model_name != self.config.model_name
                {
                    return Err(ClientError::InvalidInput(
                        "The task model does not match this turn".to_string(),
                    ));
                }
                (position, task)
            }
            None => {
                let position = self.session_store.latest_event_seq(&session.id)?;
                let task = self.session_store.create_task(
                    &session.id,
                    prompt,
                    &self.config.model_provider,
                    &self.config.model_name,
                )?;
                (position, task)
            }
        };
        // A worker's first task receives the parent task's spawn-time snapshot
        // before anything can reach the model. This is the cross-session
        // handoff edge; later continuations inherit through ordinary ancestry.
        self.seed_worker_inherited_protected_paths(&session.id, &task)?;
        // Context assembly is the next thing to happen, and it is read-only,
        // so a crash here is resumable. The finer in-flight states are set at
        // the action sites when their markers go in.
        task = self
            .session_store
            .update_task_status(&task, TaskStatus::PreparingContext, None)?;
        // §5.4: a turn stopped by the token ceiling is recoverable — "the user
        // can raise the ceiling and resume, and the remaining steps are what
        // resumption starts from". A task is one turn, so this new task would
        // otherwise start with no plan at all (`context.md` §3.1).
        //
        // The same is true of a turn a crash interrupted and the user chose to
        // continue (`docs/specs/45_crash_recovery_prompt.md` §5.8), and of a
        // tool-round stop the user asked again after (spec 61 §5.7).
        //
        // Narrow on purpose: only a task that actually handed its work to this
        // one — a ceiling stop, a tool-round stop or a recorded supersession —
        // and only while its plan still has work outstanding. Carrying a plan
        // into an unrelated next question would put steps on the panel the user
        // never asked for, and a normally-completed task has nothing to resume.
        //
        // A continue-incomplete run already wrote its `plan_resumed` edge in
        // the same locked transaction as `task_created` (spec 101 Task 3), so
        // it must not be carried twice.
        if !options.plan_already_carried {
            self.carry_plan_from_the_previous_turn(&session.id, &task)?;
        }
        // Spec 65 §5.5: the model's notes from the previous turn are this
        // turn's starting record, pinned from the first request and marked as
        // carried. Read from the active log, so a rewind carries the notes of
        // the turn before the rewound one with no second ordering rule.
        let turn_notes = self.carry_notes_from_the_previous_turn(&session.id, &task)?;
        // Spec 55 §5.5: compaction runs at the turn boundary, before the request
        // is assembled and never between tool rounds. It runs *before* this
        // turn's user message is appended, so the current request cannot be
        // inside the summarised span even if it alone exceeds the window. A
        // failure never fails the turn; it is audited and the request uses the
        // recency window instead.
        let mut conversation = self.session_store.read_conversation(&session.id)?;
        if self.config.conversation_auto_compact
            && crate::compaction::conversation_tokens(&conversation)
                > self.conversation_window_tokens()
        {
            match self.compact_conversation(
                repository_root,
                &session.id,
                model_adapter,
                sink.cancel,
            ) {
                Ok(crate::compaction::CompactionOutcome::Compacted(_)) => {
                    conversation = self.session_store.read_conversation(&session.id)?;
                }
                // Nothing outside the window, the user stopped the turn, or the
                // attempt failed. A failure was already audited and recorded by
                // `compact_conversation`, which is why the manual route reports
                // it too; the turn continues either way (proposal §5.7).
                Ok(
                    crate::compaction::CompactionOutcome::NothingToCompact
                    | crate::compaction::CompactionOutcome::Cancelled
                    | crate::compaction::CompactionOutcome::Failed(_),
                )
                | Err(_) => {}
            }
        }
        let user_message =
            self.session_store
                .append_message(&session.id, Some(&task.id), "user", prompt)?;
        if is_direct_user_task {
            self.record_direct_prompt_protected_paths(
                repository_root,
                &session.id,
                &task.id,
                prompt,
            )?;
        }
        // Spec 29 §5.2: an explicit request is still a proposal. The user's own
        // sentence is the statement, so no model paraphrase or extra round trip
        // sits between what they asked and what the card will show.
        self.propose_explicit_memory(
            repository_root,
            &session.id,
            Some(&task.id),
            &user_message.id,
            prompt,
        )?;
        self.take_turn_checkpoint(
            repository_root,
            &session,
            &task,
            prompt,
            position,
            &user_message.id,
        );

        // Before context assembly, which can take a while on a large repository:
        // a client that stops during it must still know which session it stopped.
        sink.session(&session.id);
        sink.phase(
            PhaseKind::Context,
            "",
            0,
            self.tool_round_limit(prompt_enters_web_debug_mode(prompt)),
            0,
            self.default_exploration_round_limit(),
            turn_notes.text.chars().count(),
        );

        // Spec 32 `context.md` §3, §4: context is assembled once per turn, so
        // this is the one place a hook's additions can become ordinary
        // `hook_context` items. `RequestApproval` has no approval step here, so
        // it denies like `deny` does. A refused turn never reaches the model.
        let before_context = {
            let ctx = HookContext::root();
            let parts = self.hooks.envelope_parts(
                Some(&session),
                Some(&task.id),
                serde_json::json!({"flow": "chat", "explicitPaths": explicit_paths}),
            );
            let mut outcome =
                self.hooks
                    .dispatch(HookEvent::BeforeContext, parts, repository_root, &ctx);
            // Warnings are findings too, so this records both (`context.md`
            // §10). A finding that could not be journalled does not decide the
            // turn either way.
            for finding in &outcome.findings {
                let _ = self.session_store.record_finding(&session.id, finding);
            }
            if matches!(outcome.verdict, Verdict::Deny | Verdict::RequestApproval) {
                let message = hook_refusal(&outcome);
                self.session_store.append_message(
                    &session.id,
                    Some(&task.id),
                    "assistant",
                    &message,
                )?;
                let _ = self.session_store.update_task_status_with_kind(
                    &task,
                    TaskStatus::Failed,
                    Some(&message),
                    Some("hook_denied"),
                );
                return Err(ClientError::PolicyBlocked(message));
            }
            // A `before_context` hook's requested commands run under the
            // session's mode and profile, like any other (`context.md` §9).
            if !outcome.commands.is_empty() {
                let mode = self.session_store.session_mode(&session.id);
                let requested = self.hooks.run_requested_commands(
                    &mut outcome,
                    &session,
                    Some(&task),
                    mode,
                    repository_root,
                    &ctx,
                );
                for finding in &requested {
                    let _ = self.session_store.record_finding(&session.id, finding);
                }
            }
            outcome.context
        };
        hook_context.extend(before_context);

        // The plan a resumed turn carries (`carry_plan_from_the_previous_turn`
        // above). Read once here and handed to assembly so the model sees where
        // the previous turn got to. `run_agentic_turn` reads the same plan again
        // for its own mutable copy; nothing writes one between the two reads.
        let carried_plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)
            .ok()
            .flatten();
        // Session-scoped controls (spec 27 §5.4), read once before assembly so
        // every source sees the same list.
        let context_pins = self
            .session_store
            .context_pins(&session.id)
            .unwrap_or_default();
        let context_restriction = self.session_store.context_restriction(&session.id)?;
        // Spec 51 D5: pages fetched earlier in this session are assembled here,
        // as the last and lowest-priority category. A page fetched in this very
        // turn is already a tool result; it reaches the plan on the next turn.
        let external_references = self.session_store.read_external_references(&session.id)?;
        // Spec 30 §5.3: recall before assembly, from the turn's own prompt and
        // named paths. The entries travel as ordinary context items; there is
        // no second path to the model (§5.1).
        let (project_identity, recalled) = self.recall_memory_for_turn(
            repository_root,
            &index.repository_id,
            &session.id,
            prompt,
            explicit_paths,
        )?;
        let mut context = self.context_manager.build_context(ContextRequest {
            repository_root,
            repository_id: &index.repository_id,
            task_id: &task.id,
            prompt,
            index: Some(&index),
            explicit_paths,
            limit: self.config.context_token_budget(),
            window: self.config.context_window(),
            hook_context: &hook_context,
            plan: carried_plan.as_ref(),
            pinned: &context_pins,
            restriction: context_restriction.as_deref(),
            conversation_summary: conversation.summary.as_ref(),
            external_references: &external_references,
            memory: &recalled,
        })?;
        // Spec 30 §5.6: the world can change between assembly and the call.
        // This drops an entry that is no longer sendable and its manifest row,
        // before the manifest is streamed, so what the user is shown is what
        // the request carries.
        let recalled = self.recheck_memory_before_send(
            repository_root,
            &project_identity,
            &session.id,
            &mut context,
            recalled,
        )?;
        // What this turn is about to send (spec 27 §5.1, C9). Sent before the
        // first call, so the default (no pause) still gives the user a
        // pre-send view; the shell relays it as its `context` event.
        //
        // The matching `context_assembled` record is written only once the turn
        // is committed to sending (below, and on `Send` in
        // `resume_after_context_decision`). A pause has assembled a plan but
        // sent nothing, and a declined turn never sends: recording at assembly
        // would put a "what this turn was sent" manifest on a task that sent
        // nothing (proposal §5.3, C9). Shown once the messages are built, so
        // it can count what they add (spec 79).
        let prompt_conversation = build_conversation(
            &conversation.messages,
            conversation.summary.as_ref(),
            self.conversation_window_tokens(),
            &self.scanner,
        );
        let model_prompt =
            build_model_prompt_with_conversation(&context, &prompt_conversation, None);
        let mode = self.session_store.session_mode(&session.id);
        // A worker is never invited to hand off further work (spec 57 §5.0).
        let offer_handoffs = self.config.enable_worker_sessions
            && self.session_store.worker_record(&session.id)?.is_none();
        let messages = vec![
            ModelMessage::system(system_prompt_for_session(
                mode,
                &options,
                offer_handoffs,
                &self.config.shell,
            )),
            ModelMessage::user(model_prompt),
        ];
        note_turn_overhead(&mut context.manifest, &prompt_conversation, &messages);
        sink.context(&context.manifest);

        // The one interruption point between assembly and sending (spec 27
        // §5.1). Off by default: a confirmation on every message is how the
        // feature gets abandoned. Nothing has been sent when this pauses.
        if self.config.pause_for_context_review {
            return self.pause_for_context_review(
                repository_root,
                session,
                task,
                context,
                prompt_conversation,
                conversation.summary.clone(),
                recalled,
                messages,
                turn_notes,
                options,
            );
        }

        // The turn is now committed to sending, so this is its one
        // `context_assembled` record. The manager holds no audit log and adding
        // one would be a second audit path for one event (proposal §5.5,
        // Correction C9). The manifest holds no content, and `AuditLog::record`
        // redacts every field value anyway.
        let mut context_fields = vec![
            ("actor", "system".to_string()),
            ("sessionId", session.id.clone()),
        ];
        context_fields.extend(context.manifest.audit_fields());
        self.audit_log
            .record("context_assembled", &context_fields)?;
        // Spec 30 C9: a recall is recorded at send, not at assembly. A pause
        // returned above, and a declined review never reaches here.
        self.record_memory_recall(&context.manifest, &recalled)?;

        self.run_agentic_turn(
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
        )
    }
}
