use super::*;

impl ChatOrchestrator {
    /// Continues a turn paused on a host approval before a fetch (spec 51
    /// §5.3).
    ///
    /// `Once` and `Always` perform the request the dialog showed; `Deny`
    /// performs none and records a tool result telling the model the host was
    /// refused, so it can answer honestly rather than retrying. `Always` writes
    /// the per-repository key to **user** config through
    /// [`Config::grant_web_domain_always`] before the request, so the
    /// same host does not pause again in this checkout while another checkout
    /// still does.
    pub fn resume_after_fetch_decision(
        &self,
        proposal_id: &str,
        decision: FetchDecision,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(proposal_id)?;
        let mut turn_notes = TurnNotesState::from_pending(&pending);
        self.refuse_unless_paused_for(proposal_id, &pending, PausedFor::Fetch)?;
        let Some(fetch_call) = pending.fetch_call.clone() else {
            // `refuse_unless_paused_for` just proved the pause is a fetch.
            unreachable!("a fetch pause must carry its call");
        };
        let repository_root = PathBuf::from(&pending.repository_root);
        let repository_id = pending.session.repository_id.clone();
        let decision_name = match decision {
            FetchDecision::Once => "approved_once",
            FetchDecision::Always => "approved_always",
            FetchDecision::Deny => "denied",
        };
        if decision == FetchDecision::Always {
            // A failed write must not consume the pause: the user's decision is
            // still unanswered, so it is put back for the same route to retry
            // (the command route writes before resuming for the same reason).
            if let Err(error) = Config::grant_web_domain_always(
                &self.config,
                &repository_root,
                &fetch_call.approval_key,
            ) {
                self.pending_commands.save(&pending)?;
                return Err(error);
            }
        }
        // The host, the decision and the checkout only. The URL's query is the
        // part that could carry data out, and the audit does not need it.
        self.audit_log.record(
            "web_fetch_approval_decision",
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", proposal_id.to_string()),
                ("host", fetch_call.host.clone()),
                ("decision", decision_name.to_string()),
                ("repositoryId", repository_id),
            ],
        )?;

        let mut messages = pending.messages.clone();
        let mut after_tool: Option<(ToolAction, ActionOutcome)> = None;
        let (assistant_summary, tool_result_content) = if decision == FetchDecision::Deny {
            (
                format!("Attempted to fetch from {}.", fetch_call.host),
                format!(
                    "The user refused to let this task read from {}. Nothing was requested. \
                     Do not ask for this host again in this turn; answer with what you already \
                     know, and say the page could not be read if it matters.",
                    fetch_call.host
                ),
            )
        } else {
            // Re-run the egress guard against the stored URL before the request.
            // The string cannot have changed since the pause, but user config
            // and the files this task read can, and a refusal is still a
            // refusal — the approval is not a way around constraint 3.
            let guard_content = read_files_from_messages(&pending.messages);
            let files_read: Vec<ReadFileContent<'_>> = guard_content
                .iter()
                .map(|(path, content)| ReadFileContent { path, content })
                .collect();
            let allowed = self.web_domain_allowlist_for(&repository_root);
            let inspection = match inspect_with_policy(
                &fetch_call.url,
                &allowed,
                &self.scanner,
                &files_read,
                self.allow_loopback_fetch,
            ) {
                Ok(inspection) => inspection,
                Err(error) => {
                    let result = format!(
                        "Cannot fetch that URL: {}. Nothing was requested.",
                        error.reason()
                    );
                    self.append_fetch_exchange(
                        &pending,
                        &mut messages,
                        &mut turn_notes,
                        "Attempted to fetch a URL.".to_string(),
                        &result,
                        None,
                        &repository_root,
                    )?;
                    return self.resume_fetch_turn(
                        pending,
                        messages,
                        turn_notes,
                        model_adapter,
                        sink,
                    );
                }
            };
            if let Decision::Refused(reason) = &inspection.decision {
                let result = format!(
                    "Cannot fetch from {}: {}. Nothing was requested.",
                    inspection.url.host,
                    reason.message()
                );
                self.append_fetch_exchange(
                    &pending,
                    &mut messages,
                    &mut turn_notes,
                    format!("Attempted to fetch from {}.", inspection.url.host),
                    &result,
                    None,
                    &repository_root,
                )?;
                return self.resume_fetch_turn(pending, messages, turn_notes, model_adapter, sink);
            }
            // A host that was approved meanwhile is a standing grant; a host
            // the user approved just now is a one-off unless `Always` wrote it.
            let decision_basis = match (&inspection.decision, decision) {
                (Decision::AlreadyAllowed, _) | (_, FetchDecision::Always) => None,
                _ => Some("approved_once"),
            };
            let (summary, content, action_outcome) = self.fetch_after_inspection(
                &repository_root,
                &pending.session,
                &pending.task,
                &inspection.url,
                decision_basis,
                sink.cancel,
            )?;
            after_tool = Some((
                ToolAction::FetchUrl {
                    url: fetch_call.url.clone(),
                },
                action_outcome,
            ));
            (summary, content)
        };

