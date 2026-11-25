use super::*;

type McpTokenResolverFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// What a tool arm observed, so its marker is finished with the tool's own
/// answer rather than with the fact that dispatch returned.
///
/// Kept separate from the arm's text output because the text is for the model
/// and this is for the log: a tool that reports an error in prose still has to
/// record that it failed, and reading the prose back to find out would be
/// guessing. See
/// `docs/specs/21_task_plan_progress_and_budget/context.md` §3.4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ActionOutcome {
    /// Dispatch succeeded and the tool reported nothing to the contrary.
    Ok,
    /// A command ran. The code is whatever the process reported, and `None`
    /// means it was killed or signalled — not that it passed. The purpose is
    /// the execution's own (spec 63 §5.3), so the step's status can tell a
    /// failed check from a search that found nothing.
    CommandExit(Option<i32>, CommandPurpose),
    /// A file was read, carrying what was read and the hash of the content.
    ///
    /// Both travel out of the arm for the same reason the exit code does:
    /// nothing downstream can recover them, and a step whose only work was
    /// reading would otherwise report *completed unverified* despite the
    /// engine having watched the read happen.
    FileRead { path: String, hash: String },
    /// The tool itself reported a failure: an MCP `is_error` or transport
    /// error, a browser diagnostic that could not run. Not a command, so there
    /// is no exit code to carry.
    Failed,
    /// A stop reached a concurrent batch before this call started, so it did
    /// nothing. Distinct from `Failed`: nothing was attempted, and the turn is
    /// about to end cancelled rather than treat the tool as having errored.
    Cancelled,
}

/// The evidence an arm's outcome supports, or `None` where the engine observed
/// nothing a step's status could honestly rest on.
///
/// Requirement 6 lives here as much as in the enum: this is the only place a
/// tool dispatch becomes [`Evidence`], the inputs are values the engine holds,
/// and no model output reaches it. Nothing constructs `Evidence` from a tool
/// argument, and nothing should.
///
/// [`ActionOutcome::Ok`] yields `None` rather than a synthetic success record.
/// A `read_file` that worked says nothing about whether the step it served is
/// done, and minting evidence from it would make every step look verified —
/// the letter-not-spirit failure requirement 6 exists to prevent.
/// [`ActionOutcome::Failed`] likewise: it carries no code, so there is nothing
/// to record that would not be invented, and the step's status follows from
/// the *absence* of confirming evidence.
///
/// `command` is the command line that ran, when the action was one. Only its
/// hash is kept, so a later run of the same command can supersede this exit
/// (spec 21 §5.3). `working_directory` is the directory it ran in, already
/// repository-relative and normalized the way a `CheckId` is; spec 105's check
/// identity is that directory *and* the command.
pub(super) fn evidence_for(
    outcome: &ActionOutcome,
    marker_id: &str,
    command: Option<&str>,
    working_directory: Option<&str>,
) -> Option<crate::plan::Evidence> {
    match outcome {
        ActionOutcome::CommandExit(exit_code, purpose) => {
            Some(crate::plan::Evidence::CommandExit {
                marker_id: marker_id.to_string(),
                exit_code: *exit_code,
                purpose: *purpose,
                command_hash: command.map(command_hash),
                working_directory: working_directory.unwrap_or_default().to_string(),
            })
        }
        // No marker id: unlike a command, the fact worth keeping is *which
        // content* was read, and the hash says that across turns while a
        // marker id only points at the call. §5.3's rule that evidence must
        // carry the fact rather than a pointer into a store that expires.
        ActionOutcome::FileRead { path, hash } => Some(crate::plan::Evidence::FileRead {
            path: path.clone(),
            hash: hash.clone(),
        }),
        ActionOutcome::Ok | ActionOutcome::Failed | ActionOutcome::Cancelled => None,
    }
}

/// The hash `Evidence::CommandExit::command_hash` holds: of the exact text, so
/// only an identical command line counts as the same command.
pub(super) fn command_hash(command: &str) -> String {
    crate::hash::sha256(command)
}

/// The command line a tool action runs, if it runs one.
pub(super) fn action_command(action: &ToolAction) -> Option<&str> {
    match action {
        ToolAction::Command(request) => Some(request.command.as_str()),
        _ => None,
    }
}

