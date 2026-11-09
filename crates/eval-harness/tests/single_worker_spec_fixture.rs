//! Spec 106 Task 2: the `single_worker_spec` fixture is what its acceptance
//! data says it is.
//!
//! These are not scenarios. They pin the fixture's starting state with the real
//! toolchain, so the deterministic and live tiers both start from a repository
//! whose workspace check already fails on the protected crate only, whose
//! scoped check passes, and whose answers are not readable by the worker.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use eval_harness::acceptance::AcceptanceFixture;
use eval_harness::fixture;
use workspace_engine::{ProtectedPathPromptOutcome, parse_protected_path_prompt};

const FIXTURE: &str = "single_worker_spec";

fn acceptance() -> AcceptanceFixture {
    AcceptanceFixture::load(FIXTURE).expect("the acceptance data should load")
}

/// Run a shell command in `root` with the fixture's own target directory, so
/// the test never shares or pollutes another build.
fn run(root: &Path, command: &str) -> Output {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .expect("the command should start")
}

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn the_acceptance_data_names_the_fixture_and_its_contract() {
    let acceptance = acceptance();
    assert_eq!(acceptance.fixture, FIXTURE);
    assert_eq!(
        acceptance.protected_paths,
        vec!["crates/legacy/".to_string()]
    );
    assert!(!acceptance.clarification.keywords.is_empty());
    assert!(!acceptance.clarification.answer.trim().is_empty());
    assert_ne!(
        acceptance.expected.tie_cents_if_half_up, acceptance.expected.tie_cents,
        "the clarification must change an acceptance detail"
    );
    assert!(
        acceptance
            .expected_command_families
            .iter()
            .any(|family| family == "cargo test")
    );
}

#[test]
fn the_worker_prompt_declares_the_protected_path() {
    let acceptance = acceptance();
    match parse_protected_path_prompt(&acceptance.prompt) {
        ProtectedPathPromptOutcome::Request { paths } => {
            assert_eq!(paths, vec!["crates/legacy/".to_string()]);
        }
        other => panic!("the prompt must declare `crates/legacy/`: {other:?}"),
    }
}

#[test]
fn the_answers_are_not_inside_the_repository_the_worker_sees() {
    let acceptance = acceptance();
    let materialized = fixture::materialize(FIXTURE).expect("the fixture materializes");
    for leaked in ["acceptance.toml", "prompt.md"] {
        assert!(
            !materialized.root.join(leaked).exists(),
            "{leaked} must stay outside the fixture"
        );
    }
    let mut stack = vec![materialized.root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("the fixture directory reads") {
            let path = entry.expect("a directory entry").path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let text = fs::read_to_string(&path).unwrap_or_default().to_lowercase();
            for secret in &acceptance.clarification.answer_markers {
                assert!(
                    !text.contains(&secret.to_lowercase()),
                    "{} reveals the clarification answer ({secret})",
                    path.display()
                );
            }
        }
    }
    let _ = fs::remove_dir_all(&materialized.root);
}

#[test]
fn the_workspace_check_fails_only_on_the_pre_existing_legacy_test() {
    let acceptance = acceptance();
    let materialized = fixture::materialize(FIXTURE).expect("the fixture materializes");
    assert_eq!(materialized.version, "1");

    let workspace = run(&materialized.root, "cargo test --offline --no-fail-fast");
    let output = combined(&workspace);
    assert!(
        !workspace.status.success(),
        "the workspace check must already fail: {output}"
    );
    assert!(
        output.contains(&acceptance.pre_existing_failure.test),
        "the pre-existing failure is the legacy test: {output}"
    );
    let failed_tests: Vec<&str> = output
        .lines()
        .filter(|line| line.starts_with("test ") && line.ends_with("... FAILED"))
        .collect();
    assert_eq!(
        failed_tests.len(),
        1,
        "nothing but the legacy test may fail at task start: {output}"
    );
    assert!(
        failed_tests[0].contains(&acceptance.pre_existing_failure.test),
        "{failed_tests:?}"
    );

    let scoped = run(&materialized.root, &acceptance.scoped_check.command);
    assert!(
        scoped.status.success(),
        "the scoped check must pass at task start: {}",
        combined(&scoped)
    );
    let _ = fs::remove_dir_all(&materialized.root);
}
