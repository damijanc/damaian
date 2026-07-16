use crate::error::{ClientError, Result};
use std::fs;
use std::path::{Path, PathBuf};

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
    pub shell: String,
    pub model_provider: String,
    pub model_name: String,
    pub model_base_url: String,
    pub model_api_key_env: String,
    pub model_reasoning_level: String,
    pub model_providers: Vec<ModelProviderConfig>,
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelProviderConfigOverlay {
    pub id: String,
    pub label: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub models: Option<Vec<String>>,
    pub supports_native_tools: Option<bool>,
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
        if let Some(path) = user_path {
            if path.exists() {
                config.apply_overlay(ConfigOverlay::load(path)?);
            }
        }
        if let Some(path) = repo_path {
            if path.exists() {
                config.apply_overlay(ConfigOverlay::load(path)?);
            }
        }
        if let Some(path) = admin_path {
            if path.exists() {
                config.apply_overlay(ConfigOverlay::load(path)?);
            }
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
            return;
        }

        let id = overlay.id;
        self.model_providers.push(ModelProviderConfig {
            label: overlay.label.unwrap_or_else(|| id.clone()),
            base_url: overlay.base_url.unwrap_or_default(),
            api_key_env: overlay.api_key_env.unwrap_or_default(),
            models: overlay.models.unwrap_or_default(),
            supports_native_tools: overlay.supports_native_tools.unwrap_or(false),
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
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string()),
            model_provider: "openai".to_string(),
            model_name: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4.1".to_string()),
            model_base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            model_api_key_env: "OPENAI_API_KEY".to_string(),
            model_reasoning_level: "default".to_string(),
            model_providers: Vec::new(),
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
    pub shell: Option<String>,
    pub model_provider: Option<String>,
    pub model_name: Option<String>,
    pub model_base_url: Option<String>,
    pub model_api_key_env: Option<String>,
    pub model_reasoning_level: Option<String>,
    pub model_providers: Vec<ModelProviderConfigOverlay>,
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
            _ => {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown model provider config key: model_provider.{provider_key}"
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
        }),
        "deepseek" => Some(ModelProviderConfig {
            id: "deepseek".to_string(),
            label: "DeepSeek".to_string(),
            base_url: std::env::var("DEEPSEEK_BASE_URL")
                .unwrap_or_else(|_| "https://api.deepseek.com".to_string()),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            models: vec![
                std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-chat".to_string()),
                "deepseek-reasoner".to_string(),
            ],
            supports_native_tools: false,
        }),
        "openai-compatible" => Some(ModelProviderConfig {
            id: "openai-compatible".to_string(),
            label: "Custom".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key_env: "OPENAI_API_KEY".to_string(),
            models: vec!["configured-model".to_string()],
            supports_native_tools: false,
        }),
        _ => None,
    }
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
