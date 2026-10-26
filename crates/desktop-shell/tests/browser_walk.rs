//! TEMPORARY review aid — not part of the branch. Seeds a data dir and serves
//! the real UI so the spec-55 surfaces can be walked in a browser.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use desktop_shell::{ShellOptions, run_server_with_ready};
use workspace_engine::compaction::{ConversationSummary, SUMMARY_FORMAT_VERSION};
use workspace_engine::{Config, WorkspaceEngine};

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn seed(engine: &WorkspaceEngine, repo_id: &str, title: &str, turns: usize) -> (String, Vec<u64>) {
    let session = engine.session_store.create_session(repo_id, title).unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, title, "deepseek", "deepseek-chat")
        .unwrap();
    for turn in 0..turns {
        let role = if turn % 2 == 0 { "user" } else { "assistant" };
        engine
            .session_store
            .append_message(
                &session.id,
                Some(&task.id),
                role,
                &format!(
                    "{title} — message {turn}. {}",
                    "The upload client retries on a 5xx and gives up after three attempts. "
                        .repeat(8)
                ),
            )
            .unwrap();
    }
    let seqs = engine
        .session_store
        .read_messages_with_seq(&session.id)
        .unwrap()
        .into_iter()
        .map(|(seq, _)| seq)
        .collect();
    engine
        .session_store
        .update_task_status(&task, workspace_engine::TaskStatus::Complete, None)
        .unwrap();
    (session.id, seqs)
}

#[test]
#[ignore = "temporary browser walk for spec 55"]
fn serves_the_compaction_walk() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = std::env::temp_dir().join(format!("damaian-walk-55-{stamp}"));
    let repo = base.join("repo");
    let data = base.join("data");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&data).unwrap();
    unsafe {
        std::env::set_var("DAMAIAN_DATA_DIR", &data);
    }
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "walk@example.test"]);
    git(&repo, &["config", "user.name", "Walk"]);
    fs::write(
        repo.join("upload.rs"),
        "pub fn upload() {\n    todo!()\n}\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "Initial"]);

    fs::create_dir_all(data.join("config")).unwrap();
    fs::write(
        data.join("config").join("user.conf"),
        "conversation_auto_compact=false\nconversation_window_share=0.01\n\
         model_provider=deepseek\nmodel_name=deepseek-v4-flash\n\
         model_base_url=https://api.deepseek.com\nmodel_api_key_env=DEEPSEEK_API_KEY\n\
         model_provider.deepseek.label=DeepSeek\n\
         model_provider.deepseek.base_url=https://api.deepseek.com\n\
         model_provider.deepseek.api_key_env=DEEPSEEK_API_KEY\n\
         model_provider.deepseek.models=deepseek-v4-flash|deepseek-v4-pro\n\
         model_provider.deepseek.supports_native_tools=true\n",
    )
    .unwrap();

    let repo_arg = repo.to_string_lossy().to_string();
    let config = Config::load_for_repository(Some(&repo)).unwrap();
    let engine = WorkspaceEngine::new(config);
    let repo_id = engine.indexer.repository_id_for_path(&repo_arg).unwrap();

    // A: over the window, no summary — the elision marker and a manual button.
    let (elision, _) = seed(&engine, &repo_id, "Walk the upload client", 14);
    // B: a summary covering the first four messages, with narrative.
    let (summarised, seqs) = seed(&engine, &repo_id, "Compacted upload client", 8);
    engine
        .session_store
        .append_conversation_compaction(
            &summarised,
            &ConversationSummary {
                id: "summary_walk".to_string(),
                through_seq: seqs[3],
                format_version: SUMMARY_FORMAT_VERSION,
                objective: "Add retry to the upload client, but do not change the public API."
                    .to_string(),
                files_changed: vec!["upload.rs".to_string()],
                plan_steps: Vec::new(),
                approvals: Vec::new(),
                validation: Vec::new(),
                constraints: vec!["Do not change the public API.".to_string()],
                decisions: vec![
                    "Reused the existing retry helper instead of adding a new one.".to_string(),
                ],
                failed_approaches: vec![
                    "Sleeping between attempts made the tests slow.".to_string(),
                ],
                uncertainty: vec!["The 32-bit path is untested.".to_string()],
                model_run_id: None,
                message_count: 4,
            },
        )
        .unwrap();
    // C: a recorded failure, shown on the marker after reload.
    let (failed, _) = seed(&engine, &repo_id, "Failed compaction walk", 4);
    engine
        .session_store
        .record_conversation_compaction_failure(
            &failed,
            "the provider refused the summarisation call",
        )
        .unwrap();

    // D: a spare session for an end-to-end DeepSeek compaction check.
    let (live, _) = seed(&engine, &repo_id, "Live DeepSeek check", 6);

    println!("sessions: elision={elision} summarised={summarised} failed={failed} live={live}");
    println!("repo: {repo_arg}");
    println!("data: {}", data.display());

    run_server_with_ready(
        ShellOptions {
            port: 4877,
            default_repo: Some(repo_arg),
            api_token: "walk-token".to_string(),
        },
        |port| println!("READY {port}"),
    )
    .unwrap();
}
