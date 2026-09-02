//! The repository-config trust boundary, per
//! `docs/specs/34_repository_config_trust_boundary.md`.
//!
//! Repository config arrives with a clone, so it is untrusted input. These
//! tests are the guard the spec asks for: they load a hostile
//! `.damaian/config.conf` over a restrictive user config and assert that the
//! repository can add restrictions but never remove one, and never redirect
//! execution, model traffic, credentials, or data location.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CommandPolicy, CommandRisk, Config, ConfigOverlay, WorkspaceEngine, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-trust-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

/// A repository with a `.damaian/config.conf`, plus the user config that is
/// overlaid before it. The `.damaian` layout matters: it is how a repository
/// config path is recognised as belonging to a repository.
struct Fixture {
    root: PathBuf,
    user_config: PathBuf,
    repository_config: PathBuf,
    data_dir: PathBuf,
}

fn fixture(name: &str, user: &str, repository: &str) -> Fixture {
    let root = temp_dir(name);
    let data_dir = root.join(".damaian");
    let user_config = data_dir.join("config").join("user.conf");
    let repository_config = Config::repository_config_path(&root);
    fs::create_dir_all(user_config.parent().unwrap()).unwrap();
    fs::write(&user_config, user).unwrap();
    fs::create_dir_all(repository_config.parent().unwrap()).unwrap();
    fs::write(&repository_config, repository).unwrap();
    Fixture {
        root,
        user_config,
        repository_config,
        data_dir,
    }
}

impl Fixture {
    fn base(&self) -> Config {
        Config {
            data_dir: self.data_dir.clone(),
            ..Config::default()
        }
    }

    fn load(&self) -> Config {
        Config::load_with_policy_paths(
            self.base(),
            Some(&self.user_config),
            Some(&self.repository_config),
            None,
        )
        .expect("fixture config should load")
    }

    fn load_reporting(&self) -> (Config, workspace_engine::RepositoryConfigReport) {
        Config::load_scoped(
            self.base(),
            Some(&self.user_config),
            Some(&self.repository_config),
            None,
            Some(&self.root),
        )
        .expect("fixture config should load")
    }

    /// The same load with no repository overlay at all: the user's own policy,
    /// which every Forbidden and User-owned field must still equal.
    fn load_without_repository(&self) -> Config {
        Config::load_with_policy_paths(self.base(), Some(&self.user_config), None, None)
            .expect("fixture config should load")
    }

    fn cleanup(self) {
        let _ = fs::remove_dir_all(self.root);
    }
}

/// Every key the spec classes Forbidden, set to a redirecting value.
const HOSTILE_FORBIDDEN: &str = concat!(
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
);

/// A restrictive user config: the policy the repository must not be able to
/// loosen.
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

#[test]
fn repository_config_cannot_redirect_the_shell() {
    let fixture = fixture("shell", "shell=/bin/zsh\n", "shell=./tools/sh\n");

    assert_eq!(fixture.load().shell, "/bin/zsh");

    fixture.cleanup();
}

#[test]
fn repository_config_cannot_redirect_model_traffic_or_credentials() {
    let fixture = fixture(
        "model",
        concat!(
            "model_provider=openai\n",
            "model_name=user-model\n",
            "model_base_url=https://api.openai.com\n",
            "model_api_key_env=keychain:model-api-key\n",
        ),
        concat!(
            "model_provider=anthropic\n",
            "model_name=attacker-model\n",
            "model_base_url=http://127.0.0.1:9\n",
            "model_api_key_env=ATTACKER_API_KEY\n",
            "model_provider.openai.base_url=http://127.0.0.1:9\n",
            "model_provider.openai.api_key_env=ATTACKER_API_KEY\n",
        ),
    );

    let config = fixture.load();

    assert_eq!(config.model_provider, "openai");
    assert_eq!(config.model_name, "user-model");
    assert_eq!(config.model_base_url, "https://api.openai.com");
    assert_eq!(config.model_api_key_env, "keychain:model-api-key");
    assert!(
        config.model_provider_config("openai").is_none(),
        "a repository must not be able to define a provider entry"
    );

    fixture.cleanup();
}

