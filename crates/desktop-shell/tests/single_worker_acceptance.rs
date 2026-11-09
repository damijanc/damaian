//! Spec 106 Task 5: the live tier of the single-worker acceptance gate
//! (`context.md` D4, D9).
//!
//! The real `damaian-desktop-shell --worker` binary is started by the
//! shell's own `workers::Supervisor`, in a fresh git worktree of a freshly
//! materialized `single_worker_spec` fixture, with an isolated data directory.
//! The shared acceptance driver answers its decisions over the real control
//! socket; patches are applied the way the shell applies them. The run is
//! judged from durable records, its report is written under the data
//! directory, and its summary — deviations included — is appended to the parent
//! (main) session the worker was spawned from.
//!
//! Every test here starts a real process, so every test is `#[ignore]`d, per
//! `AGENTS.md`. Nothing here kills by name: the supervisor ends the worker.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use desktop_shell::workers::{SpawnRequest, Supervisor, WorkerTimeouts};
use eval_harness::acceptance::AcceptanceFixture;
use eval_harness::acceptance::criteria::{CriteriaInput, ProtectedSnapshot, evaluate};
use eval_harness::acceptance::driver::{DEFAULT_MAX_DECISIONS, drive};
use eval_harness::acceptance::policy::Policy;
use eval_harness::acceptance::report::{AcceptanceReport, RunMetadata};
use eval_harness::acceptance::worker::{WorkerControl, WorkerTiming, WorkerTransport};
use eval_harness::fixture;
use serde_json::{Map, Value};
use workspace_engine::{ClientError, Config, SessionStore};

const FIXTURE: &str = "single_worker_spec";

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// The worker's socket lives under the data directory and its path must fit in
/// 104 bytes, so the data directory is short (the fixture's own is not).
fn short_data_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsa-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("config")).unwrap();
    dir
}

/// Sends the acceptance transport's requests over the real control socket.
struct SupervisorControl<'a> {
    supervisor: &'a Supervisor,
    session_id: String,
}

impl WorkerControl for SupervisorControl<'_> {
    fn request(&mut self, request: Value) -> workspace_engine::Result<Value> {
        let Value::Object(mut fields) = request else {
            return Err(ClientError::InvalidInput(
                "a request must be an object".to_string(),
            ));
        };
        let command = fields
            .remove("command")
            .and_then(|command| command.as_str().map(str::to_string))
            .ok_or_else(|| ClientError::InvalidInput("a request needs a command".to_string()))?;
        let arguments: Map<String, Value> = fields
            .into_iter()
            .map(|(key, value)| {
                // The socket takes every argument as a string, as the shell's
                // routes send them.
                let value = match value {
                    Value::String(text) => Value::String(text),
                    other => Value::String(other.to_string()),
                };
                (key, value)
            })
            .collect();
        self.supervisor
            .control(&self.session_id, &command, arguments)
            .map_err(ClientError::InvalidInput)
    }
}

/// What a run needs to know about the provider it talks to.
struct Provider {
    /// `None` for the mocked wiring run.
    name: Option<String>,
    model: Option<String>,
    api_key: String,
}

struct LiveRun {
    report: AcceptanceReport,
    data_dir: PathBuf,
    parent_session: String,
}

/// One acceptance run through the real worker binary.
fn run_through_real_worker(provider: &Provider) -> LiveRun {
    let acceptance = AcceptanceFixture::load(FIXTURE).unwrap();
    let materialized = fixture::materialize(FIXTURE).unwrap();
    let source = fs::canonicalize(&materialized.root).unwrap();
    let data_dir = short_data_dir();
    let config = Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        ..Config::default()
    };
    // The worker loads its provider from this file (`load_worker_config`).
    let mut user_config = String::new();
    if let (Some(name), Some(model)) = (&provider.name, &provider.model) {
        user_config.push_str(&format!("model_provider={name}\nmodel_name={model}\n"));
    }
    fs::write(config.user_config_path(), user_config).unwrap();

    // Taken on the source fixture: the worktree starts from its HEAD, and the
    // snapshot compares repository-relative paths.
    let protected_before = ProtectedSnapshot::take(&source, &acceptance.protected_paths);
    let store = SessionStore::new(&data_dir);
    let parent = store
        .create_session(
            &workspace_engine::hash::repository_id_for_root(&source),
            "Acceptance (main session)",
        )
        .unwrap();

    let supervisor = Supervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_damaian-desktop-shell")),
        WorkerTimeouts::default(),
    );
    let clock = Instant::now();
    let spawned = supervisor
        .spawn(
            &config,
            SpawnRequest {
                repo: source.clone(),
                prompt: acceptance.prompt.clone(),
                include_uncommitted: false,
                parent_session_id: Some(parent.id.clone()),
                parent_task_id: None,
            },
            &provider.api_key,
        )
        .unwrap();
    let worktree = fs::canonicalize(&spawned.worktree).unwrap();

    let control = SupervisorControl {
        supervisor: &supervisor,
        session_id: spawned.session_id.clone(),
    };
    let mut transport = WorkerTransport::new(
        control,
        &data_dir,
        &spawned.session_id,
        &worktree,
        WorkerTiming::live(),
    )
    .unwrap();
    let outcome = drive(
        &mut transport,
        &Policy::new(acceptance.clone()),
        DEFAULT_MAX_DECISIONS,
    );
    let criteria = evaluate(&CriteriaInput {
        store: &store,
        session_id: &spawned.session_id,
        repository_root: &worktree,
        acceptance: &acceptance,
        outcome: &outcome,
        protected_before: &protected_before,
    })
    .unwrap();
    let report = AcceptanceReport::build(
        RunMetadata {
            run_id: format!("live-{}", spawned.session_id),
            tier: "live".to_string(),
            fixture: FIXTURE.to_string(),
            provider: provider.name.clone(),
            model: provider.model.clone(),
            duration_ms: clock.elapsed().as_millis(),
        },
        &outcome,
        criteria,
        &store,
        &spawned.session_id,
    )
    .unwrap();
    let report_dir = report.write(&data_dir).unwrap();
    // D9: the summary, deviations included, reaches the main session the
    // worker was spawned from, not only a file.
    store
        .append_message(&parent.id, None, "assistant", &report.return_summary())
        .unwrap();
    supervisor.shutdown_all();
    println!(
        "{}\nreport: {}\ndata directory kept for inspection: {}",
        report.return_summary(),
        report_dir.display(),
        data_dir.display()
    );
    let _ = fs::remove_dir_all(&materialized.root);
    LiveRun {
        report,
        data_dir,
        parent_session: parent.id,
    }
}

