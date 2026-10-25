use std::path::{Path, PathBuf};

use serde::Deserialize;
use workspace_engine::{ClientError, ConfigOverlay, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Deterministic,
    Live,
}

impl Tier {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "deterministic" => Ok(Self::Deterministic),
            "live" => Ok(Self::Live),
            other => Err(ClientError::InvalidInput(format!(
                "unknown tier {other}: expected `deterministic` or `live`"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::Live => "live",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScenarioToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct Turn {
    pub content: String,
    pub tool_calls: Vec<ScenarioToolCall>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct Asserts {
    pub patch_touches: Option<Vec<String>>,
    pub approval_required: Option<bool>,
    pub files_changed_outside_patch: Option<u64>,
    pub file_references_resolve: Option<bool>,
    pub context_contains: Option<Vec<String>>,
    pub context_excludes: Option<Vec<String>>,
    /// That the final answer names each of these strings. Unlike
    /// `context_contains`, this reads the response rather than the retrieved
    /// context, so a live-tier run checks the model's own wording. Each entry
    /// is matched as a substring.
    pub response_contains: Option<Vec<String>>,
    pub context_ranks_within: Option<(String, u64)>,
    pub refused: Option<bool>,
    pub absent_everywhere: Option<String>,
    pub command_executed: Option<bool>,
    pub patch_applied: Option<bool>,
    pub model_calls_at_most: Option<u64>,
    /// How many round-budget continuations the run granted (spec 61 §5.4).
    /// Zero is a real answer — a turn answered with *Answer now* never
    /// continues — so this is an exact count, not an upper bound.
    pub continuations: Option<u64>,
    /// How many tasks the run's store holds. A continued turn is the same task,
    /// so this reads 1; a turn that started over reads more.
    pub task_count: Option<u64>,
    /// The task's terminal state, as `TaskStatus::as_str`, rather than
    /// `finalStatus`. A paused turn's run is `completed` while its task still
    /// waits for a human, so the two answer different questions.
    pub final_task_status: Option<String>,
    /// Strings the first model request after a round-budget continuation must
    /// contain. This is the §5.8 claim that the resumed turn replayed the
    /// user's request and the earlier tool results, checked against the request
    /// itself rather than the log it was rebuilt from. Script-derived, so it is
    /// `deterministic_only`.
    pub continuation_request_contains: Option<Vec<String>>,
    /// Strings the first model request after a scripted answer must contain.
    /// This is the spec 50 §9 claim that the answer reached the model as a tool
    /// result rather than being recorded and dropped. Script-derived, so it is
    /// `deterministic_only`.
    pub answer_request_contains: Option<Vec<String>>,
    /// That the run dispatched at least this many tool calls, which proves a
    /// batch ran every call the model made in one message. Script-derived, so a
    /// scenario using it lists it in `deterministic_only`.
    pub tool_calls_at_least: Option<u64>,
    /// That the run used at most this many tool rounds. A batch of N calls in
    /// one message is one round, not N, so this is the other half of the
    /// batching claim. Script-derived, like `tool_calls_at_least`.
    pub tool_rounds_at_most: Option<u64>,
    /// How many of the run's tool rounds were exploration rounds — every
    /// dispatched call a read (spec 59 §5.6). Read from the session log, so it
    /// is a measurement of the run; an exact count is script-dependent in the
    /// deterministic tier and therefore `deterministic_only`.
    pub exploration_rounds: Option<u64>,
    /// What the classifier must conclude about the task left mid-action by
    /// `crash_mid_action`, as its `TaskStatus` string.
    pub recovered_classification: Option<String>,
    /// That the interrupted action is not repeated: neither automatically, nor
    /// when a resume is asked for outright.
    pub auto_retry_refused: Option<bool>,
    /// How many findings the run's exploration commands recorded (spec 63
    /// §5.5), which should be zero. Fails as well when the run executed no
    /// exploration command, since a zero over nothing measured nothing.
    pub exploration_findings: Option<u64>,
    /// The verification outcome the run must report (spec 23 Task 9).
    pub verification_outcome: Option<String>,
    /// A phrase the final report's summary must not contain. The negative
    /// scenario uses `"complete"`, which is the claim an optimistic loop would
    /// make.
    pub verification_summary_excludes: Option<String>,
    /// The exact commands the report lists as passed. The point is that none of
    /// them was merely skipped or stale.
    pub verification_passed_commands: Option<Vec<String>>,
    /// Assertion names that depend on a scripted tool call and are therefore
    /// skipped in the live tier (proposal §5.3), so a scenario stays one
    /// definition rather than two that drift.
    #[serde(default)]
    pub deterministic_only: Vec<String>,
}

/// How a scenario answers a round-budget pause (spec 61 §5.8).
///
/// Kept separate from the engine's `RoundBudgetDecision` because this is the
/// scenario file's wire vocabulary (`continue` / `answer_now`), and the loader
/// is the boundary where a file becomes a validated run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundBudgetDecision {
    Continue,
    AnswerNow,
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: String,
    pub fixture: String,
    pub tier: Tier,
    pub prompt: String,
    pub provider: Option<String>,
    pub blocked_on: Option<String>,
    /// A path and replacement content the runner writes *after* the patch is
    /// proposed and before it is applied. The only way to create the `base_hash`
    /// conflict §5.4 asserts, since the conflict is defined by the file changing
    /// between preview and apply.
    pub modify_after_proposal: Option<(String, String)>,
    /// When set, the runner resumes the turn with this approval decision after
    /// it stops on a command proposal, so the approved and denied paths are
    /// exercised end to end rather than stopping at the proposal.
    pub approval_decision: Option<bool>,
    /// When set, the runner answers a round-budget pause with this decision and
    /// keeps driving, so a continued turn's state and a forced final call are
    /// exercised end to end (spec 61 §5.8).
    pub round_budget_decision: Option<RoundBudgetDecision>,
    /// The answers the user gives, in order, when the turn asks a question
    /// (spec 50 §9). Non-empty makes the run interactive — `chat_orchestrator`
    /// is told a user is present, so `ask_user` is offered — and the runner
    /// resumes each question through `resume_after_answer`. Empty is the
    /// headless default every other scenario keeps.
    pub answers: Vec<String>,
    /// Config this run needs beyond the defaults, in the same `key=value` form
    /// as `user.conf` (spec 59 §5.7's zero-allowance case is one). Parsed at the
    /// boundary so a malformed line is a load error, not a silently ignored
    /// setting. Applied at user scope: the harness is the user here.
    pub config_overlay: Option<ConfigOverlay>,
    /// When set, the runner applies the proposed patch and drives the
    /// verification continuation to a report, including any check card and
    /// repair turns (spec 23 Task 9).
    pub verify_after_apply: bool,
    /// How each check on a verification card is answered, matched by command.
    /// A check the list omits is declined, which is the card's default.
    pub check_decisions: Vec<CheckDecision>,
    /// When set, the runner leaves the turn's task mid-action — an
    /// `action_started` with no `action_finished` — then reopens the store and
    /// classifies, which is what a restart does. See [`CrashMidAction`].
    pub crash_mid_action: Option<CrashMidAction>,
    pub turns: Vec<Turn>,
    pub asserts: Asserts,
}

// The wire shape. Kept separate from `Scenario` so tier/provider validation
// happens once, at the boundary, and the rest of the harness cannot hold an
// unvalidated scenario.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawScenario {
    name: String,
    fixture: String,
    tier: String,
    prompt: String,
    provider: Option<String>,
    blocked_on: Option<String>,
    #[serde(default)]
    modify_after_proposal: Option<ModifyAfterProposal>,
    #[serde(default)]
    approval_decision: Option<bool>,
    #[serde(default)]
    round_budget_decision: Option<RoundBudgetDecision>,
    /// Scripted answers, in the order the questions are answered. An empty list
    /// leaves the run headless.
    #[serde(default)]
    answers: Vec<String>,
    /// The config as the same `key=value` lines `user.conf` uses. A string
    /// rather than a table because the engine's parser is the one authority on
    /// what the keys are and what values they accept.
    #[serde(default)]
    config_overlay: Option<String>,
    #[serde(default)]
    verify_after_apply: bool,
    #[serde(default)]
    check_decisions: Vec<CheckDecision>,
    #[serde(default)]
    crash_mid_action: Option<CrashMidAction>,
    #[serde(default, rename = "turn")]
    turns: Vec<RawTurn>,
    #[serde(default, rename = "assert")]
    asserts: Asserts,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct ModifyAfterProposal {
    path: String,
    content: String,
}

/// The crash to leave behind, for the scenario that measures recovery.
///
/// **What is simulated and what is real.** The process is not killed — the
/// runner writes the *signature* a kill leaves: the task moved to
/// `running_tool` and an `action_started` with no `action_finished`. That the
/// signature is genuinely what survives a real `SIGKILL` is proven separately,
/// by spec 17's `#[ignore]`d
/// `a_real_sigkill_mid_action_leaves_a_readable_log_and_an_unknown_outcome`.
/// This scenario's job is the other half: that the engine, restarted, refuses
/// to repeat the action.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct CrashMidAction {
    /// The action name, as the dispatch layer records it (`run_command`).
    pub action: String,
    /// What the action was operating on, for the evidence the classifier keeps.
    pub reference: String,
    /// Whether the interrupted action could be observed outside this process.
    /// `true` is what makes the outcome unknowable and the retry unsafe.
    pub side_effecting: bool,
}

/// One check card answer (spec 23 Task 9), matched by command so a scenario
/// does not have to know a proposal id the engine creates at run time.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct CheckDecision {
    pub command: String,
    pub approved: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawTurn {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<RawToolCall>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawToolCall {
    name: String,
    // `toml::Value` has no `Default`, so the usual `#[serde(default)]` will not
    // compile here; an empty table is the right default for a no-argument call.
    #[serde(default = "empty_arguments")]
    arguments: toml::Value,
}

fn empty_arguments() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

pub fn scenarios_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios")
}

pub fn load(path: &Path) -> Result<Scenario> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| ClientError::Io(format!("{}: {error}", path.display())))?;
    let raw: RawScenario = toml::from_str(&text)
        .map_err(|error| ClientError::InvalidInput(format!("{}: {error}", path.display())))?;

    let tier = Tier::parse(&raw.tier)?;
    if tier == Tier::Deterministic
        && let Some(provider) = raw.provider.as_deref()
        && provider != "mock"
    {
        return Err(ClientError::InvalidInput(format!(
            "{}: a deterministic scenario cannot name the real provider {provider}; the \
             deterministic tier must not reach the network",
            path.display()
        )));
    }

    let mut turns = Vec::with_capacity(raw.turns.len());
    for turn in raw.turns {
        let mut tool_calls = Vec::with_capacity(turn.tool_calls.len());
        for call in turn.tool_calls {
            // Round-tripped through JSON because that is what the orchestrator
            // decodes: `ToolCall::arguments_json` is a JSON string.
            let arguments = serde_json::to_value(&call.arguments).map_err(|error| {
                ClientError::InvalidInput(format!(
                    "{}: tool call `{}` has arguments that are not representable as JSON: {error}",
                    path.display(),
                    call.name
                ))
            })?;
            tool_calls.push(ScenarioToolCall {
                name: call.name,
                arguments,
            });
        }
        turns.push(Turn {
            content: turn.content,
            tool_calls,
            truncated: turn.truncated,
        });
    }

    Ok(Scenario {
        name: raw.name,
        fixture: raw.fixture,
        tier,
        prompt: raw.prompt,
        provider: raw.provider,
        blocked_on: raw.blocked_on,
        modify_after_proposal: raw
            .modify_after_proposal
            .map(|change| (change.path, change.content)),
        approval_decision: raw.approval_decision,
        round_budget_decision: raw.round_budget_decision,
        answers: raw.answers,
        config_overlay: match raw.config_overlay {
            Some(text) => Some(ConfigOverlay::parse(&text).map_err(|error| {
                ClientError::InvalidInput(format!("{}: config_overlay: {error}", path.display()))
            })?),
            None => None,
        },
        verify_after_apply: raw.verify_after_apply,
        check_decisions: raw.check_decisions,
        crash_mid_action: raw.crash_mid_action,
        turns,
        asserts: raw.asserts,
    })
}

pub fn load_all() -> Result<Vec<Scenario>> {
    let dir = scenarios_dir();
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|error| ClientError::Io(format!("{}: {error}", dir.display())))?
    {
        let path = entry
            .map_err(|error| ClientError::Io(format!("{}: {error}", dir.display())))?
            .path();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            paths.push(path);
        }
    }
    paths.sort();
    paths.iter().map(|path| load(path)).collect()
}
