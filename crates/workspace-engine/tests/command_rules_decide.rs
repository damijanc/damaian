//! Spec 111 Task 4 (context.md D7): the command rules decide, and the card
//! and the audit name what decided. Each test stores a proposal the way a
//! turn does (`ValidationOrchestrator::propose_command`), so the decision,
//! the card's text and the audit record are the ones a user would see.
//! Nothing here runs a command.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::command_policy::ApprovalCause;
use workspace_engine::{CommandProposal, Config, RepositoryConfigReport, WorkspaceEngine};

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// A repository with user and repository config, and an audit log.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    data_dir: PathBuf,
}

impl Fixture {
    fn new(name: &str, user: &str, repository: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "damaian-rules-decide-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let root = base.join("project");
        let data_dir = base.join("data");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(data_dir.join("config")).unwrap();
        fs::write(data_dir.join("config/user.conf"), user).unwrap();
        let repository_config = Config::repository_config_path(&root);
        fs::create_dir_all(repository_config.parent().unwrap()).unwrap();
        fs::write(&repository_config, repository).unwrap();
        Self {
            base,
            root,
            data_dir,
        }
    }

    fn load(&self) -> (Config, RepositoryConfigReport) {
        Config::load_scoped(
            Config {
                data_dir: self.data_dir.clone(),
                audit_enabled: true,
                ..Config::default()
            },
            Some(&self.data_dir.join("config/user.conf")),
            Some(&Config::repository_config_path(&self.root)),
            None,
            Some(&self.root),
        )
        .unwrap()
    }

    fn engine(&self) -> WorkspaceEngine {
        let (config, report) = self.load();
        WorkspaceEngine::new_with_report(config, &report)
    }

    fn propose(&self, command: &str) -> CommandProposal {
        self.engine()
            .validation_orchestrator
            .propose_command(&self.root, command, "test")
            .unwrap()
    }

