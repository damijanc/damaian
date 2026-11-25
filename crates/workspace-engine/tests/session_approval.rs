//! Spec 109 D5 (Task 5): **Approve for this session**. Which commands are
//! offered it, decided by why the policy asks and with no list of
//! ecosystems; what choosing it covers (the exact command in the exact
//! directory, in this session only); the before/after check that revokes it
//! when a covered run changes the repository; and that repository config can
//! only turn it off.
//!
//! Proposals are stored and run through the `ValidationOrchestrator`, inside
//! the session scope a turn sets, so the decisions, the session log and the
//! audit are the ones a turn produces.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::cancel::CancelToken;
use workspace_engine::session_approval::with_session;
use workspace_engine::{CommandProposal, Config, RepositoryConfigReport, WorkspaceEngine};

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// The check script the tests approve. It writes only ignored build output,
/// unless a `write-source` file asks it to change a tracked file, which is
/// the run the backstop must catch.
const CHECK_SCRIPT: &str = "#!/bin/sh\nmkdir -p target\necho built > target/out\nif [ -f write-source ]; then echo changed > src.txt; fi\necho ok\n";

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    data_dir: PathBuf,
}

impl Fixture {
    /// A Git repository with a committed `src.txt`, an ignored `target/`,
    /// and `bin/check`; user and repository config as given.
    fn new(user: &str, repository: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // No test name goes into this path, so the process id is what keeps
        // it unique: nextest gives each test its own process with the counter
        // at 1, and macOS time has microsecond resolution, so two tests started
        // together would share a base and one's `Drop` would delete the other's
        // (OBSERVATIONS 101).
        let base = std::env::temp_dir().join(format!(
            "damaian-session-approval-{}-{now}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let root = base.join("project");
        let data_dir = base.join("data");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("crates/member")).unwrap();
        fs::create_dir_all(data_dir.join("config")).unwrap();
        fs::write(data_dir.join("config/user.conf"), user).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("src.txt"), "original\n").unwrap();
        fs::write(root.join("crates/member/lib.txt"), "member\n").unwrap();
        let script = root.join("bin/check");
        fs::write(&script, CHECK_SCRIPT).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let repository_config = Config::repository_config_path(&root);
        fs::create_dir_all(repository_config.parent().unwrap()).unwrap();
        fs::write(&repository_config, repository).unwrap();
        let fixture = Self {
            base,
            root,
            data_dir,
        };
        fixture.git(&["init", "-q"]);
        fixture.git(&["add", "."]);
        fixture.git(&[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ]);
        fixture
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
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

    /// A fresh engine, as each shell request builds one, so state that
    /// survives here survives a restart.
    fn engine(&self) -> WorkspaceEngine {
        let (config, report) = self.load();
        WorkspaceEngine::new_with_report(config, &report)
    }

    fn propose_in(&self, session: &str, directory: &Path, command: &str) -> CommandProposal {
        with_session(session, || {
            self.engine()
                .validation_orchestrator
                .propose_command_at(&self.root, directory, command, "test")
                .unwrap()
        })
    }

    fn propose(&self, session: &str, command: &str) -> CommandProposal {
        self.propose_in(session, &self.root, command)
    }

    fn approve_for_session(&self, session: &str, proposal: &CommandProposal) {
        self.engine()
            .validation_orchestrator
            .approve_for_session(session, &proposal.id, "test_user")
            .unwrap();
    }

    fn run(
        &self,
        session: &str,
        proposal: &CommandProposal,
        approved: bool,
    ) -> workspace_engine::CommandRunRecord {
        with_session(session, || {
            self.engine()
                .validation_orchestrator
                .run_proposal(
                    &proposal.id,
                    approved,
                    "test_user",
                    None,
                    &CancelToken::new(),
                    &mut |_| {},
                )
                .unwrap()
        })
    }

    /// Why **Approve for this session** is not offered for `command` at the
    /// root, or `None` when it is.
    fn refusal(&self, command: &str) -> Option<String> {
        self.engine()
            .validation_orchestrator
            .session_approval_refusal(&self.propose("refusals", command))
    }

    fn audit(&self, event_type: &str) -> Vec<serde_json::Value> {
        fs::read_to_string(self.data_dir.join("audit/events.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| event["eventType"] == event_type)
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn unknown_programs_from_any_ecosystem_are_offered_by_why_the_policy_asks() {
    let fixture = Fixture::new("", "");
    // No list of languages decides: each asks only because Damaian does not
    // know the program, or because checks ask first.
    for command in [
        "cargo test",
        "cargo nextest run -p workspace-engine --test plan_turn",
        "cargo clippy --all-targets -- -D warnings",
        "cargo fmt --all -- --check",
        "stack test",
        "lein test",
        "go test ./...",
        "phpunit",
        "ctest",
        "bin/check",
    ] {
        assert_eq!(fixture.refusal(command), None, "{command}");
    }
}

#[test]
fn chains_writers_and_unanalysable_commands_are_never_offered() {
    let fixture = Fixture::new("", "");
    for (command, reason) in [
        ("cargo test && cargo clippy", "shell control"),
        ("cargo test | tee log", "shell control"),
        ("cargo clippy --fix", "writes or removes files"),
        ("stack test --fix", "writes or removes files"),
        ("cargo fmt", "writes or removes files"),
        ("prettier --write src", "writes or removes files"),
        ("make $TARGET", "cannot be approved"),
        ("curl https://example.com", "network"),
        ("docker compose up", "Docker"),
        ("rm -rf src", "writes or removes files"),
        ("git add .", "writes or removes files"),
        ("git commit -m x", "change your system or repository"),
    ] {
        let refusal = fixture.refusal(command);
        assert!(
            refusal.as_deref().is_some_and(|text| text.contains(reason)),
            "{command}: {refusal:?}"
        );
    }
    // A command that already runs without asking has nothing to offer.
    assert!(
        fixture
            .refusal("git status")
            .is_some_and(|text| text.contains("runs without asking"))
    );
}

#[test]
fn settings_and_places_that_rule_it_out() {
    // Every command asks: a session rule could not lift that.
    let every = Fixture::new("require_approval_for_all_commands=true\n", "");
    assert!(
        every
            .refusal("stack test")
            .is_some_and(|text| text.contains("every command asks"))
    );
    // Turned off by the user.
    let off = Fixture::new("session_command_approval=false\n", "");
    assert!(
        off.refusal("stack test")
            .is_some_and(|text| text.contains("turned off"))
    );
    // Outside a Git checkout the backstop cannot see what a run changed.
    let fixture = Fixture::new("", "");
    fs::remove_dir_all(fixture.root.join(".git")).unwrap();
    assert!(
        fixture
            .refusal("stack test")
            .is_some_and(|text| text.contains("not a Git checkout"))
    );
    // A user rule that asks every time is not lifted by a session approval.
    let asks = Fixture::new("command_rule = ask stack test\n", "");
    assert!(
        asks.refusal("stack test")
            .is_some_and(|text| text.contains("command rule asks"))
    );
}

#[test]
fn approved_once_it_covers_that_command_in_that_directory_in_that_session_only() {
    let fixture = Fixture::new("", "");
    let first = fixture.propose("s1", "stack test");
    assert!(first.requires_approval);
    fixture.approve_for_session("s1", &first);

    let again = fixture.propose("s1", "stack test");
    assert!(!again.requires_approval, "{again:?}");
    let rule = again.rule.as_ref().expect("the session rule decided");
    assert_eq!(rule.source, "session");
    assert_eq!(rule.summary(), "Approved for this session");

    // Another command, another directory and another session still ask.
    assert!(fixture.propose("s1", "stack test --fast").requires_approval);
    assert!(fixture.propose("s1", "lein test").requires_approval);
    assert!(
        fixture
            .propose_in("s1", &fixture.root.join("crates/member"), "stack test")
            .requires_approval
    );
    assert!(fixture.propose("s2", "stack test").requires_approval);
    // Outside any turn's scope nothing is approved for a session.
    assert!(
        fixture
            .engine()
            .validation_orchestrator
            .propose_command(&fixture.root, "stack test", "test")
            .unwrap()
            .requires_approval
    );

    let chosen = fixture.audit("command_approved_for_session");
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[0]["sessionId"], "s1");
    assert_eq!(chosen[0]["command"], "stack test");
}

#[test]
fn a_covered_run_is_audited_and_a_run_that_changes_only_ignored_output_keeps_it() {
    let fixture = Fixture::new("", "");
    let first = fixture.propose("s1", "bin/check");
    fixture.approve_for_session("s1", &first);
    // The approving run is covered too.
    let record = fixture.run("s1", &first, true);
    assert_eq!(record.execution.exit_code, Some(0));
    assert_eq!(record.session_approval_revoked, None);

    let later = fixture.propose("s1", "bin/check");
    assert!(!later.requires_approval);
    let record = fixture.run("s1", &later, false);
    assert_eq!(record.session_approval_revoked, None);
    assert!(fixture.root.join("target/out").exists());
    let executed = fixture.audit("stored_command_executed");
    assert_eq!(executed.last().unwrap()["approvedBy"], "session_approval");
    assert!(!fixture.propose("s1", "bin/check").requires_approval);
}

#[test]
fn a_covered_run_that_changes_a_file_revokes_the_approval_and_says_so() {
    let fixture = Fixture::new("", "");
    let first = fixture.propose("s1", "bin/check");
    fixture.approve_for_session("s1", &first);
    fixture.run("s1", &first, true);

    // The flag file exists before the run, so only the script's own write
    // differs between before and after.
    fs::write(fixture.root.join("write-source"), "").unwrap();
    let later = fixture.propose("s1", "bin/check");
    assert!(!later.requires_approval);
    let record = fixture.run("s1", &later, false);
    assert_eq!(
        record.session_approval_revoked,
        Some(vec!["src.txt".to_string()])
    );
    let revoked = fixture.audit("command_session_approval_revoked");
    assert_eq!(revoked.len(), 1);
    assert_eq!(revoked[0]["reason"], "changed_repository");
    assert_eq!(revoked[0]["changed"], "src.txt");

    let next = fixture.propose("s1", "bin/check");
    assert!(next.requires_approval, "{next:?}");
    let notices = workspace_engine::session_approval::take_notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.contains("`bin/check` changed the repository (`src.txt`)")),
        "{notices:?}"
    );
}

#[test]
fn a_restart_keeps_it_and_archive_or_revoke_ends_it() {
    let fixture = Fixture::new("", "");
    let first = fixture.propose("s1", "stack test");
    fixture.approve_for_session("s1", &first);
    let other = fixture.propose("s2", "stack test");
    fixture.approve_for_session("s2", &other);

    // Every `propose` builds a fresh engine from disk: a restart.
    assert!(!fixture.propose("s1", "stack test").requires_approval);

    let engine = fixture.engine();
    let revoked = engine
        .validation_orchestrator
        .revoke_session_approvals("s1", "test_user")
        .unwrap();
    assert_eq!(revoked, 1);
    assert!(fixture.propose("s1", "stack test").requires_approval);
    // Another session's approval is its own.
    assert!(!fixture.propose("s2", "stack test").requires_approval);
    assert_eq!(
        engine
            .validation_orchestrator
            .revoke_session_approvals("s1", "test_user")
            .unwrap(),
        0
    );
}

#[test]
fn archiving_a_session_ends_its_approvals() {
    let fixture = Fixture::new("", "");
    let engine = fixture.engine();
    let session = engine
        .session_store
        .create_session(&fixture.root.to_string_lossy(), "Session")
        .unwrap();
    let first = fixture.propose(&session.id, "stack test");
    fixture.approve_for_session(&session.id, &first);
    assert!(!fixture.propose(&session.id, "stack test").requires_approval);

    engine
        .session_store
        .archive_session(&session.id, false)
        .unwrap();
    assert!(fixture.propose(&session.id, "stack test").requires_approval);
    engine.session_store.unarchive_session(&session.id).unwrap();
    assert!(fixture.propose(&session.id, "stack test").requires_approval);
}

#[test]
fn a_repository_can_turn_it_off_but_never_on() {
    // A repository turning it off stops the offer and earlier approvals.
    let fixture = Fixture::new("", "");
    let first = fixture.propose("s1", "stack test");
    fixture.approve_for_session("s1", &first);
    fs::write(
        Config::repository_config_path(&fixture.root),
        "session_command_approval=false\n",
    )
    .unwrap();
    assert!(fixture.propose("s1", "stack test").requires_approval);
    assert!(
        fixture
            .refusal("stack test")
            .is_some_and(|text| text.contains("turned off"))
    );
    let refused = fixture
        .engine()
        .validation_orchestrator
        .approve_for_session("s1", &fixture.propose("s1", "lein test").id, "test_user");
    assert!(refused.is_err());

    // A repository cannot turn it on when the user turned it off.
    let off = Fixture::new(
        "session_command_approval=false\n",
        "session_command_approval=true\n",
    );
    let (config, report) = off.load();
    assert!(!config.session_command_approval);
    assert!(
        report
            .rejected_keys
            .iter()
            .any(|key| key.key == "session_command_approval"),
        "{:?}",
        report.rejected_keys
    );
    assert!(
        off.refusal("stack test")
            .is_some_and(|text| text.contains("turned off"))
    );
}

#[test]
fn a_command_that_does_not_qualify_cannot_be_approved_for_the_session() {
    let fixture = Fixture::new("", "");
    let chain = fixture.propose("s1", "stack test && stack build");
    let refused = fixture
        .engine()
        .validation_orchestrator
        .approve_for_session("s1", &chain.id, "test_user");
    assert!(refused.is_err());
    assert!(fixture.audit("command_approved_for_session").is_empty());
    assert!(
        fixture
            .propose("s1", "stack test && stack build")
            .requires_approval
    );
}