/// The parent session's last message: where the summary was appended.
fn last_parent_message(data_dir: &Path, parent: &str) -> String {
    SessionStore::new(data_dir)
        .read_messages(parent)
        .unwrap()
        .last()
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

/// Spec 106 requirement 6: one live acceptance run against an explicitly
/// selected real provider, through the real worker binary. It calls the
/// provider over the network and spends real tokens, so it is ignored by
/// default and never runs in CI.
///
/// Run it by hand, from the repository root, with the key read inline for the
/// one command (never stored in config, files or logs). For DeepSeek, whose key
/// is the Keychain item `deepseek-api-key`:
///
/// ```sh
/// DAMAIAN_EVAL_PROVIDER=deepseek DAMAIAN_EVAL_MODEL=deepseek-v4-flash \
///   DEEPSEEK_API_KEY="$(security find-generic-password -s deepseek-api-key -w)" \
///   cargo test -p desktop-shell --test single_worker_acceptance --locked \
///   -- --ignored --nocapture live_single_worker_acceptance
/// ```
///
/// The key variable is the provider entry's `model_api_key_env`. The run prints
/// its summary and the report directory, keeps its data directory, and fails
/// unless every requirement-8 criterion passes. Spec 106 Task 6 needs three
/// consecutive passes from fresh state; record each run's report in its tasks.md.
#[test]
#[ignore = "needs a real provider, its API key and the network; see the doc comment"]
fn live_single_worker_acceptance() {
    let name = std::env::var("DAMAIAN_EVAL_PROVIDER")
        .expect("set DAMAIAN_EVAL_PROVIDER to the provider to accept against");
    let model = std::env::var("DAMAIAN_EVAL_MODEL")
        .expect("set DAMAIAN_EVAL_MODEL to the model to accept against");
    let mut probe = Config {
        model_provider: name.clone(),
        ..Config::default()
    };
    probe.apply_model_provider_defaults();
    let api_key = std::env::var(&probe.model_api_key_env).unwrap_or_else(|_| {
        panic!(
            "set {} for {name}; read it inline for this one command",
            probe.model_api_key_env
        )
    });
    let run = run_through_real_worker(&Provider {
        name: Some(name),
        model: Some(model),
        api_key,
    });
    assert!(
        last_parent_message(&run.data_dir, &run.parent_session).starts_with("Acceptance run"),
        "the summary reaches the main session"
    );
    assert!(run.report.passed, "{}", run.report.return_summary());
}

/// The live tier's wiring, without a provider: the real worker binary is
/// spawned in a fresh worktree, answers on its socket, is driven by the shared
/// driver, ends, and the report and the main-session summary are written. The
/// mocked model answers once with text, so the acceptance itself fails; this
/// run proves the plumbing only. It starts a real process, so it is ignored.
///
/// ```sh
/// cargo test -p desktop-shell --test single_worker_acceptance --locked \
///   -- --ignored --nocapture the_live_tier_wiring_runs_against_a_mocked_worker
/// ```
#[test]
#[ignore = "starts the real worker binary; see the doc comment"]
fn the_live_tier_wiring_runs_against_a_mocked_worker() {
    // SAFETY: set before this test starts any thread or process that reads the
    // environment; the worker it spawns inherits it.
    unsafe { std::env::set_var("DAMAIAN_MOCK_MODEL_RESPONSE", "Done by the mocked worker.") };
    let run = run_through_real_worker(&Provider {
        name: None,
        model: None,
        api_key: "mocked-worker-key".to_string(),
    });
    let report = &run.report;
    assert!(
        report.ended_status.is_some(),
        "the worker ran its prompt and ended, answering on its socket throughout \
         (data kept at {}): {}",
        run.data_dir.display(),
        report.return_summary()
    );
    assert!(
        !report
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("transport failed")),
        "the socket answered every request: {}",
        report.return_summary()
    );
    assert!(
        !report.passed,
        "a single text answer cannot pass the acceptance: {}",
        report.return_summary()
    );
    assert_eq!(report.criteria.len(), 10, "every criterion was judged");
    let report_dir = run
        .data_dir
        .join("acceptance")
        .join(&report.metadata.run_id);
    assert!(report_dir.join("report.json").is_file());
    assert!(report_dir.join("report.md").is_file());
    assert!(
        last_parent_message(&run.data_dir, &run.parent_session).starts_with("Acceptance run"),
        "the summary reaches the main session"
    );
    let _ = fs::remove_dir_all(&run.data_dir);
}
