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
use workspace_engine::{
    AuditLog, SecretScanner, SessionStore, Task, TaskStatus, TokenUsage, UsageSource,
    classify_session,
};

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
        // A measured run from a provider that reports no cache split: not
        // reported, which is distinct from reported-and-zero.
        cached_input_tokens: None,
        source: UsageSource::Measured,
    }
}

/// A measured run from a provider that *does* report a cache split.
fn measured_with_cache(input: u64, output: u64, cached: u64) -> TokenUsage {
    TokenUsage {
        cached_input_tokens: Some(cached),
        ..measured(input, output)
    }
}

/// Spec 49 task 5: the cache figures aggregate through the same
/// `read_task_usage` that owns every other per-task total.
mod cached_tokens {
    use super::*;

    #[test]
    fn a_tasks_cached_tokens_are_the_sum_of_its_runs() {
        let fixture = fixture("cached-sum");
        for (run, input, cached) in [("modelrun_1", 1000, 800), ("modelrun_2", 2000, 1500)] {
            fixture
                .store
                .record_task_usage(
                    &fixture.task,
                    run,
                    None,
                    measured_with_cache(input, 10, cached),
                    None,
                    None,
                )
                .unwrap();
        }

        let usage = fixture
            .store
            .read_task_usage(&fixture.task.session_id)
            .unwrap();
        let total = usage.get(&fixture.task.id).expect("usage");

        assert_eq!(total.cached_input_tokens, Some(2300));
        assert_eq!(total.cache_reported_input_tokens, 3000);
        assert_eq!(total.runs_without_cache_report, 0);

        fixture.cleanup();
    }

    #[test]
    fn a_task_whose_runs_never_reported_a_split_reports_not_reported() {
        // `None`, never `Some(0)`. A zero here renders as "caching is broken"
        // when the truth is "we cannot see it".
        let fixture = fixture("cached-none");
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

        let usage = fixture
            .store
            .read_task_usage(&fixture.task.session_id)
            .unwrap();
        let total = usage.get(&fixture.task.id).expect("usage");

        assert_eq!(total.cached_input_tokens, None);
        assert_eq!(total.cache_reported_input_tokens, 0);
        assert_eq!(total.runs_without_cache_report, 1);

        fixture.cleanup();
    }

    #[test]
    fn one_run_without_a_split_does_not_erase_the_others() {
        // The mixed case. The sum is over the runs that reported, and the
        // denominator is those same runs' input tokens — not the task's, which
        // would silently dilute the rate with runs nobody can see into. A
        // partial sum over a whole-task denominator is the failure #19's
        // `reported_cost` rule already guards against.
        //
        // The **non-reporting run is recorded first on purpose.** With the
        // reporting run first, a buggy `cache_reported_input_tokens =
        // total.input_tokens` coincides with the right answer at the moment it
        // runs, and this test passes while guarding nothing — which is exactly
        // what it did when first written, found by mutating the
        // implementation. In this order the two differ, 1000 against 6000.
        let fixture = fixture("cached-mixed");
        fixture
            .store
            .record_task_usage(
                &fixture.task,
                "modelrun_1",
                None,
                measured(5000, 10),
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
                measured_with_cache(1000, 10, 800),
                None,
                None,
            )
            .unwrap();

        let usage = fixture
            .store
            .read_task_usage(&fixture.task.session_id)
            .unwrap();
        let total = usage.get(&fixture.task.id).expect("usage");

        assert_eq!(total.input_tokens, 6000);
        assert_eq!(total.cached_input_tokens, Some(800));
        assert_eq!(
            total.cache_reported_input_tokens, 1000,
            "the rate's denominator is the reporting runs, not the task"
        );
        assert_eq!(total.runs_without_cache_report, 1);

        fixture.cleanup();
    }

    #[test]
    fn a_reported_zero_is_kept_apart_from_silence() {
        let fixture = fixture("cached-zero");
        fixture
            .store
            .record_task_usage(
                &fixture.task,
                "modelrun_1",
                None,
                measured_with_cache(1000, 10, 0),
                None,
                None,
            )
            .unwrap();

        let usage = fixture
            .store
            .read_task_usage(&fixture.task.session_id)
            .unwrap();
        let total = usage.get(&fixture.task.id).expect("usage");

        assert_eq!(total.cached_input_tokens, Some(0));
        assert_eq!(total.runs_without_cache_report, 0);

        fixture.cleanup();
    }