/// Why the agent loop ended.
///
/// An enum rather than two booleans: two flags admit a state where both are
/// set, and the audit status string and the task status would then each have to
/// pick one arbitrarily. The loop can only stop for one reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StopReason {
    /// The model produced an answer, or the turn is pausing for a human.
    Answered,
    /// `agent_max_tool_rounds` was reached and the model still wanted a tool.
    ToolBudget,
    /// `agent_max_task_tokens` was reached. Unlike [`Self::ToolBudget`] this is
    /// detected *before* a model call rather than after one, so no tokens are
    /// spent discovering it (`context.md` §3.3).
    TokenBudget,
    /// A bounded worker task stopped for one of spec 103 Task 6's typed
    /// outcomes. The status and the outcome are written together at settlement.
    TaskTerminal(Box<crate::task_outcomes::TaskTerminalOutcome>),
}

impl StopReason {
    /// The name written to `plan_left_open` and read by the eval harness.
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::ToolBudget => "tool_budget",
            Self::TokenBudget => "token_budget",
            Self::TaskTerminal(outcome) => outcome.kind.as_str(),
        }
    }
}

/// Which stage of a turn is running. Drives the progress indicator, so the user
/// can tell a slow provider apart from a running tool apart from a hang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseKind {
    Context,
    Model,
    /// The model is producing thinking-mode reasoning and no answer text yet
    /// (spec 81). Follows the round's `Model` phase, repeated as
    /// [`TurnPhase::reasoning_chars`] grows, so a long wait shows movement.
    Thinking,
    Tool,
    Finalizing,
    /// A line of a running command's output, streamed before it exits
    /// (requirement 5). Distinct from the other kinds so a client can render it
    /// as a log line rather than replacing the status badge it is not.
    Output,
}

impl PhaseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Model => "model",
            Self::Thinking => "thinking",
            Self::Tool => "tool",
            Self::Finalizing => "finalizing",
            Self::Output => "output",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnPhase {
    pub kind: PhaseKind,
    /// Human-readable detail, supplied here rather than in the UI so the
    /// frontend never needs to know tool names. Empty when the kind says it all.
    pub label: String,
    /// **1-based**, unlike the loop counter it comes from.
    pub round: u32,
    pub max_rounds: u32,
    /// Exploration rounds used so far this turn (spec 59 §5.5). A count, not a
    /// position: the label shows it as "12 reads", and there is no "1-based"
    /// variant of it.
    pub exploration_round: u32,
    pub max_exploration_rounds: u32,
    /// Characters of `turn_notes` this turn holds (spec 65 §5.8). A count, not
    /// a budget: the label shows "· notes 2.1k" only when it is non-zero.
    pub notes_chars: usize,
    /// Characters of reasoning this model call has streamed so far. Non-zero
    /// only on a `Thinking` phase (spec 81); a count, never the text.
    pub reasoning_chars: usize,
    /// The phase belongs to a failure-time baseline run (spec 114 D5): its
    /// running and finished lines and its output. A flag rather than a new
    /// [`PhaseKind`], so a baseline's output stays an `Output` line and every
    /// reader of the kind is unchanged; a client keys its Baseline tag on
    /// this, never on the label's text.
    pub baseline: bool,
}

impl TurnPhase {
    pub(super) fn new(
        kind: PhaseKind,
        label: impl Into<String>,
        round: u32,
        max_rounds: u32,
        exploration_round: u32,
        max_exploration_rounds: u32,
        notes_chars: usize,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            round: round + 1,
            max_rounds,
            exploration_round,
            max_exploration_rounds,
            notes_chars,
            reasoning_chars: 0,
            baseline: false,
        }
    }
}

