use super::*;

/// What a paused turn is waiting for. Every resume matches on this
/// exhaustively and refuses the kinds it does not own (spec 50 `context.md`
/// §2), because the pending-turn file is keyed only by an id and each resume
/// loads whatever file its id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PausedFor {
    /// A command, patch or browser diagnostic that needs approval.
    Action,
    PlanReview,
    ProviderFallback,
    RoundBudget,
    /// A batch of verification checks waiting for one card's decisions
    /// (spec 23).
    CheckBatch,
    /// A failed check's baseline waiting for one card's decisions before the
    /// verification can classify the failure (spec 109 D1).
    FailureBaseline,
    Question,
    /// A host approval before a `fetch_url` request (spec 51 §5.3). Nothing
    /// was requested, and the only route that may continue it is
    /// `resume_after_fetch_decision`.
    Fetch,
    /// A pre-send context review (spec 27 §5.1). Nothing was sent, and the
    /// only route that may continue it is
    /// `resume_after_context_decision`.
    ContextReview,
    /// A file carrying more than one marker. No writer produces this, and
    /// guessing which pause it means is exactly the mistake this type exists
    /// to prevent, so every resume refuses it.
    Conflicting,
}

impl PausedFor {
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Action => "a command or patch approval",
            Self::PlanReview => "a plan review",
            Self::ProviderFallback => "a provider fallback offer",
            Self::RoundBudget => "a round-budget pause",
            Self::CheckBatch => "a check approval",
            Self::FailureBaseline => "a failed check's baseline approval",
            Self::Question => "a question",
            Self::Fetch => "a fetch approval",
            Self::ContextReview => "a context review",
            Self::Conflicting => "more than one kind of pause",
        }
    }

    pub(super) fn resume_method(self) -> &'static str {
        match self {
            Self::Action => "resume_after_command_decision",
            Self::PlanReview => "resume_after_plan_decision",
            Self::ProviderFallback => "resume_after_fallback_decision",
            Self::RoundBudget => "resume_round_budget",
            Self::CheckBatch => "resume_after_check_decision",
            Self::FailureBaseline => "resume_after_check_decision",
            Self::Question => "resume_after_answer",
            Self::Fetch => "resume_after_fetch_decision",
            Self::ContextReview => "resume_after_context_decision",
            Self::Conflicting => "",
        }
    }
}

/// What a question pause is waiting for. The question's text and options live
/// in the session log; this carries only the id that addresses them, so the
/// pause cannot drift from what was asked (spec 50 `context.md` §2, §6).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingQuestion {
    pub(super) question_id: String,
    /// Whether this is an ordinary model question or the engine's own
    /// uncertain-repair question (spec 102 D2). The answer route reads it to
    /// decide between continuing the model turn and running one repair turn.
    /// Defaulted so a pending question written before this field still loads.
    #[serde(default)]
    pub(super) kind: PendingQuestionKind,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PendingQuestionKind {
    #[default]
    Agent,
    RepairDecision,
}

/// What a fetch pause is waiting for (spec 51 §5.3).
///
/// The URL is stored verbatim because the resumed request is the one the user
/// approved: re-deriving it from parsed parts could change it, and a dialog
/// that showed a different string than the request carries would be asking a
/// different question. The host and approval key are kept alongside so neither
/// the decision route nor the audit has to parse it again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingFetchCall {
    pub host: String,
    pub url: String,
    pub approval_key: String,
    #[serde(default)]
    pub long_components: Vec<FetchComponentFlag>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub fragment: Option<String>,
}

/// The kind of pause a saved turn represents. Every writer sets at most one
/// marker; the tuple match makes a new field on [`PendingChatTurn`] a compile
/// error here, which is where each resume's decision starts.
pub(super) fn paused_for(pending: &PendingChatTurn) -> PausedFor {
    match (
        pending.plan_review.is_some(),
        pending.provider_fallback.is_some(),
        pending.round_budget.is_some(),
        pending.question.is_some(),
        pending.check_batch.is_some(),
        pending.failure_baseline.is_some(),
        pending.fetch_call.is_some(),
        pending.context_review.is_some(),
    ) {
        (false, false, false, false, false, false, false, false) => PausedFor::Action,
        (true, false, false, false, false, false, false, false) => PausedFor::PlanReview,
        (false, true, false, false, false, false, false, false) => PausedFor::ProviderFallback,
        (false, false, true, false, false, false, false, false) => PausedFor::RoundBudget,
        (false, false, false, true, false, false, false, false) => PausedFor::Question,
        (false, false, false, false, true, false, false, false) => PausedFor::CheckBatch,
        (false, false, false, false, false, true, false, false) => PausedFor::FailureBaseline,
        (false, false, false, false, false, false, true, false) => PausedFor::Fetch,
        (false, false, false, false, false, false, false, true) => PausedFor::ContextReview,
        _ => PausedFor::Conflicting,
    }
}

/// One dispatched call in a turn, as the elision notice and the pre-cut
/// warning describe it (spec 65 §5.3).
///
/// `message_index` is the index of the round's assistant message in `messages`.
/// That array is append-only within a turn, so the index stays stable and lets
/// `bounded_messages` decide which entries precede the cut without re-deriving
/// round boundaries. `key` is the canonical identity from [`ChatOrchestrator::call_key`]
/// (for repeat detection on resume) and `line` its rendered, already-redacted
/// display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RecordedCall {
    pub(super) round: u32,
    pub(super) message_index: usize,
    pub(super) line: String,
    #[serde(default)]
    pub(super) key: Option<Vec<String>>,
    #[serde(default)]
    pub(super) writes: bool,
    /// The read result this call produced was replaced by a within-turn
    /// manifest (spec 103 Task 4). The model may repeat the call through the
    /// ordinary tool path, so the resume-time seen-call set ignores it.
    #[serde(default)]
    pub(super) compacted: bool,
}

