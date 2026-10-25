use crate::clarification::MAX_QUESTIONS_PER_TURN_CEILING;
use crate::context_manager::{ContextBudget, ContextCategory};
use crate::error::{ClientError, Result};
use crate::hash::repository_id_for_root;
use crate::model::TokenUsage;
use crate::profile::{ProfileCapabilities, ProfileId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Repository-context budget used when neither the provider config nor the
/// built-in per-model table specifies one. Matches the value that was
/// previously hardcoded at the two `build_context` call sites, so a config
/// that says nothing behaves exactly as before.
pub const DEFAULT_CONTEXT_TOKEN_BUDGET: u32 = 16_000;

pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    ".git/",
    ".gitignore",
    "node_modules/",
    "vendor/",
    ".venv/",
    "venv/",
    "dist/",
    "build/",
    "target/",
    "coverage/",
    ".damaian/",
    "*.min.js",
    "*.map",
];

pub const DEFAULT_RESTRICTED_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "**/.env",
    "**/.env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    // Both the root-level and the nested form of each directory. `**/` requires
    // at least one leading path segment, so `**/secrets/**` alone matches
    // `nested/secrets/token` but *not* a top-level `secrets/token` — which is
    // the more common layout. The `.env` entries above already pair a bare
    // pattern with a `**/` one for the same reason; these two were missing their
    // bare form, leaving a root `secrets/` or `credentials/` directory
    // unprotected. Found by the spec 18 eval harness's `restricted_path`
    // scenario, which now guards it.
    "secrets/**",
    "credentials/**",
    "**/secrets/**",
    "**/credentials/**",
];

/// Where a [`ConfigOverlay`] came from. [`ConfigScope::Repository`] is
/// untrusted: that file arrives with a clone, so it may add restrictions but
/// never remove one. See `docs/specs/34_repository_config_trust_boundary.md`.
/// [`ConfigScope::Profile`] is restrict-only too, by design rather than
/// distrust (spec 31, `context.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    User,
    /// Untrusted: arrives with a clone.
    Repository,
    Admin,
    /// The permission profile selected for this checkout, applied last. It
    /// narrows capability exactly as far as repository config may, and sets
    /// no preference or redirecting key.
    Profile,
}

/// Why a repository-sourced key was not honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryKeyClass {
    /// No legitimate repository use case: redirects execution, model traffic,
    /// credentials, or data location.
    Forbidden,
    /// Honoured only in the more restrictive direction; this value would have
    /// loosened the user's policy.
    RestrictOnly,
    /// The user's own decision about this repository, stored in user config.
    UserOwned,
    /// Damaian could not parse the key or its value, so the line was skipped.
    /// Only repository scope produces this: user and admin config are the
    /// user's own files, where a broken key is a mistake to report loudly
    /// rather than an untrusted input to tolerate.
    Unparsable,
}

impl RepositoryKeyClass {
    pub fn as_str(self) -> &'static str {
        match self {
            RepositoryKeyClass::Forbidden => "forbidden",
            RepositoryKeyClass::RestrictOnly => "restrict_only",
            RepositoryKeyClass::UserOwned => "user_owned",
            RepositoryKeyClass::Unparsable => "unparsable",
        }
    }
}

/// A repository-sourced key that was ignored. Carries the key name and why —
/// never the value, which is attacker-controlled text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedConfigKey {
    pub key: String,
    pub class: RepositoryKeyClass,
}

impl RejectedConfigKey {
    fn new(key: impl Into<String>, class: RepositoryKeyClass) -> Self {
        Self {
            key: key.into(),
            class,
        }
    }
}

/// A key one overlay applied, recorded where it was applied so the effective
/// policy can name its source without a second resolver (spec 31,
/// `context.md` §8). Only what was taken is recorded: a refused value never
/// appears here, and a list key carries only the entries this scope put or
/// kept in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedKey {
    /// The key as `to_policy_text` names it: `mcp_server.<id>.<field>` and
    /// `model_provider.<id>.<field>` per field.
    pub key: String,
    pub scope: ConfigScope,
    /// For a list key, the entries this scope contributed. `None` for a
    /// scalar.
    pub entries: Option<Vec<String>>,
    /// Admin config loosened what user and repository config had resolved,
    /// by the direction the restrict-only merge refuses at untrusted scopes.
    pub widened: bool,
}

/// What [`Config::apply_overlay_scoped`] did with one overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayOutcome {
    /// Keys the scope may not set, or set only in the other direction.
    pub rejected: Vec<RejectedConfigKey>,
    /// Keys applied, in the order they were applied.
    pub applied: Vec<AppliedKey>,
}

impl OverlayOutcome {
    fn record(
        &mut self,
        scope: ConfigScope,
        key: impl Into<String>,
        entries: Option<Vec<String>>,
        loosened: bool,
    ) {
        self.applied.push(AppliedKey {
            key: key.into(),
            scope,
            entries,
            // User config loosening a default is the user's own choice; only
            // admin config overruling what earlier scopes resolved is flagged.
            widened: loosened && scope == ConfigScope::Admin,
        });
    }
}

/// What repository scope attempted, for the audit trail and the one-time
/// notice. Produced by [`Config::load_for_repository_reporting`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepositoryConfigReport {
    /// The repository whose config produced this report, when one was loaded.
    pub repository_root: Option<PathBuf>,
    /// Keys the repository config set that were not honoured.
    pub rejected_keys: Vec<RejectedConfigKey>,
    /// `command_allowlist` entries found in repository config. Never applied;
    /// carried so the user can be offered the one-time migration.
    pub repository_allowlist_entries: Vec<String>,
    /// The permission profile applied last, when this checkout has one
    /// selected (spec 31).
    pub permission_profile: Option<ProfileId>,
    /// Keys the selected profile carried and could not apply. Deliberately
    /// not in [`Self::rejected_keys`], nor in [`Self::is_empty`]: they are not
    /// the repository's refusals and must not reach its notice.
    pub profile_rejected_keys: Vec<RejectedConfigKey>,
    /// Every key applied by user, repository, admin and profile config, in
    /// that order (spec 31 Task 6). A key with no entry has its default.
    pub applied: Vec<AppliedKey>,
    /// `command_allowlist` entries the `Allow Always` fold added, applied
    /// after every overlay.
    pub allow_always_entries: Vec<String>,
}

impl RepositoryConfigReport {
    pub fn is_empty(&self) -> bool {
        self.rejected_keys.is_empty() && self.repository_allowlist_entries.is_empty()
    }

    /// The rejected keys worth telling the user about: everything a repository
    /// asked for and did not get.
    pub fn rejected_key_names(&self) -> Vec<&str> {
        self.rejected_keys
            .iter()
            .map(|rejected| rejected.key.as_str())
            .collect()
    }
}

// `PartialEq` so spec 31 can pin "no profile selected changes nothing" as one
// comparison. No `Eq`: provider price rates are `Option<f64>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub data_dir: PathBuf,
    pub max_file_bytes: u64,
    /// Lines an unranged `read_file` returns before it truncates and says so.
    ///
    /// Spec 47 §5.5. Restrict-only: a repository may lower it, never raise it,
    /// because raising it pulls more of the user's files into a model request
    /// than the user chose to allow.
    pub max_read_lines: usize,
    /// Paths `list_directory` returns before it truncates and says so.
    /// Restrict-only, same reasoning as `max_read_lines`.
    pub max_list_entries: usize,
    /// Matches `search_content` returns before it truncates and says so.
    pub max_search_matches: usize,
    /// Characters of a matching line returned before it is trimmed, so one
    /// minified line cannot fill the model's context on its own.
    pub max_match_line_chars: usize,
    /// Tokens the repository map may take in model context (spec 24 §5.4).
    /// `0` leaves the map out. Restrict-only, same reasoning as
    /// `max_read_lines`: it bounds how much of the repository's layout
    /// leaves the machine per turn (spec 24 `context.md` §7).
    pub repository_map_max_tokens: usize,
    /// Tokens the `symbols` context item may take (spec 25 `context.md`
    /// §9). `0` leaves the item out. Restrict-only like
    /// `repository_map_max_tokens`: it bounds how much of the repository's
    /// declarations leaves the machine per turn.
    pub symbol_context_max_tokens: usize,
    /// Per-category shares of the context limit and the observed-window safety
    /// margin (spec 26 §5.2, §5.7). Restrict-only: a repository may lower a
    /// share and raise the margin, never the reverse.
    pub context_budget: ContextBudget,
    pub max_command_output_bytes: usize,
    /// Seconds a running command may run before its process group is killed
    /// and it is reported as timed out.
    ///
    /// Restrict-only, on the same reasoning as the token ceiling: a repository
    /// may shorten how long its own commands run, but may not lengthen one past
    /// what the user chose. Spec 47 requirement 5.
    pub command_timeout_secs: usize,
    pub allowed_roots: Vec<PathBuf>,
    pub ignore_patterns: Vec<String>,
    pub restricted_patterns: Vec<String>,
    pub command_allowlist: Vec<String>,
    /// `Allow Always` grants, keyed by repository id. The user's own decision
    /// about one repository, so it lives in user config rather than in the
    /// repository — a repository cannot grant itself an allowlist entry. A
    /// repository-scoped load folds the matching entries into
    /// [`Self::command_allowlist`].
    pub command_allowlist_by_repository: BTreeMap<String, Vec<String>>,
    /// The permission profile the user selected for each checkout, keyed by
    /// repository id the way `Allow Always` is. User-owned: a repository
    /// cannot choose its own profile in either direction (spec 31,
    /// `context.md` §4).
    pub permission_profile_by_repository: BTreeMap<String, ProfileId>,
    pub command_blocklist: Vec<String>,
    pub secret_patterns: Vec<String>,
    pub require_approval_for_file_edits: bool,
    pub require_approval_for_risky_commands: bool,
    pub require_approval_for_all_commands: bool,
    /// Profile capability keys (spec 31, `context.md` §3). Each defaults to
    /// today's behaviour, so a user who sets none sees no change, and each
    /// can only be turned down by repository config.
    pub allow_file_edits: bool,
    pub command_access: CommandAccess,
    pub allow_browser_diagnostics: bool,
    pub allow_mutating_mcp_tools: bool,
    pub block_generated_secrets: bool,
    pub audit_enabled: bool,
    pub audit_retention_days: u64,
    /// How long a session checkpoint is kept. Defaults to the same window as
    /// the audit log, since both are local recovery data with the same
    /// sensitivity.
    pub checkpoint_retention_days: u64,
    /// Ceiling on the checkpoint store for one repository. Cleanup drops the
    /// oldest checkpoints until the store is back under it, so a long-running
    /// repository cannot fill the disk.
    pub checkpoint_max_total_bytes: u64,
    /// How many changed or untracked paths a working-tree census may cover
    /// before command effects are reported as uncovered for the turn. A
    /// checkpoint that claims coverage it does not have is worse than one that
    /// admits the gap — and a census that takes ten seconds reads as a hang.
    ///
    /// The default is set from a measurement, not a guess: ~2.7 ms per changed
    /// path (hashing the file and writing its blob), and the census runs twice
    /// per approved command, so 1000 paths caps it at roughly five seconds in
    /// the worst case. See `docs/specs/16_session_checkpoints_and_rewind.md` §7.
    pub checkpoint_census_max_paths: u64,
    /// Off by default: enabling local semantic search downloads a small
    /// embedding model on first use (a one-time network fetch), which the
    /// user should opt into rather than have happen implicitly.
    pub enable_semantic_search: bool,
    /// Off by default (spec 57 §5.0). On, every session that is not itself a
    /// worker is told how to write a `worker-prompt` handoff block. Off, the
    /// system prompt is byte-for-byte what it was before the setting existed.
    pub enable_worker_sessions: bool,
    /// On by default: the filesystem watcher in `index_cache.rs` is what keeps
    /// a cached index fresh between the periodic full rescans.
    ///
    /// Callers that build many short-lived repositories in one process — the
    /// test suites and the eval harness — turn it off. Registering an FSEvents
    /// stream costs seconds of waiting on `fseventsd` per repository, and those
    /// callers index a repository once and throw it away, so a watcher is pure
    /// cost to them. Leaving it on is right for the app, where one repository
    /// is watched for the length of a session.
    pub enable_index_watcher: bool,
    /// Default number of tool/model rounds an agentic chat turn may spend
    /// before the model is asked to answer with the evidence it has.
    pub agent_max_tool_rounds: u32,
    /// Read-only rounds one turn may spend before they begin to count against
    /// [`Self::agent_max_tool_rounds`] (spec 59). Restrict-only, lower wins: a
    /// repository may lower it, never raise it, because raising it spends the
    /// user's money on the repository's say-so. `0` restores today's behaviour,
    /// where every read counts as an action round.
    pub agent_max_exploration_rounds: u32,
    /// Larger budget for turns that are explicitly debugging a web page or
    /// that call a first-class browser diagnostic tool.
    pub agent_web_debug_max_tool_rounds: u32,
    /// How many substantially identical failed tool calls may be retried before
    /// the model is told to change approach.
    pub agent_tool_retry_limit: u32,
    /// How many `ask_user` questions one turn may ask (spec 50). `0` means the
    /// tool is not offered at all, rather than offered and always refused.
    pub agent_max_questions_per_turn: u32,
    /// Total tokens one task may spend before it is stopped at a step boundary.
    ///
    /// `None` means no ceiling, which is the default so that upgrading cannot
    /// break an existing configuration. Spec 21 §5.4.
    ///
    /// **Per task, and a task is one turn** (spec 21 `context.md` §3.1), so a
    /// session of five turns under a 100k ceiling can spend 500k. §4 rules out
    /// per-session budgets deliberately; the user guide has to say so plainly
    /// rather than let a reader assume otherwise.
    pub agent_max_task_tokens: Option<u64>,
    /// Most messages a model request may carry inside one turn. The within-turn
    /// array grows by an assistant message and a tool result per round, and a
    /// tool result can be most of a file, so a long turn would otherwise send a
    /// request that grows without bound — the half of compaction spec 47
    /// requirement 6 owns (`OBSERVATIONS.md` entry 6).
    ///
    /// Restrict-only: a repository may lower the window, never raise it, for
    /// the same reason as the read caps. The session log still holds every
    /// round; only the request is bounded.
    pub agent_max_turn_messages: usize,
    /// Characters of `turn_notes` one turn may hold (spec 65).
    ///
    /// The notes are sent in every request of the turn and carried into the
    /// next turn of the session, so this is a bound on what a long
    /// investigation adds to every later call. Restrict-only, lower wins: a
    /// repository may lower it, never raise it, because raising it spends the
    /// user's money on the repository's say-so. `0` turns the feature off
    /// completely: no tool, no notes block, no notice changes, and nothing
    /// carried — the requests are exactly what they were before spec 65.
    pub agent_max_turn_notes_chars: usize,
    /// Directories the user made project roots, repository-relative (spec 24
    /// §5.7). Validated when the map is built, not here, so a bad entry is
    /// shown in the map's `excluded` rather than failing the config load.
    pub project_roots_added: Vec<String>,
    /// Detected roots the user said are not projects. Kept in the map, marked
    /// removed, so the decision can be seen and undone.
    pub project_roots_removed: Vec<String>,
    pub shell: String,
    pub model_provider: String,
    pub model_name: String,
    pub model_base_url: String,
    pub model_api_key_env: String,
    pub model_reasoning_level: String,
    pub model_providers: Vec<ModelProviderConfig>,
    /// Global kill-switch for MCP. Defaults to true; an admin overlay can set
    /// it false to disable all MCP servers regardless of user config.
    pub mcp_enabled: bool,
    /// Admin-oriented allowlist of permitted MCP server ids. When non-empty,
    /// only listed servers are offered even if the user enabled others.
    pub mcp_server_allowlist: Vec<String>,
    pub mcp_servers: Vec<McpServerConfig>,
    /// Lifecycle hooks (spec 32). A repository may define one of its own but
    /// never touch a user's or an admin's, and a profile may carry none.
    pub hooks: Vec<HookConfig>,
    /// The user's reviews of repository-defined hooks, keyed by repository id
    /// and then hook id. User-owned, like `Allow Always`: a repository cannot
    /// approve its own hook.
    pub hook_reviews_by_repository: BTreeMap<String, BTreeMap<String, HookReview>>,
    /// `language_server.<language>=<command line>` (spec 25 `context.md` §6),
    /// keyed by language. A program Damaian spawns, so it is set by user or
    /// admin config only, like `shell`.
    pub language_servers: BTreeMap<String, String>,
}

/// The languages `language_server.<language>` accepts: the names
/// `detect_language` returns for which `extract_declarations` has a tier to
/// improve on. Each is also the server's LSP language id.
pub const LANGUAGE_SERVER_LANGUAGES: [&str; 8] = [
    "go",
    "java",
    "javascript",
    "kotlin",
    "php",
    "python",
    "rust",
    "typescript",
];

// No `Eq`: the price rates are `Option<f64>`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelProviderConfig {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub api_key_env: String,
    pub models: Vec<String>,
    /// Whether to use the provider's native `tools`/`tool_calls` contract
    /// instead of the `DAMAIAN_COMMAND_V1` text envelope. Defaults to true;
    /// set it to false for a provider that does not implement the contract
    /// reliably.
    pub supports_native_tools: bool,
    /// Explicit `max_tokens` to send with every request to this provider.
    /// `None` omits the field and lets the provider apply its own default —
    /// fine for providers whose default is generous, but dangerous for ones
    /// that default low (DeepSeek defaults to 4096), because a large
    /// `propose_patch` call gets truncated mid-arguments and the resulting
    /// partial JSON is unusable.
    pub max_output_tokens: Option<u32>,
    /// How many tokens of repository context to pack into a request. `None`
    /// falls back to the built-in per-model budget. This is an input-side
    /// budget and is unrelated to [`Self::max_output_tokens`]: it bounds what
    /// the model reads, not what it writes. Raising it improves answer quality
    /// on large repositories but is billed on every turn, so the defaults stay
    /// well below what a model's context window technically allows.
    pub context_token_budget: Option<u32>,
    /// Whether to ask this provider to report token usage on a stream.
    /// Defaults to true: `stream_options` is standard for OpenAI-compatible
    /// APIs, and the adapter probes once for a provider that rejects it. Set
    /// false to skip even the probe. Spec 19 §5.2.
    pub provider_reports_usage: bool,
    /// The user's own price per million input tokens. **Not a built-in price
    /// table** — spec 19 §4 rules one out, because prices change and a stale
    /// table reports confident wrong numbers. `None` means no cost is shown.
    pub price_per_million_input_tokens: Option<f64>,
    /// The user's own price per million output tokens. Both rates must be set
    /// for a cost to be computed; one alone would silently omit half the bill.
    pub price_per_million_output_tokens: Option<f64>,
    /// The user's own price per million input tokens the provider served from
    /// its prompt cache — commonly a fraction of the uncached rate. `None`
    /// does **not** suppress the cost the way a missing base rate does: the
    /// figure is computed at the full input rate and labelled an upper bound
    /// instead, because that error can only run in the safe direction.
    /// Spec 49 §5.3.
    pub price_per_million_cached_input_tokens: Option<f64>,
    /// Whether this provider requires an explicit prompt-cache breakpoint
    /// marker rather than caching automatically. Defaults to `false`.
    /// Declared now because adding it later is a config-schema change across
    /// persisted provider settings, but dormant: no configured provider sets
    /// it, so turning it on is the only thing that changes a request body.
    /// Spec 49 §5.5.
    pub supports_explicit_cache_breakpoints: bool,
}

/// A cost computed from the user's own configured rates, and whether it is
/// exact or a ceiling.
///
/// The two travel together rather than as an `f64` beside a `bool` because
/// the label is the part that gets dropped: a caller that forgets it prints a
/// figure which reads as what the user will be billed, when the truth is "no
/// more than this". Spec 49 §5.3.
///
/// The fields are private and there is deliberately no `Deref`, no
/// `From<CostEstimate> for f64` and no public constructor taking a bare
/// number: reaching the amount means naming [`Self::amount`], and every
/// surface that does is a place a reviewer can see is also asking
/// [`Self::is_upper_bound`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostEstimate {
    amount: f64,
    upper_bound: bool,
}

impl CostEstimate {
    /// Every input token priced at the rate that actually applies to it.
    pub(crate) fn exact(amount: f64) -> Self {
        Self {
            amount,
            upper_bound: false,
        }
    }

    /// The provider reported cached tokens and the user configured no cached
    /// rate, so this prices them as though none were cached. It can only be
    /// too high.
    pub(crate) fn upper_bound(amount: f64) -> Self {
        Self {
            amount,
            upper_bound: true,
        }
    }

    pub fn amount(&self) -> f64 {
        self.amount
    }

    /// True when the figure ignores a cache discount the provider reported.
    /// A surface rendering the amount must say so — "at most $0.04", not
    /// "$0.04".
    pub fn is_upper_bound(&self) -> bool {
        self.upper_bound
    }
}

// No `Eq`: the price rates are `Option<f64>`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelProviderConfigOverlay {
    pub id: String,
    pub label: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub models: Option<Vec<String>>,
    pub supports_native_tools: Option<bool>,
    pub max_output_tokens: Option<u32>,
    pub context_token_budget: Option<u32>,
    pub provider_reports_usage: Option<bool>,
    pub price_per_million_input_tokens: Option<f64>,
    pub price_per_million_output_tokens: Option<f64>,
    pub price_per_million_cached_input_tokens: Option<f64>,
    pub supports_explicit_cache_breakpoints: Option<bool>,
}

