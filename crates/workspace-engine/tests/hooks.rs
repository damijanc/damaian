//! Hooks, per `docs/specs/32_hooks/proposal.md`.
//!
//! Task 1 pins hook configuration and how each config scope may set it
//! (`context.md` §7.1): a repository may define a hook of its own, which a
//! later task keeps inactive until the user reviews it, and may never touch a
//! user's hook or record a review. A profile carries neither.
//!
//! Task 2 pins the contract (`context.md` §5, §6, §12): the envelope's field
//! names, the closed response schema with every unknown field a widening
//! attempt, the bounds as failures, the order hooks run in, and resolution
//! by `deny` > `request_approval` > `warn` > `allow` whatever that order.
//!
//! Task 3 pins the runner (`context.md` §2, §8): the envelope goes in on
//! stdin, the environment is cleared, a hook past its deadline or over the
//! output ceiling is killed with its group and fails, and stderr comes back
//! redacted and bounded.
//!
//! Task 5 pins requested commands and recursion (`context.md` §9): a hook's
//! command runs only when policy would run it unasked, is bracketed by a
//! `hook_command` marker, never re-enters the hook that asked for it, and is
//! bounded by `MAX_HOOK_DEPTH` without the nested action being blocked.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use workspace_engine::finding::{FindingSource, Severity};
use workspace_engine::hooks::{
    self, HOOK_API_VERSION, HookActivation, HookContext, HookDispatcher, HookEnvelope,
    HookEnvelopeParts, HookFailureKind, HookOutcome, HookRun, MAX_HOOK_COMMANDS,
    MAX_HOOK_CONTEXT_CHARS, MAX_HOOK_CONTEXT_ENTRIES, MAX_HOOK_DEPTH, MAX_HOOK_FINDINGS,
    MAX_HOOK_REASON_CHARS, MAX_HOOK_STDERR_BYTES, MAX_HOOK_STDOUT_BYTES, ResponseError, Verdict,
    WideningAttempt, activation, hook_definition_hash,
};
use workspace_engine::{
    AuditLog, CancelToken, ChatTurnOptions, ChatTurnResult, CommandPolicy, CommandRunner,
    CommandStore, Config, ConfigOverlay, ConfigScope, EffectivePolicy, HookConfig, HookEvent,
    HookMode, HookReview, HookSource, MAX_HOOK_TIMEOUT_MS, MockModelAdapter, ModelAdapter,
    ModelProviderConfig, ProcessRegistry, RepositoryConfigReport, RepositoryKeyClass,
    SecretScanner, Session, SessionMode, SessionStore, SourceKind, Task, TaskStatus, ToolCall,
    TurnProgress, TurnSink, ValidationOrchestrator, WorkspaceEngine, repository_id_for_root,
    split_profile_keys,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-hooks-{name}-{now}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Loads user, repository and admin config at their real scopes, the way a
/// desktop request does.
fn try_load(
    name: &str,
    user: &str,
    repository: &str,
    admin: &str,
) -> workspace_engine::Result<(Config, RepositoryConfigReport)> {
    let root = temp_dir(name);
    let data_dir = root.join(".damaian-data");
    let user_config = data_dir.join("config").join("user.conf");
    let admin_config = data_dir.join("config").join("admin.conf");
    let repository_config = Config::repository_config_path(&root);
    write(&user_config, user);
    write(&admin_config, admin);
    write(&repository_config, repository);
    let base = Config {
        data_dir,
        enable_index_watcher: false,
        ..Config::default()
    };
    let loaded = Config::load_scoped(
        base,
        Some(&user_config),
        Some(&repository_config),
        Some(&admin_config),
        Some(&root),
    );
    let _ = fs::remove_dir_all(&root);
    loaded
}

fn load(name: &str, user: &str, repository: &str) -> (Config, RepositoryConfigReport) {
    try_load(name, user, repository, "").expect("fixture config should load")
}

fn refusal(report: &RepositoryConfigReport, key: &str) -> Option<RepositoryKeyClass> {
    report
        .rejected_keys
        .iter()
        .find(|rejected| rejected.key == key)
        .map(|rejected| rejected.class)
}

const USER_GUARD: &str = concat!(
    "hook.guard.command=/usr/local/bin/guard\n",
    "hook.guard.events=before_patch_apply|before_tool\n",
);

const REPOSITORY_LINT: &str = concat!(
    "hook.lint.command=./scripts/lint-hook.sh\n",
    "hook.lint.events=after_patch_apply\n",
    "hook.lint.mode=optional\n",
);

#[test]
fn a_user_hook_parses_with_defaults() {
    let (config, report) = load("defaults", USER_GUARD, "");
    assert!(
        report.rejected_keys.is_empty(),
        "{:?}",
        report.rejected_keys
    );
    let hook = config.hook("guard").expect("the user hook is configured");
    assert_eq!(hook.command, "/usr/local/bin/guard");
    assert!(hook.args.is_empty());
    assert_eq!(
        hook.events,
        [HookEvent::BeforePatchApply, HookEvent::BeforeTool]
    );
    // Fail closed by default (proposal §5.5).
    assert_eq!(hook.mode, HookMode::Mandatory);
    assert_eq!(hook.order, 0);
    assert_eq!(hook.timeout_ms, None);
    assert!(hook.enabled);
    assert_eq!(hook.source, HookSource::User);
}

#[test]
fn every_event_name_parses_and_prints_as_itself() {
    let names = [
        "session_start",
        "before_context",
        "before_tool",
        "after_tool",
        "before_patch_apply",
        "after_patch_apply",
        "after_check",
        "before_task_complete",
        "session_end",
    ];
    assert_eq!(HookEvent::ALL.len(), names.len());
    for (event, name) in HookEvent::ALL.iter().zip(names) {
        assert_eq!(event.as_str(), name);
        assert_eq!(HookEvent::parse(name), Some(*event));
    }
    assert_eq!(HookEvent::parse("before_lunch"), None);
}

#[test]
fn a_hook_without_command_or_events_is_a_user_config_error() {
    for (name, user) in [
        ("no-events", "hook.guard.command=/usr/local/bin/guard\n"),
        ("no-command", "hook.guard.events=before_tool\n"),
    ] {
        let error = try_load(name, user, "", "").expect_err("an incomplete user hook must fail");
        assert!(error.to_string().contains("guard"), "{name}: {error}");
    }
}

#[test]
fn an_incomplete_repository_hook_is_skipped_as_unparsable() {
    let (config, report) = load(
        "incomplete-repository",
        "",
        "hook.lint.command=./scripts/lint-hook.sh\n",
    );
    assert!(config.hook("lint").is_none());
    assert_eq!(
        refusal(&report, "hook.lint.command"),
        Some(RepositoryKeyClass::Unparsable)
    );
}

#[test]
fn an_unknown_event_or_mode_is_unparsable_at_repository_scope_and_fatal_at_user_scope() {
    for (field, line) in [
        ("events", "hook.guard.events=before_lunch\n"),
        ("mode", "hook.guard.mode=sometimes\n"),
        ("order", "hook.guard.order=first\n"),
        ("timeout_ms", "hook.guard.timeout_ms=0\n"),
        ("enabled", "hook.guard.enabled=maybe\n"),
    ] {
        assert!(
            try_load(field, &format!("{USER_GUARD}{line}"), "", "").is_err(),
            "{field}: a bad user value must fail the load"
        );
        let (_, report) = load(field, "", line);
        assert_eq!(
            refusal(&report, &format!("hook.guard.{field}")),
            Some(RepositoryKeyClass::Unparsable),
            "{field}: {:?}",
            report.rejected_keys
        );
    }
}

#[test]
fn the_timeout_is_capped() {
    let at_cap = format!("{USER_GUARD}hook.guard.timeout_ms={MAX_HOOK_TIMEOUT_MS}\n");
    let (config, _) = load("timeout-cap", &at_cap, "");
    assert_eq!(
        config.hook("guard").unwrap().timeout_ms,
        Some(MAX_HOOK_TIMEOUT_MS)
    );
    let over = format!(
        "{USER_GUARD}hook.guard.timeout_ms={}\n",
        MAX_HOOK_TIMEOUT_MS + 1
    );
    assert!(try_load("timeout-over", &over, "", "").is_err());
}

#[test]
fn an_id_outside_the_profile_id_rule_is_rejected() {
    let long = "a".repeat(41);
    for id in ["Guard", "guard-hook", "guard.hook", "", long.as_str()] {
        let user = format!("hook.{id}.command=/bin/sh\nhook.{id}.events=before_tool\n");
        assert!(
            try_load("bad-id", &user, "", "").is_err(),
            "{id:?} must be rejected"
        );
    }
    let ok = "a".repeat(40);
    let user = format!("hook.{ok}.command=/bin/sh\nhook.{ok}.events=before_tool\n");
    assert!(try_load("good-id", &user, "", "").is_ok());
}

#[test]
fn a_repository_hook_is_defined_as_repository_sourced() {
    let (config, report) = load("repository-hook", "", REPOSITORY_LINT);
    assert!(
        report.rejected_keys.is_empty(),
        "{:?}",
        report.rejected_keys
    );
    let hook = config.hook("lint").expect("a repository may define a hook");
    assert_eq!(hook.source, HookSource::Repository);
    assert_eq!(hook.mode, HookMode::Optional);
    assert_eq!(hook.command, "./scripts/lint-hook.sh");
}

#[test]
fn a_repository_cannot_touch_a_user_hook() {
    for (field, line) in [
        ("enabled", "hook.guard.enabled=false\n"),
        ("mode", "hook.guard.mode=optional\n"),
        ("timeout_ms", "hook.guard.timeout_ms=60000\n"),
        ("order", "hook.guard.order=-1\n"),
        ("command", "hook.guard.command=/usr/bin/true\n"),
        ("args", "hook.guard.args=--off\n"),
        ("events", "hook.guard.events=session_end\n"),
    ] {
        let (config, report) = load(field, USER_GUARD, line);
        assert_eq!(
            refusal(&report, &format!("hook.guard.{field}")),
            Some(RepositoryKeyClass::Forbidden),
            "{field}: {:?}",
            report.rejected_keys
        );
        let hook = config.hook("guard").unwrap();
        assert!(hook.enabled, "{field}");
        assert_eq!(hook.mode, HookMode::Mandatory, "{field}");
        assert_eq!(hook.timeout_ms, None, "{field}");
        assert_eq!(hook.order, 0, "{field}");
        assert_eq!(hook.command, "/usr/local/bin/guard", "{field}");
        assert!(hook.args.is_empty(), "{field}");
        assert_eq!(
            hook.events,
            [HookEvent::BeforePatchApply, HookEvent::BeforeTool],
            "{field}"
        );
        assert_eq!(hook.source, HookSource::User, "{field}");
    }
}

#[test]
fn a_repository_may_disable_its_own_hook() {
    let (config, report) = load(
        "disable-own",
        "",
        &format!("{REPOSITORY_LINT}hook.lint.enabled=false\n"),
    );
    assert!(
        report.rejected_keys.is_empty(),
        "{:?}",
        report.rejected_keys
    );
    assert!(!config.hook("lint").unwrap().enabled);
}

#[test]
fn a_repository_cannot_record_a_review() {
    let key = "hook_review.repo_0123456789abcdef.lint";
    let (config, report) = load(
        "repository-review",
        "",
        &format!("{REPOSITORY_LINT}{key}=sha256:abc\n"),
    );
    assert_eq!(refusal(&report, key), Some(RepositoryKeyClass::UserOwned));
    assert!(config.hook_reviews_by_repository.is_empty());
}

#[test]
fn a_user_review_is_recorded_per_repository_and_hook() {
    let (config, report) = load(
        "user-review",
        concat!(
            "hook_review.repo_0123456789abcdef.lint=sha256:abc\n",
            "hook_review.repo_0123456789abcdef.notify=declined\n",
        ),
        "",
    );
    assert!(report.rejected_keys.is_empty());
    let reviews = &config.hook_reviews_by_repository["repo_0123456789abcdef"];
    assert_eq!(
        reviews["lint"],
        HookReview::Approved {
            hash: "sha256:abc".to_string()
        }
    );
    assert_eq!(reviews["notify"], HookReview::Declined);
    assert!(try_load("bad-review", "hook_review.repo_0123.lint=yes\n", "", "").is_err());
}

#[test]
fn admin_may_disable_a_user_hook_and_it_is_attributed_as_a_widening() {
    let (config, report) = try_load(
        "admin-disable",
        USER_GUARD,
        "",
        "hook.guard.enabled=false\nhook.guard.mode=optional\n",
    )
    .unwrap();
    let hook = config.hook("guard").unwrap();
    assert!(!hook.enabled);
    assert_eq!(hook.mode, HookMode::Optional);
    for key in ["hook.guard.enabled", "hook.guard.mode"] {
        let applied = report
            .applied
            .iter()
            .find(|applied| applied.key == key && applied.scope == ConfigScope::Admin)
            .unwrap_or_else(|| panic!("{key} was not attributed to admin"));
        assert!(applied.widened, "{key} loosened a user hook");
    }
    let policy = EffectivePolicy::from_load(&config, &report, None);
    assert!(policy.rule("hook.guard.enabled").unwrap().admin_widened);
}

#[test]
fn a_profile_cannot_carry_a_hook_or_a_review() {
    let text = concat!(
        "hook.guard.enabled=false\n",
        "hook.extra.command=/bin/sh\n",
        "hook.extra.events=before_tool\n",
        "hook_review.repo_0123456789abcdef.lint=sha256:abc\n",
        "allow_file_edits=false\n",
    );
    let overlay = ConfigOverlay::parse(text).unwrap();
    let (carried, refused) = split_profile_keys(overlay.clone());
    let class = |key: &str| {
        refused
            .iter()
            .find(|rejected| rejected.key == key)
            .map(|rejected| rejected.class)
    };
    assert_eq!(
        class("hook.guard.enabled"),
        Some(RepositoryKeyClass::Forbidden)
    );
    assert_eq!(
        class("hook.extra.command"),
        Some(RepositoryKeyClass::Forbidden)
    );
    assert_eq!(
        class("hook_review.repo_0123456789abcdef.lint"),
        Some(RepositoryKeyClass::UserOwned)
    );
    let carried_text = carried.to_policy_text();
    assert!(!carried_text.contains("hook"), "{carried_text}");
    assert!(carried_text.contains("allow_file_edits=false"));

    // The merge at profile scope agrees and changes nothing.
    let (mut config, _) = load("profile-merge", USER_GUARD, "");
    let before = config.clone();
    let outcome = config.apply_overlay_scoped(overlay, ConfigScope::Profile);
    assert_eq!(config.hooks, before.hooks);
    assert!(config.hook("extra").is_none());
    assert!(
        outcome
            .rejected
            .iter()
            .any(|rejected| rejected.key == "hook.guard.enabled"
                && rejected.class == RepositoryKeyClass::Forbidden)
    );
}

#[test]
fn hooks_round_trip_through_the_overlay_text() {
    let text = concat!(
        "hook.guard.command=/usr/local/bin/guard\n",
        "hook.guard.args=--strict|--quiet\n",
        "hook.guard.events=before_patch_apply|before_tool\n",
        "hook.guard.mode=optional\n",
        "hook.guard.order=-5\n",
        "hook.guard.timeout_ms=2500\n",
        "hook.guard.enabled=false\n",
        "hook_review.repo_0123456789abcdef.lint=sha256:abc\n",
        "hook_review.repo_0123456789abcdef.notify=declined\n",
    );
    let overlay = ConfigOverlay::parse(text).unwrap();
    let printed = overlay.to_policy_text();
    assert_eq!(
        ConfigOverlay::parse(&printed).unwrap(),
        overlay,
        "{printed}"
    );
    for line in text.lines() {
        assert!(printed.contains(line), "{line} was not printed: {printed}");
    }
}

#[test]
fn the_policy_view_lists_each_hook_with_its_source_and_a_human_name() {
    let (config, report) = load("policy-view", USER_GUARD, REPOSITORY_LINT);
    let policy = EffectivePolicy::from_load(&config, &report, None);
    for (key, kind) in [
        ("hook.guard.command", SourceKind::User),
        ("hook.guard.events", SourceKind::User),
        ("hook.guard.mode", SourceKind::Default),
        ("hook.lint.command", SourceKind::Repository),
        ("hook.lint.mode", SourceKind::Repository),
    ] {
        let rule = policy
            .rule(key)
            .unwrap_or_else(|| panic!("{key} is missing from the policy view"));
        assert_eq!(rule.sources[0].kind, kind, "{key}");
        assert_ne!(rule.label, rule.key, "{key} has no human name");
    }
    assert_eq!(policy.rule("hook.guard.mode").unwrap().value, "mandatory");
    assert_eq!(
        policy.rule("hook.guard.events").unwrap().value,
        "before_patch_apply|before_tool"
    );
}

// ---- Task 2: the contract -------------------------------------------------

/// Every verdict, by rank. A `match` with no wildcard, so a new verdict does
/// not compile until someone decides where it ranks. There is no verdict
/// above `Allow` that grants anything (proposal §5.2), and adding one must
/// be a visible decision, not a silent addition.
fn rank(verdict: Verdict) -> u8 {
    match verdict {
        Verdict::Allow => 0,
        Verdict::Warn => 1,
        Verdict::RequestApproval => 2,
        Verdict::Deny => 3,
    }
}

const ALL_VERDICTS: [Verdict; 4] = [
    Verdict::Allow,
    Verdict::Warn,
    Verdict::RequestApproval,
    Verdict::Deny,
];

fn permutations(items: &[Verdict]) -> Vec<Vec<Verdict>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for index in 0..items.len() {
        let mut rest = items.to_vec();
        let first = rest.remove(index);
        for mut tail in permutations(&rest) {
            tail.insert(0, first);
            out.push(tail);
        }
    }
    out
}