/// The notes and call list a turn carries across a pause (spec 65 §5.6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct TurnNotesState {
    #[serde(default)]
    pub(super) text: String,
    #[serde(default)]
    pub(super) carried: bool,
    #[serde(default)]
    pub(super) calls: Vec<RecordedCall>,
}

impl TurnNotesState {
    pub(super) fn from_pending(pending: &PendingChatTurn) -> Self {
        TurnNotesState {
            text: pending.turn_notes.clone(),
            carried: pending.turn_notes_carried,
            calls: pending.calls.clone(),
        }
    }

    pub(super) fn view(&self) -> Option<TurnNotesView> {
        (!self.text.is_empty()).then(|| TurnNotesView {
            text: self.text.clone(),
            carried: self.carried,
        })
    }
}

/// Proves a paused turn's segment record still describes the durable segment.
///
/// The paused turn may be resumed one round on (a command or patch decision
/// re-enters the round that was deferred) or on the same round (a question or
/// fetch decision), so its counter may differ from the durable baseline by at
/// most one. Everything else must match byte-for-byte; a mismatch is corrupt
/// state, not a later segment.
pub(super) fn validate_segment_resume(
    resume: &TaskSegmentPauseState,
    started: &TaskSegmentStarted,
    action_rounds: u32,
    exploration_rounds: u32,
) -> Result<()> {
    let totals_match = resume.total_action_rounds
        == started
            .total_action_rounds
            .saturating_add(resume.segment_action_rounds)
        && resume.total_exploration_rounds
            == started
                .total_exploration_rounds
                .saturating_add(resume.segment_exploration_rounds);
    let counters_match = (resume.segment_action_rounds == action_rounds
        || resume.segment_action_rounds.saturating_add(1) == action_rounds)
        && resume.segment_exploration_rounds == exploration_rounds;
    if resume.segment == started.segment
        && resume.start_digest == started.start_digest
        && totals_match
        && counters_match
    {
        return Ok(());
    }
    Err(ClientError::InvalidInput(format!(
        "the paused task segment {} does not match durable segment {}",
        resume.segment, started.segment
    )))
}

/// An ordinary Code-mode turn's continuation state (spec 107 D3, option A).
///
/// The worker's [`SegmentRunState`] without its durable records: an ordinary
/// session has no approved budget to account against across processes, so a
/// decline returns to the existing round-budget card and the card's pause
/// carries the counters. The totals therefore cover the rounds since the turn
/// started or last resumed — every resume is a decision the user took. The
/// budget is the effective worker default (`worker_task_budget_*`), which a
/// repository or profile can only lower.
pub(super) struct OrdinaryContinuation {
    pub(super) budget: TaskExecutionBudget,
    pub(super) prior_action_rounds: u32,
    pub(super) prior_exploration_rounds: u32,
    pub(super) start_digest: String,
    /// The next checkpoint's round allowance once one was crossed: the
    /// remaining budget capped at an ordinary segment. `None` before the first
    /// crossing, so a turn that never reaches it is unchanged.
    pub(super) action_allowance: Option<u32>,
}

impl OrdinaryContinuation {
    pub(super) fn new(budget: TaskExecutionBudget, start_digest: String) -> Self {
        Self {
            budget,
            prior_action_rounds: 0,
            prior_exploration_rounds: 0,
            start_digest,
            action_allowance: None,
        }
    }

    pub(super) fn totals(&self, action_rounds: u32, exploration_rounds: u32) -> SegmentTotals {
        SegmentTotals {
            action_rounds: self.prior_action_rounds.saturating_add(action_rounds),
            exploration_rounds: self
                .prior_exploration_rounds
                .saturating_add(exploration_rounds),
        }
    }

    pub(super) fn advance(&mut self, totals: SegmentTotals, digest: String, action_allowance: u32) {
        self.prior_action_rounds = totals.action_rounds;
        self.prior_exploration_rounds = totals.exploration_rounds;
        self.start_digest = digest;
        self.action_allowance = Some(action_allowance);
    }
}

/// One automatic worker segment's in-memory state (spec 103 Task 5).
///
/// The current segment's counters stay in the turn loop (`action_rounds`,
/// `exploration_rounds`); this carries the monotonic task totals and the
/// boundary decisions. Totals never reset at a boundary; only the current
/// segment's counters do.
pub(super) struct SegmentRunState {
    pub(super) segment: u32,
    pub(super) prior_action_rounds: u32,
    pub(super) prior_exploration_rounds: u32,
    pub(super) start_digest: String,
    pub(super) action_allowance: u32,
    pub(super) exploration_allowance: u32,
    pub(super) budget: TaskExecutionBudget,
    /// Whether this is the bounded pre-plan phase (spec 103 R17) rather than an
    /// approved task budget. The segment machinery is the same; only what
    /// exhaustion means differs.
    pub(super) preplan: bool,
}

impl SegmentRunState {
    pub(super) fn resumed(
        budget: &TaskExecutionBudget,
        started: &TaskSegmentStarted,
        normal_action_rounds: u32,
        normal_exploration_rounds: u32,
        preplan: bool,
    ) -> Self {
        // Task totals are bounded by the approved budget and Task 1's absolute
        // task ceilings. The legacy one-turn caps bound one ordinary turn and
        // must not shrink a multi-segment task total.
        let action_cap = budget
            .action_rounds
            .min(crate::task_budget::ABSOLUTE_TASK_ACTION_ROUND_CEILING);
        let exploration_cap = budget
            .exploration_rounds
            .min(crate::task_budget::ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING);
        Self {
            segment: started.segment,
            prior_action_rounds: started.total_action_rounds,
            prior_exploration_rounds: started.total_exploration_rounds,
            start_digest: started.start_digest.clone(),
            action_allowance: normal_action_rounds
                .min(action_cap.saturating_sub(started.total_action_rounds)),
            exploration_allowance: normal_exploration_rounds
                .min(exploration_cap.saturating_sub(started.total_exploration_rounds)),
            budget: *budget,
            preplan,
        }
    }