/// How the client talks to an MCP server. `Stdio` spawns a local subprocess
/// and speaks newline-delimited JSON-RPC over its stdin/stdout; `Http` posts
/// JSON-RPC to a remote Streamable-HTTP endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpTransport {
    #[default]
    Stdio,
    Http,
}

impl McpTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            McpTransport::Stdio => "stdio",
            McpTransport::Http => "http",
        }
    }
}

/// A user-configured MCP server. Mirrors [`ModelProviderConfig`] in how it is
/// parsed, overlaid (upsert-by-id), and serialized. Secrets (`auth_token_env`)
/// are keychain references, never plaintext — same rule as model API keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    pub id: String,
    pub label: String,
    pub transport: McpTransport,
    /// stdio only: the executable to spawn.
    pub command: String,
    /// stdio only: arguments passed to `command`.
    pub args: Vec<String>,
    /// stdio only: extra environment variables (`KEY=VALUE`) for the child.
    pub env: Vec<(String, String)>,
    /// http only: the Streamable-HTTP endpoint URL.
    pub url: String,
    /// http only: `keychain:<account>` (or env var name) resolving to a bearer
    /// token sent as `Authorization: Bearer <token>`. Empty means no auth.
    pub auth_token_env: String,
    /// Off by default: a newly added server is only offered to the model once
    /// the user turns it on.
    pub enabled: bool,
    /// On by default: gate every `tools/call` on this server through the
    /// approval flow, since MCP tools can have arbitrary external side effects.
    pub require_approval: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerConfigOverlay {
    pub id: String,
    pub label: Option<String>,
    pub transport: Option<McpTransport>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<Vec<(String, String)>>,
    pub url: Option<String>,
    pub auth_token_env: Option<String>,
    pub enabled: Option<bool>,
    pub require_approval: Option<bool>,
}

/// Longest a hook may run before it is killed and counted as failed (spec 32
/// `context.md` §7). A hook on `before_tool` runs on every tool call, so a
/// minute is already generous; past it the hook is not a check, it is a job.
pub const MAX_HOOK_TIMEOUT_MS: u64 = 60_000;

/// A lifecycle point a hook can run at (spec 32 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HookEvent {
    SessionStart,
    BeforeContext,
    BeforeTool,
    AfterTool,
    BeforePatchApply,
    AfterPatchApply,
    AfterCheck,
    BeforeTaskComplete,
    SessionEnd,
}

impl HookEvent {
    /// Every event, in lifecycle order.
    pub const ALL: [HookEvent; 9] = [
        HookEvent::SessionStart,
        HookEvent::BeforeContext,
        HookEvent::BeforeTool,
        HookEvent::AfterTool,
        HookEvent::BeforePatchApply,
        HookEvent::AfterPatchApply,
        HookEvent::AfterCheck,
        HookEvent::BeforeTaskComplete,
        HookEvent::SessionEnd,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::SessionStart => "session_start",
            HookEvent::BeforeContext => "before_context",
            HookEvent::BeforeTool => "before_tool",
            HookEvent::AfterTool => "after_tool",
            HookEvent::BeforePatchApply => "before_patch_apply",
            HookEvent::AfterPatchApply => "after_patch_apply",
            HookEvent::AfterCheck => "after_check",
            HookEvent::BeforeTaskComplete => "before_task_complete",
            HookEvent::SessionEnd => "session_end",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.as_str() == value)
    }

    /// How long a hook at this event may run when it sets no `timeout_ms`
    /// (spec 32 `context.md` §7). Short where the hook sits on every tool
    /// call or in front of the turn; longer where it runs once per action
    /// that already took a while.
    pub fn default_timeout_ms(self) -> u64 {
        match self {
            HookEvent::SessionStart | HookEvent::BeforeContext | HookEvent::BeforeTool => 2_000,
            HookEvent::SessionEnd => 5_000,
            HookEvent::BeforePatchApply
            | HookEvent::AfterTool
            | HookEvent::AfterPatchApply
            | HookEvent::AfterCheck
            | HookEvent::BeforeTaskComplete => 10_000,
        }
    }
}

/// What a hook's failure does to the action (spec 32 §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HookMode {
    /// A failure blocks the action. The default, because a user who wrote a
    /// hook to enforce a rule wants it enforced when the hook is broken.
    #[default]
    Mandatory,
    /// A failure is a warning and the action proceeds.
    Optional,
}

impl HookMode {
    pub fn as_str(self) -> &'static str {
        match self {
            HookMode::Mandatory => "mandatory",
            HookMode::Optional => "optional",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mandatory" => Some(HookMode::Mandatory),
            "optional" => Some(HookMode::Optional),
            _ => None,
        }
    }
}

/// Which config file defined a hook. A repository-defined hook does not run
/// until the user has reviewed it (spec 32 `context.md` §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookSource {
    User,
    Repository,
    Admin,
}

impl HookSource {
    pub fn as_str(self) -> &'static str {
        match self {
            HookSource::User => "user",
            HookSource::Repository => "repository",
            HookSource::Admin => "admin",
        }
    }
}

/// A configured hook: an executable Damaian runs at its events, which can
/// only answer with a verdict (spec 32 §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfig {
    pub id: String,
    /// Run directly with `args`, never through `shell`.
    pub command: String,
    pub args: Vec<String>,
    pub events: Vec<HookEvent>,
    pub mode: HookMode,
    /// Hooks for one event run by `order`, then by id (spec 32 §5.6).
    pub order: i64,
    /// `None` takes the event's default.
    pub timeout_ms: Option<u64>,
    pub enabled: bool,
    pub source: HookSource,
}

impl HookConfig {
    /// The deadline this hook runs under at `event`.
    pub fn deadline_ms(&self, event: HookEvent) -> u64 {
        self.timeout_ms
            .unwrap_or_else(|| event.default_timeout_ms())
            .min(MAX_HOOK_TIMEOUT_MS)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookConfigOverlay {
    pub id: String,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub events: Option<Vec<HookEvent>>,
    pub mode: Option<HookMode>,
    pub order: Option<i64>,
    pub timeout_ms: Option<u64>,
    pub enabled: Option<bool>,
}

impl HookConfigOverlay {
    /// The fields this overlay sets, by the names `to_policy_text` prints
    /// after `hook.<id>.`.
    pub(crate) fn present_fields(&self) -> Vec<&'static str> {
        let Self {
            id: _,
            command,
            args,
            events,
            mode,
            order,
            timeout_ms,
            enabled,
        } = self;
        [
            ("command", command.is_some()),
            ("args", args.is_some()),
            ("events", events.is_some()),
            ("mode", mode.is_some()),
            ("order", order.is_some()),
            ("timeout_ms", timeout_ms.is_some()),
            ("enabled", enabled.is_some()),
        ]
        .into_iter()
        .filter_map(|(field, present)| present.then_some(field))
        .collect()
    }
}

/// The user's decision about one repository-defined hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookReview {
    /// Approved at this definition hash. A hook whose hash has changed since
    /// is not the hook the user approved.
    Approved {
        hash: String,
    },
    Declined,
}

impl HookReview {
    pub fn parse(value: &str) -> Result<Self> {
        if value == "declined" {
            return Ok(HookReview::Declined);
        }
        match value.strip_prefix("sha256:") {
            Some(hex) if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) => {
                Ok(HookReview::Approved {
                    hash: value.to_string(),
                })
            }
            _ => Err(ClientError::InvalidInput(
                "a hook review must be sha256:<hex> or declined".to_string(),
            )),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            HookReview::Approved { hash } => hash,
            HookReview::Declined => "declined",
        }
    }
}

impl ModelProviderConfigOverlay {
    /// The fields this overlay sets, by the names `to_policy_text` prints
    /// after `model_provider.<id>.`.
    fn present_fields(&self) -> Vec<&'static str> {
        // Exhaustive, so a new field cannot go unattributed.
        let Self {
            id: _,
            label,
            base_url,
            api_key_env,
            models,
            supports_native_tools,
            max_output_tokens,
            context_token_budget,
            provider_reports_usage,
            price_per_million_input_tokens,
            price_per_million_output_tokens,
            price_per_million_cached_input_tokens,
            supports_explicit_cache_breakpoints,
        } = self;
        [
            ("label", label.is_some()),
            ("base_url", base_url.is_some()),
            ("api_key_env", api_key_env.is_some()),
            ("models", models.is_some()),
            ("supports_native_tools", supports_native_tools.is_some()),
            ("max_output_tokens", max_output_tokens.is_some()),
            ("context_token_budget", context_token_budget.is_some()),
            ("provider_reports_usage", provider_reports_usage.is_some()),
            (
                "price_per_million_input_tokens",
                price_per_million_input_tokens.is_some(),
            ),
            (
                "price_per_million_output_tokens",
                price_per_million_output_tokens.is_some(),
            ),
            (
                "price_per_million_cached_input_tokens",
                price_per_million_cached_input_tokens.is_some(),
            ),
            (
                "supports_explicit_cache_breakpoints",
                supports_explicit_cache_breakpoints.is_some(),
            ),
        ]
        .into_iter()
        .filter_map(|(field, present)| present.then_some(field))
        .collect()
    }
}

impl McpServerConfigOverlay {
    /// The fields this overlay sets, by the names `to_policy_text` prints
    /// after `mcp_server.<id>.`.
    fn present_fields(&self) -> Vec<&'static str> {
        let Self {
            id: _,
            label,
            transport,
            command,
            args,
            env,
            url,
            auth_token_env,
            enabled,
            require_approval,
        } = self;
        [
            ("label", label.is_some()),
            ("transport", transport.is_some()),
            ("command", command.is_some()),
            ("args", args.is_some()),
            ("env", env.is_some()),
            ("url", url.is_some()),
            ("auth_token_env", auth_token_env.is_some()),
            ("enabled", enabled.is_some()),
            ("require_approval", require_approval.is_some()),
        ]
        .into_iter()
        .filter_map(|(field, present)| present.then_some(field))
        .collect()
    }
}

