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

/// The placeholder a scenario writes where the stub's base URL belongs.
///
/// The server binds `127.0.0.1:0`, so the port does not exist until the run
/// starts and cannot be written into a scenario file. The runner substitutes
/// this in the prompt and in string tool-call arguments.
pub const STUB_BASE_PLACEHOLDER: &str = "{stub_base}";

/// One page the harness serves from a loopback stub (spec 51 Task 9).
///
/// A page is matched by exact path — the query string is ignored — so a
/// scenario can count requests to a collection endpoint that must never be
/// reached. A path no page names answers `404`, which makes a mistyped
/// scenario fail as a page that was fetched rather than silently as a
/// non-fetch.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct StubPage {
    /// Request path including the leading slash, e.g. `/docs/install`.
    pub path: String,
    /// HTTP status; 200 unless the scenario needs a redirect or an error.
    #[serde(default = "default_stub_status")]
    pub status: u16,
    #[serde(default = "default_stub_content_type")]
    pub content_type: String,
    #[serde(default)]
    pub body: String,
    /// A `location` header for a 3xx page. The tool never follows it; the page
    /// exists so a redirect can be measured without the internet.
    #[serde(default)]
    pub location: Option<String>,
}

fn default_stub_status() -> u16 {
    200
}

fn default_stub_content_type() -> String {
    "text/plain; charset=utf-8".to_string()
}

/// One message written into the session before a scenario's first ask.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct SeedMessage {
    pub role: String,
    pub content: String,
}