    pub(super) fn total_action_rounds(&self, current: u32) -> u32 {
        self.prior_action_rounds.saturating_add(current)
    }

    pub(super) fn total_exploration_rounds(&self, current: u32) -> u32 {
        self.prior_exploration_rounds.saturating_add(current)
    }

    pub(super) fn pause_state(
        &self,
        action_rounds: u32,
        exploration_rounds: u32,
    ) -> TaskSegmentPauseState {
        TaskSegmentPauseState {
            segment: self.segment,
            segment_action_rounds: action_rounds,
            segment_exploration_rounds: exploration_rounds,
            total_action_rounds: self.total_action_rounds(action_rounds),
            total_exploration_rounds: self.total_exploration_rounds(exploration_rounds),
            start_digest: self.start_digest.clone(),
        }
    }

    /// The live state of this open segment, for a terminal outcome.
    ///
    /// Unlike the durable `segment_started` payload, this carries the counters
    /// the segment reached, the monotonic task totals and the digest of the
    /// engine state at the stop.
    pub(super) fn snapshot(
        &self,
        action_rounds: u32,
        exploration_rounds: u32,
        tokens: SegmentTokenTotals,
        progress_digest: String,
    ) -> TaskSegmentSnapshot {
        TaskSegmentSnapshot {
            segment: self.segment,
            segment_action_rounds: action_rounds,
            segment_exploration_rounds: exploration_rounds,
            total_action_rounds: self.total_action_rounds(action_rounds),
            total_exploration_rounds: self.total_exploration_rounds(exploration_rounds),
            tokens,
            start_digest: self.start_digest.clone(),
            progress_digest: Some(progress_digest),
        }
    }
}

/// One completed native tool call, with the typed engine facts Task 4 needs to
/// compact it without reading model-visible prose (spec 103 Task 4).
///
/// `call` is Task 3's reducer metadata. `plan_step_id` is the step that was
/// open when the call ran, captured from the plan at dispatch time. `read_key`
/// is [`ChatOrchestrator::call_key`]'s canonical identity, so a compacted read
/// can be removed from the seen-call set by key rather than by text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrackedCompletedToolCall {
    #[serde(flatten)]
    pub(super) call: CompletedToolCall,
    #[serde(default)]
    pub(super) plan_step_id: Option<String>,
    #[serde(default)]
    pub(super) read_key: Option<Vec<String>>,
}

/// The calls that completed in one assistant round. Batched calls stay together
/// so a provider-valid request keeps its batched pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrackedCompletedToolRound {
    pub(super) calls: Vec<TrackedCompletedToolCall>,
}

/// The durable completed-tool tracker a paused turn carries (spec 103 Task 4).
///
/// Historical pending turns have no such field and load with an empty tracker,
/// which is exactly the pre-Task-4 request path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CompletedToolTracker {
    #[serde(default)]
    pub(super) rounds: Vec<TrackedCompletedToolRound>,
}

impl From<Vec<TrackedCompletedToolRound>> for CompletedToolTracker {
    fn from(rounds: Vec<TrackedCompletedToolRound>) -> Self {
        Self { rounds }
    }
}

impl CompletedToolTracker {
    pub(super) fn push_round(&mut self, calls: Vec<TrackedCompletedToolCall>) {
        if !calls.is_empty() {
            self.rounds.push(TrackedCompletedToolRound { calls });
        }
    }

    pub(super) fn as_compaction_rounds(&self) -> Vec<CompletedToolRound> {
        self.rounds
            .iter()
            .map(|round| CompletedToolRound {
                calls: round.calls.iter().map(|call| call.call.clone()).collect(),
            })
            .collect()
    }

    pub(super) fn calls(&self) -> impl Iterator<Item = &TrackedCompletedToolCall> {
        self.rounds.iter().flat_map(|round| round.calls.iter())
    }

    /// The newest completed call that ran while this plan step was open.
    pub(super) fn latest_call_id_for_step(&self, step_id: &str) -> Option<&str> {
        self.rounds
            .iter()
            .rev()
            .find_map(|round| {
                round
                    .calls
                    .iter()
                    .rev()
                    .find(|call| call.plan_step_id.as_deref() == Some(step_id))
            })
            .map(|call| call.call.call_id.as_str())
    }

    /// Every call id the tracker can describe.
    pub(super) fn completed_call_ids(&self) -> BTreeSet<&str> {
        self.calls()
            .map(|call| call.call.call_id.as_str())
            .collect()
    }

    /// The canonical keys of reads whose bodies a preparation attempt replaced.
    pub(super) fn compacted_read_keys(&self, call_ids: &BTreeSet<String>) -> Vec<Vec<String>> {
        self.calls()
            .filter(|call| call_ids.contains(&call.call.call_id))
            .filter_map(|call| call.read_key.clone())
            .collect()
    }
}

