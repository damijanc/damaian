use super::*;

impl ChatOrchestrator {
    /// Turns the user's own "remember that…" into a proposal. The statement is
    /// the user's sentence, not a model paraphrase, and the store still
    /// screens and caps it. A refused request is audited by the store and does
    /// not fail the turn; it is also shown to the user, because a request that
    /// looks ignored is indistinguishable from one Damaian silently dropped
    /// (spec 29 §5.5).
    pub(super) fn propose_explicit_memory(
        &self,
        root: &Path,
        session_id: &str,
        task_id: Option<&str>,
        message_id: &str,
        prompt: &str,
    ) -> Result<()> {
        let request = match crate::memory::explicit_memory_request_outcome(prompt) {
            crate::memory::ExplicitMemoryRequestOutcome::Absent => return Ok(()),
            crate::memory::ExplicitMemoryRequestOutcome::Request(request) => request,
            crate::memory::ExplicitMemoryRequestOutcome::Refused(refusal) => {
                // The parser drops an over-long statement before a proposal
                // exists, so this is its only audit record.
                self.audit_log.record(
                    "memory_proposal_failed",
                    &[
                        ("sessionId", session_id.to_string()),
                        ("reason", refusal.reason_code().to_string()),
                    ],
                )?;
                return self.announce_memory_refusal(session_id, task_id, &refusal);
            }
        };
        let proposal = MemoryProposal::new(
            request.scope,
            request.category,
            request.statement,
            ProposalOrigin::UserRequest {
                session_id: session_id.to_string(),
                message_id: message_id.to_string(),
            },
            Vec::new(),
            None,
        );
        match self.memory_store.propose_reporting_refusal(root, proposal) {
            Ok(crate::memory::ProposalOutcome::Proposed(_)) => Ok(()),
            Ok(crate::memory::ProposalOutcome::Refused(refusal)) => {
                self.announce_memory_refusal(session_id, task_id, &refusal)
            }
            // An explicit request is never capped, so this is a suggestion-only
            // path and should not be reachable from here.
            Ok(crate::memory::ProposalOutcome::SuggestionCapReached) => Ok(()),
            Err(error) => {
                // A failure here is the log refusing the write. Neither is a
                // reason to fail the user's turn.
                self.audit_log.record(
                    "memory_proposal_failed",
                    &[
                        ("sessionId", session_id.to_string()),
                        ("reason", error.to_string()),
                    ],
                )?;
                let message = format!("Damaian could not save that memory. {error}");
                self.session_store.append_message_with_kind(
                    session_id,
                    task_id,
                    "assistant",
                    &message,
                    Some("memory_refusal"),
                )?;
                Ok(())
            }
        }
    }

    /// Appends the store's plain refusal sentence to the conversation as an
    /// engine line, not as something the model said.
    fn announce_memory_refusal(
        &self,
        session_id: &str,
        task_id: Option<&str>,
        refusal: &crate::memory::MemoryRefusal,
    ) -> Result<()> {
        let message = crate::memory::MemoryError::Refused(refusal.clone()).to_string();
        self.session_store.append_message_with_kind(
            session_id,
            task_id,
            "assistant",
            &message,
            Some("memory_refusal"),
        )?;
        Ok(())
    }

    /// Recalls the entries that apply to this task (spec 30 §5.3).
    ///
    /// Runs once per turn, before assembly: the returned list is what the
    /// request carries and what a paused review keeps. A store that cannot be
    /// read fails the turn rather than silently sending without memory — the
    /// user is entitled to know that what they confirmed was not consulted.
    pub(super) fn recall_memory_for_turn(
        &self,
        repository_root: &Path,
        repository_id: &str,
        session_id: &str,
        prompt: &str,
        explicit_paths: &[String],
    ) -> Result<(ProjectIdentity, Vec<RecalledMemory>)> {
        let identity =
            ProjectIdentity::resolve(repository_root, repository_id.to_string(), &self.git)?;
        let recalled = self.memory_store.recall(
            repository_root,
            &identity,
            session_id,
            prompt,
            explicit_paths,
            self.config.memory_max_entries as usize,
        )?;
        Ok((identity, recalled))
    }

    /// The send-time half of spec 30 §5.6.
    ///
    /// An entry deleted, expired, superseded, or withdrawn between assembly and
    /// the model call must not be sent, and its `memory` manifest row must go
    /// with it or the inspector names a statement the request no longer
    /// carries (C14). Returns the entries the plan still sends, so the caller
    /// records a recall only for what actually went out.
    pub(super) fn recheck_memory_before_send(
        &self,
        repository_root: &Path,
        identity: &ProjectIdentity,
        session_id: &str,
        plan: &mut ContextPlan,
        recalled: Vec<RecalledMemory>,
    ) -> Result<Vec<RecalledMemory>> {
        // Only entries this plan actually carries: one the category budget
        // left out is not part of the request, and re-checking it would audit
        // an expiry for something never sent.
        let mut sent: Vec<RecalledMemory> = recalled
            .into_iter()
            .filter(|recalled| {
                plan.manifest
                    .memory
                    .iter()
                    .any(|reference| reference.entry_id == recalled.entry.id)
            })
            .collect();
        if sent.is_empty() {
            return Ok(sent);
        }
        let exclusions =
            self.memory_store
                .recheck_recall(repository_root, identity, session_id, &sent)?;
        if exclusions.is_empty() {
            return Ok(sent);
        }
        let mut prepared = PreparedContext::from_plan(plan);
        for exclusion in &exclusions {
            prepared.remove_memory(&exclusion.entry_id, exclusion.detail())?;
        }
        *plan = prepared.into_plan();
        sent.retain(|recalled| {
            !exclusions
                .iter()
                .any(|exclusion| exclusion.entry_id == recalled.entry.id)
        });
        Ok(sent)
    }

    /// Records one recall per entry the request carries, once, at send
    /// (spec 30 C9). The manifest names what was included after the budget,
    /// user removals and the send-time re-check, so it is the truth to record
    /// from rather than the list recall returned.
    pub(super) fn record_memory_recall(
        &self,
        manifest: &ContextManifest,
        recalled: &[RecalledMemory],
    ) -> Result<()> {
        let entries: Vec<MemoryEntry> = manifest
            .memory
            .iter()
            .filter_map(|reference| {
                recalled
                    .iter()
                    .find(|recalled| recalled.entry.id == reference.entry_id)
            })
            .map(|recalled| recalled.entry.clone())
            .collect();
        self.memory_store.record_recall(&entries)?;
        Ok(())
    }
}
