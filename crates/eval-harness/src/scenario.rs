use std::path::{Path, PathBuf};

use serde::Deserialize;
use workspace_engine::{ClientError, Result};

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
    pub context_ranks_within: Option<(String, u64)>,
    pub refused: Option<bool>,
    pub absent_everywhere: Option<String>,
    pub command_executed: Option<bool>,
    pub patch_applied: Option<bool>,
    pub model_calls_at_most: Option<u64>,
    /// Assertion names that depend on a scripted tool call and are therefore
    /// skipped in the live tier (proposal §5.3), so a scenario stays one
    /// definition rather than two that drift.
    #[serde(default)]
    pub deterministic_only: Vec<String>,
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