/// The typed compaction identity of a dispatched action.
///
/// Every value comes from the action the engine decoded and dispatched, never
/// from the result text. `read_only` says whether the action's effect makes its
/// result a candidate for compaction at all.
fn compaction_identity(
    action: &ToolAction,
) -> Option<(CompactedToolKind, ToolIdentity, Option<LineRange>, bool)> {
    match action {
        ToolAction::ReadFile { path, range, .. } => Some((
            CompactedToolKind::ReadFile,
            ToolIdentity::path(path.clone()),
            *range,
            true,
        )),
        ToolAction::ListDirectory { dir, .. } => Some((
            CompactedToolKind::ListDirectory,
            ToolIdentity::path(dir.clone().unwrap_or_else(|| ".".to_string())),
            None,
            true,
        )),
        ToolAction::SearchContent { pattern, .. } => Some((
            CompactedToolKind::SearchContent,
            ToolIdentity::query(pattern.clone()),
            None,
            true,
        )),
        ToolAction::SearchCodebase { query, .. } => Some((
            CompactedToolKind::SearchCodebase,
            ToolIdentity::query(query.clone()),
            None,
            true,
        )),
        ToolAction::ReadGitStatus { .. } => Some((
            CompactedToolKind::ReadGitStatus,
            ToolIdentity::command("git status"),
            None,
            true,
        )),
        ToolAction::ReadGitDiff {
            staged,
            paths,
            stat,
            base,
            ..
        } => {
            let mut command = String::from("git diff");
            if *stat {
                command.push_str(" --stat");
            }
            if *staged {
                command.push_str(" --staged");
            }
            if let Some(base) = base {
                command.push(' ');
                command.push_str(base);
            }
            if !paths.is_empty() {
                command.push_str(" --");
                for path in paths {
                    command.push(' ');
                    command.push_str(path);
                }
            }
            Some((
                CompactedToolKind::ReadGitDiff,
                ToolIdentity::command(command),
                None,
                true,
            ))
        }
        ToolAction::ListWorktrees { .. } => Some((
            CompactedToolKind::ListWorktrees,
            ToolIdentity::command("git worktree list"),
            None,
            true,
        )),
        ToolAction::ReadGitLog {
            base, max_count, ..
        } => {
            let mut command = String::from("git log");
            if let Some(base) = base {
                command.push(' ');
                command.push_str(base);
            }
            if let Some(max_count) = max_count {
                command.push_str(&format!(" --max-count={max_count}"));
            }
            Some((
                CompactedToolKind::ReadGitLog,
                ToolIdentity::command(command),
                None,
                true,
            ))
        }
        ToolAction::Command(request) => Some((
            CompactedToolKind::Command,
            ToolIdentity::command(request.command.clone()),
            None,
            false,
        )),
        ToolAction::WebDiagnostic(call) => Some((
            CompactedToolKind::WebDiagnostic,
            ToolIdentity::url(call.url.clone()),
            None,
            false,
        )),
        ToolAction::McpCall {
            server_id,
            tool_name,
            ..
        } => Some((
            CompactedToolKind::McpCall,
            ToolIdentity::endpoint(format!("{server_id}/{tool_name}")),
            None,
            false,
        )),
        ToolAction::FetchUrl { url } => Some((
            CompactedToolKind::FetchUrl,
            ToolIdentity::url(url.clone()),
            None,
            false,
        )),
        ToolAction::ProposePatch(edit) => Some((
            CompactedToolKind::Command,
            ToolIdentity::path(
                edit.changes
                    .first()
                    .map(|change| change.path.clone())
                    .unwrap_or_default(),
            ),
            None,
            false,
        )),
        ToolAction::EditFile { edits, .. } => Some((
            CompactedToolKind::Command,
            ToolIdentity::path(
                edits
                    .first()
                    .map(|edit| edit.path.clone())
                    .unwrap_or_default(),
            ),
            None,
            false,
        )),
        ToolAction::ProposePlan(_)
        | ToolAction::CompleteStep
        | ToolAction::AskUser(_)
        | ToolAction::TurnNotes { .. }
        | ToolAction::ProposeMemory { .. }
        | ToolAction::McpToolSearch { .. } => Some((
            CompactedToolKind::Command,
            ToolIdentity::endpoint(tool_action_marker(action).0.to_string()),
            None,
            false,
        )),
    }
}

/// Turns one completed native dispatch into the tracker entry Task 4 persists.
///
/// A cancelled dispatch did nothing, so it is not a completed round. A call
/// without a native id (the text-envelope command) has no provider pairing to
/// preserve and is not tracked either.
pub(super) fn tracked_completed_call(
    call: &ToolCall,
    action: &ToolAction,
    outcome: &ActionOutcome,
    plan_step_id: Option<String>,
    read_key: Option<Vec<String>>,
) -> Option<TrackedCompletedToolCall> {
    if *outcome == ActionOutcome::Cancelled {
        return None;
    }
    let (tool_kind, identity, range, read_only) = compaction_identity(action)?;
    let result_outcome = match outcome {
        ActionOutcome::Cancelled => ToolResultOutcome::UnknownExternalOutcome,
        ActionOutcome::Failed => ToolResultOutcome::FailedUnresolved,
        ActionOutcome::CommandExit(Some(0), _) => ToolResultOutcome::Succeeded,
        ActionOutcome::CommandExit(_, _) => ToolResultOutcome::FailedUnresolved,
        ActionOutcome::FileRead { .. } => ToolResultOutcome::Succeeded,
        ActionOutcome::Ok => ToolResultOutcome::Succeeded,
    };
    Some(TrackedCompletedToolCall {
        call: CompletedToolCall {
            call_id: call.id.clone(),
            tool_kind,
            identity,
            range,
            outcome: result_outcome,
            include_excerpt: read_only,
        },
        plan_step_id,
        read_key,
    })
}