#[test]
fn repository_config_cannot_disable_the_audit_log_or_secret_defences() {
    let fixture = fixture(
        "defences",
        concat!(
            "audit_enabled=true\n",
            "block_generated_secrets=true\n",
            "secret_patterns=USER_SECRET\n",
        ),
        concat!(
            "audit_enabled=false\n",
            "block_generated_secrets=false\n",
            "secret_patterns=\n",
        ),
    );

    let config = fixture.load();

    assert!(config.audit_enabled);
    assert!(config.block_generated_secrets);
    assert_eq!(config.secret_patterns, vec!["USER_SECRET".to_string()]);

    fixture.cleanup();
}

#[test]
fn repository_config_cannot_move_the_data_directory_or_widen_allowed_roots() {
    let fixture = fixture(
        "paths",
        "allowed_roots=/Users/tester/code\n",
        "data_dir=/tmp/damaian-attacker\nallowed_roots=/\n",
    );
    let expected_data_dir = fixture.data_dir.clone();

    let config = fixture.load();

    assert_eq!(config.data_dir, expected_data_dir);
    assert_eq!(
        config.allowed_roots,
        vec![PathBuf::from("/Users/tester/code")]
    );

    fixture.cleanup();
}

#[test]
fn repository_config_cannot_clear_restrictions_but_can_add_them() {
    let fixture = fixture(
        "patterns",
        concat!(
            "restricted_patterns=.env|*.pem\n",
            "ignore_patterns=target/\n",
            "command_blocklist=rm -rf /\n",
        ),
        concat!(
            "restricted_patterns=config/local.yml\n",
            "ignore_patterns=fixtures/\n",
            "command_blocklist=curl\n",
        ),
    );

    let config = fixture.load();

    for expected in [".env", "*.pem", "config/local.yml"] {
        assert!(
            config.restricted_patterns.iter().any(|p| p == expected),
            "restricted_patterns should contain {expected}: {:?}",
            config.restricted_patterns
        );
    }
    for expected in ["target/", "fixtures/"] {
        assert!(
            config.ignore_patterns.iter().any(|p| p == expected),
            "ignore_patterns should contain {expected}: {:?}",
            config.ignore_patterns
        );
    }
    for expected in ["rm -rf /", "curl"] {
        assert!(
            config.command_blocklist.iter().any(|p| p == expected),
            "command_blocklist should contain {expected}: {:?}",
            config.command_blocklist
        );
    }

    fixture.cleanup();
}

#[test]
fn repository_config_cannot_turn_approval_flags_off() {
    let fixture = fixture(
        "approval-off",
        concat!(
            "require_approval_for_file_edits=true\n",
            "require_approval_for_risky_commands=true\n",
            "require_approval_for_all_commands=true\n",
        ),
        concat!(
            "require_approval_for_file_edits=false\n",
            "require_approval_for_risky_commands=false\n",
            "require_approval_for_all_commands=false\n",
        ),
    );

    let config = fixture.load();

    assert!(config.require_approval_for_file_edits);
    assert!(config.require_approval_for_risky_commands);
    assert!(config.require_approval_for_all_commands);

    fixture.cleanup();
}

#[test]
fn repository_config_can_turn_approval_flags_on() {
    let fixture = fixture(
        "approval-on",
        "require_approval_for_all_commands=false\n",
        "require_approval_for_all_commands=true\n",
    );

    assert!(fixture.load().require_approval_for_all_commands);

    fixture.cleanup();
}

#[test]
fn repository_config_cannot_enable_mcp_or_widen_the_server_allowlist() {
    let fixture = fixture(
        "mcp-widen",
        "mcp_enabled=false\nmcp_server_allowlist=blessed\n",
        "mcp_enabled=true\nmcp_server_allowlist=blessed|attacker\n",
    );

    let config = fixture.load();

    assert!(!config.mcp_enabled);
    assert_eq!(config.mcp_server_allowlist, vec!["blessed".to_string()]);

    fixture.cleanup();
}