/// What the dispatcher will do: run hooks in order, stop after a `Deny`,
/// resolve what ran.
fn dispatch_in_order(verdicts: &[Verdict]) -> (Verdict, usize) {
    let mut seen = Vec::new();
    for verdict in verdicts {
        seen.push(*verdict);
        if hooks::short_circuits(*verdict) {
            break;
        }
    }
    let ran = seen.len();
    (hooks::resolve(seen), ran)
}

#[test]
fn the_verdicts_are_exactly_the_four_and_rank_deny_highest() {
    for verdict in ALL_VERDICTS {
        assert_eq!(Verdict::parse(verdict.as_str()), Some(verdict));
    }
    assert_eq!(
        ALL_VERDICTS.map(Verdict::as_str),
        ["allow", "warn", "request_approval", "deny"]
    );
    for pair in ALL_VERDICTS.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{:?} must rank below {:?}",
            pair[0],
            pair[1]
        );
        assert!(rank(pair[0]) < rank(pair[1]));
    }
    for widening in ["approve", "grant", "allow_all", "Allow", "DENY", ""] {
        assert_eq!(Verdict::parse(widening), None, "{widening}");
    }
}

#[test]
fn resolution_is_the_strongest_verdict_in_every_order() {
    assert_eq!(hooks::resolve(Vec::new()), Verdict::Allow);
    // Every non-empty subset of the four verdicts, in every order.
    for mask in 1u8..16 {
        let subset: Vec<Verdict> = ALL_VERDICTS
            .into_iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, verdict)| verdict)
            .collect();
        let strongest = *subset.iter().max_by_key(|verdict| rank(**verdict)).unwrap();
        for order in permutations(&subset) {
            assert_eq!(hooks::resolve(order.clone()), strongest, "{order:?}");
            let mut reversed = order.clone();
            reversed.reverse();
            assert_eq!(hooks::resolve(reversed), strongest, "{order:?} reversed");
            assert_eq!(
                dispatch_in_order(&order).0,
                strongest,
                "{order:?} dispatched"
            );
        }
    }
}

#[test]
fn a_deny_short_circuits_and_nothing_else_does() {
    assert!(hooks::short_circuits(Verdict::Deny));
    for verdict in [Verdict::Allow, Verdict::Warn, Verdict::RequestApproval] {
        assert!(!hooks::short_circuits(verdict), "{verdict:?}");
    }
    // A request is not the end: a later hook may still deny (§5.6).
    assert_eq!(
        dispatch_in_order(&[Verdict::RequestApproval, Verdict::Allow, Verdict::Deny]),
        (Verdict::Deny, 3)
    );
    assert_eq!(
        dispatch_in_order(&[Verdict::Deny, Verdict::RequestApproval, Verdict::Warn]),
        (Verdict::Deny, 1)
    );
}

fn hook(id: &str, order: i64, events: &[HookEvent], enabled: bool) -> HookConfig {
    HookConfig {
        id: id.to_string(),
        command: "/bin/sh".to_string(),
        args: vec![format!("{id}.sh")],
        events: events.to_vec(),
        mode: HookMode::Mandatory,
        order,
        timeout_ms: None,
        enabled,
        source: HookSource::User,
    }
}

fn ids(hooks: &[&HookConfig]) -> Vec<String> {
    hooks.iter().map(|hook| hook.id.clone()).collect()
}

#[test]
fn hooks_run_by_order_then_id_whatever_order_they_were_configured_in() {
    let tool = [HookEvent::BeforeTool];
    let configured = vec![
        hook("zeta", 0, &tool, true),
        hook("alpha", 5, &tool, true),
        hook("mid", 0, &tool, true),
        hook("first", -3, &tool, true),
        hook("beta", 0, &tool, true),
        hook("off", -10, &tool, false),
        hook("patches", -10, &[HookEvent::BeforePatchApply], true),
    ];
    let expected = ["first", "beta", "mid", "zeta", "alpha"];
    assert_eq!(
        ids(&hooks::ordered(&configured, HookEvent::BeforeTool)),
        expected
    );
    let mut reversed = configured.clone();
    reversed.reverse();
    assert_eq!(
        ids(&hooks::ordered(&reversed, HookEvent::BeforeTool)),
        expected
    );
    let mut rotated = configured.clone();
    rotated.rotate_left(3);
    assert_eq!(
        ids(&hooks::ordered(&rotated, HookEvent::BeforeTool)),
        expected
    );

    assert_eq!(
        ids(&hooks::ordered(&configured, HookEvent::BeforePatchApply)),
        ["patches"]
    );
    assert!(hooks::ordered(&configured, HookEvent::SessionEnd).is_empty());
}

#[test]
fn the_bounds_are_the_ones_context_md_names() {
    assert_eq!(HOOK_API_VERSION, 1);
    assert_eq!(MAX_HOOK_STDOUT_BYTES, 64 * 1024);
    assert_eq!(MAX_HOOK_REASON_CHARS, 1_000);
    assert_eq!(MAX_HOOK_FINDINGS, 20);
    assert_eq!(MAX_HOOK_CONTEXT_ENTRIES, 4);
    assert_eq!(MAX_HOOK_CONTEXT_CHARS, 4_000);
    assert_eq!(MAX_HOOK_COMMANDS, 3);
    assert_eq!(MAX_HOOK_STDERR_BYTES, 8 * 1024);
}

#[test]
fn every_failure_kind_has_its_audited_name() {
    let kinds = [
        (HookFailureKind::SpawnFailed, "spawn_failed"),
        (HookFailureKind::NonZeroExit, "non_zero_exit"),
        (HookFailureKind::Timeout, "timeout"),
        (HookFailureKind::Malformed, "malformed"),
        (HookFailureKind::VersionMismatch, "version_mismatch"),
        (HookFailureKind::OutputTooLarge, "output_too_large"),
        (HookFailureKind::ChangedSinceReview, "changed_since_review"),
    ];
    for (kind, name) in kinds {
        assert_eq!(kind.as_str(), name);
    }
}

#[test]
fn the_envelope_serialises_to_the_proposal_field_names() {
    let envelope = HookEnvelope {
        event: HookEvent::BeforePatchApply,
        hook_api_version: HOOK_API_VERSION,
        session_id: Some("session_1".to_string()),
        task_id: Some("task_1".to_string()),
        mode: "code".to_string(),
        profile: "default".to_string(),
        repository_root: PathBuf::from("/work/app"),
        hook_depth: 0,
        payload: json!({
            "patchId": "patch_1",
            "files": [{ "path": "migrations/003_add_index.sql", "status": "modified" }],
        }),
    };
    let value = serde_json::to_value(&envelope).unwrap();
    assert_eq!(
        value,
        json!({
            "event": "before_patch_apply",
            "hookApiVersion": 1,
            "sessionId": "session_1",
            "taskId": "task_1",
            "mode": "code",
            "profile": "default",
            "repositoryRoot": "/work/app",
            "hookDepth": 0,
            "payload": {
                "patchId": "patch_1",
                "files": [{ "path": "migrations/003_add_index.sql", "status": "modified" }],
            },
        })
    );

    // Every event goes out under its config name, and a dispatch with no
    // task (a legacy patch, `session_end`) says so with `null`.
    for event in HookEvent::ALL {
        let value = serde_json::to_value(HookEnvelope {
            event,
            task_id: None,
            ..envelope.clone()
        })
        .unwrap();
        assert_eq!(value["event"], event.as_str());
        assert!(value["taskId"].is_null());
    }
}

fn parse(text: &str) -> Result<hooks::HookResponse, ResponseError> {
    hooks::parse_response(text.as_bytes())
}

fn widening(text: &str) -> Vec<WideningAttempt> {
    match parse(text) {
        Err(ResponseError::Widening { attempts }) => attempts,
        other => panic!("{text} should be a widening attempt, got {other:?}"),
    }
}

fn malformed(text: &str) {
    match parse(text) {
        Err(ResponseError::Malformed(_)) => {}
        other => panic!("{text} should be malformed, got {other:?}"),
    }
}

#[test]
fn empty_output_is_allow() {
    for text in ["", "   ", "\n", " \t\r\n "] {
        let response = parse(text).unwrap_or_else(|error| panic!("{text:?}: {error:?}"));
        assert_eq!(response.verdict, Verdict::Allow);
        assert!(response.reason.is_none());
        assert!(response.findings.is_empty());
        assert!(response.context.is_empty());
        assert!(response.commands.is_empty());
    }
}

#[test]
fn a_full_response_parses() {
    let response = parse(
        r#"{
            "hookApiVersion": 1,
            "verdict": "deny",
            "reason": "migrations/ is applied by the release process, not by hand",
            "findings": [
                { "severity": "error", "summary": "hand-edited migration",
                  "details": "use the release tool", "path": "migrations/003.sql", "line": 4 },
                { "severity": "info", "summary": "note" }
            ],
            "context": ["Migrations are applied by CI."],
            "commands": ["cargo test"]
        }"#,
    )
    .unwrap();
    assert_eq!(response.verdict, Verdict::Deny);
    assert_eq!(
        response.reason.as_deref(),
        Some("migrations/ is applied by the release process, not by hand")
    );
    assert_eq!(response.findings.len(), 2);
    let first = &response.findings[0];
    assert_eq!(first.severity, Severity::Error);
    assert_eq!(first.summary, "hand-edited migration");
    assert_eq!(first.details.as_deref(), Some("use the release tool"));
    assert_eq!(first.path.as_deref(), Some("migrations/003.sql"));
    assert_eq!(first.line, Some(4));
    let second = &response.findings[1];
    assert_eq!(second.severity, Severity::Info);
    assert!(second.details.is_none() && second.path.is_none() && second.line.is_none());
    assert_eq!(response.context, ["Migrations are applied by CI."]);
    assert_eq!(response.commands, ["cargo test"]);
}