        self.append_fetch_exchange(
            &pending,
            &mut messages,
            &mut turn_notes,
            assistant_summary,
            &tool_result_content,
            after_tool.as_ref(),
            &repository_root,
        )?;
        self.resume_fetch_turn(pending, messages, turn_notes, model_adapter, sink)
    }

    /// Feeds a fetch decision back to the model and continues the turn.
    ///
    /// Split from [`Self::resume_after_fetch_decision`] because every refusal
    /// path and both approvals need the identical tail: one assistant summary,
    /// one tool result, the pending link cleared, and the loop resumed one round
    /// on.
    fn resume_fetch_turn(
        &self,
        pending: PendingChatTurn,
        messages: Vec<ModelMessage>,
        turn_notes: TurnNotesState,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let repository_root = PathBuf::from(&pending.repository_root);
        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        let task = self.session_store.update_task_status(
            &pending.task,
            TaskStatus::PreparingContext,
            None,
        )?;
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
            CompletedToolTracker::from(pending.completed_tool_rounds.clone()),
            pending.step_evidence,
            pending.plan_reminder_sent,
            model_adapter,
            sink,
            pending.turn_options,
            segment_pause.as_ref(),
        )?;
        if pending.turn_options.verification_turn {
            return self.close_verification_resume(&repository_root, result);
        }
        Ok(result)
    }

    /// Appends one fetch call's assistant summary and tool result, and records
    /// the call so the per-turn cap and the repeat check see it.
    ///
    /// A fetch pause happens before the call is fed back, so the main loop never
    /// records it. Without this the resumed call would not count toward
    /// `web_fetch_max_per_turn` and an identical retry would not be a repeat.
    #[allow(clippy::too_many_arguments)]
    fn append_fetch_exchange(
        &self,
        pending: &PendingChatTurn,
        messages: &mut Vec<ModelMessage>,
        turn_notes: &mut TurnNotesState,
        assistant_summary: String,
        tool_result: &str,
        after_tool: Option<&(ToolAction, ActionOutcome)>,
        repository_root: &Path,
    ) -> Result<()> {
        let mut tool_result = tool_result.to_string();
        if let Some((action, outcome)) = after_tool {
            let payload = tool_event_payload(&self.scanner, action, Some(outcome), None);
            let after = self.dispatch_tool_event(
                HookEvent::AfterTool,
                &pending.session,
                &pending.task,
                repository_root,
                payload,
            );
            if let Some(note) = hook_after_note(&after) {
                tool_result.push_str("\n\n");
                tool_result.push_str(&note);
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
            &tool_result,
        )?;

        let message_index = if let Some(call) = &pending.matched_tool_call {
            let round_message = pending
                .round_calls_fed_back
                .then(|| messages.iter().rposition(|m| !m.tool_calls.is_empty()))
                .flatten();
            match round_message {
                Some(index) => {
                    messages[index].tool_calls.push(call.clone());
                    index
                }
                None => {
                    let index = messages.len();
                    messages.push(ModelMessage::assistant_with_tool_calls(
                        pending.last_content.clone(),
                        vec![call.clone()],
                        pending.reasoning_content.clone(),
                    ));
                    index
                }
            }
        } else {
            // `fetch_url` exists only as a native call, so this is unreachable.
            // It mirrors the command resume's text fallback rather than
            // inventing a second rule.
            messages.push(
                ModelMessage::assistant(pending.last_content.clone())
                    .with_reasoning_content(pending.reasoning_content.clone()),
            );
            messages.push(ModelMessage::user(format!("Fetch result:\n{tool_result}")));
            0
        };
        if pending.matched_tool_call.is_some() {
            messages.push(ModelMessage::tool(
                pending
                    .matched_tool_call
                    .as_ref()
                    .map(|call| call.id.clone())
                    .unwrap_or_default(),
                tool_result,
            ));
        }

        if pending.matched_tool_call.is_some() {
            let url = pending
                .fetch_call
                .as_ref()
                .map(|call| call.url.clone())
                .unwrap_or_default();
            let line = self
                .render_call_line(repository_root, &ToolAction::FetchUrl { url: url.clone() })
                .map(|line| {
                    truncate_for_prompt(
                        &single_line(&self.scanner.redact(&line).text),
                        CALL_LINE_MAX_CHARS,
                    )
                })
                .unwrap_or_default();
            turn_notes.calls.push(RecordedCall {
                round: pending.round + pending.exploration_rounds + 1,
                message_index,
                line,
                key: Some(vec!["fetch_url".to_string(), url]),
                writes: true,
                compacted: false,
            });
        }
        Ok(())
    }

    /// Dispatch one `fetch_url` call (spec 51 §5.3, §5.5, D5, D9, D12).
    ///
    /// Every refusal is a failed tool result the model can read — an
    /// unparsable URL, a secret or repository content in it, an unapproved
    /// host, the per-turn cap, a redirect, a transport error — never an aborted
    /// turn. A successful page comes back in the fixed external-content frame
    /// and is recorded for the next assembly (D5).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn dispatch_fetch_url(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        url: &str,
        calls: &[RecordedCall],
        messages: &[ModelMessage],
        cancel: &CancelToken,
    ) -> Result<FetchDispatch> {
        // Spec 51 §5.6: the cap is read from config and counted from this
        // turn's own call list, which a pause carries, so a resumed turn cannot
        // refresh it.
        let cap = self.config.web_fetch_max_per_turn as usize;
        let used = calls
            .iter()
            .filter(|call| {
                call.key
                    .as_ref()
                    .is_some_and(|key| key.first().map(String::as_str) == Some("fetch_url"))
            })
            .count();
        if used >= cap {
            return Ok(FetchDispatch::Result(
                "Attempted to fetch a URL.".to_string(),
                format!(
                    "A fetch was not made: this turn has already fetched {used} of {cap} pages. \
                     Answer with what you have instead of fetching more."
                ),
                ActionOutcome::Failed,
            ));
        }

        // D9: exactly what `read_file` returned this task, never files reached
        // another way, and never the file as it is on disk now — the model saw
        // the tool result, and that is what a URL could carry out. The guard
        // runs before the host decision, so an approved host cannot carry a
        // credential out.
        let guard_content = read_files_from_messages(messages);
        let files_read: Vec<ReadFileContent<'_>> = guard_content
            .iter()
            .map(|(path, content)| ReadFileContent { path, content })
            .collect();
        // The allowlist is read afresh for this checkout, because `Allow
        // Always` writes user config during the session and the loaded config
        // is a snapshot from before that write (D8, spec 34 §5.4).
        let allowed = self.web_domain_allowlist_for(repository_root);
        let inspection = match inspect_with_policy(
            url,
            &allowed,
            &self.scanner,
            &files_read,
            self.allow_loopback_fetch,
        ) {
            Ok(inspection) => inspection,
            Err(error) => {
                return Ok(FetchDispatch::Result(
                    "Attempted to fetch a URL.".to_string(),
                    format!(
                        "Cannot fetch that URL: {}. Nothing was requested.",
                        error.reason()
                    ),
                    ActionOutcome::Failed,
                ));
            }
        };
        let host = inspection.url.host.clone();
        match &inspection.decision {
            Decision::Refused(reason) => Ok(FetchDispatch::Result(
                format!("Attempted to fetch from {host}."),
                format!(
                    "Cannot fetch from {host}: {}. Nothing was requested.",
                    reason.message()
                ),
                ActionOutcome::Failed,
            )),
            // Nothing has been requested: the turn pauses before the transport
            // is built, and `FetchDecision` decides what happens next.
            Decision::NeedsApproval => Ok(FetchDispatch::NeedsApproval(inspection)),
            Decision::AlreadyAllowed => {
                let (summary, content, outcome) = self.fetch_after_inspection(
                    repository_root,
                    session,
                    task,
                    &inspection.url,
                    None,
                    cancel,
                )?;
                Ok(FetchDispatch::Result(summary, content, outcome))
            }
        }
    }

    /// The hosts approved for this checkout, from the config this engine
    /// loaded plus the user config on disk.
    ///
    /// `Allow Always` writes `web_domain_allowlist.<repository id>` to user
    /// config, and the engine's `Config` was built before that write. Re-reading
    /// the map here is what makes the next fetch in the same session honour the
    /// grant. Only user config is read, and only the map keyed by this
    /// checkout's id, so a repository config still cannot add a host (Task 2
    /// refuses that key at repository scope).
    fn web_domain_allowlist_for(&self, repository_root: &Path) -> Vec<String> {
        let mut entries = self.config.web_domain_allowlist.clone();
        for entry in self.repository_web_domains_for(repository_root) {
            if !entries
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&entry))
            {
                entries.push(entry);
            }
        }
        entries
    }

    /// The `Allow Always` grants recorded for one checkout, newest source
    /// first: user config on disk, then the map the engine loaded.
    fn repository_web_domains_for(&self, repository_root: &Path) -> Vec<String> {
        let repository_id = repository_id_for_root(repository_root);
        let mut entries: Vec<String> = Vec::new();
        if let Ok(overlay) = ConfigOverlay::load_or_default(self.config.user_config_path())
            && let Some(granted) = overlay
                .web_domain_allowlist_by_repository
                .get(&repository_id)
        {
            entries.extend(granted.iter().cloned());
        }
        if let Some(granted) = self
            .config
            .web_domain_allowlist_by_repository
            .get(&repository_id)
        {
            for entry in granted {
                if !entries
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(entry))
                {
                    entries.push(entry.clone());
                }
            }
        }
        entries
    }

    /// Runs the request for an allowed or just-approved URL and records it.
    ///
    /// `decision_basis` is `Some` only when the reason is not in the allowlist
    /// — a one-off approval — so the audit names what actually happened rather
    /// than claiming a standing grant.
    fn fetch_after_inspection(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        url: &ExternalUrl,
        decision_basis: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<(String, String, ActionOutcome)> {
        let host = url.host.clone();
        let mut transport = CurlFetchTransport::new(
            ProcessRegistry::open(&self.config.data_dir)?,
            &self.config.data_dir,
            &session.id,
            SecretScanner::new(self.config.secret_patterns.clone()),
        );
        if self.allow_loopback_fetch {
            transport = transport.with_non_public_allowed_for_tests();
        }
        let outcome = match transport.fetch(url, cancel) {
            Ok(outcome) => outcome,
            // A stop belongs to the turn: the caller closes the marker and ends
            // the turn cancelled instead of recording a result (spec 08).
            Err(ClientError::Cancelled) => {
                return Ok((
                    format!("Attempted to fetch from {host}."),
                    "The fetch was stopped.".to_string(),
                    ActionOutcome::Cancelled,
                ));
            }
            Err(error) => {
                return Ok((
                    format!("Attempted to fetch from {host}."),
                    format!("Cannot fetch from {host}: {error}. Check the URL and try again."),
                    ActionOutcome::Failed,
                ));
            }
        };

        // D10: no redirect is followed, of any kind. The target is named so the
        // model can make a new call, which needs that host's own approval.
        if let Some(target) = &outcome.redirect_to {
            let location = self.scanner.redact(&target.location).text;
            self.audit_log.record(
                "web_fetch_redirect",
                &[
                    ("actor", "assistant".to_string()),
                    ("sessionId", session.id.clone()),
                    ("taskId", task.id.clone()),
                    ("url", url.request_url()),
                    ("host", host.clone()),
                    ("status", outcome.status.to_string()),
                    ("location", location.clone()),
                    ("crossHost", target.cross_host.to_string()),
                ],
            )?;
            return Ok((
                format!("Attempted to fetch from {host}."),
                format!(
                    "The page answered with a redirect to {location}{}. This tool does not \
                     follow redirects. Fetch that URL as a new call if you need it; its host \
                     needs its own approval.",
                    if target.cross_host {
                        " (a different host)"
                    } else {
                        ""
                    }
                ),
                ActionOutcome::Failed,
            ));
        }

        let converted = ConvertedResponse::from_outcome(url, &outcome, &self.scanner);
        // D5: the page enters the next assembly as the lowest-priority context
        // category. The payload is the converted, redacted, bounded text and
        // never the raw body; `contentHash` lets a reader verify the stored
        // text without storing the bytes again.
        let reference = ExternalReference::from_converted(&converted);
        self.session_store
            .record_external_reference(&session.id, &reference)?;
        let content_hash = crate::hash::sha256_hex(reference.text.as_bytes());
        // The audit basis is the per-repository grant, read now rather than
        // snapshotted at load, so an `Allow Always` write in this session is
        // visible to the next fetch (context.md §4).
        let approval_basis = match decision_basis {
            Some(basis) => basis.to_string(),
            None => {
                let repository_entries = self.repository_web_domains_for(repository_root);
                if HostAllowlist::from_config(&repository_entries).permits(url) {
                    "repository_allowlist".to_string()
                } else {
                    "user_allowlist".to_string()
                }
            }
        };
        self.audit_log.record(
            "web_fetch",
            &[
                ("actor", "assistant".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("url", url.request_url()),
                ("host", host.clone()),
                ("status", converted.status.to_string()),
                ("bytes", converted.bytes.to_string()),
                (
                    "contentType",
                    converted.content_type.clone().unwrap_or_default(),
                ),
                ("cached", converted.cached.to_string()),
                ("approval", approval_basis),
                ("contentHash", content_hash),
            ],
        )?;

        Ok((
            format!("Fetched {host}"),
            converted.render_for_model(),
            ActionOutcome::Ok,
        ))
    }
}
