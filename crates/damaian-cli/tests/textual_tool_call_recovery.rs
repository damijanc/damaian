//! A complete tool call written as DeepSeek text must reach the same dispatch
//! and patch-review path as a native call. Spec 18 §7, observation #43.
//!
//! `DAMAIAN_MOCK_MODEL_RESPONSE` carries the exact failing shape through the
//! CLI's own `ask` command, so this is end to end: adapter recovery, tool
//! dispatch, proposal creation, and the audit record that says a patch is
//! waiting for review.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("damaian-cli-textual-{name}-{stamp}"));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn textual_propose_patch() -> String {
    concat!(
        "<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>function<｜tool▁sep｜>propose_patch\n",
        "```json\n",
        r#"{"summary":"Add a greeting","files":[{"path":"src/main.rs","status":"modified","content":"fn main() {\n    println!(\"hello\");\n}\n"}]}"#,
        "\n```<｜tool▁call▁end｜><｜tool▁calls▁end｜>",
    )
    .to_string()
}

#[test]
fn a_textual_tool_call_in_a_mock_response_reaches_patch_review() {
    let data_dir = temp_dir("data");
    let repo = temp_dir("repo");
    fs::create_dir_all(repo.join("src")).expect("source dir should be created");
    fs::write(repo.join("src/main.rs"), "fn main() {}\n").expect("source should be written");
    fs::write(repo.join("README.md"), "# Fixture\n").expect("readme should be written");

    let response = textual_propose_patch();
    let output = Command::new(env!("CARGO_BIN_EXE_damaian"))
        .arg("ask")
        .arg(&repo)
        .arg("Add a greeting to src/main.rs and propose the patch.")
        .env("DAMAIAN_DATA_DIR", &data_dir)
        .env("DAMAIAN_MOCK_MODEL_RESPONSE", &response)
        .env_remove("DAMAIAN_ADMIN_CONFIG")
        .output()
        .expect("cli should run");

    assert!(
        output.status.success(),
        "ask should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let audit = fs::read_to_string(data_dir.join("audit/events.jsonl"))
        .expect("the turn should write an audit log");
    assert!(
        audit.contains("\"eventType\":\"model_response_completed\"")
            && audit.contains("\"status\":\"patch_proposal_ready\""),
        "the recovered call should produce a patch awaiting review:\n{audit}"
    );
    assert!(
        audit.contains("\"eventType\":\"model_tool_call_markup_recovered\""),
        "the recovery should be visible in the audit log:\n{audit}"
    );
}
