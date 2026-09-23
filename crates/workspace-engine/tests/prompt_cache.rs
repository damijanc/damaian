//! Spec 49 Task 8: prefix-stability guards.
//!
//! `context.md` §3.1: `system_prompt()` (`chat.rs`) is a static string with no
//! timestamp, session id, run id, or round counter, and it is `messages[0]` of
//! every request. Both tests here assert what that already produces — they are
//! guards on a property that is easy to destroy accidentally and silent when
//! destroyed, not fixes for a bug. This slice changes no request; if a change
//! here ever needed to touch `build_model_prompt` or `system_prompt` to pass,
//! it would have left the slice.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use workspace_engine::{Config, MockModelAdapter, WorkspaceEngine};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-prompt-cache-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn write_fixture(root: &Path, relative_path: &str, content: &str) {
    let path = root.join(relative_path);
    fs::create_dir_all(path.parent().expect("fixture should have parent")).unwrap();
    fs::write(path, content).unwrap();
}

/// Matches `foundation.rs`'s `test_config`: registering the FSEvents watcher
/// costs real wall-clock time per fixture and nothing here needs freshness.
fn test_config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    }
}

/// A small, unchanging repository. Both tests assemble against it more than
/// once and rely on it not changing between assemblies.
fn repo_fixture(name: &str) -> PathBuf {
    let repo = temp_dir(name);
    write_fixture(
        &repo,
        "README.md",
        "# Prompt cache prefix test fixture\n\nA small, unchanging repository.\n",
    );
    write_fixture(
        &repo,
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    );
    repo
}

/// Requirement 4's assertion half. Two turns against the same unchanged
/// repository and the same prompt assemble byte-identical requests —
/// `ModelRequest` derives `Eq`, so the whole first request of each turn,
/// including the assembled context section, is compared directly rather than
/// field by field.
#[test]
fn two_assemblies_of_an_unchanged_repository_are_identical() {
    let repo = repo_fixture("two-assemblies");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let prompt = "What does this repository do?";
    let mut on_token = |_token: &str| {};

    let mut first_adapter = MockModelAdapter::new("This repository adds two numbers.");
    engine
        .chat_orchestrator
        .ask(&repo, prompt, &[], &mut first_adapter, &mut on_token)
        .unwrap();

    let mut second_adapter = MockModelAdapter::new("This repository adds two numbers.");
    engine
        .chat_orchestrator
        .ask(&repo, prompt, &[], &mut second_adapter, &mut on_token)
        .unwrap();

    assert_eq!(
        first_adapter.requests[0], second_adapter.requests[0],
        "assembling the same prompt against an unchanged repository twice must \
         produce byte-identical requests"
    );

    fs::remove_dir_all(repo).unwrap();
}

/// Requirement 5. The stable prefix is `system_prompt()` at `messages[0]`
/// (`context.md` §3.1) — this asserts it stays identical across a measurable
/// wall-clock gap, which is the test that fails the day someone interpolates
/// "Current time:" or similar into it.
#[test]
fn no_clock_value_enters_the_stable_prefix() {
    let repo = repo_fixture("no-clock-value");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let prompt = "What does this repository do?";
    let mut on_token = |_token: &str| {};

    let mut first_adapter = MockModelAdapter::new("This repository adds two numbers.");
    engine
        .chat_orchestrator
        .ask(&repo, prompt, &[], &mut first_adapter, &mut on_token)
        .unwrap();

    // A measurable interval: long enough that any clock read at
    // millisecond or second granularity would differ between the two calls.
    thread::sleep(Duration::from_millis(1_100));

    let mut second_adapter = MockModelAdapter::new("This repository adds two numbers.");
    engine
        .chat_orchestrator
        .ask(&repo, prompt, &[], &mut second_adapter, &mut on_token)
        .unwrap();

    assert_eq!(
        first_adapter.requests[0].messages[0].content,
        second_adapter.requests[0].messages[0].content,
        "the stable prefix (the system prompt) must not change across a \
         measurable interval"
    );

    fs::remove_dir_all(repo).unwrap();
}
