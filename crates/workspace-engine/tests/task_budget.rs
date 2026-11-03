//! The durable worker-task execution budget and its restrict-only policy,
//! per `docs/specs/103_bounded_worker_continuation.md` Task 1.
//!
//! These tests hold three boundaries: a budget is a finite typed fact rather
//! than prose or configuration text; repository and profile config may only
//! lower it; and a plan approval plus its budget land and replay as one fact
//! while historical logs remain readable with no budget.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::{PlanStep, StepStatus, TaskPlan};
use workspace_engine::{
    ABSOLUTE_TASK_ACTION_ROUND_CEILING, ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
    ABSOLUTE_TASK_INPUT_TOKEN_CEILING, ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING, Config, ConfigOverlay,
    ConfigScope, PlanApproval, RepositoryKeyClass, SessionStore, Task, TaskExecutionBudget,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-task-budget-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn step(id: &str) -> PlanStep {
    PlanStep {
        id: id.to_string(),
        title: format!("Step {id}"),
        detail: None,
        status: StepStatus::InProgress,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
        check: None,
    }
}

struct Fixture {
    data_dir: PathBuf,
    store: SessionStore,
    session_id: String,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let data_dir = temp_dir(name);
        let store = SessionStore::new(&data_dir);
        let session = store
            .create_session("repo_1", "Task budget")
            .expect("a session should be created");
        Self {
            data_dir,
            store,
            session_id: session.id,
        }
    }

    fn task(&self, prompt: &str) -> Task {
        self.store
            .create_task(&self.session_id, prompt, "mock", "m")
            .expect("a task should be created")
    }

    fn plan(&self, task: &Task) {
        let mut plan = TaskPlan::new(&task.id, 0);
        plan.steps.push(step("step_1"));
        self.store
            .create_plan(task, &plan)
            .expect("a plan should be created");
    }

    fn log_path(&self) -> PathBuf {
        self.data_dir
            .join("sessions")
            .join(format!("{}.jsonl", self.session_id))
    }

    fn events(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.log_path())
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Appends a hand-written event, bypassing the store's validation, so a
    /// replay test can reproduce a payload another version or an edit produced.
    fn append_raw_event(&self, event_type: &str, payload: serde_json::Value) {
        let event = serde_json::json!({
            "eventId": "evt_raw",
            "seq": 9999,
            "timestampMs": 0,
            "eventType": event_type,
            "payload": payload,
        });
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path())
            .expect("the session log should open");
        writeln!(file, "{event}").expect("the raw event should be written");
    }
}

fn budget(action: u32, exploration: u32, input: u64, output: u64) -> TaskExecutionBudget {
    TaskExecutionBudget {
        action_rounds: action,
        exploration_rounds: exploration,
        input_tokens: input,
        output_tokens: output,
    }
}

#[test]
fn the_default_policy_is_finite_and_covers_a_segment() {
    let config = Config::default();
    let policy = config.task_budget_policy();

    assert!(policy.default.validate_against(&policy.ceiling).is_ok());
    assert!(config.validate_task_budget(&policy.default).is_ok());
    assert!(
        policy.default.action_rounds >= config.agent_max_tool_rounds,
        "a default total must cover at least one action segment"
    );
    assert!(
        policy.default.exploration_rounds >= config.agent_max_exploration_rounds,
        "a default total must cover at least one exploration segment"
    );
    assert!(policy.default.action_rounds > 0);
    assert!(policy.default.exploration_rounds > 0);
    assert!(policy.default.input_tokens > 0);
    assert!(policy.default.output_tokens > 0);
}

#[test]
fn validation_rejects_zero_overflow_and_a_value_above_the_effective_ceiling() {
    assert!(TaskExecutionBudget::try_new(0, 1, 1, 1).is_err());
    assert!(TaskExecutionBudget::try_new(1, 0, 1, 1).is_err());
    assert!(TaskExecutionBudget::try_new(1, 1, 0, 1).is_err());
    assert!(TaskExecutionBudget::try_new(1, 1, 1, 0).is_err());
    assert!(TaskExecutionBudget::try_new(u64::from(u32::MAX) + 1, 1, 1, 1).is_err());

    let config = Config::default();
    let policy = config.task_budget_policy();
    assert!(config.validate_task_budget(&policy.ceiling).is_ok());

    let above_ceiling = budget(
        policy.ceiling.action_rounds + 1,
        policy.ceiling.exploration_rounds,
        policy.ceiling.input_tokens,
        policy.ceiling.output_tokens,
    );
    assert!(config.validate_task_budget(&above_ceiling).is_err());

    // Per-segment values are upper checkpoint limits, not minimums: a user or
    // repository may deliberately approve a total below one segment, and the
    // segment logic caps a segment by the remaining total instead.
    let below_one_segment = budget(
        1,
        1,
        policy.default.input_tokens,
        policy.default.output_tokens,
    );
    assert!(
        config.validate_task_budget(&below_one_segment).is_ok(),
        "an approved total may be lower than one per-segment allowance"
    );
}