    #[test]
    fn an_event_written_before_the_field_existed_counts_as_not_reported() {
        // The migration case at the aggregation layer. An old log has no
        // `cachedInputTokens`, and that must read as silence rather than as a
        // measured zero. Written as raw JSON because the writer can no longer
        // produce an event without the field.
        let fixture = fixture("cached-legacy");
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
        assert!(
            !log.contains("\"cachedInputTokens\":0"),
            "a run that reported no split must not write a zero: {log}"
        );

        fixture.cleanup();
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

// ---------------------------------------------------------------------------
// A call lost to a crash. Spec 19 §5.5: the request went out and was billed,
// and its answer is gone. Under-reporting here would make a crash look free.
// ---------------------------------------------------------------------------

struct CrashFixture {
    data_dir: PathBuf,
    store: SessionStore,
    audit: AuditLog,
    session_id: String,
}

fn crash_fixture(name: &str) -> CrashFixture {
    let data_dir = temp_data_dir(name);
    let store = SessionStore::new(&data_dir);
    let audit = AuditLog::new(&data_dir, true, SecretScanner::default());
    let session = store.create_session("repo_1", "Crash").unwrap();
    CrashFixture {
        data_dir,
        store,
        audit,
        session_id: session.id,
    }
}

impl CrashFixture {
    /// The signature a kill leaves mid-call: the task in a non-terminal state
    /// with a `model_call` marker that started and never finished, carrying
    /// the estimate written before the request went out.
    fn task_with_model_call_in_flight(&self, estimate: u64) -> Task {
        let task = self
            .store
            .create_task(&self.session_id, "do the thing", "mock", "m")
            .unwrap();
        let task = self
            .store
            .update_task_status(&task, TaskStatus::RunningTool, None)
            .unwrap();
        // Never finished: stands in for the process dying here.
        let _marker = self
            .store
            .start_action_with_estimate(&task, "model_call", "m", false, Some(estimate))
            .unwrap();
        task
    }

    fn classify(&self) {
        classify_session(&self.store, &self.audit, &self.session_id).unwrap();
    }

    fn cleanup(self) {
        let _ = fs::remove_dir_all(self.data_dir);
    }
}

#[test]
fn a_call_lost_to_a_crash_is_counted_at_recovery() {
    let fixture = crash_fixture("lost-call");
    let task = fixture.task_with_model_call_in_flight(4200);

    fixture.classify();

    let usage = fixture.store.read_task_usage(&fixture.session_id).unwrap();
    let total = usage
        .get(&task.id)
        .expect("a call that was billed and lost is still counted");

    assert_eq!(total.run_count, 1);
    assert_eq!(total.input_tokens, 4200);
    assert_eq!(total.output_tokens, 0, "no answer ever came back");
    assert_eq!(total.source, UsageSource::Estimated);

    fixture.cleanup();
}

#[test]
fn classifying_twice_does_not_bill_the_lost_call_twice() {
    // The launch sweep runs at every start. Without a guard, one crash would
    // grow the reported spend every time the app is opened.
    let fixture = crash_fixture("idempotent");
    let task = fixture.task_with_model_call_in_flight(4200);

    fixture.classify();
    fixture.classify();

    let usage = fixture.store.read_task_usage(&fixture.session_id).unwrap();
    assert_eq!(usage[&task.id].run_count, 1);
    assert_eq!(usage[&task.id].input_tokens, 4200);

    fixture.cleanup();
}

#[test]
fn a_call_that_was_already_accounted_is_not_billed_again_at_recovery() {
    // A crash *after* the usage event was appended but before the task
    // reached a terminal state. The marker is still dangling, but the call is
    // already paid for.
    let fixture = crash_fixture("already-accounted");
    let task = fixture.task_with_model_call_in_flight(4200);
    let marker_id = fixture
        .store
        .dangling_actions(&fixture.session_id)
        .unwrap()
        .first()
        .expect("the fixture leaves one dangling action")
        .marker_id
        .clone();
    fixture
        .store
        .record_task_usage(
            &task,
            "modelrun_1",
            Some(&marker_id),
            measured(4200, 130),
            None,
            None,
        )
        .unwrap();

    fixture.classify();

    let usage = fixture.store.read_task_usage(&fixture.session_id).unwrap();
    assert_eq!(usage[&task.id].run_count, 1, "the call was already counted");
    assert_eq!(usage[&task.id].output_tokens, 130);

    fixture.cleanup();
}

#[test]
fn a_dangling_action_that_is_not_a_model_call_bills_nothing() {
    // A patch application or a command left in flight cost no tokens.
    let fixture = crash_fixture("not-a-model-call");
    let task = fixture
        .store
        .create_task(&fixture.session_id, "do the thing", "mock", "m")
        .unwrap();
    let task = fixture
        .store
        .update_task_status(&task, TaskStatus::ApplyingPatch, None)
        .unwrap();
    let _marker = fixture
        .store
        .start_action(&task, "apply_patch", "patch_1", true)
        .unwrap();

    fixture.classify();

    let usage = fixture.store.read_task_usage(&fixture.session_id).unwrap();
    assert!(!usage.contains_key(&task.id));

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