/// One file written into the fixture after its baseline commit. Not committed,
/// so it exercises the ignore rule. It does **not** prove index residency: the
/// index is built lazily after seed files are written. The direct store test
/// `ignored_evidence_is_refused_after_the_file_was_indexed` owns that case.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct SeedFile {
    pub path: String,
    pub content: String,
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
    /// Exactly how many `turn_notes` calls the run dispatched (spec 65 §5.9).
    pub notes_calls: Option<u64>,
    /// That at least this many notes were carried from an earlier turn into a
    /// later one (spec 65 §5.9). A count, not a boolean, so a run that carried
    /// twice still passes.
    pub carried_notes_at_least: Option<u64>,
    /// That no read repeated an earlier read in the run (spec 65 §5.9). Read
    /// from the log, where a repeat anywhere counts; the deterministic scenario
    /// expects zero.
    pub repeated_elided_reads: Option<u64>,
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
    /// How many plan reminders the run sent (spec 66 §5.5). A scenario that
    /// asserts this without a plan fails rather than comparing `None`.
    pub plan_reminders: Option<u64>,
    /// The final plan's verified / unverified / outstanding counts (spec 66
    /// §5.5). Each reads `RecordedPlan`, which exists only when the turn made a
    /// plan, so a planless run fails the assertion instead of reading a
    /// vacuous zero.
    pub plan_verified: Option<u64>,
    pub plan_unverified: Option<u64>,
    pub plan_outstanding: Option<u64>,
    /// Whether the engine recorded `plan_left_open` for the run.
    pub plan_left_open: Option<bool>,
    /// Evidence items on the steps still open in the final plan.
    pub plan_open_evidence: Option<u64>,
    /// Strings the task's last assistant message must contain, read from the
    /// session log. Script-derived, so it is `deterministic_only`.
    pub last_assistant_message_contains: Option<Vec<String>>,
    /// Assertion names that depend on a scripted tool call and are therefore
    /// skipped in the live tier (proposal §5.3), so a scenario stays one
    /// definition rather than two that drift.
    /// Strings the first model request after a compaction must contain. Read
    /// from the mock adapter's recorded request, so it measures what was sent
    /// rather than what the script said. Script-derived: `deterministic_only`.
    pub compacted_request_contains: Option<Vec<String>>,
    /// Strings that request must not contain.
    pub compacted_request_excludes: Option<Vec<String>>,
    /// Strings the *system* message of that request must not contain. This is
    /// the half that proves a summary never becomes an instruction from
    /// Damaian rather than merely being present somewhere. `deterministic_only`.
    pub compacted_system_excludes: Option<Vec<String>>,
    /// Strings the final answer must not contain (requirement 7's live
    /// behavioural half). Reads the response, not a request.
    pub response_excludes: Option<Vec<String>>,
    /// How many `conversation_compacted` events the run appended.
    pub conversation_compactions: Option<u64>,
    /// Whether the newest summary is facts-only (the model half failed or was
    /// unusable). `Some(false)` means a narrative was recorded.
    pub compaction_facts_only: Option<bool>,
    /// Spec 29 §5.8: how many stored memory entries the run left behind. The
    /// harness never clicks Confirm, so an injection or refusal scenario
    /// expects zero, and the value is read from the memory files rather than
    /// inferred from the model's answer.
    pub memory_entries: Option<u64>,
    /// How many proposals are still pending in the run's session logs.
    pub memory_proposals: Option<u64>,
    /// How many entries are in the user-scope file. User scope is off by
    /// default, so this is zero unless a scenario enables it.
    pub memory_user_entries: Option<u64>,
    /// Strings that must not appear in any memory record or pending-proposal
    /// event. The refusal audit is excluded on purpose: a refused path may be
    /// named there, as §5.5a says.
    pub memory_absent: Option<Vec<String>>,
    /// Strings the audit log must contain, such as a refusal reason.
    pub memory_audit_contains: Option<Vec<String>>,
    /// How many HTTP requests the scenario's loopback page server received
    /// (spec 51 Task 9). An exact count, not an upper bound: the claim a fetch
    /// scenario makes is "nothing left the machine", and zero is the answer
    /// that matters for a refused URL.
    pub fetch_requests: Option<u64>,
    /// How many `web_fetch` audit events the run recorded. One per successful
    /// fetch, never for a refused URL.
    pub fetch_audits: Option<u64>,
    /// How many `web_fetch_approval_decision` audit events the run recorded.
    /// The committed fetch scenarios pre-approve their stub host, so this is
    /// zero; a non-zero value means the run paused on an approval the harness
    /// cannot answer.
    pub fetch_approval_decisions: Option<u64>,
    /// Strings the run's tool results must contain. Read from the session log,
    /// because that is what the model was actually given; the run record keeps
    /// only a marker reference.
    pub tool_results_contain: Option<Vec<String>>,
    /// Strings no tool result may contain — a seeded credential that would
    /// otherwise reach the model unredacted.
    pub tool_results_exclude: Option<Vec<String>>,
    /// Strings that must appear in a tool result **only** on a line beginning
    /// with `> `. This is the "fetched content is data" claim: the page's own
    /// words reach the model inside the reference frame, never as bare text.
    pub tool_results_quoted: Option<Vec<String>>,
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
    /// That someone can answer a mid-turn question even though the scenario
    /// scripts no answer. `fetch_url` is offered on the same condition as
    /// `ask_user` [spec 51 §5.3], and a fetch scenario pre-approves its stub
    /// host so it never actually pauses; this is how it says a user is there
    /// without inventing an answer nothing will consume.
    pub user_present: bool,
    /// Pages served from a loopback stub for this run (spec 51 Task 9).
    pub stub_pages: Vec<StubPage>,
    /// A second user turn in the same session, run after the first finishes
    /// (spec 65 §5.9). Its scripted responses are the rest of `turns`, so one
    /// scenario can measure what a later turn inherits. `None` is one turn, as
    /// every scenario before this one had.
    pub follow_up_prompt: Option<String>,
    /// Messages written before the first ask (spec 55 §5.9).
    pub seed_conversation: Vec<SeedMessage>,
    /// Files written after the fixture's baseline commit, without committing
    /// them (spec 29 §5.8). This makes a committed file ignored; it does not
    /// prove the file is still index-resident, because the index is built after
    /// these writes.
    pub seed_files: Vec<SeedFile>,
    /// Whether to compact the seeded conversation before the ask.
    pub compact_before_prompt: bool,
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
    /// That a user is present without scripting an answer (spec 51 Task 9).
    #[serde(default)]
    user_present: bool,
    /// Pages served from a loopback stub (spec 51 Task 9).
    #[serde(default, rename = "stub_page")]
    stub_page: Vec<StubPage>,
    /// A second user turn in the same session (spec 65 §5.9). The scripted
    /// responses continue from where the first turn stopped.
    #[serde(default)]
    follow_up_prompt: Option<String>,
    /// Messages written before the first ask, so a scenario can start with a
    /// conversation long enough to compact (spec 55 §5.9).
    #[serde(default)]
    seed_conversation: Vec<SeedMessage>,
    /// Files written after the baseline commit, never committed.
    #[serde(default)]
    seed_files: Vec<SeedFile>,
    /// Compact the seeded conversation before the first ask, through the same
    /// public path the manual control uses. The compaction consumes the first
    /// scripted `[[turn]]`, which is therefore its narrative reply.
    #[serde(default)]
    compact_before_prompt: bool,
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

    let stub_pages = raw.stub_page;
    for page in &stub_pages {
        if !page.path.starts_with('/') {
            return Err(ClientError::InvalidInput(format!(
                "{}: stub page path {:?} must start with `/`",
                path.display(),
                page.path
            )));
        }
    }
    // A placeholder with nothing to substitute would reach the model as a
    // literal URL and fail as a parse error, which reads like an engine defect
    // rather than a scenario mistake. Refuse it at the boundary instead.
    if stub_pages.is_empty() {
        let uses_placeholder = raw.prompt.contains(STUB_BASE_PLACEHOLDER)
            || raw
                .follow_up_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains(STUB_BASE_PLACEHOLDER))
            || raw
                .seed_conversation
                .iter()
                .any(|message| message.content.contains(STUB_BASE_PLACEHOLDER))
            || turns.iter().any(|turn| {
                turn.tool_calls
                    .iter()
                    .any(|call| value_contains_placeholder(&call.arguments))
            });
        if uses_placeholder {
            return Err(ClientError::InvalidInput(format!(
                "{}: {STUB_BASE_PLACEHOLDER} is used but the scenario declares no [[stub_page]]",
                path.display()
            )));
        }
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
        user_present: raw.user_present,
        stub_pages,
        follow_up_prompt: raw.follow_up_prompt,
        seed_conversation: raw.seed_conversation,
        seed_files: raw.seed_files,
        compact_before_prompt: raw.compact_before_prompt,
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

fn value_contains_placeholder(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => text.contains(STUB_BASE_PLACEHOLDER),
        serde_json::Value::Array(items) => items.iter().any(value_contains_placeholder),
        serde_json::Value::Object(map) => map.values().any(value_contains_placeholder),
        _ => false,
    }
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