/// Out-of-band progress about a turn, distinct from the answer text itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnProgress {
    /// The turn's session id, reported as soon as it exists so a client that
    /// stops before the turn finishes can still identify what it stopped.
    Session(String),
    Phase(TurnPhase),
    /// The turn's plan, whole, each time it changes (spec 21 §5.6).
    ///
    /// Sent whole rather than as a delta because the panel's job is to show
    /// the current state of every step at once, and a client that missed one
    /// delta — a reconnect, a dropped frame — would then render a plan that
    /// never existed. The plan is small and changes a handful of times per
    /// turn, so there is nothing to save by being clever here.
    Plan(TaskPlan),
    /// The plan at the moment the turn truly ended, sent once so the panel can
    /// stop describing an open step as running (spec 66 §5.4). A pause sends
    /// none: the turn is not over. Carries the plan whole, like [`Self::Plan`],
    /// so the final event also carries evidence attached at the end.
    PlanFinished(TaskPlan),
    /// One browser diagnostic run, already redacted, sent once per run when
    /// it finishes (spec 12 `context.md` §3.3). The same record is appended
    /// to the session log, so a reload shows what the live stream did.
    /// Boxed because a record carries a whole report, several times larger
    /// than every other variant.
    WebDiagnostic(Box<WebDiagnosticRecord>),
    /// What this turn is sending, emitted after assembly and before the first
    /// model call (spec 27 §5.1, C9). Boxed because a manifest carries every
    /// included ref and exclusion; it is sent once or twice per turn, not per
    /// token. The shell sees the manifest only — never an item's content.
    Context(Box<ContextManifest>),
    /// The turn is holding at a round boundary because its steering asked it
    /// to (spec 57 §5.4). Only a host that passed `steering` ever sees this.
    Paused,
    /// The paused turn continues, in the same task.
    Resumed,
    /// A steering note entered the conversation as this user message.
    NoteAdded {
        message_id: String,
    },
    /// The task's active protected paths at the start of this turn (spec 98).
    ///
    /// This is the resolved durable record, not a re-parse of any prompt, and
    /// it carries the task id so a viewer can join it to the turn. A worker's
    /// progress sink writes it as its own line. The desktop panel reads the same
    /// record from the session payload, so the shell's SSE relay does not need a
    /// second rendering route.
    ProtectedPaths(TaskProtectedPaths),
}

/// The per-turn side channel: where answer tokens go, where progress goes, and
/// whether the user has asked to stop.
///
/// Grouped into one value because all three travel together through the whole
/// turn, and threading them as separate parameters would push
/// [`ChatOrchestrator::run_agentic_turn`]'s argument list further past the point
/// where clippy already objects.
pub struct TurnSink<'a> {
    pub on_token: &'a mut dyn FnMut(&str),
    pub on_progress: &'a mut dyn FnMut(TurnProgress),
    pub cancel: &'a CancelToken,
    /// Pause and steering notes, applied at round boundaries. `None` for every
    /// host but a worker, and then the turn is exactly what it was before
    /// spec 57 (§5.4).
    pub steering: Option<&'a TurnSteering>,
}

impl TurnSink<'_> {
    pub(super) fn session(&mut self, session_id: &str) {
        (self.on_progress)(TurnProgress::Session(session_id.to_string()));
    }

    // The counts travel together: every caller already has all of them at the
    // point it reports progress, and grouping them into a struct would trade a
    // stable call shape for one that has to be rebuilt at seven sites.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn phase(
        &mut self,
        kind: PhaseKind,
        label: impl Into<String>,
        round: u32,
        max_rounds: u32,
        exploration_round: u32,
        max_exploration_rounds: u32,
        notes_chars: usize,
    ) {
        (self.on_progress)(TurnProgress::Phase(TurnPhase::new(
            kind,
            label,
            round,
            max_rounds,
            exploration_round,
            max_exploration_rounds,
            notes_chars,
        )));
    }

    pub(super) fn plan(&mut self, plan: &TaskPlan) {
        (self.on_progress)(TurnProgress::Plan(plan.clone()));
    }

    pub(super) fn plan_finished(&mut self, plan: &TaskPlan) {
        (self.on_progress)(TurnProgress::PlanFinished(plan.clone()));
    }

    pub(super) fn web_diagnostic(&mut self, record: WebDiagnosticRecord) {
        (self.on_progress)(TurnProgress::WebDiagnostic(Box::new(record)));
    }

    pub(super) fn context(&mut self, manifest: &ContextManifest) {
        (self.on_progress)(TurnProgress::Context(Box::new(manifest.clone())));
    }

    pub(super) fn progress(&mut self, event: TurnProgress) {
        (self.on_progress)(event);
    }
}

