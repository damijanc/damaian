use crate::error::{ClientError, Result};
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
    "**/secrets/**",
    "**/credentials/**",
];

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub max_file_bytes: u64,
    pub max_command_output_bytes: usize,
    pub allowed_roots: Vec<PathBuf>,
    pub ignore_patterns: Vec<String>,
    pub restricted_patterns: Vec<String>,
    pub command_allowlist: Vec<String>,
    pub command_blocklist: Vec<String>,
    pub secret_patterns: Vec<String>,
    pub require_approval_for_file_edits: bool,
    pub require_approval_for_risky_commands: bool,
    pub require_approval_for_all_commands: bool,
    pub block_generated_secrets: bool,
    pub audit_enabled: bool,
    pub audit_retention_days: u64,
    /// Off by default: enabling local semantic search downloads a small
    /// embedding model on first use (a one-time network fetch), which the
    /// user should opt into rather than have happen implicitly.
    pub enable_semantic_search: bool,
    /// Default number of tool/model rounds an agentic chat turn may spend
    /// before the model is asked to answer with the evidence it has.
    pub agent_max_tool_rounds: u32,
    /// Larger budget for turns that are explicitly debugging a web page or
    /// that call a first-class browser diagnostic tool.
    pub agent_web_debug_max_tool_rounds: u32,
    /// How many substantially identical failed tool calls may be retried before
    /// the model is told to change approach.
    pub agent_tool_retry_limit: u32,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProviderConfig {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub api_key_env: String,
    pub models: Vec<String>,
    /// Opt-in: whether to use the provider's native `tools`/`tool_calls`
    /// contract instead of the `DAMAIAN_COMMAND_V1` text envelope. Defaults
    /// to false so existing providers are unaffected until explicitly
    /// enabled.
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelProviderConfigOverlay {
    pub id: String,
    pub label: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub models: Option<Vec<String>>,
    pub supports_native_tools: Option<bool>,
    pub max_output_tokens: Option<u32>,
    pub context_token_budget: Option<u32>,
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
        let config = Self::default();
        let default_data_dir = config.data_dir.clone();
        let user_path = default_data_dir.join("config").join("user.conf");
        let repo_path = repository_root.map(Self::repository_config_path);
        let admin_path = std::env::var("DAMAIAN_ADMIN_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_data_dir.join("config").join("admin.conf"));
        Self::load_with_policy_paths(
            config,
            Some(user_path.as_path()),
            repo_path.as_deref(),
            Some(admin_path.as_path()),
        )
    }

    pub fn load_with_policy_paths(
        mut config: Self,
        user_path: Option<&Path>,
        repo_path: Option<&Path>,
        admin_path: Option<&Path>,
    ) -> Result<Self> {
        if let Some(path) = user_path
            && path.exists()
        {
            config.apply_overlay(ConfigOverlay::load(path)?);
        }
        if let Some(path) = repo_path
            && path.exists()
        {
            config.apply_overlay(ConfigOverlay::load(path)?);
        }
        if let Some(path) = admin_path
            && path.exists()
        {
            config.apply_overlay(ConfigOverlay::load(path)?);
        }
        Ok(config)
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

    pub fn apply_overlay(&mut self, overlay: ConfigOverlay) {
        if let Some(value) = overlay.data_dir {
            self.data_dir = value;
        }
        if let Some(value) = overlay.max_file_bytes {
            self.max_file_bytes = value;
        }
        if let Some(value) = overlay.max_command_output_bytes {
            self.max_command_output_bytes = value;
        }
        if let Some(value) = overlay.allowed_roots {
            self.allowed_roots = value;
        }
        if let Some(value) = overlay.ignore_patterns {
            self.ignore_patterns = value;
        }
        if let Some(value) = overlay.restricted_patterns {
            self.restricted_patterns = value;
        }
        if let Some(value) = overlay.command_allowlist {
            self.command_allowlist = value;
        }
        if let Some(value) = overlay.command_blocklist {
            self.command_blocklist = value;
        }
        if let Some(value) = overlay.secret_patterns {
            self.secret_patterns = value;
        }
        if let Some(value) = overlay.require_approval_for_file_edits {
            self.require_approval_for_file_edits = value;
        }
        if let Some(value) = overlay.require_approval_for_risky_commands {
            self.require_approval_for_risky_commands = value;
        }
        if let Some(value) = overlay.require_approval_for_all_commands {
            self.require_approval_for_all_commands = value;
        }
        if let Some(value) = overlay.block_generated_secrets {
            self.block_generated_secrets = value;
        }
        if let Some(value) = overlay.audit_enabled {
            self.audit_enabled = value;
        }
        if let Some(value) = overlay.audit_retention_days {
            self.audit_retention_days = value;
        }
        if let Some(value) = overlay.enable_semantic_search {
            self.enable_semantic_search = value;
        }
        if let Some(value) = overlay.agent_max_tool_rounds {
            self.agent_max_tool_rounds = value;
        }
        if let Some(value) = overlay.agent_web_debug_max_tool_rounds {
            self.agent_web_debug_max_tool_rounds = value;
        }
        if let Some(value) = overlay.agent_tool_retry_limit {
            self.agent_tool_retry_limit = value;
        }
        if let Some(value) = overlay.shell {
            self.shell = value;
        }
        for provider in overlay.model_providers {
            self.upsert_model_provider(provider);
        }
        if let Some(value) = overlay.model_provider {
            self.model_provider = value;
            self.apply_model_provider_defaults();
        }
        if let Some(value) = overlay.model_name {
            self.model_name = value;
        }
        if let Some(value) = overlay.model_base_url {
            self.model_base_url = value;
        }
        if let Some(value) = overlay.model_api_key_env {
            self.model_api_key_env = value;
        }
        if let Some(value) = overlay.model_reasoning_level {
            self.model_reasoning_level = value;
        }
        if let Some(value) = overlay.mcp_enabled {
            self.mcp_enabled = value;
        }
        if let Some(value) = overlay.mcp_server_allowlist {
            self.mcp_server_allowlist = value;
        }
        for server in overlay.mcp_servers {
            self.upsert_mcp_server(server);
        }
    }

    /// Whether the active model provider is configured to use native
    /// `tools`/`tool_calls` instead of the `DAMAIAN_COMMAND_V1` text
    /// envelope. Defaults to false for any provider that hasn't explicitly
    /// opted in.
    pub fn supports_native_tools(&self) -> bool {
        self.model_provider_config(&self.model_provider)
            .cloned()
            .or_else(|| builtin_model_provider_config(&self.model_provider))
            .map(|provider| provider.supports_native_tools)
            .unwrap_or(false)
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

    fn upsert_model_provider(&mut self, overlay: ModelProviderConfigOverlay) {
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
            return;
        }

        let id = overlay.id;
        self.model_providers.push(ModelProviderConfig {
            label: overlay.label.unwrap_or_else(|| id.clone()),
            base_url: overlay.base_url.unwrap_or_default(),
            api_key_env: overlay.api_key_env.unwrap_or_default(),
            models: overlay.models.unwrap_or_default(),
            supports_native_tools: overlay.supports_native_tools.unwrap_or(false),
            max_output_tokens: overlay.max_output_tokens,
            context_token_budget: overlay.context_token_budget,
            id,
        });
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
            "max_command_output_bytes",
            &self.max_command_output_bytes.to_string(),
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
            "enable_semantic_search",
            &self.enable_semantic_search.to_string(),
        );
        push_line(
            &mut output,
            "agent_max_tool_rounds",
            &self.agent_max_tool_rounds.to_string(),
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
        output
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: Self::default_data_dir(),
            max_file_bytes: 1024 * 1024,
            max_command_output_bytes: 1024 * 1024,
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
            command_blocklist: Vec::new(),
            secret_patterns: Vec::new(),
            require_approval_for_file_edits: true,
            require_approval_for_risky_commands: true,
            require_approval_for_all_commands: false,
            block_generated_secrets: true,
            audit_enabled: true,
            audit_retention_days: 90,
            enable_semantic_search: false,
            agent_max_tool_rounds: 8,
            agent_web_debug_max_tool_rounds: 12,
            agent_tool_retry_limit: 2,
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
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigOverlay {
    pub data_dir: Option<PathBuf>,
    pub max_file_bytes: Option<u64>,
    pub max_command_output_bytes: Option<usize>,
    pub allowed_roots: Option<Vec<PathBuf>>,
    pub ignore_patterns: Option<Vec<String>>,
    pub restricted_patterns: Option<Vec<String>>,
    pub command_allowlist: Option<Vec<String>>,
    pub command_blocklist: Option<Vec<String>>,
    pub secret_patterns: Option<Vec<String>>,
    pub require_approval_for_file_edits: Option<bool>,
    pub require_approval_for_risky_commands: Option<bool>,
    pub require_approval_for_all_commands: Option<bool>,
    pub block_generated_secrets: Option<bool>,
    pub audit_enabled: Option<bool>,
    pub audit_retention_days: Option<u64>,
    pub enable_semantic_search: Option<bool>,
    pub agent_max_tool_rounds: Option<u32>,
    pub agent_web_debug_max_tool_rounds: Option<u32>,
    pub agent_tool_retry_limit: Option<u32>,
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
        match key {
            "data_dir" => self.data_dir = Some(PathBuf::from(value)),
            "max_file_bytes" => self.max_file_bytes = Some(parse_u64(key, value)?),
            "max_command_output_bytes" => {
                self.max_command_output_bytes = Some(parse_u64(key, value)? as usize)
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
            "block_generated_secrets" => {
                self.block_generated_secrets = Some(parse_bool(key, value)?)
            }
            "audit_enabled" => self.audit_enabled = Some(parse_bool(key, value)?),
            "audit_retention_days" => self.audit_retention_days = Some(parse_u64(key, value)?),
            "enable_semantic_search" => self.enable_semantic_search = Some(parse_bool(key, value)?),
            "agent_max_tool_rounds" => {
                self.agent_max_tool_rounds = Some(parse_round_count(key, value)?)
            }
            "agent_web_debug_max_tool_rounds" => {
                self.agent_web_debug_max_tool_rounds = Some(parse_round_count(key, value)?)
            }
            "agent_tool_retry_limit" => {
                self.agent_tool_retry_limit = Some(parse_retry_limit(key, value)?)
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

    pub fn to_policy_text(&self) -> String {
        let mut output = String::new();
        if let Some(value) = &self.data_dir {
            push_line(&mut output, "data_dir", &value.to_string_lossy());
        }
        if let Some(value) = self.max_file_bytes {
            push_line(&mut output, "max_file_bytes", &value.to_string());
        }
        if let Some(value) = self.max_command_output_bytes {
            push_line(&mut output, "max_command_output_bytes", &value.to_string());
        }
        if let Some(value) = &self.allowed_roots {
            push_line(&mut output, "allowed_roots", &join_paths(value));
        }
        if let Some(value) = &self.ignore_patterns {
            push_line(&mut output, "ignore_patterns", &join_list(value));
        }
        if let Some(value) = &self.restricted_patterns {
            push_line(&mut output, "restricted_patterns", &join_list(value));
        }
        if let Some(value) = &self.command_allowlist {
            push_line(&mut output, "command_allowlist", &join_list(value));
        }
        if let Some(value) = &self.command_blocklist {
            push_line(&mut output, "command_blocklist", &join_list(value));
        }
        if let Some(value) = &self.secret_patterns {
            push_line(&mut output, "secret_patterns", &join_list(value));
        }
        if let Some(value) = self.require_approval_for_file_edits {
            push_line(
                &mut output,
                "require_approval_for_file_edits",
                &value.to_string(),
            );
        }
        if let Some(value) = self.require_approval_for_risky_commands {
            push_line(
                &mut output,
                "require_approval_for_risky_commands",
                &value.to_string(),
            );
        }
        if let Some(value) = self.require_approval_for_all_commands {
            push_line(
                &mut output,
                "require_approval_for_all_commands",
                &value.to_string(),
            );
        }
        if let Some(value) = self.block_generated_secrets {
            push_line(&mut output, "block_generated_secrets", &value.to_string());
        }
        if let Some(value) = self.audit_enabled {
            push_line(&mut output, "audit_enabled", &value.to_string());
        }
        if let Some(value) = self.audit_retention_days {
            push_line(&mut output, "audit_retention_days", &value.to_string());
        }
        if let Some(value) = self.enable_semantic_search {
            push_line(&mut output, "enable_semantic_search", &value.to_string());
        }
        if let Some(value) = self.agent_max_tool_rounds {
            push_line(&mut output, "agent_max_tool_rounds", &value.to_string());
        }
        if let Some(value) = self.agent_web_debug_max_tool_rounds {
            push_line(
                &mut output,
                "agent_web_debug_max_tool_rounds",
                &value.to_string(),
            );
        }
        if let Some(value) = self.agent_tool_retry_limit {
            push_line(&mut output, "agent_tool_retry_limit", &value.to_string());
        }
        if let Some(value) = &self.shell {
            push_line(&mut output, "shell", value);
        }
        if let Some(value) = &self.model_provider {
            push_line(&mut output, "model_provider", value);
        }
        if let Some(value) = &self.model_name {
            push_line(&mut output, "model_name", value);
        }
        if let Some(value) = &self.model_base_url {
            push_line(&mut output, "model_base_url", value);
        }
        if let Some(value) = &self.model_api_key_env {
            push_line(&mut output, "model_api_key_env", value);
        }
        if let Some(value) = &self.model_reasoning_level {
            push_line(&mut output, "model_reasoning_level", value);
        }
        for provider in &self.model_providers {
            push_model_provider_overlay(&mut output, provider);
        }
        if let Some(value) = self.mcp_enabled {
            push_line(&mut output, "mcp_enabled", &value.to_string());
        }
        if let Some(value) = &self.mcp_server_allowlist {
            push_line(&mut output, "mcp_server_allowlist", &join_list(value));
        }
        for server in &self.mcp_servers {
            push_mcp_server_overlay(&mut output, server);
        }
        output
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
            supports_native_tools: false,
            max_output_tokens: None,
            context_token_budget: None,
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
            supports_native_tools: false,
            // Fallback for a DeepSeek model not in `builtin_model_output_tokens`.
            // Deliberately the conservative legacy ceiling: an unrecognized
            // model may still be a legacy alias, and a too-small ceiling costs
            // an extra round trip whereas a too-large one is a hard API error.
            max_output_tokens: Some(8192),
            context_token_budget: None,
        }),
        "openai-compatible" => Some(ModelProviderConfig {
            id: "openai-compatible".to_string(),
            label: "Custom".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key_env: "OPENAI_API_KEY".to_string(),
            models: vec!["configured-model".to_string()],
            supports_native_tools: false,
            max_output_tokens: None,
            context_token_budget: None,
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

fn is_builtin_model_provider(id: &str) -> bool {
    matches!(id, "openai" | "deepseek" | "openai-compatible")
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
    if let Some(value) = &provider.label {
        push_line(
            output,
            &format!("model_provider.{}.label", provider.id),
            value,
        );
    }
    if let Some(value) = &provider.base_url {
        push_line(
            output,
            &format!("model_provider.{}.base_url", provider.id),
            value,
        );
    }
    if let Some(value) = &provider.api_key_env {
        push_line(
            output,
            &format!("model_provider.{}.api_key_env", provider.id),
            value,
        );
    }
    if let Some(value) = &provider.models {
        push_line(
            output,
            &format!("model_provider.{}.models", provider.id),
            &join_list(value),
        );
    }
    if let Some(value) = provider.supports_native_tools {
        push_line(
            output,
            &format!("model_provider.{}.supports_native_tools", provider.id),
            &value.to_string(),
        );
    }
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

fn push_line(output: &mut String, key: &str, value: &str) {
    output.push_str(key);
    output.push('=');
    output.push_str(value);
    output.push('\n');
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