    /// The audit events of `event_type`, as JSON.
    fn audit(&self, event_type: &str) -> Vec<serde_json::Value> {
        let path = self.data_dir.join("audit/events.jsonl");
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|event| event["eventType"] == event_type)
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// Acceptance §4: a user `allow` runs a command the built-ins would block,
/// with a warning on the card and an override audit record.
#[test]
fn a_user_allow_over_a_built_in_block_runs_warns_and_is_audited() {
    let fixture = Fixture::new(
        "override",
        "command_rule=allow rm -rf /tmp/damaian-scratch/**\n",
        "",
    );
    let proposal = fixture.propose("rm -rf /tmp/damaian-scratch/cache");

    assert!(!proposal.blocked, "{proposal:?}");
    assert!(!proposal.requires_approval, "{proposal:?}");
    assert!(proposal.causes.is_empty(), "{:?}", proposal.causes);
    let rule = proposal.rule.clone().expect("the rule that decided");
    assert_eq!(rule.rule, "allow rm -rf /tmp/damaian-scratch/**");
    assert_eq!(rule.source, "user");
    assert_eq!(
        rule.summary(),
        "Allowed by your rule `allow rm -rf /tmp/damaian-scratch/**` (user config)"
    );
    assert_eq!(
        rule.warning().as_deref(),
        Some(
            "Damaian would block this: recursive `rm` of `/tmp/damaian-scratch/cache`. Your \
             rule `allow rm -rf /tmp/damaian-scratch/**` allows it."
        )
    );

    let overrides = fixture.audit("command_rule_override");
    assert_eq!(overrides.len(), 1, "{overrides:?}");
    let event = &overrides[0];
    assert_eq!(event["proposalId"], proposal.id.as_str());
    assert_eq!(event["rule"], "allow rm -rf /tmp/damaian-scratch/**");
    assert_eq!(event["ruleSource"], "user");
    assert_eq!(event["builtInDefault"], "recursive_remove");

    // The stored proposal keeps what decided, for a card shown after a
    // reload or a crash.
    let stored = fixture
        .engine()
        .validation_orchestrator
        .load_proposal(&proposal.id)
        .unwrap();
    assert_eq!(stored.rule, proposal.rule);

    // The rule covers what it names and nothing beside it.
    let root = fixture.propose("rm -rf /");
    assert!(root.blocked);
    assert_eq!(fixture.audit("command_rule_override").len(), 1);
}

#[test]
fn a_user_deny_blocks_and_the_card_names_the_rule() {
    let fixture = Fixture::new("deny", "command_rule=deny cargo publish *\n", "");
    let proposal = fixture.propose("cargo publish --dry-run");

    assert!(proposal.blocked);
    assert_eq!(
        proposal.causes,
        vec![ApprovalCause::RuleDenied {
            rule: "deny cargo publish *".to_string(),
            source: "user".to_string(),
        }]
    );
    assert_eq!(
        proposal.reasons[0],
        "Blocked by your rule `deny cargo publish *` (user config)"
    );
    let stored = fixture.audit("command_proposal_stored");
    assert_eq!(stored[0]["causes"], "rule_denied");
    assert_eq!(stored[0]["rule"], "deny cargo publish *");
    assert!(fixture.audit("command_rule_override").is_empty());
}

#[test]
fn a_repository_ask_asks_and_the_card_names_the_repository_rule() {
    let fixture = Fixture::new("repository-ask", "", "command_rule=ask cargo test *\n");
    let proposal = fixture.propose("cargo test --workspace");

    assert!(proposal.requires_approval);
    assert!(!proposal.blocked);
    assert_eq!(
        proposal.reasons[0],
        "Asks first: repository rule `ask cargo test *` (repository config)"
    );
    let rule = proposal.rule.expect("the rule that decided");
    assert_eq!(rule.source, "repository");
    assert_eq!(
        rule.summary(),
        "Asked by repository rule `ask cargo test *` (repository config)"
    );
    // The rule leads; the defaults' own reason (here the risky-commands
    // setting, which a validation command asks under) follows it.
    assert_eq!(
        fixture.audit("command_proposal_stored")[0]["causes"],
        "rule_asks,setting"
    );
}

#[test]
fn a_built_in_block_names_itself_when_no_rule_allows_it() {
    let fixture = Fixture::new("built-in", "", "");
    let proposal = fixture.propose("rm -fr /");

    assert!(proposal.blocked);
    assert_eq!(
        proposal.reasons[0],
        "Blocked: Damaian would block this (recursive `rm` of `/`); no rule of yours allows it"
    );
    let rule = proposal.rule.expect("the default that decided");
    assert_eq!(rule.source, "default");
    assert_eq!(rule.summary(), "Damaian's default: recursive `rm` of `/`");
    assert_eq!(rule.warning(), None);
}

/// Owner decision, 2026-11-21 (context.md D11): a blocklisted command that
/// cannot be analysed stays blocked rather than dropping to "ask".
#[test]
fn a_blocklisted_command_that_cannot_be_analysed_stays_blocked() {
    let fixture = Fixture::new("unanalysable", "command_blocklist=git push\n", "");
    let proposal = fixture.propose("git push $REMOTE");
    assert!(proposal.blocked, "{proposal:?}");
    assert_eq!(
        proposal.causes,
        vec![ApprovalCause::RuleDenied {
            rule: "deny git push *".to_string(),
            source: "user".to_string(),
        }]
    );

    // A repository's deny is a deny too; an unanalysable command no deny
    // names still only asks.
    let fixture = Fixture::new(
        "unanalysable-repository",
        "",
        "command_blocklist=npm publish\n",
    );
    assert!(fixture.propose("npm publish $PACKAGE").blocked);
    let other = fixture.propose("npm install $PACKAGE");
    assert!(other.requires_approval && !other.blocked, "{other:?}");
}

/// D10: the old allowlist branch kept `require_approval_for_all_commands`, so
/// a migrated `allow` still asks when it is on, and the card names the rule.
#[test]
fn an_allow_still_asks_when_every_command_must() {
    let fixture = Fixture::new(
        "all-ask",
        "command_allowlist=npm test\nrequire_approval_for_all_commands=true\n",
        "",
    );
    let proposal = fixture.propose("npm test");
    assert!(proposal.requires_approval);
    assert!(matches!(
        proposal.causes.as_slice(),
        [ApprovalCause::Setting { key, .. }] if key == "require_approval_for_all_commands"
    ));
    let rule = proposal.rule.expect("the rule that allowed it");
    assert_eq!(
        rule.summary(),
        format!(
            "Allowed by your rule `allow npm test in {}` (user config)",
            fixture.root.display()
        )
    );
}

/// Requirement 9: Always allow writes a user rule of the exact command, and
/// that rule is what lets the next proposal run.
#[test]
fn always_allow_writes_an_exact_user_rule_that_decides() {
    let fixture = Fixture::new("always", "", "");
    let proposal = fixture.propose("make release");
    assert!(proposal.requires_approval);
    fixture
        .engine()
        .validation_orchestrator
        .allow_command_always(&proposal.id, "tester")
        .unwrap();

    let written = fs::read_to_string(fixture.data_dir.join("config/user.conf")).unwrap();
    let line = format!(
        "command_rule=allow make release in {}",
        fixture.root.display()
    );
    assert!(written.contains(&line), "{written}");
    let granted = fixture.audit("command_allowlisted");
    assert_eq!(
        granted[0]["rule"],
        format!("allow make release in {}", fixture.root.display())
    );

    let again = fixture.propose("make release");
    assert!(!again.requires_approval, "{again:?}");
    assert_eq!(again.rule.map(|rule| rule.source).as_deref(), Some("user"));
    // Exact: another command, or the same one elsewhere, still asks.
    assert!(fixture.propose("make release-all").requires_approval);
    let elsewhere = fixture.root.join("sub");
    fs::create_dir_all(&elsewhere).unwrap();
    let elsewhere = fixture
        .engine()
        .validation_orchestrator
        .propose_command_at(&fixture.root, Path::new(&elsewhere), "make release", "test")
        .unwrap();
    assert!(elsewhere.requires_approval);
}

/// A command no rule can say exactly is not offered *Always allow*: the
/// grant would cover more than was approved (`*` reads as a wildcard).
#[test]
fn always_allow_is_not_offered_for_a_command_a_rule_cannot_say_exactly() {
    let config = Config::default();
    assert!(workspace_engine::allow_always_eligible(
        &config,
        "make release",
        false
    ));
    for command in ["find . -name '*.rs'", "echo $HOME", "grep x in /tmp"] {
        assert!(
            !workspace_engine::allow_always_eligible(&config, command, false),
            "{command}"
        );
    }
}

/// Spec 111 D11 (Task 3b) still holds now that the rules decide: which shell
/// special parameters a command may use, where, and that the opt-in reaches
/// the rules only.
#[test]
fn shell_special_parameters_decide_as_d11_says_with_rules_live() {
    let runs = |fixture: &Fixture, command: &str| {
        let proposal = fixture.propose(command);
        !proposal.requires_approval && !proposal.blocked
    };
    let fixture = Fixture::new(
        "parameters",
        "command_rule=allow mytool *\ncommand_rule=allow other /tmp/run-*\n",
        "",
    );
    // `$?` `$#` `$$` `$-` inside a word.
    for command in [
        "other /tmp/run-$$",
        "other /tmp/run-$?",
        "other /tmp/run-$#",
        "other /tmp/run-$-",
    ] {
        assert!(runs(&fixture, command), "{command}");
    }
    // `$1`–`$9` `$@` `$*` `$!` only as a whole argument.
    for command in [
        "mytool \"$1\"",
        "mytool $9",
        "mytool \"$@\"",
        "mytool $*",
        "mytool $!",
    ] {
        assert!(runs(&fixture, command), "{command}");
    }
    for command in [
        "other /tmp/run-$1",
        "other /tmp/run-$@",
        "other /tmp/run-$!",
    ] {
        assert!(!runs(&fixture, command), "{command}");
    }
    // `$0` `$_` `$IFS` only when the user turns them on.
    for command in ["mytool $0", "mytool $_", "mytool $IFS"] {
        assert!(!runs(&fixture, command), "{command}");
    }

    let opted_in = Fixture::new(
        "parameters-opt-in",
        "command_rule=allow mytool *\ncommand_shell_parameters=$0 $_ $IFS\n",
        "",
    );
    for command in ["mytool $0", "mytool $_", "mytool $IFS"] {
        assert!(runs(&opted_in, command), "{command}");
    }
    // The opt-in is the rules' only: the read-only boundary keeps the
    // default set, so a read-only command using `$_` still asks.
    assert!(!runs(&opted_in, "ls $_"));

    // A repository may only narrow the key.
    let narrowed = Fixture::new(
        "parameters-narrowed",
        "command_rule=allow mytool *\ncommand_shell_parameters=$0 $_\n",
        "command_shell_parameters=$0 $IFS\n",
    );
    assert!(runs(&narrowed, "mytool $0"));
    assert!(!runs(&narrowed, "mytool $_"));
    assert!(!runs(&narrowed, "mytool $IFS"));

    // A quoted `'$$'` is a literal file name and never the real `$$`.
    let quoted = Fixture::new(
        "parameters-quoted",
        "command_rule=allow other /tmp/'$$'\n",
        "",
    );
    assert!(runs(&quoted, "other /tmp/'$$'"));
    assert!(!runs(&quoted, "other /tmp/$$"));
    let real = Fixture::new("parameters-real", "command_rule=allow other /tmp/$$\n", "");
    assert!(runs(&real, "other /tmp/$$"));
    assert!(!runs(&real, "other /tmp/'$$'"));
}