/// Resolves an MCP server's `auth_token_env` reference (`keychain:<account>`
/// or an environment variable name) to the actual bearer token. The engine
/// never reads the keychain itself; the desktop shell injects a resolver that
/// does. Wrapped so [`ChatOrchestrator`] can stay `Debug`/`Clone`.
#[derive(Clone)]
pub struct McpTokenResolver(pub(super) McpTokenResolverFn);

impl McpTokenResolver {
    pub fn new(resolver: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(resolver))
    }

    pub(super) fn resolve(&self, reference: &str) -> Option<String> {
        (self.0)(reference)
    }
}

impl std::fmt::Debug for McpTokenResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("McpTokenResolver(..)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommandProposal {
    pub id: String,
    pub command: String,
    pub prompt: String,
    pub risk: String,
    pub requires_approval: bool,
    pub blocked: bool,
    /// The policy's deciding cause in plain words (spec 62 §5.2): the first
    /// rendered `reasons` entry, which is derived from the classification's
    /// causes. Shown beside the risk badge, outside the rationale disclosure,
    /// because it is what tells the user whether this prompt is routine.
    pub cause: String,
    /// The model's own stated reason for wanting the command (spec 62 §5.2).
    /// Stored on the proposal all along; the card shows it as "The agent says:
    /// …" so the user sees both what the agent wants and why Damaian asks.
    pub reason: String,
    /// Whether the approval UI may offer "allow always" for this proposal.
    /// Always false for MCP tool calls, which aren't shell commands and so
    /// have no `command_allowlist` entry to write.
    pub allow_always: bool,
    /// Whether the UI may offer a session-scoped browser diagnostic allowance.
    /// This is intentionally separate from command allowlisting: it writes a
    /// session event, not repository or user config.
    pub allow_browser_diagnostics_for_session: bool,
    /// Whether the Approve button may offer **Approve for this session**
    /// (spec 109 D5): the command qualifies by why the policy asks.
    pub session_approvable: bool,
    /// Why it is not offered, in words for the card; empty when it is, and
    /// for a call that is not a local command.
    pub session_approval_refusal: String,
    /// Where the command runs, relative to the repository root; empty at the
    /// root and for a call that is not a local command. A check card shows it,
    /// since two checks can share a command and differ only here (spec 90).
    pub directory: String,
}

/// Every check the verification loop wants to run that needs approval, as one
/// card (spec 23 proposal §5.3).
///
/// One card for many checks because a loop that produces five sequential
/// prompts is a loop users click through blindly. Each entry keeps its own
/// proposal, so *Allow Always* remains per check and exact-command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCheckBatch {
    /// The id to hand back to
    /// [`ChatOrchestrator::resume_after_check_decision`].
    pub id: String,
    pub checks: Vec<AgentCommandProposal>,
    /// The card's own heading and explanation when it is not the ordinary
    /// "checks need approval" card: a failed check's baseline (spec 109 D1)
    /// says why it asks. `None` keeps the renderer's ordinary wording.
    pub title: Option<String>,
    pub note: Option<String>,
}

/// A patch the model proposed mid-conversation via the `propose_patch` tool
/// call (as opposed to `EditOrchestrator::propose_edit`'s dedicated one-shot
/// flow). Carries the same `ProposedFilePatch` data the text-envelope path
/// produces, so the UI can render it with the exact same component either
/// way (`patch_id` + `summary` + `files` mirrors `/api/propose-edit`'s
/// response shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPatchProposal {
    pub patch_id: String,
    pub summary: String,
    pub files: Vec<ProposedFilePatch>,
    /// The engine's typed one-shot repair-scope review (spec 102 Task 5), when
    /// this patch expands the repair beyond the task's changed paths and the
    /// active report's finding locations. `None` for an ordinary patch and for
    /// a repair that stays in scope.
    pub repair_scope: Option<crate::repair_scope::RepairScopeReview>,
}

