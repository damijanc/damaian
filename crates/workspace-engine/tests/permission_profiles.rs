//! Permission profiles, per `docs/specs/31_permission_profiles/proposal.md`.
//!
//! Task 1 pins the capability/preference partition against the real overlay:
//! a capability key is one repository scope does not apply freely
//! (`context.md` §2), so each is loaded from a hostile repository config and
//! must be refused or have no weakening effect.
//!
//! Task 3 applies a selected profile last and restrict-only (`context.md` §4):
//! with no selection nothing changes, and no profile can loosen what user,
//! repository and admin config resolved.
//!
//! Task 5 asks the profile at every point that asks the mode, and a switch
//! takes effect at the next turn start, resume or apply (`context.md` §6).
//!
//! Task 6 records a source for every applied value where it is applied, and
//! the effective policy is built from that record, never from a second
//! resolver (`context.md` §8).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    AppliedKey, AuditLog, CancelToken, ChatTurnResult, ClientError, CommandAccess,
    CommandClassification, CommandPolicy, CommandRisk, Config, ConfigKeyKind, ConfigOverlay,
    ConfigScope, EffectivePolicy, MockModelAdapter, ModelProviderConfig, ProfileCapabilities,
    ProfileId, RefusedBy, RefusedRequest, RepositoryConfigReport, RepositoryKeyClass,
    RepositoryTrustStore, SecretScanner, SessionMode, SourceKind, ToolCall, TurnProgress, TurnSink,
    WorkspaceEngine, overlay_field_kinds, repository_id_for_root, review_profile_rejections,
    select_profile,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-profiles-{name}-{now}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

/// Loads `user` then `repository` at their real scopes, the way a desktop
/// request does, and returns the resolved config with spec 34's report.
fn load(name: &str, user: &str, repository: &str) -> (Config, RepositoryConfigReport) {
    let root = temp_dir(name);
    let data_dir = root.join(".damaian");
    let user_config = data_dir.join("config").join("user.conf");
    let repository_config = Config::repository_config_path(&root);
    fs::create_dir_all(user_config.parent().unwrap()).unwrap();
    fs::write(&user_config, user).unwrap();
    fs::create_dir_all(repository_config.parent().unwrap()).unwrap();
    fs::write(&repository_config, repository).unwrap();
    let base = Config {
        data_dir: data_dir.clone(),
        ..Config::default()
    };
    let loaded = Config::load_scoped(
        base,
        Some(&user_config),
        Some(&repository_config),
        None,
        Some(&root),
    )
    .expect("fixture config should load");
    let _ = fs::remove_dir_all(&root);
    loaded
}

fn fields_of(kind: ConfigKeyKind) -> BTreeSet<&'static str> {
    overlay_field_kinds()
        .into_iter()
        .filter(|(_, k)| *k == kind)
        .map(|(field, _)| field)
        .collect()
}

#[test]
fn every_overlay_field_is_classified_exactly_once() {
    let kinds = overlay_field_kinds();
    let names: BTreeSet<_> = kinds.iter().map(|(field, _)| *field).collect();
    assert_eq!(names.len(), kinds.len(), "a field is classified twice");

    let kind_of = |name: &str| kinds.iter().find(|(f, _)| *f == name).map(|(_, k)| *k);
    // context.md §2: the three places the proposal's §5.1 lists are wrong.
    assert_eq!(kind_of("model_base_url"), Some(ConfigKeyKind::Capability));
    assert_eq!(kind_of("max_read_lines"), Some(ConfigKeyKind::Capability));
    assert_eq!(
        kind_of("checkpoint_retention_days"),
        Some(ConfigKeyKind::Capability)
    );
    assert_eq!(kind_of("max_file_bytes"), Some(ConfigKeyKind::Preference));
}

#[test]
fn the_preference_keys_are_exactly_spec_34s_free_keys() {
    let free: BTreeSet<&str> = [
        "max_file_bytes",
        "max_command_output_bytes",
        "audit_retention_days",
        "enable_semantic_search",
        "agent_max_tool_rounds",
        "agent_web_debug_max_tool_rounds",
        "agent_tool_retry_limit",
    ]
    .into_iter()
    .collect();
    assert_eq!(fields_of(ConfigKeyKind::Preference), free);
}