impl Config {
    pub fn default_data_dir() -> PathBuf {
        if let Ok(value) = std::env::var("DAMAIAN_DATA_DIR") {
            return PathBuf::from(value);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("DamaianClient")
    }

    pub fn load_for_repository(repository_root: Option<&Path>) -> Result<Self> {
        Self::load_for_repository_reporting(repository_root).map(|(config, _)| config)
    }

    /// The same load, also reporting what the repository's own config asked
    /// for and did not get. Callers that can audit or tell the user — the
    /// desktop shell and the CLI — should use this one.
    pub fn load_for_repository_reporting(
        repository_root: Option<&Path>,
    ) -> Result<(Self, RepositoryConfigReport)> {
        let config = Self::default();
        let default_data_dir = config.data_dir.clone();
        let user_path = default_data_dir.join("config").join("user.conf");
        let repo_path = repository_root.map(Self::repository_config_path);
        let admin_path = std::env::var("DAMAIAN_ADMIN_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_data_dir.join("config").join("admin.conf"));
        Self::load_scoped(
            config,
            Some(user_path.as_path()),
            repo_path.as_deref(),
            Some(admin_path.as_path()),
            repository_root,
        )
    }

    pub fn load_with_policy_paths(
        config: Self,
        user_path: Option<&Path>,
        repo_path: Option<&Path>,
        admin_path: Option<&Path>,
    ) -> Result<Self> {
        Self::load_with_policy_paths_reporting(config, user_path, repo_path, admin_path)
            .map(|(config, _)| config)
    }

    pub fn load_with_policy_paths_reporting(
        config: Self,
        user_path: Option<&Path>,
        repo_path: Option<&Path>,
        admin_path: Option<&Path>,
    ) -> Result<(Self, RepositoryConfigReport)> {
        // No repository root was passed, so infer it from the layout
        // `repository_config_path` writes. A path shaped some other way is
        // still applied at repository scope — it is still repository config —
        // but its per-repository `Allow Always` entries cannot be resolved,
        // because there is no repository identity to resolve them against.
        let derived = repo_path.and_then(repository_root_from_config_path);
        Self::load_scoped(config, user_path, repo_path, admin_path, derived)
    }

    /// Applies the three overlays in order, each at its own scope, and folds in
    /// the `Allow Always` entries the user granted for this repository.
    ///
    /// Admin comes last and stays trusted: `DAMAIAN_ADMIN_CONFIG` or
    /// `<data_dir>/config/admin.conf` is a local file, not something a clone
    /// carries.
    pub fn load_scoped(
        mut config: Self,
        user_path: Option<&Path>,
        repo_path: Option<&Path>,
        admin_path: Option<&Path>,
        repository_root: Option<&Path>,
    ) -> Result<(Self, RepositoryConfigReport)> {
        let mut report = RepositoryConfigReport {
            repository_root: repository_root.map(Path::to_path_buf),
            ..RepositoryConfigReport::default()
        };
        if let Some(path) = user_path
            && path.exists()
        {
            let outcome =
                config.apply_overlay_scoped(ConfigOverlay::load(path)?, ConfigScope::User);
            report.applied.extend(outcome.applied);
        }
        if let Some(path) = repo_path
            && path.exists()
        {
            // Untrusted, so it is read and parsed tolerantly: a line Damaian
            // cannot read or parse is reported and skipped, never propagated.
            // A repository that can fail this load denies Damaian the config
            // for itself, which is the availability half of "restrict but
            // never break". The parse failures are reported ahead of the
            // classification refusals, which is the order they happened in.
            let (overlay, mut rejected) = match fs::read_to_string(path) {
                Ok(content) => ConfigOverlay::parse_untrusted(&content),
                // `exists()` already followed the symlink, so this is a
                // directory at `config.conf`, a permissions problem, or
                // non-UTF-8 — the same hole through a different door.
                Err(_) => (
                    ConfigOverlay::default(),
                    vec![RejectedConfigKey::new(
                        "(unreadable)",
                        RepositoryKeyClass::Unparsable,
                    )],
                ),
            };
            report.repository_allowlist_entries =
                overlay.command_allowlist.clone().unwrap_or_default();
            let outcome = config.apply_overlay_scoped(overlay, ConfigScope::Repository);
            rejected.extend(outcome.rejected);
            report.rejected_keys = rejected;
            report.applied.extend(outcome.applied);
        }
        if let Some(path) = admin_path
            && path.exists()
        {
            let outcome =
                config.apply_overlay_scoped(ConfigOverlay::load(path)?, ConfigScope::Admin);
            report.applied.extend(outcome.applied);
        }
        // User and admin config are the user's own files, so a hook they
        // left without a command or events is a mistake to report, as a bad
        // key in them is. Repository hooks never get here incomplete.
        if let Some(hook) = config
            .hooks
            .iter()
            .find(|hook| hook.command.is_empty() || hook.events.is_empty())
        {
            return Err(ClientError::InvalidInput(format!(
                "hook {} needs both hook.{}.command and hook.{}.events",
                hook.id, hook.id, hook.id
            )));
        }
        // Last and restrict-only (spec 31, `context.md` §4), so admin can
        // widen the base but cannot undo a narrower profile the user chose.
        // With no selection nothing is applied, which is Full: the empty
        // overlay.
        if let Some(root) = repository_root
            && let Some(profile) = config
                .permission_profile_by_repository
                .get(&repository_id_for_root(root))
                .cloned()
        {
            let (overlay, mut rejected) = profile.overlay(&config.data_dir)?;
            let outcome = config.apply_overlay_scoped(overlay, ConfigScope::Profile);
            rejected.extend(outcome.rejected);
            report.profile_rejected_keys = rejected;
            report.applied.extend(outcome.applied);
            report.permission_profile = Some(profile);
        }
        if let Some(root) = repository_root {
            report.allow_always_entries = config.apply_repository_allowlist(root);
        }
        // Spec 26 §5.2: an oversubscribed share set is a config error, and the
        // resolved config is where the sum is finally known.
        config.validate_context_budget()?;
        Ok((config, report))
    }

    pub fn user_config_path(&self) -> PathBuf {
        self.data_dir.join("config").join("user.conf")
    }

    pub fn admin_config_path(&self) -> PathBuf {
        std::env::var("DAMAIAN_ADMIN_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| self.data_dir.join("config").join("admin.conf"))
    }

    pub fn repository_config_path(repository_root: impl AsRef<Path>) -> PathBuf {
        repository_root
            .as_ref()
            .join(".damaian")
            .join("config.conf")
    }

    /// Applies an overlay at user scope: every value it sets is taken.
    pub fn apply_overlay(&mut self, overlay: ConfigOverlay) {
        self.apply_overlay_scoped(overlay, ConfigScope::User);
    }

    /// Applies an overlay, honouring what the scope is allowed to change.
    /// Returns the keys that were refused, which the caller is expected to
    /// audit and surface — [`RejectedConfigKey`] deliberately carries no
    /// values, since a refused `shell` or `model_api_key_env` is
    /// attacker-controlled text — and the keys that were applied, which is the
    /// only record of where a value came from (spec 31, `context.md` §8).
    pub fn apply_overlay_scoped(
        &mut self,
        overlay: ConfigOverlay,
        scope: ConfigScope,
    ) -> OverlayOutcome {
        // Exhaustive destructuring, deliberately without `..`: a field added to
        // `ConfigOverlay` and not classified here fails to compile. That is
        // what keeps this boundary from decaying — the alternative default is
        // repository-writable, which is the defect this exists to fix.
        let ConfigOverlay {
            data_dir,
            max_file_bytes,
            max_read_lines,
            max_list_entries,
            max_search_matches,
            max_match_line_chars,
            repository_map_max_tokens,
            symbol_context_max_tokens,
            context_budget_instructions,
            context_budget_hook_context,
            context_budget_pinned,
            context_budget_plan_state,
            context_budget_repository_map,
            context_budget_symbols,
            context_budget_search_results,
            context_budget_file_ranges,
            context_budget_findings,
            context_budget_conversation_summary,
            context_budget_safety_margin,
            max_command_output_bytes,
            command_timeout_secs,
            allowed_roots,
            ignore_patterns,
            restricted_patterns,
            command_allowlist,
            command_allowlist_by_repository,
            permission_profile_by_repository,
            command_blocklist,
            secret_patterns,
            require_approval_for_file_edits,
            require_approval_for_risky_commands,
            require_approval_for_all_commands,
            allow_file_edits,
            command_access,
            allow_browser_diagnostics,
            allow_mutating_mcp_tools,
            block_generated_secrets,
            audit_enabled,
            audit_retention_days,
            checkpoint_retention_days,
            checkpoint_max_total_bytes,
            checkpoint_census_max_paths,
            enable_semantic_search,
            enable_worker_sessions,
            agent_max_tool_rounds,
            agent_web_debug_max_tool_rounds,
            agent_tool_retry_limit,
            agent_max_questions_per_turn,
            agent_max_task_tokens,
            agent_max_turn_messages,
            agent_max_turn_notes_chars,
            agent_max_exploration_rounds,
            project_roots_added,
            project_roots_removed,
            shell,
            model_provider,
            model_name,
            model_base_url,
            model_api_key_env,
            model_reasoning_level,
            model_providers,
            mcp_enabled,
            mcp_server_allowlist,
            mcp_servers,
            hooks,
            hook_reviews_by_repository,
            language_servers,
        } = overlay;

        let mut outcome = OverlayOutcome::default();
        // Exhaustive, so a new scope is a decision and never trusted by
        // default.
        let trusted = match scope {
            ConfigScope::User | ConfigScope::Admin => true,
            // A profile narrows exactly as far as an untrusted repository may:
            // it adds restrictions and removes none (spec 31, `context.md` §4).
            ConfigScope::Repository | ConfigScope::Profile => false,
        };
        let forbidden = RepositoryKeyClass::Forbidden;
        // Refusals are collected apart from `outcome` only so both can be
        // written while a helper holds the refusal list.
        let mut refused = Vec::new();
        let rejected = &mut refused;

        // Forbidden: redirects execution, model traffic, credentials, or where
        // data lands. These have no restrict-only direction, so an admin value
        // is attributed to admin but never marked a widening.
        if let Some(value) = scoped(data_dir, "data_dir", trusted, forbidden, rejected) {
            self.data_dir = value;
            outcome.record(scope, "data_dir", None, false);
        }
        if let Some(value) = scoped(allowed_roots, "allowed_roots", trusted, forbidden, rejected) {
            let entries = value
                .iter()
                .map(|root| root.to_string_lossy().to_string())
                .collect();
            self.allowed_roots = value;
            outcome.record(scope, "allowed_roots", Some(entries), false);
        }
        if let Some(value) = scoped(
            secret_patterns,
            "secret_patterns",
            trusted,
            forbidden,
            rejected,
        ) {
            outcome.record(scope, "secret_patterns", Some(value.clone()), false);
            self.secret_patterns = value;
        }
        if let Some(value) = scoped(
            block_generated_secrets,
            "block_generated_secrets",
            trusted,
            forbidden,
            rejected,
        ) {
            self.block_generated_secrets = value;
            outcome.record(scope, "block_generated_secrets", None, false);
        }
        if let Some(value) = scoped(audit_enabled, "audit_enabled", trusted, forbidden, rejected) {
            self.audit_enabled = value;
            outcome.record(scope, "audit_enabled", None, false);
        }
        // Adds text to every session's system prompt, which a repository must
        // not be able to do (spec 57 §5.8).
        if let Some(value) = scoped(
            enable_worker_sessions,
            "enable_worker_sessions",
            trusted,
            forbidden,
            rejected,
        ) {
            self.enable_worker_sessions = value;
            outcome.record(scope, "enable_worker_sessions", None, false);
        }
        if let Some(value) = scoped(shell, "shell", trusted, forbidden, rejected) {
            self.shell = value;
            outcome.record(scope, "shell", None, false);
        }
        // A program Damaian starts unprompted, with the repository as its
        // working directory: the same power as `shell`, so a clone may not name
        // one and a profile does not carry one (spec 25 `context.md` §6).
        for (language, command) in language_servers {
            let key = format!("language_server.{language}");
            if let Some(command) = scoped(Some(command), &key, trusted, forbidden, rejected) {
                self.language_servers.insert(language, command);
                outcome.record(scope, key, None, false);
            }
        }
        for provider in model_providers {
            let key = format!("model_provider.{}", provider.id);
            if trusted {
                for field in provider.present_fields() {
                    outcome.record(scope, format!("{key}.{field}"), None, false);
                }
                self.upsert_model_provider(provider);
            } else {
                rejected.push(RejectedConfigKey::new(key, forbidden));
            }
        }
        if let Some(value) = scoped(
            model_provider,
            "model_provider",
            trusted,
            forbidden,
            rejected,
        ) {
            self.model_provider = value;
            outcome.record(scope, "model_provider", None, false);
            // Choosing a provider also sets these three from its defaults, so
            // the scope that chose it is what they now hold.
            let before = (
                self.model_name.clone(),
                self.model_base_url.clone(),
                self.model_api_key_env.clone(),
            );
            self.apply_model_provider_defaults();
            for (key, was, now) in [
                ("model_name", &before.0, &self.model_name),
                ("model_base_url", &before.1, &self.model_base_url),
                ("model_api_key_env", &before.2, &self.model_api_key_env),
            ] {
                if was != now {
                    outcome.record(scope, key, None, false);
                }
            }
        }
        if let Some(value) = scoped(model_name, "model_name", trusted, forbidden, rejected) {
            self.model_name = value;
            outcome.record(scope, "model_name", None, false);
        }
        if let Some(value) = scoped(
            model_base_url,
            "model_base_url",
            trusted,
            forbidden,
            rejected,
        ) {
            self.model_base_url = value;
            outcome.record(scope, "model_base_url", None, false);
        }
        if let Some(value) = scoped(
            model_api_key_env,
            "model_api_key_env",
            trusted,
            forbidden,
            rejected,
        ) {
            self.model_api_key_env = value;
            outcome.record(scope, "model_api_key_env", None, false);
        }
        if let Some(value) = scoped(
            model_reasoning_level,
            "model_reasoning_level",
            trusted,
            forbidden,
            rejected,
        ) {
            self.model_reasoning_level = value;
            outcome.record(scope, "model_reasoning_level", None, false);
        }

        // User-owned: an `Allow Always` decision is the user's, and lives in
        // user config keyed by repository. A repository copy is never honoured.
        if let Some(value) = scoped(
            command_allowlist,
            "command_allowlist",
            trusted,
            RepositoryKeyClass::UserOwned,
            rejected,
        ) {
            outcome.record(scope, "command_allowlist", Some(value.clone()), false);
            self.command_allowlist = value;
        }
        // The per-repository maps are not policy keys of this checkout: what
        // they contribute is attributed by the `Allow Always` fold and the
        // profile scope, so applying them records nothing.
        for (repository_id, entries) in command_allowlist_by_repository {
            let key = format!("command_allowlist.{repository_id}");
            if trusted {
                self.command_allowlist_by_repository
                    .insert(repository_id, entries);
            } else {
                rejected.push(RejectedConfigKey::new(key, RepositoryKeyClass::UserOwned));
            }
        }
        // User-owned for the same reason: choosing a checkout's profile is the
        // user's decision, and a profile cannot select another profile.
        for (repository_id, profile) in permission_profile_by_repository {
            let key = format!("permission_profile.{repository_id}");
            if trusted {
                self.permission_profile_by_repository
                    .insert(repository_id, profile);
            } else {
                rejected.push(RejectedConfigKey::new(key, RepositoryKeyClass::UserOwned));
            }
        }

        // Restrict-only: either scope may add a restriction; a repository
        // cannot take one away.
        for (key, current, incoming) in [
            (
                "ignore_patterns",
                &mut self.ignore_patterns,
                ignore_patterns,
            ),
            (
                "restricted_patterns",
                &mut self.restricted_patterns,
                restricted_patterns,
            ),
            (
                "command_blocklist",
                &mut self.command_blocklist,
                command_blocklist,
            ),
        ] {
            if let Some(value) = incoming
                && let Some((entries, loosened)) = union_patterns(current, value, trusted)
            {
                outcome.record(scope, key, Some(entries), loosened);
            }
        }
        // `true` is the restrictive value of the approval flags, `false` that
        // of the profile capability keys and `mcp_enabled`.
        for (key, current, incoming, restrictive) in [
            (
                "require_approval_for_file_edits",
                &mut self.require_approval_for_file_edits,
                require_approval_for_file_edits,
                true,
            ),
            (
                "require_approval_for_risky_commands",
                &mut self.require_approval_for_risky_commands,
                require_approval_for_risky_commands,
                true,
            ),
            (
                "require_approval_for_all_commands",
                &mut self.require_approval_for_all_commands,
                require_approval_for_all_commands,
                true,
            ),
            // Profile capability keys: a repository may turn each down, never
            // up.
            (
                "allow_file_edits",
                &mut self.allow_file_edits,
                allow_file_edits,
                false,
            ),
        ] {
            if let Some(value) = incoming
                && let Some(loosened) =
                    restrict_only_flag(current, value, restrictive, key, trusted, rejected)
            {
                outcome.record(scope, key, None, loosened);
            }
        }
        if let Some(value) = command_access
            && let Some(loosened) = restrict_only_access(
                &mut self.command_access,
                value,
                "command_access",
                trusted,
                rejected,
            )
        {
            outcome.record(scope, "command_access", None, loosened);
        }
        for (key, current, incoming, restrictive) in [
            (
                "allow_browser_diagnostics",
                &mut self.allow_browser_diagnostics,
                allow_browser_diagnostics,
                false,
            ),
            (
                "allow_mutating_mcp_tools",
                &mut self.allow_mutating_mcp_tools,
                allow_mutating_mcp_tools,
                false,
            ),
            ("mcp_enabled", &mut self.mcp_enabled, mcp_enabled, false),
        ] {
            if let Some(value) = incoming
                && let Some(loosened) =
                    restrict_only_flag(current, value, restrictive, key, trusted, rejected)
            {
                outcome.record(scope, key, None, loosened);
            }
        }
        if let Some(value) = mcp_server_allowlist
            && let Some((entries, loosened)) = intersect_allowlist(
                &mut self.mcp_server_allowlist,
                value,
                "mcp_server_allowlist",
                trusted,
                rejected,
            )
        {
            outcome.record(scope, "mcp_server_allowlist", Some(entries), loosened);
        }
        for server in mcp_servers {
            let fields = match scope {
                ConfigScope::User | ConfigScope::Admin => {
                    let existing = self.mcp_server_config(&server.id);
                    // A server that was off, or did not exist, being enabled
                    // or losing its approval gate is the direction a
                    // repository is refused.
                    let enabled_before = existing.is_some_and(|server| server.enabled);
                    let gated_before = existing.is_none_or(|server| server.require_approval);
                    let loosened = |field: &str| match field {
                        "enabled" => server.enabled == Some(true) && !enabled_before,
                        "require_approval" => {
                            server.require_approval == Some(false) && gated_before
                        }
                        _ => false,
                    };
                    let fields: Vec<_> = server
                        .present_fields()
                        .into_iter()
                        .map(|field| (field, loosened(field)))
                        .collect();
                    let id = server.id.clone();
                    self.upsert_mcp_server(server);
                    (id, fields)
                }
                ConfigScope::Repository => {
                    let id = server.id.clone();
                    let fields = self.upsert_mcp_server_from_repository(server, true, rejected);
                    (id, fields.into_iter().map(|field| (field, false)).collect())
                }
                // A server definition is not a profile key: it is where
                // `auth_token_env` lives, and an export must never carry one
                // (`context.md` §9). A profile may only disable or gate a
                // server the user already has.
                ConfigScope::Profile => {
                    let id = server.id.clone();
                    let fields = self.upsert_mcp_server_from_repository(server, false, rejected);
                    (id, fields.into_iter().map(|field| (field, false)).collect())
                }
            };
            let (id, fields) = fields;
            for (field, loosened) in fields {
                outcome.record(scope, format!("mcp_server.{id}.{field}"), None, loosened);
            }
        }
        // Spec 32 `context.md` §7.1. A repository may define a hook of its
        // own, which runs only once the user has reviewed it, but may not
        // touch one a user or admin defined in any field: disabling,
        // softening or retiming a user's hook is how a clone would switch off
        // the rule the hook enforces. A profile defines and changes nothing.
        for hook in hooks {
            let id = hook.id.clone();
            let fields = hook.present_fields();
            let refuse_all = |class: RepositoryKeyClass, rejected: &mut Vec<RejectedConfigKey>| {
                for field in &fields {
                    rejected.push(RejectedConfigKey::new(format!("hook.{id}.{field}"), class));
                }
            };
            let source = match scope {
                ConfigScope::User => HookSource::User,
                ConfigScope::Admin => HookSource::Admin,
                ConfigScope::Repository => {
                    if self.hook(&id).is_some() {
                        refuse_all(forbidden, rejected);
                        continue;
                    }
                    // Repository config is parsed line by line, so a hook it
                    // left half-defined is skipped here rather than failing
                    // the load the way an incomplete user hook does.
                    if hook.command.is_none() || hook.events.is_none() {
                        refuse_all(RepositoryKeyClass::Unparsable, rejected);
                        continue;
                    }
                    HookSource::Repository
                }
                ConfigScope::Profile => {
                    refuse_all(forbidden, rejected);
                    continue;
                }
            };
            // Only admin can overrule a hook another scope defined, and when
            // it loosens one the policy view says so.
            let loosened = |field: &str| {
                let Some(before) = self.hook(&id) else {
                    return false;
                };
                match field {
                    "enabled" => hook.enabled == Some(false) && before.enabled,
                    "mode" => {
                        hook.mode == Some(HookMode::Optional) && before.mode == HookMode::Mandatory
                    }
                    "timeout_ms" => hook.timeout_ms.is_some_and(|after| {
                        before
                            .events
                            .iter()
                            .any(|event| after > before.deadline_ms(*event))
                    }),
                    _ => false,
                }
            };
            let recorded: Vec<_> = fields
                .iter()
                .map(|field| (*field, loosened(field)))
                .collect();
            self.upsert_hook(hook, source);
            for (field, loosened) in recorded {
                outcome.record(scope, format!("hook.{id}.{field}"), None, loosened);
            }
        }
        // User-owned, like the other per-repository decisions: a repository
        // cannot approve its own hook, and a profile cannot either.
        for (repository_id, reviews) in hook_reviews_by_repository {
            for (hook_id, review) in reviews {
                if trusted {
                    self.hook_reviews_by_repository
                        .entry(repository_id.clone())
                        .or_default()
                        .insert(hook_id, review);
                } else {
                    rejected.push(RejectedConfigKey::new(
                        format!("hook_review.{repository_id}.{hook_id}"),
                        RepositoryKeyClass::UserOwned,
                    ));
                }
            }
        }

        // Restrict-only, unlike the three `agent_*` round bounds in the "Free"
        // block below. Lowering a ceiling is a nuisance; raising one the user
        // set low spends the user's money, which is a capability rather than a
        // preference. `context.md` §3.8.
        if let Some(value) = agent_max_task_tokens
            && let Some(loosened) = restrict_only_ceiling(
                &mut self.agent_max_task_tokens,
                value,
                "agent_max_task_tokens",
                trusted,
                rejected,
            )
        {
            outcome.record(scope, "agent_max_task_tokens", None, loosened);
        }

        // Restrict-only, lower wins, for the same reason as the token ceiling:
        // a repository may shrink the read allowance, never widen it, because
        // raising it spends the user's money on the repository's say-so (spec
        // 59 §5.4). A `u32` apply block because the read caps' loop below is
        // `usize`-typed.
        if let Some(value) = agent_max_exploration_rounds
            && let Some(loosened) = restrict_only_u32(
                &mut self.agent_max_exploration_rounds,
                value,
                "agent_max_exploration_rounds",
                trusted,
                rejected,
            )
        {
            outcome.record(scope, "agent_max_exploration_rounds", None, loosened);
        }

        // Restrict-only like the token ceiling: a repository may shrink the
        // window, never widen what one request carries.
        //
        // The four read caps are restrict-only for the same reason: a lower
        // cap is a restriction and always allowed, a higher one widens what a
        // repository can pull into the user's context. Spec 47 §5.5.
        //
        // And the command deadline: shortening it is a restriction a
        // repository may apply; lengthening one the user chose would let a
        // clone run its commands past the user's bound. Spec 47 requirement 5.
        for (key, current, incoming) in [
            (
                "agent_max_turn_messages",
                &mut self.agent_max_turn_messages,
                agent_max_turn_messages,
            ),
            (
                "agent_max_turn_notes_chars",
                &mut self.agent_max_turn_notes_chars,
                agent_max_turn_notes_chars,
            ),
            ("max_read_lines", &mut self.max_read_lines, max_read_lines),
            (
                "max_list_entries",
                &mut self.max_list_entries,
                max_list_entries,
            ),
            (
                "max_search_matches",
                &mut self.max_search_matches,
                max_search_matches,
            ),
            (
                "max_match_line_chars",
                &mut self.max_match_line_chars,
                max_match_line_chars,
            ),
            (
                "repository_map_max_tokens",
                &mut self.repository_map_max_tokens,
                repository_map_max_tokens,
            ),
            (
                "symbol_context_max_tokens",
                &mut self.symbol_context_max_tokens,
                symbol_context_max_tokens,
            ),
            (
                "command_timeout_secs",
                &mut self.command_timeout_secs,
                command_timeout_secs,
            ),
        ] {
            if let Some(value) = incoming
                && let Some(loosened) = restrict_only_limit(current, value, key, trusted, rejected)
            {
                outcome.record(scope, key, None, loosened);
            }
        }

        // Spec 26 §5.2: each category share is restrict-only and lower-wins —
        // a share steers model traffic toward or away from a repository's own
        // content, so a clone may lower one but never raise it. The margin is
        // the opposite direction: reserving more of an observed window is the
        // restriction, so a repository may raise it and never lower it. A
        // refused value is never clipped into force; it is reported by key.
        for (key, category, incoming) in [
            (
                "context_budget.instructions",
                ContextCategory::Instructions,
                context_budget_instructions,
            ),
            (
                "context_budget.hook_context",
                ContextCategory::HookContext,
                context_budget_hook_context,
            ),
            (
                "context_budget.pinned",
                ContextCategory::Pinned,
                context_budget_pinned,
            ),
            (
                "context_budget.plan_state",
                ContextCategory::PlanState,
                context_budget_plan_state,
            ),
            (
                "context_budget.repository_map",
                ContextCategory::RepositoryMap,
                context_budget_repository_map,
            ),
            (
                "context_budget.symbols",
                ContextCategory::Symbols,
                context_budget_symbols,
            ),
            (
                "context_budget.search_results",
                ContextCategory::SearchResults,
                context_budget_search_results,
            ),
            (
                "context_budget.file_ranges",
                ContextCategory::FileRanges,
                context_budget_file_ranges,
            ),
            (
                "context_budget.findings",
                ContextCategory::Findings,
                context_budget_findings,
            ),
            (
                "context_budget.conversation_summary",
                ContextCategory::ConversationSummary,
                context_budget_conversation_summary,
            ),
        ] {
            if let Some(value) = incoming
                && let Some(current) = self.context_budget.share_mut(category)
                && let Some(loosened) = restrict_only_share(current, value, key, trusted, rejected)
            {
                outcome.record(scope, key, None, loosened);
            }
        }
        if let Some(value) = context_budget_safety_margin
            && let Some(loosened) = restrict_only_minimum(
                &mut self.context_budget.safety_margin_bps,
                value,
                "context_budget.safety_margin",
                trusted,
                rejected,
            )
        {
            outcome.record(scope, "context_budget.safety_margin", None, loosened);
        }
        // The user's own profile may lower the retention window, which is
        // what Offline private is for (`context.md` §3).
        if let Some(value) = checkpoint_retention_days {
            let held = match scope {
                ConfigScope::User | ConfigScope::Repository | ConfigScope::Admin => {
                    match scoped(
                        Some(value),
                        "checkpoint_retention_days",
                        trusted,
                        forbidden,
                        rejected,
                    ) {
                        Some(value) => {
                            self.checkpoint_retention_days = value;
                            true
                        }
                        None => false,
                    }
                }
                ConfigScope::Profile => lower_wins(&mut self.checkpoint_retention_days, value),
            };
            if held {
                outcome.record(scope, "checkpoint_retention_days", None, false);
            }
        }
        if let Some(value) = scoped(
            checkpoint_max_total_bytes,
            "checkpoint_max_total_bytes",
            trusted,
            forbidden,
            rejected,
        ) {
            self.checkpoint_max_total_bytes = value;
            outcome.record(scope, "checkpoint_max_total_bytes", None, false);
        }
        if let Some(value) = scoped(
            checkpoint_census_max_paths,
            "checkpoint_census_max_paths",
            trusted,
            forbidden,
            rejected,
        ) {
            self.checkpoint_census_max_paths = value;
            outcome.record(scope, "checkpoint_census_max_paths", None, false);
        }

        // Free: preferences and budgets, with no capability behind them. A
        // profile narrows capability and does not reconfigure Damaian, so it
        // sets none of these (`context.md` §4).
        if let Some(value) = preference(max_file_bytes, "max_file_bytes", scope, rejected) {
            self.max_file_bytes = value;
            outcome.record(scope, "max_file_bytes", None, false);
        }
        if let Some(value) = preference(
            max_command_output_bytes,
            "max_command_output_bytes",
            scope,
            rejected,
        ) {
            self.max_command_output_bytes = value;
            outcome.record(scope, "max_command_output_bytes", None, false);
        }
        // The one preference a profile sets, and only lower: Offline private
        // keeps less history (`context.md` §3).
        if let Some(value) = audit_retention_days {
            let held = match scope {
                ConfigScope::User | ConfigScope::Repository | ConfigScope::Admin => {
                    self.audit_retention_days = value;
                    true
                }
                ConfigScope::Profile => lower_wins(&mut self.audit_retention_days, value),
            };
            if held {
                outcome.record(scope, "audit_retention_days", None, false);
            }
        }
        if let Some(value) = preference(
            enable_semantic_search,
            "enable_semantic_search",
            scope,
            rejected,
        ) {
            self.enable_semantic_search = value;
            outcome.record(scope, "enable_semantic_search", None, false);
        }
        if let Some(value) = preference(
            agent_max_tool_rounds,
            "agent_max_tool_rounds",
            scope,
            rejected,
        ) {
            self.agent_max_tool_rounds = value;
            outcome.record(scope, "agent_max_tool_rounds", None, false);
        }
        if let Some(value) = preference(
            agent_web_debug_max_tool_rounds,
            "agent_web_debug_max_tool_rounds",
            scope,
            rejected,
        ) {
            self.agent_web_debug_max_tool_rounds = value;
            outcome.record(scope, "agent_web_debug_max_tool_rounds", None, false);
        }
        if let Some(value) = preference(
            agent_tool_retry_limit,
            "agent_tool_retry_limit",
            scope,
            rejected,
        ) {
            self.agent_tool_retry_limit = value;
            outcome.record(scope, "agent_tool_retry_limit", None, false);
        }
        // Parsing already refused anything over the ceiling, at every scope.
        if let Some(value) = preference(
            agent_max_questions_per_turn,
            "agent_max_questions_per_turn",
            scope,
            rejected,
        ) {
            self.agent_max_questions_per_turn = value;
            outcome.record(scope, "agent_max_questions_per_turn", None, false);
        }
        if let Some(value) = preference(project_roots_added, "project_roots_added", scope, rejected)
        {
            self.project_roots_added = value;
            outcome.record(scope, "project_roots_added", None, false);
        }
        if let Some(value) = preference(
            project_roots_removed,
            "project_roots_removed",
            scope,
            rejected,
        ) {
            self.project_roots_removed = value;
            outcome.record(scope, "project_roots_removed", None, false);
        }

        outcome.rejected = refused;
        outcome
    }

    /// Whether the active model provider is configured to use native
    /// `tools`/`tool_calls` instead of the `DAMAIAN_COMMAND_V1` text
    /// envelope. Defaults to true for every provider; an explicit
    /// `supports_native_tools=false` opts out.
    pub fn supports_native_tools(&self) -> bool {
        self.model_provider_config(&self.model_provider)
            .cloned()
            .or_else(|| builtin_model_provider_config(&self.model_provider))
            .map(|provider| provider.supports_native_tools)
            .unwrap_or(false)
    }

    /// Whether the active model provider requires an explicit prompt-cache
    /// breakpoint marker rather than caching automatically. Defaults to
    /// false for any provider that hasn't explicitly opted in — dormant
    /// today because no built-in or configured provider sets it. Spec 49
    /// §5.5.
    pub fn supports_explicit_cache_breakpoints(&self) -> bool {
        self.model_provider_config(&self.model_provider)
            .cloned()
            .or_else(|| builtin_model_provider_config(&self.model_provider))
            .map(|provider| provider.supports_explicit_cache_breakpoints)
            .unwrap_or(false)
    }

    /// Whether the active model endpoint is a loopback address. Local
    /// runtimes such as Ollama serve without credentials, so callers
    /// resolving the API key may treat a missing key as empty rather than a
    /// configuration error. Remote endpoints keep failing loudly.
    pub fn model_endpoint_is_loopback(&self) -> bool {
        crate::web_diagnostics::is_loopback_url(&self.model_base_url)
    }

    /// The explicit `max_tokens` to send for the active provider and model.
    /// `None` leaves the field off the request entirely.
    ///
    /// Resolved most specific first:
    /// 1. a user-configured `model_provider.<id>.max_output_tokens`,
    /// 2. the built-in ceiling for this exact model,
    /// 3. the built-in provider-wide fallback for unrecognized models.
    ///
    /// Step 3 matters because an overlay routinely creates a partial entry for
    /// a built-in provider — setting only `label` and `supports_native_tools`,
    /// say — and such an entry must not silently drop the ceiling and
    /// reinstate the 4096-token truncation this field exists to prevent.
    /// Whether to ask the active provider for token usage. Defaults to true:
    /// the field is standard for OpenAI-compatible APIs, and the adapter's
    /// one-shot probe handles a provider that rejects it without the user
    /// having to know. Spec 19 §5.2.
    pub fn provider_reports_usage(&self) -> bool {
        self.model_provider_config(&self.model_provider)
            .map(|provider| provider.provider_reports_usage)
            .or_else(|| {
                builtin_model_provider_config(&self.model_provider)
                    .map(|provider| provider.provider_reports_usage)
            })
            .unwrap_or(true)
    }

    /// Cost from the user's own configured rates, or `None` when they have set
    /// none.
    ///
    /// Deliberately not a built-in price table (spec 19 §4): prices change,
    /// and nothing in this repository can keep one current, so a stale table
    /// would report confident wrong numbers. A figure from here is labelled
    /// estimated and attributed to the user's rates — never to the provider,
    /// whose own figure lives in `reported_cost`.
    ///
    /// Both rates must be set. One alone would produce a number that silently
    /// omits half the bill, which is worse than showing nothing.
    ///
    /// Where the provider reported cached input tokens and the user has
    /// configured a cached rate, the input tokens are split across the two
    /// rates. Where they have not, the figure is computed at the full input
    /// rate and comes back labelled an upper bound — see [`CostEstimate`].
    /// Spec 49 §5.3.
    pub fn estimated_cost(&self, usage: &TokenUsage) -> Option<CostEstimate> {
        let provider = self.model_provider_config(&self.model_provider)?;
        let input_rate = provider.price_per_million_input_tokens?;
        let output_rate = provider.price_per_million_output_tokens?;
        let output_cost = (usage.output_tokens as f64 / 1_000_000.0) * output_rate;

        // `None` is "the provider did not say", and a provider that said
        // nothing is not owed a discount. `Some(0)` is a measurement — it said
        // none hit — so the full rate is exactly right and calling it a bound
        // would understate what we know.
        let cached = usage.cached_input_tokens.filter(|cached| *cached > 0);
        let Some(cached) = cached else {
            return Some(CostEstimate::exact(
                (usage.input_tokens as f64 / 1_000_000.0) * input_rate + output_cost,
            ));
        };

        match provider.price_per_million_cached_input_tokens {
            Some(cached_rate) => {
                // `extract_usage` guarantees the subset invariant, so this
                // cannot underflow.
                let uncached = usage.input_tokens.saturating_sub(cached);
                Some(CostEstimate::exact(
                    (uncached as f64 / 1_000_000.0) * input_rate
                        + (cached as f64 / 1_000_000.0) * cached_rate
                        + output_cost,
                ))
            }
            // The user has not said what a cached token costs, so the only
            // honest figure is the one that assumes no discount — which can
            // only be too high, never too low. #19 shows nothing for a figure
            // that could be too low; this one is safe to show and label.
            None => Some(CostEstimate::upper_bound(
                (usage.input_tokens as f64 / 1_000_000.0) * input_rate + output_cost,
            )),
        }
    }

    pub fn max_output_tokens(&self) -> Option<u32> {
        self.model_provider_config(&self.model_provider)
            .and_then(|provider| provider.max_output_tokens)
            .or_else(|| builtin_model_output_tokens(&self.model_provider, &self.model_name))
            .or_else(|| {
                builtin_model_provider_config(&self.model_provider)
                    .and_then(|provider| provider.max_output_tokens)
            })
    }

    /// How many tokens of repository context to pack into a request, resolved
    /// with the same precedence as [`Self::max_output_tokens`] and falling back
    /// to [`DEFAULT_CONTEXT_TOKEN_BUDGET`]. Always returns a usable number —
    /// unlike the output ceiling, there is no "omit it" option, since some
    /// budget has to be chosen before context can be assembled.
    pub fn context_token_budget(&self) -> usize {
        self.model_provider_config(&self.model_provider)
            .and_then(|provider| provider.context_token_budget)
            .or_else(|| builtin_model_context_budget(&self.model_provider, &self.model_name))
            .or_else(|| {
                builtin_model_provider_config(&self.model_provider)
                    .and_then(|provider| provider.context_token_budget)
            })
            .unwrap_or(DEFAULT_CONTEXT_TOKEN_BUDGET) as usize
    }

    /// The category shares must fit the window: an oversubscribed set is a
    /// config error naming the key family, never a silent scale-down
    /// (spec 26 §5.2).
    pub fn validate_context_budget(&self) -> Result<()> {
        self.context_budget.validate()
    }

    pub fn apply_model_provider_defaults(&mut self) {
        if let Some(provider) = self
            .model_provider_config(&self.model_provider)
            .cloned()
            .or_else(|| builtin_model_provider_config(&self.model_provider))
        {
            self.model_base_url = provider.base_url;
            if provider.api_key_env.starts_with("keychain:")
                || !is_builtin_model_provider(&provider.id)
                || !is_keychain_reference(&self.model_api_key_env)
            {
                self.model_api_key_env = provider.api_key_env;
            }
            if let Some(model) = provider.models.first() {
                self.model_name = model.clone();
            }
        }
    }

    pub fn model_provider_config(&self, id: &str) -> Option<&ModelProviderConfig> {
        self.model_providers
            .iter()
            .find(|provider| provider.id == id)
    }

    /// The provider a refused turn may be offered to answer with instead, or
    /// `None` when there is nothing to offer and the refusal fails closed
    /// (spec 56 §5.2).
    ///
    /// Names only: whether a key actually sits behind `api_key_env` is the
    /// shell's question, because the engine never resolves credentials. A
    /// candidate is a *different* configured provider whose native-tool support
    /// matches the active one's, because the paused turn's messages were built
    /// for that contract (spec 56 `tasks.md` C3). An exhausted quota is
    /// offered only a provider with its own key: one sharing the active key
    /// variable is the same empty account under another name (C2).
    /// Deterministic: the first match in configuration order.
    pub fn fallback_provider_for(
        &self,
        refusal: &crate::error::ProviderRefusal,
    ) -> Option<&ModelProviderConfig> {
        let native_tools = self.supports_native_tools();
        self.model_providers.iter().find(|candidate| {
            candidate.id != self.model_provider
                && !candidate.api_key_env.trim().is_empty()
                && candidate.supports_native_tools == native_tools
                && (*refusal != crate::error::ProviderRefusal::QuotaExhausted
                    || candidate.api_key_env != self.model_api_key_env)
        })
    }

    fn upsert_model_provider(&mut self, overlay: ModelProviderConfigOverlay) {
        // A built-in provider is not in `model_providers` — it is produced on
        // demand by `builtin_model_provider_config` — so an overlay naming one
        // used to fall through to the insert path below and fill every unset
        // field with a default. The resulting blank entry then shadowed the
        // built-in completely, emptying its base URL, key variable and model
        // list. Seeding from the built-in first keeps a partial override
        // partial, which is what setting one key is asking for.
        if !self
            .model_providers
            .iter()
            .any(|provider| provider.id == overlay.id)
            && let Some(builtin) = builtin_model_provider_config(&overlay.id)
        {
            // The two token budgets are deliberately left unset. Both resolve
            // through three levels — a listed entry, then the per-model table,
            // then the built-in provider's fallback — and the built-in's own
            // values are that last-resort fallback. Copying them into a listed
            // entry would promote them to the first level and outrank the
            // per-model table, so a DeepSeek V4 install asking only for a
            // price would silently drop from a 65536 output ceiling to the
            // 8192 legacy fallback. Level three still consults the built-in.
            self.model_providers.push(ModelProviderConfig {
                max_output_tokens: None,
                context_token_budget: None,
                ..builtin
            });
        }

        if let Some(provider) = self
            .model_providers
            .iter_mut()
            .find(|provider| provider.id == overlay.id)
        {
            if let Some(value) = overlay.label {
                provider.label = value;
            }
            if let Some(value) = overlay.base_url {
                provider.base_url = value;
            }
            if let Some(value) = overlay.api_key_env {
                provider.api_key_env = value;
            }
            if let Some(value) = overlay.models {
                provider.models = value;
            }
            if let Some(value) = overlay.supports_native_tools {
                provider.supports_native_tools = value;
            }
            if let Some(value) = overlay.max_output_tokens {
                provider.max_output_tokens = Some(value);
            }
            if let Some(value) = overlay.context_token_budget {
                provider.context_token_budget = Some(value);
            }
            if let Some(value) = overlay.provider_reports_usage {
                provider.provider_reports_usage = value;
            }
            if let Some(value) = overlay.price_per_million_input_tokens {
                provider.price_per_million_input_tokens = Some(value);
            }
            if let Some(value) = overlay.price_per_million_output_tokens {
                provider.price_per_million_output_tokens = Some(value);
            }
            if let Some(value) = overlay.price_per_million_cached_input_tokens {
                provider.price_per_million_cached_input_tokens = Some(value);
            }
            if let Some(value) = overlay.supports_explicit_cache_breakpoints {
                provider.supports_explicit_cache_breakpoints = value;
            }
            return;
        }

        let id = overlay.id;
        self.model_providers.push(ModelProviderConfig {
            label: overlay.label.unwrap_or_else(|| id.clone()),
            base_url: overlay.base_url.unwrap_or_default(),
            api_key_env: overlay.api_key_env.unwrap_or_default(),
            models: overlay.models.unwrap_or_default(),
            supports_native_tools: overlay.supports_native_tools.unwrap_or(true),
            max_output_tokens: overlay.max_output_tokens,
            context_token_budget: overlay.context_token_budget,
            provider_reports_usage: overlay.provider_reports_usage.unwrap_or(true),
            price_per_million_input_tokens: overlay.price_per_million_input_tokens,
            price_per_million_output_tokens: overlay.price_per_million_output_tokens,
            price_per_million_cached_input_tokens: overlay.price_per_million_cached_input_tokens,
            supports_explicit_cache_breakpoints: overlay
                .supports_explicit_cache_breakpoints
                .unwrap_or(false),
            id,
        });
    }

    pub fn hook(&self, id: &str) -> Option<&HookConfig> {
        self.hooks.iter().find(|hook| hook.id == id)
    }

    pub fn mcp_server_config(&self, id: &str) -> Option<&McpServerConfig> {
        self.mcp_servers.iter().find(|server| server.id == id)
    }

    /// The MCP servers whose tools should actually be offered to the model:
    /// requires the global switch on, the server enabled, and — when an admin
    /// allowlist is set — the server to appear on it.
    pub fn active_mcp_servers(&self) -> Vec<&McpServerConfig> {
        if !self.mcp_enabled {
            return Vec::new();
        }
        self.mcp_servers
            .iter()
            .filter(|server| server.enabled)
            .filter(|server| {
                self.mcp_server_allowlist.is_empty()
                    || self.mcp_server_allowlist.contains(&server.id)
            })
            .collect()
    }

    fn upsert_mcp_server(&mut self, overlay: McpServerConfigOverlay) {
        if let Some(server) = self
            .mcp_servers
            .iter_mut()
            .find(|server| server.id == overlay.id)
        {
            if let Some(value) = overlay.label {
                server.label = value;
            }
            if let Some(value) = overlay.transport {
                server.transport = value;
            }
            if let Some(value) = overlay.command {
                server.command = value;
            }
            if let Some(value) = overlay.args {
                server.args = value;
            }
            if let Some(value) = overlay.env {
                server.env = value;
            }
            if let Some(value) = overlay.url {
                server.url = value;
            }
            if let Some(value) = overlay.auth_token_env {
                server.auth_token_env = value;
            }
            if let Some(value) = overlay.enabled {
                server.enabled = value;
            }
            if let Some(value) = overlay.require_approval {
                server.require_approval = value;
            }
            return;
        }

        let id = overlay.id;
        self.mcp_servers.push(McpServerConfig {
            label: overlay.label.unwrap_or_else(|| id.clone()),
            transport: overlay.transport.unwrap_or_default(),
            command: overlay.command.unwrap_or_default(),
            args: overlay.args.unwrap_or_default(),
            env: overlay.env.unwrap_or_default(),
            url: overlay.url.unwrap_or_default(),
            auth_token_env: overlay.auth_token_env.unwrap_or_default(),
            enabled: overlay.enabled.unwrap_or(false),
            require_approval: overlay.require_approval.unwrap_or(true),
            id,
        });
    }

    /// Upserts a hook by id. `source` is kept only for a new hook: admin
    /// changing a user's hook does not make it an admin hook.
    fn upsert_hook(&mut self, overlay: HookConfigOverlay, source: HookSource) {
        let HookConfigOverlay {
            id,
            command,
            args,
            events,
            mode,
            order,
            timeout_ms,
            enabled,
        } = overlay;
        let index = match self.hooks.iter().position(|hook| hook.id == id) {
            Some(index) => index,
            None => {
                self.hooks.push(HookConfig {
                    id,
                    command: String::new(),
                    args: Vec::new(),
                    events: Vec::new(),
                    mode: HookMode::default(),
                    order: 0,
                    timeout_ms: None,
                    enabled: true,
                    source,
                });
                self.hooks.len() - 1
            }
        };
        let hook = &mut self.hooks[index];
        if let Some(value) = command {
            hook.command = value;
        }
        if let Some(value) = args {
            hook.args = value;
        }
        if let Some(value) = events {
            hook.events = value;
        }
        if let Some(value) = mode {
            hook.mode = value;
        }
        if let Some(value) = order {
            hook.order = value;
        }
        if let Some(value) = timeout_ms {
            hook.timeout_ms = Some(value);
        }
        if let Some(value) = enabled {
            hook.enabled = value;
        }
    }

    /// Upserts a repository-supplied MCP server. A repository may *suggest* a
    /// server — that is legitimate and stays inert, since a new server is
    /// created disabled and approval-gated whatever the file asked for — but it
    /// may not enable one, clear its approval gate, or redefine a server the
    /// user already configured. That last case is not in the spec's
    /// classification table and is refused anyway: overlaying the `command` of
    /// a server the user has already enabled would redirect a process they
    /// trust, which requirement 2 forbids.
    ///
    /// `may_define` is false at profile scope, where a definition is refused
    /// even for a new server and nothing is created.
    ///
    /// Returns the fields it applied, for attribution.
    fn upsert_mcp_server_from_repository(
        &mut self,
        overlay: McpServerConfigOverlay,
        may_define: bool,
        rejected: &mut Vec<RejectedConfigKey>,
    ) -> Vec<&'static str> {
        let McpServerConfigOverlay {
            id,
            label,
            transport,
            command,
            args,
            env,
            url,
            auth_token_env,
            enabled,
            require_approval,
        } = overlay;
        let already_configured = self.mcp_server_config(&id).is_some();
        let mut narrowed = McpServerConfigOverlay {
            id: id.clone(),
            ..McpServerConfigOverlay::default()
        };

        match enabled {
            // Logical AND: disabling is a restriction, enabling is not.
            Some(false) => narrowed.enabled = Some(false),
            Some(true) => rejected.push(RejectedConfigKey::new(
                format!("mcp_server.{id}.enabled"),
                RepositoryKeyClass::RestrictOnly,
            )),
            None => {}
        }
        match require_approval {
            Some(true) => narrowed.require_approval = Some(true),
            Some(false) => rejected.push(RejectedConfigKey::new(
                format!("mcp_server.{id}.require_approval"),
                RepositoryKeyClass::RestrictOnly,
            )),
            None => {}
        }

        let definition = [
            ("label", label.is_some()),
            ("transport", transport.is_some()),
            ("command", command.is_some()),
            ("args", args.is_some()),
            ("env", env.is_some()),
            ("url", url.is_some()),
            ("auth_token_env", auth_token_env.is_some()),
        ];
        if already_configured || !may_define {
            for (field, present) in definition {
                if present {
                    rejected.push(RejectedConfigKey::new(
                        format!("mcp_server.{id}.{field}"),
                        RepositoryKeyClass::Forbidden,
                    ));
                }
            }
            if !already_configured {
                // Nothing to narrow, and creating an entry would be defining one.
                return Vec::new();
            }
        } else {
            if definition.iter().all(|(_, present)| !present) {
                // Nothing to define, and nothing a narrowed flag could apply
                // to: creating an empty server entry would only be clutter.
                return Vec::new();
            }
            narrowed.label = label;
            narrowed.transport = transport;
            narrowed.command = command;
            narrowed.args = args;
            narrowed.env = env;
            narrowed.url = url;
            narrowed.auth_token_env = auth_token_env;
            narrowed.enabled = Some(false);
            narrowed.require_approval = Some(true);
        }
        let fields = narrowed.present_fields();
        self.upsert_mcp_server(narrowed);
        fields
    }

    /// The `Allow Always` entries the user granted for `repository_root`,
    /// folded into the effective [`Self::command_allowlist`]. Called during a
    /// repository-scoped load; the entries live in user config keyed by
    /// repository id, never in the repository's own file.
    ///
    /// Returns the entries it added, which the effective policy attributes to
    /// this repository's `Allow Always`. An entry user config already listed
    /// is not added, so it stays attributed to user config.
    pub fn apply_repository_allowlist(&mut self, repository_root: &Path) -> Vec<String> {
        let repository_id = repository_id_for_root(repository_root);
        let Some(entries) = self.command_allowlist_by_repository.get(&repository_id) else {
            return Vec::new();
        };
        let mut added = Vec::new();
        for entry in entries.clone() {
            if !self.command_allowlist.contains(&entry) {
                self.command_allowlist.push(entry.clone());
                added.push(entry);
            }
        }
        added
    }

    /// The resolved values spec 31's refusal points consult.
    pub fn profile_capabilities(&self) -> ProfileCapabilities {
        ProfileCapabilities {
            allow_file_edits: self.allow_file_edits,
            command_access: self.command_access,
            allow_browser_diagnostics: self.allow_browser_diagnostics,
            allow_mutating_mcp_tools: self.allow_mutating_mcp_tools,
            mcp_enabled: self.mcp_enabled,
        }
    }

    pub fn to_policy_text(&self) -> String {
        let mut output = String::new();
        push_line(&mut output, "data_dir", &self.data_dir.to_string_lossy());
        push_line(
            &mut output,
            "max_file_bytes",
            &self.max_file_bytes.to_string(),
        );
        push_line(
            &mut output,
            "max_read_lines",
            &self.max_read_lines.to_string(),
        );
        push_line(
            &mut output,
            "max_list_entries",
            &self.max_list_entries.to_string(),
        );
        push_line(
            &mut output,
            "max_search_matches",
            &self.max_search_matches.to_string(),
        );
        push_line(
            &mut output,
            "max_match_line_chars",
            &self.max_match_line_chars.to_string(),
        );
        push_line(
            &mut output,
            "repository_map_max_tokens",
            &self.repository_map_max_tokens.to_string(),
        );
        push_line(
            &mut output,
            "symbol_context_max_tokens",
            &self.symbol_context_max_tokens.to_string(),
        );
        for (key, value) in context_budget_policy_lines(&self.context_budget) {
            push_line(&mut output, key, &format_basis_points(value));
        }
        push_line(
            &mut output,
            "max_command_output_bytes",
            &self.max_command_output_bytes.to_string(),
        );
        push_line(
            &mut output,
            "command_timeout_secs",
            &self.command_timeout_secs.to_string(),
        );
        push_line(
            &mut output,
            "allowed_roots",
            &join_paths(&self.allowed_roots),
        );
        push_line(
            &mut output,
            "ignore_patterns",
            &join_list(&self.ignore_patterns),
        );
        push_line(
            &mut output,
            "restricted_patterns",
            &join_list(&self.restricted_patterns),
        );
        push_line(
            &mut output,
            "command_allowlist",
            &join_list(&self.command_allowlist),
        );
        // Deliberately not the per-repository maps: this text answers "what
        // applies in this repository", and another checkout's `Allow Always`
        // grants and profile selection do not. They are visible in user config, which Settings shows
        // verbatim.
        push_line(
            &mut output,
            "command_blocklist",
            &join_list(&self.command_blocklist),
        );
        push_line(
            &mut output,
            "secret_patterns",
            &join_list(&self.secret_patterns),
        );
        push_line(
            &mut output,
            "require_approval_for_file_edits",
            &self.require_approval_for_file_edits.to_string(),
        );
        push_line(
            &mut output,
            "require_approval_for_risky_commands",
            &self.require_approval_for_risky_commands.to_string(),
        );
        push_line(
            &mut output,
            "require_approval_for_all_commands",
            &self.require_approval_for_all_commands.to_string(),
        );
        push_line(
            &mut output,
            "allow_file_edits",
            &self.allow_file_edits.to_string(),
        );
        push_line(&mut output, "command_access", self.command_access.as_str());
        push_line(
            &mut output,
            "allow_browser_diagnostics",
            &self.allow_browser_diagnostics.to_string(),
        );
        push_line(
            &mut output,
            "allow_mutating_mcp_tools",
            &self.allow_mutating_mcp_tools.to_string(),
        );
        push_line(
            &mut output,
            "block_generated_secrets",
            &self.block_generated_secrets.to_string(),
        );
        push_line(
            &mut output,
            "audit_enabled",
            &self.audit_enabled.to_string(),
        );
        push_line(
            &mut output,
            "audit_retention_days",
            &self.audit_retention_days.to_string(),
        );
        push_line(
            &mut output,
            "checkpoint_retention_days",
            &self.checkpoint_retention_days.to_string(),
        );
        push_line(
            &mut output,
            "checkpoint_max_total_bytes",
            &self.checkpoint_max_total_bytes.to_string(),
        );
        push_line(
            &mut output,
            "checkpoint_census_max_paths",
            &self.checkpoint_census_max_paths.to_string(),
        );
        push_line(
            &mut output,
            "enable_semantic_search",
            &self.enable_semantic_search.to_string(),
        );
        push_line(
            &mut output,
            "enable_worker_sessions",
            &self.enable_worker_sessions.to_string(),
        );
        push_line(
            &mut output,
            "agent_max_tool_rounds",
            &self.agent_max_tool_rounds.to_string(),
        );
        push_line(
            &mut output,
            "agent_max_exploration_rounds",
            &self.agent_max_exploration_rounds.to_string(),
        );
        push_line(
            &mut output,
            "agent_web_debug_max_tool_rounds",
            &self.agent_web_debug_max_tool_rounds.to_string(),
        );
        push_line(
            &mut output,
            "agent_tool_retry_limit",
            &self.agent_tool_retry_limit.to_string(),
        );
        push_line(
            &mut output,
            "agent_max_questions_per_turn",
            &self.agent_max_questions_per_turn.to_string(),
        );
        // Only when set. Printing `0` for "no ceiling" would read as a ceiling
        // of zero, which is a different and much worse configuration than the
        // default — and `config-show` is one of the two touch points the
        // exhaustive destructure does not catch (`context.md` §3.8).
        if let Some(value) = self.agent_max_task_tokens {
            push_line(&mut output, "agent_max_task_tokens", &value.to_string());
        }
        push_line(
            &mut output,
            "agent_max_turn_messages",
            &self.agent_max_turn_messages.to_string(),
        );
        push_line(
            &mut output,
            "agent_max_turn_notes_chars",
            &self.agent_max_turn_notes_chars.to_string(),
        );
        push_line(
            &mut output,
            "project_roots_added",
            &join_list(&self.project_roots_added),
        );
        push_line(
            &mut output,
            "project_roots_removed",
            &join_list(&self.project_roots_removed),
        );
        push_line(&mut output, "shell", &self.shell);
        push_line(&mut output, "model_provider", &self.model_provider);
        push_line(&mut output, "model_name", &self.model_name);
        push_line(&mut output, "model_base_url", &self.model_base_url);
        push_line(&mut output, "model_api_key_env", &self.model_api_key_env);
        push_line(
            &mut output,
            "model_reasoning_level",
            &self.model_reasoning_level,
        );
        for provider in &self.model_providers {
            push_model_provider_config(&mut output, provider);
        }
        push_line(&mut output, "mcp_enabled", &self.mcp_enabled.to_string());
        if !self.mcp_server_allowlist.is_empty() {
            push_line(
                &mut output,
                "mcp_server_allowlist",
                &join_list(&self.mcp_server_allowlist),
            );
        }
        for server in &self.mcp_servers {
            push_mcp_server_config(&mut output, server);
        }
        // Not the reviews: like the other per-repository maps, they are the
        // user's decisions about every checkout, not policy of this one.
        for hook in &self.hooks {
            push_hook_config(&mut output, hook);
        }
        for (language, command) in &self.language_servers {
            push_line(&mut output, &format!("language_server.{language}"), command);
        }
        output
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: Self::default_data_dir(),
            max_file_bytes: 1024 * 1024,
            // 400 lines keeps an unranged read well inside the 16k default
            // context budget; a bigger file is read by range instead.
            max_read_lines: 400,
            max_list_entries: 200,
            max_search_matches: 50,
            max_match_line_chars: 500,
            // Spec 26's planned 5% share of the 16,000-token default context
            // budget (spec 24 `context.md` §7).
            repository_map_max_tokens: 800,
            // Spec 26's planned 10% share of the same budget (spec 25
            // `context.md` §9).
            symbol_context_max_tokens: 1_600,
            context_budget: ContextBudget::default(),
            max_command_output_bytes: 1024 * 1024,
            command_timeout_secs: 600,
            allowed_roots: Vec::new(),
            ignore_patterns: DEFAULT_IGNORE_PATTERNS
                .iter()
                .map(|pattern| pattern.to_string())
                .collect(),
            restricted_patterns: DEFAULT_RESTRICTED_PATTERNS
                .iter()
                .map(|pattern| pattern.to_string())
                .collect(),
            command_allowlist: Vec::new(),
            command_allowlist_by_repository: BTreeMap::new(),
            permission_profile_by_repository: BTreeMap::new(),
            command_blocklist: Vec::new(),
            secret_patterns: Vec::new(),
            require_approval_for_file_edits: true,
            require_approval_for_risky_commands: true,
            require_approval_for_all_commands: false,
            allow_file_edits: true,
            command_access: CommandAccess::All,
            allow_browser_diagnostics: true,
            allow_mutating_mcp_tools: true,
            block_generated_secrets: true,
            audit_enabled: true,
            audit_retention_days: 90,
            checkpoint_retention_days: 90,
            checkpoint_max_total_bytes: 1_073_741_824,
            checkpoint_census_max_paths: 1_000,
            enable_semantic_search: false,
            enable_worker_sessions: false,
            enable_index_watcher: true,
            agent_max_tool_rounds: 8,
            agent_web_debug_max_tool_rounds: 12,
            agent_tool_retry_limit: 2,
            agent_max_questions_per_turn: 2,
            // Three times the action budget, so a discovery turn can read
            // roughly what the motivating session needed several times over
            // (spec 59 §5.4). The number is a starting point; the long-task
            // scenario is what checks it.
            agent_max_exploration_rounds: 24,
            // No ceiling by default: this must not change what an existing
            // configuration does on upgrade (§5.4).
            agent_max_task_tokens: None,
            agent_max_turn_messages: 24,
            // Roughly 2,000 tokens, under 2% of the request #59's motivating
            // session sent (spec 65 §5.7).
            agent_max_turn_notes_chars: 8_000,
            project_roots_added: Vec::new(),
            project_roots_removed: Vec::new(),
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string()),
            model_provider: "openai".to_string(),
            model_name: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4.1".to_string()),
            model_base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            model_api_key_env: "OPENAI_API_KEY".to_string(),
            model_reasoning_level: "default".to_string(),
            model_providers: Vec::new(),
            mcp_enabled: true,
            mcp_server_allowlist: Vec::new(),
            mcp_servers: Vec::new(),
            hooks: Vec::new(),
            hook_reviews_by_repository: BTreeMap::new(),
            // None: with nothing configured no server is ever spawned, and the
            // heuristic tier stands alone (proposal §5.4).
            language_servers: BTreeMap::new(),
        }
    }
}