/// Conversation state saved when a chat turn pauses on a command that needs
/// human approval, so [`ChatOrchestrator::resume_after_command_decision`]
/// can pick the turn back up once the user decides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PendingChatTurn {
    pub(super) proposal_id: String,
    pub(super) session: Session,
    pub(super) task: Task,
    pub(super) repository_root: String,
    pub(super) context_files: Vec<String>,
    /// The action rounds the turn has used. Every pause and resume carries
    /// this, so the action budget continues where it stopped (spec 61 §5.2).
    pub(super) round: u32,
    /// The exploration (read-only) rounds the turn has used. A pause resumes
    /// the same turn, so this is carried with it rather than restarted: an
    /// approval or *Continue* must not refresh the allowance (spec 59 §5.2).
    /// `#[serde(default)]` keeps pending turns written before this field
    /// existed loadable, with no exploration rounds spent.
    #[serde(default)]
    pub(super) exploration_rounds: u32,
    /// The turn's notes at the pause (spec 65 §5.6), already redacted. A
    /// pending turn written before this field existed loads with no notes and
    /// resumes as it always did.
    #[serde(default)]
    pub(super) turn_notes: String,
    /// Whether the notes still hold text carried from an earlier turn. A
    /// replace clears it; an append keeps it.
    #[serde(default)]
    pub(super) turn_notes_carried: bool,
    /// Every dispatched call this turn, oldest first. Persisted so the
    /// elision notice survives a pause and `seen_read_calls` can be rebuilt
    /// after a resume (requirement 8). `#[serde(default)]` keeps older
    /// pending turns loadable with an empty list.
    #[serde(default)]
    pub(super) calls: Vec<RecordedCall>,
    /// Completed native tool rounds, with the typed metadata Task 4 needs to
    /// compact a later request deterministically (spec 103 Task 4).
    /// `#[serde(default)]` keeps pending turns written before this tracker
    /// existed loadable, with the pre-Task-4 request path.
    #[serde(default)]
    pub(super) completed_tool_rounds: Vec<TrackedCompletedToolRound>,
    /// Evidence accrued for the step that was open when the turn paused. A
    /// pause carries it so a step closed after the resume is judged on
    /// everything it ran (spec 66 §5.3), without attaching anything to the step
    /// early — attaching is the true turn end's job. `#[serde(default)]` keeps
    /// pending turns written before this field existed loadable; the worst case
    /// is a resumed step judged on less evidence, which is today's behaviour.
    #[serde(default)]
    pub(super) step_evidence: Vec<crate::plan::Evidence>,
    /// Whether this turn has already sent its one plan reminder (spec 66
    /// §5.2). Carried so an approval, plan-review, question, *Continue* or
    /// crash resume cannot remind a second time. `#[serde(default)]` keeps
    /// older pending turns loadable; they resume as `false`, which is today's
    /// behaviour plus at most one reminder.
    #[serde(default)]
    pub(super) plan_reminder_sent: bool,
    pub(super) messages: Vec<ModelMessage>,
    pub(super) matched_tool_call: Option<ToolCall>,
    pub(super) last_content: String,
    #[serde(default)]
    pub(super) turn_options: ChatTurnOptions,
    /// Thinking-mode reasoning behind `matched_tool_call`, which must be
    /// replayed with it when the turn resumes — a pause for human approval
    /// must not lose it, or the resumed request is rejected outright.
    /// `#[serde(default)]` keeps pending turns written before this field
    /// existed loadable.
    #[serde(default)]
    pub(super) reasoning_content: Option<String>,
    /// True when earlier calls of the paused round already ran, so `messages`
    /// ends with the round's assistant message and their results. The resumed
    /// call joins that message rather than opening a second one for the same
    /// response. `#[serde(default)]` keeps pending turns written before this
    /// field existed loadable; they resume in the per-call shape they were
    /// saved in.
    #[serde(default)]
    pub(super) round_calls_fed_back: bool,
    /// Present when the paused action is an MCP tool call rather than a shell
    /// command; carries everything needed to execute it on resume. `#[serde(default)]`
    /// keeps older on-disk pending turns (which predate MCP) loadable.
    #[serde(default)]
    pub(super) mcp_call: Option<PendingMcpCall>,
    #[serde(default)]
    pub(super) web_diagnostic_call: Option<PendingWebDiagnosticCall>,
    /// Present when the pause is a plan review rather than an action approval
    /// (spec 21 §5.5). Nothing was dispatched, so there is no call to carry —
    /// only what the turn was about to do, which the review card shows and the
    /// resumed turn puts back in front of the model. `#[serde(default)]` keeps
    /// pending turns written before the gate existed loadable.
    #[serde(default)]
    pub(super) plan_review: Option<PendingPlanReview>,
    /// Present when the pause is a provider-fallback offer (spec 56). As with
    /// a plan review nothing was dispatched: `messages` is the request the
    /// provider refused, and `round` is the round it refused in, because that
    /// round never completed. `#[serde(default)]` keeps pending turns written
    /// before the offer existed loadable.
    #[serde(default)]
    pub(super) provider_fallback: Option<PendingProviderFallback>,
    /// Present when the pause is a round-budget stop (spec 61). As with a plan
    /// review nothing was dispatched: the turn hit its round limit and paused
    /// before the forced final call, so `messages` is the conversation so far
    /// and `round` the round that hit the limit. `#[serde(default)]` keeps
    /// pending turns written before the pause existed loadable.
    #[serde(default)]
    pub(super) round_budget: Option<RoundBudgetPause>,
    /// The automatic worker-segment state at this pause (spec 103 Task 5).
    ///
    /// Defaulted and omitted when absent, so an unbudgeted worker, an inline
    /// task and every historical pending turn keep the exact pending-turn shape
    /// they had before segments existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) segment: Option<TaskSegmentPauseState>,
    /// Present when the pause is a question the model asked (spec 50). As with
    /// a plan review nothing was dispatched: the turn stopped to let a human
    /// answer, and `round` is the round the question was asked in, because
    /// that round has not been charged yet. `#[serde(default)]` keeps pending
    /// turns written before the pause existed loadable.
    #[serde(default)]
    pub(super) question: Option<PendingQuestion>,
    /// Present when the pause is a verification check batch (spec 23). Carries
    /// the checks to decide and the attempt's file hashes, so the resumed turn
    /// finishes the report without re-deriving what the check saw.
    #[serde(default)]
    pub(super) check_batch: Option<PendingCheckBatch>,
    /// Present when a failed check's baseline needs its own approval before
    /// the verification can classify the failure (spec 109 D1). The card is
    /// the existing check batch; this marker carries the baseline batch and
    /// what the verification needs to finish once it is decided. Defaulted so
    /// a pending turn written before this pause existed still loads.
    #[serde(default)]
    pub(super) failure_baseline: Option<PendingFailureBaseline>,
    /// Present when the pause is a host approval before a fetch (spec 51
    /// §5.3). Nothing was requested: an unapproved host stops the turn before
    /// the request is built. `#[serde(default)]` keeps pending turns written
    /// before the pause existed loadable.
    #[serde(default)]
    pub(super) fetch_call: Option<PendingFetchCall>,
    /// Present when the pause is a pre-send context review (spec 27 §5.1).
    /// `#[serde(default)]` keeps pending turns written before the review
    /// existed loadable.
    #[serde(default)]
    pub(super) context_review: Option<PendingContextReview>,
}