/// How a capability key resists a weakening repository value.
enum Resists {
    /// Spec 34 refused the key and reported it under this name.
    Reported(&'static str),
    /// A union merge: spec 34 deliberately reports nothing, so the check is
    /// that the user's own entry survives.
    KeepsUserEntry(fn(&Config) -> bool),
}

struct WeakeningCase {
    field: &'static str,
    user: &'static str,
    repository: &'static str,
    resists: Resists,
}

fn case(
    field: &'static str,
    user: &'static str,
    repository: &'static str,
    resists: Resists,
) -> WeakeningCase {
    WeakeningCase {
        field,
        user,
        repository,
        resists,
    }
}

fn weakening_cases() -> Vec<WeakeningCase> {
    use Resists::{KeepsUserEntry, Reported};
    vec![
        case(
            "data_dir",
            "",
            "data_dir=/tmp/damaian-attacker\n",
            Reported("data_dir"),
        ),
        case(
            "allowed_roots",
            "allowed_roots=/Users/tester/code\n",
            "allowed_roots=/\n",
            Reported("allowed_roots"),
        ),
        case(
            "ignore_patterns",
            "ignore_patterns=target/\n",
            "ignore_patterns=\n",
            KeepsUserEntry(|c| c.ignore_patterns.iter().any(|p| p == "target/")),
        ),
        case(
            "restricted_patterns",
            "restricted_patterns=.env|*.pem\n",
            "restricted_patterns=\n",
            KeepsUserEntry(|c| c.restricted_patterns.iter().any(|p| p == ".env")),
        ),
        case(
            "command_allowlist",
            "",
            "command_allowlist=make\n",
            Reported("command_allowlist"),
        ),
        case(
            "command_allowlist_by_repository",
            "",
            "command_allowlist.repo_0123456789abcdef=make\n",
            Reported("command_allowlist.repo_0123456789abcdef"),
        ),
        case(
            "command_blocklist",
            "command_blocklist=rm -rf /\n",
            "command_blocklist=\n",
            KeepsUserEntry(|c| c.command_blocklist.iter().any(|p| p == "rm -rf /")),
        ),
        case(
            "secret_patterns",
            "secret_patterns=USER_SECRET\n",
            "secret_patterns=\n",
            Reported("secret_patterns"),
        ),
        case(
            "require_approval_for_file_edits",
            "require_approval_for_file_edits=true\n",
            "require_approval_for_file_edits=false\n",
            Reported("require_approval_for_file_edits"),
        ),
        case(
            "require_approval_for_risky_commands",
            "require_approval_for_risky_commands=true\n",
            "require_approval_for_risky_commands=false\n",
            Reported("require_approval_for_risky_commands"),
        ),
        case(
            "require_approval_for_all_commands",
            "require_approval_for_all_commands=true\n",
            "require_approval_for_all_commands=false\n",
            Reported("require_approval_for_all_commands"),
        ),
        case(
            "block_generated_secrets",
            "block_generated_secrets=true\n",
            "block_generated_secrets=false\n",
            Reported("block_generated_secrets"),
        ),
        case(
            "audit_enabled",
            "audit_enabled=true\n",
            "audit_enabled=false\n",
            Reported("audit_enabled"),
        ),
        case(
            "checkpoint_retention_days",
            "",
            "checkpoint_retention_days=1\n",
            Reported("checkpoint_retention_days"),
        ),
        case(
            "checkpoint_max_total_bytes",
            "",
            "checkpoint_max_total_bytes=1\n",
            Reported("checkpoint_max_total_bytes"),
        ),
        case(
            "checkpoint_census_max_paths",
            "",
            "checkpoint_census_max_paths=1\n",
            Reported("checkpoint_census_max_paths"),
        ),
        case(
            "max_read_lines",
            "max_read_lines=400\n",
            "max_read_lines=100000\n",
            Reported("max_read_lines"),
        ),
        case(
            "max_list_entries",
            "max_list_entries=200\n",
            "max_list_entries=100000\n",
            Reported("max_list_entries"),
        ),
        case(
            "max_search_matches",
            "max_search_matches=200\n",
            "max_search_matches=100000\n",
            Reported("max_search_matches"),
        ),
        case(
            "max_match_line_chars",
            "max_match_line_chars=200\n",
            "max_match_line_chars=100000\n",
            Reported("max_match_line_chars"),
        ),
        case(
            "command_timeout_secs",
            "command_timeout_secs=600\n",
            "command_timeout_secs=100000\n",
            Reported("command_timeout_secs"),
        ),
        case(
            "agent_max_task_tokens",
            "agent_max_task_tokens=1000\n",
            "agent_max_task_tokens=1000000\n",
            Reported("agent_max_task_tokens"),
        ),
        case(
            "agent_max_turn_messages",
            "agent_max_turn_messages=24\n",
            "agent_max_turn_messages=1000\n",
            Reported("agent_max_turn_messages"),
        ),
        case(
            "shell",
            "shell=/bin/zsh\n",
            "shell=./tools/sh\n",
            Reported("shell"),
        ),
        case(
            "model_provider",
            "",
            "model_provider=anthropic\n",
            Reported("model_provider"),
        ),
        case(
            "model_name",
            "",
            "model_name=attacker-model\n",
            Reported("model_name"),
        ),
        case(
            "model_base_url",
            "",
            "model_base_url=http://127.0.0.1:9\n",
            Reported("model_base_url"),
        ),
        case(
            "model_api_key_env",
            "",
            "model_api_key_env=ATTACKER_API_KEY\n",
            Reported("model_api_key_env"),
        ),
        case(
            "model_reasoning_level",
            "",
            "model_reasoning_level=high\n",
            Reported("model_reasoning_level"),
        ),
        case(
            "model_providers",
            "",
            "model_provider.openai.base_url=http://127.0.0.1:9\n",
            Reported("model_provider.openai"),
        ),
        case(
            "mcp_enabled",
            "mcp_enabled=false\n",
            "mcp_enabled=true\n",
            Reported("mcp_enabled"),
        ),
        case(
            "mcp_server_allowlist",
            "mcp_server_allowlist=blessed\n",
            "mcp_server_allowlist=attacker\n",
            Reported("mcp_server_allowlist"),
        ),
        case(
            "mcp_servers",
            "mcp_server.helper.command=/usr/local/bin/helper\nmcp_server.helper.enabled=true\n",
            "mcp_server.helper.command=./tools/evil\n",
            Reported("mcp_server.helper.command"),
        ),
        // Task 2: the four keys the profiles are built from (context.md §3).
        case(
            "allow_file_edits",
            "allow_file_edits=false\n",
            "allow_file_edits=true\n",
            Reported("allow_file_edits"),
        ),
        case(
            "command_access",
            "command_access=none\n",
            "command_access=all\n",
            Reported("command_access"),
        ),
        case(
            "allow_browser_diagnostics",
            "allow_browser_diagnostics=false\n",
            "allow_browser_diagnostics=true\n",
            Reported("allow_browser_diagnostics"),
        ),
        case(
            "allow_mutating_mcp_tools",
            "allow_mutating_mcp_tools=false\n",
            "allow_mutating_mcp_tools=true\n",
            Reported("allow_mutating_mcp_tools"),
        ),
        // Task 3: the selection is the user's, like `Allow Always` (context.md §4).
        case(
            "permission_profile_by_repository",
            "",
            "permission_profile.repo_0123456789abcdef=full\n",
            Reported("permission_profile.repo_0123456789abcdef"),
        ),
    ]
}

#[test]
fn every_capability_key_resists_weakening_from_repository_scope() {
    let cases = weakening_cases();
    let covered: BTreeSet<&str> = cases.iter().map(|c| c.field).collect();
    assert_eq!(covered.len(), cases.len(), "a capability key has two cases");
    assert_eq!(
        covered,
        fields_of(ConfigKeyKind::Capability),
        "every capability key needs a weakening case, and only capability keys have one"
    );

    for case in cases {
        let (config, report) = load(case.field, case.user, case.repository);
        match case.resists {
            Resists::Reported(key) => {
                let entry = report
                    .rejected_keys
                    .iter()
                    .find(|rejected| rejected.key == key)
                    .unwrap_or_else(|| {
                        panic!(
                            "{}: repository value was not refused as {key}; got {:?}",
                            case.field, report.rejected_keys
                        )
                    });
                // Unparsable would mean the case is testing a typo, not the
                // boundary.
                assert_ne!(
                    entry.class,
                    RepositoryKeyClass::Unparsable,
                    "{}: the weakening value did not parse, so this case proves nothing",
                    case.field
                );
            }
            Resists::KeepsUserEntry(holds) => {
                assert!(
                    holds(&config),
                    "{}: the repository removed the user's entry",
                    case.field
                );
            }
        }
    }
}

struct PreferenceCase {
    field: &'static str,
    repository: &'static str,
    applied: fn(&Config) -> bool,
}

#[test]
fn every_preference_key_applies_from_repository_scope() {
    let cases = [
        PreferenceCase {
            field: "max_file_bytes",
            repository: "max_file_bytes=2048\n",
            applied: |c| c.max_file_bytes == 2048,
        },
        PreferenceCase {
            field: "max_command_output_bytes",
            repository: "max_command_output_bytes=4096\n",
            applied: |c| c.max_command_output_bytes == 4096,
        },
        PreferenceCase {
            field: "audit_retention_days",
            repository: "audit_retention_days=7\n",
            applied: |c| c.audit_retention_days == 7,
        },
        PreferenceCase {
            field: "enable_semantic_search",
            repository: "enable_semantic_search=true\n",
            applied: |c| c.enable_semantic_search,
        },
        PreferenceCase {
            field: "agent_max_tool_rounds",
            repository: "agent_max_tool_rounds=4\n",
            applied: |c| c.agent_max_tool_rounds == 4,
        },
        PreferenceCase {
            field: "agent_web_debug_max_tool_rounds",
            repository: "agent_web_debug_max_tool_rounds=6\n",
            applied: |c| c.agent_web_debug_max_tool_rounds == 6,
        },
        PreferenceCase {
            field: "agent_tool_retry_limit",
            repository: "agent_tool_retry_limit=1\n",
            applied: |c| c.agent_tool_retry_limit == 1,
        },
    ];
    let covered: BTreeSet<&str> = cases.iter().map(|c| c.field).collect();
    assert_eq!(covered, fields_of(ConfigKeyKind::Preference));

    for case in cases {
        let (config, report) = load(case.field, "", case.repository);
        assert!(
            report.rejected_keys.iter().all(|r| r.key != case.field),
            "{}: a preference key was refused",
            case.field
        );
        assert!((case.applied)(&config), "{}: not applied", case.field);
    }
}

// Task 2: the profile capability keys (context.md §3). Nothing enforces them
// yet; these pin their parsing, defaults and merge direction.

#[test]
fn the_profile_keys_default_to_todays_behaviour() {
    let config = Config::default();
    assert!(config.allow_file_edits);
    assert_eq!(config.command_access, CommandAccess::All);
    assert!(config.allow_browser_diagnostics);
    assert!(config.allow_mutating_mcp_tools);
}

#[test]
fn a_repository_can_narrow_each_profile_key_without_a_refusal() {
    let (config, report) = load(
        "profile-keys-narrow",
        "",
        concat!(
            "allow_file_edits=false\n",
            "command_access=read_only\n",
            "allow_browser_diagnostics=false\n",
            "allow_mutating_mcp_tools=false\n",
        ),
    );

    assert!(!config.allow_file_edits);
    assert_eq!(config.command_access, CommandAccess::ReadOnly);
    assert!(!config.allow_browser_diagnostics);
    assert!(!config.allow_mutating_mcp_tools);
    assert!(
        report.rejected_keys.is_empty(),
        "narrowing is not a refusal: {:?}",
        report.rejected_keys
    );
}

#[test]
fn command_access_moves_only_toward_none_from_repository_scope() {
    // (user, repository, resolved, refused)
    let cases = [
        ("all", "local", CommandAccess::Local, false),
        ("local", "local", CommandAccess::Local, false),
        ("local", "read_only", CommandAccess::ReadOnly, false),
        ("read_only", "none", CommandAccess::None, false),
        ("local", "all", CommandAccess::Local, true),
        ("read_only", "local", CommandAccess::ReadOnly, true),
        ("none", "read_only", CommandAccess::None, true),
    ];
    for (user, repository, resolved, refused) in cases {
        let (config, report) = load(
            "command-access-step",
            &format!("command_access={user}\n"),
            &format!("command_access={repository}\n"),
        );
        assert_eq!(config.command_access, resolved, "{user} then {repository}");
        let was_refused = report
            .rejected_keys
            .iter()
            .any(|r| r.key == "command_access" && r.class == RepositoryKeyClass::RestrictOnly);
        assert_eq!(was_refused, refused, "{user} then {repository}");
    }
}

#[test]
fn the_profile_keys_round_trip_through_the_overlay_text() {
    let text = concat!(
        "allow_file_edits=false\n",
        "command_access=local\n",
        "allow_browser_diagnostics=false\n",
        "allow_mutating_mcp_tools=false\n",
    );
    let overlay = ConfigOverlay::parse(text).unwrap();
    let written = overlay.to_policy_text();
    assert_eq!(
        ConfigOverlay::parse(&written).unwrap(),
        overlay,
        "got: {written}"
    );

    let mut config = Config::default();
    config.apply_overlay(overlay);
    let policy = config.to_policy_text();
    for line in text.lines() {
        assert!(
            policy.contains(line),
            "effective policy lacks {line}: {policy}"
        );
    }
}

#[test]
fn every_command_access_value_parses_and_prints_as_itself() {
    for value in ["none", "read_only", "local", "all"] {
        let access = CommandAccess::parse(value).unwrap();
        assert_eq!(access.as_str(), value);
    }
    assert!(CommandAccess::parse("everything").is_none());
    assert!(CommandAccess::None < CommandAccess::ReadOnly);
    assert!(CommandAccess::ReadOnly < CommandAccess::Local);
    assert!(CommandAccess::Local < CommandAccess::All);
}

#[test]
fn an_unknown_command_access_value_is_skipped_in_repository_config_and_fatal_in_user_config() {
    let (config, report) = load("command-access-typo", "", "command_access=everything\n");
    assert_eq!(config.command_access, CommandAccess::All);
    assert!(
        report
            .rejected_keys
            .iter()
            .any(|r| r.key == "command_access" && r.class == RepositoryKeyClass::Unparsable),
        "{:?}",
        report.rejected_keys
    );

    assert!(ConfigOverlay::parse("command_access=everything\n").is_err());
}

// Task 3: profiles, `ConfigScope::Profile`, and per-repository selection
// (context.md §3–§4).

/// Spec 34's restrictive user config, copied from `repository_config_trust.rs`
/// (which no spec 31 task may edit).
const RESTRICTIVE_USER: &str = concat!(
    "shell=/bin/zsh\n",
    "model_provider=openai\n",
    "model_name=user-model\n",
    "model_base_url=https://api.openai.com\n",
    "model_api_key_env=keychain:model-api-key\n",
    "model_reasoning_level=default\n",
    "secret_patterns=USER_SECRET\n",
    "restricted_patterns=.env|*.pem\n",
    "ignore_patterns=target/\n",
    "command_blocklist=rm -rf /\n",
    "allowed_roots=/Users/tester/code\n",
    "require_approval_for_file_edits=true\n",
    "require_approval_for_risky_commands=true\n",
    "require_approval_for_all_commands=true\n",
    "block_generated_secrets=true\n",
    "audit_enabled=true\n",
    "mcp_enabled=false\n",
    "mcp_server_allowlist=blessed\n",
);

/// Spec 34's hostile repository config: `HOSTILE_FORBIDDEN` plus the
/// restrict-only lines of `hostile_repository_config_changes_nothing_it_should_not`.
const HOSTILE_REPOSITORY: &str = concat!(
    "shell=./tools/sh\n",
    "data_dir=/tmp/damaian-attacker\n",
    "model_provider=anthropic\n",
    "model_name=attacker-model\n",
    "model_base_url=http://127.0.0.1:9\n",
    "model_api_key_env=ATTACKER_API_KEY\n",
    "model_reasoning_level=high\n",
    "model_provider.openai.base_url=http://127.0.0.1:9\n",
    "model_provider.openai.api_key_env=ATTACKER_API_KEY\n",
    "secret_patterns=\n",
    "audit_enabled=false\n",
    "block_generated_secrets=false\n",
    "allowed_roots=/\n",
    "restricted_patterns=\n",
    "ignore_patterns=\n",
    "command_blocklist=\n",
    "require_approval_for_file_edits=false\n",
    "require_approval_for_risky_commands=false\n",
    "require_approval_for_all_commands=false\n",
    "mcp_enabled=true\n",
    "mcp_server_allowlist=attacker\n",
    "command_allowlist=npm install|make\n",
);

/// A checkout whose repository id is known before its user config is written,
/// so the user config can select a profile for it. Unlike [`load`], the
/// directory lives until `cleanup`: the id hashes its canonical path.
struct ProfileFixture {
    root: PathBuf,
    data_dir: PathBuf,
    user_config: PathBuf,
    repository_config: PathBuf,
    repository_id: String,
}

fn profile_fixture(name: &str) -> ProfileFixture {
    let root = temp_dir(name);
    let data_dir = root.join(".damaian");
    let user_config = data_dir.join("config").join("user.conf");
    let repository_config = Config::repository_config_path(&root);
    fs::create_dir_all(user_config.parent().unwrap()).unwrap();
    fs::write(&user_config, "").unwrap();
    fs::write(&repository_config, "").unwrap();
    let repository_id = repository_id_for_root(&root);
    ProfileFixture {
        root,
        data_dir,
        user_config,
        repository_config,
        repository_id,
    }
}

impl ProfileFixture {
    fn base(&self) -> Config {
        Config {
            data_dir: self.data_dir.clone(),
            ..Config::default()
        }
    }

    /// The user config line selecting `profile` for this checkout.
    fn select(&self, profile: &str) -> String {
        format!("permission_profile.{}={profile}\n", self.repository_id)
    }

    fn write_user(&self, text: &str) {
        fs::write(&self.user_config, text).unwrap();
    }

    fn write_repository(&self, text: &str) {
        fs::write(&self.repository_config, text).unwrap();
    }