// No `Eq`: it carries provider overlays, whose price rates are `Option<f64>`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConfigOverlay {
    pub data_dir: Option<PathBuf>,
    pub max_file_bytes: Option<u64>,
    pub max_read_lines: Option<usize>,
    pub max_list_entries: Option<usize>,
    pub max_search_matches: Option<usize>,
    pub max_match_line_chars: Option<usize>,
    pub repository_map_max_tokens: Option<usize>,
    pub symbol_context_max_tokens: Option<usize>,
    /// `context_budget.*` shares, in basis points. Each is its own field so the
    /// overlay macro classifies it and a repository raise is refused by name.
    pub context_budget_instructions: Option<u16>,
    pub context_budget_hook_context: Option<u16>,
    pub context_budget_pinned: Option<u16>,
    pub context_budget_plan_state: Option<u16>,
    pub context_budget_repository_map: Option<u16>,
    pub context_budget_symbols: Option<u16>,
    pub context_budget_search_results: Option<u16>,
    pub context_budget_file_ranges: Option<u16>,
    pub context_budget_findings: Option<u16>,
    pub context_budget_conversation_summary: Option<u16>,
    pub context_budget_safety_margin: Option<u16>,
    pub max_command_output_bytes: Option<usize>,
    pub command_timeout_secs: Option<usize>,
    pub allowed_roots: Option<Vec<PathBuf>>,
    pub ignore_patterns: Option<Vec<String>>,
    pub restricted_patterns: Option<Vec<String>>,
    pub command_allowlist: Option<Vec<String>>,
    /// `command_allowlist.<repository_id>` entries, from user or admin config.
    pub command_allowlist_by_repository: BTreeMap<String, Vec<String>>,
    /// `permission_profile.<repository_id>` entries, from user or admin config.
    pub permission_profile_by_repository: BTreeMap<String, ProfileId>,
    pub command_blocklist: Option<Vec<String>>,
    pub secret_patterns: Option<Vec<String>>,
    pub require_approval_for_file_edits: Option<bool>,
    pub require_approval_for_risky_commands: Option<bool>,
    pub require_approval_for_all_commands: Option<bool>,
    pub allow_file_edits: Option<bool>,
    pub command_access: Option<CommandAccess>,
    pub allow_browser_diagnostics: Option<bool>,
    pub allow_mutating_mcp_tools: Option<bool>,
    pub block_generated_secrets: Option<bool>,
    pub audit_enabled: Option<bool>,
    pub audit_retention_days: Option<u64>,
    pub checkpoint_retention_days: Option<u64>,
    pub checkpoint_max_total_bytes: Option<u64>,
    pub checkpoint_census_max_paths: Option<u64>,
    pub enable_semantic_search: Option<bool>,
    pub enable_worker_sessions: Option<bool>,
    pub agent_max_tool_rounds: Option<u32>,
    pub agent_web_debug_max_tool_rounds: Option<u32>,
    pub agent_tool_retry_limit: Option<u32>,
    pub agent_max_questions_per_turn: Option<u32>,
    pub agent_max_task_tokens: Option<u64>,
    pub agent_max_turn_messages: Option<usize>,
    pub agent_max_turn_notes_chars: Option<usize>,
    pub agent_max_exploration_rounds: Option<u32>,
    pub project_roots_added: Option<Vec<String>>,
    pub project_roots_removed: Option<Vec<String>>,
    pub shell: Option<String>,
    pub model_provider: Option<String>,
    pub model_name: Option<String>,
    pub model_base_url: Option<String>,
    pub model_api_key_env: Option<String>,
    pub model_reasoning_level: Option<String>,
    pub model_providers: Vec<ModelProviderConfigOverlay>,
    pub mcp_enabled: Option<bool>,
    pub mcp_server_allowlist: Option<Vec<String>>,
    pub mcp_servers: Vec<McpServerConfigOverlay>,
    pub hooks: Vec<HookConfigOverlay>,
    /// `hook_review.<repository_id>.<hook_id>` entries, from user or admin
    /// config.
    pub hook_reviews_by_repository: BTreeMap<String, BTreeMap<String, HookReview>>,
    /// `language_server.<language>` entries, by language.
    pub language_servers: BTreeMap<String, String>,
}