/// A plan put up for review before the turn takes its first mutating step
/// (spec 21 §5.5).
///
/// Carries the whole plan rather than a summary: the user is being asked to
/// reorder, retitle or delete steps, and cannot do that from a count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPlanProposal {
    /// The id to hand back to
    /// [`ChatOrchestrator::resume_after_plan_decision`]. Distinct from the
    /// task id: a task may be reviewed once, but the pending turn behind the
    /// pause is what the id addresses.
    pub id: String,
    pub plan: TaskPlan,
    /// What the turn was about to do when it stopped, in the same words the
    /// progress line uses — so the card can say what the approval unblocks
    /// rather than asking the user to approve in the abstract.
    pub deferred_action: String,
    /// The task execution budget this review offers, when the review belongs to
    /// a genuine worker session (spec 103 Task 2).
    ///
    /// `None` for an inline plan review and for every historical review. The
    /// review carries its own labels, units and rows, so a client renders it
    /// generically and returns the exact [`TaskBudgetReview::budget`] it was
    /// given.
    pub budget_review: Option<TaskBudgetReview>,
}

/// A prompt for a worker started from a paused plan, and the task whose
/// protections the worker inherits (spec 94, spec 98).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanWorkerPrompt {
    pub prompt: String,
    /// The paused plan task's id. The shell passes it with the spawn request so
    /// the worker snapshots that task's durable protected paths rather than
    /// re-parsing this constructed prompt.
    pub task_id: String,
}

/// A question the model asked mid-turn, waiting for the user's answer
/// (spec 50). Resumed through [`ChatOrchestrator::resume_after_answer`].
///
/// Carries no proposal id, because a question is not a proposal: the id
/// addresses the paused turn, and answering it authorises nothing. The text is
/// already redacted — the chat loop runs the secret scanner before it is stored
/// (`context.md` §7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentQuestionProposal {
    /// The id to hand back to [`ChatOrchestrator::resume_after_answer`].
    pub question_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub allow_free_text: bool,
}

/// One host approval the model asked for mid-turn (spec 51 §5.3 constraint 1).
///
/// Carries the host, the URL exactly as the model wrote it, and the display
/// flags the guard computed. The URL is never shortened here: the dialog is a
/// consent surface, and a user approving a request has to be able to read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFetchProposal {
    /// The id to hand back to [`ChatOrchestrator::resume_after_fetch_decision`].
    pub id: String,
    pub host: String,
    /// The raw URL, verbatim. The dialog shows this string and nothing derived
    /// from it.
    pub url: String,
    /// What `Allow Always` stores (D8): the host plus a non-default port.
    pub approval_key: String,
    /// A query or fragment over the display threshold, so the dialog can break
    /// it out rather than leave it inside a long line.
    pub long_components: Vec<FetchComponentFlag>,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

/// One over-long URL component, as the dialog renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchComponentFlag {
    /// `"query"` or `"fragment"`.
    pub component: String,
    pub bytes: usize,
}

/// What the user decided about a fetch pause (spec 51 §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchDecision {
    /// Read this URL now, without remembering the host.
    Once,
    /// Read this URL now and remember the host for this checkout.
    Always,
    /// Read nothing. The model is told the host was refused.
    Deny,
}

/// A one-shot offer to answer a refused turn with a different configured
/// provider (spec 56). Declined by default: nothing switches until the user
/// approves this one occurrence, and there is no "always".
///
/// Names both sides so the card can say who refused and who would answer —
/// approving in the abstract is what the spec rules out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFallbackProposal {
    /// The id to hand back to
    /// [`ChatOrchestrator::resume_after_fallback_decision`].
    pub id: String,
    pub refused_provider: String,
    pub refused_model: String,
    pub fallback_provider: String,
    /// The model the fallback would answer with: its first configured model,
    /// or empty when it lists none and the shell's own default applies.
    pub fallback_model: String,
    /// The refusal's stable code, e.g. `provider_rate_limited`.
    pub refusal_code: String,
    /// The provider's own message, shown verbatim (spec 48 §5.7).
    pub message: String,
}