#[test]
fn repository_and_profile_overlays_lower_every_budget_field_and_refuse_raises() {
    let mut config = Config::default();

    let repository = ConfigOverlay::parse(concat!(
        "worker_task_budget_action_rounds=80\n",
        "worker_task_budget_exploration_rounds=120\n",
        "worker_task_budget_input_tokens=7000000\n",
        "worker_task_budget_output_tokens=400000\n",
        "worker_task_budget_max_action_rounds=200\n",
        "worker_task_budget_max_exploration_rounds=300\n",
        "worker_task_budget_max_input_tokens=50000000\n",
        "worker_task_budget_max_output_tokens=3000000\n",
    ))
    .unwrap();
    let outcome = config.apply_overlay_scoped(repository, ConfigScope::Repository);
    assert!(outcome.rejected.is_empty(), "{:?}", outcome.rejected);
    assert_eq!(config.worker_task_budget_action_rounds, 80);
    assert_eq!(config.worker_task_budget_exploration_rounds, 120);
    assert_eq!(config.worker_task_budget_input_tokens, 7_000_000);
    assert_eq!(config.worker_task_budget_output_tokens, 400_000);
    assert_eq!(config.worker_task_budget_max_action_rounds, 200);
    assert_eq!(config.worker_task_budget_max_exploration_rounds, 300);
    assert_eq!(config.worker_task_budget_max_input_tokens, 50_000_000);
    assert_eq!(config.worker_task_budget_max_output_tokens, 3_000_000);

    let profile = ConfigOverlay::parse(concat!(
        "worker_task_budget_action_rounds=64\n",
        "worker_task_budget_exploration_rounds=96\n",
        "worker_task_budget_input_tokens=6000000\n",
        "worker_task_budget_output_tokens=300000\n",
        "worker_task_budget_max_action_rounds=150\n",
        "worker_task_budget_max_exploration_rounds=200\n",
        "worker_task_budget_max_input_tokens=40000000\n",
        "worker_task_budget_max_output_tokens=2000000\n",
    ))
    .unwrap();
    let outcome = config.apply_overlay_scoped(profile, ConfigScope::Profile);
    assert!(outcome.rejected.is_empty(), "{:?}", outcome.rejected);
    assert_eq!(config.worker_task_budget_action_rounds, 64);
    assert_eq!(config.worker_task_budget_exploration_rounds, 96);
    assert_eq!(config.worker_task_budget_input_tokens, 6_000_000);
    assert_eq!(config.worker_task_budget_output_tokens, 300_000);
    assert_eq!(config.worker_task_budget_max_action_rounds, 150);
    assert_eq!(config.worker_task_budget_max_exploration_rounds, 200);
    assert_eq!(config.worker_task_budget_max_input_tokens, 40_000_000);
    assert_eq!(config.worker_task_budget_max_output_tokens, 2_000_000);

    let raises = ConfigOverlay::parse(concat!(
        "worker_task_budget_action_rounds=100\n",
        "worker_task_budget_exploration_rounds=145\n",
        "worker_task_budget_input_tokens=9000000\n",
        "worker_task_budget_output_tokens=600000\n",
        "worker_task_budget_max_action_rounds=256\n",
        "worker_task_budget_max_exploration_rounds=384\n",
        "worker_task_budget_max_input_tokens=64000000\n",
        "worker_task_budget_max_output_tokens=4000000\n",
    ))
    .unwrap();
    let outcome = config.apply_overlay_scoped(raises.clone(), ConfigScope::Repository);
    let rejected: Vec<_> = outcome
        .rejected
        .iter()
        .map(|rejected| (rejected.key.as_str(), rejected.class))
        .collect();
    assert_eq!(
        rejected.len(),
        8,
        "every field must be refused separately: {rejected:?}"
    );
    assert!(
        rejected
            .iter()
            .all(|(_, class)| *class == RepositoryKeyClass::RestrictOnly)
    );
    assert_eq!(config.worker_task_budget_action_rounds, 64);
    assert_eq!(config.worker_task_budget_max_input_tokens, 40_000_000);

    let profile_raise = config.apply_overlay_scoped(raises, ConfigScope::Profile);
    assert_eq!(
        profile_raise.rejected.len(),
        8,
        "{:?}",
        profile_raise.rejected
    );
    assert!(
        profile_raise
            .rejected
            .iter()
            .all(|rejected| rejected.class == RepositoryKeyClass::RestrictOnly)
    );
}

