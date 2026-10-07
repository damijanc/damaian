//! The effective policy with a source for every rule (spec 31, proposal §5.7).
//!
//! Built from what [`Config::load_scoped`] recorded while it applied each
//! overlay, never by resolving the files a second time (`context.md` §8): a
//! second resolver would drift from the real merge. The values are the ones
//! `Config::to_policy_text` prints, so this view and the plain text cannot
//! disagree on what a key holds.
//!
//! A refused request is shown by key and class only. Its value is text the
//! repository or a custom profile file chose, and is never carried here.

use crate::config::{AppliedKey, Config, ConfigScope, RejectedConfigKey, RepositoryConfigReport};
use crate::error::Result;
use crate::mode::SessionMode;
use crate::profile::ProfileId;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Where a rule, or one entry of a list rule, came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    /// No config file set it.
    Default,
    User,
    Repository,
    Admin,
    /// The permission profile selected for this checkout.
    Profile,
    /// A `command_allowlist` entry granted with Allow Always for this
    /// checkout.
    AllowAlways,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicySource {
    pub kind: SourceKind,
    /// What a person reads: "user config", "profile: safe_local", …
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyEntry {
    pub value: String,
    pub source: PolicySource,
}

/// Which untrusted layer asked for something it did not get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefusedBy {
    Repository,
    Profile,
}

impl RefusedBy {
    fn label(self) -> &'static str {
        match self {
            RefusedBy::Repository => "repository config",
            RefusedBy::Profile => "the permission profile",
        }
    }
}

/// A refused request: the key and why, never the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefusedRequest {
    pub key: String,
    /// [`rule_label`] of the key, or the key itself when it has none (an
    /// unknown or unparsable line).
    pub label: String,
    /// [`crate::RepositoryKeyClass::as_str`]: `forbidden`, `restrict_only`,
    /// `user_owned` or `unparsable`.
    pub class: String,
    pub by: RefusedBy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    pub key: String,
    /// What the rule means, for a person who does not know the config keys.
    pub label: String,
    /// As `Config::to_policy_text` prints it; a list is `|`-joined.
    pub value: String,
    /// For a scalar, the one source whose value holds. For a list, the
    /// distinct sources of its entries, in entry order.
    pub sources: Vec<PolicySource>,
    /// Per-entry sources, for the list keys only.
    pub entries: Option<Vec<PolicyEntry>>,
    /// Admin config loosened what user and repository config resolved.
    pub admin_widened: bool,
    pub refused: Vec<RefusedRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectivePolicy {
    /// "Safe local development ∩ Code mode", or the profile alone.
    pub header: String,
    /// The profile id in force; `full` when none is selected.
    pub profile: String,
    pub profile_label: String,
    /// Whether this checkout has a selection, as opposed to the default.
    pub profile_selected: bool,
    pub mode: Option<String>,
    /// One per line of `Config::to_policy_text`, in its order.
    pub rules: Vec<PolicyRule>,
    /// Refusals of keys that have no rule, such as a `model_provider.<id>`
    /// the repository tried to define, or an unparsable line.
    pub other_refused: Vec<RefusedRequest>,
}