/// The pause a turn takes when it reaches its round budget with the model
/// still wanting a tool (spec 61). Resumed through `resume_round_budget`.
///
/// Names the rounds the turn has used and been granted so the row can say
/// "Used 8 of 8" without the model guessing a number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRoundBudgetProposal {
    /// The id to hand back to `resume_round_budget`.
    pub id: String,
    /// The round that hit the limit.
    pub rounds_used: u32,
    /// The round limit the turn had this segment (`max_rounds`).
    pub rounds_total: u32,
    /// The running total of extra rounds granted across continuations.
    pub rounds_granted: u32,
    /// How many more rounds a *Continue* grants (`default_tool_round_limit`).
    /// Surfaced so the button can say "Continue — 8 more" without the client
    /// guessing a config value.
    pub rounds_more: u32,
    /// `ABSOLUTE_TURN_ROUND_CAP` reached, so only *Answer now* is offered.
    pub at_hard_cap: bool,
}

/// How the user answers a round-budget pause (spec 61 §5.2).
///
/// `Continue` grants the same turn another `agent_max_tool_rounds`;
/// `AnswerNow` makes the forced final call with tools dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundBudgetDecision {
    Continue,
    AnswerNow,
}

/// How a plan revision's `check` key maps onto the step it names (spec 105 §2,
/// §7).
///
/// The revision is the full surviving step list, so the three wire spellings
/// mean three different things: the key absent means the client predates
/// structured checks and wants the existing check preserved — never a silent
/// removal; `null` removes the check; an object sets it. The user shapes a
/// proposed check here, and only the settled plan stores what they approved.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PlanRevisionCheck {
    #[default]
    Preserve,
    Remove,
    Set(PlannedCheck),
}

/// One step of a user's revision to a proposed plan.
///
/// Identifies the step by id rather than position so a reorder and a deletion
/// in the same revision cannot be misread as each other. A step the plan does
/// not contain is ignored, and one the user omits is dropped — that is how
/// deletion is expressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRevisionStep {
    pub id: String,
    pub title: String,
    /// The structured check the user settled on for this step: preserve the
    /// existing one, remove it, or set a new one.
    pub check: PlanRevisionCheck,
}

// No `Eq`: it carries a `ModelRun`, whose `reported_cost` is an `Option<f64>`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatTurnResult {
    pub session: Session,
    pub task: Task,
    pub model_run: ModelRun,
    pub context_files: Vec<String>,
    pub response: String,
    pub command_proposal: Option<AgentCommandProposal>,
    pub patch_proposal: Option<AgentPatchProposal>,
    /// The turn stopped to have its plan reviewed before taking its first
    /// mutating step. Resumed through
    /// [`ChatOrchestrator::resume_after_plan_decision`].
    pub plan_proposal: Option<AgentPlanProposal>,
    /// The model asked a question mid-turn and the turn is waiting for the
    /// user's answer. Resumed through
    /// [`ChatOrchestrator::resume_after_answer`] (spec 50).
    pub question_proposal: Option<AgentQuestionProposal>,
    /// The turn stopped before a request because the host has no approval.
    /// Resumed through [`ChatOrchestrator::resume_after_fetch_decision`]
    /// (spec 51 §5.3).
    pub fetch_proposal: Option<AgentFetchProposal>,
    /// The provider refused and a different configured provider could answer.
    /// The turn is paused, not failed, until
    /// [`ChatOrchestrator::resume_after_fallback_decision`] decides.
    pub fallback_proposal: Option<AgentFallbackProposal>,
    /// The turn reached its round budget and paused before the forced final
    /// call. Resumed through `resume_round_budget` (spec 61).
    pub round_budget_proposal: Option<AgentRoundBudgetProposal>,
    /// The verification loop is holding on a card listing the checks that need
    /// approval. Resumed through [`ChatOrchestrator::resume_after_check_decision`]
    /// (spec 23).
    pub check_batch: Option<AgentCheckBatch>,
    /// The report a verification turn produced, as data for the report card.
    /// The same content is in `response`; this is the structured copy.
    pub verification_report: Option<crate::verification::VerificationReport>,
    /// The user stopped this turn. Distinct from a failure: `response` holds
    /// whatever had been generated, and it is persisted.
    pub cancelled: bool,
    /// What this task has spent, summed over every model call it made — not
    /// just [`Self::model_run`], which is only the last one.
    ///
    /// Read back from the log rather than accumulated in memory, so the
    /// figure a client sees when the turn ends is the same one it sees after
    /// a reload. `None` only if the log could not be read.
    pub usage: Option<TaskUsage>,
    /// Cost computed from the user's own configured rates, when they have set
    /// any. Kept separate from [`TaskUsage::reported_cost`] on purpose: one is
    /// what the provider charged, the other is the user's own arithmetic, and
    /// presenting the second as the first would launder a guess into a fact.
    ///
    /// A [`CostEstimate`] rather than a bare `f64` so the upper-bound label
    /// travels with the number: spec 49 §5.3.
    pub estimated_cost: Option<CostEstimate>,
    /// The turn's final notes, already redacted, and whether they include text
    /// carried from an earlier turn (spec 65 §5.8). Empty notes show nothing.
    pub turn_notes: Option<TurnNotesView>,
    /// The turn paused after assembly and before its first model call so the
    /// user can review its context (spec 27 §5.1). Resumed through
    /// [`ChatOrchestrator::resume_after_context_decision`]. Carries the
    /// manifest only; the prepared items stay on disk.
    pub context_review: Option<AgentContextReview>,
    /// The engine-owned outcome and action model for this turn's task
    /// (spec 101 Task 3). It carries the durable completion reasons and
    /// waivers, so a live result and a reloaded session render the same truth.
    /// A CLI reads this field directly rather than rebuilding it.
    pub task_control: TaskControl,
    /// The engine-owned indicator and stop-row presentation for the live turn.
    pub turn_presentation: TurnPresentation,
}

