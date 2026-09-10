//! Per-task token accounting, per `docs/specs/19_token_and_cost_accounting/`.
//!
//! The rule these tests exist to hold is requirement 4: an estimate is never
//! presented as measured, and an aggregate is only as trustworthy as its
//! weakest term. The second rule is requirement 5: a call that reached the
//! provider is counted, because under-reporting makes Damaian look cheaper
//! than it is.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{SessionStore, Task, TokenUsage, UsageSource};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_data_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-usage-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

struct Fixture {
    data_dir: PathBuf,
    store: SessionStore,
    task: Task,
}

fn fixture(name: &str) -> Fixture {
    let data_dir = temp_data_dir(name);
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Usage").unwrap();
    let task = store
        .create_task(&session.id, "do the thing", "mock", "m")
        .unwrap();
    Fixture {
        data_dir,
        store,
        task,
    }
}

impl Fixture {
    fn session_log(&self) -> String {
        let path = Path::new(&self.data_dir)
            .join("sessions")
            .join(format!("{}.jsonl", self.task.session_id));
        fs::read_to_string(path).unwrap_or_default()
    }

    fn cleanup(self) {
        let _ = fs::remove_dir_all(self.data_dir);
    }
}

fn measured(input: u64, output: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        source: UsageSource::Measured,
    }
}

#[test]
fn a_tasks_usage_is_the_sum_of_its_runs() {
    let fixture = fixture("sum");
    fixture
        .store
        .record_task_usage(
            &fixture.task,
            "modelrun_1",
            None,
            measured(100, 10),
            None,
            None,
        )
        .unwrap();
    fixture
        .store
        .record_task_usage(
            &fixture.task,
            "modelrun_2",
            None,
            measured(200, 20),
            None,
            None,
        )
        .unwrap();

    let usage = fixture
        .store
        .read_task_usage(&fixture.task.session_id)
        .unwrap();
    let total = usage
        .get(&fixture.task.id)
        .expect("the task should have usage");

    assert_eq!(total.input_tokens, 300);
    assert_eq!(total.output_tokens, 30);
    assert_eq!(total.run_count, 2);
    assert_eq!(total.source, UsageSource::Measured);

    fixture.cleanup();
}

#[test]
fn one_estimated_run_makes_the_whole_total_estimated() {
    let fixture = fixture("mixed");
    fixture
        .store
        .record_task_usage(
            &fixture.task,
            "modelrun_1",
            None,
            measured(100, 10),
            None,
            None,
        )
        .unwrap();
    fixture
        .store
        .record_task_usage(
            &fixture.task,
            "modelrun_2",
            None,
            TokenUsage::estimated(50, 5),
            None,
            None,
        )
        .unwrap();

    let usage = fixture
        .store
        .read_task_usage(&fixture.task.session_id)
        .unwrap();
    let total = &usage[&fixture.task.id];

    assert_eq!(total.input_tokens, 150);
    assert_eq!(
        total.source,
        UsageSource::Estimated,
        "a total containing an estimate is an estimate"
    );

    fixture.cleanup();
}

#[test]
fn cost_is_summed_only_when_every_run_reported_one() {
    // A partial sum would understate the bill while looking like a real
    // figure, which is worse than reporting nothing.
    let fixture = fixture("cost-partial");
    fixture
        .store
        .record_task_usage(
            &fixture.task,
            "modelrun_1",
            None,
            measured(1, 1),
            Some(0.01),
            None,
        )
        .unwrap();
    fixture
        .store
        .record_task_usage(
            &fixture.task,
            "modelrun_2",
            None,
            measured(1, 1),
            None,
            None,
        )
        .unwrap();

    let usage = fixture
        .store
        .read_task_usage(&fixture.task.session_id)
        .unwrap();
    assert_eq!(usage[&fixture.task.id].reported_cost, None);

    fixture.cleanup();
}

#[test]
fn cost_is_reported_when_every_run_carried_one() {
    // The other half of the rule above: it must not be so cautious that a
    // fully-reported task reports nothing.
    let fixture = fixture("cost-full");
    for (run, cost) in [("modelrun_1", 0.01), ("modelrun_2", 0.02)] {
        fixture
            .store
            .record_task_usage(&fixture.task, run, None, measured(1, 1), Some(cost), None)
            .unwrap();
    }

    let usage = fixture
        .store
        .read_task_usage(&fixture.task.session_id)
        .unwrap();
    let cost = usage[&fixture.task.id]
        .reported_cost
        .expect("both runs reported a cost");
    assert!((cost - 0.03).abs() < 1e-9, "cost was {cost}");

    fixture.cleanup();
}

#[test]
fn a_session_written_before_this_change_reports_no_runs_rather_than_a_zero() {
    // Absence is distinguishable from a task that genuinely used nothing.
    // A zero would be indistinguishable from a real measurement of zero.
    let fixture = fixture("legacy");

    let usage = fixture
        .store
        .read_task_usage(&fixture.task.session_id)
        .unwrap();

    assert!(
        !usage.contains_key(&fixture.task.id),
        "a task with no usage events must be absent, not zero"
    );

    fixture.cleanup();
}

#[test]
fn a_usage_record_carries_no_prompt_or_file_content() {
    // Requirement 7, asserted against the bytes on disk rather than the type,
    // because the type is what a later change would widen.
    let fixture = fixture("no-content");
    fixture
        .store
        .record_task_usage(
            &fixture.task,
            "modelrun_1",
            None,
            measured(100, 10),
            None,
            None,
        )
        .unwrap();

    let log = fixture.session_log();
    let usage_line = log
        .lines()
        .find(|line| line.contains("task_usage_recorded"))
        .expect("the usage event should be on disk");

    assert!(
        !usage_line.contains("do the thing"),
        "the prompt reached a usage record: {usage_line}"
    );

    fixture.cleanup();
}