#[test]
fn config_parse_rejects_zero_and_overflow_for_every_budget_key() {
    let round_keys = [
        "worker_task_budget_action_rounds",
        "worker_task_budget_exploration_rounds",
        "worker_task_budget_max_action_rounds",
        "worker_task_budget_max_exploration_rounds",
    ];
    for key in round_keys {
        assert!(
            ConfigOverlay::parse(&format!("{key}=0\n")).is_err(),
            "{key} accepted zero"
        );
        assert!(
            ConfigOverlay::parse(&format!("{key}={}\n", u64::from(u32::MAX) + 1)).is_err(),
            "{key} accepted an overflowing value"
        );
    }

    let token_keys = [
        "worker_task_budget_input_tokens",
        "worker_task_budget_output_tokens",
        "worker_task_budget_max_input_tokens",
        "worker_task_budget_max_output_tokens",
    ];
    for key in token_keys {
        assert!(
            ConfigOverlay::parse(&format!("{key}=0\n")).is_err(),
            "{key} accepted zero"
        );
    }

    // The absolute ceiling itself is a valid configuration value, so the
    // boundary is inclusive rather than off by one.
    assert!(
        ConfigOverlay::parse(&format!(
            "worker_task_budget_action_rounds={}\n",
            workspace_engine::ABSOLUTE_TASK_ACTION_ROUND_CEILING
        ))
        .is_ok()
    );
    assert!(
        ConfigOverlay::parse(&format!(
            "worker_task_budget_action_rounds={}\n",
            workspace_engine::ABSOLUTE_TASK_ACTION_ROUND_CEILING + 1
        ))
        .is_err()
    );
}

#[test]
fn plan_approval_and_budget_land_as_one_event_and_replay() {
    let fixture = Fixture::new("atomic");
    let task = fixture.task("implement the change");
    let approved = budget(64, 96, 4_000_000, 250_000);

    fixture
        .store
        .approve_plan_with_budget(&task, "tester", &approved)
        .unwrap();

    let events = fixture.events();
    let approvals: Vec<_> = events
        .iter()
        .filter(|event| {
            event.get("eventType").and_then(|kind| kind.as_str()) == Some("plan_approved")
        })
        .collect();
    assert_eq!(
        approvals.len(),
        1,
        "the approval and its budget must be one event, not two"
    );
    assert!(
        events.iter().all(
            |event| event.get("eventType").and_then(|kind| kind.as_str())
                != Some("task_budget_approved")
        ),
        "a second event would reintroduce the torn-line failure"
    );

    let payload: PlanApproval = serde_json::from_value(approvals[0]["payload"].clone())
        .expect("the typed approval payload");
    assert_eq!(payload.task_id, task.id);
    assert_eq!(payload.approved_by, "tester");
    assert_eq!(
        payload.budget,
        Some(approved),
        "the single payload carries the reviewed budget"
    );

    let reread = SessionStore::new(&fixture.data_dir);
    assert!(
        reread
            .read_plan_approved(&fixture.session_id, &task.id)
            .unwrap()
    );
    assert_eq!(
        reread
            .read_task_budget(&fixture.session_id, &task.id)
            .unwrap(),
        Some(approved)
    );
}

#[test]
fn a_budget_follows_only_legitimate_continuations_and_never_leaks() {
    let fixture = Fixture::new("ancestry");
    let approved_task = fixture.task("first");
    fixture.plan(&approved_task);
    let approved = budget(64, 96, 4_000_000, 250_000);
    fixture
        .store
        .approve_plan_with_budget(&approved_task, "tester", &approved)
        .unwrap();

    let continuation = fixture.task("second");
    fixture
        .store
        .resume_plan(&continuation, &approved_task.id)
        .unwrap();

    let unrelated = fixture.task("unrelated");

    let deeper = fixture.task("third");
    fixture
        .store
        .resume_plan(&deeper, &continuation.id)
        .unwrap();

    assert_eq!(
        fixture
            .store
            .read_task_budget(&fixture.session_id, &approved_task.id)
            .unwrap(),
        Some(approved)
    );
    assert_eq!(
        fixture
            .store
            .read_task_budget(&fixture.session_id, &continuation.id)
            .unwrap(),
        Some(approved)
    );
    assert_eq!(
        fixture
            .store
            .read_task_budget(&fixture.session_id, &deeper.id)
            .unwrap(),
        Some(approved)
    );
    assert_eq!(
        fixture
            .store
            .read_task_budget(&fixture.session_id, &unrelated.id)
            .unwrap(),
        None
    );

    let other = Fixture::new("ancestry-other-session");
    let other_task = other.task("same session shape");
    assert_eq!(
        other
            .store
            .read_task_budget(&other.session_id, &other_task.id)
            .unwrap(),
        None
    );

    // A continuation that proposes a new plan is a different plan and drops the
    // approval that belonged to the carried one.
    fixture.plan(&continuation);
    assert_eq!(
        fixture
            .store
            .read_task_budget(&fixture.session_id, &continuation.id)
            .unwrap(),
        None
    );
    let after_new_plan = fixture.task("after the new plan");
    fixture
        .store
        .resume_plan(&after_new_plan, &continuation.id)
        .unwrap();
    assert_eq!(
        fixture
            .store
            .read_task_budget(&fixture.session_id, &after_new_plan.id)
            .unwrap(),
        None
    );
}