/// Which commands a permission profile lets run at all, narrowest first, so
/// `Ord` is the restriction order (spec 31, `context.md` §3). `CommandPolicy`
/// enforces it as a block, so a stored proposal run by id is refused too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandAccess {
    None,
    /// What Plan mode already allows: low risk, no approval, read-only.
    ReadOnly,
    /// Everything except a command Damaian recognises as networked. A name
    /// heuristic, not a sandbox (`context.md` §7).
    Local,
    All,
}

impl CommandAccess {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(CommandAccess::None),
            "read_only" => Some(CommandAccess::ReadOnly),
            "local" => Some(CommandAccess::Local),
            "all" => Some(CommandAccess::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CommandAccess::None => "none",
            CommandAccess::ReadOnly => "read_only",
            CommandAccess::Local => "local",
            CommandAccess::All => "all",
        }
    }
}

/// Whether a config key can grant or remove the ability to read, write,
/// execute, or reach the network (spec 31 §5.1). Derived from spec 34's
/// repository classes by one rule: a key repository scope does not apply
/// freely is a capability key (`docs/specs/31_permission_profiles/context.md`
/// §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKeyKind {
    Capability,
    Preference,
}

impl ConfigKeyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigKeyKind::Capability => "capability",
            ConfigKeyKind::Preference => "preference",
        }
    }
}

// Declares the partition once. The expansion destructures `ConfigOverlay`
// exhaustively, deliberately without `..`, and builds the list from the same
// names. So a new field fails to compile until it is classified, and the
// list cannot leave out a field the destructure names.
macro_rules! classify_overlay_fields {
    ($($field:ident => $kind:ident,)+) => {
        /// Every [`ConfigOverlay`] field, by field name, with its kind.
        pub fn overlay_field_kinds() -> Vec<(&'static str, ConfigKeyKind)> {
            let ConfigOverlay { $($field: _,)+ } = ConfigOverlay::default();
            vec![$((stringify!($field), ConfigKeyKind::$kind),)+]
        }
    };
}

classify_overlay_fields! {
    data_dir => Capability,
    max_file_bytes => Preference,
    max_read_lines => Capability,
    max_list_entries => Capability,
    max_search_matches => Capability,
    max_match_line_chars => Capability,
    repository_map_max_tokens => Capability,
    symbol_context_max_tokens => Capability,
    // Spec 26 §5.2: a share steers model traffic toward or away from a
    // repository's own content, so a clone may lower one but never raise it.
    // The safety margin is the reverse direction (higher is restrictive).
    context_budget_instructions => Capability,
    context_budget_hook_context => Capability,
    context_budget_pinned => Capability,
    context_budget_plan_state => Capability,
    context_budget_repository_map => Capability,
    context_budget_symbols => Capability,
    context_budget_search_results => Capability,
    context_budget_file_ranges => Capability,
    context_budget_findings => Capability,
    context_budget_conversation_summary => Capability,
    context_budget_safety_margin => Capability,
    max_command_output_bytes => Preference,
    command_timeout_secs => Capability,
    allowed_roots => Capability,
    ignore_patterns => Capability,
    restricted_patterns => Capability,
    command_allowlist => Capability,
    command_allowlist_by_repository => Capability,
    // User-owned at repository scope, like the allowlist map above.
    permission_profile_by_repository => Capability,
    command_blocklist => Capability,
    secret_patterns => Capability,
    require_approval_for_file_edits => Capability,
    require_approval_for_risky_commands => Capability,
    require_approval_for_all_commands => Capability,
    allow_file_edits => Capability,
    command_access => Capability,
    allow_browser_diagnostics => Capability,
    allow_mutating_mcp_tools => Capability,
    block_generated_secrets => Capability,
    audit_enabled => Capability,
    // Free at repository scope in spec 34, so a preference here. Whether a
    // clone should be able to shorten the audit trail is spec 34's
    // question, left open (context.md §2).
    audit_retention_days => Preference,
    checkpoint_retention_days => Capability,
    checkpoint_max_total_bytes => Capability,
    checkpoint_census_max_paths => Capability,
    enable_semantic_search => Preference,
    // Adds text to the user's system prompt; a clone must not (spec 57 §5.8).
    enable_worker_sessions => Capability,
    agent_max_tool_rounds => Preference,
    agent_web_debug_max_tool_rounds => Preference,
    agent_tool_retry_limit => Preference,
    // Asking widens no capability; validation holds it under a hard ceiling
    // (spec 50 `context.md` §8).
    agent_max_questions_per_turn => Preference,
    agent_max_task_tokens => Capability,
    agent_max_turn_messages => Capability,
    // Spec 65: restrict-only like the request window, and sent on every later
    // call including the next turn's, so a capability.
    agent_max_turn_notes_chars => Capability,
    // Spec 59: restrict-only, so a capability — a repository may lower the
    // read allowance, never raise it (§5.4).
    agent_max_exploration_rounds => Capability,
    // A root changes which directory an approved command runs in, never
    // whether it needs approval or what is readable, and committing a
    // manifest makes the same root (spec 24, context.md §6).
    project_roots_added => Preference,
    project_roots_removed => Preference,
    shell => Capability,
    model_provider => Capability,
    model_name => Capability,
    model_base_url => Capability,
    model_api_key_env => Capability,
    model_reasoning_level => Capability,
    model_providers => Capability,
    mcp_enabled => Capability,
    mcp_server_allowlist => Capability,
    mcp_servers => Capability,
    // A hook is an executable Damaian runs, so its definition is capability
    // whatever it can answer, and a review is the user's own decision about
    // one repository (spec 32, context.md §7.1).
    hooks => Capability,
    hook_reviews_by_repository => Capability,
    // A program Damaian spawns in the background, like `shell` (spec 25,
    // context.md §6).
    language_servers => Capability,
}