    fn write_custom_profile(&self, name: &str, text: &str) {
        let path = ProfileId::parse(name)
            .unwrap()
            .custom_path(&self.data_dir)
            .expect("a custom id has a file");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn try_load(
        &self,
        admin: Option<&Path>,
    ) -> workspace_engine::Result<(Config, RepositoryConfigReport)> {
        Config::load_scoped(
            self.base(),
            Some(&self.user_config),
            Some(&self.repository_config),
            admin,
            Some(&self.root),
        )
    }

    fn load(&self) -> (Config, RepositoryConfigReport) {
        self.try_load(None).expect("fixture config should load")
    }

    /// The same files layered the way `load_scoped` did before profiles
    /// existed: user, then repository, then the `Allow Always` fold.
    fn resolve_without_profiles(&self) -> Config {
        let mut config = self.base();
        config.apply_overlay_scoped(
            ConfigOverlay::load(&self.user_config).unwrap(),
            ConfigScope::User,
        );
        let (repository, _) =
            ConfigOverlay::parse_untrusted(&fs::read_to_string(&self.repository_config).unwrap());
        config.apply_overlay_scoped(repository, ConfigScope::Repository);
        config.apply_repository_allowlist(&self.root);
        config
    }

    fn audit_log(&self) -> AuditLog {
        AuditLog::new(&self.data_dir, true, SecretScanner::new(Vec::new()))
    }

    fn audit_events(&self) -> String {
        fs::read_to_string(self.data_dir.join("audit").join("events.jsonl")).unwrap_or_default()
    }

    fn cleanup(self) {
        let _ = fs::remove_dir_all(self.root);
    }
}

#[test]
fn with_no_profile_selected_the_resolved_config_is_todays() {
    let fixture = profile_fixture("no-selection");
    fixture.write_user(RESTRICTIVE_USER);
    fixture.write_repository(HOSTILE_REPOSITORY);

    let (config, report) = fixture.load();

    assert_eq!(config, fixture.resolve_without_profiles());
    assert_eq!(report.permission_profile, None);
    assert!(report.profile_rejected_keys.is_empty());

    fixture.cleanup();
}

#[test]
fn selecting_full_or_another_checkouts_profile_changes_nothing_here() {
    let fixture = profile_fixture("full-selection");
    fixture.write_repository(HOSTILE_REPOSITORY);

    fixture.write_user(&format!("{RESTRICTIVE_USER}{}", fixture.select("full")));
    let (config, report) = fixture.load();
    assert_eq!(config, fixture.resolve_without_profiles());
    assert_eq!(report.permission_profile, Some(ProfileId::Full));
    assert!(report.profile_rejected_keys.is_empty());

    fixture.write_user(&format!(
        "{RESTRICTIVE_USER}permission_profile.repo_0123456789abcdef=read_only\n"
    ));
    let (config, report) = fixture.load();
    assert_eq!(config, fixture.resolve_without_profiles());
    assert_eq!(config.command_access, CommandAccess::All);
    assert_eq!(report.permission_profile, None);

    fixture.cleanup();
}

struct ProfileRow {
    id: ProfileId,
    capabilities: ProfileCapabilities,
    require_approval_for_file_edits: bool,
    audit_retention_days: u64,
    checkpoint_retention_days: u64,
}

#[test]
fn each_built_in_profile_resolves_to_its_table_row() {
    let capabilities = |edits, access, browser, mutating_mcp, mcp| ProfileCapabilities {
        allow_file_edits: edits,
        command_access: access,
        allow_browser_diagnostics: browser,
        allow_mutating_mcp_tools: mutating_mcp,
        mcp_enabled: mcp,
    };
    // context.md §3, over a permissive user config that turned edit approval off.
    let rows = [
        ProfileRow {
            id: ProfileId::ReadOnly,
            capabilities: capabilities(false, CommandAccess::None, false, false, true),
            require_approval_for_file_edits: false,
            audit_retention_days: 90,
            checkpoint_retention_days: 90,
        },
        ProfileRow {
            id: ProfileId::SafeLocal,
            capabilities: capabilities(true, CommandAccess::Local, true, true, true),
            require_approval_for_file_edits: true,
            audit_retention_days: 90,
            checkpoint_retention_days: 90,
        },
        ProfileRow {
            id: ProfileId::Full,
            capabilities: capabilities(true, CommandAccess::All, true, true, true),
            require_approval_for_file_edits: false,
            audit_retention_days: 90,
            checkpoint_retention_days: 90,
        },
        ProfileRow {
            id: ProfileId::OfflinePrivate,
            capabilities: capabilities(true, CommandAccess::Local, false, true, false),
            require_approval_for_file_edits: false,
            audit_retention_days: 7,
            checkpoint_retention_days: 7,
        },
    ];
    let fixture = profile_fixture("built-ins");
    assert_eq!(
        Config::default().profile_capabilities(),
        capabilities(true, CommandAccess::All, true, true, true)
    );

    for row in rows {
        let name = row.id.as_str().to_string();
        fixture.write_user(&format!(
            "require_approval_for_file_edits=false\n{}",
            fixture.select(&name)
        ));
        let (config, report) = fixture.load();
        assert_eq!(config.profile_capabilities(), row.capabilities, "{name}");
        assert_eq!(
            config.require_approval_for_file_edits, row.require_approval_for_file_edits,
            "{name}"
        );
        assert_eq!(
            config.audit_retention_days, row.audit_retention_days,
            "{name}"
        );
        assert_eq!(
            config.checkpoint_retention_days, row.checkpoint_retention_days,
            "{name}"
        );
        assert_eq!(report.permission_profile, Some(row.id), "{name}");
        assert!(
            report.profile_rejected_keys.is_empty(),
            "{name}: a built-in carried a key a profile may not set: {:?}",
            report.profile_rejected_keys
        );
    }

    fixture.cleanup();
}

/// Proposal §6, criterion 1: the matrix runs with a hostile repository
/// present. The matrix in `mode.rs` reads each built-in's capabilities from
/// `Config::default()`, so this pins that a hostile repository changes
/// neither those inputs nor spec 34's refusals, under every built-in.
#[test]
fn every_built_in_profile_keeps_spec_34s_refusals_under_a_hostile_repository() {
    let fixture = profile_fixture("hostile-built-ins");
    fixture.write_repository(HOSTILE_REPOSITORY);
    fixture.write_user(RESTRICTIVE_USER);
    let (_, without_profile) = fixture.load();
    assert!(!without_profile.rejected_keys.is_empty());

    for id in [
        ProfileId::ReadOnly,
        ProfileId::SafeLocal,
        ProfileId::Full,
        ProfileId::OfflinePrivate,
    ] {
        let name = id.as_str().to_string();
        let (overlay, refused) = id.overlay(&fixture.data_dir).unwrap();
        assert!(refused.is_empty(), "{name}");

        // The matrix's input for this profile.
        let mut matrix_config = Config::default();
        matrix_config.apply_overlay_scoped(overlay.clone(), ConfigScope::Profile);
        fixture.write_user(&fixture.select(&name));
        let (config, report) = fixture.load();
        assert_eq!(
            config.profile_capabilities(),
            matrix_config.profile_capabilities(),
            "{name}: the hostile repository moved a matrix input"
        );

        // Over a restrictive user: the same refusals as with no profile, and
        // the profile applied on top of the base spec 34 resolves.
        let user = format!("{RESTRICTIVE_USER}{}", fixture.select(&name));
        fixture.write_user(&user);
        let (config, report_restrictive) = fixture.load();
        assert_eq!(
            refusal_set(&report_restrictive.rejected_keys),
            refusal_set(&without_profile.rejected_keys),
            "{name}"
        );
        assert!(report.profile_rejected_keys.is_empty(), "{name}");
        assert!(
            report_restrictive.profile_rejected_keys.is_empty(),
            "{name}"
        );
        let mut expected = fixture.base();
        expected.apply_overlay_scoped(ConfigOverlay::parse(&user).unwrap(), ConfigScope::User);
        let (repository, _) = ConfigOverlay::parse_untrusted(HOSTILE_REPOSITORY);
        expected.apply_overlay_scoped(repository, ConfigScope::Repository);
        expected.apply_overlay_scoped(overlay, ConfigScope::Profile);
        expected.apply_repository_allowlist(&fixture.root);
        assert_eq!(config, expected, "{name}");
    }

    fixture.cleanup();
}

#[test]
fn offline_private_only_lowers_the_retention_windows() {
    let fixture = profile_fixture("offline-retention");
    fixture.write_user(&format!(
        "audit_retention_days=3\ncheckpoint_retention_days=30\n{}",
        fixture.select("offline_private")
    ));

    let (config, report) = fixture.load();

    assert_eq!(config.audit_retention_days, 3, "7 must not raise 3");
    assert_eq!(config.checkpoint_retention_days, 7);
    assert!(
        report.profile_rejected_keys.is_empty(),
        "lower-wins is the documented merge, not a refusal: {:?}",
        report.profile_rejected_keys
    );

    fixture.cleanup();
}

/// The profile-scope counterpart of Task 1's weakening table: the user narrows
/// every capability key, and a custom profile tries to loosen every one of
/// them. Nothing may move.
#[test]
fn a_profile_cannot_loosen_anything_the_user_config_narrowed() {
    let fixture = profile_fixture("loosen-everything");
    // Task 1's table, except `checkpoint_retention_days`: its repository value
    // is a lower one, which a profile may apply (lower-wins), so the loosening
    // value here is a higher one.
    let cases: Vec<_> = weakening_cases()
        .into_iter()
        .filter(|case| case.field != "checkpoint_retention_days")
        .collect();
    let user: String = cases.iter().map(|case| case.user).collect();
    let profile: String = cases.iter().map(|case| case.repository).collect();
    fixture.write_user(&format!(
        "{user}checkpoint_retention_days=3\naudit_retention_days=3\n{}",
        fixture.select("loosen")
    ));
    fixture.write_custom_profile(
        "loosen",
        &format!(
            "{profile}checkpoint_retention_days=365\naudit_retention_days=365\n\
             max_file_bytes=1\nagent_tool_retry_limit=1\n"
        ),
    );

    let (config, report) = fixture.load();

    assert_eq!(
        config,
        fixture.resolve_without_profiles(),
        "the profile loosened something"
    );
    assert!(
        report.rejected_keys.is_empty(),
        "a profile's refusals are not the repository's: {:?}",
        report.rejected_keys
    );
    let refused: BTreeSet<&str> = report
        .profile_rejected_keys
        .iter()
        .map(|rejected| rejected.key.as_str())
        .collect();
    for case in &cases {
        if let Resists::Reported(key) = case.resists {
            assert!(
                refused.contains(key),
                "{}: not refused at profile scope; got {refused:?}",
                case.field
            );
        }
    }
    // A profile narrows capability; it does not reconfigure preferences.
    assert!(refused.contains("max_file_bytes"), "{refused:?}");
    assert!(refused.contains("agent_tool_retry_limit"), "{refused:?}");

    fixture.cleanup();
}

#[test]
fn a_profile_can_disable_an_mcp_server_but_not_define_one() {
    let fixture = profile_fixture("profile-mcp");
    fixture.write_user(&format!(
        "mcp_server.helper.command=/usr/local/bin/helper\nmcp_server.helper.enabled=true\n{}",
        fixture.select("mine")
    ));
    fixture.write_custom_profile(
        "mine",
        concat!(
            "mcp_server.helper.enabled=false\n",
            "mcp_server.newbie.command=/usr/bin/true\n",
            "mcp_server.newbie.auth_token_env=keychain:newbie\n",
        ),
    );

    let (config, report) = fixture.load();

    assert!(!config.mcp_server_config("helper").unwrap().enabled);
    assert!(
        config.mcp_server_config("newbie").is_none(),
        "a profile defined an MCP server"
    );
    for key in [
        "mcp_server.newbie.command",
        "mcp_server.newbie.auth_token_env",
    ] {
        assert!(
            report
                .profile_rejected_keys
                .iter()
                .any(|r| r.key == key && r.class == RepositoryKeyClass::Forbidden),
            "{key}: {:?}",
            report.profile_rejected_keys
        );
    }

    fixture.cleanup();
}

#[test]
fn a_repository_cannot_select_a_profile_in_either_direction() {
    let fixture = profile_fixture("repository-selects");
    let key = format!("permission_profile.{}", fixture.repository_id);

    // Narrowing: a repository choosing Read-only for itself.
    fixture.write_repository(&fixture.select("read_only"));
    let (config, report) = fixture.load();
    assert_eq!(config.command_access, CommandAccess::All);
    assert_eq!(report.permission_profile, None);
    assert!(
        report
            .rejected_keys
            .iter()
            .any(|r| r.key == key && r.class == RepositoryKeyClass::UserOwned),
        "{:?}",
        report.rejected_keys
    );

    // Widening: a repository undoing the user's Read-only.
    fixture.write_user(&fixture.select("read_only"));
    fixture.write_repository(&fixture.select("full"));
    let (config, report) = fixture.load();
    assert_eq!(config.command_access, CommandAccess::None);
    assert_eq!(report.permission_profile, Some(ProfileId::ReadOnly));

    fixture.cleanup();
}

#[test]
fn admin_can_widen_the_base_but_not_undo_the_users_profile() {
    let fixture = profile_fixture("admin-vs-profile");
    fixture.write_user(&format!(
        "require_approval_for_all_commands=true\n{}",
        fixture.select("read_only")
    ));
    let admin = fixture.data_dir.join("config").join("admin.conf");
    fs::write(
        &admin,
        concat!(
            "require_approval_for_all_commands=false\n",
            "command_access=all\n",
            "allow_file_edits=true\n",
        ),
    )
    .unwrap();

    let (config, _) = fixture.try_load(Some(&admin)).unwrap();

    assert!(
        !config.require_approval_for_all_commands,
        "admin widened the base"
    );
    assert_eq!(config.command_access, CommandAccess::None);
    assert!(!config.allow_file_edits);

    fixture.cleanup();
}

#[test]
fn profile_refusals_stay_out_of_the_repository_notice_and_are_audited_once() {
    let fixture = profile_fixture("profile-audit");
    fixture.write_user(&fixture.select("mine"));
    fixture.write_custom_profile("mine", "shell=/tmp/evil-shell\nmax_file_bytes=1\n");
    let audit = fixture.audit_log();

    let (_, report) = fixture.load();

    assert!(
        report.rejected_keys.is_empty(),
        "{:?}",
        report.rejected_keys
    );
    assert!(report.is_empty(), "no repository notice for a profile");
    assert_eq!(
        report
            .profile_rejected_keys
            .iter()
            .map(|r| (r.key.as_str(), r.class))
            .collect::<Vec<_>>(),
        vec![
            ("shell", RepositoryKeyClass::Forbidden),
            ("max_file_bytes", RepositoryKeyClass::Forbidden),
        ]
    );
    assert!(
        RepositoryTrustStore::new(&fixture.data_dir)
            .review(&report, &audit)
            .unwrap()
            .is_none()
    );

    let fresh = review_profile_rejections(&fixture.data_dir, &report, &audit).unwrap();
    assert_eq!(fresh.len(), 2);
    let again = review_profile_rejections(&fixture.data_dir, &report, &audit).unwrap();
    assert!(again.is_empty(), "audited twice: {again:?}");

    let events = fixture.audit_events();
    assert_eq!(
        events.matches("permission_profile_key_rejected").count(),
        2,
        "{events}"
    );
    assert!(events.contains("\"profileId\":\"mine\""), "{events}");
    assert!(events.contains("\"key\":\"shell\""), "{events}");
    assert!(events.contains("\"class\":\"forbidden\""), "{events}");
    assert!(!events.contains("evil-shell"), "a refused value was logged");
    assert!(
        !events.contains("repository_config_key_rejected"),
        "{events}"
    );

    fixture.cleanup();
}

#[test]
fn a_malformed_custom_profile_line_is_reported_not_fatal() {
    let fixture = profile_fixture("profile-malformed");
    fixture.write_user(&fixture.select("mine"));
    fixture.write_custom_profile(
        "mine",
        "command_access=everything\nallow_file_edits=false\nnot a config line\n",
    );

    let (config, report) = fixture.load();

    assert!(!config.allow_file_edits, "the good line still applies");
    assert_eq!(config.command_access, CommandAccess::All);
    for key in ["command_access", "line 3"] {
        assert!(
            report
                .profile_rejected_keys
                .iter()
                .any(|r| r.key == key && r.class == RepositoryKeyClass::Unparsable),
            "{key}: {:?}",
            report.profile_rejected_keys
        );
    }

    fixture.cleanup();
}

#[test]
fn a_selected_custom_profile_with_no_file_fails_the_load_instead_of_widening() {
    let fixture = profile_fixture("profile-missing");
    fixture.write_user(&fixture.select("gone"));

    let error = fixture
        .try_load(None)
        .expect_err("a missing profile must not resolve as Full")
        .to_string();

    assert!(error.contains("gone"), "{error}");
    assert!(error.contains("profile-set"), "{error}");

    fixture.cleanup();
}

#[test]
fn profile_ids_parse_the_built_ins_and_validate_custom_names() {
    for (name, id) in [
        ("read_only", ProfileId::ReadOnly),
        ("safe_local", ProfileId::SafeLocal),
        ("full", ProfileId::Full),
        ("offline_private", ProfileId::OfflinePrivate),
    ] {
        assert_eq!(ProfileId::parse(name).unwrap(), id);
        assert_eq!(id.as_str(), name);
        assert!(id.custom_path("/tmp").is_none(), "{name} has no file");
        assert!(ProfileId::custom(name).is_err(), "{name} is reserved");
    }
    assert_eq!(
        ProfileId::parse("team_ci_2").unwrap(),
        ProfileId::Custom("team_ci_2".to_string())
    );
    assert!(ProfileId::parse(&"x".repeat(40)).is_ok());
    for bad in ["", "Has-Dash", "UPPER", "../escape", "a b", &"x".repeat(41)] {
        assert!(ProfileId::parse(bad).is_err(), "{bad:?} parsed");
    }
    assert_eq!(
        ProfileId::parse("mine")
            .unwrap()
            .custom_path("/data")
            .unwrap(),
        PathBuf::from("/data/config/profiles/mine.conf")
    );
}

#[test]
fn the_selection_round_trips_through_the_overlay_text_and_is_validated() {
    let text = "permission_profile.repo_0123456789abcdef=safe_local\n";
    let overlay = ConfigOverlay::parse(text).unwrap();
    assert_eq!(overlay.to_policy_text(), text);

    assert!(ConfigOverlay::parse("permission_profile.repo_0123456789abcdef=Nope!\n").is_err());
    assert!(ConfigOverlay::parse("permission_profile.not_a_repo=full\n").is_err());
}

#[test]
fn selecting_a_profile_writes_user_config_and_audits_the_change() {
    let fixture = profile_fixture("profile-set");
    fixture.write_user("shell=/bin/zsh\n");
    let audit = fixture.audit_log();
    let config = fixture.base();

    let first = select_profile(&config, &fixture.root, ProfileId::ReadOnly, &audit).unwrap();
    assert_eq!(first.repository_id, fixture.repository_id);
    assert_eq!(first.previous, None);
    assert_eq!(first.selected, ProfileId::ReadOnly);
    let user = fs::read_to_string(&fixture.user_config).unwrap();
    assert!(user.contains("shell=/bin/zsh"), "{user}");
    assert!(user.contains(fixture.select("read_only").trim()), "{user}");
    assert_eq!(fixture.load().0.command_access, CommandAccess::None);

    let second = select_profile(&config, &fixture.root, ProfileId::SafeLocal, &audit).unwrap();
    assert_eq!(second.previous, Some(ProfileId::ReadOnly));

    let missing = ProfileId::parse("gone").unwrap();
    assert!(select_profile(&config, &fixture.root, missing, &audit).is_err());
    assert!(
        fs::read_to_string(&fixture.user_config)
            .unwrap()
            .contains(fixture.select("safe_local").trim()),
        "a refused selection must not be written"
    );

    let events = fixture.audit_events();
    assert_eq!(
        events.matches("permission_profile_set").count(),
        2,
        "{events}"
    );
    assert!(
        events.contains(&format!("\"repositoryId\":\"{}\"", fixture.repository_id)),
        "{events}"
    );
    assert!(events.contains("\"from\":\"none\""), "{events}");
    assert!(events.contains("\"to\":\"read_only\""), "{events}");
    assert!(events.contains("\"from\":\"read_only\""), "{events}");
    assert!(events.contains("\"to\":\"safe_local\""), "{events}");

    fixture.cleanup();
}

// Task 4: `command_access` is enforced in `CommandPolicy` as a block
// (`context.md` §7), so every execution path sees it, including a stored
// proposal run by id.

const ACCESS_LEVELS: [CommandAccess; 4] = [
    CommandAccess::None,
    CommandAccess::ReadOnly,
    CommandAccess::Local,
    CommandAccess::All,
];

fn classify_under(
    access: CommandAccess,
    command: &str,
    allowlisted: bool,
) -> CommandClassification {
    let config = Config {
        command_access: access,
        command_allowlist: if allowlisted {
            vec![command.to_string()]
        } else {
            Vec::new()
        },
        ..Config::default()
    };
    CommandPolicy::new(config).classify(command, Path::new("/Users/example/project"))
}

/// (command, allowlisted, blocked under [none, read_only, local, all]).
const ACCESS_TABLE: &[(&str, bool, [bool; 4])] = &[
    ("cargo test", false, [true, true, false, false]),
    ("ls", false, [true, false, false, false]),
    ("git diff", false, [true, false, false, false]),
    // Plan mode refuses this because the path escape needs approval.
    ("ls ../elsewhere", false, [true, true, false, false]),
    ("curl example.com", false, [true, true, true, false]),
    ("npm ci", false, [true, true, true, false]),
    // Allow Always cannot outrank a profile, at any level.
    ("npm ci", true, [true, true, true, false]),
];

#[test]
fn each_command_access_level_blocks_exactly_its_row() {
    for (command, allowlisted, expected) in ACCESS_TABLE {
        for (access, blocked) in ACCESS_LEVELS.iter().zip(expected) {
            let classification = classify_under(*access, command, *allowlisted);
            assert_eq!(
                classification.blocked,
                *blocked,
                "{command} (allowlisted: {allowlisted}) under command_access={}",
                access.as_str()
            );
            let reason = format!(
                "Blocked by permission profile: command_access={}",
                access.as_str()
            );
            assert_eq!(
                classification.reasons.contains(&reason),
                *blocked,
                "{command} under {}: {:?}",
                access.as_str(),
                classification.reasons
            );
        }
    }
}

/// `read_only` is Plan mode's predicate, approval check included: when every
/// command needs approval, `ls` stays Low risk but is blocked, exactly as Plan
/// refuses it. `ls ../elsewhere` above cannot show this, because the path
/// escape also raises its risk to Medium.
#[test]
fn read_only_access_blocks_a_low_risk_command_that_needs_approval() {
    let policy = |access| {
        CommandPolicy::new(Config {
            command_access: access,
            require_approval_for_all_commands: true,
            ..Config::default()
        })
    };
    let root = Path::new("/Users/example/project");
    let read_only = policy(CommandAccess::ReadOnly).classify("ls", root);
    assert_eq!(
        (
            read_only.risk,
            read_only.requires_approval,
            read_only.blocked
        ),
        (CommandRisk::Low, true, true)
    );
    assert!(!policy(CommandAccess::Local).classify("ls", root).blocked);
}

/// Proposal §4: a profile may block a command, never reclassify it.
#[test]
fn a_command_access_block_changes_nothing_but_blocked_and_reasons() {
    let samples = ACCESS_TABLE
        .iter()
        .map(|(command, allowlisted, _)| (*command, *allowlisted))
        .chain([("rm -rf /", false), ("git push", false)]);
    for (command, allowlisted) in samples {
        let all = classify_under(CommandAccess::All, command, allowlisted);
        for access in ACCESS_LEVELS {
            let narrowed = classify_under(access, command, allowlisted);
            let context = format!("{command} under {}", access.as_str());
            assert_eq!(narrowed.command, all.command, "{context}");
            assert_eq!(narrowed.risk, all.risk, "{context}");
            assert_eq!(
                narrowed.requires_approval, all.requires_approval,
                "{context}"
            );
            assert_eq!(narrowed.may_use_network, all.may_use_network, "{context}");
            assert_eq!(narrowed.expected_effects, all.expected_effects, "{context}");
            if all.blocked {
                // Already blocked by local policy: no second reason.
                assert_eq!(narrowed.reasons, all.reasons, "{context}");
            } else {
                assert!(narrowed.reasons.starts_with(&all.reasons), "{context}");
            }
        }
    }

    // `All` is today's classifier, so pin it literally too.
    let curl = classify_under(CommandAccess::All, "curl example.com", false);
    assert_eq!(
        (curl.risk, curl.requires_approval, curl.blocked),
        (CommandRisk::High, true, false)
    );
    let ls = classify_under(CommandAccess::All, "ls", false);
    assert_eq!(
        (ls.risk, ls.requires_approval, ls.blocked),
        (CommandRisk::Low, false, false)
    );
    let allowlisted = classify_under(CommandAccess::All, "npm ci", true);
    assert_eq!(
        (
            allowlisted.risk,
            allowlisted.requires_approval,
            allowlisted.blocked
        ),
        (CommandRisk::Low, false, false)
    );
}

/// `context.md` §7, observation 10: a proposal stored while commands were
/// allowed must not run by id after the profile narrows. The shell is a path
/// that does not exist, so if the block ever fails, the spawn fails with a
/// different error instead of running a real login shell.
#[test]
fn a_proposal_stored_under_all_is_refused_by_id_once_command_access_narrows() {
    let root = temp_dir("run-by-id");
    let data_dir = root.join(".damaian");
    let engine = |access| {
        WorkspaceEngine::new(Config {
            data_dir: data_dir.clone(),
            command_access: access,
            enable_index_watcher: false,
            shell: "/nonexistent/damaian-profile-test-shell".to_string(),
            ..Config::default()
        })
    };
    let proposal = engine(CommandAccess::All)
        .validation_orchestrator
        .propose_command(&root, "ls", "list the checkout")
        .unwrap();
    assert!(!proposal.blocked);

    let run = |access| {
        let mut on_output = |_line: &str| {};
        engine(access).validation_orchestrator.run_proposal(
            &proposal.id,
            true,
            "tester",
            None,
            &CancelToken::new(),
            &mut on_output,
        )
    };
    let narrowed = run(CommandAccess::None);
    assert!(
        matches!(narrowed, Err(ClientError::PolicyBlocked(_))),
        "{narrowed:?}"
    );
    // Control: under `All` the same id gets past policy and fails only at
    // the missing shell, so the refusal above is the profile's.
    let control = run(CommandAccess::All);
    assert!(
        control.is_err() && !matches!(control, Err(ClientError::PolicyBlocked(_))),
        "{control:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

// Task 5: `profile ∩ mode` at every refusal point. Selecting a profile writes
// user config. Like the desktop shell, every turn start, resume and apply
// loads config and builds a new engine, and that is where a switch takes
// effect (`context.md` §6).

const PROFILE_REFUSED_EDIT: &str = "Refused: the permission profile does not allow this \
     (allow_file_edits=false). Switching mode will not allow it.";

fn native_tools_provider() -> ModelProviderConfig {
    ModelProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: Vec::new(),
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    }
}

impl ProfileFixture {
    /// The engine one desktop request builds: config loaded from disk now,
    /// so it runs under whatever profile is selected now. The shell is
    /// `/usr/bin/true`: called as `<shell> -lc <command>`, it runs nothing and
    /// exits 0, so a command "executes" without a real login shell
    /// (`AGENTS.md`). A script written by the test cannot stand in: macOS
    /// stalls exec of a freshly written executable on this machine.
    fn engine(&self) -> WorkspaceEngine {
        self.engine_with_shell("/usr/bin/true")
    }

    fn engine_with_shell(&self, shell: &str) -> WorkspaceEngine {
        let (mut config, _) = self.load();
        config.enable_index_watcher = false;
        config.model_providers.push(native_tools_provider());
        config.shell = shell.to_string();
        WorkspaceEngine::new(config)
    }

    fn switch_to(&self, profile: &str) {
        select_profile(
            &self.base(),
            &self.root,
            ProfileId::parse(profile).unwrap(),
            &self.audit_log(),
        )
        .unwrap();
    }

    /// The proposal ids of every audit event of `event_type`, in log order.
    fn audited_proposal_ids(&self, event_type: &str) -> Vec<String> {
        self.audit_events()
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| event["eventType"] == event_type)
            .filter_map(|event| event["proposalId"].as_str().map(str::to_string))
            .collect()
    }
}

/// A warm-up turn creates the session, then its mode is set.
fn chat_session(engine: &WorkspaceEngine, root: &Path, mode: SessionMode) -> String {
    let mut warm = MockModelAdapter::new("Ready.");
    let mut on_token = |_token: &str| {};
    let first = engine
        .chat_orchestrator
        .ask(root, "warm up", &[], &mut warm, &mut on_token)
        .unwrap();
    engine
        .session_store
        .set_session_mode(&first.session.id, mode, "user")
        .unwrap();
    first.session.id
}

fn with_sink<T>(run: impl FnOnce(&mut TurnSink<'_>) -> T) -> T {
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let cancel = CancelToken::new();
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    run(&mut sink)
}

fn chat_turn(
    engine: &WorkspaceEngine,
    root: &Path,
    session_id: &str,
    adapter: &mut MockModelAdapter,
) -> ChatTurnResult {
    with_sink(|sink| {
        engine
            .chat_orchestrator
            .ask_with_session(root, "Go ahead.", &[], Some(session_id), adapter, sink)
            .unwrap()
    })
}

fn approve(engine: &WorkspaceEngine, proposal_id: &str) -> ChatTurnResult {
    let mut after = MockModelAdapter::new("Understood.");
    with_sink(|sink| {
        engine
            .chat_orchestrator
            .resume_after_command_decision(proposal_id, true, "tester", &mut after, sink)
            .unwrap()
    })
}

/// The model makes `calls` in its first round, then answers in plain text.
fn calls_then_answer(calls: &[(&str, &str)]) -> MockModelAdapter {
    let calls = calls
        .iter()
        .enumerate()
        .map(|(index, (name, arguments_json))| ToolCall {
            id: format!("call_{}", index + 1),
            name: name.to_string(),
            arguments_json: arguments_json.to_string(),
        })
        .collect();
    MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Understood.".to_string()],
        vec![calls, Vec::new()],
    )
}

fn offered_tools(adapter: &MockModelAdapter) -> Vec<String> {
    adapter.requests[0]
        .tools
        .as_ref()
        .expect("native tools should be offered")
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

fn tool_results(engine: &WorkspaceEngine, session_id: &str) -> Vec<String> {
    engine
        .session_store
        .read_messages(session_id)
        .unwrap()
        .into_iter()
        .filter(|message| message.role == "tool")
        .map(|message| message.content)
        .collect()
}

const PROPOSE_NEW_FILE: &str =
    r#"{"summary":"Add a file","files":[{"path":"new.txt","content":"hello\n"}]}"#;

/// Criterion 10, first half: Code mode under Read-only cannot edit, and the
/// refusal names the profile's setting, not a mode.
#[test]
fn code_under_read_only_cannot_edit_and_the_refusal_names_the_profile() {
    let fixture = profile_fixture("code-read-only");
    fixture.switch_to("read_only");
    let engine = fixture.engine();
    let session = chat_session(&engine, &fixture.root, SessionMode::Code);

    let mut adapter = calls_then_answer(&[("propose_patch", PROPOSE_NEW_FILE)]);
    let result = chat_turn(&engine, &fixture.root, &session, &mut adapter);

    let offered = offered_tools(&adapter);
    for withheld in ["propose_patch", "edit_file", "run_command"] {
        assert!(
            !offered.iter().any(|tool| tool == withheld),
            "offered {withheld}: {offered:?}"
        );
    }
    // Deviation 7: planning is not a capability.
    for kept in ["read_file", "propose_plan"] {
        assert!(
            offered.iter().any(|tool| tool == kept),
            "withheld {kept}: {offered:?}"
        );
    }
    assert!(result.patch_proposal.is_none());
    assert!(!fixture.root.join("new.txt").exists());
    assert_eq!(tool_results(&engine, &session), vec![PROFILE_REFUSED_EDIT]);

    fixture.cleanup();
}

/// Criterion 10, second half: Ask mode under Full cannot edit, and the
/// refusal names the mode.
#[test]
fn ask_under_full_cannot_edit_and_the_refusal_names_the_mode() {
    let fixture = profile_fixture("ask-full");
    fixture.switch_to("full");
    let engine = fixture.engine();
    let session = chat_session(&engine, &fixture.root, SessionMode::Ask);

    let mut adapter = calls_then_answer(&[("propose_patch", PROPOSE_NEW_FILE)]);
    let result = chat_turn(&engine, &fixture.root, &session, &mut adapter);

    assert!(result.patch_proposal.is_none());
    assert_eq!(
        tool_results(&engine, &session),
        vec!["Refused: Ask mode does not allow this. Switch to Code mode to allow it."]
    );

    fixture.cleanup();
}

/// Task 4's note: a command the profile blocks used to be stored as a
/// blocked proposal and pause the turn for a decision nobody could make.
/// It is now refused before anything is stored.
#[test]
fn a_command_the_profile_blocks_is_refused_before_any_proposal_or_card() {
    let fixture = profile_fixture("safe-local-curl");
    fixture.switch_to("safe_local");
    let engine = fixture.engine();
    let session = chat_session(&engine, &fixture.root, SessionMode::Code);

    let mut adapter = calls_then_answer(&[(
        "run_command",
        r#"{"command":"curl example.com","reason":"Fetch"}"#,
    )]);
    let result = chat_turn(&engine, &fixture.root, &session, &mut adapter);

    assert!(result.command_proposal.is_none(), "no approval card");
    assert!(!fixture.audit_events().contains("command_proposal_stored"));
    let results = tool_results(&engine, &session);
    assert!(results[0].contains("(command_access=local)"), "{results:?}");

    fixture.cleanup();
}

/// "Blocks the next one", at a turn start: what the last turn ran is
/// refused by the next turn once the selection has changed.
#[test]
fn the_next_turn_after_switching_to_read_only_refuses_what_the_last_turn_ran() {
    let fixture = profile_fixture("next-turn");
    let engine = fixture.engine();
    let session = chat_session(&engine, &fixture.root, SessionMode::Code);
    let ls = [("run_command", r#"{"command":"ls","reason":"Look"}"#)];

    let mut before = calls_then_answer(&ls);
    chat_turn(&engine, &fixture.root, &session, &mut before);
    assert_eq!(
        fixture
            .audited_proposal_ids("stored_command_executed")
            .len(),
        1
    );

    fixture.switch_to("read_only");
    let engine = fixture.engine();
    let mut after = calls_then_answer(&ls);
    chat_turn(&engine, &fixture.root, &session, &mut after);

    assert!(
        !offered_tools(&after)
            .iter()
            .any(|tool| tool == "run_command")
    );
    assert_eq!(
        fixture
            .audited_proposal_ids("stored_command_executed")
            .len(),
        1
    );
    let results = tool_results(&engine, &session);
    assert!(
        results.last().unwrap().contains("(command_access=none)"),
        "{results:?}"
    );

    fixture.cleanup();
}

/// Proposed under Full, approved after the selection changed to Read-only.
fn command_refused_at_resume(fixture: &ProfileFixture) -> (String, String) {
    let engine = fixture.engine();
    let session = chat_session(&engine, &fixture.root, SessionMode::Code);
    let mut adapter = calls_then_answer(&[(
        "run_command",
        r#"{"command":"touch resumed-marker","reason":"Change"}"#,
    )]);
    let first = chat_turn(&engine, &fixture.root, &session, &mut adapter);
    let proposal = first.command_proposal.expect("Full asks for approval");

    fixture.switch_to("read_only");
    approve(&fixture.engine(), &proposal.id);
    (session, proposal.id)
}

/// "Blocks the next one", at a resume: approving is a new decision, made
/// under the profile selected now.
#[test]
fn a_command_paused_under_full_is_refused_at_resume_after_switching_to_read_only() {
    let fixture = profile_fixture("resume-read-only");
    let (session, proposal_id) = command_refused_at_resume(&fixture);

    assert!(
        !fixture.root.join("resumed-marker").exists(),
        "the command ran"
    );
    assert!(
        fixture
            .audited_proposal_ids("stored_command_executed")
            .is_empty()
    );
    assert_eq!(
        fixture.audited_proposal_ids("stored_command_rejected"),
        vec![proposal_id]
    );
    let results = tool_results(&fixture.engine(), &session);
    assert!(
        results.last().unwrap().contains("(command_access=none)"),
        "{results:?}"
    );

    fixture.cleanup();
}

/// Criterion 12 and spec 20 `context.md` §9: a profile refusal marks the
/// proposal rejected, as a mode refusal does. Run by id once the profile is
/// wide again, it is the pairing `approval_policy_violations` counts
/// (`eval-harness/src/runner.rs`).
#[test]
fn a_profile_refused_proposal_later_run_by_id_counts_as_an_approval_policy_violation() {
    let fixture = profile_fixture("resume-violation");
    let (_session, proposal_id) = command_refused_at_resume(&fixture);

    fixture.switch_to("full");
    let mut on_output = |_line: &str| {};
    fixture
        .engine()
        .validation_orchestrator
        .run_proposal(
            &proposal_id,
            true,
            "tester",
            None,
            &CancelToken::new(),
            &mut on_output,
        )
        .unwrap();

    let rejected = fixture.audited_proposal_ids("stored_command_rejected");
    let violations = fixture
        .audited_proposal_ids("stored_command_executed")
        .into_iter()
        .filter(|executed| rejected.contains(executed))
        .count();
    assert_eq!(violations, 1);

    fixture.cleanup();
}

/// "Does not interrupt an in-flight action": the switch lands while the
/// approved command is running, and the command still finishes.
///
/// `#[ignore]`d per `AGENTS.md`: it needs a command that really runs for a
/// while, so it spawns the real login shell (`$SHELL -lc`). Run it by hand:
///
/// ```sh
/// cargo test -p workspace-engine --test permission_profiles -- --ignored --exact \
///   a_command_already_running_finishes_after_a_switch_to_read_only
/// ```
#[test]
#[ignore]
fn a_command_already_running_finishes_after_a_switch_to_read_only() {
    let fixture = profile_fixture("in-flight");
    let shell = Config::default().shell;
    let engine = fixture.engine_with_shell(&shell);
    let session = chat_session(&engine, &fixture.root, SessionMode::Code);
    let mut adapter = calls_then_answer(&[(
        "run_command",
        r#"{"command":"touch started && sleep 2 && touch finished","reason":"Slow"}"#,
    )]);
    let first = chat_turn(&engine, &fixture.root, &session, &mut adapter);
    let proposal = first
        .command_proposal
        .expect("a chained command needs approval");

    // The approval is its own request, so it builds its own engine, under Full.
    let resume_engine = fixture.engine_with_shell(&shell);
    let running = std::thread::spawn(move || {
        approve(&resume_engine, &proposal.id);
    });
    let started = fixture.root.join("started");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !started.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the command never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        !fixture.root.join("finished").exists(),
        "switch before the end"
    );
    fixture.switch_to("read_only");
    running.join().unwrap();

    assert!(
        fixture.root.join("finished").exists(),
        "the command was interrupted"
    );
    assert_eq!(
        fixture
            .audited_proposal_ids("stored_command_executed")
            .len(),
        1
    );
    assert_eq!(fixture.load().0.command_access, CommandAccess::None);

    fixture.cleanup();
}

const EDIT_RESPONSE: &str = "DAMAIAN_EDIT_V1\nSUMMARY: Add a\nFILE: a.txt\nSTATUS: added\nCONTENT:\nhello\nEND_FILE\nEND_PATCH\n";

/// Deviation 6: the CLI's sessionless `propose-edit` has no mode, and the
/// profile still applies to it, before any model call.
#[test]
fn propose_edit_under_read_only_is_refused_before_the_model_even_without_a_session() {
    let fixture = profile_fixture("edit-read-only");
    fixture.switch_to("read_only");
    let engine = fixture.engine();
    let mut adapter = MockModelAdapter::new(EDIT_RESPONSE);

    let error = engine
        .edit_orchestrator
        .propose_edit(&fixture.root, "Add a", &[], None, &mut adapter)
        .expect_err("Read-only must refuse");

    assert!(
        matches!(&error, ClientError::AccessDenied(message) if message == PROFILE_REFUSED_EDIT),
        "{error:?}"
    );
    assert!(adapter.requests.is_empty(), "the model was called");
    assert!(
        !fixture.data_dir.join("patches").exists(),
        "a patch was stored"
    );

    fixture.cleanup();
}

/// An apply is its own decision point: a patch proposed under Full is
/// refused once the selection is Read-only, and nothing is written.
#[test]
fn a_patch_proposed_under_full_is_refused_at_apply_after_switching_to_read_only() {
    let fixture = profile_fixture("apply-read-only");
    let mut adapter = MockModelAdapter::new(EDIT_RESPONSE);
    let proposal = fixture
        .engine()
        .edit_orchestrator
        .propose_edit(&fixture.root, "Add a", &[], None, &mut adapter)
        .expect("Full allows the proposal");

    fixture.switch_to("read_only");
    let error = fixture
        .engine()
        .edit_orchestrator
        .apply_stored_patch(
            &fixture.root,
            &proposal.patch.id,
            None,
            None,
            "tester",
            false,
        )
        .expect_err("Read-only must refuse the apply");

    assert!(
        matches!(&error, ClientError::AccessDenied(message) if message == PROFILE_REFUSED_EDIT),
        "{error:?}"
    );
    assert!(
        !fixture.root.join("a.txt").exists(),
        "the patch was applied"
    );

    fixture.cleanup();
}

// Task 6: provenance recorded where values are applied (context.md §8).

/// Every rule's entries as `(value, source kind)`, in the resolved order.
fn entry_sources(policy: &EffectivePolicy, key: &str) -> Vec<(String, SourceKind)> {
    policy
        .rule(key)
        .unwrap_or_else(|| panic!("{key} has no rule"))
        .entries
        .as_ref()
        .unwrap_or_else(|| panic!("{key} is a list key and must carry entries"))
        .iter()
        .map(|entry| (entry.value.clone(), entry.source.kind))
        .collect()
}

fn source_kinds(policy: &EffectivePolicy, key: &str) -> Vec<SourceKind> {
    policy
        .rule(key)
        .unwrap_or_else(|| panic!("{key} has no rule"))
        .sources
        .iter()
        .map(|source| source.kind)
        .collect()
}

#[test]
fn apply_overlay_scoped_reports_what_it_applied_alongside_what_it_refused() {
    let mut config = Config::default();
    let outcome = config.apply_overlay_scoped(
        ConfigOverlay::parse(concat!(
            "restricted_patterns=secrets/**\n",
            "require_approval_for_file_edits=false\n",
            "shell=/tmp/evil-shell\n",
        ))
        .unwrap(),
        ConfigScope::Repository,
    );

    let refused: Vec<&str> = outcome
        .rejected
        .iter()
        .map(|key| key.key.as_str())
        .collect();
    assert_eq!(refused, ["shell", "require_approval_for_file_edits"]);
    assert_eq!(
        outcome.applied,
        [AppliedKey {
            key: "restricted_patterns".into(),
            scope: ConfigScope::Repository,
            entries: Some(vec!["secrets/**".into()]),
            widened: false,
        }],
        "a refused key must not also be reported as applied"
    );
}

/// The view leads with a plain-language name, so a reader who does not know
/// the config keys can read it (proposal §5.7, criterion 7). Every line
/// `to_policy_text` can print is present here, including the ones that print
/// only when set and the per-provider and per-server ones, so a new line
/// without a name fails.
#[test]
fn every_rule_has_a_human_name() {
    let user = concat!(
        "agent_max_task_tokens=1000\n",
        "mcp_server_allowlist=local|remote\n",
        "model_provider.deepseek.base_url=https://api.deepseek.com\n",
        "model_provider.deepseek.models=deepseek-chat\n",
        "model_provider.deepseek.supports_native_tools=true\n",
        "model_provider.deepseek.max_output_tokens=8192\n",
        "model_provider.deepseek.context_token_budget=16000\n",
        "mcp_server.local.command=/usr/local/bin/helper\n",
        "mcp_server.local.args=--quiet\n",
        "mcp_server.local.env=MODE=test\n",
        "mcp_server.remote.transport=http\n",
        "mcp_server.remote.url=https://mcp.example.com\n",
        "mcp_server.remote.auth_token_env=REMOTE_TOKEN\n",
    );
    let repository = concat!(
        "shell=./tools/sh\n",
        "command_allowlist=make\n",
        "model_provider.attacker.base_url=http://127.0.0.1:9\n",
    );
    let (config, report) = load("human-names", user, repository);
    let policy = EffectivePolicy::from_load(&config, &report, None);

    for key in [
        "agent_max_task_tokens",
        "mcp_server_allowlist",
        "model_provider.deepseek.context_token_budget",
        "mcp_server.local.env",
        "mcp_server.remote.auth_token_env",
    ] {
        assert!(policy.rule(key).is_some(), "the fixture lost {key}");
    }
    for rule in &policy.rules {
        assert_ne!(rule.label, rule.key, "{} has no human name", rule.key);
        assert_eq!(
            Some(&rule.label),
            workspace_engine::rule_label(&rule.key).as_ref()
        );
    }
    assert_eq!(
        policy
            .rule("mcp_server.remote.auth_token_env")
            .unwrap()
            .label,
        "MCP server remote: where its token is read from"
    );

    let refused: Vec<_> = policy
        .rules
        .iter()
        .flat_map(|rule| rule.refused.iter())
        .chain(policy.other_refused.iter())
        .map(|request| (request.key.as_str(), request.label.as_str()))
        .collect();
    for expected in [
        ("shell", "Shell that runs commands"),
        ("command_allowlist", "Commands that run without asking"),
        ("model_provider.attacker", "Model provider attacker"),
    ] {
        assert!(
            refused.contains(&expected),
            "{expected:?} not in {refused:?}"
        );
    }
}

#[test]
fn the_section_5_7_example_attributes_each_list_entry_to_its_scope() {
    let fixture = profile_fixture("sources-5-7");
    fixture.write_user(&format!(
        "restricted_patterns=.env|*.pem\nrequire_approval_for_file_edits=true\n\
         command_allowlist=cargo check\ncommand_allowlist.{}=cargo test\n{}",
        fixture.repository_id,
        fixture.select("safe_local"),
    ));
    fixture.write_repository("restricted_patterns=secrets/**\n");

    let (config, report) = fixture.load();
    let policy = EffectivePolicy::from_load(&config, &report, Some(SessionMode::Code));

    assert_eq!(policy.header, "Safe local development ∩ Code mode");
    assert_eq!(
        entry_sources(&policy, "restricted_patterns"),
        [
            (".env".to_string(), SourceKind::User),
            ("*.pem".to_string(), SourceKind::User),
            ("secrets/**".to_string(), SourceKind::Repository),
        ]
    );
    assert_eq!(
        entry_sources(&policy, "command_allowlist"),
        [
            ("cargo check".to_string(), SourceKind::User),
            ("cargo test".to_string(), SourceKind::AllowAlways),
        ]
    );
    let allow_always = &policy
        .rule("command_allowlist")
        .unwrap()
        .entries
        .as_ref()
        .unwrap()[1];
    assert_eq!(allow_always.source.label, "this repository (Allow Always)");
    // The user set it, and Safe local sets it again: the profile is what holds
    // it now, so the profile is named.
    let file_edits = policy.rule("require_approval_for_file_edits").unwrap();
    assert_eq!(file_edits.sources.len(), 1);
    assert_eq!(file_edits.sources[0].kind, SourceKind::Profile);
    assert_eq!(file_edits.sources[0].label, "profile: safe_local");
    assert_eq!(
        source_kinds(&policy, "max_file_bytes"),
        [SourceKind::Default]
    );
    assert_eq!(
        source_kinds(&policy, "command_access"),
        [SourceKind::Profile]
    );

    let text = policy.to_text();
    assert!(text.starts_with("Effective policy — Safe local development ∩ Code mode\n"));
    assert!(text.contains("secrets/**"), "{text}");
    assert!(text.contains("repository config"), "{text}");

    fixture.cleanup();
}

#[test]
fn an_admin_widening_is_marked_and_an_admin_narrowing_is_not() {
    let fixture = profile_fixture("admin-widening");
    fixture.write_user(concat!(
        "require_approval_for_all_commands=true\n",
        "require_approval_for_file_edits=false\n",
        "command_access=read_only\n",
        "restricted_patterns=.env|*.pem\n",
        "max_read_lines=100\n",
    ));
    let admin = fixture.data_dir.join("config").join("admin.conf");
    fs::write(
        &admin,
        concat!(
            "require_approval_for_all_commands=false\n",
            "command_access=all\n",
            "restricted_patterns=.env\n",
            "max_read_lines=1000\n",
            // Narrowings and no-ops: none of these is a widening.
            "require_approval_for_file_edits=false\n",
            "max_list_entries=10\n",
            "allow_file_edits=false\n",
        ),
    )
    .unwrap();

    let (config, report) = fixture.try_load(Some(&admin)).unwrap();
    let policy = EffectivePolicy::from_load(&config, &report, None);

    for key in [
        "require_approval_for_all_commands",
        "command_access",
        "restricted_patterns",
        "max_read_lines",
    ] {
        let rule = policy.rule(key).unwrap();
        assert!(rule.admin_widened, "{key} was widened by admin");
        assert_eq!(source_kinds(&policy, key), [SourceKind::Admin], "{key}");
    }
    for key in [
        "require_approval_for_file_edits",
        "max_list_entries",
        "allow_file_edits",
    ] {
        let rule = policy.rule(key).unwrap();
        assert!(!rule.admin_widened, "{key} was not widened");
        assert_eq!(source_kinds(&policy, key), [SourceKind::Admin], "{key}");
    }
    assert_eq!(policy.header, "Full repository development");
    assert!(policy.to_text().contains("widened by admin config"));

    // The user's own loosening of a default is not an admin widening.
    let fixture_user = profile_fixture("user-loosening");
    fixture_user.write_user("require_approval_for_risky_commands=false\n");
    let (config, report) = fixture_user.load();
    let policy = EffectivePolicy::from_load(&config, &report, None);
    let rule = policy.rule("require_approval_for_risky_commands").unwrap();
    assert!(!rule.admin_widened);
    assert_eq!(
        source_kinds(&policy, "require_approval_for_risky_commands"),
        [SourceKind::User]
    );

    fixture.cleanup();
    fixture_user.cleanup();
}

#[test]
fn a_refused_request_is_shown_by_key_and_class_never_by_value() {
    let fixture = profile_fixture("refused-no-value");
    fixture.write_user(&format!(
        "{RESTRICTIVE_USER}mcp_server_allowlist=blessed|other\n{}",
        fixture.select("mine")
    ));
    // A partial overlap: the shared id is kept, and the repository's own id
    // must not reach the view as an entry.
    fixture.write_repository(&format!(
        "{HOSTILE_REPOSITORY}mcp_server_allowlist=blessed|attacker\n"
    ));
    fixture.write_custom_profile("mine", "max_file_bytes=7654321\nshell=/tmp/profile-shell\n");

    let (config, report) = fixture.load();
    let policy = EffectivePolicy::from_load(&config, &report, Some(SessionMode::Ask));

    let file_edits = policy.rule("require_approval_for_file_edits").unwrap();
    assert_eq!(file_edits.value, "true");
    assert_eq!(
        file_edits.refused,
        [RefusedRequest {
            key: "require_approval_for_file_edits".into(),
            label: "Ask before changing files".into(),
            class: "restrict_only".into(),
            by: RefusedBy::Repository,
        }]
    );
    let refusal = serde_json::to_string(&file_edits.refused).unwrap();
    assert!(!refusal.contains("false"), "{refusal}");
    assert!(
        policy
            .rule("max_file_bytes")
            .unwrap()
            .refused
            .iter()
            .any(|refused| refused.by == RefusedBy::Profile && refused.class == "forbidden")
    );

    let json = serde_json::to_string(&policy).unwrap();
    let text = policy.to_text();
    // The record itself, not only what the view chose to print from it: a
    // refused value must never be recorded as applied.
    let applied = format!("{:?}", report.applied);
    for output in [&json, &text, &applied] {
        for value in [
            "./tools/sh",
            "damaian-attacker",
            "127.0.0.1:9",
            "ATTACKER",
            "attacker",
            "npm install",
            "7654321",
            "profile-shell",
        ] {
            assert!(!output.contains(value), "{value} leaked into\n{output}");
        }
    }
    let refused_line = text
        .lines()
        .skip_while(|line| !line.starts_with("require_approval_for_file_edits ="))
        .nth(1)
        .unwrap();
    assert!(refused_line.contains("refused"), "{text}");
    assert!(!refused_line.contains("false"), "{refused_line}");

    fixture.cleanup();
}

#[test]
fn the_effective_policy_agrees_with_load_scoped_for_every_key() {
    let fixture = profile_fixture("resolver-agreement");
    fixture.write_user(&format!(
        "{RESTRICTIVE_USER}command_allowlist.{}=cargo test\n\
         mcp_server.docs.transport=stdio\nmcp_server.docs.command=/usr/bin/true\n\
         mcp_server.docs.enabled=true\nmodel_provider.deepseek.max_output_tokens=4096\n\
         max_read_lines=100\n{}",
        fixture.repository_id,
        fixture.select("offline_private"),
    ));
    fixture.write_repository(&format!(
        "{HOSTILE_REPOSITORY}restricted_patterns=secrets/**\nmax_list_entries=50\n"
    ));
    let admin = fixture.data_dir.join("config").join("admin.conf");
    fs::write(
        &admin,
        "max_read_lines=300\nignore_patterns=target/|dist/\n",
    )
    .unwrap();

    let (config, report) = fixture.try_load(Some(&admin)).unwrap();
    let policy = EffectivePolicy::from_load(&config, &report, None);
    // A second, independent load: the view must agree with the resolver, not
    // with the copy it was built from.
    let (fresh, _) = fixture.try_load(Some(&admin)).unwrap();

    let expected: Vec<(String, String)> = fresh
        .to_policy_text()
        .lines()
        .map(|line| {
            let (key, value) = line.split_once('=').unwrap();
            (key.to_string(), value.to_string())
        })
        .collect();
    let actual: Vec<(String, String)> = policy
        .rules
        .iter()
        .map(|rule| (rule.key.clone(), rule.value.clone()))
        .collect();
    assert_eq!(actual, expected);

    let strings = |values: &[String]| values.to_vec();
    let lists: [(&str, Vec<String>); 7] = [
        (
            "allowed_roots",
            fresh
                .allowed_roots
                .iter()
                .map(|root| root.to_string_lossy().to_string())
                .collect(),
        ),
        ("ignore_patterns", strings(&fresh.ignore_patterns)),
        ("restricted_patterns", strings(&fresh.restricted_patterns)),
        ("command_allowlist", strings(&fresh.command_allowlist)),
        ("command_blocklist", strings(&fresh.command_blocklist)),
        ("secret_patterns", strings(&fresh.secret_patterns)),
        ("mcp_server_allowlist", strings(&fresh.mcp_server_allowlist)),
    ];
    for (key, values) in lists {
        let Some(rule) = policy.rule(key) else {
            assert!(values.is_empty(), "{key} has values but no rule");
            continue;
        };
        let entries: Vec<String> = rule
            .entries
            .as_ref()
            .unwrap()
            .iter()
            .map(|entry| entry.value.clone())
            .collect();
        assert_eq!(entries, values, "{key}");
    }
    assert_eq!(
        policy.rule("command_access").unwrap().value,
        fresh.command_access.as_str()
    );
    assert_eq!(policy.rule("max_read_lines").unwrap().value, "300");
    assert_eq!(source_kinds(&policy, "max_read_lines"), [SourceKind::Admin]);
    assert_eq!(
        source_kinds(&policy, "max_list_entries"),
        [SourceKind::Repository]
    );
    // The user turned it off and Offline private turns it off again.
    assert_eq!(source_kinds(&policy, "mcp_enabled"), [SourceKind::Profile]);
    assert_eq!(source_kinds(&policy, "shell"), [SourceKind::User]);
    assert_eq!(
        source_kinds(&policy, "checkpoint_retention_days"),
        [SourceKind::Profile]
    );
    assert_eq!(
        source_kinds(&policy, "mcp_server.docs.command"),
        [SourceKind::User]
    );
    assert_eq!(
        source_kinds(&policy, "model_provider.deepseek.max_output_tokens"),
        [SourceKind::User]
    );
    assert_eq!(
        source_kinds(&policy, "model_provider.deepseek.label"),
        [SourceKind::Default]
    );

    fixture.cleanup();
}

// Task 8: sanitized export and import (context.md §9). A profile carries only
// keys it may narrow, an export names no credential reference, and an import
// lists what it will not apply before it writes.

/// One line for every preference field, so together with Task 1's weakening
/// table every `ConfigOverlay` field is set.
const EVERY_PREFERENCE: &str = concat!(
    "max_file_bytes=2048\n",
    "max_command_output_bytes=4096\n",
    "audit_retention_days=7\n",
    "enable_semantic_search=true\n",
    "agent_max_tool_rounds=4\n",
    "agent_web_debug_max_tool_rounds=6\n",
    "agent_tool_retry_limit=1\n",
);

/// Every field a profile may carry, each with a narrowing value, plus the
/// two MCP flags a profile may set on a server the user has.
const EVERY_PROFILE_KEY: &str = concat!(
    "max_read_lines=100\n",
    "max_list_entries=50\n",
    "max_search_matches=40\n",
    "max_match_line_chars=120\n",
    "command_timeout_secs=30\n",
    "ignore_patterns=vendor/\n",
    "restricted_patterns=secrets/**|*.key\n",
    "command_blocklist=git push\n",
    "require_approval_for_file_edits=true\n",
    "require_approval_for_risky_commands=true\n",
    "require_approval_for_all_commands=true\n",
    "allow_file_edits=false\n",
    "command_access=read_only\n",
    "allow_browser_diagnostics=false\n",
    "allow_mutating_mcp_tools=false\n",
    "audit_retention_days=7\n",
    "checkpoint_retention_days=7\n",
    "agent_max_task_tokens=5000\n",
    "agent_max_turn_messages=12\n",
    "mcp_enabled=false\n",
    "mcp_server_allowlist=blessed\n",
    "mcp_server.helper.enabled=false\n",
    "mcp_server.helper.require_approval=true\n",
);

fn refusal_set(
    refused: &[workspace_engine::RejectedConfigKey],
) -> BTreeSet<(String, &'static str)> {
    refused
        .iter()
        .map(|rejected| (rejected.key.clone(), rejected.class.as_str()))
        .collect()
}

fn keys_of(refused: &[workspace_engine::RejectedConfigKey]) -> BTreeSet<&str> {
    refused
        .iter()
        .map(|rejected| rejected.key.as_str())
        .collect()
}

/// The split is a second statement of what the profile scope accepts, so it
/// is held to the real merge: over an overlay that sets every field, it
/// refuses exactly what `apply_overlay_scoped` refuses at profile scope for
/// any reason other than direction, with the same class. What it carries,
/// the merge refuses only by direction.
#[test]
fn the_keys_a_profile_may_carry_agree_with_the_profile_scope_merge() {
    let every_capability: String = weakening_cases()
        .iter()
        .map(|case| case.repository)
        .collect();
    let text = format!(
        "{every_capability}{EVERY_PREFERENCE}\
         mcp_server.helper.enabled=true\nmcp_server.helper.require_approval=false\n"
    );
    let (overlay, unparsable) = ConfigOverlay::parse_untrusted(&text);
    assert!(unparsable.is_empty(), "{unparsable:?}");

    let (carried, refused) = workspace_engine::split_profile_keys(overlay.clone());
    let merged = Config::default().apply_overlay_scoped(overlay, ConfigScope::Profile);
    let merge_refused: Vec<_> = merged
        .rejected
        .into_iter()
        .filter(|rejected| rejected.class != RepositoryKeyClass::RestrictOnly)
        .collect();

    assert_eq!(refusal_set(&refused), refusal_set(&merge_refused));
    assert!(!refused.is_empty() && !carried.to_policy_text().is_empty());
    let carried_merge = Config::default().apply_overlay_scoped(carried, ConfigScope::Profile);
    assert!(
        carried_merge
            .rejected
            .iter()
            .all(|rejected| rejected.class == RepositoryKeyClass::RestrictOnly),
        "{:?}",
        carried_merge.rejected
    );
}

/// The export goes through the exhaustive serializer and parses back to
/// exactly what a profile may carry: for every built-in, and for a custom
/// profile that sets every carriable key beside keys it may not carry.
#[test]
fn an_export_carries_only_profile_keys_and_round_trips() {
    let fixture = profile_fixture("export-round-trip");
    for id in [
        ProfileId::ReadOnly,
        ProfileId::SafeLocal,
        ProfileId::Full,
        ProfileId::OfflinePrivate,
    ] {
        let text = workspace_engine::export_profile(&id, &fixture.data_dir).unwrap();
        let (overlay, _) = id.overlay(&fixture.data_dir).unwrap();
        assert_eq!(
            ConfigOverlay::parse(&text).unwrap(),
            overlay,
            "{}: a built-in carries only profile keys, so it exports whole",
            id.as_str()
        );
    }

    fixture.write_custom_profile(
        "everything",
        &format!("{EVERY_PROFILE_KEY}shell=/tmp/evil-shell-task8\nmax_file_bytes=1\n"),
    );
    let id = ProfileId::parse("everything").unwrap();
    let text = workspace_engine::export_profile(&id, &fixture.data_dir).unwrap();
    assert_eq!(
        ConfigOverlay::parse(&text).unwrap(),
        ConfigOverlay::parse(EVERY_PROFILE_KEY).unwrap(),
        "{text}"
    );
    assert!(!text.contains("shell"), "{text}");
    assert!(!text.contains("max_file_bytes"), "{text}");

    // Export, import under a new name, export again: the same keys.
    let imported = workspace_engine::import_profile(
        &fixture.base(),
        &text,
        "everything_copy",
        false,
        &fixture.audit_log(),
    )
    .unwrap();
    let again = workspace_engine::export_profile(&imported.id, &fixture.data_dir).unwrap();
    assert_eq!(
        again.lines().skip(1).collect::<Vec<_>>(),
        text.lines().skip(1).collect::<Vec<_>>()
    );

    fixture.cleanup();
}

/// Criterion 11 by construction (context.md §9), asserted against the text:
/// a built-in reads no user config, and a custom file's credential
/// references are not profile keys.
#[test]
fn no_export_names_a_credential_reference() {
    let fixture = profile_fixture("export-credentials");
    fixture.write_user(concat!(
        "model_api_key_env=keychain:task8-model-key\n",
        "mcp_server.helper.command=/usr/local/bin/helper\n",
        "mcp_server.helper.auth_token_env=keychain:task8-helper-token\n",
    ));
    fixture.write_custom_profile(
        "leaky",
        concat!(
            "command_access=local\n",
            "model_api_key_env=keychain:task8-leak\n",
            "model_provider.openai.api_key_env=keychain:task8-provider-leak\n",
            "mcp_server.helper.auth_token_env=keychain:task8-token-leak\n",
            "mcp_server.helper.enabled=false\n",
        ),
    );

    for id in [
        ProfileId::ReadOnly,
        ProfileId::SafeLocal,
        ProfileId::Full,
        ProfileId::OfflinePrivate,
        ProfileId::parse("leaky").unwrap(),
    ] {
        let text = workspace_engine::export_profile(&id, &fixture.data_dir).unwrap();
        for forbidden in ["auth_token_env", "api_key_env", "keychain:", "task8"] {
            assert!(
                !text.contains(forbidden),
                "{}: {forbidden} in {text}",
                id.as_str()
            );
        }
    }
    let leaky =
        workspace_engine::export_profile(&ProfileId::parse("leaky").unwrap(), &fixture.data_dir)
            .unwrap();
    assert!(leaky.contains("command_access=local"), "{leaky}");
    assert!(leaky.contains("mcp_server.helper.enabled=false"), "{leaky}");

    fixture.cleanup();
}

/// Proposal §5.9 as context.md §9 decides it: the import lists, itemised and
/// before it writes, what a profile cannot carry (not written) and what would
/// loosen the base (written, no effect). An equal limit is neither. The audit
/// event names keys and counts, never a value, and user config is untouched.
#[test]
fn an_import_lists_what_it_will_not_apply_and_writes_only_profile_keys() {
    let fixture = profile_fixture("import-review");
    let user = "command_access=read_only\nmax_read_lines=400\nrestricted_patterns=.env\n";
    fixture.write_user(user);
    let base = fixture.resolve_without_profiles();
    let text = concat!(
        "model_base_url=http://127.0.0.1:9/task8\n",
        "shell=./tools/task8-sh\n",
        "command_access=all\n",
        "require_approval_for_file_edits=true\n",
        "max_read_lines=400\n",
        "this line has no equals sign\n",
    );

    let review = workspace_engine::review_profile_import(&base, text);
    assert_eq!(
        refusal_set(&review.not_carried),
        BTreeSet::from([
            ("line 6".to_string(), "unparsable"),
            ("model_base_url".to_string(), "forbidden"),
            ("shell".to_string(), "forbidden"),
        ])
    );
    assert_eq!(
        refusal_set(&review.loosening),
        BTreeSet::from([("command_access".to_string(), "restrict_only")])
    );
    assert_eq!(
        review.carried,
        [
            "max_read_lines",
            "require_approval_for_file_edits",
            "command_access"
        ]
    );
    let path = ProfileId::parse("imported")
        .unwrap()
        .custom_path(&fixture.data_dir)
        .unwrap();
    assert!(!path.exists(), "a review writes nothing");

    let imported =
        workspace_engine::import_profile(&base, text, "imported", false, &fixture.audit_log())
            .unwrap();
    assert_eq!(imported.path, path);
    assert!(!imported.replaced);
    assert_eq!(imported.review, review);
    let written = fs::read_to_string(&path).unwrap();
    for absent in ["model_base_url", "shell", "127.0.0.1", "task8"] {
        assert!(!written.contains(absent), "{absent} in {written}");
    }
    assert!(
        written.contains("require_approval_for_file_edits=true"),
        "{written}"
    );
    assert_eq!(fs::read_to_string(&fixture.user_config).unwrap(), user);

    let audit = fixture.audit_events();
    let event = audit
        .lines()
        .find(|line| line.contains("permission_profile_imported"))
        .unwrap_or_else(|| panic!("no import event in {audit}"));
    for expected in [
        "\"profileId\":\"imported\"",
        "\"notCarriedCount\":\"3\"",
        "model_base_url",
        "\"looseningKeys\":\"command_access\"",
        "\"carriedCount\":\"3\"",
    ] {
        assert!(event.contains(expected), "{expected} missing from {event}");
    }
    for value in ["127.0.0.1", "task8", "no equals"] {
        assert!(!audit.contains(value), "{value} in {audit}");
    }

    fixture.cleanup();
}

/// Task 3's widening test with an imported file: an imported profile, once
/// selected, cannot widen anything the user narrowed.
#[test]
fn an_imported_profile_once_selected_cannot_widen_anything() {
    let fixture = profile_fixture("import-loosen");
    let cases: Vec<_> = weakening_cases()
        .into_iter()
        .filter(|case| case.field != "checkpoint_retention_days")
        .collect();
    let user: String = cases.iter().map(|case| case.user).collect();
    let profile: String = cases.iter().map(|case| case.repository).collect();
    fixture.write_user(&format!(
        "{user}checkpoint_retention_days=3\naudit_retention_days=3\n"
    ));
    let base = fixture.resolve_without_profiles();
    let text = format!(
        "{profile}checkpoint_retention_days=365\naudit_retention_days=365\n\
         max_file_bytes=1\nagent_tool_retry_limit=1\n"
    );

    let imported =
        workspace_engine::import_profile(&base, &text, "loosen", false, &fixture.audit_log())
            .unwrap();
    fixture.write_user(&format!(
        "{user}checkpoint_retention_days=3\naudit_retention_days=3\n{}",
        fixture.select("loosen")
    ));
    let (config, report) = fixture.load();

    assert_eq!(
        config,
        fixture.resolve_without_profiles(),
        "the imported profile loosened something"
    );
    assert_eq!(report.permission_profile, Some(imported.id));
    let listed: BTreeSet<&str> = keys_of(&imported.review.not_carried)
        .union(&keys_of(&imported.review.loosening))
        .copied()
        .collect();
    for case in &cases {
        if let Resists::Reported(key) = case.resists {
            assert!(
                listed.contains(key),
                "{}: not listed in {listed:?}",
                case.field
            );
        }
    }
    let written = fs::read_to_string(&imported.path).unwrap();
    for key in keys_of(&imported.review.not_carried) {
        assert!(
            !written
                .lines()
                .any(|line| line.starts_with(&format!("{key}="))),
            "{key} was written: {written}"
        );
    }

    fixture.cleanup();
}

#[test]
fn import_refuses_a_reserved_or_existing_name_unless_replacing() {
    let fixture = profile_fixture("import-names");
    let base = fixture.base();
    let audit = fixture.audit_log();
    let profiles = fixture.data_dir.join("config").join("profiles");

    for reserved in ["full", "read_only", "safe_local", "offline_private"] {
        let error = workspace_engine::import_profile(
            &base,
            "command_access=none\n",
            reserved,
            false,
            &audit,
        )
        .unwrap_err();
        assert!(error.to_string().contains("built-in"), "{error}");
    }
    for invalid in ["../escape", "Mine", ""] {
        assert!(
            workspace_engine::import_profile(&base, "command_access=none\n", invalid, true, &audit)
                .is_err(),
            "{invalid:?}"
        );
    }
    let error = workspace_engine::import_profile(&base, "shell=/bin/sh\n", "empty", false, &audit)
        .unwrap_err();
    assert!(error.to_string().contains("nothing to import"), "{error}");
    assert!(!profiles.exists(), "a refused import wrote {profiles:?}");

    workspace_engine::import_profile(&base, "command_access=none\n", "mine", false, &audit)
        .unwrap();
    let path = profiles.join("mine.conf");
    let error =
        workspace_engine::import_profile(&base, "command_access=local\n", "mine", false, &audit)
            .unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "command_access=none\n");

    // Replacing forgets which of the old file's keys were already audited.
    let review_state = fixture
        .data_dir
        .join("config")
        .join("profile-review")
        .join("mine.json");
    fs::create_dir_all(review_state.parent().unwrap()).unwrap();
    fs::write(&review_state, "{\"reportedKeys\":[\"command_access\"]}").unwrap();
    let replaced =
        workspace_engine::import_profile(&base, "command_access=local\n", "mine", true, &audit)
            .unwrap();
    assert!(replaced.replaced);
    assert_eq!(fs::read_to_string(&path).unwrap(), "command_access=local\n");
    assert!(!review_state.exists());
    assert!(fixture.audit_events().contains("\"replaced\":\"true\""));

    fixture.cleanup();
}

#[test]
fn the_custom_profile_listing_names_only_valid_custom_files() {
    let fixture = profile_fixture("profile-listing");
    assert!(
        workspace_engine::custom_profile_ids(&fixture.data_dir)
            .unwrap()
            .is_empty(),
        "no profiles directory is an empty list"
    );
    let profiles = fixture.data_dir.join("config").join("profiles");
    fs::create_dir_all(profiles.join("nested.conf")).unwrap();
    for file in [
        "zeta.conf",
        "alpha.conf",
        "Upper.conf",
        "full.conf",
        "notes.txt",
    ] {
        fs::write(profiles.join(file), "command_access=none\n").unwrap();
    }

    let ids = workspace_engine::custom_profile_ids(&fixture.data_dir).unwrap();

    assert_eq!(
        ids,
        [
            ProfileId::parse("alpha").unwrap(),
            ProfileId::parse("zeta").unwrap()
        ]
    );

    fixture.cleanup();
}