#[test]
fn historical_plan_approvals_and_sessions_have_no_budget() {
    let fixture = Fixture::new("historical");
    let task = fixture.task("an older turn");
    fixture.store.approve_plan(&task, "tester").unwrap();

    assert!(
        fixture
            .store
            .read_plan_approved(&fixture.session_id, &task.id)
            .unwrap()
    );
    assert_eq!(
        fixture
            .store
            .read_task_budget(&fixture.session_id, &task.id)
            .unwrap(),
        None
    );

    // A plain approval keeps the original payload shape: the budget key is
    // absent rather than serialized as null, so older readers see exactly what
    // they saw before task budgets existed.
    let events = fixture.events();
    let approval = events
        .iter()
        .find(|event| {
            event.get("eventType").and_then(|kind| kind.as_str()) == Some("plan_approved")
        })
        .expect("the plan approval event");
    let keys: std::collections::BTreeSet<_> = approval["payload"]
        .as_object()
        .expect("the approval payload")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        std::collections::BTreeSet::from(["approvedBy", "taskId"])
    );

    // The committed legacy session fixture has no budget anywhere. Copying it
    // into a store proves the reader treats a historical log as "no budget"
    // rather than requiring a new field.
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("legacy_session.jsonl");
    fs::copy(
        fixture_path,
        fixture
            .data_dir
            .join("sessions")
            .join("session_legacy_fixture.jsonl"),
    )
    .unwrap();
    assert_eq!(
        fixture
            .store
            .read_task_budget("session_legacy_fixture", "task_legacy_done")
            .unwrap(),
        None
    );
}

#[test]
fn an_invalid_budget_writes_no_plan_approval() {
    let fixture = Fixture::new("invalid");
    let task = fixture.task("an invalid review");
    let invalid = budget(0, 96, 4_000_000, 250_000);

    assert!(
        fixture
            .store
            .approve_plan_with_budget(&task, "tester", &invalid)
            .is_err()
    );
    assert!(
        !fixture
            .store
            .read_plan_approved(&fixture.session_id, &task.id)
            .unwrap()
    );
    assert!(fixture.events().iter().all(|event| {
        event.get("eventType").and_then(|value| value.as_str()) != Some("plan_approved")
    }));
}

#[test]
fn corrupt_or_invalid_persisted_budgets_read_as_no_budget() {
    let fixture = Fixture::new("invalid-replay");

    let invalid = [
        budget(0, 96, 4_000_000, 250_000),
        budget(64, 0, 4_000_000, 250_000),
        budget(64, 96, 0, 250_000),
        budget(64, 96, 4_000_000, 0),
        budget(
            ABSOLUTE_TASK_ACTION_ROUND_CEILING + 1,
            96,
            4_000_000,
            250_000,
        ),
        budget(
            64,
            ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING + 1,
            4_000_000,
            250_000,
        ),
        budget(64, 96, ABSOLUTE_TASK_INPUT_TOKEN_CEILING + 1, 250_000),
        budget(64, 96, 4_000_000, ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING + 1),
    ];

    for (index, invalid) in invalid.into_iter().enumerate() {
        let task = fixture.task(&format!("invalid budget {index}"));
        fixture.append_raw_event(
            "plan_approved",
            serde_json::json!({
                "taskId": &task.id,
                "approvedBy": "tester",
                "budget": serde_json::to_value(invalid).unwrap(),
            }),
        );
        assert_eq!(
            fixture
                .store
                .read_task_budget(&fixture.session_id, &task.id)
                .unwrap(),
            None,
            "case {index} must not replay as an approved budget"
        );
        assert!(
            fixture
                .store
                .read_plan_approved(&fixture.session_id, &task.id)
                .unwrap(),
            "case {index} is still a plan approval"
        );
    }

    let corrupt = fixture.task("corrupt budget");
    fixture.append_raw_event(
        "plan_approved",
        serde_json::json!({
            "taskId": &corrupt.id,
            "approvedBy": "tester",
            "budget": "not a budget",
        }),
    );
    assert_eq!(
        fixture
            .store
            .read_task_budget(&fixture.session_id, &corrupt.id)
            .unwrap(),
        None
    );

    // The absolute ceiling remains inclusive when a persisted value is replayed.
    let ceiling = budget(
        ABSOLUTE_TASK_ACTION_ROUND_CEILING,
        ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
        ABSOLUTE_TASK_INPUT_TOKEN_CEILING,
        ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING,
    );
    let at_ceiling = fixture.task("valid at the ceiling");
    fixture.append_raw_event(
        "plan_approved",
        serde_json::json!({
            "taskId": &at_ceiling.id,
            "approvedBy": "tester",
            "budget": serde_json::to_value(ceiling).unwrap(),
        }),
    );
    assert_eq!(
        fixture
            .store
            .read_task_budget(&fixture.session_id, &at_ceiling.id)
            .unwrap(),
        Some(ceiling)
    );
}

