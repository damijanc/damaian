//! Hooks, per `docs/specs/32_hooks/proposal.md`.
//!
//! Task 1 pins hook configuration and how each config scope may set it
//! (`context.md` §7.1): a repository may define a hook of its own, which a
//! later task keeps inactive until the user reviews it, and may never touch a
//! user's hook or record a review. A profile carries neither.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    Config, ConfigOverlay, ConfigScope, EffectivePolicy, HookEvent, HookMode, HookReview,
    HookSource, MAX_HOOK_TIMEOUT_MS, RepositoryConfigReport, RepositoryKeyClass, SourceKind,
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
