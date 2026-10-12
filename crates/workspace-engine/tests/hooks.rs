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

use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use workspace_engine::finding::Severity;
use workspace_engine::hooks::{
    self, HOOK_API_VERSION, HookEnvelope, HookFailureKind, HookRun, MAX_HOOK_COMMANDS,
    MAX_HOOK_CONTEXT_CHARS, MAX_HOOK_CONTEXT_ENTRIES, MAX_HOOK_FINDINGS, MAX_HOOK_REASON_CHARS,
    MAX_HOOK_STDERR_BYTES, MAX_HOOK_STDOUT_BYTES, ResponseError, Verdict, WideningAttempt,
};
use workspace_engine::{
    Config, ConfigOverlay, ConfigScope, EffectivePolicy, HookConfig, HookEvent, HookMode,
    HookReview, HookSource, MAX_HOOK_TIMEOUT_MS, ProcessRegistry, RepositoryConfigReport,
    RepositoryKeyClass, SecretScanner, SourceKind, split_profile_keys,
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

    let run = run(
        &hook,
        &runner_envelope(&root, json!({})),
        &root,
        &data_dir,
        Duration::from_millis(300),
    );

    assert_eq!(run.failure, Some(HookFailureKind::Timeout));
    assert!(
        run.duration < Duration::from_secs(1),
        "the deadline was not enforced: {:?}",
        run.duration
    );
    assert_eq!(registry_entries(&data_dir), 0);
    let recorded = fs::read_to_string(&pids).expect("the hook recorded its pids");
    let pids: Vec<i32> = recorded
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    assert_eq!(pids.len(), 2);
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
        Duration::from_secs(10),
    );
    assert_eq!(run_endless.failure, Some(HookFailureKind::OutputTooLarge));
    assert!(run_endless.stdout.len() <= MAX_HOOK_STDOUT_BYTES + 1);
    assert!(
        run_endless.duration < Duration::from_secs(2),
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