#[test]
fn the_newest_plan_approval_replaces_an_earlier_budget() {
    let fixture = Fixture::new("newest-approval");
    let task = fixture.task("a re-reviewed plan");
    let approved = budget(64, 96, 4_000_000, 250_000);
    let read = || {
        fixture
            .store
            .read_task_budget(&fixture.session_id, &task.id)
            .unwrap()
    };

    fixture
        .store
        .approve_plan_with_budget(&task, "tester", &approved)
        .unwrap();
    assert_eq!(read(), Some(approved));

    // An ordinary approval is the newest state for the task: the earlier
    // allowance must not survive an approval that carries no budget.
    fixture.store.approve_plan(&task, "tester").unwrap();
    assert_eq!(read(), None);

    fixture
        .store
        .approve_plan_with_budget(&task, "tester", &approved)
        .unwrap();
    assert_eq!(read(), Some(approved));

    // A corrupt budget on a later approval clears the earlier allowance too,
    // rather than leaving a value the user did not just approve.
    fixture.append_raw_event(
        "plan_approved",
        serde_json::json!({
            "taskId": &task.id,
            "approvedBy": "tester",
            "budget": "not a budget",
        }),
    );
    assert_eq!(read(), None);

    fixture
        .store
        .approve_plan_with_budget(&task, "tester", &approved)
        .unwrap();
    assert_eq!(read(), Some(approved));

    // An invalid numeric budget is refused by validation, but the newest
    // approval still wins: the older valid budget is not retained.
    fixture.append_raw_event(
        "plan_approved",
        serde_json::json!({
            "taskId": &task.id,
            "approvedBy": "tester",
            "budget": serde_json::to_value(budget(0, 96, 4_000_000, 250_000)).unwrap(),
        }),
    );
    assert_eq!(read(), None);
}

#[test]
fn the_stored_approval_carries_only_numbers_and_identities() {
    let approval = PlanApproval {
        task_id: "task_1".to_string(),
        approved_by: "tester".to_string(),
        budget: Some(budget(64, 96, 4_000_000, 250_000)),
        plan: None,
    };
    let value = serde_json::to_value(&approval).unwrap();
    let keys: std::collections::BTreeSet<_> = value
        .as_object()
        .expect("an approval object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        std::collections::BTreeSet::from(["approvedBy", "budget", "taskId"])
    );

    let budget_keys: std::collections::BTreeSet<_> = value["budget"]
        .as_object()
        .expect("a budget object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        budget_keys,
        std::collections::BTreeSet::from([
            "actionRounds",
            "explorationRounds",
            "inputTokens",
            "outputTokens",
        ])
    );
    assert_eq!(approval.budget, Some(budget(64, 96, 4_000_000, 250_000)));
    assert_eq!(approval.task_id, "task_1");

    // A legacy payload without a budget decodes as no budget, and a new plain
    // approval serializes the same shape.
    let legacy: PlanApproval = serde_json::from_value(serde_json::json!({
        "taskId": "task_1",
        "approvedBy": "tester",
    }))
    .expect("a legacy approval payload");
    assert_eq!(legacy.budget, None);

    let plain = PlanApproval {
        budget: None,
        ..approval
    };
    let plain_value = serde_json::to_value(&plain).unwrap();
    assert!(plain_value.get("budget").is_none());
}