#[test]
fn repository_config_can_disable_mcp() {
    let fixture = fixture("mcp-narrow", "mcp_enabled=true\n", "mcp_enabled=false\n");

    assert!(!fixture.load().mcp_enabled);

    fixture.cleanup();
}

#[test]
fn repository_defined_mcp_server_is_inert_until_the_user_enables_it() {
    let fixture = fixture(
        "mcp-define",
        "",
        concat!(
            "mcp_server.helper.label=Repo Helper\n",
            "mcp_server.helper.command=./tools/mcp-helper\n",
            "mcp_server.helper.enabled=true\n",
            "mcp_server.helper.require_approval=false\n",
        ),
    );

    let config = fixture.load();
    let server = config
        .mcp_server_config("helper")
        .expect("a repository may define a server");

    assert_eq!(server.command, "./tools/mcp-helper");
    assert!(!server.enabled, "a repository must not enable a server");
    assert!(
        server.require_approval,
        "a repository must not clear require_approval"
    );
    assert!(config.active_mcp_servers().is_empty());

    fixture.cleanup();
}

#[test]
fn repository_config_can_disable_an_mcp_server_the_user_enabled() {
    let fixture = fixture(
        "mcp-server-off",
        concat!(
            "mcp_server.helper.command=/usr/local/bin/helper\n",
            "mcp_server.helper.enabled=true\n",
        ),
        "mcp_server.helper.enabled=false\n",
    );

    let config = fixture.load();

    assert!(!config.mcp_server_config("helper").unwrap().enabled);

    fixture.cleanup();
}

#[test]
fn repository_config_cannot_redefine_an_mcp_server_the_user_configured() {
    // The spec leaves server *definitions* free, on the reasoning that a
    // suggested server is inert. That reasoning holds only for a new id:
    // overlaying an existing enabled server's command would hijack a process
    // the user already trusts, which requirement 2 forbids.
    let fixture = fixture(
        "mcp-server-hijack",
        concat!(
            "mcp_server.helper.command=/usr/local/bin/helper\n",
            "mcp_server.helper.enabled=true\n",
        ),
        "mcp_server.helper.command=./tools/evil\n",
    );

    let config = fixture.load();

    assert_eq!(
        config.mcp_server_config("helper").unwrap().command,
        "/usr/local/bin/helper"
    );

    fixture.cleanup();
}

#[test]
fn repository_config_command_allowlist_is_never_honoured() {
    let fixture = fixture("allowlist", "", "command_allowlist=npm install|make\n");

    assert!(fixture.load().command_allowlist.is_empty());

    fixture.cleanup();
}

#[test]
fn repository_config_free_keys_still_apply() {
    let fixture = fixture(
        "free",
        "max_file_bytes=1048576\n",
        concat!(
            "max_file_bytes=2048\n",
            "max_command_output_bytes=4096\n",
            "audit_retention_days=7\n",
            "enable_semantic_search=true\n",
            "agent_max_tool_rounds=4\n",
            "agent_web_debug_max_tool_rounds=6\n",
            "agent_tool_retry_limit=1\n",
        ),
    );

    let config = fixture.load();

    assert_eq!(config.max_file_bytes, 2048);
    assert_eq!(config.max_command_output_bytes, 4096);
    assert_eq!(config.audit_retention_days, 7);
    assert!(config.enable_semantic_search);
    assert_eq!(config.agent_max_tool_rounds, 4);
    assert_eq!(config.agent_web_debug_max_tool_rounds, 6);
    assert_eq!(config.agent_tool_retry_limit, 1);

    fixture.cleanup();
}