/// The persisted half of a failure-baseline pause (spec 109 D1).
///
/// The verification stays paused between running its checks and classifying
/// their failures: this records the baseline batch whose proposal ids the
/// card's decisions address, and the attempt's inputs, so the resumed turn
/// finishes the same report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingFailureBaseline {
    pub(super) batch: BaselineBatch,
    pub(super) attempt: u32,
    pub(super) files_changed: Vec<String>,
    pub(super) file_hashes: BTreeMap<String, String>,
}

/// The persisted half of a check-batch pause (spec 23 §3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingCheckBatch {
    pub(super) checks: Vec<PendingCheckDecision>,
    /// The task whose plan owns this verification, and the patch that made it
    /// due.
    pub(super) due_task_id: String,
    pub(super) patch_id: String,
    pub(super) attempt: u32,
    pub(super) files_changed: Vec<String>,
    pub(super) file_hashes: BTreeMap<String, String>,
    /// Set when the card is for build checks (spec 109 D10): they are decided
    /// and run before the verification selects anything else, which it then
    /// does on resume. Absent from every pause written before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) builds: Option<PendingBuildStage>,
}

/// The build checks a build card's verification already has: run or reused
/// before the card, so the resumed verification does not run them again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingBuildStage {
    pub(super) done: Vec<BuildRecord>,
}

/// Where a verification's build stage left it (spec 109 D10).
pub(super) enum BuildStage {
    /// A build check needs approval; the turn is parked on its card.
    Paused(Box<ChatTurnResult>),
    /// Every build check ran, was reused, or could not run.
    Done(Vec<BuildRecord>),
}

/// One check waiting on the user's decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingCheckDecision {
    pub(super) proposal_id: String,
    /// The plan step this check owns, so the resumed turn can close it.
    pub(super) step_id: String,
    pub(super) working_directory: String,
    pub(super) command: String,
}

impl PendingCheckDecision {
    pub(super) fn check(&self) -> CheckId {
        CheckId::new(self.working_directory.clone(), self.command.clone())
    }
}

/// The browser half of a verification attempt (spec 23 proposal §5.4).
///
/// A web asset changed and the session has a diagnostic target: rerun it. With
/// no target, or when diagnostics are not approved for this session, the check
/// is still selected and reported as skipped, never inferred as verified.
pub(super) enum BrowserCheck {
    Ready {
        check: CheckId,
        call: WebDiagnosticCall,
    },
    Skipped {
        check: CheckId,
        reason: SkipReason,
    },
}