/// A plain-language name for a rule key. The settings view leads with it,
/// because a reader who did not build Damaian cannot be expected to know what
/// `require_approval_for_risky_commands` decides (spec 31 §5.7). `None` for a
/// key with no name, which the view then shows as the key itself;
/// `every_rule_has_a_human_name` keeps that from happening to a rule.
pub fn rule_label(key: &str) -> Option<String> {
    let fixed = match key {
        "data_dir" => "Damaian's data folder",
        "max_file_bytes" => "Largest file Damaian reads (bytes)",
        "max_read_lines" => "Lines returned per file read",
        "max_list_entries" => "Entries returned per folder listing",
        "max_search_matches" => "Matches returned per search",
        "max_match_line_chars" => "Characters kept per search match",
        "max_command_output_bytes" => "Command output kept (bytes)",
        "command_timeout_secs" => "Time limit per command (seconds)",
        "allowed_roots" => "Folders Damaian may open as a repository",
        "ignore_patterns" => "Files left out of the index",
        "restricted_patterns" => "Files Damaian may not read",
        "command_allowlist" => "Commands that run without asking",
        "command_blocklist" => "Commands that are always blocked",
        "secret_patterns" => "Extra patterns redacted as secrets",
        "require_approval_for_file_edits" => "Ask before changing files",
        "require_approval_for_risky_commands" => "Ask before test, build and lint commands",
        "require_approval_for_all_commands" => "Ask before every command",
        "allow_file_edits" => "File changes allowed",
        "command_access" => "Which commands may run",
        "allow_browser_diagnostics" => "Browser diagnostics allowed",
        "allow_mutating_mcp_tools" => "MCP tools that change things allowed",
        "block_generated_secrets" => "Stop changes that add a secret",
        "audit_enabled" => "Audit log on",
        "audit_retention_days" => "Audit files kept (days)",
        "checkpoint_retention_days" => "Checkpoints kept (days)",
        "checkpoint_max_total_bytes" => "Checkpoint storage limit (bytes)",
        "checkpoint_census_max_paths" => "Changed files a command checkpoint covers",
        "enable_semantic_search" => "Semantic search",
        "agent_max_tool_rounds" => "Tool rounds per turn",
        "agent_web_debug_max_tool_rounds" => "Tool rounds per browser-debugging turn",
        "agent_tool_retry_limit" => "Retries of a failing tool call",
        "agent_max_task_tokens" => "Token limit per turn",
        "agent_max_turn_messages" => "Messages sent per model request",
        "project_roots_added" => "Folders you made project roots",
        "project_roots_removed" => "Folders you said are not project roots",
        "shell" => "Shell that runs commands",
        "model_provider" => "Model provider",
        "model_name" => "Model",
        "model_base_url" => "Where model requests are sent",
        "model_api_key_env" => "Where the API key is read from",
        "model_reasoning_level" => "Reasoning level",
        "mcp_enabled" => "MCP servers on",
        "mcp_server_allowlist" => "MCP servers allowed",
        _ => "",
    };
    if !fixed.is_empty() {
        return Some(fixed.to_string());
    }
    // The per-checkout and per-server keys, which reach the view as refusals
    // (`command_allowlist.<repository id>`) or as rules of their own.
    let (prefix, rest) = key.split_once('.')?;
    let owner = match prefix {
        "command_allowlist" => return rule_label(prefix),
        "permission_profile" => return Some("Permission profile".to_string()),
        "model_provider" => "Model provider",
        "mcp_server" => "MCP server",
        _ => return None,
    };
    let Some((id, field)) = rest.rsplit_once('.') else {
        return Some(format!("{owner} {rest}"));
    };
    let field = match (prefix, field) {
        ("model_provider", "label") => "name",
        ("model_provider", "base_url") => "where requests are sent",
        ("model_provider", "api_key_env") => "where the API key is read from",
        ("model_provider", "models") => "models",
        ("model_provider", "supports_native_tools") => "native tool calling",
        ("model_provider", "max_output_tokens") => "largest reply (tokens)",
        ("model_provider", "context_token_budget") => "context sent per request (tokens)",
        ("model_provider", field) if field.starts_with("price_per_million_") => "price",
        ("mcp_server", "label") => "name",
        ("mcp_server", "enabled") => "on",
        ("mcp_server", "require_approval") => "ask before each call",
        ("mcp_server", "transport") => "transport",
        ("mcp_server", "command") => "command it runs",
        ("mcp_server", "args") => "arguments",
        ("mcp_server", "env") => "environment",
        ("mcp_server", "url") => "address",
        ("mcp_server", "auth_token_env") => "where its token is read from",
        _ => return None,
    };
    Some(format!("{owner} {id}: {field}"))
}

/// The rules whose value is a list, with the resolved entries in order.
fn list_entries(config: &Config, key: &str) -> Option<Vec<String>> {
    let strings = |values: &[String]| values.to_vec();
    Some(match key {
        "allowed_roots" => config
            .allowed_roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect(),
        "ignore_patterns" => strings(&config.ignore_patterns),
        "restricted_patterns" => strings(&config.restricted_patterns),
        "command_allowlist" => strings(&config.command_allowlist),
        "command_blocklist" => strings(&config.command_blocklist),
        "secret_patterns" => strings(&config.secret_patterns),
        "mcp_server_allowlist" => strings(&config.mcp_server_allowlist),
        _ => return None,
    })
}

impl EffectivePolicy {
    /// Loads the effective config for `repository_root` the way the desktop
    /// shell and the CLI do, and attributes it.
    pub fn resolve(repository_root: Option<&Path>, mode: Option<SessionMode>) -> Result<Self> {
        let (config, report) = Config::load_for_repository_reporting(repository_root)?;
        Ok(Self::from_load(&config, &report, mode))
    }