impl ConfigOverlay {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        Self::parse(&content)
    }

    pub fn parse(content: &str) -> Result<Self> {
        let mut overlay = Self::default();
        for (line_number, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((key, value)) = trimmed.split_once('=') else {
                return Err(ClientError::InvalidInput(format!(
                    "Invalid config line {}: expected key=value",
                    line_number + 1
                )));
            };
            overlay.set(key.trim(), value.trim())?;
        }
        Ok(overlay)
    }

    /// Parses config that arrived with a clone, where the content is untrusted
    /// input: a line that does not parse is collected and skipped instead of
    /// failing the file.
    ///
    /// Spec 34's principle is that a repository may restrict but never break,
    /// and failing the parse is breaking — the `?` on a whole-file parse lets
    /// any repository deny Damaian the config for that repository by shipping
    /// one bad line, and turns a typo in a shared repository config into an
    /// unexplained hard failure for everyone who clones it. The keys that did
    /// parse are still subject to the scope filter in
    /// [`Config::apply_overlay_scoped`]; this only decides which lines reach
    /// it. User and admin config keep the strict [`Self::parse`], because a
    /// broken key in the user's own file is a mistake to report, not untrusted
    /// input to tolerate.
    pub fn parse_untrusted(content: &str) -> (Self, Vec<RejectedConfigKey>) {
        let mut overlay = Self::default();
        let mut rejected = Vec::new();
        for (line_number, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((key, value)) = trimmed.split_once('=') else {
                // The line names no key, so it is reported by position. Its
                // text is repository-controlled and stays out of the report,
                // exactly as a refused value does.
                rejected.push(RejectedConfigKey::new(
                    format!("line {}", line_number + 1),
                    RepositoryKeyClass::Unparsable,
                ));
                continue;
            };
            // `set` can mutate before it fails: `model_provider.<id>.<field>`
            // pushes the provider entry before it matches the field. So a
            // failed line is rolled back, or the half-built entry survives and
            // is reported a second time as a phantom rejection of its own.
            let snapshot = overlay.clone();
            if overlay.set(key.trim(), value.trim()).is_err() {
                overlay = snapshot;
                rejected.push(RejectedConfigKey::new(
                    bounded_key_name(key.trim()),
                    RepositoryKeyClass::Unparsable,
                ));
            }
        }
        (overlay, rejected)
    }

    pub fn load_or_default(path: impl AsRef<Path>) -> Result<Self> {
        if path.as_ref().exists() {
            Self::load(path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, self.to_policy_text())?;
        Ok(())
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        if let Some(provider_key) = key.strip_prefix("model_provider.") {
            return self.set_model_provider_config(provider_key, value);
        }
        if let Some(server_key) = key.strip_prefix("mcp_server.") {
            return self.set_mcp_server_config(server_key, value);
        }
        if let Some(hook_key) = key.strip_prefix("hook.") {
            return self.set_hook_config(hook_key, value);
        }
        if let Some(review_key) = key.strip_prefix("hook_review.") {
            return self.set_hook_review(review_key, value);
        }
        if let Some(repository_id) = key.strip_prefix("command_allowlist.") {
            return self.set_repository_command_allowlist(repository_id, value);
        }
        if let Some(repository_id) = key.strip_prefix("permission_profile.") {
            return self.set_repository_permission_profile(repository_id, value);
        }
        if let Some(language) = key.strip_prefix("language_server.") {
            return self.set_language_server(language, value);
        }
        match key {
            "data_dir" => self.data_dir = Some(PathBuf::from(value)),
            "max_file_bytes" => self.max_file_bytes = Some(parse_u64(key, value)?),
            "max_read_lines" => self.max_read_lines = Some(parse_read_lines(key, value)?),
            "max_list_entries" => self.max_list_entries = Some(parse_read_lines(key, value)?),
            "max_search_matches" => self.max_search_matches = Some(parse_read_lines(key, value)?),
            "max_match_line_chars" => {
                self.max_match_line_chars = Some(parse_read_lines(key, value)?)
            }
            // `0` is valid: it turns the map off for model context.
            "repository_map_max_tokens" => {
                self.repository_map_max_tokens = Some(parse_u64(key, value)? as usize)
            }
            // `0` is valid here too: it leaves the `symbols` item out.
            "symbol_context_max_tokens" => {
                self.symbol_context_max_tokens = Some(parse_u64(key, value)? as usize)
            }
            // Spec 26 §5.2: decimals parsed into integer basis points.
            "context_budget.instructions" => {
                self.context_budget_instructions = Some(parse_basis_points(key, value)?)
            }
            "context_budget.hook_context" => {
                self.context_budget_hook_context = Some(parse_basis_points(key, value)?)
            }
            "context_budget.pinned" => {
                self.context_budget_pinned = Some(parse_basis_points(key, value)?)
            }
            "context_budget.plan_state" => {
                self.context_budget_plan_state = Some(parse_basis_points(key, value)?)
            }
            "context_budget.repository_map" => {
                self.context_budget_repository_map = Some(parse_basis_points(key, value)?)
            }
            "context_budget.symbols" => {
                self.context_budget_symbols = Some(parse_basis_points(key, value)?)
            }
            "context_budget.search_results" => {
                self.context_budget_search_results = Some(parse_basis_points(key, value)?)
            }
            "context_budget.file_ranges" => {
                self.context_budget_file_ranges = Some(parse_basis_points(key, value)?)
            }
            "context_budget.findings" => {
                self.context_budget_findings = Some(parse_basis_points(key, value)?)
            }
            "context_budget.conversation_summary" => {
                self.context_budget_conversation_summary = Some(parse_basis_points(key, value)?)
            }
            "context_budget.safety_margin" => {
                self.context_budget_safety_margin = Some(parse_basis_points(key, value)?)
            }
            "max_command_output_bytes" => {
                self.max_command_output_bytes = Some(parse_u64(key, value)? as usize)
            }
            "command_timeout_secs" => {
                self.command_timeout_secs = Some(parse_timeout_seconds(key, value)?)
            }
            "allowed_roots" => self.allowed_roots = Some(split_paths(value)),
            "ignore_patterns" => self.ignore_patterns = Some(split_list(value)),
            "restricted_patterns" => self.restricted_patterns = Some(split_list(value)),
            "command_allowlist" => self.command_allowlist = Some(split_list(value)),
            "command_blocklist" => self.command_blocklist = Some(split_list(value)),
            "secret_patterns" => self.secret_patterns = Some(split_list(value)),
            "require_approval_for_file_edits" => {
                self.require_approval_for_file_edits = Some(parse_bool(key, value)?)
            }
            "require_approval_for_risky_commands" => {
                self.require_approval_for_risky_commands = Some(parse_bool(key, value)?)
            }
            "require_approval_for_all_commands" => {
                self.require_approval_for_all_commands = Some(parse_bool(key, value)?)
            }
            "allow_file_edits" => self.allow_file_edits = Some(parse_bool(key, value)?),
            "command_access" => {
                self.command_access = Some(CommandAccess::parse(value).ok_or_else(|| {
                    ClientError::InvalidInput(format!(
                        "{key} must be none, read_only, local, or all"
                    ))
                })?)
            }
            "allow_browser_diagnostics" => {
                self.allow_browser_diagnostics = Some(parse_bool(key, value)?)
            }
            "allow_mutating_mcp_tools" => {
                self.allow_mutating_mcp_tools = Some(parse_bool(key, value)?)
            }
            "block_generated_secrets" => {
                self.block_generated_secrets = Some(parse_bool(key, value)?)
            }
            "audit_enabled" => self.audit_enabled = Some(parse_bool(key, value)?),
            "audit_retention_days" => self.audit_retention_days = Some(parse_u64(key, value)?),
            "checkpoint_retention_days" => {
                self.checkpoint_retention_days = Some(parse_u64(key, value)?)
            }
            "checkpoint_max_total_bytes" => {
                self.checkpoint_max_total_bytes = Some(parse_u64(key, value)?)
            }
            "checkpoint_census_max_paths" => {
                self.checkpoint_census_max_paths = Some(parse_u64(key, value)?)
            }
            "enable_semantic_search" => self.enable_semantic_search = Some(parse_bool(key, value)?),
            "enable_worker_sessions" => self.enable_worker_sessions = Some(parse_bool(key, value)?),
            "agent_max_tool_rounds" => {
                self.agent_max_tool_rounds = Some(parse_round_count(key, value)?)
            }
            "agent_web_debug_max_tool_rounds" => {
                self.agent_web_debug_max_tool_rounds = Some(parse_round_count(key, value)?)
            }
            "agent_tool_retry_limit" => {
                self.agent_tool_retry_limit = Some(parse_retry_limit(key, value)?)
            }
            "agent_max_questions_per_turn" => {
                self.agent_max_questions_per_turn = Some(parse_question_cap(key, value)?)
            }
            "agent_max_task_tokens" => {
                self.agent_max_task_tokens = Some(parse_task_token_ceiling(key, value)?)
            }
            "project_roots_added" => self.project_roots_added = Some(split_list(value)),
            "project_roots_removed" => self.project_roots_removed = Some(split_list(value)),
            "agent_max_turn_messages" => {
                self.agent_max_turn_messages = Some(parse_read_lines(key, value)?)
            }
            "agent_max_turn_notes_chars" => {
                self.agent_max_turn_notes_chars = Some(parse_turn_notes_chars(key, value)?)
            }
            "agent_max_exploration_rounds" => {
                self.agent_max_exploration_rounds = Some(parse_exploration_round_count(key, value)?)
            }
            "shell" => self.shell = Some(value.to_string()),
            "model_provider" => self.model_provider = Some(normalize_model_provider(value)?),
            "model_name" => self.model_name = Some(value.to_string()),
            "model_base_url" => self.model_base_url = Some(value.to_string()),
            "model_api_key_env" => {
                self.model_api_key_env = Some(parse_model_api_key_reference(value)?)
            }
            "model_reasoning_level" => {
                self.model_reasoning_level = Some(normalize_model_reasoning_level(value)?)
            }
            "mcp_enabled" => self.mcp_enabled = Some(parse_bool(key, value)?),
            "mcp_server_allowlist" => {
                self.mcp_server_allowlist = Some(
                    split_list(value)
                        .iter()
                        .map(|id| normalize_mcp_server_id(id))
                        .collect::<Result<Vec<_>>>()?,
                )
            }
            _ => {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown config key: {key}"
                )));
            }
        }
        Ok(())
    }

    /// `command_allowlist.<repository_id>=cmd|cmd` — the `Allow Always` grants
    /// for one repository. Valid in user and admin config; refused at
    /// repository scope when the overlay is applied.
    fn set_repository_command_allowlist(&mut self, repository_id: &str, value: &str) -> Result<()> {
        let repository_id = repository_id.trim();
        if !repository_id.starts_with("repo_") {
            return Err(ClientError::InvalidInput(format!(
                "Invalid repository id in config key: command_allowlist.{repository_id}"
            )));
        }
        self.command_allowlist_by_repository
            .insert(repository_id.to_string(), split_list(value));
        Ok(())
    }

    /// `permission_profile.<repository_id>=<profile id>` — the profile the
    /// user selected for one checkout. Refused at repository scope when the
    /// overlay is applied.
    fn set_repository_permission_profile(
        &mut self,
        repository_id: &str,
        value: &str,
    ) -> Result<()> {
        let repository_id = repository_id.trim();
        if !repository_id.starts_with("repo_") {
            return Err(ClientError::InvalidInput(format!(
                "Invalid repository id in config key: permission_profile.{repository_id}"
            )));
        }
        self.permission_profile_by_repository
            .insert(repository_id.to_string(), ProfileId::parse(value)?);
        Ok(())
    }

    /// `language_server.<language>=<command line>`. An unknown language is an
    /// error rather than an entry that is never used, so a typo is reported.
    /// Whether the scope may set it is decided when the overlay is applied.
    fn set_language_server(&mut self, language: &str, value: &str) -> Result<()> {
        let language = language.trim();
        if !LANGUAGE_SERVER_LANGUAGES.contains(&language) {
            return Err(ClientError::InvalidInput(format!(
                "Unknown language in config key: language_server.{language} (expected one of {})",
                LANGUAGE_SERVER_LANGUAGES.join(", ")
            )));
        }
        if value.trim().is_empty() {
            return Err(ClientError::InvalidInput(format!(
                "language_server.{language} must name a command"
            )));
        }
        self.language_servers
            .insert(language.to_string(), value.to_string());
        Ok(())
    }

    fn set_model_provider_config(&mut self, provider_key: &str, value: &str) -> Result<()> {
        let Some((raw_id, field)) = provider_key.rsplit_once('.') else {
            return Err(ClientError::InvalidInput(format!(
                "Invalid model provider config key: model_provider.{provider_key}"
            )));
        };
        let id = normalize_model_provider(raw_id)?;
        let index = if let Some(index) = self
            .model_providers
            .iter()
            .position(|provider| provider.id == id)
        {
            index
        } else {
            self.model_providers.push(ModelProviderConfigOverlay {
                id: id.clone(),
                ..ModelProviderConfigOverlay::default()
            });
            self.model_providers.len() - 1
        };
        let provider = &mut self.model_providers[index];
        match field {
            "label" => provider.label = Some(value.to_string()),
            "base_url" => provider.base_url = Some(value.trim_end_matches('/').to_string()),
            "api_key_env" => provider.api_key_env = Some(parse_model_api_key_reference(value)?),
            "models" => provider.models = Some(split_list(value)),
            "supports_native_tools" => {
                provider.supports_native_tools = Some(parse_bool(field, value)?)
            }
            "max_output_tokens" => {
                provider.max_output_tokens = Some(parse_token_count(provider_key, field, value)?);
            }
            "context_token_budget" => {
                provider.context_token_budget =
                    Some(parse_token_count(provider_key, field, value)?);
            }
            "provider_reports_usage" => {
                provider.provider_reports_usage = Some(parse_bool(field, value)?);
            }
            "price_per_million_input_tokens" => {
                provider.price_per_million_input_tokens = Some(parse_price(provider_key, value)?);
            }
            "price_per_million_output_tokens" => {
                provider.price_per_million_output_tokens = Some(parse_price(provider_key, value)?);
            }
            "price_per_million_cached_input_tokens" => {
                provider.price_per_million_cached_input_tokens =
                    Some(parse_price(provider_key, value)?);
            }
            "supports_explicit_cache_breakpoints" => {
                provider.supports_explicit_cache_breakpoints = Some(parse_bool(field, value)?);
            }
            _ => {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown model provider config key: model_provider.{provider_key}"
                )));
            }
        }
        Ok(())
    }

    fn set_mcp_server_config(&mut self, server_key: &str, value: &str) -> Result<()> {
        let Some((raw_id, field)) = server_key.rsplit_once('.') else {
            return Err(ClientError::InvalidInput(format!(
                "Invalid mcp server config key: mcp_server.{server_key}"
            )));
        };
        let id = normalize_mcp_server_id(raw_id)?;
        let index = if let Some(index) = self.mcp_servers.iter().position(|server| server.id == id)
        {
            index
        } else {
            self.mcp_servers.push(McpServerConfigOverlay {
                id: id.clone(),
                ..McpServerConfigOverlay::default()
            });
            self.mcp_servers.len() - 1
        };
        let server = &mut self.mcp_servers[index];
        match field {
            "label" => server.label = Some(value.to_string()),
            "transport" => server.transport = Some(parse_mcp_transport(value)?),
            "command" => server.command = Some(value.to_string()),
            "args" => server.args = Some(split_list(value)),
            "env" => server.env = Some(split_env(value)),
            "url" => server.url = Some(value.trim_end_matches('/').to_string()),
            "auth_token_env" => server.auth_token_env = Some(parse_model_api_key_reference(value)?),
            "enabled" => server.enabled = Some(parse_bool(field, value)?),
            "require_approval" => server.require_approval = Some(parse_bool(field, value)?),
            _ => {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown mcp server config key: mcp_server.{server_key}"
                )));
            }
        }
        Ok(())
    }

    /// `hook.<id>.<field>=value`. Whether the scope may set it is decided
    /// when the overlay is applied.
    fn set_hook_config(&mut self, hook_key: &str, value: &str) -> Result<()> {
        let Some((id, field)) = hook_key.rsplit_once('.') else {
            return Err(ClientError::InvalidInput(format!(
                "Invalid hook config key: hook.{hook_key}"
            )));
        };
        let id = validate_hook_id(id)?;
        // Parsed before the entry is created, so a bad value leaves nothing
        // behind even outside `parse_untrusted`'s rollback.
        let mut parsed = HookConfigOverlay::default();
        match field {
            "command" => {
                if value.is_empty() {
                    return Err(ClientError::InvalidInput(format!(
                        "hook.{id}.command must name an executable"
                    )));
                }
                parsed.command = Some(value.to_string());
            }
            "args" => parsed.args = Some(split_list(value)),
            "events" => {
                let events = split_list(value)
                    .iter()
                    .map(|name| {
                        HookEvent::parse(name).ok_or_else(|| {
                            ClientError::InvalidInput(format!(
                                "hook.{id}.events: unknown event {name}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                if events.is_empty() {
                    return Err(ClientError::InvalidInput(format!(
                        "hook.{id}.events must name at least one event"
                    )));
                }
                parsed.events = Some(events);
            }
            "mode" => {
                parsed.mode = Some(HookMode::parse(value).ok_or_else(|| {
                    ClientError::InvalidInput(format!(
                        "hook.{id}.mode must be mandatory or optional"
                    ))
                })?)
            }
            "order" => {
                parsed.order = Some(value.parse::<i64>().map_err(|_| {
                    ClientError::InvalidInput(format!("hook.{id}.order must be an integer"))
                })?)
            }
            "timeout_ms" => {
                let timeout = parse_u64(field, value)?;
                if timeout == 0 || timeout > MAX_HOOK_TIMEOUT_MS {
                    return Err(ClientError::InvalidInput(format!(
                        "hook.{id}.timeout_ms must be between 1 and {MAX_HOOK_TIMEOUT_MS}"
                    )));
                }
                parsed.timeout_ms = Some(timeout);
            }
            "enabled" => parsed.enabled = Some(parse_bool(field, value)?),
            _ => {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown hook config key: hook.{hook_key}"
                )));
            }
        }
        let index = match self.hooks.iter().position(|hook| hook.id == id) {
            Some(index) => index,
            None => {
                self.hooks.push(HookConfigOverlay {
                    id: id.clone(),
                    ..HookConfigOverlay::default()
                });
                self.hooks.len() - 1
            }
        };
        let hook = &mut self.hooks[index];
        let HookConfigOverlay {
            id: _,
            command,
            args,
            events,
            mode,
            order,
            timeout_ms,
            enabled,
        } = parsed;
        hook.command = command.or(hook.command.take());
        hook.args = args.or(hook.args.take());
        hook.events = events.or(hook.events.take());
        hook.mode = mode.or(hook.mode);
        hook.order = order.or(hook.order);
        hook.timeout_ms = timeout_ms.or(hook.timeout_ms);
        hook.enabled = enabled.or(hook.enabled);
        Ok(())
    }

    /// `hook_review.<repository_id>.<hook_id>=sha256:<hex>|declined` — the
    /// user's decision about one repository-defined hook. Refused at
    /// repository scope when the overlay is applied.
    fn set_hook_review(&mut self, review_key: &str, value: &str) -> Result<()> {
        let Some((repository_id, hook_id)) = review_key.rsplit_once('.') else {
            return Err(ClientError::InvalidInput(format!(
                "Invalid hook review key: hook_review.{review_key}"
            )));
        };
        if !repository_id.starts_with("repo_") {
            return Err(ClientError::InvalidInput(format!(
                "Invalid repository id in config key: hook_review.{review_key}"
            )));
        }
        let hook_id = validate_hook_id(hook_id)?;
        let review = HookReview::parse(value)?;
        self.hook_reviews_by_repository
            .entry(repository_id.to_string())
            .or_default()
            .insert(hook_id, review);
        Ok(())
    }

    pub fn to_policy_text(&self) -> String {
        // Exhaustive destructuring, deliberately without `..`: a field added to
        // `ConfigOverlay` fails to compile here, and one bound but never written
        // is an unused variable, which the lint gate rejects. Every `save()`
        // rewrites the whole file through this function, so a field it skips
        // is erased from the user's config by any unrelated write.
        let ConfigOverlay {
            data_dir,
            max_file_bytes,
            max_read_lines,
            max_list_entries,
            max_search_matches,
            max_match_line_chars,
            repository_map_max_tokens,
            symbol_context_max_tokens,
            context_budget_instructions,
            context_budget_hook_context,
            context_budget_pinned,
            context_budget_plan_state,
            context_budget_repository_map,
            context_budget_symbols,
            context_budget_search_results,
            context_budget_file_ranges,
            context_budget_findings,
            context_budget_conversation_summary,
            context_budget_safety_margin,
            max_command_output_bytes,
            command_timeout_secs,
            allowed_roots,
            ignore_patterns,
            restricted_patterns,
            command_allowlist,
            command_allowlist_by_repository,
            permission_profile_by_repository,
            command_blocklist,
            secret_patterns,
            require_approval_for_file_edits,
            require_approval_for_risky_commands,
            require_approval_for_all_commands,
            allow_file_edits,
            command_access,
            allow_browser_diagnostics,
            allow_mutating_mcp_tools,
            block_generated_secrets,
            audit_enabled,
            audit_retention_days,
            checkpoint_retention_days,
            checkpoint_max_total_bytes,
            checkpoint_census_max_paths,
            enable_semantic_search,
            enable_worker_sessions,
            agent_max_tool_rounds,
            agent_web_debug_max_tool_rounds,
            agent_tool_retry_limit,
            agent_max_questions_per_turn,
            agent_max_task_tokens,
            agent_max_turn_messages,
            agent_max_turn_notes_chars,
            agent_max_exploration_rounds,
            project_roots_added,
            project_roots_removed,
            shell,
            model_provider,
            model_name,
            model_base_url,
            model_api_key_env,
            model_reasoning_level,
            model_providers,
            mcp_enabled,
            mcp_server_allowlist,
            mcp_servers,
            hooks,
            hook_reviews_by_repository,
            language_servers,
        } = self;
        let mut output = String::new();
        if let Some(value) = data_dir {
            push_line(&mut output, "data_dir", &value.to_string_lossy());
        }
        if let Some(value) = max_read_lines {
            push_line(&mut output, "max_read_lines", &value.to_string());
        }
        if let Some(value) = max_list_entries {
            push_line(&mut output, "max_list_entries", &value.to_string());
        }
        if let Some(value) = max_search_matches {
            push_line(&mut output, "max_search_matches", &value.to_string());
        }
        if let Some(value) = max_match_line_chars {
            push_line(&mut output, "max_match_line_chars", &value.to_string());
        }
        if let Some(value) = repository_map_max_tokens {
            push_line(&mut output, "repository_map_max_tokens", &value.to_string());
        }
        if let Some(value) = symbol_context_max_tokens {
            push_line(&mut output, "symbol_context_max_tokens", &value.to_string());
        }
        for (key, value) in [
            ("context_budget.instructions", context_budget_instructions),
            ("context_budget.hook_context", context_budget_hook_context),
            ("context_budget.pinned", context_budget_pinned),
            ("context_budget.plan_state", context_budget_plan_state),
            (
                "context_budget.repository_map",
                context_budget_repository_map,
            ),
            ("context_budget.symbols", context_budget_symbols),
            (
                "context_budget.search_results",
                context_budget_search_results,
            ),
            ("context_budget.file_ranges", context_budget_file_ranges),
            ("context_budget.findings", context_budget_findings),
            (
                "context_budget.conversation_summary",
                context_budget_conversation_summary,
            ),
            ("context_budget.safety_margin", context_budget_safety_margin),
        ] {
            if let Some(value) = value {
                push_line(&mut output, key, &format_basis_points(*value));
            }
        }
        if let Some(value) = max_file_bytes {
            push_line(&mut output, "max_file_bytes", &value.to_string());
        }
        if let Some(value) = max_command_output_bytes {
            push_line(&mut output, "max_command_output_bytes", &value.to_string());
        }
        if let Some(value) = command_timeout_secs {
            push_line(&mut output, "command_timeout_secs", &value.to_string());
        }
        if let Some(value) = allowed_roots {
            push_line(&mut output, "allowed_roots", &join_paths(value));
        }
        if let Some(value) = ignore_patterns {
            push_line(&mut output, "ignore_patterns", &join_list(value));
        }
        if let Some(value) = restricted_patterns {
            push_line(&mut output, "restricted_patterns", &join_list(value));
        }
        if let Some(value) = command_allowlist {
            push_line(&mut output, "command_allowlist", &join_list(value));
        }
        for (repository_id, entries) in command_allowlist_by_repository {
            push_line(
                &mut output,
                &format!("command_allowlist.{repository_id}"),
                &join_list(entries),
            );
        }
        for (repository_id, profile) in permission_profile_by_repository {
            push_line(
                &mut output,
                &format!("permission_profile.{repository_id}"),
                profile.as_str(),
            );
        }
        if let Some(value) = command_blocklist {
            push_line(&mut output, "command_blocklist", &join_list(value));
        }
        if let Some(value) = secret_patterns {
            push_line(&mut output, "secret_patterns", &join_list(value));
        }
        if let Some(value) = require_approval_for_file_edits {
            push_line(
                &mut output,
                "require_approval_for_file_edits",
                &value.to_string(),
            );
        }
        if let Some(value) = require_approval_for_risky_commands {
            push_line(
                &mut output,
                "require_approval_for_risky_commands",
                &value.to_string(),
            );
        }
        if let Some(value) = require_approval_for_all_commands {
            push_line(
                &mut output,
                "require_approval_for_all_commands",
                &value.to_string(),
            );
        }
        if let Some(value) = allow_file_edits {
            push_line(&mut output, "allow_file_edits", &value.to_string());
        }
        if let Some(value) = command_access {
            push_line(&mut output, "command_access", value.as_str());
        }
        if let Some(value) = allow_browser_diagnostics {
            push_line(&mut output, "allow_browser_diagnostics", &value.to_string());
        }
        if let Some(value) = allow_mutating_mcp_tools {
            push_line(&mut output, "allow_mutating_mcp_tools", &value.to_string());
        }
        if let Some(value) = block_generated_secrets {
            push_line(&mut output, "block_generated_secrets", &value.to_string());
        }
        if let Some(value) = audit_enabled {
            push_line(&mut output, "audit_enabled", &value.to_string());
        }
        if let Some(value) = audit_retention_days {
            push_line(&mut output, "audit_retention_days", &value.to_string());
        }
        if let Some(value) = checkpoint_retention_days {
            push_line(&mut output, "checkpoint_retention_days", &value.to_string());
        }
        if let Some(value) = checkpoint_max_total_bytes {
            push_line(
                &mut output,
                "checkpoint_max_total_bytes",
                &value.to_string(),
            );
        }
        if let Some(value) = checkpoint_census_max_paths {
            push_line(
                &mut output,
                "checkpoint_census_max_paths",
                &value.to_string(),
            );
        }
        if let Some(value) = enable_semantic_search {
            push_line(&mut output, "enable_semantic_search", &value.to_string());
        }
        if let Some(value) = enable_worker_sessions {
            push_line(&mut output, "enable_worker_sessions", &value.to_string());
        }
        if let Some(value) = agent_max_tool_rounds {
            push_line(&mut output, "agent_max_tool_rounds", &value.to_string());
        }
        if let Some(value) = agent_max_exploration_rounds {
            push_line(
                &mut output,
                "agent_max_exploration_rounds",
                &value.to_string(),
            );
        }
        if let Some(value) = agent_web_debug_max_tool_rounds {
            push_line(
                &mut output,
                "agent_web_debug_max_tool_rounds",
                &value.to_string(),
            );
        }
        if let Some(value) = agent_max_task_tokens {
            push_line(&mut output, "agent_max_task_tokens", &value.to_string());
        }
        if let Some(value) = agent_tool_retry_limit {
            push_line(&mut output, "agent_tool_retry_limit", &value.to_string());
        }
        if let Some(value) = agent_max_questions_per_turn {
            push_line(
                &mut output,
                "agent_max_questions_per_turn",
                &value.to_string(),
            );
        }
        if let Some(value) = agent_max_turn_messages {
            push_line(&mut output, "agent_max_turn_messages", &value.to_string());
        }
        if let Some(value) = agent_max_turn_notes_chars {
            push_line(
                &mut output,
                "agent_max_turn_notes_chars",
                &value.to_string(),
            );
        }
        if let Some(value) = project_roots_added {
            push_line(&mut output, "project_roots_added", &join_list(value));
        }
        if let Some(value) = project_roots_removed {
            push_line(&mut output, "project_roots_removed", &join_list(value));
        }
        if let Some(value) = shell {
            push_line(&mut output, "shell", value);
        }
        if let Some(value) = model_provider {
            push_line(&mut output, "model_provider", value);
        }
        if let Some(value) = model_name {
            push_line(&mut output, "model_name", value);
        }
        if let Some(value) = model_base_url {
            push_line(&mut output, "model_base_url", value);
        }
        if let Some(value) = model_api_key_env {
            push_line(&mut output, "model_api_key_env", value);
        }
        if let Some(value) = model_reasoning_level {
            push_line(&mut output, "model_reasoning_level", value);
        }
        for provider in model_providers {
            push_model_provider_overlay(&mut output, provider);
        }
        if let Some(value) = mcp_enabled {
            push_line(&mut output, "mcp_enabled", &value.to_string());
        }
        if let Some(value) = mcp_server_allowlist {
            push_line(&mut output, "mcp_server_allowlist", &join_list(value));
        }
        for server in mcp_servers {
            push_mcp_server_overlay(&mut output, server);
        }
        for hook in hooks {
            push_hook_overlay(&mut output, hook);
        }
        for (repository_id, reviews) in hook_reviews_by_repository {
            for (hook_id, review) in reviews {
                push_line(
                    &mut output,
                    &format!("hook_review.{repository_id}.{hook_id}"),
                    review.as_str(),
                );
            }
        }
        for (language, command) in language_servers {
            push_line(&mut output, &format!("language_server.{language}"), command);
        }
        output
    }
}

fn hook_events_text(events: &[HookEvent]) -> String {
    events
        .iter()
        .map(|event| event.as_str())
        .collect::<Vec<_>>()
        .join("|")
}

fn push_hook_config(output: &mut String, hook: &HookConfig) {
    let id = &hook.id;
    push_line(output, &format!("hook.{id}.command"), &hook.command);
    if !hook.args.is_empty() {
        push_line(output, &format!("hook.{id}.args"), &join_list(&hook.args));
    }
    push_line(
        output,
        &format!("hook.{id}.events"),
        &hook_events_text(&hook.events),
    );
    push_line(output, &format!("hook.{id}.mode"), hook.mode.as_str());
    push_line(output, &format!("hook.{id}.order"), &hook.order.to_string());
    if let Some(value) = hook.timeout_ms {
        push_line(output, &format!("hook.{id}.timeout_ms"), &value.to_string());
    }
    push_line(
        output,
        &format!("hook.{id}.enabled"),
        &hook.enabled.to_string(),
    );
}

fn push_hook_overlay(output: &mut String, hook: &HookConfigOverlay) {
    let HookConfigOverlay {
        id,
        command,
        args,
        events,
        mode,
        order,
        timeout_ms,
        enabled,
    } = hook;
    if let Some(value) = command {
        push_line(output, &format!("hook.{id}.command"), value);
    }
    if let Some(value) = args {
        push_line(output, &format!("hook.{id}.args"), &join_list(value));
    }
    if let Some(value) = events {
        push_line(
            output,
            &format!("hook.{id}.events"),
            &hook_events_text(value),
        );
    }
    if let Some(value) = mode {
        push_line(output, &format!("hook.{id}.mode"), value.as_str());
    }
    if let Some(value) = order {
        push_line(output, &format!("hook.{id}.order"), &value.to_string());
    }
    if let Some(value) = timeout_ms {
        push_line(output, &format!("hook.{id}.timeout_ms"), &value.to_string());
    }
    if let Some(value) = enabled {
        push_line(output, &format!("hook.{id}.enabled"), &value.to_string());
    }
}

fn builtin_model_provider_config(id: &str) -> Option<ModelProviderConfig> {
    match id {
        "openai" => Some(ModelProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            api_key_env: "OPENAI_API_KEY".to_string(),
            models: vec![
                std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4.1".to_string()),
                "gpt-4.1-mini".to_string(),
                "o4-mini".to_string(),
            ],
            supports_native_tools: true,
            max_output_tokens: None,
            context_token_budget: None,
            provider_reports_usage: true,
            price_per_million_input_tokens: None,
            price_per_million_output_tokens: None,
            price_per_million_cached_input_tokens: None,
            supports_explicit_cache_breakpoints: false,
        }),
        "deepseek" => Some(ModelProviderConfig {
            id: "deepseek".to_string(),
            label: "DeepSeek".to_string(),
            base_url: std::env::var("DEEPSEEK_BASE_URL")
                .unwrap_or_else(|_| "https://api.deepseek.com".to_string()),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            models: vec![
                std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".to_string()),
                "deepseek-v4-pro".to_string(),
            ],
            supports_native_tools: true,
            // Fallback for a DeepSeek model not in `builtin_model_output_tokens`.
            // Deliberately the conservative legacy ceiling: an unrecognized
            // model may still be a legacy alias, and a too-small ceiling costs
            // an extra round trip whereas a too-large one is a hard API error.
            max_output_tokens: Some(8192),
            context_token_budget: None,
            provider_reports_usage: true,
            price_per_million_input_tokens: None,
            price_per_million_output_tokens: None,
            price_per_million_cached_input_tokens: None,
            supports_explicit_cache_breakpoints: false,
        }),
        "openrouter" => Some(ModelProviderConfig {
            id: "openrouter".to_string(),
            label: "OpenRouter".to_string(),
            // OpenRouter publishes `/api/v1` as its base. The transport strips
            // a trailing `/v1` before appending `/v1/chat/completions`, so
            // both this and the bare `/api` form work.
            base_url: std::env::var("OPENROUTER_BASE_URL")
                .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string()),
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            models: vec![
                std::env::var("OPENROUTER_MODEL")
                    .unwrap_or_else(|_| "anthropic/claude-sonnet-5.5".to_string()),
                "anthropic/claude-opus-5.5".to_string(),
                "anthropic/claude-haiku-5.5".to_string(),
                "openai/gpt-6.1-sol".to_string(),
                "google/gemini-3.8-flash".to_string(),
                "deepseek/deepseek-v4.1-flash".to_string(),
                "x-ai/grok-4.7".to_string(),
                "moonshotai/kimi-k3.1-pro".to_string(),
                "z-ai/glm-5.3".to_string(),
                "qwen/qwen3.8-max-prime".to_string(),
            ],
            supports_native_tools: true,
            max_output_tokens: None,
            context_token_budget: None,
            provider_reports_usage: true,
            price_per_million_input_tokens: None,
            price_per_million_output_tokens: None,
            price_per_million_cached_input_tokens: None,
            supports_explicit_cache_breakpoints: false,
        }),
        "openai-compatible" => Some(ModelProviderConfig {
            id: "openai-compatible".to_string(),
            label: "Custom".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key_env: "OPENAI_API_KEY".to_string(),
            models: vec!["configured-model".to_string()],
            supports_native_tools: true,
            max_output_tokens: None,
            context_token_budget: None,
            provider_reports_usage: true,
            price_per_million_input_tokens: None,
            price_per_million_output_tokens: None,
            price_per_million_cached_input_tokens: None,
            supports_explicit_cache_breakpoints: false,
        }),
        _ => None,
    }
}

