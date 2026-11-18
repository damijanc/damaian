use super::*;

impl ChatOrchestrator {
    /// Applies the narrow protected-path grammar to a prompt the user typed.
    ///
    /// Only the original direct-user task boundary calls this. A continuation's
    /// prompt is constructed by Damaian, so it inherits through task ancestry
    /// and is never re-parsed. A refusal is shown as an engine line and writes
    /// no protection event.
    pub(super) fn record_direct_prompt_protected_paths(
        &self,
        repository_root: &Path,
        session_id: &str,
        task_id: &str,
        prompt: &str,
    ) -> Result<()> {
        let result = record_task_protected_paths_from_prompt(
            &self.session_store,
            &self.path_policy,
            repository_root,
            session_id,
            task_id,
            prompt,
        )?;
        match result {
            crate::protected_paths::ProtectedPathPromptResult::NoRequest => Ok(()),
            crate::protected_paths::ProtectedPathPromptResult::Refused(error) => self
                .announce_protected_path_refusal(
                    session_id,
                    task_id,
                    &format!("Protected paths were not recorded: {error}"),
                ),
            crate::protected_paths::ProtectedPathPromptResult::Recorded(update) => {
                for refusal in &update.refused {
                    self.announce_protected_path_refusal(
                        session_id,
                        task_id,
                        &format!("Protected path was not recorded: {}", refusal.message()),
                    )?;
                }
                Ok(())
            }
        }
    }

    /// Copies a worker's spawn-time snapshot onto its first task.
    ///
    /// The parent and its worker are separate sessions, so `plan_resumed`
    /// ancestry cannot carry protected paths across the handoff. The snapshot
    /// was resolved from the parent task's durable record at spawn and stored
    /// in the worker's own `WorkerRecord`; this writes one
    /// `inherited_worker_handoff` event on the first task, before its first
    /// model request. It never inspects the prompt, which may be model text.
    ///
    /// Only the first task is seeded. A later user-started task in the same
    /// worker session starts empty, exactly as a new task in any other session
    /// does. The source check makes a restart idempotent: a task that already
    /// has its handoff event gets no second one.
    pub(super) fn seed_worker_inherited_protected_paths(
        &self,
        session_id: &str,
        task: &Task,
    ) -> Result<()> {
        let Some(record) = self.session_store.worker_record(session_id)? else {
            return Ok(());
        };
        if record.inherited_protected_paths.is_empty() {
            return Ok(());
        }
        let is_first_task = self
            .session_store
            .read_tasks(session_id)?
            .first()
            .is_some_and(|first| first.id == task.id);
        if !is_first_task {
            return Ok(());
        }
        let already_seeded = self
            .session_store
            .read_task_protected_path_events(session_id)?
            .iter()
            .any(|event| {
                event.task_id == task.id
                    && event.source == ProtectedPathSource::InheritedWorkerHandoff
            });
        if already_seeded {
            return Ok(());
        }
        self.session_store.record_task_protected_paths(
            session_id,
            &task.id,
            ProtectedPathAction::Add,
            &record.inherited_protected_paths,
            ProtectedPathSource::InheritedWorkerHandoff,
        )?;
        Ok(())
    }

    fn announce_protected_path_refusal(
        &self,
        session_id: &str,
        task_id: &str,
        message: &str,
    ) -> Result<()> {
        self.session_store.append_message_with_kind(
            session_id,
            Some(task_id),
            "assistant",
            message,
            Some("protected_path_refusal"),
        )?;
        self.audit_log.record(
            "task_protected_path_refusal",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("taskId", task_id.to_string()),
                ("message", message.to_string()),
            ],
        )?;
        Ok(())
    }

    /// Resolves the task's durable exclusions and refuses a whole proposal if
    /// any file it would change is covered.
    pub(super) fn ensure_patch_not_protected(
        &self,
        session_id: &str,
        task_id: &str,
        patch: &ProposedPatch,
    ) -> Result<()> {
        let active = self
            .session_store
            .task_protected_paths(session_id, task_id)?;
        ensure_patch_paths_allowed(&active, patch.files.iter().map(|file| file.path.as_str()))
    }
}

/// Appends or replaces the task's protected-path reminder in the system
/// message. The reminder is derived from the durable resolved set every turn;
/// it never re-parses a prompt.
///
/// The reminder reaches the model, so it passes the secret scanner like every
/// other inserted model-facing text. A repository can have a credential-shaped
/// file name, and the path is still a detected credential once it leaves the
/// machine.
pub(crate) fn apply_protected_paths_reminder(
    messages: &mut [ModelMessage],
    active: &TaskProtectedPaths,
    scanner: &SecretScanner,
) {
    let Some(system) = messages.iter_mut().find(|message| message.role == "system") else {
        return;
    };
    if let Some(start) = system.content.find(PROTECTED_PATHS_REMINDER_MARKER) {
        system.content.truncate(start);
    }
    if let Some(reminder) = protected_paths_reminder(active) {
        let reminder = scanner.redact(&reminder).text;
        system.content.push_str(PROTECTED_PATHS_REMINDER_MARKER);
        system.content.push('\n');
        system.content.push_str(&reminder);
    }
}
