use super::*;
use crate::protected_paths::TaskProtectedPaths;

/// Everything one agentic turn carries from round to round. A pause persists
/// the durable part of it as a `PendingChatTurn`; the rest is rebuilt when the
/// turn resumes.
pub(super) struct TurnState<'a> {
    pub(super) repository_root: &'a Path,
    pub(super) session: Session,
    pub(super) task: Task,
    pub(super) context_files: Vec<String>,
    pub(super) messages: Vec<ModelMessage>,
    pub(super) action_rounds: u32,
    pub(super) exploration_rounds: u32,
    pub(super) turn_notes: TurnNotesState,
    /// Completed native tool rounds, carried across a pause so the next
    /// request after a reload compacts from the same typed facts.
    pub(super) completed_tool_rounds: CompletedToolTracker,
    /// Evidence accrued since the open step started (spec 66 §5.3).
    pub(super) step_evidence: Vec<crate::plan::Evidence>,
    /// Whether this turn already sent its one plan reminder (spec 66 §5.2).
    pub(super) plan_reminder_sent: bool,
    pub(super) turn_options: ChatTurnOptions,
    pub(super) active_protected: TaskProtectedPaths,
    /// Per-turn MCP runtime: connects lazily, caches tool lists and
    /// connections for this turn, and tears everything down on drop.
    pub(super) mcp: McpRuntime,
    /// Captured once before any tool is offered and never re-read: a mode
    /// change mid-turn takes effect on the next turn (spec 20 §5.4).
    pub(super) mode: SessionMode,
    pub(super) ask_user_offered: bool,
    pub(super) fetch_url_offered: bool,
    pub(super) native_tools: Option<Vec<ToolDefinition>>,
    /// The MCP tools the turn's messages named when `native_tools` was last
    /// extended (spec 33 §5.1 stage 2), so a round that named nothing new
    /// contacts no server.
    pub(super) named_mcp_tools: BTreeSet<McpToolKey>,
    /// Whatever the model has produced so far, so a stop between rounds
    /// still has an answer to preserve.
    pub(super) partial_response: String,
    /// Whether any model call in this turn has been accounted for, so a stop
    /// between rounds does not record a zero on top of real spending.
    pub(super) recorded_any_usage: bool,
    pub(super) web_debug_mode: bool,
    pub(super) failed_browser_calls: HashMap<String, u32>,
    pub(super) plan: Option<TaskPlan>,
    pub(super) plan_approved: bool,
    pub(super) worker_session: bool,
    pub(super) preplan_final_attempt: bool,
    /// Extra rounds past the configured segment, from a token ceiling or a
    /// round-budget *Continue* (spec 61 §5.2).
    pub(super) round_budget_extension: u32,
    pub(super) max_exploration_rounds: u32,
    /// Already-dispatched read calls since the last write (spec 59 §5.3).
    pub(super) seen_read_calls: HashSet<Vec<String>>,
    /// The answer that triggered the one plan reminder, if one was sent
    /// (spec 66 `context.md` D4).
    pub(super) pre_reminder_answer: Option<String>,
    /// Automatic worker segments (spec 103 Task 5).
    pub(super) segment_tracker: Option<SegmentRunState>,
    /// Spec 107 D3 (A): an ordinary Code-mode turn crossing its checkpoint.
    pub(super) ordinary_continuation: Option<OrdinaryContinuation>,
    /// A request prepared for an automatic boundary, reused for the very next
    /// model call so one crossing records one compaction attempt.
    pub(super) boundary_request: Option<PreparedTurnRequest>,
}

/// How the round loop ended: the final model run, the answer shown to the
/// user, any card it raised, and why it stopped.
pub(super) type RoundOutcome = (ModelRun, String, TurnProposals, StopReason);

/// What a phase of the round loop decided. The loop, which owns the turn's
/// state, carries it out: a pause takes that state by value.
pub(super) enum RoundStep {
    /// Go on with the rest of this round.
    Proceed,
    /// Start the next round from the top.
    NextRound,
    /// End the round loop with this outcome.
    Finish(Box<RoundOutcome>),
    /// Pause the turn for the round-budget card.
    PauseForRoundBudget,
}

/// Why a model call ended the round instead of answering.
pub(super) enum ModelCallStop {
    /// The user stopped the turn mid-stream; the run is accounted already.
    Stopped(Box<ModelRun>),
    /// The provider refused and another configured provider could answer
    /// (spec 56): the turn pauses on a declined-by-default offer.
    FallbackOffered(PendingProviderFallback),
}