/// Built-in `max_tokens` per model, consulted when the provider config
/// doesn't pin one. Keyed by model rather than provider because a single
/// provider serves models with wildly different limits: DeepSeek's retired
/// `deepseek-chat`/`deepseek-reasoner` aliases cap at 8192, while the V4
/// models accept up to 384000.
///
/// The V4 entries deliberately sit well below that 384000 maximum. `max_tokens`
/// is only a ceiling — you're billed for what's generated, not what's
/// reserved — but it's also the sole bound on a runaway generation, and 64k
/// output already covers any realistic multi-file patch. Raise it per install
/// with `model_provider.deepseek.max_output_tokens`, which takes precedence
/// over everything here.
fn builtin_model_output_tokens(provider: &str, model: &str) -> Option<u32> {
    match (provider, model) {
        ("deepseek", "deepseek-v4-flash" | "deepseek-v4-pro") => Some(65_536),
        // Retired 2026-07-24 and served only as compatibility aliases; kept
        // here so an un-migrated config still gets a correct ceiling instead
        // of an over-large one the legacy endpoint would reject.
        ("deepseek", "deepseek-chat" | "deepseek-reasoner") => Some(8_192),
        _ => None,
    }
}

/// Built-in repository-context budget per model, consulted when the provider
/// config doesn't pin one.
///
/// These sit far below what each model's context window technically allows —
/// v4-flash accepts 1M tokens, and this grants 64k. Context is re-sent and
/// re-billed on every single turn, so the budget is tuned to "enough of the
/// repository to answer well" rather than "everything that fits". Installs
/// that want more can raise `model_provider.<id>.context_token_budget`.
fn builtin_model_context_budget(provider: &str, model: &str) -> Option<u32> {
    match (provider, model) {
        ("deepseek", "deepseek-v4-flash" | "deepseek-v4-pro") => Some(64_000),
        _ => None,
    }
}

fn parse_token_count(provider_key: &str, field: &str, value: &str) -> Result<u32> {
    let parsed = parse_u64(field, value)?;
    if parsed == 0 || parsed > u32::MAX as u64 {
        return Err(ClientError::InvalidInput(format!(
            "model_provider.{provider_key} must be between 1 and {}",
            u32::MAX
        )));
    }
    Ok(parsed as u32)
}

/// A price per million tokens, in the user's own currency.
///
/// Rejects a negative or non-numeric value rather than clamping: a price the
/// user mistyped should be a loud error, not a quietly wrong bill. Zero is
/// allowed — a locally hosted model genuinely costs nothing per token.
fn parse_price(provider_key: &str, value: &str) -> Result<f64> {
    let parsed: f64 = value.trim().parse().map_err(|_| {
        ClientError::InvalidInput(format!(
            "model_provider.{provider_key} must be a number, got: {value}"
        ))
    })?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(ClientError::InvalidInput(format!(
            "model_provider.{provider_key} must be zero or more"
        )));
    }
    Ok(parsed)
}

fn is_builtin_model_provider(id: &str) -> bool {
    matches!(
        id,
        "openai" | "deepseek" | "openrouter" | "openai-compatible"
    )
}

fn push_model_provider_config(output: &mut String, provider: &ModelProviderConfig) {
    push_line(
        output,
        &format!("model_provider.{}.label", provider.id),
        &provider.label,
    );
    push_line(
        output,
        &format!("model_provider.{}.base_url", provider.id),
        &provider.base_url,
    );
    push_line(
        output,
        &format!("model_provider.{}.api_key_env", provider.id),
        &provider.api_key_env,
    );
    push_line(
        output,
        &format!("model_provider.{}.models", provider.id),
        &join_list(&provider.models),
    );
    push_line(
        output,
        &format!("model_provider.{}.supports_native_tools", provider.id),
        &provider.supports_native_tools.to_string(),
    );
    if let Some(value) = provider.max_output_tokens {
        push_line(
            output,
            &format!("model_provider.{}.max_output_tokens", provider.id),
            &value.to_string(),
        );
    }
    if let Some(value) = provider.context_token_budget {
        push_line(
            output,
            &format!("model_provider.{}.context_token_budget", provider.id),
            &value.to_string(),
        );
    }
}

fn push_model_provider_overlay(output: &mut String, provider: &ModelProviderConfigOverlay) {
    // Exhaustive destructuring, deliberately without `..`, and every binding is
    // used below — the same guard `apply_overlay_scoped` uses, for the same
    // reason. A field added to the overlay and not written here compiles
    // cleanly and silently loses the user's setting on the next save. That is
    // exactly what happened to `provider_reports_usage` and the two price
    // keys: `set` accepted them, `save` dropped them, and `config-set`
    // reported success either way. Adding a field now fails the build instead.
    let ModelProviderConfigOverlay {
        id,
        label,
        base_url,
        api_key_env,
        models,
        supports_native_tools,
        max_output_tokens,
        context_token_budget,
        provider_reports_usage,
        price_per_million_input_tokens,
        price_per_million_output_tokens,
        price_per_million_cached_input_tokens,
        supports_explicit_cache_breakpoints,
    } = provider;

    if let Some(value) = label {
        push_line(output, &format!("model_provider.{id}.label"), value);
    }
    if let Some(value) = base_url {
        push_line(output, &format!("model_provider.{id}.base_url"), value);
    }
    if let Some(value) = api_key_env {
        push_line(output, &format!("model_provider.{id}.api_key_env"), value);
    }
    if let Some(value) = models {
        push_line(
            output,
            &format!("model_provider.{id}.models"),
            &join_list(value),
        );
    }
    if let Some(value) = supports_native_tools {
        push_line(
            output,
            &format!("model_provider.{id}.supports_native_tools"),
            &value.to_string(),
        );
    }
    if let Some(value) = max_output_tokens {
        push_line(
            output,
            &format!("model_provider.{id}.max_output_tokens"),
            &value.to_string(),
        );
    }
    if let Some(value) = context_token_budget {
        push_line(
            output,
            &format!("model_provider.{id}.context_token_budget"),
            &value.to_string(),
        );
    }
    if let Some(value) = provider_reports_usage {
        push_line(
            output,
            &format!("model_provider.{id}.provider_reports_usage"),
            &value.to_string(),
        );
    }
    if let Some(value) = price_per_million_input_tokens {
        push_line(
            output,
            &format!("model_provider.{id}.price_per_million_input_tokens"),
            &value.to_string(),
        );
    }
    if let Some(value) = price_per_million_output_tokens {
        push_line(
            output,
            &format!("model_provider.{id}.price_per_million_output_tokens"),
            &value.to_string(),
        );
    }
    if let Some(value) = price_per_million_cached_input_tokens {
        push_line(
            output,
            &format!("model_provider.{id}.price_per_million_cached_input_tokens"),
            &value.to_string(),
        );
    }
    if let Some(value) = supports_explicit_cache_breakpoints {
        push_line(
            output,
            &format!("model_provider.{id}.supports_explicit_cache_breakpoints"),
            &value.to_string(),
        );
    }
}

fn push_mcp_server_config(output: &mut String, server: &McpServerConfig) {
    let id = &server.id;
    push_line(output, &format!("mcp_server.{id}.label"), &server.label);
    push_line(
        output,
        &format!("mcp_server.{id}.transport"),
        server.transport.as_str(),
    );
    match server.transport {
        McpTransport::Stdio => {
            push_line(output, &format!("mcp_server.{id}.command"), &server.command);
            push_line(
                output,
                &format!("mcp_server.{id}.args"),
                &join_list(&server.args),
            );
            push_line(
                output,
                &format!("mcp_server.{id}.env"),
                &join_env(&server.env),
            );
        }
        McpTransport::Http => {
            push_line(output, &format!("mcp_server.{id}.url"), &server.url);
            push_line(
                output,
                &format!("mcp_server.{id}.auth_token_env"),
                &server.auth_token_env,
            );
        }
    }
    push_line(
        output,
        &format!("mcp_server.{id}.enabled"),
        &server.enabled.to_string(),
    );
    push_line(
        output,
        &format!("mcp_server.{id}.require_approval"),
        &server.require_approval.to_string(),
    );
}

fn push_mcp_server_overlay(output: &mut String, server: &McpServerConfigOverlay) {
    let id = &server.id;
    if let Some(value) = &server.label {
        push_line(output, &format!("mcp_server.{id}.label"), value);
    }
    if let Some(value) = server.transport {
        push_line(
            output,
            &format!("mcp_server.{id}.transport"),
            value.as_str(),
        );
    }
    if let Some(value) = &server.command {
        push_line(output, &format!("mcp_server.{id}.command"), value);
    }
    if let Some(value) = &server.args {
        push_line(output, &format!("mcp_server.{id}.args"), &join_list(value));
    }
    if let Some(value) = &server.env {
        push_line(output, &format!("mcp_server.{id}.env"), &join_env(value));
    }
    if let Some(value) = &server.url {
        push_line(output, &format!("mcp_server.{id}.url"), value);
    }
    if let Some(value) = &server.auth_token_env {
        push_line(output, &format!("mcp_server.{id}.auth_token_env"), value);
    }
    if let Some(value) = server.enabled {
        push_line(
            output,
            &format!("mcp_server.{id}.enabled"),
            &value.to_string(),
        );
    }
    if let Some(value) = server.require_approval {
        push_line(
            output,
            &format!("mcp_server.{id}.require_approval"),
            &value.to_string(),
        );
    }
}

/// Validates and normalizes an MCP server id. Kept to the same charset as
/// model-provider ids (lowercase alphanumeric plus `-`/`.`); notably `_` is
/// disallowed so the `mcp__<id>__<tool>` tool namespace stays unambiguous to
/// split on `__`.
pub fn normalize_mcp_server_id(value: &str) -> Result<String> {
    let id = value.trim().to_ascii_lowercase();
    if id.is_empty() {
        return Err(ClientError::InvalidInput(
            "mcp server id is required".to_string(),
        ));
    }
    if id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
    {
        Ok(id)
    } else {
        Err(ClientError::InvalidInput(
            "mcp server id can contain only letters, numbers, dots, and dashes".to_string(),
        ))
    }
}

/// A hook id is the custom-profile id rule, `[a-z0-9_]{1,40}`: it is printed
/// in the policy view and in audit events, so it stays short and plain.
fn validate_hook_id(id: &str) -> Result<String> {
    if !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|character| matches!(character, 'a'..='z' | '0'..='9' | '_'))
    {
        Ok(id.to_string())
    } else {
        Err(ClientError::InvalidInput(
            "a hook id is 1 to 40 lowercase letters, digits or underscores".to_string(),
        ))
    }
}

pub fn parse_mcp_transport(value: &str) -> Result<McpTransport> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "stdio" | "local" => Ok(McpTransport::Stdio),
        "http" | "https" | "sse" | "streamable-http" | "remote" => Ok(McpTransport::Http),
        _ => Err(ClientError::InvalidInput(
            "mcp server transport must be stdio or http".to_string(),
        )),
    }
}

fn split_env(value: &str) -> Vec<(String, String)> {
    value
        .split('|')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .filter_map(|item| {
            item.split_once('=')
                .map(|(key, val)| (key.trim().to_string(), val.trim().to_string()))
        })
        .collect()
}

fn join_env(values: &[(String, String)]) -> String {
    values
        .iter()
        .map(|(key, val)| format!("{key}={val}"))
        .collect::<Vec<_>>()
        .join("|")
}

pub fn normalize_model_provider(value: &str) -> Result<String> {
    let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
    let provider = match normalized.as_str() {
        "open-ai" | "openai" => "openai".to_string(),
        "deep-seek" | "deepseek" | "deedseek" => "deepseek".to_string(),
        "open-router" | "openrouter" => "openrouter".to_string(),
        "custom" | "openai-compatible" | "open-ai-compatible" => "openai-compatible".to_string(),
        "" => {
            return Err(ClientError::InvalidInput(
                "model_provider is required".to_string(),
            ));
        }
        other => other.to_string(),
    };
    if provider
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
    {
        Ok(provider)
    } else {
        Err(ClientError::InvalidInput(
            "model_provider can contain only letters, numbers, dots, and dashes".to_string(),
        ))
    }
}