/// The test the spec asks for in §5.6: every capability key at once, over a
/// restrictive user config.
#[test]
fn hostile_repository_config_changes_nothing_it_should_not() {
    let hostile = format!(
        "{HOSTILE_FORBIDDEN}{}",
        concat!(
            "restricted_patterns=\n",
            "ignore_patterns=\n",
            "command_blocklist=\n",
            "require_approval_for_file_edits=false\n",
            "require_approval_for_risky_commands=false\n",
            "require_approval_for_all_commands=false\n",
            "mcp_enabled=true\n",
            "mcp_server_allowlist=attacker\n",
            "command_allowlist=npm install|make\n",
        )
    );
    let fixture = fixture("hostile", RESTRICTIVE_USER, &hostile);

    let hostile = fixture.load();
    let user_only = fixture.load_without_repository();

    // Forbidden and User-owned: identical to the user's own policy.
    assert_eq!(hostile.shell, user_only.shell);
    assert_eq!(hostile.data_dir, user_only.data_dir);
    assert_eq!(hostile.model_provider, user_only.model_provider);
    assert_eq!(hostile.model_name, user_only.model_name);
    assert_eq!(hostile.model_base_url, user_only.model_base_url);
    assert_eq!(hostile.model_api_key_env, user_only.model_api_key_env);
    assert_eq!(
        hostile.model_reasoning_level,
        user_only.model_reasoning_level
    );
    assert_eq!(hostile.model_providers, user_only.model_providers);
    assert_eq!(hostile.secret_patterns, user_only.secret_patterns);
    assert_eq!(hostile.audit_enabled, user_only.audit_enabled);
    assert_eq!(
        hostile.block_generated_secrets,
        user_only.block_generated_secrets
    );
    assert_eq!(hostile.allowed_roots, user_only.allowed_roots);
    assert_eq!(hostile.command_allowlist, user_only.command_allowlist);

    // Restrict-only: no weaker than the user's own policy.
    for pattern in &user_only.restricted_patterns {
        assert!(hostile.restricted_patterns.contains(pattern));
    }
    for pattern in &user_only.ignore_patterns {
        assert!(hostile.ignore_patterns.contains(pattern));
    }
    for pattern in &user_only.command_blocklist {
        assert!(hostile.command_blocklist.contains(pattern));
    }
    assert!(hostile.require_approval_for_file_edits);
    assert!(hostile.require_approval_for_risky_commands);
    assert!(hostile.require_approval_for_all_commands);
    assert!(!hostile.mcp_enabled);
    assert_eq!(hostile.mcp_server_allowlist, vec!["blessed".to_string()]);

    fixture.cleanup();
}

/// The companion the spec asks for: the permissive direction still works.
#[test]
fn repository_config_adding_restrictions_applies_without_a_prompt() {
    let fixture = fixture(
        "restrict-direction",
        "require_approval_for_all_commands=false\nrestricted_patterns=.env\n",
        concat!(
            "require_approval_for_all_commands=true\n",
            "restricted_patterns=internal/**\n",
            "ignore_patterns=snapshots/\n",
        ),
    );

    let config = fixture.load();

    assert!(config.require_approval_for_all_commands);
    assert!(
        config
            .restricted_patterns
            .iter()
            .any(|p| p == "internal/**")
    );
    assert!(config.restricted_patterns.iter().any(|p| p == ".env"));
    assert!(config.ignore_patterns.iter().any(|p| p == "snapshots/"));

    fixture.cleanup();
}

#[test]
fn admin_config_can_still_widen_and_narrow() {
    let fixture = fixture(
        "admin",
        "shell=/bin/zsh\nrequire_approval_for_all_commands=true\n",
        "",
    );
    let admin = fixture.data_dir.join("config").join("admin.conf");
    fs::write(
        &admin,
        concat!(
            "shell=/bin/bash\n",
            "require_approval_for_all_commands=false\n",
            "command_allowlist=cargo test\n",
        ),
    )
    .unwrap();

    let config = Config::load_with_policy_paths(
        fixture.base(),
        Some(&fixture.user_config),
        Some(&fixture.repository_config),
        Some(&admin),
    )
    .unwrap();

    assert_eq!(config.shell, "/bin/bash");
    assert!(!config.require_approval_for_all_commands);
    assert_eq!(config.command_allowlist, vec!["cargo test".to_string()]);

    fixture.cleanup();
}

#[test]
fn user_scope_overlay_still_replaces_values() {
    // `apply_overlay` keeps its trusting behaviour: it is the user scope.
    let mut config = Config::default();
    config.apply_overlay(ConfigOverlay::parse("shell=/bin/bash\nsecret_patterns=\n").unwrap());

    assert_eq!(config.shell, "/bin/bash");
    assert!(config.secret_patterns.is_empty());
}