impl BrowserCheck {
    pub(super) fn check(&self) -> &CheckId {
        match self {
            Self::Ready { check, .. } | Self::Skipped { check, .. } => check,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingPlanReview {
    pub(super) deferred_action: String,
    /// The engine-proposed budget offer (spec 103 Task 2), persisted verbatim
    /// so a reload or worker restart reattaches the same values. Defaulted so
    /// every pending turn written before budgets existed loads budgetless and
    /// keeps today's approval behaviour. Skipped on the wire when absent, so
    /// an inline review keeps the exact pending-turn shape it had before task
    /// budgets existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) budget: Option<TaskBudgetReview>,
}

/// The persisted half of a pre-send context review (spec 27 §5.1).
///
/// The prepared plan is what the user may edit; the conversation block and the
/// summary are what the resumed turn needs to re-render the user message
/// without re-reading the transcript. `summary` and `summary_removed` are both
/// kept so the resume can either send the summary or state that the user
/// removed it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingContextReview {
    pub(super) prepared: PreparedContext,
    pub(super) conversation: PromptConversation,
    pub(super) summary: Option<ConversationSummary>,
    pub(super) summary_removed: bool,
    /// The entries the paused turn recalled, as assembly rendered them
    /// (spec 30 §5.6). A send is the same turn, so it re-checks these rather
    /// than recalling again: a new recall could rank differently and would put
    /// entries in the request the user never reviewed. `#[serde(default)]`
    /// keeps a review written before this field existed loadable; the memory
    /// category was empty in those reviews anyway.
    #[serde(default)]
    pub(super) recalled_memory: Vec<RecalledMemory>,
}

/// The persisted half of a round-budget pause (spec 61). `rounds_granted` is
/// the running total of extra rounds granted across continuations, so the
/// resumed turn can seed its `round_budget_extension` from it and keep its
/// limit where it stopped instead of resetting to the base segment.
///
/// Public because [`PausedTurns::round_budget_pause`] returns it, and crash
/// recovery reads its two fields to decide whether the pause can be
/// re-presented.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoundBudgetPause {
    /// The round that hit the limit.
    pub rounds_used: u32,
    /// The running total of granted rounds across continuations.
    pub rounds_granted: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingProviderFallback {
    pub(super) refused_provider: String,
    pub(super) refused_model: String,
    pub(super) fallback_provider: String,
    pub(super) fallback_model: String,
    /// [`ProviderRefusal::code`], kept as the string the decline records as
    /// `failureKind` — the refusal itself is not serialisable and its code is
    /// all the decline needs.
    pub(super) refusal_code: String,
    pub(super) message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingMcpCall {
    pub(super) server_id: String,
    pub(super) tool_name: String,
    pub(super) arguments_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingWebDiagnosticCall {
    pub(super) call: WebDiagnosticCall,
}

/// A read-only view of the turns [`ChatOrchestrator`] has parked on disk.
///
/// Crash recovery needs one fact out of a paused turn — what a plan review is
/// holding back — and needs it without an orchestrator, which takes thirteen
/// dependencies to build and belongs to a repository rather than to the
/// data directory recovery sweeps. Read-only on purpose: *resuming* a paused
/// turn stays the orchestrator's, and nothing here can take one.
///
/// It parses the file as loose JSON rather than as [`PendingChatTurn`], so a
/// pending turn written by a different version still answers this question
/// instead of failing to deserialise and looking like an absent one.
#[derive(Debug, Clone)]
pub struct PausedTurns {
    pub(super) data_dir: PathBuf,
}

impl PausedTurns {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    /// The session a paused turn belongs to, or `None` when the id names no
    /// paused turn (spec 110). The desktop shell reads it to know whose work a
    /// resume request is before it runs anything, so a session that is already
    /// working refuses the second request instead of running two at once.
    pub fn session_id(&self, proposal_id: &str) -> Option<String> {
        let path = PendingCommandStore::new(&self.data_dir).path_for(proposal_id);
        let content = fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        value
            .get("session")?
            .get("id")?
            .as_str()
            .map(str::to_string)
    }

    /// What a paused plan review is holding back, or `None` when the id names
    /// no paused turn, or one that is not a plan review.
    ///
    /// `None` is the signal that a plan review cannot be re-presented: the
    /// turn behind it is gone, so approving the plan would have nothing to
    /// continue.
    pub fn plan_review_deferred_action(&self, proposal_id: &str) -> Option<String> {
        let path = PendingCommandStore::new(&self.data_dir).path_for(proposal_id);
        let content = fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        value
            .get("plan_review")?
            .get("deferred_action")?
            .as_str()
            .map(str::to_string)
    }

    /// The engine-proposed budget a paused plan review offered, or `None` for
    /// an inline or historical review (spec 103 Task 2).
    ///
    /// Read back verbatim from the pending turn rather than rebuilt from
    /// configuration: a reload or worker restart must reattach the exact
    /// values the user is looking at, even if a config edit happened since.
    ///
    /// A missing pending turn or a review without a `budget` field is a
    /// budgetless review, not an error. A field that is present but does not
    /// decode, or decodes to a budget that fails its own bounds, is `Err`: a
    /// caller reattaching a review must not silently downgrade a corrupt
    /// allowance to "no allowance", because that would present a different
    /// decision from the one the log says was offered.
    pub fn plan_budget_review(&self, proposal_id: &str) -> Result<Option<TaskBudgetReview>> {
        let path = PendingCommandStore::new(&self.data_dir).path_for(proposal_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(None);
        };
        let value: serde_json::Value = serde_json::from_str(&content).map_err(|error| {
            ClientError::InvalidInput(format!(
                "the paused review {proposal_id} is not readable: {error}"
            ))
        })?;
        let Some(budget) = value
            .get("plan_review")
            .and_then(|review| review.get("budget"))
        else {
            return Ok(None);
        };
        if budget.is_null() {
            return Ok(None);
        }
        let review =
            serde_json::from_value::<TaskBudgetReview>(budget.clone()).map_err(|error| {
                ClientError::InvalidInput(format!(
                    "the paused review {proposal_id} has a corrupt task budget: {error}"
                ))
            })?;
        review.budget.validate()?;
        Ok(Some(review))
    }

    /// The provider a paused fallback offer would answer with, or `None` when
    /// the id names no paused turn, or one that is not a fallback offer
    /// (spec 56). The shell reads it to know whose engine and key to build
    /// before resuming; recovery reads it to know whether the offer can still
    /// be made.
    pub fn provider_fallback_target(&self, proposal_id: &str) -> Option<String> {
        let path = PendingCommandStore::new(&self.data_dir).path_for(proposal_id);
        let content = fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        value
            .get("provider_fallback")?
            .get("fallback_provider")?
            .as_str()
            .map(str::to_string)
    }

    /// The rounds a paused round-budget turn had used and been granted, or
    /// `None` when the id names no paused turn, or one that is not a
    /// round-budget pause (spec 61). Recovery reads it to know whether the
    /// pause can still be re-presented; a missing or unreadable file means
    /// resuming would have nothing to continue, which fails the task instead.
    pub fn round_budget_pause(&self, proposal_id: &str) -> Option<RoundBudgetPause> {
        let path = PendingCommandStore::new(&self.data_dir).path_for(proposal_id);
        let content = fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        let round_budget = value.get("round_budget")?;
        Some(RoundBudgetPause {
            rounds_used: round_budget.get("rounds_used")?.as_u64()? as u32,
            rounds_granted: round_budget.get("rounds_granted")?.as_u64()? as u32,
        })
    }

    /// The host approval a paused fetch turn was waiting on, or `None` when the
    /// id names no paused turn, or one that is not a fetch pause (spec 51).
    ///
    /// Recovery reads it to re-present the dialog after a crash. The whole call
    /// is returned because the card shows the URL verbatim, and resuming must
    /// ask about the same string the dialog showed.
    pub fn fetch_call(&self, proposal_id: &str) -> Option<PendingFetchCall> {
        let path = PendingCommandStore::new(&self.data_dir).path_for(proposal_id);
        let content = fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        serde_json::from_value(value.get("fetch_call")?.clone()).ok()
    }

    /// The manifest a paused context review would send, or `None` when the id
    /// names no paused turn, or one that is not a context review (spec 27).
    ///
    /// Read as loose JSON for the same reason as the other readers: a file
    /// written by a different version must answer "not a review" rather than
    /// fail to deserialise and look absent.
    pub fn context_review_manifest(&self, proposal_id: &str) -> Option<ContextManifest> {
        let path = PendingCommandStore::new(&self.data_dir).path_for(proposal_id);
        let content = fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        let manifest = value
            .get("context_review")?
            .get("prepared")?
            .get("manifest")?
            .clone();
        serde_json::from_value(manifest).ok()
    }

    /// The proposal ids a paused check batch holds, in card order, or `None`
    /// when the id names no paused turn, or one that is not a check batch
    /// (spec 23). Recovery reads it to know whether the batch can still be
    /// re-presented; an empty list means the card would ask nothing.
    pub fn check_batch_proposals(&self, proposal_id: &str) -> Option<Vec<String>> {
        let path = PendingCommandStore::new(&self.data_dir).path_for(proposal_id);
        let content = fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        if let Some(checks) = value
            .get("check_batch")
            .and_then(|batch| batch.get("checks"))
            .and_then(|checks| checks.as_array())
        {
            return Some(
                checks
                    .iter()
                    .filter_map(|check| {
                        check
                            .get("proposal_id")
                            .and_then(|id| id.as_str())
                            .map(str::to_string)
                    })
                    .collect(),
            );
        }
        // A failed check's baseline uses the same card (spec 109 D1); its
        // batch is serialized camelCase on the `failure_baseline` marker.
        let checks = value
            .get("failure_baseline")?
            .get("batch")?
            .get("checks")?
            .as_array()?;
        Some(
            checks
                .iter()
                .filter_map(|check| {
                    check
                        .get("proposalId")
                        .and_then(|id| id.as_str())
                        .map(str::to_string)
                })
                .collect(),
        )
    }
}

/// Takes a paused turn off disk without resuming it, for a pause that was
/// settled because a newer decision replaced it (spec 113 D3). Goes through
/// [`PendingCommandStore::take`], the method the resume routes use, so there
/// is one way a paused turn leaves the disk. Returns whether one was taken: a
/// patch pause saves no paused turn, and a file a resume already took is gone.
pub(crate) fn take_superseded_paused_turn(data_dir: &Path, proposal_id: &str) -> bool {
    PendingCommandStore::new(data_dir).take(proposal_id).is_ok()
}

#[derive(Debug, Clone)]
pub(super) struct PendingCommandStore {
    pub(super) data_dir: PathBuf,
}

impl PendingCommandStore {
    pub(super) fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    fn path_for(&self, proposal_id: &str) -> PathBuf {
        self.data_dir
            .join("chat")
            .join("pending")
            .join(format!("{proposal_id}.json"))
    }

    pub(super) fn has(&self, proposal_id: &str) -> bool {
        self.path_for(proposal_id).exists()
    }

    /// Whether the pending turn is a question pause. Parsed as loose JSON so a
    /// file written by a version without the field still answers "no", and an
    /// unreadable file is not mistaken for a question.
    pub(super) fn is_question(&self, proposal_id: &str) -> bool {
        let Ok(content) = fs::read_to_string(self.path_for(proposal_id)) else {
            return false;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
            return false;
        };
        value
            .get("question")
            .is_some_and(|question| !question.is_null())
    }

    pub(super) fn save(&self, pending: &PendingChatTurn) -> Result<()> {
        let path = self.path_for(&pending.proposal_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(pending).map_err(|error| {
            ClientError::InvalidInput(format!("Failed to serialize pending chat turn: {error}"))
        })?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Loads the pending state without consuming it. Used by a paused review's
    /// edit routes, where the turn must remain resumable afterwards.
    pub(super) fn load(&self, proposal_id: &str) -> Result<PendingChatTurn> {
        let path = self.path_for(proposal_id);
        let content = fs::read_to_string(&path).map_err(|_| {
            ClientError::InvalidInput(format!("No pending chat turn for proposal: {proposal_id}"))
        })?;
        serde_json::from_str(&content).map_err(|error| {
            ClientError::InvalidInput(format!("Failed to parse pending chat turn: {error}"))
        })
    }

    /// Loads and removes the pending state — a command decision can only be
    /// resumed once.
    pub(super) fn take(&self, proposal_id: &str) -> Result<PendingChatTurn> {
        let path = self.path_for(proposal_id);
        let content = fs::read_to_string(&path).map_err(|_| {
            ClientError::InvalidInput(format!("No pending chat turn for proposal: {proposal_id}"))
        })?;
        let pending: PendingChatTurn = serde_json::from_str(&content).map_err(|error| {
            ClientError::InvalidInput(format!("Failed to parse pending chat turn: {error}"))
        })?;
        let _ = fs::remove_file(&path);
        Ok(pending)
    }
}