pub fn normalize_model_reasoning_level(value: &str) -> Result<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "default" | "auto" => Ok("default".to_string()),
        "minimal" | "low" | "medium" | "high" => Ok(value.trim().to_ascii_lowercase()),
        _ => Err(ClientError::InvalidInput(
            "model_reasoning_level must be default, minimal, low, medium, or high".to_string(),
        )),
    }
}

/// The repository a repository-config path belongs to: the inverse of
/// [`Config::repository_config_path`]. `None` for any other layout.
fn repository_root_from_config_path(path: &Path) -> Option<&Path> {
    if path.file_name()? != "config.conf" {
        return None;
    }
    let damaian = path.parent()?;
    if damaian.file_name()? != ".damaian" {
        return None;
    }
    damaian.parent()
}

/// A key only a trusted scope may set. An untrusted value is dropped and
/// recorded under `class`.
fn scoped<T>(
    value: Option<T>,
    key: &str,
    trusted: bool,
    class: RepositoryKeyClass,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<T> {
    match value {
        Some(value) if trusted => Some(value),
        Some(_) => {
            rejected.push(RejectedConfigKey::new(key, class));
            None
        }
        None => None,
    }
}

/// A preference key. Every scope but a profile applies it as given; a profile
/// narrows capability and does not reconfigure Damaian, so there it is
/// refused (spec 31, `context.md` §4).
fn preference<T>(
    value: Option<T>,
    key: &str,
    scope: ConfigScope,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<T> {
    match scope {
        ConfigScope::User | ConfigScope::Repository | ConfigScope::Admin => value,
        ConfigScope::Profile => {
            if value.is_some() {
                rejected.push(RejectedConfigKey::new(key, RepositoryKeyClass::Forbidden));
            }
            None
        }
    }
}

/// A retention window a profile may only shorten. A profile value is an upper
/// bound ("keep at most this long"), so a higher one is the documented merge
/// rather than a refusal, and nothing is recorded — as with `union_patterns`.
///
/// Returns whether the profile's value is what now holds: lower, or equal.
fn lower_wins(current: &mut u64, value: u64) -> bool {
    if value < *current {
        *current = value;
    }
    value == *current
}

/// A list of restrictions. A trusted scope replaces it; an untrusted one may
/// only append, so a repository listing fewer patterns than the user removes
/// none of them. Appending is the documented merge, not a refusal, so nothing
/// is recorded: a repository config that names only its own additions is the
/// normal, legitimate case.
///
/// Returns the entries this scope now holds in force, and whether a trusted
/// scope removed an entry, which is the loosening an untrusted one cannot do.
/// An untrusted scope listing nothing holds nothing, so returns `None`.
fn union_patterns(
    current: &mut Vec<String>,
    incoming: Vec<String>,
    trusted: bool,
) -> Option<(Vec<String>, bool)> {
    if trusted {
        let removed = current.iter().any(|entry| !incoming.contains(entry));
        *current = incoming.clone();
        return Some((incoming, removed));
    }
    if incoming.is_empty() {
        return None;
    }
    for entry in &incoming {
        if !current.contains(entry) {
            current.push(entry.clone());
        }
    }
    Some((incoming, false))
}

/// A boolean an untrusted scope may only push toward `restrictive`. The other
/// direction is a weakening attempt: ignored, and recorded when it would
/// actually have changed something.
///
/// Returns `Some(loosened)` when the value was applied, where `loosened` is
/// the move an untrusted scope would have been refused; `None` when not.
fn restrict_only_flag(
    current: &mut bool,
    incoming: bool,
    restrictive: bool,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<bool> {
    if trusted || incoming == restrictive {
        let loosens = incoming != restrictive && *current != incoming;
        *current = incoming;
        return Some(loosens);
    }
    // An untrusted value equal to the current one is a no-op: not refused,
    // and not what holds the value either.
    if *current != incoming {
        rejected.push(RejectedConfigKey::new(
            key,
            RepositoryKeyClass::RestrictOnly,
        ));
    }
    None
}

/// [`CommandAccess`] has four levels rather than two, so it cannot use
/// [`restrict_only_flag`]: an untrusted scope may move it toward
/// `CommandAccess::None`, never toward `CommandAccess::All`.
fn restrict_only_access(
    current: &mut CommandAccess,
    incoming: CommandAccess,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<bool> {
    let loosens = incoming > *current;
    if trusted || !loosens {
        *current = incoming;
        return Some(loosens);
    }
    rejected.push(RejectedConfigKey::new(
        key,
        RepositoryKeyClass::RestrictOnly,
    ));
    None
}

/// An allowlist where empty means "no restriction". An untrusted scope may
/// narrow it — set one where there was none, or intersect an existing one —
/// but never add an id or clear it.
///
/// Returns the ids this scope now holds in force — after a narrowing, only the
/// ones it shared with the list it narrowed, so an id it was refused is never
/// reported as applied — and whether a trusted scope loosened the list.
fn intersect_allowlist(
    current: &mut Vec<String>,
    incoming: Vec<String>,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<(Vec<String>, bool)> {
    if trusted {
        let loosens = !current.is_empty()
            && (incoming.is_empty() || incoming.iter().any(|id| !current.contains(id)));
        *current = incoming.clone();
        return Some((incoming, loosens));
    }
    if incoming.is_empty() {
        if !current.is_empty() {
            rejected.push(RejectedConfigKey::new(
                key,
                RepositoryKeyClass::RestrictOnly,
            ));
        }
        return None;
    }
    if current.is_empty() {
        *current = incoming.clone();
        return Some((incoming, false));
    }
    if incoming.iter().any(|id| !current.contains(id)) {
        rejected.push(RejectedConfigKey::new(
            key,
            RepositoryKeyClass::RestrictOnly,
        ));
    }
    // An empty intersection cannot be stored: empty means "no restriction"
    // here, so writing it back would widen rather than narrow. A repository
    // whose list shares nothing with the user's is substituting its own, not
    // narrowing theirs, so the user's list stands.
    if !current.iter().any(|id| incoming.contains(id)) {
        return None;
    }
    current.retain(|id| incoming.contains(id));
    Some((current.clone(), false))
}

fn push_line(output: &mut String, key: &str, value: &str) {
    output.push_str(key);
    output.push('=');
    output.push_str(value);
    output.push('\n');
}

/// The resolved `context_budget.*` lines, in the order proposal §5.2 lists
/// them. `Config::to_policy_text` and the effective-policy view read the same
/// names.
fn context_budget_policy_lines(budget: &ContextBudget) -> Vec<(&'static str, u16)> {
    let mut lines = vec![
        (
            "context_budget.instructions",
            budget.share(ContextCategory::Instructions),
        ),
        (
            "context_budget.hook_context",
            budget.share(ContextCategory::HookContext),
        ),
        (
            "context_budget.pinned",
            budget.share(ContextCategory::Pinned),
        ),
        (
            "context_budget.plan_state",
            budget.share(ContextCategory::PlanState),
        ),
        (
            "context_budget.repository_map",
            budget.share(ContextCategory::RepositoryMap),
        ),
        (
            "context_budget.symbols",
            budget.share(ContextCategory::Symbols),
        ),
        (
            "context_budget.search_results",
            budget.share(ContextCategory::SearchResults),
        ),
        (
            "context_budget.file_ranges",
            budget.share(ContextCategory::FileRanges),
        ),
        (
            "context_budget.findings",
            budget.share(ContextCategory::Findings),
        ),
        (
            "context_budget.conversation_summary",
            budget.share(ContextCategory::ConversationSummary),
        ),
    ];
    lines.push(("context_budget.safety_margin", budget.safety_margin_bps));
    lines
}

/// The canonical decimal form of a basis-point value. Four places is exact:
/// one basis point is `0.0001`.
fn format_basis_points(bps: u16) -> String {
    format!("{}.{:04}", bps / 10_000, bps % 10_000)
}

/// Parses `0.10` or `0` into basis points without ever using a float: the last
/// token's allocation must not depend on rounding (proposal §5.2, Correction
/// C7). More than four decimal places is an error rather than a rounded value,
/// and so is a value above `1.00`.
fn parse_basis_points(key: &str, value: &str) -> Result<u16> {
    let value = value.trim();
    let invalid = || {
        ClientError::InvalidInput(format!(
            "{key} must be a decimal between 0 and 1 with at most four decimal places"
        ))
    };
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (value, None),
    };
    if whole.is_empty() || !whole.chars().all(|character| character.is_ascii_digit()) {
        return Err(invalid());
    }
    let whole: u32 = whole.parse().map_err(|_| invalid())?;
    let fractional = match fraction {
        None => 0,
        Some(fraction) => {
            if fraction.is_empty()
                || fraction.len() > 4
                || !fraction.chars().all(|character| character.is_ascii_digit())
            {
                return Err(invalid());
            }
            let digits: u32 = fraction.parse().map_err(|_| invalid())?;
            digits * 10u32.pow(4 - fraction.len() as u32)
        }
    };
    let bps = whole
        .checked_mul(10_000)
        .and_then(|points| points.checked_add(fractional))
        .ok_or_else(invalid)?;
    if bps > 10_000 {
        return Err(invalid());
    }
    Ok(bps as u16)
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split('|')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn split_paths(value: &str) -> Vec<PathBuf> {
    split_list(value).into_iter().map(PathBuf::from).collect()
}

fn join_list(values: &[String]) -> String {
    values.join("|")
}

fn join_paths(values: &[PathBuf]) -> String {
    values
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("|")
}

/// Bounds a rejected key name for the report.
///
/// Every other rejected key comes from the known vocabulary in
/// [`ConfigOverlay::set`], but an *unparsable* one can be any text the
/// repository put left of the `=` — and it reaches a notice dialog. So it is
/// trimmed to what a dialog can show: printable, single-line, and short. Long
/// enough to name the typo the user has to go and fix.
fn bounded_key_name(key: &str) -> String {
    const MAX_CHARS: usize = 64;
    let mut printable = key.chars().filter(|character| !character.is_control());
    let bounded: String = printable.by_ref().take(MAX_CHARS).collect();
    if bounded.is_empty() {
        return "(empty key)".to_string();
    }
    if printable.next().is_some() {
        format!("{bounded}…")
    } else {
        bounded
    }
}

fn parse_bool(key: &str, value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ClientError::InvalidInput(format!(
            "{key} must be true or false"
        ))),
    }
}

fn parse_u64(key: &str, value: &str) -> Result<u64> {
    value
        .parse()
        .map_err(|_| ClientError::InvalidInput(format!("{key} must be an unsigned integer")))
}

fn parse_round_count(key: &str, value: &str) -> Result<u32> {
    let parsed = parse_u64(key, value)?;
    if (1..=16).contains(&parsed) {
        Ok(parsed as u32)
    } else {
        Err(ClientError::InvalidInput(format!(
            "{key} must be between 1 and 16"
        )))
    }
}

/// The exploration allowance differs from `parse_round_count`: `0` is valid
/// ("no exploration allowance", spec 59 requirement 6) and a value above the
/// ceiling is clamped to it rather than refused (requirement 5). The ceiling is
/// `ABSOLUTE_EXPLORATION_ROUND_CAP` (32), defined beside
/// `ABSOLUTE_TOOL_ROUND_CAP` in `chat.rs`; the round loop clamps again at
/// runtime via `default_exploration_round_limit`.
fn parse_exploration_round_count(key: &str, value: &str) -> Result<u32> {
    let parsed = parse_u64(key, value)?;
    Ok(parsed.min(32) as u32)
}

/// Absolute upper bound for `agent_max_turn_notes_chars` (spec 65 §5.7).
///
/// The notes are sent in every request of the turn and carried into the next
/// turn, so this is a bound on what a session adds to every later call.
/// Raising it past this constant requires explicit work, not configuration.
pub const ABSOLUTE_TURN_NOTES_CHARS: usize = 32_000;

/// `agent_max_turn_notes_chars` accepts `0` ("notes off") and clamps a larger
/// value to [`ABSOLUTE_TURN_NOTES_CHARS`], like the exploration allowance.
/// Unlike `parse_read_lines`, zero is meaningful rather than a broken cap.
fn parse_turn_notes_chars(key: &str, value: &str) -> Result<usize> {
    let parsed = parse_u64(key, value)? as usize;
    Ok(parsed.min(ABSOLUTE_TURN_NOTES_CHARS))
}

/// Takes the *lower* of the two at an untrusted scope, and whatever was given
/// at a trusted one.
///
/// `None` means no ceiling, so any value at all is a tightening and a
/// repository may set one where the user set none. What it may not do is raise
/// a ceiling the user chose, or remove one — both spend the user's money on the
/// repository's say-so. `context.md` §3.8.
/// A cap of zero would make every read return nothing, which is
/// indistinguishable from Damaian being unable to read the repository at all.
/// Refuse it at parse time rather than letting it surface later as a file the
/// model insists is empty — the same reasoning as `parse_task_token_ceiling`.
fn parse_read_lines(key: &str, value: &str) -> Result<usize> {
    let parsed = parse_u64(key, value)? as usize;
    if parsed >= 1 {
        Ok(parsed)
    } else {
        Err(ClientError::InvalidInput(format!(
            "{key} must be at least 1"
        )))
    }
}

/// A deadline of zero would kill every command at spawn, which reads as
/// Damaian being unable to run anything rather than as a timeout. Refuse it at
/// parse time, the same reasoning as `parse_task_token_ceiling`.
fn parse_timeout_seconds(key: &str, value: &str) -> Result<usize> {
    let parsed = parse_u64(key, value)? as usize;
    if parsed >= 1 {
        Ok(parsed)
    } else {
        Err(ClientError::InvalidInput(format!(
            "{key} must be at least 1"
        )))
    }
}

/// [`restrict_only_ceiling`] for a value that always has one.
///
/// A repository may lower a cap — a restriction is always allowed — but raising
/// one it did not set would let a cloned repository widen what Damaian reads on
/// the user's behalf.
///
/// Returns `Some(loosened)` when applied. An equal value is refused at an
/// untrusted scope, as it always was, but is not a loosening at a trusted one.
fn restrict_only_limit(
    current: &mut usize,
    value: usize,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<bool> {
    if trusted || value < *current {
        let loosens = value > *current;
        *current = value;
        return Some(loosens);
    }
    rejected.push(RejectedConfigKey::new(
        key,
        RepositoryKeyClass::RestrictOnly,
    ));
    None
}

/// [`restrict_only_limit`] for a `u32` value: the read allowance and the round
/// bounds are `u32`-typed, while the read caps' loop above is `usize`-typed, so
/// they cannot share that helper. Spec 59 §5.4.
fn restrict_only_u32(
    current: &mut u32,
    value: u32,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<bool> {
    if trusted || value < *current {
        let loosens = value > *current;
        *current = value;
        return Some(loosens);
    }
    rejected.push(RejectedConfigKey::new(
        key,
        RepositoryKeyClass::RestrictOnly,
    ));
    None
}

/// [`restrict_only_limit`] for a context share in basis points (spec 26 §5.2).
///
/// Lowering a share is the restriction: it sends less of the repository's own
/// content. A raise is refused and reported, never clipped to the current value.
fn restrict_only_share(
    current: &mut u16,
    value: u16,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<bool> {
    if trusted || value < *current {
        let loosens = value > *current;
        *current = value;
        return Some(loosens);
    }
    rejected.push(RejectedConfigKey::new(
        key,
        RepositoryKeyClass::RestrictOnly,
    ));
    None
}

/// The opposite direction of [`restrict_only_share`]: the margin reserves part
/// of an observed window, so a *higher* margin is more restrictive and an
/// untrusted scope may only raise it.
fn restrict_only_minimum(
    current: &mut u16,
    value: u16,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<bool> {
    if trusted || value > *current {
        let loosens = value < *current;
        *current = value;
        return Some(loosens);
    }
    rejected.push(RejectedConfigKey::new(
        key,
        RepositoryKeyClass::RestrictOnly,
    ));
    None
}

fn restrict_only_ceiling(
    current: &mut Option<u64>,
    value: u64,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) -> Option<bool> {
    if trusted {
        let loosens = current.is_some_and(|existing| value > existing);
        *current = Some(value);
        return Some(loosens);
    }
    match *current {
        Some(existing) if value >= existing => {
            rejected.push(RejectedConfigKey::new(
                key,
                RepositoryKeyClass::RestrictOnly,
            ));
            None
        }
        _ => {
            *current = Some(value);
            Some(false)
        }
    }
}

/// A ceiling low enough to stop every turn before its first call is
/// indistinguishable from Damaian being broken, so a typo is refused at parse
/// time rather than surfacing later as a hang nobody can explain.
fn parse_task_token_ceiling(key: &str, value: &str) -> Result<u64> {
    let parsed = parse_u64(key, value)?;
    if parsed >= 1000 {
        Ok(parsed)
    } else {
        Err(ClientError::InvalidInput(format!(
            "{key} must be at least 1000"
        )))
    }
}

fn parse_retry_limit(key: &str, value: &str) -> Result<u32> {
    let parsed = parse_u64(key, value)?;
    if (1..=8).contains(&parsed) {
        Ok(parsed as u32)
    } else {
        Err(ClientError::InvalidInput(format!(
            "{key} must be between 1 and 8"
        )))
    }
}

/// `0` is valid: it means `ask_user` is not offered. Above the ceiling is an
/// error, never a clamp, so a repository that asks for more is refused and
/// reported instead of quietly getting the most allowed (spec 50
/// `context.md` §8).
fn parse_question_cap(key: &str, value: &str) -> Result<u32> {
    let parsed = parse_u64(key, value)?;
    if parsed <= u64::from(MAX_QUESTIONS_PER_TURN_CEILING) {
        Ok(parsed as u32)
    } else {
        Err(ClientError::InvalidInput(format!(
            "{key} must be between 0 and {MAX_QUESTIONS_PER_TURN_CEILING}"
        )))
    }
}

fn parse_model_api_key_reference(value: &str) -> Result<String> {
    let value = value.trim();
    if let Some(account) = value.strip_prefix("keychain:") {
        let account = account.trim();
        if account.is_empty() {
            return Err(ClientError::InvalidInput(
                "model_api_key_env keychain account is required".to_string(),
            ));
        }
        if account.chars().any(char::is_control) {
            return Err(ClientError::InvalidInput(
                "model_api_key_env keychain account cannot contain control characters".to_string(),
            ));
        }
        return Ok(format!("keychain:{account}"));
    }

    let mut chars = value.chars();
    let starts_valid = chars
        .next()
        .map(|character| character == '_' || character.is_ascii_alphabetic())
        .unwrap_or(false);
    let rest_valid = chars.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if starts_valid && rest_valid {
        return Ok(value.to_string());
    }

    Err(ClientError::InvalidInput(
        "model_api_key_env must be an environment variable name or keychain:<account>; do not paste the API key into config"
            .to_string(),
    ))
}

fn is_keychain_reference(value: &str) -> bool {
    value.trim_start().starts_with("keychain:")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 57 requirement 1: the user has to turn workers on, and the user
    /// config is the one place that can.
    #[test]
    fn worker_sessions_are_off_by_default() {
        assert!(!Config::default().enable_worker_sessions);

        let mut config = Config::default();
        config.apply_overlay(ConfigOverlay::parse("enable_worker_sessions=true\n").unwrap());
        assert!(config.enable_worker_sessions);
        assert!(
            config
                .to_policy_text()
                .contains("enable_worker_sessions=true")
        );
    }

    /// Spec 26 §5.2: decimals are stored and allocated as integer basis points,
    /// and a set that oversubscribes is an error naming the family rather than
    /// a silent scale-down.
    #[test]
    fn context_budget_shares_are_stored_in_basis_points_and_an_oversubscribed_set_is_a_config_error()
     {
        let mut config = Config::default();
        let overlay = ConfigOverlay::parse(concat!(
            "context_budget.instructions=0.10\n",
            "context_budget.repository_map=0.0500\n",
            "context_budget.safety_margin=0.15\n",
        ))
        .unwrap();
        assert_eq!(overlay.context_budget_instructions, Some(1_000));
        assert_eq!(
            ConfigOverlay::parse(&overlay.to_policy_text()).unwrap(),
            overlay,
            "the overlay serializer must round-trip the new keys"
        );
        config.apply_overlay(overlay);
        assert_eq!(
            config.context_budget.share(ContextCategory::Instructions),
            1_000
        );
        assert_eq!(
            config.context_budget.share(ContextCategory::RepositoryMap),
            500
        );
        assert_eq!(config.context_budget.safety_margin_bps, 1_500);
        assert!(config.validate_context_budget().is_ok());
        assert!(
            config
                .to_policy_text()
                .contains("context_budget.instructions=0.1000")
        );

        // More decimal places than a basis point can express is refused rather
        // than rounded: the last token's allocation must not depend on it.
        assert!(ConfigOverlay::parse("context_budget.instructions=0.12345\n").is_err());
        assert!(ConfigOverlay::parse("context_budget.instructions=1.01\n").is_err());

        let mut oversubscribed = Config::default();
        oversubscribed
            .context_budget
            .set_share(ContextCategory::Instructions, 10_000);
        oversubscribed
            .context_budget
            .set_share(ContextCategory::FileRanges, 2_500);
        let error = oversubscribed.validate_context_budget().unwrap_err();
        assert!(
            error.to_string().contains("context_budget"),
            "the error must name the key family: {error}"
        );
    }
}