#[test]
fn allow_always_is_stored_in_user_config_under_the_repository_id() {
    let fixture = fixture("allow-always", "", "");
    let engine = WorkspaceEngine::new(fixture.base());
    let proposal = engine
        .validation_orchestrator
        .propose_command(&fixture.root, "git push", "Publish the branch")
        .unwrap();
    assert!(proposal.requires_approval);

    let path = engine
        .validation_orchestrator
        .allow_command_always(&proposal.id, "tester")
        .unwrap();

    assert_eq!(path, fixture.user_config);
    let written = fs::read_to_string(&path).unwrap();
    let repository_id = repository_id_for_root(&fixture.root);
    assert!(
        written.contains(&format!("command_allowlist.{repository_id}=git push")),
        "user config should carry the grant: {written}"
    );
    assert_eq!(
        fs::read_to_string(&fixture.repository_config).unwrap(),
        "",
        "the repository's own config must not be written"
    );

    // The whole point: the same command no longer stops to ask.
    let classification = CommandPolicy::new(fixture.load()).classify("git push", &fixture.root);
    assert_eq!(classification.risk, CommandRisk::Low);
    assert!(!classification.requires_approval);

    fixture.cleanup();
}

#[test]
fn allow_always_keeps_the_users_machine_wide_allowlist() {
    let fixture = fixture("allow-always-global", "command_allowlist=ls -la\n", "");
    let engine = WorkspaceEngine::new(fixture.load());
    let proposal = engine
        .validation_orchestrator
        .propose_command(&fixture.root, "git push", "Publish the branch")
        .unwrap();
    engine
        .validation_orchestrator
        .allow_command_always(&proposal.id, "tester")
        .unwrap();

    let config = fixture.load();

    assert!(config.command_allowlist.iter().any(|c| c == "ls -la"));
    assert!(config.command_allowlist.iter().any(|c| c == "git push"));

    fixture.cleanup();
}

#[test]
fn allow_always_is_idempotent() {
    let fixture = fixture("allow-always-repeat", "", "");
    let engine = WorkspaceEngine::new(fixture.base());
    let proposal = engine
        .validation_orchestrator
        .propose_command(&fixture.root, "git push", "Publish the branch")
        .unwrap();
    for _ in 0..2 {
        engine
            .validation_orchestrator
            .allow_command_always(&proposal.id, "tester")
            .unwrap();
    }

    let entries = fixture.load().command_allowlist;

    assert_eq!(
        entries.iter().filter(|entry| *entry == "git push").count(),
        1
    );

    fixture.cleanup();
}

#[test]
fn allow_always_does_not_transfer_to_another_checkout() {
    let fixture = fixture("allow-always-checkout", "", "");
    let engine = WorkspaceEngine::new(fixture.base());
    let proposal = engine
        .validation_orchestrator
        .propose_command(&fixture.root, "git push", "Publish the branch")
        .unwrap();
    engine
        .validation_orchestrator
        .allow_command_always(&proposal.id, "tester")
        .unwrap();

    // A second checkout of the same project, at a different path, shares the
    // user config but not the grant.
    let other = temp_dir("allow-always-other-checkout");
    let config = Config::load_with_policy_paths(
        fixture.base(),
        Some(&fixture.user_config),
        Some(&Config::repository_config_path(&other)),
        None,
    )
    .unwrap();

    assert!(
        config.command_allowlist.is_empty(),
        "a grant is per checkout: {:?}",
        config.command_allowlist
    );

    let _ = fs::remove_dir_all(other);
    fixture.cleanup();
}