/// A paused pre-send context review (spec 27 §5.1).
///
/// `manifest` is the whole view: categories, included refs by path and range,
/// exclusions and the limit. It deliberately has no `items` field — the shell
/// must never receive item content, and a structure that cannot hold it is a
/// stronger guarantee than a renderer that remembers not to print it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentContextReview {
    pub id: String,
    pub manifest: ContextManifest,
}

/// What the user decided on a paused context review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextDecision {
    Send,
    Decline,
}

/// What the user sees of a turn's notes (spec 65 §5.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnNotesView {
    pub text: String,
    pub carried: bool,
}

/// What a turn ended holding out for a human, if anything.
///
/// Grouped rather than carried as three separate `Option`s through the loop's
/// break value: every `break` site has to name all of them, and a fourth kind
/// of pause would otherwise mean editing eight places that have no opinion
/// about it.
#[derive(Debug, Default)]
pub(super) struct TurnProposals {
    pub(super) command: Option<AgentCommandProposal>,
    pub(super) patch: Option<AgentPatchProposal>,
    pub(super) plan: Option<AgentPlanProposal>,
    pub(super) question: Option<AgentQuestionProposal>,
    pub(super) fetch: Option<AgentFetchProposal>,
    pub(super) checks: Option<AgentCheckBatch>,
    /// Whether the work a plan review defers changes code rather than only
    /// documentation (spec 107 D3): decides the one worker suggestion.
    pub(super) plan_changes_code: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ChatTurnOptions {
    /// This turn has already been offered — and has approved — a provider
    /// fallback, so a second refusal fails rather than offering again
    /// (spec 56 §5.5: a loop of offers is the failure mode the offer exists
    /// to avoid). Carried here because these options are already saved with
    /// every paused turn, so the guard survives a later command pause and a
    /// restart within the same turn. Callers leave it `false`; the engine sets
    /// it on an approved switch. Public only so callers can build the struct
    /// with `..Default::default()` — and setting it can only *suppress* an
    /// offer, which is the fail-closed direction, never cause a switch.
    #[serde(default)]
    pub provider_fallback_used: bool,
    /// A real person is present *while this turn runs*, which is what
    /// `fetch_url` needs because its first host approval is a dialog on the
    /// turn's own path (spec 51 §5.3).
    ///
    /// Spec 104 split the old `user_present` test in two. The alias keeps
    /// pending turns written before the split loadable: an old `true` was
    /// always an interactive session, so it grants both capabilities below.
    #[serde(default, alias = "user_present")]
    pub synchronous_user_present: bool,
    /// A question can be delivered to a person and answered later, without
    /// the asker being synchronously present (spec 104). The desktop app's
    /// ordinary sessions set this, app-managed workers set only this, and a
    /// truly headless CLI or eval turn sets neither. `ask_user` is offered
    /// when this is true and the per-turn cap is above zero.
    #[serde(default)]
    pub question_route_available: bool,
    /// Extra tool rounds granted to this turn across round-budget continuations
    /// (spec 61 §5.2). Seeded on resume from the pending turn's running
    /// `rounds_granted`, so the resumed turn's `max_rounds` continues where it
    /// stopped instead of resetting to the base segment. Callers leave it `0`.
    #[serde(default)]
    pub round_budget_extension: u32,
    /// The user chose *Answer now* on a round-budget pause: this re-entry is
    /// the forced final call, so it must not pause again at the limit
    /// (spec 61 §5.2). Callers leave it `false`.
    #[serde(default)]
    pub round_budget_answer_now: bool,
    /// This turn is Damaian's verification continuation (spec 23). Its plan
    /// steps are the checks the applied patch triggered, not a plan the model
    /// proposed, so the plan review gate does not apply: asking the user to
    /// approve "run `cargo test`" would be asking about Damaian's own
    /// bookkeeping, and the repair request is what the model actually answers.
    /// Set only by `ChatOrchestrator::run_repair_turn`; a paused turn carries
    /// it so a resume keeps the same exemption.
    #[serde(default)]
    pub verification_turn: bool,
    /// The run's plan was already carried by a durable transaction (spec 101
    /// Task 3). Set only by the engine's continue-incomplete operation, whose
    /// session-store transaction wrote `task_created` and `plan_resumed` in one
    /// batch; the ordinary turn path must not write a second resume edge.
    #[serde(default)]
    pub plan_already_carried: bool,
}

impl ChatTurnOptions {
    /// An ordinary desktop session: a person is here and a question can also
    /// be answered after the turn pauses.
    pub fn interactive() -> Self {
        Self {
            synchronous_user_present: true,
            question_route_available: true,
            ..Self::default()
        }
    }

