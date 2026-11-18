use super::*;

impl ChatOrchestrator {
    /// Takes the checkpoint the turn can be rewound to. Best-effort: a store
    /// that cannot be written is recorded in the audit log and the turn still
    /// runs, because refusing to answer at all would be a worse failure than
    /// losing the ability to rewind one turn.
    pub(super) fn take_turn_checkpoint(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        prompt: &str,
        position: u64,
        user_message_id: &str,
    ) {
        let summary = checkpoint_summary(prompt);
        let created = self.checkpoint_store.create_checkpoint(
            repository_root,
            CheckpointRequest {
                session_id: &session.id,
                task_id: Some(&task.id),
                user_message_id: Some(user_message_id),
                summary: &summary,
                conversation: CheckpointConversation {
                    last_event_seq: position,
                    task_status: TaskStatus::PreparingContext.as_str().to_string(),
                },
                pending_approvals: Vec::new(),
                // Nothing has happened yet: paths arrive as the turn accepts a
                // patch or runs an approved command.
                paths: Vec::new(),
            },
        );
        match created {
            Ok(_) => {
                // Retention runs here rather than on a timer: it is cheap
                // unless something actually expired, and a turn is the moment
                // a new checkpoint was just added.
                let _ = self
                    .checkpoint_store
                    .cleanup(&session.repository_id, Some(&session.id));
            }
            Err(error) => {
                let _ = self.audit_log.record(
                    "checkpoint_creation_failed",
                    &[
                        ("actor", "system".to_string()),
                        ("sessionId", session.id.clone()),
                        ("taskId", task.id.clone()),
                        ("error", error.to_string()),
                    ],
                );
            }
        }
    }

    /// Records on the turn's checkpoint what it is waiting on, or clears it
    /// once the decision is in. Best-effort, like the rest of the checkpoint
    /// path: a missing note is worth less than a failed turn.
    pub(super) fn note_pending_approvals(
        &self,
        session: &Session,
        task: &Task,
        pending_approvals: Vec<PendingApproval>,
    ) {
        let Ok(Some(manifest)) = self
            .checkpoint_store
            .read_checkpoint_for_task(&session.repository_id, &task.id)
        else {
            return;
        };
        let _ = self
            .checkpoint_store
            .set_pending_approvals(&manifest, pending_approvals);
    }

    /// Turns a finished command into findings and appends them to the
    /// session (spec 22 `context.md` §13.4). Returns the evidence linking the
    /// open step to them, or `None` when the run found nothing.
    pub(super) fn record_command_findings(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        execution: &CommandExecution,
    ) -> Result<Option<crate::plan::Evidence>> {
        let findings = findings_from_execution(execution, &default_parsers(), &self.scanner);
        let evidence = self.record_findings(
            repository_root,
            Path::new(&execution.working_directory),
            &session.id,
            &task.id,
            &execution.id,
            findings,
        )?;
        // `after_check` runs only for validation commands, and only after the
        // findings it describes are already in the log (`context.md` §3). The
        // ids are evidence of what this check reported, not a path or content.
        if is_validation_command(&execution.command) {
            let finding_ids = match &evidence {
                Some(crate::plan::Evidence::Findings { refs, .. }) => refs.clone(),
                _ => Vec::new(),
            };
            let payload = serde_json::json!({
                "command": execution.command,
                "exitCode": execution.exit_code,
                "termination": execution.termination.as_str(),
                "findingIds": finding_ids,
            });
            let outcome = self.dispatch_tool_event(
                HookEvent::AfterCheck,
                session,
                task,
                repository_root,
                payload,
            );
            self.record_after_refusal(&session.id, &task.id, HookEvent::AfterCheck, &outcome);
        }
        Ok(evidence)
    }

    /// Dispatches one per-action event, records its findings, and runs any
    /// commands it requested (`context.md` §9). Every action dispatch goes
    /// through here so a hook's findings cannot be recorded at one call site
    /// and forgotten at another.
    pub(super) fn dispatch_tool_event(
        &self,
        event: HookEvent,
        session: &Session,
        task: &Task,
        repository_root: &Path,
        payload: serde_json::Value,
    ) -> HookOutcome {
        let parts = self
            .hooks
            .envelope_parts(Some(session), Some(&task.id), payload);
        let mut outcome = self
            .hooks
            .dispatch(event, parts, repository_root, &HookContext::root());
        for finding in &outcome.findings {
            let _ = self.session_store.record_finding(&session.id, finding);
        }
        if !outcome.commands.is_empty() {
            let mode = self.session_store.session_mode(&session.id);
            let requested = self.hooks.run_requested_commands(
                &mut outcome,
                session,
                Some(task),
                mode,
                repository_root,
                &HookContext::root(),
            );
            for finding in &requested {
                let _ = self.session_store.record_finding(&session.id, finding);
            }
        }
        outcome
    }

    /// Records what an `after_*` verdict means (`context.md` §4): a `deny` is
    /// an error finding and undoes nothing; `request_approval` has no approval
    /// step left and is recorded as a warning.
    pub(super) fn record_after_refusal(
        &self,
        session_id: &str,
        task_id: &str,
        event: HookEvent,
        outcome: &HookOutcome,
    ) {
        let (severity, label) = match outcome.verdict {
            Verdict::Deny => (Severity::Error, "refused"),
            Verdict::RequestApproval => (Severity::Warning, "asked for approval"),
            Verdict::Allow | Verdict::Warn => return,
        };
        for (hook_id, reason) in &outcome.reasons {
            let draft = FindingDraft {
                source: FindingSource::Hook,
                severity,
                summary: format!(
                    "Hook `{hook_id}` {label} after {}: {reason}",
                    event.as_str()
                ),
                details: None,
                range: None,
                code: None,
            };
            let finding = Finding::new(draft, &self.scanner)
                .with_task_id(task_id.to_string())
                .with_origin_ref(format!("hook:{hook_id}"));
            let _ = self.session_store.record_finding(session_id, &finding);
        }
    }