    /// Attributes one result of [`Config::load_scoped`]. `config` and `report`
    /// must come from the same load.
    pub fn from_load(
        config: &Config,
        report: &RepositoryConfigReport,
        mode: Option<SessionMode>,
    ) -> Self {
        let profile = report.permission_profile.clone().unwrap_or(ProfileId::Full);
        let source = |scope: ConfigScope| match scope {
            ConfigScope::User => PolicySource {
                kind: SourceKind::User,
                label: "user config".into(),
            },
            ConfigScope::Repository => PolicySource {
                kind: SourceKind::Repository,
                label: "repository config".into(),
            },
            ConfigScope::Admin => PolicySource {
                kind: SourceKind::Admin,
                label: "admin config".into(),
            },
            ConfigScope::Profile => PolicySource {
                kind: SourceKind::Profile,
                label: format!("profile: {}", profile.as_str()),
            },
        };
        let default = PolicySource {
            kind: SourceKind::Default,
            label: "default".into(),
        };
        let allow_always = PolicySource {
            kind: SourceKind::AllowAlways,
            label: "this repository (Allow Always)".into(),
        };

        let mut by_key: BTreeMap<&str, Vec<&AppliedKey>> = BTreeMap::new();
        for applied in &report.applied {
            by_key
                .entry(applied.key.as_str())
                .or_default()
                .push(applied);
        }
        let mut refused: BTreeMap<String, Vec<RefusedRequest>> = BTreeMap::new();
        let refusals = |keys: &[RejectedConfigKey], by: RefusedBy| {
            keys.iter()
                .map(move |rejected| RefusedRequest {
                    key: rejected.key.clone(),
                    label: rule_label(&rejected.key).unwrap_or_else(|| rejected.key.clone()),
                    class: rejected.class.as_str().to_string(),
                    by,
                })
                .collect::<Vec<_>>()
        };
        for request in refusals(&report.rejected_keys, RefusedBy::Repository)
            .into_iter()
            .chain(refusals(&report.profile_rejected_keys, RefusedBy::Profile))
        {
            refused
                .entry(request.key.clone())
                .or_default()
                .push(request);
        }

        let text = config.to_policy_text();
        let mut rules = Vec::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let applied = by_key.get(key).map(Vec::as_slice).unwrap_or_default();
            let last = applied.last().map(|applied| source(applied.scope));
            let entries = list_entries(config, key).map(|values| {
                // The last scope to hold an entry is the one that holds it
                // now: a trusted scope replaces the list, an untrusted one
                // adds to it, and both report what they hold.
                let mut entry_source: BTreeMap<&str, PolicySource> = BTreeMap::new();
                for applied in applied {
                    for entry in applied.entries.iter().flatten() {
                        entry_source.insert(entry, source(applied.scope));
                    }
                }
                if key == "command_allowlist" {
                    for entry in &report.allow_always_entries {
                        entry_source.insert(entry, allow_always.clone());
                    }
                }
                values
                    .iter()
                    .map(|value| PolicyEntry {
                        value: value.clone(),
                        source: entry_source
                            .get(value.as_str())
                            .cloned()
                            .unwrap_or_else(|| default.clone()),
                    })
                    .collect::<Vec<_>>()
            });
            let sources = match &entries {
                Some(entries) if !entries.is_empty() => {
                    let mut sources: Vec<PolicySource> = Vec::new();
                    for entry in entries {
                        if !sources.contains(&entry.source) {
                            sources.push(entry.source.clone());
                        }
                    }
                    sources
                }
                _ => vec![last.unwrap_or_else(|| default.clone())],
            };
            rules.push(PolicyRule {
                key: key.to_string(),
                label: rule_label(key).unwrap_or_else(|| key.to_string()),
                value: value.to_string(),
                sources,
                entries,
                // `widened` is only ever set at admin scope, where it is
                // recorded; it is not re-derived here.
                admin_widened: applied.iter().any(|applied| applied.widened),
                refused: refused.remove(key).unwrap_or_default(),
            });
        }

        let profile_label = profile.label().to_string();
        let header = match mode {
            Some(mode) => format!("{profile_label} ∩ {} mode", mode.label()),
            None => profile_label.clone(),
        };
        EffectivePolicy {
            header,
            profile: profile.as_str().to_string(),
            profile_label,
            profile_selected: report.permission_profile.is_some(),
            mode: mode.map(|mode| mode.as_str().to_string()),
            rules,
            other_refused: refused.into_values().flatten().collect(),
        }
    }

    pub fn rule(&self, key: &str) -> Option<&PolicyRule> {
        self.rules.iter().find(|rule| rule.key == key)
    }

    /// The plain-text form `damaian config-show --sources` prints.
    pub fn to_text(&self) -> String {
        let labels = |sources: &[PolicySource]| {
            sources
                .iter()
                .map(|source| source.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let refusal = |request: &RefusedRequest| {
            format!(
                "refused: {} asked to set {} ({})",
                request.by.label(),
                request.key,
                request.class
            )
        };
        let mut output = format!("Effective policy — {}\n\n", self.header);
        for rule in &self.rules {
            output.push_str(&format!(
                "{} = {}    [{}]",
                rule.key,
                rule.value,
                labels(&rule.sources)
            ));
            if rule.admin_widened {
                output.push_str(" (widened by admin config)");
            }
            output.push('\n');
            for request in &rule.refused {
                output.push_str(&format!("    {}\n", refusal(request)));
            }
            // A list whose entries all share one source says so on its own
            // line already.
            if let Some(entries) = &rule.entries
                && rule.sources.len() > 1
            {
                for entry in entries {
                    output.push_str(&format!(
                        "    {}    [{}]\n",
                        entry.value, entry.source.label
                    ));
                }
            }
        }
        if !self.other_refused.is_empty() {
            output.push_str("\nOther refused requests:\n");
            for request in &self.other_refused {
                output.push_str(&format!("    {}\n", refusal(request)));
            }
        }
        output
    }
}