    /// An app-managed worker: no one is present synchronously, but a question
    /// becomes a durable pause the worker's own session can answer.
    pub fn worker() -> Self {
        Self {
            question_route_available: true,
            ..Self::default()
        }
    }

    /// Whether `ask_user` may be offered. An old persisted `user_present=true`
    /// grants the route through [`Self::synchronous_user_present`], so a
    /// pending turn written before spec 104 keeps its capability.
    pub fn question_route(&self) -> bool {
        self.question_route_available || self.synchronous_user_present
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResumeDecisionOptions {
    pub allow_browser_diagnostics_for_session: bool,
}

/// What `dispatch_fetch_url` concluded (spec 51 §5.3).
///
/// A host with no approval is not a failure: it is a pause. Keeping it as a
/// separate case is what makes "no request before the dialog" structural —
/// the request path is only reachable through `Result`.
pub(super) enum FetchDispatch {
    Result(String, String, ActionOutcome),
    NeedsApproval(Inspection),
}

/// One recalled memory resolved for the context inspector (spec 30 §5.4, C3).
///
/// The manifest carries only the entry id, match reason and recall-time
/// verification because it is written to the audit log. This is the display-time
/// read that turns a row back into the fields a user can inspect. The statement
/// is `None` when the id no longer resolves in a scope this project and session
/// may read, or when the entry has been deleted, so a deleted entry reads as
/// "deleted" without leaving its text in the panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDisplayEntry {
    pub entry_id: String,
    pub statement: Option<String>,
    pub scope: Option<MemoryScope>,
    pub category: Option<MemoryCategory>,
    pub provenance: Option<MemoryProvenance>,
    pub created_at_ms: Option<u128>,
    pub verification: Verification,
    pub match_reason: MemoryMatchReason,
    /// Whether the secret scanner removed a detected credential from
    /// `statement`. The stored record is not rewritten (spec 30 C13.2).
    pub redacted: bool,
    /// True for an id that no longer resolves here, including one owned by
    /// another project or a `Deleted` entry.
    pub deleted: bool,
}