    /// A `request_approval` at a tool with its own review step is already
    /// satisfied (`context.md` §4): the patch is reviewed before it applies.
    /// It still leaves a warning, so the hook's request is visible.
    pub(super) fn record_satisfied_approval(
        &self,
        session_id: &str,
        task_id: &str,
        outcome: &HookOutcome,
    ) {
        if outcome.verdict != Verdict::RequestApproval {
            return;
        }
        for (hook_id, reason) in &outcome.reasons {
            let draft = FindingDraft {
                source: FindingSource::Hook,
                severity: Severity::Warning,
                summary: format!(
                    "Hook `{hook_id}` asked for approval before this tool, and the patch review \
                     already provides it: {reason}"
                ),
                details: None,
                range: None,
                code: None,
            };
            let finding = Finding::new(draft, &self.scanner)
                .with_task_id(task_id.to_string())
                .with_origin_ref(format!("hook:{hook_id}"));
            let _ = self.session_store.record_finding(session_id, &finding);
        }
    }

    /// A recorded browser diagnostic's findings. The repository is walked
    /// only when a console problem has a location to map (§14).
    pub(super) fn record_web_findings(
        &self,
        repository_root: &Path,
        session_id: &str,
        record: &WebDiagnosticRecord,
    ) -> Result<()> {
        let files = if has_located_console_problem(&record.report) {
            repository_files(repository_root, &self.config.ignore_patterns)
        } else {
            Vec::new()
        };
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        let findings = findings_from_web_record(record, &files, &self.scanner);
        self.record_findings(
            repository_root,
            repository_root,
            session_id,
            &record.task_id,
            &record.id,
            findings,
        )
        .map(|_| ())
    }

    /// Keeps a range only when its printed path resolves to a file in the
    /// repository, hashing the file then. The printed path is resolved from
    /// the command's working directory, so a tool in a sub-root keeps its
    /// range (spec 24 `context.md` §8). Attaches the task and origin, and
    /// appends each finding (`context.md` §13.4).
    fn record_findings(
        &self,
        repository_root: &Path,
        working_directory: &Path,
        session_id: &str,
        task_id: &str,
        origin_ref: &str,
        findings: Vec<Finding>,
    ) -> Result<Option<crate::plan::Evidence>> {
        let mut refs = Vec::new();
        let mut failing = 0;
        for finding in findings {
            let mut finding = finding.with_task_id(task_id).with_origin_ref(origin_ref);
            if let Some(printed) = finding.range().map(|range| range.path.clone()) {
                finding = match resolve_finding_path(repository_root, working_directory, &printed) {
                    Some(relative) => match crate::hash::file_hash(repository_root.join(&relative))
                    {
                        Ok(hash) => finding.with_range_path(relative).with_file_hash(hash),
                        Err(_) => finding.without_range(),
                    },
                    None => finding.without_range(),
                };
            }
            if finding.severity() == Severity::Error {
                failing += 1;
            }
            self.session_store.record_finding(session_id, &finding)?;
            refs.push(finding.id().to_string());
        }
        Ok((!refs.is_empty()).then_some(crate::plan::Evidence::Findings { refs, failing }))
    }

    /// Adds what an approved command changed to the turn's checkpoint, and
    /// returns the paths it changed, `None` when the census could not tell.
    /// Best-effort, like the rest of the checkpoint path.
    pub(super) fn record_command_effects(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        census: &CommandCensus,
    ) -> Option<Vec<String>> {
        let after = self
            .checkpoint_store
            .begin_command_census(repository_root)
            .unwrap_or_else(|_| CommandCensus::unavailable());
        if let Ok(Some(manifest)) = self
            .checkpoint_store
            .read_checkpoint_for_task(&session.repository_id, &task.id)
        {
            let _ = self.checkpoint_store.record_command_effects_between(
                repository_root,
                &manifest,
                census,
                &after,
            );
        }
        census.changed_paths(&after)
    }

    /// Makes verification due for the files an approved command changed
    /// (spec 85). Only in Code mode, as for a patch (spec 23 §5.1), and only
    /// when the census saw a change: a command that changed nothing, or one
    /// whose effects could not be read, leaves verification as it was.
    pub(super) fn record_command_verification_due(
        &self,
        session_id: &str,
        task_id: &str,
        changed: Option<Vec<String>>,
    ) -> Result<()> {
        let Some(paths) = changed.filter(|paths| !paths.is_empty()) else {
            return Ok(());
        };
        if self.session_store.session_mode(session_id) != SessionMode::Code {
            return Ok(());
        }
        self.session_store.append_verification_event(
            session_id,
            &VerificationEvent::Due {
                task_id: task_id.to_string(),
                patch_id: String::new(),
                paths,
                checks: Vec::new(),
            },
        )
    }

    /// Records what the turn left on disk, so a later rewind can tell the
    /// agent's own changes from somebody else's. Best-effort for the same
    /// reason as [`Self::take_turn_checkpoint`].
    pub(super) fn seal_turn_checkpoint(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
    ) {
        let Ok(Some(manifest)) = self
            .checkpoint_store
            .read_checkpoint_for_task(&session.repository_id, &task.id)
        else {
            return;
        };
        let _ = self
            .checkpoint_store
            .seal_checkpoint(repository_root, &manifest);
    }
}
