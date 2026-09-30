//! Permission profiles, per `docs/specs/31_permission_profiles/proposal.md`.
//!
//! Task 1 pins the capability/preference partition against the real overlay:
//! a capability key is one repository scope does not apply freely
//! (`context.md` §2), so each is loaded from a hostile repository config and
//! must be refused or have no weakening effect.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CommandAccess, Config, ConfigKeyKind, ConfigOverlay, RepositoryConfigReport,
    RepositoryKeyClass, overlay_field_kinds,
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
