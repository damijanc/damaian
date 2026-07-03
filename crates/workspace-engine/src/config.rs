use std::path::PathBuf;

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
    pub require_approval_for_file_edits: bool,
    pub require_approval_for_risky_commands: bool,
    pub require_approval_for_all_commands: bool,
    pub block_generated_secrets: bool,
    pub shell: String,
    pub model_provider: String,
    pub model_name: String,
    pub model_base_url: String,
    pub model_api_key_env: String,
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
            require_approval_for_file_edits: true,
            require_approval_for_risky_commands: true,
            require_approval_for_all_commands: false,
            block_generated_secrets: true,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string()),
            model_provider: "openai-compatible".to_string(),
            model_name: std::env::var("OPENAI_MODEL")
                .unwrap_or_else(|_| "configured-model".to_string()),
            model_base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            model_api_key_env: "OPENAI_API_KEY".to_string(),
        }
    }
}