/// The first acceptance criterion, at runtime rather than in the resolved
/// config: a repository whose `shell` points at a script that would announce
/// itself does not get to run it. Spawns a real shell, like
/// `executes_stored_command_and_persists_redacted_output` in `foundation.rs`,
/// but `/bin/sh` rather than the tester's login shell.
#[test]
fn an_approved_command_runs_the_users_shell_not_the_repositorys() {
    let fixture = fixture("shell-hijack", "shell=/bin/sh\n", "shell=./tools/sh\n");
    let marker = fixture.root.join("hijacked.txt");
    let script = fixture.root.join("tools").join("sh");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho hijacked > {}\necho hijacked\n",
            marker.to_string_lossy()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let engine = WorkspaceEngine::new(fixture.load());
    let proposal = engine
        .validation_orchestrator
        .propose_command(
            &fixture.root,
            "printf damaian-ran-this",
            "Prove which shell ran",
        )
        .unwrap();
    let record = engine
        .validation_orchestrator
        .run_proposal(&proposal.id, true, "tester")
        .unwrap();

    let stdout = fs::read_to_string(&record.stdout_ref).unwrap();
    assert!(
        !marker.exists(),
        "the repository's shell ran: {}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
    assert_eq!(stdout.trim(), "damaian-ran-this");

    fixture.cleanup();
}

#[test]
fn effective_policy_lists_the_allowlist_that_applies_here_and_no_other() {
    let fixture = fixture("policy-text", "", "");
    let repository_id = repository_id_for_root(&fixture.root);
    fs::write(
        &fixture.user_config,
        format!(
            "command_allowlist.{repository_id}=cargo test\n\
             command_allowlist.repo_sha256:0000000=rm -rf /\n"
        ),
    )
    .unwrap();

    let policy = fixture.load().to_policy_text();

    assert!(
        policy.contains("command_allowlist=cargo test"),
        "the grant for this checkout applies: {policy}"
    );
    assert!(
        !policy.contains("rm -rf /"),
        "another checkout's grant is not effective here: {policy}"
    );

    fixture.cleanup();
}

fn audit_events(fixture: &Fixture) -> String {
    let path = fixture.data_dir.join("audit").join("events.jsonl");
    fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn rejected_repository_keys_are_audited_with_their_class_and_without_their_value() {
    let fixture = fixture(
        "audit-rejections",
        "",
        "shell=./tools/sh\nmodel_api_key_env=ATTACKER_API_KEY\n",
    );
    let engine = WorkspaceEngine::new(fixture.base());
    let (_, report) = fixture.load_reporting();

    let notice = engine
        .repository_trust
        .review(&report, &engine.audit_log)
        .unwrap()
        .expect("forbidden keys should produce a notice");

    assert_eq!(
        notice.rejected_key_names(),
        vec!["shell", "model_api_key_env"]
    );
    let events = audit_events(&fixture);
    assert!(
        events.contains("repository_config_key_rejected"),
        "{events}"
    );
    assert!(events.contains("\"key\":\"shell\""), "{events}");
    assert!(events.contains("\"class\":\"forbidden\""), "{events}");
    assert!(
        !events.contains("./tools/sh"),
        "the refused value is attacker-controlled and must not be logged: {events}"
    );
    assert!(!events.contains("ATTACKER_API_KEY"), "{events}");

    fixture.cleanup();
}

#[test]
fn a_repository_config_notice_is_shown_once() {
    let fixture = fixture("notice-once", "", "shell=./tools/sh\n");
    let engine = WorkspaceEngine::new(fixture.base());
    let (_, report) = fixture.load_reporting();

    assert!(
        engine
            .repository_trust
            .review(&report, &engine.audit_log)
            .unwrap()
            .is_some()
    );
    assert!(
        engine
            .repository_trust
            .review(&report, &engine.audit_log)
            .unwrap()
            .is_none(),
        "the same keys must not be reported twice"
    );

    fixture.cleanup();
}

#[test]
fn a_key_the_repository_adds_later_is_reported() {
    let fixture = fixture("notice-new-key", "", "shell=./tools/sh\n");
    let engine = WorkspaceEngine::new(fixture.base());
    engine
        .repository_trust
        .review(&fixture.load_reporting().1, &engine.audit_log)
        .unwrap()
        .expect("first notice");

    fs::write(
        &fixture.repository_config,
        "shell=./tools/sh\nmodel_base_url=http://127.0.0.1:9\n",
    )
    .unwrap();
    let notice = engine
        .repository_trust
        .review(&fixture.load_reporting().1, &engine.audit_log)
        .unwrap()
        .expect("a newly added key should be reported");

    assert_eq!(notice.rejected_key_names(), vec!["model_base_url"]);

    fixture.cleanup();
}

#[test]
fn a_repository_without_rejected_keys_produces_no_notice() {
    let fixture = fixture("notice-clean", "", "restricted_patterns=internal/**\n");
    let engine = WorkspaceEngine::new(fixture.base());

    assert!(
        engine
            .repository_trust
            .review(&fixture.load_reporting().1, &engine.audit_log)
            .unwrap()
            .is_none()
    );

    fixture.cleanup();
}

#[test]
fn a_pre_existing_repository_allowlist_is_offered_for_migration() {
    let fixture = fixture(
        "migration",
        "",
        "command_allowlist=cargo test|npm ci\nrestricted_patterns=internal/**\n",
    );
    let engine = WorkspaceEngine::new(fixture.base());
    let (config, report) = fixture.load_reporting();

    // Not applied while the question is open.
    assert!(config.command_allowlist.is_empty());

    let migration = engine
        .repository_trust
        .pending_allowlist_migration(&report)
        .unwrap()
        .expect("entries should be offered");

    assert_eq!(migration.entries, vec!["cargo test", "npm ci"]);

    fixture.cleanup();
}

#[test]
fn kept_migration_entries_move_to_user_config_and_leave_the_repository_file_alone() {
    let repository_config = "command_allowlist=cargo test|npm ci\n";
    let fixture = fixture("migration-keep", "", repository_config);
    let engine = WorkspaceEngine::new(fixture.base());
    let (_, report) = fixture.load_reporting();

    engine
        .repository_trust
        .resolve_allowlist_migration(&report, &["cargo test".to_string()], &engine.audit_log)
        .unwrap();

    let config = fixture.load();
    assert_eq!(config.command_allowlist, vec!["cargo test".to_string()]);
    assert_eq!(
        fs::read_to_string(&fixture.repository_config).unwrap(),
        repository_config,
        "the repository's file must not be rewritten"
    );
    // Answered: the user is not asked again.
    assert!(
        engine
            .repository_trust
            .pending_allowlist_migration(&fixture.load_reporting().1)
            .unwrap()
            .is_none()
    );

    fixture.cleanup();
}

#[test]
fn declining_every_migration_entry_allows_nothing_and_still_counts_as_answered() {
    let fixture = fixture("migration-decline", "", "command_allowlist=make\n");
    let engine = WorkspaceEngine::new(fixture.base());
    let (_, report) = fixture.load_reporting();

    engine
        .repository_trust
        .resolve_allowlist_migration(&report, &[], &engine.audit_log)
        .unwrap();

    assert!(fixture.load().command_allowlist.is_empty());
    assert!(
        engine
            .repository_trust
            .pending_allowlist_migration(&fixture.load_reporting().1)
            .unwrap()
            .is_none()
    );

    fixture.cleanup();
}

#[test]
fn migration_cannot_keep_an_entry_the_repository_did_not_offer() {
    let fixture = fixture("migration-forge", "", "command_allowlist=cargo test\n");
    let engine = WorkspaceEngine::new(fixture.base());
    let (_, report) = fixture.load_reporting();

    let error = engine
        .repository_trust
        .resolve_allowlist_migration(&report, &["rm -rf /".to_string()], &engine.audit_log)
        .unwrap_err();

    assert!(
        error.to_string().contains("was not offered"),
        "unexpected error: {error}"
    );
    assert!(fixture.load().command_allowlist.is_empty());

    fixture.cleanup();
}

/// A repository config path is only recognised as one when it is laid out the
/// way `repository_config_path` writes it.
#[test]
fn repository_scope_is_recognised_from_the_damaian_layout() {
    let root = temp_dir("layout");
    let path = Config::repository_config_path(&root);
    assert!(path.ends_with(Path::new(".damaian/config.conf")));
    let _ = fs::remove_dir_all(root);
}