#[test]
fn a_missing_verdict_is_allow_and_each_verdict_parses() {
    assert_eq!(
        parse(r#"{"hookApiVersion":1}"#).unwrap().verdict,
        Verdict::Allow
    );
    for verdict in ALL_VERDICTS {
        let text = format!(
            r#"{{"hookApiVersion":1,"verdict":"{}","reason":"because"}}"#,
            verdict.as_str()
        );
        assert_eq!(parse(&text).unwrap().verdict, verdict, "{text}");
    }
    // `warn` and `allow` may come without a reason.
    assert_eq!(
        parse(r#"{"hookApiVersion":1,"verdict":"warn"}"#)
            .unwrap()
            .verdict,
        Verdict::Warn
    );
}

#[test]
fn an_unknown_verdict_or_field_is_a_widening_attempt() {
    // Even with no version: the version is the hook's own claim, so the
    // attempt is reported whatever it says.
    assert_eq!(
        widening(r#"{"verdict":"approve"}"#),
        [WideningAttempt::Verdict("approve".to_string())]
    );
    assert_eq!(
        widening(r#"{"hookApiVersion":1,"verdict":"allow","grant":"all"}"#),
        [WideningAttempt::Field("grant".to_string())]
    );
    assert_eq!(
        widening(r#"{"hookApiVersion":2,"verdict":"allow","autoApprove":true}"#),
        [WideningAttempt::Field("autoApprove".to_string())]
    );
    // Every attempt is named, not just the first, in a stable order.
    assert_eq!(
        widening(r#"{"hookApiVersion":1,"verdict":"skip_approval","zz":1,"aa":2}"#),
        [
            WideningAttempt::Field("aa".to_string()),
            WideningAttempt::Field("zz".to_string()),
            WideningAttempt::Verdict("skip_approval".to_string()),
        ]
    );
    // A finding is closed too.
    assert_eq!(
        widening(
            r#"{"hookApiVersion":1,"findings":[{"severity":"info","summary":"s","approved":true}]}"#
        ),
        [WideningAttempt::Field("findings.approved".to_string())]
    );
    // The name, never the value: a hook cannot smuggle text through it.
    let attempts = widening(r#"{"hookApiVersion":1,"grant":"sk-secret-value"}"#);
    assert!(!format!("{attempts:?}").contains("sk-secret-value"));
    // A widening attempt is handled as malformed output (§5.5).
    assert_eq!(
        ResponseError::Widening { attempts }.failure_kind(),
        HookFailureKind::Malformed
    );
}

#[test]
fn the_api_version_is_required_and_must_be_one() {
    malformed(r#"{"verdict":"allow"}"#);
    malformed(r#"{"hookApiVersion":"1"}"#);
    malformed(r#"{"hookApiVersion":1.5}"#);
    malformed(r#"{"hookApiVersion":null}"#);
    for version in ["0", "2", "99"] {
        let text = format!(r#"{{"hookApiVersion":{version},"verdict":"allow"}}"#);
        match parse(&text) {
            Err(error @ ResponseError::VersionMismatch(_)) => {
                assert_eq!(error.failure_kind(), HookFailureKind::VersionMismatch)
            }
            other => panic!("{text} should be a version mismatch, got {other:?}"),
        }
    }
}

#[test]
fn output_that_is_not_one_json_object_is_malformed() {
    for text in [
        "allow",
        "{",
        "[]",
        "1",
        r#""allow""#,
        r#"{"hookApiVersion":1} {"hookApiVersion":1}"#,
    ] {
        malformed(text);
    }
    match hooks::parse_response(&[0xff, 0xfe, b'{', b'}']) {
        Err(ResponseError::Malformed(_)) => {}
        other => panic!("invalid UTF-8 should be malformed, got {other:?}"),
    }
    assert_eq!(
        ResponseError::Malformed(String::new()).failure_kind(),
        HookFailureKind::Malformed
    );
}

#[test]
fn a_field_of_the_wrong_shape_is_malformed() {
    for text in [
        r#"{"hookApiVersion":1,"verdict":3}"#,
        r#"{"hookApiVersion":1,"reason":["a"]}"#,
        r#"{"hookApiVersion":1,"findings":{}}"#,
        r#"{"hookApiVersion":1,"findings":["oops"]}"#,
        r#"{"hookApiVersion":1,"findings":[{"summary":"no severity"}]}"#,
        r#"{"hookApiVersion":1,"findings":[{"severity":"fatal","summary":"s"}]}"#,
        r#"{"hookApiVersion":1,"findings":[{"severity":"info"}]}"#,
        r#"{"hookApiVersion":1,"findings":[{"severity":"info","summary":"s","line":0}]}"#,
        r#"{"hookApiVersion":1,"findings":[{"severity":"info","summary":"s","line":-1}]}"#,
        r#"{"hookApiVersion":1,"context":"one string"}"#,
        r#"{"hookApiVersion":1,"context":[1]}"#,
        r#"{"hookApiVersion":1,"commands":[{"command":"ls"}]}"#,
        r#"{"hookApiVersion":1,"commands":["  "]}"#,
    ] {
        malformed(text);
    }
}

#[test]
fn deny_and_request_approval_need_a_reason() {
    for verdict in ["deny", "request_approval"] {
        malformed(&format!(r#"{{"hookApiVersion":1,"verdict":"{verdict}"}}"#));
        malformed(&format!(
            r#"{{"hookApiVersion":1,"verdict":"{verdict}","reason":"  "}}"#
        ));
    }
}

fn response_with(field: &str, value: serde_json::Value) -> String {
    let mut object = json!({ "hookApiVersion": 1, "verdict": "warn", "reason": "r" });
    object[field] = value;
    object.to_string()
}

#[test]
fn every_bound_is_a_failure_not_a_truncation() {
    // At each bound: accepted, whole.
    let reason = "r".repeat(MAX_HOOK_REASON_CHARS);
    assert_eq!(
        parse(&response_with("reason", json!(reason)))
            .unwrap()
            .reason
            .unwrap(),
        reason
    );
    // Characters, not bytes: a thousand two-byte characters fit.
    let wide = "é".repeat(MAX_HOOK_REASON_CHARS);
    assert_eq!(
        parse(&response_with("reason", json!(wide)))
            .unwrap()
            .reason
            .unwrap(),
        wide
    );
    let finding = json!({ "severity": "warning", "summary": "s" });
    assert_eq!(
        parse(&response_with(
            "findings",
            json!(vec![finding.clone(); MAX_HOOK_FINDINGS])
        ))
        .unwrap()
        .findings
        .len(),
        MAX_HOOK_FINDINGS
    );
    let entry = "c".repeat(MAX_HOOK_CONTEXT_CHARS);
    let context = parse(&response_with(
        "context",
        json!(vec![entry.clone(); MAX_HOOK_CONTEXT_ENTRIES]),
    ))
    .unwrap()
    .context;
    assert_eq!(context, vec![entry.clone(); MAX_HOOK_CONTEXT_ENTRIES]);
    assert_eq!(
        parse(&response_with(
            "commands",
            json!(vec!["ls"; MAX_HOOK_COMMANDS])
        ))
        .unwrap()
        .commands
        .len(),
        MAX_HOOK_COMMANDS
    );

    // One past each bound: malformed.
    malformed(&response_with(
        "reason",
        json!("r".repeat(MAX_HOOK_REASON_CHARS + 1)),
    ));
    malformed(&response_with(
        "findings",
        json!(vec![finding; MAX_HOOK_FINDINGS + 1]),
    ));
    malformed(&response_with(
        "context",
        json!(vec![entry; MAX_HOOK_CONTEXT_ENTRIES + 1]),
    ));
    malformed(&response_with(
        "context",
        json!(["c".repeat(MAX_HOOK_CONTEXT_CHARS + 1)]),
    ));
    malformed(&response_with(
        "commands",
        json!(vec!["ls"; MAX_HOOK_COMMANDS + 1]),
    ));
}

#[test]
fn output_over_the_byte_ceiling_is_too_large() {
    // Valid JSON padded with whitespace, so only the size can fail it.
    let head = r#"{"hookApiVersion":1,"verdict":"allow"}"#;
    let at_ceiling = format!("{head}{}", " ".repeat(MAX_HOOK_STDOUT_BYTES - head.len()));
    assert_eq!(at_ceiling.len(), MAX_HOOK_STDOUT_BYTES);
    assert_eq!(parse(&at_ceiling).unwrap().verdict, Verdict::Allow);

    let over = format!("{at_ceiling} ");
    assert_eq!(over.len(), 64 * 1024 + 1);
    match parse(&over) {
        Err(error @ ResponseError::OutputTooLarge) => {
            assert_eq!(error.failure_kind(), HookFailureKind::OutputTooLarge)
        }
        other => panic!("64 KiB + 1 should be too large, got {other:?}"),
    }
    // Whitespace alone over the ceiling is still too large, not `allow`.
    assert!(matches!(
        parse(&" ".repeat(MAX_HOOK_STDOUT_BYTES + 1)),
        Err(ResponseError::OutputTooLarge)
    ));
}

#[test]
fn null_optional_fields_count_as_absent() {
    let response = parse(
        r#"{"hookApiVersion":1,"verdict":null,"reason":null,"findings":null,
            "context":null,"commands":null}"#,
    )
    .unwrap();
    assert_eq!(response.verdict, Verdict::Allow);
    assert!(response.reason.is_none());
    let response = parse(
        r#"{"hookApiVersion":1,"findings":[{"severity":"warning","summary":"s",
            "details":null,"path":null,"line":null}]}"#,
    )
    .unwrap();
    assert!(response.findings[0].path.is_none());
}

// Task 3: the runner. Every hook here is `/bin/sh <script>`, never an
// executable written by the test (freshly written executables stall on this
// Mac) and never a login shell.

fn runner_fixture(name: &str) -> (PathBuf, PathBuf) {
    let root = temp_dir(name);
    let data_dir = root.join(".damaian-data");
    fs::create_dir_all(&data_dir).unwrap();
    (root, data_dir)
}

fn script(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join("hooks").join(format!("{name}.sh"));
    write(&path, body);
    path
}

fn sh_hook(id: &str, script: &Path) -> HookConfig {
    HookConfig {
        args: vec![script.to_string_lossy().to_string()],
        ..hook(id, 0, &[HookEvent::BeforeTool], true)
    }
}

fn runner_envelope(root: &Path, payload: serde_json::Value) -> HookEnvelope {
    HookEnvelope {
        event: HookEvent::BeforeTool,
        hook_api_version: HOOK_API_VERSION,
        session_id: Some("session_1".to_string()),
        task_id: Some("task_1".to_string()),
        mode: "code".to_string(),
        profile: "default".to_string(),
        repository_root: root.to_path_buf(),
        hook_depth: 0,
        payload,
    }
}

fn run(
    hook: &HookConfig,
    envelope: &HookEnvelope,
    root: &Path,
    data_dir: &Path,
    timeout: Duration,
) -> HookRun {
    hooks::run_hook(
        hook,
        envelope,
        root,
        data_dir,
        "task_1",
        &SecretScanner::new(Vec::new()),
        timeout,
    )
}

fn registry_entries(data_dir: &Path) -> usize {
    ProcessRegistry::open(data_dir)
        .unwrap()
        .entries()
        .unwrap()
        .len()
}

/// Prints stdin back as the one `context` entry, JSON-escaped, so the test
/// can compare what arrived with what was sent.
const ECHO_STDIN: &str = r#"input=$(cat)
escaped=$(printf '%s' "$input" | sed 's/\\/\\\\/g; s/"/\\"/g')
printf '{"hookApiVersion":1,"context":["%s"]}' "$escaped"
"#;

#[test]
fn the_envelope_reaches_the_hook_on_stdin() {
    let (root, data_dir) = runner_fixture("runner-stdin");
    let hook = sh_hook("echo_stdin", &script(&root, "echo", ECHO_STDIN));
    let envelope = runner_envelope(&root, json!({ "tool": "read_file", "path": "src/lib.rs" }));

    let run = run(&hook, &envelope, &root, &data_dir, Duration::from_secs(10));

    assert_eq!(run.failure, None, "stderr: {}", run.stderr);
    assert!(run.exit.expect("the hook exited").success());
    let response = hooks::parse_response(&run.stdout).unwrap();
    let received: serde_json::Value = serde_json::from_str(&response.context[0]).unwrap();
    assert_eq!(received, serde_json::to_value(&envelope).unwrap());
    assert_eq!(
        registry_entries(&data_dir),
        0,
        "a finished hook leaves no registry entry"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_hook_environment_is_cleared_to_the_named_variables() {
    // SAFETY: nextest runs each test in its own process, so no other thread
    // reads the environment while this writes it.
    unsafe { std::env::set_var("MODEL_TEST_KEY", "sk-hook-must-not-see-this") };
    let (root, data_dir) = runner_fixture("runner-env");
    let hook = sh_hook(
        "print_env",
        &script(
            &root,
            "env",
            r#"names=$(env | cut -d= -f1 | tr '\n' ' ')
printf '{"hookApiVersion":1,"context":["%s","%s|%s|%s","%s"]}' \
  "$names" "$DAMAIAN_HOOK_ID" "$DAMAIAN_HOOK_EVENT" "$DAMAIAN_REPOSITORY_ROOT" "$(pwd -P)"
"#,
        ),
    );

    let run = run(
        &hook,
        &runner_envelope(&root, json!({})),
        &root,
        &data_dir,
        Duration::from_secs(10),
    );

    assert_eq!(run.failure, None, "stderr: {}", run.stderr);
    let response = hooks::parse_response(&run.stdout).unwrap();
    let names: Vec<&str> = response.context[0].split_whitespace().collect();
    assert!(
        !names.contains(&"MODEL_TEST_KEY"),
        "the model key's variable reached the hook: {names:?}"
    );
    // `context.md` §8's list, plus what `/bin/sh` exports itself on start.
    let allowed = [
        "PATH",
        "HOME",
        "USER",
        "LANG",
        "TMPDIR",
        "DAMAIAN_HOOK_ID",
        "DAMAIAN_HOOK_EVENT",
        "DAMAIAN_REPOSITORY_ROOT",
        "PWD",
        "OLDPWD",
        "SHLVL",
        "_",
    ];
    for name in &names {
        assert!(allowed.contains(name), "`{name}` was inherited by the hook");
    }
    assert!(names.contains(&"PATH"), "PATH is passed on: {names:?}");
    assert_eq!(
        response.context[1],
        format!("print_env|before_tool|{}", root.to_string_lossy())
    );
    assert_eq!(
        PathBuf::from(&response.context[2]),
        root.canonicalize().unwrap(),
        "the hook runs at the repository root"
    );
    let _ = fs::remove_dir_all(&root);
}

fn is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the pid exists; nothing is sent.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn a_hook_past_its_deadline_is_killed_with_its_group_and_unregistered() {
    let (root, data_dir) = runner_fixture("runner-timeout");
    let pids = root.join("pids");
    // A background child as well as the shell, so killing only the shell's
    // pid would leave a `sleep` behind and fail the last assertion.
    let hook = sh_hook(
        "hangs",
        &script(
            &root,
            "hang",
            &format!(
                "sleep 30 &\necho $$ $! > '{}'\nwait\n",
                pids.to_string_lossy()
            ),
        ),
    );

    // A hook can take longer than 300 ms just to start on this Mac (XProtect
    // holding a freshly linked test binary's children: `main`'s copy of this
    // test took 0.5-2 s per hook run). A deadline that expires before the
    // script records its pids proves nothing about the group kill, so that
    // run is repeated with a longer deadline. Each run is still bounded by
    // its own deadline.
    let mut recorded = None;
    for deadline in [300, 1_000, 3_000].map(Duration::from_millis) {
        let _ = fs::remove_file(&pids);
        let run = run(
            &hook,
            &runner_envelope(&root, json!({})),
            &root,
            &data_dir,
            deadline,
        );
        assert_eq!(run.failure, Some(HookFailureKind::Timeout));
        assert!(
            run.duration < deadline + Duration::from_millis(700),
            "the {deadline:?} deadline was not enforced: {:?}",
            run.duration
        );
        assert_eq!(registry_entries(&data_dir), 0);
        let written: Vec<i32> = fs::read_to_string(&pids)
            .unwrap_or_default()
            .split_whitespace()
            .map(|pid| pid.parse().unwrap())
            .collect();
        if written.len() == 2 {
            recorded = Some(written);
            break;
        }
    }
    let pids = recorded.expect("the hook never started within 3 s");
    assert!(!is_alive(pids[0]), "the hook was reaped");
    // The orphaned `sleep` is reaped by launchd, which may take a moment.
    let settle = Instant::now() + Duration::from_secs(1);
    while is_alive(pids[1]) && Instant::now() < settle {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !is_alive(pids[1]),
        "the hook's child outlived its group kill"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn output_over_the_ceiling_fails_and_reading_stops() {
    let (root, data_dir) = runner_fixture("runner-too-large");
    let envelope = runner_envelope(&root, json!({}));
    let just_over = sh_hook(
        "too_large",
        &script(&root, "large", "head -c 66560 /dev/zero | tr '\\0' a\n"),
    );
    let run_once = run(
        &just_over,
        &envelope,
        &root,
        &data_dir,
        Duration::from_secs(10),
    );
    assert_eq!(run_once.failure, Some(HookFailureKind::OutputTooLarge));
    assert!(run_once.stdout.len() <= MAX_HOOK_STDOUT_BYTES + 1);

    // Endless output is cut off at the ceiling, not run to its deadline.
    let endless = sh_hook("endless", &script(&root, "endless", "yes\n"));
    let run_endless = run(
        &endless,
        &envelope,
        &root,
        &data_dir,
        Duration::from_secs(30),
    );
    assert_eq!(run_endless.failure, Some(HookFailureKind::OutputTooLarge));
    assert!(run_endless.stdout.len() <= MAX_HOOK_STDOUT_BYTES + 1);
    // Far below the deadline, with room for a slow start (see the timeout
    // test).
    assert!(
        run_endless.duration < Duration::from_secs(10),
        "reading did not stop at the ceiling: {:?}",
        run_endless.duration
    );
    assert_eq!(registry_entries(&data_dir), 0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_non_zero_exit_is_a_failure() {
    let (root, data_dir) = runner_fixture("runner-exit");
    let hook = sh_hook(
        "exits",
        &script(
            &root,
            "exit",
            "printf '{\"hookApiVersion\":1,\"verdict\":\"allow\"}'\nexit 3\n",
        ),
    );
    let run = run(
        &hook,
        &runner_envelope(&root, json!({})),
        &root,
        &data_dir,
        Duration::from_secs(10),
    );
    assert_eq!(
        run.failure,
        Some(HookFailureKind::NonZeroExit),
        "an `allow` printed before a failing exit is still a failure"
    );
    assert_eq!(run.exit.and_then(|status| status.code()), Some(3));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_missing_command_fails_to_spawn() {
    let (root, data_dir) = runner_fixture("runner-spawn");
    let hook = HookConfig {
        command: root.join("no-such-hook").to_string_lossy().to_string(),
        args: Vec::new(),
        ..hook("missing", 0, &[HookEvent::BeforeTool], true)
    };
    let run = run(
        &hook,
        &runner_envelope(&root, json!({})),
        &root,
        &data_dir,
        Duration::from_secs(10),
    );
    assert_eq!(run.failure, Some(HookFailureKind::SpawnFailed));
    assert!(run.exit.is_none());
    assert!(run.stdout.is_empty());
    assert_eq!(registry_entries(&data_dir), 0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_hook_that_never_reads_its_stdin_still_completes() {
    let (root, data_dir) = runner_fixture("runner-no-stdin");
    // Far larger than a pipe buffer, so the write cannot finish before the
    // hook exits, and then fails with a broken pipe.
    let envelope = runner_envelope(&root, json!({ "blob": "x".repeat(1024 * 1024) }));
    let hook = sh_hook(
        "ignores_stdin",
        &script(
            &root,
            "ignore",
            "printf '{\"hookApiVersion\":1,\"verdict\":\"warn\"}'\n",
        ),
    );

    let answered = run(&hook, &envelope, &root, &data_dir, Duration::from_secs(10));

    assert_eq!(answered.failure, None, "stderr: {}", answered.stderr);
    assert_eq!(
        hooks::parse_response(&answered.stdout).unwrap().verdict,
        Verdict::Warn
    );
    assert!(
        answered.duration < Duration::from_secs(2),
        "{:?}",
        answered.duration
    );

    // One that neither reads nor exits: a write blocked on its full stdin
    // must not keep the runner from its deadline.
    let hangs = sh_hook(
        "ignores_and_hangs",
        &script(&root, "hang", "exec sleep 30\n"),
    );
    let hung = run(
        &hangs,
        &envelope,
        &root,
        &data_dir,
        Duration::from_millis(300),
    );
    assert_eq!(hung.failure, Some(HookFailureKind::Timeout));
    assert!(
        hung.duration < Duration::from_secs(1),
        "{:?}",
        hung.duration
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn stderr_is_redacted_and_keeps_only_its_last_8_kib() {
    let (root, data_dir) = runner_fixture("runner-stderr");
    let hook = sh_hook(
        "noisy",
        &script(
            &root,
            "noisy",
            "echo 'aws key AKIAIOSFODNN7EXAMPLE' >&2\n\
             head -c 20000 /dev/zero | tr '\\0' n >&2\n\
             echo >&2\n\
             echo 'second AKIAIOSFODNN7EXAMPLE end-of-stderr' >&2\n",
        ),
    );
    let run = run(
        &hook,
        &runner_envelope(&root, json!({})),
        &root,
        &data_dir,
        Duration::from_secs(10),
    );
    assert_eq!(run.failure, None);
    assert!(
        !run.stderr.contains("AKIAIOSFODNN7EXAMPLE"),
        "stderr kept a secret"
    );
    assert!(run.stderr.trim_end().ends_with("end-of-stderr"));
    assert!(
        run.stderr.len() <= MAX_HOOK_STDERR_BYTES,
        "stderr is {} bytes",
        run.stderr.len()
    );
    let _ = fs::remove_dir_all(&root);
}

// ---- Task 4: the dispatcher. `context.md` §7, §10, §11 ---------------------

/// The config the dispatcher reads, pointed at a scratch data directory so
/// the audit and PID registry never touch the user's own state.
fn config_for(root: &Path, hooks: Vec<HookConfig>) -> Config {
    let data_dir = root.join(".damaian-data");
    fs::create_dir_all(&data_dir).unwrap();
    Config {
        data_dir,
        enable_index_watcher: false,
        hooks,
        ..Config::default()
    }
}

fn dispatcher_for(config: &Config) -> HookDispatcher {
    let scanner = SecretScanner::new(Vec::new());
    let audit = AuditLog::new(&config.data_dir, true, scanner.clone());
    let policy = CommandPolicy::new(config.clone());
    let validation = ValidationOrchestrator::new(
        policy.clone(),
        CommandRunner::new(config.clone(), policy, audit.clone(), scanner.clone()),
        CommandStore::new(&config.data_dir),
        audit.clone(),
    );
    HookDispatcher::new(
        config.clone(),
        scanner,
        audit,
        validation,
        SessionStore::new(&config.data_dir),
    )
}

fn event_parts() -> HookEnvelopeParts {
    HookEnvelopeParts {
        session_id: Some("session_1".to_string()),
        task_id: Some("task_1".to_string()),
        mode: "code".to_string(),
        profile: "default".to_string(),
        payload: json!({}),
    }
}

fn dispatch(dispatcher: &HookDispatcher, event: HookEvent, root: &Path) -> HookOutcome {
    dispatcher.dispatch(event, event_parts(), root, &HookContext::root())
}

fn audit_events(data_dir: &Path) -> Vec<serde_json::Value> {
    let path = data_dir.join("audit").join("events.jsonl");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn events_of(data_dir: &Path, event_type: &str) -> Vec<serde_json::Value> {
    audit_events(data_dir)
        .into_iter()
        .filter(|event| event["eventType"] == event_type)
        .collect()
}

/// A hook script that answers with `body` as one JSON line.
fn json_response(body: &str) -> String {
    format!("cat <<'JSON'\n{body}\nJSON\n")
}

#[test]
fn the_definition_hash_covers_the_definition_and_the_files_it_names() {
    let root = temp_dir("definition-hash");
    let check = script(&root, "check", "echo one\n");
    let hook = sh_hook("check", &check);
    let base = hook_definition_hash(&hook, &root).unwrap();
    assert!(base.starts_with("sha256:"), "{base}");
    assert_eq!(base, hook_definition_hash(&hook, &root).unwrap());

    let mut mode = hook.clone();
    mode.mode = HookMode::Optional;
    let mut order = hook.clone();
    order.order = 7;
    let mut events = hook.clone();
    events.events = vec![HookEvent::AfterTool];
    let mut timeout = hook.clone();
    timeout.timeout_ms = Some(1_234);
    let mut command = hook.clone();
    command.command = "/bin/true".to_string();
    let mut args = hook.clone();
    args.args.push("--strict".to_string());
    for (field, changed) in [
        ("mode", mode),
        ("order", order),
        ("events", events),
        ("timeout_ms", timeout),
        ("command", command),
        ("args", args),
    ] {
        assert_ne!(
            base,
            hook_definition_hash(&changed, &root).unwrap(),
            "{field} is not part of the hash"
        );
    }

    // The script's content is part of the definition, because review would
    // otherwise cover only the path to it.
    write(&check, "echo two\n");
    assert_ne!(base, hook_definition_hash(&hook, &root).unwrap());
    write(&check, "echo one\n");
    assert_eq!(base, hook_definition_hash(&hook, &root).unwrap());

    // A file outside the repository is not the repository's to change.
    let outside_root = temp_dir("definition-hash-outside");
    let outside = script(&outside_root, "outside", "one\n");
    let mut outside_hook = hook.clone();
    outside_hook.args = vec![outside.to_string_lossy().to_string()];
    let with_outside = hook_definition_hash(&outside_hook, &root).unwrap();
    write(&outside, "two\n");
    assert_eq!(
        with_outside,
        hook_definition_hash(&outside_hook, &root).unwrap()
    );

    let _ = fs::remove_dir_all(&outside_root);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_repository_hook_is_pending_until_reviewed_at_its_hash() {
    let root = temp_dir("activation");
    let check = script(&root, "check", "exit 0\n");
    let mut hook = sh_hook("repo_check", &check);
    hook.source = HookSource::Repository;
    hook.events = vec![HookEvent::BeforeTool];
    let repository_id = repository_id_for_root(&root);
    let mut config = config_for(&root, vec![hook.clone()]);

    assert_eq!(
        activation(&config, &hook, &repository_id, &root),
        HookActivation::PendingReview
    );

    let hash = hook_definition_hash(&hook, &root).unwrap();
    config.hook_reviews_by_repository.insert(
        repository_id.clone(),
        BTreeMap::from([(hook.id.clone(), HookReview::Declined)]),
    );
    assert_eq!(
        activation(&config, &hook, &repository_id, &root),
        HookActivation::Declined
    );
    config.hook_reviews_by_repository.insert(
        repository_id.clone(),
        BTreeMap::from([(hook.id.clone(), HookReview::Approved { hash: hash.clone() })]),
    );
    assert_eq!(
        activation(&config, &hook, &repository_id, &root),
        HookActivation::Active
    );

    write(&check, "exit 1\n");
    assert_eq!(
        activation(&config, &hook, &repository_id, &root),
        HookActivation::ChangedSinceReview
    );

    let mut disabled = hook.clone();
    disabled.enabled = false;
    assert_eq!(
        activation(&config, &disabled, &repository_id, &root),
        HookActivation::Disabled
    );

    let mut user = hook.clone();
    user.source = HookSource::User;
    assert_eq!(
        activation(&config, &user, &repository_id, &root),
        HookActivation::Active
    );

    // A relative command must resolve inside the repository. One that exists
    // outside it is not made acceptable by being canonicalisable.
    let outside_root = temp_dir("activation-outside");
    let _outside = script(&outside_root, "escape", "exit 0\n");
    let mut escaping = hook.clone();
    escaping.command = format!(
        "../{}/hooks/escape.sh",
        outside_root.file_name().unwrap().to_string_lossy()
    );
    assert_eq!(
        activation(&config, &escaping, &repository_id, &root),
        HookActivation::ChangedSinceReview
    );

    let _ = fs::remove_dir_all(&outside_root);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_policy_view_shows_each_hooks_activation() {
    let root = temp_dir("policy-activation");
    let check = script(&root, "check", "exit 0\n");
    let mut hook = sh_hook("repo_check", &check);
    hook.source = HookSource::Repository;
    let config = config_for(&root, vec![hook.clone()]);
    let report = RepositoryConfigReport {
        repository_root: Some(root.clone()),
        ..RepositoryConfigReport::default()
    };

    let policy = EffectivePolicy::from_load(&config, &report, None);
    for key in ["hook.repo_check.command", "hook.repo_check.events"] {
        assert_eq!(
            policy.rule(key).unwrap().activation,
            Some(HookActivation::PendingReview),
            "{key}"
        );
    }

    let hash = hook_definition_hash(&hook, &root).unwrap();
    let mut approved = config.clone();
    approved.hook_reviews_by_repository.insert(
        repository_id_for_root(&root),
        BTreeMap::from([(hook.id.clone(), HookReview::Approved { hash })]),
    );
    let policy = EffectivePolicy::from_load(&approved, &report, None);
    assert_eq!(
        policy.rule("hook.repo_check.command").unwrap().activation,
        Some(HookActivation::Active)
    );

    let _ = fs::remove_dir_all(&root);
}

fn deny_hook(root: &Path, id: &str, order: i64, marker: &str) -> HookConfig {
    let body = format!(
        "echo ran >> {marker}\n{}",
        json_response(r#"{"hookApiVersion":1,"verdict":"deny","reason":"no"}"#)
    );
    let mut hook = sh_hook(id, &script(root, id, &body));
    hook.order = order;
    hook
}

#[test]
fn a_deny_short_circuits_the_hooks_after_it() {
    let root = temp_dir("short-circuit");
    let first = deny_hook(&root, "first", 1, "first.log");
    let second = deny_hook(&root, "second", 2, "second.log");
    // Configured in the opposite order: the order field decides.
    let config = config_for(&root, vec![second, first]);
    let outcome = dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);

    assert_eq!(outcome.verdict, Verdict::Deny);
    assert_eq!(outcome.deciding_hook.as_deref(), Some("first"));
    assert_eq!(outcome.reasons.len(), 1, "{:?}", outcome.reasons);
    assert_eq!(outcome.reasons[0].0, "first");
    assert!(
        root.join("first.log").exists(),
        "the first hook did not run"
    );
    assert!(!root.join("second.log").exists(), "a hook after a deny ran");
    assert_eq!(events_of(&config.data_dir, "hook_invoked").len(), 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_mandatory_failure_denies_and_an_optional_failure_warns() {
    let cases = [
        ("non_zero_exit", Some("exit 3\n")),
        ("timeout", Some("sleep 30\n")),
        ("malformed", Some("printf 'not json'\n")),
        ("version_mismatch", Some(r#"{"hookApiVersion":2}"#)),
        (
            "output_too_large",
            Some("head -c 65537 /dev/zero | tr '\\0' x\n"),
        ),
        ("spawn_failed", None),
    ];
    for (kind, body) in cases {
        for mode in [HookMode::Mandatory, HookMode::Optional] {
            let root = temp_dir(&format!("failure-{kind}-{}", mode.as_str()));
            let mut hook = match body {
                Some(body) => {
                    let body = if body.starts_with('{') {
                        json_response(body)
                    } else {
                        body.to_string()
                    };
                    sh_hook(
                        &format!("{kind}_{}", mode.as_str()),
                        &script(&root, kind, &body),
                    )
                }
                None => {
                    let mut hook = hook(
                        &format!("{kind}_{}", mode.as_str()),
                        0,
                        &[HookEvent::BeforeTool],
                        true,
                    );
                    hook.command = "/nonexistent/damaian-hook".to_string();
                    hook
                }
            };
            hook.mode = mode;
            if kind == "timeout" {
                hook.timeout_ms = Some(300);
            }
            let config = config_for(&root, vec![hook]);
            let outcome = dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);

            let expected = match mode {
                HookMode::Mandatory => Verdict::Deny,
                HookMode::Optional => Verdict::Warn,
            };
            assert_eq!(outcome.verdict, expected, "{kind} {mode:?}");
            let invoked = events_of(&config.data_dir, "hook_invoked");
            assert_eq!(invoked.len(), 1, "{kind} {mode:?}");
            assert_eq!(invoked[0]["failureKind"], kind, "{kind} {mode:?}");
            assert_eq!(invoked[0]["outcome"], "failed", "{kind}");
            if mode == HookMode::Mandatory {
                assert_eq!(outcome.reasons.len(), 1, "{kind}");
                assert_eq!(
                    outcome.reasons[0].1,
                    format!("hook `{kind}_mandatory` failed: {kind}"),
                    "{kind}"
                );
                assert!(outcome.warnings.is_empty(), "{kind}");
            } else {
                assert!(outcome.reasons.is_empty(), "{kind}");
                assert!(
                    outcome
                        .warnings
                        .iter()
                        .any(|warning| warning.contains(kind)),
                    "{kind}: {:?}",
                    outcome.warnings
                );
                assert!(
                    outcome.findings.iter().any(|finding| {
                        finding.source() == FindingSource::Hook
                            && finding.severity() == Severity::Warning
                    }),
                    "{kind}: no warning finding"
                );
            }

            let _ = fs::remove_dir_all(&root);
        }
    }
}

#[test]
fn a_hook_with_no_mode_fails_closed() {
    let root = temp_dir("default-mode");
    let mut hook = sh_hook("defaulted", &script(&root, "defaulted", "exit 1\n"));
    hook.mode = HookMode::default();
    assert_eq!(hook.mode, HookMode::Mandatory);
    let config = config_for(&root, vec![hook]);
    let outcome = dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);
    assert_eq!(outcome.verdict, Verdict::Deny);
    assert_eq!(
        outcome.reasons[0].1,
        "hook `defaulted` failed: non_zero_exit"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn an_unknown_verdict_is_audited_as_a_widening_attempt_and_denies() {
    let root = temp_dir("widening");
    let hook = sh_hook(
        "approver",
        &script(
            &root,
            "approve",
            &json_response(
                r#"{"hookApiVersion":1,"verdict":"approve","grant":"GRANT_VALUE_TEXT","reason":"SECRET_REASON_TEXT"}"#,
            ),
        ),
    );
    let config = config_for(&root, vec![hook]);
    let outcome = dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);

    assert_eq!(outcome.verdict, Verdict::Deny);
    assert_eq!(outcome.reasons[0].1, "hook `approver` failed: malformed");
    let widening = events_of(&config.data_dir, "hook_widening_attempt");
    assert_eq!(widening.len(), 2, "{widening:?}");
    let field = widening
        .iter()
        .find(|event| event.get("field").is_some())
        .expect("the unknown field was not audited");
    assert_eq!(field["field"], "grant");
    assert!(field.get("verdict").is_none(), "{field:?}");
    let verdict = widening
        .iter()
        .find(|event| event.get("verdict").is_some())
        .expect("the unknown verdict was not audited");
    assert_eq!(verdict["verdict"], "approve");
    assert!(verdict.get("field").is_none(), "{verdict:?}");
    let invoked = events_of(&config.data_dir, "hook_invoked");
    assert_eq!(invoked.len(), 1);
    assert_eq!(invoked[0]["failureKind"], "malformed");

    // Attempts are audited by name, never by the value the hook put there.
    let audit = fs::read_to_string(config.data_dir.join("audit/events.jsonl")).unwrap();
    for value in ["SECRET_REASON_TEXT", "GRANT_VALUE_TEXT"] {
        assert!(
            !audit.contains(value),
            "{value} reached the audit log: {audit}"
        );
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn hook_output_is_redacted_before_the_outcome_and_the_audit() {
    let root = temp_dir("redaction");
    let body = r#"key=AKIAIOSFODNN7EXAMPLE
echo "$key" >&2
cat <<JSON
{"hookApiVersion":1,"verdict":"deny","reason":"reason $key","findings":[{"severity":"error","summary":"summary $key","details":"details $key"}],"context":["context $key"]}
JSON
"#;
    let hook = sh_hook("leaky", &script(&root, "leaky", body));
    let mut hook = hook;
    hook.events = vec![HookEvent::BeforeContext];
    let config = config_for(&root, vec![hook]);
    let outcome = dispatch(&dispatcher_for(&config), HookEvent::BeforeContext, &root);

    const KEY: &str = "AKIAIOSFODNN7EXAMPLE";
    assert_eq!(outcome.verdict, Verdict::Deny);
    for text in outcome
        .reasons
        .iter()
        .map(|(_, reason)| reason.clone())
        .chain(outcome.warnings.iter().cloned())
        .chain(outcome.context.iter().map(|(_, context)| context.clone()))
    {
        assert!(!text.contains(KEY), "a string kept the secret: {text}");
    }
    for finding in &outcome.findings {
        assert!(!finding.summary().contains(KEY), "{}", finding.summary());
        assert!(
            !finding.details().unwrap_or_default().contains(KEY),
            "{:?}",
            finding.details()
        );
    }
    assert!(!outcome.reasons[0].1.is_empty());
    let audit = fs::read_to_string(config.data_dir.join("audit/events.jsonl")).unwrap();
    assert!(
        !audit.contains(KEY),
        "the audit log kept the secret: {audit}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_repository_hook_runs_only_after_review_at_its_hash() {
    let root = temp_dir("review-gate");
    let check = script(
        &root,
        "check",
        &format!(
            "echo approved >> runs.log\n{}",
            json_response(r#"{"hookApiVersion":1,"verdict":"allow"}"#)
        ),
    );
    let mut hook = sh_hook("repo_check", &check);
    hook.source = HookSource::Repository;
    hook.events = vec![HookEvent::BeforeTool];
    let mut config = config_for(&root, vec![hook.clone()]);
    let runs = root.join("runs.log");

    // Pending: not run, not audited.
    assert_eq!(
        dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root).verdict,
        Verdict::Allow
    );
    assert!(!runs.exists(), "a pending hook ran");
    assert!(events_of(&config.data_dir, "hook_invoked").is_empty());

    // Approved at this hash: it runs.
    let hash = hook_definition_hash(&hook, &root).unwrap();
    config.hook_reviews_by_repository.insert(
        repository_id_for_root(&root),
        BTreeMap::from([(hook.id.clone(), HookReview::Approved { hash })]),
    );
    assert_eq!(
        dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root).verdict,
        Verdict::Allow
    );
    assert_eq!(fs::read_to_string(&runs).unwrap(), "approved\n");

    // The script changed after review: it must not run, and its mandatory
    // classification turns the change into a denial.
    write(
        &check,
        &format!(
            "echo changed >> runs.log\n{}",
            json_response(r#"{"hookApiVersion":1,"verdict":"allow"}"#)
        ),
    );
    let outcome = dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);
    assert_eq!(outcome.verdict, Verdict::Deny);
    assert!(
        outcome.reasons[0].1.contains("changed_since_review"),
        "{:?}",
        outcome.reasons
    );
    assert_eq!(
        fs::read_to_string(&runs).unwrap(),
        "approved\n",
        "the changed script ran"
    );
    let invoked = events_of(&config.data_dir, "hook_invoked");
    assert_eq!(invoked.len(), 2);
    assert_eq!(invoked[1]["failureKind"], "changed_since_review");

    // Declined: not run, not audited, whichever definition is on disk.
    config.hook_reviews_by_repository.insert(
        repository_id_for_root(&root),
        BTreeMap::from([(hook.id.clone(), HookReview::Declined)]),
    );
    assert_eq!(
        dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root).verdict,
        Verdict::Allow
    );
    assert_eq!(fs::read_to_string(&runs).unwrap(), "approved\n");
    assert_eq!(events_of(&config.data_dir, "hook_invoked").len(), 2);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn every_invocation_writes_one_hook_invoked_event() {
    let root = temp_dir("audit-invocation");
    let allow = sh_hook(
        "quiet",
        &script(
            &root,
            "quiet",
            &json_response(r#"{"hookApiVersion":1,"verdict":"allow"}"#),
        ),
    );
    let config = config_for(&root, vec![allow]);
    dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);
    let invoked = events_of(&config.data_dir, "hook_invoked");
    assert_eq!(invoked.len(), 1);
    let event = &invoked[0];
    assert_eq!(event["hookId"], "quiet");
    assert_eq!(event["event"], "before_tool");
    assert_eq!(event["sessionId"], "session_1");
    assert_eq!(event["taskId"], "task_1");
    assert_eq!(event["classification"], "mandatory");
    assert_eq!(event["source"], "user");
    assert_eq!(event["verdict"], "allow");
    assert_eq!(event["outcome"], "ok");
    assert!(event.get("failureKind").is_none(), "{event}");
    assert!(event.get("reason").is_none(), "{event}");
    assert!(event["durationMs"].as_str().unwrap().parse::<u64>().is_ok());
    let _ = fs::remove_dir_all(&root);

    // A denial records its reason, and an optional classification shows in
    // the same event.
    let root = temp_dir("audit-denial");
    let mut deny = sh_hook(
        "refuser",
        &script(
            &root,
            "refuser",
            &json_response(r#"{"hookApiVersion":1,"verdict":"deny","reason":"not here"}"#),
        ),
    );
    deny.mode = HookMode::Optional;
    let config = config_for(&root, vec![deny]);
    dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);
    let invoked = events_of(&config.data_dir, "hook_invoked");
    assert_eq!(invoked.len(), 1);
    assert_eq!(invoked[0]["classification"], "optional");
    assert_eq!(invoked[0]["verdict"], "deny");
    assert_eq!(invoked[0]["outcome"], "ok");
    assert_eq!(invoked[0]["reason"], "not here");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_dispatch_with_no_hooks_spawns_nothing_and_writes_no_audit() {
    let root = temp_dir("no-hooks");
    let data_dir = root.join(".damaian-data");
    let engine = WorkspaceEngine::new(config_for(&root, Vec::new()));
    let outcome = engine.hooks.dispatch(
        HookEvent::BeforeTool,
        event_parts(),
        &root,
        &HookContext::root(),
    );
    assert_eq!(outcome.verdict, Verdict::Allow);
    assert!(outcome.findings.is_empty());
    assert!(
        !data_dir.join("audit").exists(),
        "an empty dispatch audited"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_hook_for_another_event_and_an_excluded_hook_do_not_run() {
    let root = temp_dir("not-this-event");
    let marker = |id: &str| {
        let body = format!(
            "echo ran >> {id}.log\n{}",
            json_response(r#"{"hookApiVersion":1,"verdict":"allow"}"#)
        );
        sh_hook(id, &script(&root, id, &body))
    };
    let mut patch_hook = marker("patch_only");
    patch_hook.events = vec![HookEvent::BeforePatchApply];
    let tool_hook = marker("tool_hook");
    let config = config_for(&root, vec![patch_hook, tool_hook]);

    dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);
    assert!(!root.join("patch_only.log").exists());
    assert!(root.join("tool_hook.log").exists());
    assert_eq!(events_of(&config.data_dir, "hook_invoked").len(), 1);

    fs::remove_file(root.join("tool_hook.log")).unwrap();
    let dispatcher = dispatcher_for(&config);
    let ctx = HookContext {
        depth: 0,
        excluded: BTreeSet::from(["tool_hook".to_string()]),
    };
    let outcome = dispatcher.dispatch(HookEvent::BeforeTool, event_parts(), &root, &ctx);
    assert_eq!(outcome.verdict, Verdict::Allow);
    assert!(!root.join("tool_hook.log").exists(), "an excluded hook ran");
    assert_eq!(events_of(&config.data_dir, "hook_invoked").len(), 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn context_is_kept_only_at_the_events_that_use_it() {
    let root = temp_dir("context-events");
    let mut hook = sh_hook(
        "contextual",
        &script(
            &root,
            "contextual",
            &json_response(r#"{"hookApiVersion":1,"context":["a note"]}"#),
        ),
    );
    hook.events = vec![HookEvent::BeforeContext, HookEvent::BeforeTool];
    let config = config_for(&root, vec![hook]);
    let dispatcher = dispatcher_for(&config);

    let before_context = dispatch(&dispatcher, HookEvent::BeforeContext, &root);
    assert_eq!(
        before_context.context,
        [("contextual".to_string(), "a note".to_string())]
    );
    assert!(before_context.warnings.is_empty());

    let before_tool = dispatch(&dispatcher, HookEvent::BeforeTool, &root);
    assert!(before_tool.context.is_empty(), "context was not honoured");
    assert_eq!(before_tool.warnings.len(), 1, "{:?}", before_tool.warnings);
    assert!(before_tool.warnings[0].contains("context"));
    assert!(
        before_tool
            .findings
            .iter()
            .any(|finding| finding.source() == FindingSource::Hook
                && finding.severity() == Severity::Warning)
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_warn_is_a_warning_finding_and_request_approval_keeps_its_reason() {
    let root = temp_dir("warn-request");
    let warn = sh_hook(
        "warned",
        &script(
            &root,
            "warned",
            &json_response(r#"{"hookApiVersion":1,"verdict":"warn","reason":"careful"}"#),
        ),
    );
    let config = config_for(&root, vec![warn]);
    let outcome = dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);
    assert_eq!(outcome.verdict, Verdict::Warn);
    assert_eq!(outcome.deciding_hook.as_deref(), Some("warned"));
    assert_eq!(outcome.warnings, ["careful"]);
    assert!(outcome.reasons.is_empty());
    assert!(
        outcome
            .findings
            .iter()
            .any(|finding| finding.source() == FindingSource::Hook
                && finding.severity() == Severity::Warning
                && finding.summary() == "careful"
                && finding.origin_ref() == Some("hook:warned"))
    );
    let _ = fs::remove_dir_all(&root);

    let root = temp_dir("request-approval");
    let request = sh_hook(
        "asks",
        &script(
            &root,
            "asks",
            &json_response(
                r#"{"hookApiVersion":1,"verdict":"request_approval","reason":"ask me"}"#,
            ),
        ),
    );
    let config = config_for(&root, vec![request]);
    let outcome = dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);
    assert_eq!(outcome.verdict, Verdict::RequestApproval);
    assert_eq!(outcome.deciding_hook.as_deref(), Some("asks"));
    assert_eq!(
        outcome.reasons,
        [("asks".to_string(), "ask me".to_string())]
    );
    assert!(outcome.warnings.is_empty());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_hook_finding_keeps_a_path_only_when_it_stays_in_the_repository() {
    let root = temp_dir("finding-path");
    let hook = sh_hook(
        "paths",
        &script(
            &root,
            "paths",
            &json_response(
                r#"{"hookApiVersion":1,"findings":[
                    {"severity":"warning","summary":"inside","path":"src/lib.rs","line":3},
                    {"severity":"warning","summary":"outside","path":"../outside.rs","line":1},
                    {"severity":"warning","summary":"absolute","path":"/etc/passwd","line":2}
                ]}"#,
            ),
        ),
    );
    let config = config_for(&root, vec![hook]);
    let outcome = dispatch(&dispatcher_for(&config), HookEvent::BeforeTool, &root);
    let paths: Vec<Option<String>> = outcome
        .findings
        .iter()
        .map(|finding| finding.range().map(|range| range.path.clone()))
        .collect();
    assert_eq!(
        paths,
        [Some("src/lib.rs".to_string()), None, None],
        "{:?}",
        outcome.findings
    );
    assert!(
        outcome
            .findings
            .iter()
            .all(|finding| finding.source() == FindingSource::Hook
                && finding.origin_ref() == Some("hook:paths"))
    );

    let _ = fs::remove_dir_all(&root);
}

// ---- Task 5: requested commands and recursion. `context.md` §9 -------------

const REQUESTED: &str = "cargo test";

/// A config whose commands run `/usr/bin/true` rather than a login shell,
/// with `cargo test` allowlisted so it runs without approval in Code mode
/// (Global Constraints: never spawn a real login shell).
fn command_config(root: &Path, hooks: Vec<HookConfig>) -> Config {
    let mut config = config_for(root, hooks);
    config.shell = "/usr/bin/true".to_string();
    config.command_allowlist = vec![REQUESTED.to_string()];
    config
}

fn session_and_task(config: &Config, root: &Path) -> (Session, Task) {
    let store = SessionStore::new(&config.data_dir);
    let session = store
        .create_session(&repository_id_for_root(root), "hooks")
        .unwrap();
    let task = store
        .create_task(&session.id, "prompt", "provider", "model")
        .unwrap();
    (session, task)
}

/// A hook that appends its invocation to `<id>.log` (in the repository root,
/// which is the hook's cwd) and asks for `command`.
fn requesting_hook(root: &Path, id: &str, command: &str) -> HookConfig {
    let body = format!(
        "echo ran >> {id}.log\n{}",
        json_response(&format!(
            r#"{{"hookApiVersion":1,"commands":["{command}"]}}"#
        ))
    );
    sh_hook(id, &script(root, id, &body))
}

fn run_requested(
    dispatcher: &HookDispatcher,
    outcome: &mut HookOutcome,
    session: &Session,
    task: Option<&Task>,
    mode: SessionMode,
    root: &Path,
) -> Vec<workspace_engine::finding::Finding> {
    dispatcher.run_requested_commands(outcome, session, task, mode, root, &HookContext::root())
}

fn session_events(config: &Config, session_id: &str) -> Vec<serde_json::Value> {
    let path = config
        .data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn actions(config: &Config, session_id: &str, event_type: &str) -> Vec<serde_json::Value> {
    session_events(config, session_id)
        .into_iter()
        .filter(|event| event["eventType"] == event_type)
        .collect()
}

fn pending_proposals(config: &Config) -> usize {
    let Ok(entries) = fs::read_dir(config.data_dir.join("commands").join("pending")) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "dcmd")
        })
        .count()
}

#[test]
fn a_requested_command_runs_under_the_sessions_mode_and_is_bracketed_by_a_marker() {
    let root = temp_dir("requested-runs");
    let config = command_config(&root, vec![requesting_hook(&root, "asks", REQUESTED)]);
    let dispatcher = dispatcher_for(&config);
    let (session, task) = session_and_task(&config, &root);
    let mut outcome = dispatch(&dispatcher, HookEvent::BeforeTool, &root);
    assert_eq!(outcome.commands.len(), 1, "{:?}", outcome.commands);

    let findings = run_requested(
        &dispatcher,
        &mut outcome,
        &session,
        Some(&task),
        SessionMode::Code,
        &root,
    );

    // Taking the commands does not change the hook's verdict (context.md §9).
    assert_eq!(outcome.verdict, Verdict::Allow);
    assert!(outcome.commands.is_empty(), "{:?}", outcome.commands);
    assert!(findings.is_empty(), "{findings:?}");
    assert_eq!(
        pending_proposals(&config),
        1,
        "the command was not proposed"
    );
    assert_eq!(
        events_of(&config.data_dir, "stored_command_executed").len(),
        1
    );
    assert_eq!(events_of(&config.data_dir, "hook_command_refused").len(), 0);

    let started: Vec<_> = actions(&config, &session.id, "action_started")
        .into_iter()
        .filter(|event| event["payload"]["action"] == "hook_command")
        .collect();
    let finished: Vec<_> = actions(&config, &session.id, "action_finished")
        .into_iter()
        .filter(|event| event["payload"]["action"] == "hook_command")
        .collect();
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert_eq!(
        started[0]["payload"]["markerId"],
        finished[0]["payload"]["markerId"]
    );
    assert_eq!(finished[0]["payload"]["outcome"], "ok");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_requested_command_is_refused_in_ask_mode() {
    let root = temp_dir("requested-ask");
    let config = command_config(&root, vec![requesting_hook(&root, "asks", REQUESTED)]);
    let dispatcher = dispatcher_for(&config);
    let (session, task) = session_and_task(&config, &root);
    let mut outcome = dispatch(&dispatcher, HookEvent::BeforeTool, &root);

    let findings = run_requested(
        &dispatcher,
        &mut outcome,
        &session,
        Some(&task),
        SessionMode::Ask,
        &root,
    );

    assert!(
        findings
            .iter()
            .any(|finding| finding.source() == FindingSource::Hook
                && finding.severity() == Severity::Warning
                && finding.summary().contains("asks")),
        "{findings:?}"
    );
    let refused = events_of(&config.data_dir, "hook_command_refused");
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0]["hookId"], "asks");
    assert_eq!(refused[0]["command"], REQUESTED);
    assert_eq!(
        pending_proposals(&config),
        0,
        "a refused command was stored"
    );
    assert!(
        actions(&config, &session.id, "action_started")
            .iter()
            .all(|event| event["payload"]["action"] != "hook_command")
    );
    assert_eq!(
        events_of(&config.data_dir, "stored_command_executed").len(),
        0
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_requested_command_that_needs_approval_is_refused_without_a_card_or_proposal() {
    let root = temp_dir("requested-approval");
    let mut config = command_config(&root, vec![requesting_hook(&root, "asks", REQUESTED)]);
    config.require_approval_for_all_commands = true;
    let dispatcher = dispatcher_for(&config);
    let (session, task) = session_and_task(&config, &root);
    let mut outcome = dispatch(&dispatcher, HookEvent::BeforeTool, &root);

    let findings = run_requested(
        &dispatcher,
        &mut outcome,
        &session,
        Some(&task),
        SessionMode::Code,
        &root,
    );

    assert!(!findings.is_empty(), "a refused command made no finding");
    let refused = events_of(&config.data_dir, "hook_command_refused");
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(
        refused[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("approval")),
        "{}",
        refused[0]
    );
    // No card (a pending proposal) and no marker: a hook never asks.
    assert_eq!(pending_proposals(&config), 0);
    assert!(
        actions(&config, &session.id, "action_started")
            .iter()
            .all(|event| event["payload"]["action"] != "hook_command")
    );
    assert_eq!(
        events_of(&config.data_dir, "stored_command_executed").len(),
        0
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_requested_command_does_not_reenter_the_hook_that_asked() {
    let root = temp_dir("requested-no-reentry");
    let config = command_config(&root, vec![requesting_hook(&root, "asks", REQUESTED)]);
    let dispatcher = dispatcher_for(&config);
    let (session, task) = session_and_task(&config, &root);
    let mut outcome = dispatch(&dispatcher, HookEvent::BeforeTool, &root);

    run_requested(
        &dispatcher,
        &mut outcome,
        &session,
        Some(&task),
        SessionMode::Code,
        &root,
    );

    let invocations = fs::read_to_string(root.join("asks.log")).unwrap();
    assert_eq!(
        invocations.lines().count(),
        1,
        "the hook saw its own command: {invocations}"
    );
    assert_eq!(events_of(&config.data_dir, "hook_invoked").len(), 1);
    assert_eq!(
        events_of(&config.data_dir, "stored_command_executed").len(),
        1
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_hook_that_denies_a_requested_command_stops_it() {
    let root = temp_dir("requested-denied");
    let asks = requesting_hook(&root, "asks", REQUESTED);
    // Denies only the nested dispatch, which is the one with depth 1.
    let guard = sh_hook(
        "guard",
        &script(
            &root,
            "guard",
            r#"input=$(cat)
case "$input" in
  *'"hookDepth":1'*)
    printf '{"hookApiVersion":1,"verdict":"deny","reason":"not from a hook"}\n'
    ;;
  *)
    printf '{"hookApiVersion":1}\n'
    ;;
esac
"#,
        ),
    );
    let config = command_config(&root, vec![asks, guard]);
    let dispatcher = dispatcher_for(&config);
    let (session, task) = session_and_task(&config, &root);
    let mut outcome = dispatch(&dispatcher, HookEvent::BeforeTool, &root);
    assert_eq!(outcome.commands.len(), 1, "{:?}", outcome.commands);

    let findings = run_requested(
        &dispatcher,
        &mut outcome,
        &session,
        Some(&task),
        SessionMode::Code,
        &root,
    );

    assert!(
        findings
            .iter()
            .any(|finding| finding.summary().contains("guard")),
        "{findings:?}"
    );
    assert_eq!(
        events_of(&config.data_dir, "stored_command_executed").len(),
        0
    );
    assert_eq!(pending_proposals(&config), 0);
    assert_eq!(events_of(&config.data_dir, "hook_command_refused").len(), 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn recursion_stops_at_the_depth_limit_and_the_nested_command_still_runs() {
    let root = temp_dir("requested-recursion");
    let alpha = requesting_hook(&root, "alpha", REQUESTED);
    let beta = requesting_hook(&root, "beta", REQUESTED);
    let config = command_config(&root, vec![alpha, beta]);
    let dispatcher = dispatcher_for(&config);
    let (session, task) = session_and_task(&config, &root);
    let mut outcome = dispatch(&dispatcher, HookEvent::BeforeTool, &root);
    assert_eq!(outcome.commands.len(), 2, "{:?}", outcome.commands);

    let findings = run_requested(
        &dispatcher,
        &mut outcome,
        &session,
        Some(&task),
        SessionMode::Code,
        &root,
    );

    assert!(outcome.depth_exceeded, "the outer outcome was not marked");
    assert!(
        findings
            .iter()
            .any(|finding| finding.summary().contains("depth")),
        "{findings:?}"
    );
    let exceeded = events_of(&config.data_dir, "hook_depth_exceeded");
    assert_eq!(exceeded.len(), 2, "{exceeded:?}");
    for event in &exceeded {
        assert!(
            event["hookId"] == "alpha" || event["hookId"] == "beta",
            "{event}"
        );
        assert_eq!(event["event"], "before_tool");
        assert_eq!(event["depth"], (MAX_HOOK_DEPTH + 1).to_string());
    }
    // The nested action is not blocked (proposal §5.7): every command still
    // ran, and every `hook_command` marker was finished.
    let executed = events_of(&config.data_dir, "stored_command_executed");
    assert_eq!(executed.len(), 6, "{executed:?}");
    let started: Vec<_> = actions(&config, &session.id, "action_started")
        .into_iter()
        .filter(|event| event["payload"]["action"] == "hook_command")
        .collect();
    let finished: Vec<_> = actions(&config, &session.id, "action_finished")
        .into_iter()
        .filter(|event| event["payload"]["action"] == "hook_command")
        .collect();
    assert_eq!(started.len(), 6);
    assert_eq!(finished.len(), 6);
    // Each hook ran at the root, at depth 1 and at depth 2, and no further.
    for id in ["alpha", "beta"] {
        let count = fs::read_to_string(root.join(format!("{id}.log")))
            .unwrap()
            .lines()
            .count();
        assert_eq!(count, 3, "{id} ran {count} times");
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn with_no_task_every_requested_command_is_refused() {
    let root = temp_dir("requested-no-task");
    let config = command_config(&root, vec![requesting_hook(&root, "asks", REQUESTED)]);
    let dispatcher = dispatcher_for(&config);
    let (session, _) = session_and_task(&config, &root);
    let mut outcome = dispatch(&dispatcher, HookEvent::BeforeTool, &root);

    let findings = run_requested(
        &dispatcher,
        &mut outcome,
        &session,
        None,
        SessionMode::Code,
        &root,
    );

    assert!(!findings.is_empty(), "a refused command made no finding");
    let refused = events_of(&config.data_dir, "hook_command_refused");
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(
        refused[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("task")),
        "{}",
        refused[0]
    );
    assert_eq!(
        events_of(&config.data_dir, "stored_command_executed").len(),
        0
    );

    let _ = fs::remove_dir_all(&root);
}

// ---- Task 6: attachment outside chat.rs. `context.md` §3, §4, §10 ----------

/// A hook answering with `body` on `events`, in `mode`.
fn answering_hook(
    root: &Path,
    id: &str,
    events: &[HookEvent],
    mode: HookMode,
    body: &str,
) -> HookConfig {
    let mut hook = sh_hook(id, &script(root, id, &json_response(body)));
    hook.events = events.to_vec();
    hook.mode = mode;
    hook
}

/// An engine whose data directory and PID registry stay inside `root`, so a
/// fixture never writes into the user's own state.
fn engine_for(root: &Path, hooks: Vec<HookConfig>) -> (Config, WorkspaceEngine) {
    let config = config_for(root, hooks);
    (config.clone(), WorkspaceEngine::new(config))
}

/// Writes `before` at `path`, creates the patch that replaces it with `after`,
/// attaches the patch to `session`/`task`, and stores it, so a test can apply a
/// patch the way the shell does.
fn stage_patch(
    engine: &WorkspaceEngine,
    root: &Path,
    path: &str,
    before: &str,
    after: &str,
    session: Option<(&Session, &Task)>,
) -> workspace_engine::ProposedPatch {
    write(&root.join(path), before);
    let mut patch = engine
        .patch_engine
        .create_patch(
            root,
            &[workspace_engine::ProposedChange {
                path: path.to_string(),
                new_content: after.to_string(),
                status: None,
                allow_restricted: false,
            }],
            session.map(|(_, task)| task.id.as_str()),
            "hook test",
        )
        .expect("the fixture patch is created");
    patch.session_id = session
        .map(|(session, _)| session.id.clone())
        .unwrap_or_default();
    patch.origin_session_id = String::new();
    engine
        .patch_store
        .save(&patch)
        .expect("the patch is stored");
    patch
}

fn apply_markers(config: &Config, session_id: &str) -> Vec<serde_json::Value> {
    actions(config, session_id, "action_started")
        .into_iter()
        .filter(|event| event["payload"]["action"] == "apply_patch")
        .collect()
}

const EDIT_REPLY: &str = "DAMAIAN_EDIT_V1\nSUMMARY: Add a\nFILE: a.txt\nSTATUS: added\nCONTENT:\nhello\nEND_FILE\nEND_PATCH\n";

/// Criterion 1, engine half: a denying `before_patch_apply` leaves the file
/// alone, opens no marker, and its reason reaches the caller.
#[test]
fn a_before_patch_apply_deny_blocks_the_apply_and_names_the_hook() {
    let root = temp_dir("apply-deny");
    let hook = answering_hook(
        &root,
        "no_migrations",
        &[HookEvent::BeforePatchApply],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"verdict":"deny","reason":"migrations/ is applied by the release process"}"#,
    );
    let (config, engine) = engine_for(&root, vec![hook]);
    let store = SessionStore::new(&config.data_dir);
    let session = store
        .create_session(&repository_id_for_root(&root), "apply")
        .unwrap();
    let task = store
        .create_task(&session.id, "prompt", "provider", "model")
        .unwrap();
    let patch = stage_patch(
        &engine,
        &root,
        "migrations/003.sql",
        "SELECT 1;\n",
        "SELECT 2;\n",
        Some((&session, &task)),
    );

    let error = engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.id, None, None, "tester", false)
        .expect_err("a denying hook must block the apply");

    let message = match &error {
        workspace_engine::ClientError::PolicyBlocked(message) => message,
        other => panic!("expected a policy block, got {other:?}"),
    };
    assert!(message.contains("no_migrations"), "{message}");
    assert!(message.contains("release process"), "{message}");
    assert_eq!(
        fs::read_to_string(root.join("migrations/003.sql")).unwrap(),
        "SELECT 1;\n",
        "a blocked apply wrote the file"
    );
    assert!(
        apply_markers(&config, &session.id).is_empty(),
        "a blocked apply opened a marker"
    );
    assert!(
        events_of(&config.data_dir, "stored_patch_applied").is_empty(),
        "a blocked apply was audited as applied"
    );

    let _ = fs::remove_dir_all(&root);
}

/// A mandatory failure blocks and an optional one only warns (proposal §5.5),
/// at the apply as everywhere else.
#[test]
fn a_timing_out_before_patch_apply_hook_blocks_or_warns_by_its_mode() {
    for (mode, blocks) in [(HookMode::Mandatory, true), (HookMode::Optional, false)] {
        let root = temp_dir(&format!("apply-timeout-{}", mode.as_str()));
        let mut hook = sh_hook("slow", &script(&root, "slow", "sleep 30\n"));
        hook.events = vec![HookEvent::BeforePatchApply];
        hook.mode = mode;
        // Well under the 10 s default, so the case is quick either way.
        hook.timeout_ms = Some(200);
        let (config, engine) = engine_for(&root, vec![hook]);
        let store = SessionStore::new(&config.data_dir);
        let session = store
            .create_session(&repository_id_for_root(&root), "timeout")
            .unwrap();
        let task = store
            .create_task(&session.id, "prompt", "provider", "model")
            .unwrap();
        let patch = stage_patch(
            &engine,
            &root,
            "src/a.txt",
            "before\n",
            "after\n",
            Some((&session, &task)),
        );

        let result = engine
            .edit_orchestrator
            .apply_stored_patch(&root, &patch.id, None, None, "tester", false);

        if blocks {
            let error = result.expect_err("a mandatory failure must block");
            assert!(
                matches!(&error, workspace_engine::ClientError::PolicyBlocked(message) if message.contains("slow") && message.contains("timeout")),
                "{error:?}"
            );
            assert_eq!(
                fs::read_to_string(root.join("src/a.txt")).unwrap(),
                "before\n"
            );
        } else {
            let result = result.expect("an optional failure must let the apply proceed");
            assert_eq!(result.applied_files, vec!["src/a.txt"]);
            assert!(
                result
                    .hook_warnings
                    .iter()
                    .any(|warning| warning.contains("timeout")),
                "{:?}",
                result.hook_warnings
            );
            assert_eq!(
                fs::read_to_string(root.join("src/a.txt")).unwrap(),
                "after\n"
            );
        }

        let _ = fs::remove_dir_all(&root);
    }
}

/// The action has happened: an `after_patch_apply` denial is an error finding
/// and a warning, and nothing is rolled back (`context.md` §4).
#[test]
fn an_after_patch_apply_deny_keeps_the_content_and_records_an_error_finding() {
    let root = temp_dir("apply-after-deny");
    let hook = answering_hook(
        &root,
        "after_notes",
        &[HookEvent::AfterPatchApply],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"verdict":"deny","reason":"the release notes were not updated"}"#,
    );
    let (config, engine) = engine_for(&root, vec![hook]);
    let store = SessionStore::new(&config.data_dir);
    let session = store
        .create_session(&repository_id_for_root(&root), "after")
        .unwrap();
    let task = store
        .create_task(&session.id, "prompt", "provider", "model")
        .unwrap();
    let patch = stage_patch(
        &engine,
        &root,
        "src/a.txt",
        "before\n",
        "after\n",
        Some((&session, &task)),
    );

    let result = engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.id, None, None, "tester", false)
        .expect("an after hook cannot undo the apply");

    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "after\n",
        "an after deny rolled the patch back"
    );
    assert!(
        result
            .hook_warnings
            .iter()
            .any(|warning| warning.contains("release notes")),
        "{:?}",
        result.hook_warnings
    );
    let findings = store.read_findings(&session.id, &root).unwrap();
    let error = findings
        .iter()
        .find(|finding| {
            finding.source() == FindingSource::Hook && finding.severity() == Severity::Error
        })
        .unwrap_or_else(|| panic!("no error finding in {findings:?}"));
    assert!(
        error.summary().contains("release notes"),
        "{}",
        error.summary()
    );
    assert_eq!(error.origin_ref(), Some("hook:after_notes"));

    let _ = fs::remove_dir_all(&root);
}

/// An apply is always the user's own action, so `request_approval` is already
/// satisfied: the apply proceeds and the reason is a warning on the result.
#[test]
fn request_approval_at_before_patch_apply_applies_with_a_warning() {
    let root = temp_dir("apply-request-approval");
    let hook = answering_hook(
        &root,
        "review_migration",
        &[HookEvent::BeforePatchApply],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"verdict":"request_approval","reason":"look at the migration by hand"}"#,
    );
    let (config, engine) = engine_for(&root, vec![hook]);
    let store = SessionStore::new(&config.data_dir);
    let session = store
        .create_session(&repository_id_for_root(&root), "approve")
        .unwrap();
    let task = store
        .create_task(&session.id, "prompt", "provider", "model")
        .unwrap();
    let patch = stage_patch(
        &engine,
        &root,
        "migrations/003.sql",
        "SELECT 1;\n",
        "SELECT 2;\n",
        Some((&session, &task)),
    );

    let result = engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.id, None, None, "tester", false)
        .expect("request_approval does not block an apply");

    assert_eq!(result.applied_files, vec!["migrations/003.sql"]);
    assert!(
        result.hook_warnings.iter().any(|warning| {
            warning.contains("review_migration") && warning.contains("look at the migration")
        }),
        "{:?}",
        result.hook_warnings
    );

    let _ = fs::remove_dir_all(&root);
}

/// `context.md` §9: a requested command at the apply runs through Task 5 with
/// the patch's own task, not with no task.
#[test]
fn a_before_patch_apply_hooks_requested_command_runs_with_the_patch_task() {
    let root = temp_dir("apply-requested-command");
    let body = format!(
        "echo ran >> command.log\n{}",
        json_response(r#"{"hookApiVersion":1,"commands":["cargo test"]}"#)
    );
    let mut hook = sh_hook("asks", &script(&root, "asks", &body));
    hook.events = vec![HookEvent::BeforePatchApply];
    // No real login shell (Global Constraints), and `cargo test` allowlisted so
    // it runs unasked in Code mode.
    let mut config = config_for(&root, vec![hook]);
    config.shell = "/usr/bin/true".to_string();
    config.command_allowlist = vec![REQUESTED.to_string()];
    let engine = WorkspaceEngine::new(config.clone());
    let store = SessionStore::new(&config.data_dir);
    let session = store
        .create_session(&repository_id_for_root(&root), "commands")
        .unwrap();
    let task = store
        .create_task(&session.id, "prompt", "provider", "model")
        .unwrap();
    let patch = stage_patch(
        &engine,
        &root,
        "src/a.txt",
        "before\n",
        "after\n",
        Some((&session, &task)),
    );

    engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.id, None, None, "tester", false)
        .expect("the requested command must not block the apply");

    assert_eq!(
        pending_proposals(&config),
        1,
        "the command was not proposed"
    );
    assert_eq!(
        events_of(&config.data_dir, "stored_command_executed").len(),
        1
    );
    let started: Vec<_> = actions(&config, &session.id, "action_started")
        .into_iter()
        .filter(|event| event["payload"]["action"] == "hook_command")
        .collect();
    assert_eq!(started.len(), 1, "{started:?}");

    let _ = fs::remove_dir_all(&root);
}

/// The edit flow's `before_context` additions are ordinary, redacted,
/// budgeted `hook_context` items (`context.md` §10).
#[test]
fn a_before_context_hook_adds_a_redacted_hook_context_item_to_an_edit_plan() {
    let root = temp_dir("edit-context");
    let hook = answering_hook(
        &root,
        "noter",
        &[HookEvent::BeforeContext],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"context":["Hook note: api_key = sk_live_9f8a7b6c5d4e3f2a1b0c"]}"#,
    );
    let (_config, engine) = engine_for(&root, vec![hook]);
    let mut adapter = MockModelAdapter::new(EDIT_REPLY);

    engine
        .edit_orchestrator
        .propose_edit(&root, "Add a", &[], None, &mut adapter)
        .expect("the proposal should be made");

    let request = adapter.requests.first().expect("the model was called");
    let prompt = &request.messages[1].content;
    assert!(
        prompt.contains("--- hook_context: hook:noter ---"),
        "the hook's context was not a hook_context item: {prompt}"
    );
    assert!(prompt.contains("Hook note:"), "{prompt}");
    assert!(
        !prompt.contains("sk_live_9f8a7b6c5d4e3f2a1b0c"),
        "the hook's context reached the model unredacted: {prompt}"
    );

    // Charged to the turn budget like any other item: under a tiny budget it is
    // dropped rather than made privileged.
    let plan = engine.context_manager.build_context_with_hooks(
        &root,
        &repository_id_for_root(&root),
        "task",
        "Add a",
        None,
        &[],
        4,
        &[("noter".to_string(), "x".repeat(400))],
    );
    assert!(
        plan.items.iter().any(|item| item.kind == "user_prompt"),
        "the budget was too small to compare anything: {:?}",
        plan.items
    );
    assert!(
        plan.items.iter().all(|item| item.kind != "hook_context"),
        "{:?}",
        plan.items
    );

    let _ = fs::remove_dir_all(&root);
}

/// `before_context` has no approval step, so `request_approval` denies
/// (`context.md` §4); the reason reaches the caller and no model is called.
#[test]
fn a_before_context_deny_in_the_edit_flow_refuses_the_proposal() {
    let root = temp_dir("edit-context-deny");
    let hook = answering_hook(
        &root,
        "no_secrets",
        &[HookEvent::BeforeContext],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"verdict":"request_approval","reason":"never put secrets/ in a prompt"}"#,
    );
    let (config, engine) = engine_for(&root, vec![hook]);
    let mut adapter = MockModelAdapter::new(EDIT_REPLY);

    let error = engine
        .edit_orchestrator
        .propose_edit(&root, "Add a", &[], None, &mut adapter)
        .expect_err("a before_context denial must refuse the proposal");

    let message = match &error {
        workspace_engine::ClientError::PolicyBlocked(message) => message,
        other => panic!("expected a policy block, got {other:?}"),
    };
    assert!(message.contains("no_secrets"), "{message}");
    assert!(message.contains("never put secrets/"), "{message}");
    assert!(adapter.requests.is_empty(), "the model was called");
    assert!(
        !config.data_dir.join("patches").exists(),
        "a refused proposal stored a patch"
    );

    let _ = fs::remove_dir_all(&root);
}

/// `session_end` fires on deletion and only observes: a deny is audited and
/// the session is still deleted (`context.md` §3, §4).
#[test]
fn session_end_runs_on_deletion_and_a_deny_does_not_stop_it() {
    let root = temp_dir("session-end");
    let body = format!(
        "echo ran > session_end.log\n{}",
        json_response(r#"{"hookApiVersion":1,"verdict":"deny","reason":"nothing to clean up"}"#)
    );
    let mut hook = sh_hook("at_end", &script(&root, "at_end", &body));
    hook.events = vec![HookEvent::SessionEnd];
    let (config, engine) = engine_for(&root, vec![hook]);
    let store = SessionStore::new(&config.data_dir);
    let session = store
        .create_session(&repository_id_for_root(&root), "ending")
        .unwrap();
    let log = config
        .data_dir
        .join("sessions")
        .join(format!("{}.jsonl", session.id));
    assert!(log.exists(), "the fixture session was not created");

    engine
        .delete_session(&session.id, &root)
        .expect("a deny must not cancel the delete");

    assert!(!log.exists(), "the session was not deleted");
    assert!(
        root.join("session_end.log").exists(),
        "the session_end hook did not run"
    );
    let invoked = events_of(&config.data_dir, "hook_invoked");
    assert_eq!(invoked.len(), 1, "{invoked:?}");
    assert_eq!(invoked[0]["event"], "session_end");
    assert_eq!(invoked[0]["verdict"], "deny");

    let _ = fs::remove_dir_all(&root);
}

/// The root is a caller-supplied argument — the shell takes it from the
/// request form — so it must be the session's own repository. Otherwise
/// deleting A's session with `repo=B` would run B's `session_end` hooks with
/// A's session in the envelope.
#[test]
fn deleting_a_session_with_another_repository_root_is_refused() {
    let root = temp_dir("session-end-owner");
    let other = temp_dir("session-end-other");
    let body = format!(
        "echo ran > session_end.log\n{}",
        json_response(r#"{"hookApiVersion":1,"verdict":"allow"}"#)
    );
    let mut hook = sh_hook("at_end", &script(&root, "at_end", &body));
    hook.events = vec![HookEvent::SessionEnd];
    let (config, engine) = engine_for(&root, vec![hook]);
    let store = SessionStore::new(&config.data_dir);
    let session = store
        .create_session(&repository_id_for_root(&root), "ending")
        .unwrap();
    let log = config
        .data_dir
        .join("sessions")
        .join(format!("{}.jsonl", session.id));
    assert!(log.exists(), "the fixture session was not created");

    let error = engine
        .delete_session(&session.id, &other)
        .expect_err("a root that is not the session's repository must be refused");

    assert!(
        error.to_string().contains("does not belong"),
        "the error should say why: {error}"
    );
    assert!(log.exists(), "a refused delete removed the session");
    assert!(
        !root.join("session_end.log").exists(),
        "the session_end hook ran for another repository's root"
    );
    assert!(
        !other.join("session_end.log").exists(),
        "the session_end hook ran in the other repository"
    );
    assert!(
        events_of(&config.data_dir, "hook_invoked").is_empty(),
        "a refused delete dispatched a hook"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&other);
}

/// Zero hooks means today's behaviour (`Global Constraints`): the same apply,
/// with no hook warnings and nothing audited.
#[test]
fn with_no_hooks_a_patch_apply_is_todays_result() {
    let root = temp_dir("apply-no-hooks");
    let (config, engine) = engine_for(&root, Vec::new());
    let store = SessionStore::new(&config.data_dir);
    let session = store
        .create_session(&repository_id_for_root(&root), "plain")
        .unwrap();
    let task = store
        .create_task(&session.id, "prompt", "provider", "model")
        .unwrap();
    let patch = stage_patch(
        &engine,
        &root,
        "src/a.txt",
        "before\n",
        "after\n",
        Some((&session, &task)),
    );

    let result = engine
        .edit_orchestrator
        .apply_stored_patch(&root, &patch.id, None, None, "tester", false)
        .expect("a patch with no hooks applies");

    assert_eq!(result.applied_files, vec!["src/a.txt"]);
    assert_eq!(result.warnings, Vec::<String>::new());
    assert_eq!(result.hook_warnings, Vec::<String>::new());
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "after\n"
    );
    assert!(events_of(&config.data_dir, "hook_invoked").is_empty());

    let _ = fs::remove_dir_all(&root);
}

// ---- Task 7: chat.rs turn boundaries. `context.md` §3, §4, §10 ------------

/// The active provider with native tools on, so a test can drive a pause with
/// a tool call. `ask_user` exists only as a native call (spec 50).
fn native_provider(id: &str) -> ModelProviderConfig {
    ModelProviderConfig {
        id: id.to_string(),
        label: "Test".to_string(),
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

/// One chat turn, through the same entry point the shell uses.
fn chat_turn(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: Option<&str>,
    adapter: &mut dyn ModelAdapter,
    options: ChatTurnOptions,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.ask_with_session_with_options(
        repo,
        "Do the task.",
        &[],
        session_id,
        adapter,
        &mut sink,
        options,
    )
}

fn policy_blocked(error: workspace_engine::ClientError) -> String {
    match error {
        workspace_engine::ClientError::PolicyBlocked(message) => message,
        other => panic!("expected a policy block, got {other:?}"),
    }
}

/// A hook that appends to a marker file and allows. Dispatch, not the verdict,
/// is what the pause tests observe.
fn marker_hook(root: &Path, id: &str, event: HookEvent) -> (HookConfig, PathBuf) {
    let marker = root.join(format!("{id}-ran"));
    let body = format!(
        "echo invoked >> {}\necho '{{\"hookApiVersion\":1,\"verdict\":\"allow\"}}'\n",
        marker.display()
    );
    let mut hook = sh_hook(id, &script(root, id, &body));
    hook.events = vec![event];
    (hook, marker)
}

fn hook_invocations(config: &Config, event: &str) -> usize {
    events_of(&config.data_dir, "hook_invoked")
        .into_iter()
        .filter(|entry| entry["event"] == event)
        .count()
}

/// `session_start` runs before the session exists, so a denial leaves no
/// session behind and never reaches the model. `request_approval` has no
/// approval step here and denies identically (`context.md` §4).
#[test]
fn a_denying_session_start_hook_creates_no_session() {
    for verdict in ["deny", "request_approval"] {
        let root = temp_dir(&format!("start-{verdict}"));
        write(&root.join("README.md"), "# repo\n");
        let hook = answering_hook(
            &root,
            "no_start",
            &[HookEvent::SessionStart],
            HookMode::Mandatory,
            &format!(
                r#"{{"hookApiVersion":1,"verdict":"{verdict}","reason":"not this repository"}}"#
            ),
        );
        let (config, engine) = engine_for(&root, vec![hook]);
        let mut adapter = MockModelAdapter::new("Done.");

        let message = policy_blocked(
            chat_turn(
                &engine,
                &root,
                None,
                &mut adapter,
                ChatTurnOptions::default(),
            )
            .expect_err("a refusing session_start must refuse the turn"),
        );

        assert!(message.contains("no_start"), "{message}");
        assert!(message.contains("not this repository"), "{message}");
        assert_eq!(adapter.requests.len(), 0, "the model was called");
        assert!(
            engine
                .session_store
                .list_sessions(Some(&repository_id_for_root(&root)))
                .unwrap()
                .is_empty(),
            "a refused session_start created a session"
        );
        assert!(
            !config.data_dir.join("sessions").exists(),
            "a refused session_start wrote a session log"
        );

        let _ = fs::remove_dir_all(&root);
    }
}

/// `session_start` runs before any session or task exists, so its envelope
/// carries neither; the payload is `{}` (`context.md` §6). Task 6's note
/// directed `envelope_parts(None, None, {})`, which serialises the absent id
/// as `null` rather than an empty string.
#[test]
fn a_session_start_envelope_carries_no_session_or_task() {
    let root = temp_dir("start-envelope");
    write(&root.join("README.md"), "# repo\n");
    let envelope = root.join("start-envelope.json");
    let body = format!(
        "cat > {}\necho '{{\"hookApiVersion\":1,\"verdict\":\"allow\"}}'\n",
        envelope.display()
    );
    let mut hook = sh_hook("start_envelope", &script(&root, "start_envelope", &body));
    hook.events = vec![HookEvent::SessionStart];
    let (_, engine) = engine_for(&root, vec![hook]);
    let mut adapter = MockModelAdapter::new("Done.");

    chat_turn(
        &engine,
        &root,
        None,
        &mut adapter,
        ChatTurnOptions::default(),
    )
    .expect("an allowing session_start runs the turn");

    let envelope: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&envelope).unwrap()).unwrap();
    assert_eq!(envelope["event"], "session_start");
    assert!(envelope["sessionId"].is_null(), "{envelope}");
    assert!(envelope["taskId"].is_null(), "{envelope}");
    assert_eq!(envelope["payload"], serde_json::json!({}));

    let _ = fs::remove_dir_all(&root);
}

/// A denied `before_context` fails the task with the hook's kind and message,
/// and the model is never asked (`context.md` §4).
#[test]
fn a_denying_before_context_hook_fails_the_task_without_calling_the_model() {
    let root = temp_dir("chat-context-deny");
    write(&root.join("README.md"), "# repo\n");
    let hook = answering_hook(
        &root,
        "no_context",
        &[HookEvent::BeforeContext],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"verdict":"deny","reason":"this prompt breaks the rule"}"#,
    );
    let (_, engine) = engine_for(&root, vec![hook]);
    let mut adapter = MockModelAdapter::new("Done.");

    let message = policy_blocked(
        chat_turn(
            &engine,
            &root,
            None,
            &mut adapter,
            ChatTurnOptions::default(),
        )
        .expect_err("a refusing before_context must end the turn"),
    );

    assert!(message.contains("no_context"), "{message}");
    assert!(message.contains("this prompt breaks the rule"), "{message}");
    assert_eq!(adapter.requests.len(), 0, "the model was called");
    let sessions = engine
        .session_store
        .list_sessions(Some(&repository_id_for_root(&root)))
        .unwrap();
    let session = sessions.first().expect("the task is recorded");
    let failure_kinds = engine
        .session_store
        .read_task_failure_kinds(&session.id)
        .unwrap();
    assert!(
        failure_kinds.values().any(|kind| kind == "hook_denied"),
        "{failure_kinds:?}"
    );
    let messages = engine.session_store.read_messages(&session.id).unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.role == "assistant" && message.content.contains("no_context")),
        "{messages:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

/// Context from either honoured event reaches the model prompt as an ordinary
/// `hook_context` item (`context.md` §10), with `session_start`'s carried
/// through to the same turn.
#[test]
fn context_from_session_start_and_before_context_reaches_the_model_prompt() {
    let root = temp_dir("chat-hook-context");
    write(&root.join("README.md"), "# repo\n");
    let start = answering_hook(
        &root,
        "start_note",
        &[HookEvent::SessionStart],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"verdict":"allow","context":["a note from session_start"]}"#,
    );
    let before = answering_hook(
        &root,
        "context_note",
        &[HookEvent::BeforeContext],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"verdict":"allow","context":["a note from before_context"]}"#,
    );
    let (_, engine) = engine_for(&root, vec![start, before]);
    let mut adapter = MockModelAdapter::new("Done.");

    chat_turn(
        &engine,
        &root,
        None,
        &mut adapter,
        ChatTurnOptions::default(),
    )
    .expect("an allowing context turn runs");

    let prompt = &adapter.requests[0].messages[1].content;
    assert!(
        prompt.contains("--- hook_context: hook:start_note ---"),
        "{prompt}"
    );
    assert!(prompt.contains("a note from session_start"), "{prompt}");
    assert!(
        prompt.contains("--- hook_context: hook:context_note ---"),
        "{prompt}"
    );
    assert!(prompt.contains("a note from before_context"), "{prompt}");

    let _ = fs::remove_dir_all(&root);
}

/// A command a `before_context` hook requests is an action: it runs under the
/// session's mode and profile and is bracketed by a `hook_command` marker
/// (`context.md` §9). The hook's verdict is unchanged.
#[test]
fn a_before_context_hooks_requested_command_runs_behind_a_marker() {
    let root = temp_dir("chat-context-command");
    write(&root.join("README.md"), "# repo\n");
    let hook = answering_hook(
        &root,
        "check_first",
        &[HookEvent::BeforeContext],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"verdict":"allow","commands":["pwd"]}"#,
    );
    let mut config = config_for(&root, vec![hook]);
    config.shell = "/usr/bin/true".to_string();
    let engine = WorkspaceEngine::new(config.clone());
    let mut adapter = MockModelAdapter::new("Done.");

    let result = chat_turn(
        &engine,
        &root,
        None,
        &mut adapter,
        ChatTurnOptions::default(),
    )
    .expect("the turn runs");

    let markers = actions(&config, &result.session.id, "action_started")
        .into_iter()
        .filter(|event| event["payload"]["action"] == "hook_command")
        .count();
    assert_eq!(
        markers, 1,
        "a requested command opened no hook_command marker"
    );
    assert_eq!(
        adapter.requests.len(),
        1,
        "the turn did not reach the model"
    );

    let _ = fs::remove_dir_all(&root);
}

/// A `before_task_complete` denial is a correction, not a failure: the reason
/// returns as a user-role note and the turn takes another round. A hook that
/// denies once then allows leaves a complete task after two model calls
/// (`context.md` §4).
#[test]
fn a_before_task_complete_hook_that_denies_once_then_allows_completes() {
    let root = temp_dir("chat-complete-once");
    write(&root.join("README.md"), "# repo\n");
    let counter = root.join("complete-count");
    let body = format!(
        "count=$(cat {counter} 2>/dev/null || echo 0)\n\
         count=$((count + 1))\n\
         echo \"$count\" > {counter}\n\
         if [ \"$count\" -eq 1 ]; then\n\
           echo '{{\"hookApiVersion\":1,\"verdict\":\"deny\",\"reason\":\"run the formatter first\"}}'\n\
         else\n\
           echo '{{\"hookApiVersion\":1,\"verdict\":\"allow\"}}'\n\
         fi\n",
        counter = counter.display()
    );
    let mut hook = sh_hook("before_complete", &script(&root, "before_complete", &body));
    hook.events = vec![HookEvent::BeforeTaskComplete];
    let (config, engine) = engine_for(&root, vec![hook]);
    let mut adapter = MockModelAdapter::new("Done.");

    let result = chat_turn(
        &engine,
        &root,
        None,
        &mut adapter,
        ChatTurnOptions::default(),
    )
    .expect("the turn runs");

    assert_eq!(
        adapter.requests.len(),
        2,
        "the denial did not re-enter the loop"
    );
    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "{:?}",
        result.task
    );
    assert_eq!(hook_invocations(&config, "before_task_complete"), 2);
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    assert!(
        messages.iter().any(|message| {
            message.role == "user"
                && message
                    .content
                    .contains("A hook did not let this task complete")
                && message.content.contains("run the formatter first")
        }),
        "{messages:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

/// The existing action budget bounds the re-entries. A hook that always denies
/// reaches the round budget, which is a pause under spec 61 — not a loop and
/// not a `ToolBudgetExhausted` end, because the loop pauses before spending
/// its last forced call. The round budget is exactly `agent_max_tool_rounds`
/// calls (`context.md` §4).
#[test]
fn a_before_task_complete_hook_that_always_denies_stops_at_the_round_budget() {
    let root = temp_dir("chat-complete-always");
    write(&root.join("README.md"), "# repo\n");
    let mut hook = answering_hook(
        &root,
        "always_deny",
        &[HookEvent::BeforeTaskComplete],
        HookMode::Mandatory,
        r#"{"hookApiVersion":1,"verdict":"deny","reason":"not yet"}"#,
    );
    hook.order = 0;
    let mut config = config_for(&root, vec![hook]);
    config.agent_max_tool_rounds = 2;
    let engine = WorkspaceEngine::new(config.clone());
    let mut adapter = MockModelAdapter::new("Done.");

    let result = chat_turn(
        &engine,
        &root,
        None,
        &mut adapter,
        ChatTurnOptions::default(),
    )
    .expect("a budget stop is a pause, not an error");

    assert_eq!(
        adapter.requests.len(),
        2,
        "the round budget did not stop it"
    );
    let pause = result
        .round_budget_proposal
        .as_ref()
        .expect("the turn pauses at its round budget");
    assert_eq!(pause.rounds_used, 2);
    assert_eq!(pause.rounds_total, 2);
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    assert_eq!(
        hook_invocations(&config, "before_task_complete"),
        2,
        "each completion was dispatched"
    );

    let _ = fs::remove_dir_all(&root);
}

/// A pause is not a completion. A turn that stops for command approval must
/// not dispatch `before_task_complete` (`context.md` §3, §4).
#[test]
fn a_command_approval_pause_does_not_dispatch_before_task_complete() {
    let root = temp_dir("chat-command-pause");
    write(&root.join("README.md"), "# repo\n");
    let (hook, marker) = marker_hook(&root, "complete_probe", HookEvent::BeforeTaskComplete);
    let mut config = config_for(&root, vec![hook]);
    config
        .model_providers
        .push(native_provider(&config.model_provider.clone()));
    let engine = WorkspaceEngine::new(config.clone());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![ToolCall {
            id: "call_1".to_string(),
            name: "run_command".to_string(),
            arguments_json: r#"{"command":"npm test","reason":"Run the tests"}"#.to_string(),
        }]],
    );

    let result = chat_turn(
        &engine,
        &root,
        None,
        &mut adapter,
        ChatTurnOptions::default(),
    )
    .expect("the turn pauses");

    assert!(
        result.command_proposal.is_some(),
        "the fixture did not pause for approval"
    );
    assert!(
        !marker.exists(),
        "before_task_complete ran on a command-approval pause"
    );
    assert_eq!(hook_invocations(&config, "before_task_complete"), 0);

    let _ = fs::remove_dir_all(&root);
}

/// `ask_user` is the third kind of pause (spec 50): a question, not a
/// completion, so `before_task_complete` does not see it either.
#[test]
fn a_question_pause_does_not_dispatch_before_task_complete() {
    let root = temp_dir("chat-question-pause");
    write(&root.join("README.md"), "# repo\n");
    let (hook, marker) = marker_hook(&root, "complete_probe", HookEvent::BeforeTaskComplete);
    let mut config = config_for(&root, vec![hook]);
    config.agent_max_questions_per_turn = 3;
    config
        .model_providers
        .push(native_provider(&config.model_provider.clone()));
    let engine = WorkspaceEngine::new(config.clone());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![ToolCall {
            id: "call_ask".to_string(),
            name: "ask_user".to_string(),
            arguments_json: r#"{"question":"Which one?","options":["The CLI","The shell"]}"#
                .to_string(),
        }]],
    );

    let result = chat_turn(
        &engine,
        &root,
        None,
        &mut adapter,
        ChatTurnOptions {
            user_present: true,
            ..ChatTurnOptions::default()
        },
    )
    .expect("the turn pauses for the answer");

    assert!(
        result.question_proposal.is_some(),
        "the fixture did not pause for a question"
    );
    assert!(
        !marker.exists(),
        "before_task_complete ran on a question pause"
    );
    assert_eq!(hook_invocations(&config, "before_task_complete"), 0);

    let _ = fs::remove_dir_all(&root);
}

/// Zero hooks means today's behaviour byte for byte: one model call, the same
/// two messages, `Complete`, and nothing hook-related audited or recorded.
#[test]
fn with_no_hooks_a_chat_turn_is_todays_result() {
    let root = temp_dir("chat-no-hooks");
    write(&root.join("README.md"), "# repo\n");
    let (config, engine) = engine_for(&root, Vec::new());
    let mut adapter = MockModelAdapter::new("Done.");

    let result = chat_turn(
        &engine,
        &root,
        None,
        &mut adapter,
        ChatTurnOptions::default(),
    )
    .expect("the turn runs");

    assert_eq!(result.response, "Done.");
    assert_eq!(result.task.status, TaskStatus::Complete);
    assert_eq!(adapter.requests.len(), 1);
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    let roles: Vec<&str> = messages
        .iter()
        .map(|message| message.role.as_str())
        .collect();
    assert_eq!(roles, ["user", "assistant"]);
    assert_eq!(messages[0].content, "Do the task.");
    assert_eq!(messages[1].content, "Done.");
    assert!(
        engine
            .session_store
            .read_findings(&result.session.id, &root)
            .unwrap()
            .is_empty()
    );
    assert!(events_of(&config.data_dir, "hook_invoked").is_empty());

    let _ = fs::remove_dir_all(&root);
}
