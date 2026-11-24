use super::{
    Request, ShellOptions, TurnEvent, allowed_cors_origin, api_request_requires_token,
    browser_report_from_tool_result, cached_model_api_key, chat_engine, checkpoint_list_json,
    checkpoint_restore_json, config_and_report_for_repo, default_engine,
    desktop_settings_config_path, effective_policy_for_repo, engine_for_repo,
    fetch_decision_error_code, fetch_decision_error_json, fetch_proposal_json,
    forget_model_api_key, generated_secret_warnings_json, handle_connection, index_html,
    json_error, json_optional_string, keychain, language_servers_json, mcp_browser_arguments,
    message_json, parse_form, parse_path_list, parse_plan_revision, patch_payload_json,
    percent_decode, phase_json, plan_json, plan_proposal_json, question_proposal_json,
    relay_turn_events, remember_model_api_key, render_markdown_with_optional_file_links,
    repository_config_review_json, repository_config_write_target, require_api_token,
    resolve_shell_model_api_key, run_answer_question_request, run_continue_task_request_with,
    run_fetch_decision_request_with, run_server, run_terminal_command, save_config_file,
    session_verification_json, spawn_with_shell_stack, sweep_orphaned_processes, symbol_repository,
    task_states_json, task_states_json_with_outcomes, task_usage_json, terminal_cwd_for_repo,
    turn_progress_event, validate_context_files, validate_working_folder, validate_workspace_path,
    verification_report_json, verify_data_dir_schema_at, web_diagnostic_json,
    web_diagnostic_reveal_target, worker_returns_json, write_basic_response, write_sse_event,
};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use workspace_engine::CheckpointRestoreResult;
use workspace_engine::{
    AgentFetchProposal, AgentPlanProposal, AgentQuestionProposal, AskUserInput, CancelToken,
    ChatMessage, ChatTurnOptions, ChatTurnResult, CheckId, CheckOutcome, CheckProvenanceRecord,
    CheckTermination, CompletionGateReport, CompletionReason, CompletionReasonCode, Config,
    Evidence, FailureFingerprint, FailureProvenance, FetchComponentFlag, FindingEvidence,
    GeneratedSecretWarning, LanguageServerHub, LanguageServerLaunch, LanguageServerState,
    LanguageServerStatus, MockModelAdapter, PLANNED_CHECK_VERSION, PendingQuestionRef, PhaseKind,
    PlanRevisionCheck, PlanStep, PlannedCheck, RepairScopeReview, SecretScanner, SessionMode,
    StepStatus, TaskBudgetReview, TaskPlan, TaskStatus, TaskUsage, ToolCall, TurnNotes, TurnPhase,
    TurnProgress, TurnSink, UsageSource, VerificationEvent, VerificationOutcome,
    VerificationReport, WebDiagnosticRecord, WebDiagnosticReport, WorkspaceEngine,
    finding::{Finding, FindingDraft, FindingSource, Severity, SourceRange},
    fold_verification,
};

/// Points every engine built in this test binary at a throwaway data
/// directory, and returns it.
///
/// Repository config cannot set `data_dir` — spec 34 makes it forbidden, so
/// a repository that arrives with a clone cannot redirect where data lands.
/// That leaves the environment as the only lever a test has, and without it
/// these tests write sessions, patches, and checkpoints straight into the
/// user's real `~/Library/Application Support/DamaianClient`.
pub(super) fn isolated_data_dir() -> &'static std::path::Path {
    static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
    DATA_DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "damaian-shell-test-data-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("test data dir");
        // Set once, inside the initializer, before any engine in this
        // binary is built: a concurrent test waits here rather than
        // racing, and a test binary shares its environment with nothing.
        unsafe { std::env::set_var("DAMAIAN_DATA_DIR", &dir) };
        dir
    })
}

/// A sink that starts refusing writes after `writes_before_failure`, the way
/// a socket does once the client has gone away.
struct FlakyWriter {
    written: Vec<u8>,
    writes_before_failure: usize,
}

impl Write for FlakyWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.writes_before_failure == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "client went away",
            ));
        }
        self.writes_before_failure -= 1;
        self.written.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Without a periodic write there is nothing to fail on, so a client that
// disappeared while the provider was still thinking would go unnoticed —
// exactly the long-wait case the whole feature exists for.
//
// The token is sent only once a keepalive has been written. A fixed sleep
// before sending raced the relay's start: on a loaded CI runner the relay
// began after the token was queued and never had a silent interval to fill.
#[test]
fn the_relay_writes_a_keepalive_while_the_turn_is_silent() {
    struct SignallingWriter {
        written: Vec<u8>,
        keepalive_seen: Option<std::sync::mpsc::Sender<()>>,
    }
    impl std::io::Write for SignallingWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buffer);
            if String::from_utf8_lossy(&self.written).contains(": keepalive")
                && let Some(seen) = self.keepalive_seen.take()
            {
                let _ = seen.send(());
            }
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let (sender, receiver) = std::sync::mpsc::channel();
    let (seen_sender, seen) = std::sync::mpsc::channel();
    let cancel = CancelToken::new();
    std::thread::spawn(move || {
        // Bounded, so a relay that never writes a keepalive fails the
        // assertion below instead of hanging the test.
        let _ = seen.recv_timeout(Duration::from_secs(10));
        let _ = sender.send(TurnEvent::Token("hi".to_string()));
    });

    let mut out = SignallingWriter {
        written: Vec::new(),
        keepalive_seen: Some(seen_sender),
    };
    relay_turn_events(&mut out, &cancel, None, receiver, Duration::from_millis(25)).expect("relay");

    let text = String::from_utf8_lossy(&out.written);
    assert!(text.contains(": keepalive"), "got {text:?}");
    assert!(text.contains("\"token\":\"hi\""), "got {text:?}");
    assert!(!cancel.is_cancelled());
}

/// A plan mid-turn: one step confirmed by a command, one still running.
fn sample_plan() -> TaskPlan {
    let mut plan = TaskPlan::new("task_1", 7);
    plan.steps.push(PlanStep {
        id: "step_1".to_string(),
        title: "Read the retry helper".to_string(),
        detail: None,
        status: StepStatus::Completed,
        depends_on: Vec::new(),
        started_at_ms: Some(7),
        completed_at_ms: Some(8),
        evidence: vec![Evidence::CommandExit {
            marker_id: "action_1".to_string(),
            exit_code: Some(0),
            purpose: workspace_engine::CommandPurpose::Other,
            command_hash: None,
            working_directory: String::new(),
        }],
        check: None,
        build: None,
    });
    plan.steps.push(PlanStep {
        id: "step_2".to_string(),
        title: "Add a bounded backoff".to_string(),
        detail: None,
        status: StepStatus::InProgress,
        depends_on: Vec::new(),
        started_at_ms: Some(8),
        completed_at_ms: None,
        evidence: Vec::new(),
        check: None,
        build: None,
    });
    plan
}

#[test]
fn the_plan_card_lists_a_step_check_command_and_directory() {
    // Spec 105 §7: the review card must show each proposed check's command
    // and directory, so the user can see and shape what they are approving.
    let mut plan = sample_plan();
    plan.steps[1].check = Some(PlannedCheck {
        version: PLANNED_CHECK_VERSION,
        command: "cargo nextest run -p workspace-engine".to_string(),
        working_directory: "crates/workspace-engine".to_string(),
        purpose: "validates the final tree".to_string(),
        required: true,
    });

    let json = plan_json(&plan, true);
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid plan JSON");
    let check = &value["steps"][1]["check"];
    assert_eq!(check["command"], "cargo nextest run -p workspace-engine");
    assert_eq!(check["workingDirectory"], "crates/workspace-engine");
    assert_eq!(check["purpose"], "validates the final tree");
    assert_eq!(check["required"], true);
    // The card transports the engine-internal version so a re-approval does
    // not re-stamp a future-version check.
    assert_eq!(check["version"], PLANNED_CHECK_VERSION);
    // A step without a check reads as null, not an absent key.
    assert_eq!(value["steps"][0]["check"], serde_json::Value::Null);
    // Spec 109 D10: the card also shows a step's build check, and a step
    // without one reads as null.
    assert_eq!(value["steps"][1]["build"], serde_json::Value::Null);
    plan.steps[0].build = Some(PlannedCheck {
        version: PLANNED_CHECK_VERSION,
        command: "cargo check -p workspace-engine --tests".to_string(),
        working_directory: "crates/workspace-engine".to_string(),
        purpose: "from CI".to_string(),
        required: false,
    });
    let value: serde_json::Value =
        serde_json::from_str(&plan_json(&plan, true)).expect("valid plan JSON");
    let build = &value["steps"][0]["build"];
    assert_eq!(build["command"], "cargo check -p workspace-engine --tests");
    assert_eq!(build["workingDirectory"], "crates/workspace-engine");
    assert_eq!(build["required"], false);
}

#[test]
fn a_plan_revision_round_trips_a_structured_check() {
    // Spec 105 §2: the revision's `check` key has three spellings — absent
    // preserves the existing check (a pre-check client), null removes it,
    // and an object sets it. A set check keeps its `version` verbatim, and a
    // `required: false` check parses as optional rather than being coerced.
    let parsed = parse_plan_revision(
        r#"[
                {"id":"step_1","title":"Keep its check"},
                {"id":"step_2","title":"Drop its check","check":null},
                {"id":"step_3","title":"Set a check","check":{"command":"cargo test","workingDirectory":"","purpose":"why","required":true,"version":7}},
                {"id":"step_4","title":"An optional check","check":{"command":"cargo doc","workingDirectory":"docs","purpose":"optional","required":false}}
            ]"#,
        Path::new("/repo"),
    )
    .expect("a revision parses");
    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed[0].check, PlanRevisionCheck::Preserve);
    assert_eq!(parsed[1].check, PlanRevisionCheck::Remove);
    assert_eq!(
        parsed[2].check,
        PlanRevisionCheck::Set(PlannedCheck {
            version: 7,
            command: "cargo test".to_string(),
            working_directory: String::new(),
            purpose: "why".to_string(),
            required: true,
        })
    );
    // A check without a version defaults to the current one; `required:
    // false` is preserved as false, not coerced to true.
    assert_eq!(
        parsed[3].check,
        PlanRevisionCheck::Set(PlannedCheck {
            version: PLANNED_CHECK_VERSION,
            command: "cargo doc".to_string(),
            working_directory: "docs".to_string(),
            purpose: "optional".to_string(),
            required: false,
        })
    );
}

#[test]
fn a_plan_revision_normalizes_a_check_directory_like_a_check_id() {
    // Spec 105: the card sends what the user typed; the route stores the
    // `CheckId` form so the plan and the run command's evidence agree.
    let parsed = parse_plan_revision(
        r#"[
                {"id":"step_1","title":"Root","check":{"command":"cargo test","workingDirectory":".","purpose":"why","required":true}},
                {"id":"step_2","title":"Subdirectory","check":{"command":"ls","workingDirectory":"./crates/foo/","purpose":"why","required":true}},
                {"id":"step_3","title":"Already relative","check":{"command":"ls","workingDirectory":"crates/foo","purpose":"why","required":true}}
            ]"#,
        Path::new("/repo"),
    )
    .expect("a revision parses");
    let directories: Vec<&str> = parsed
        .iter()
        .filter_map(|step| match &step.check {
            PlanRevisionCheck::Set(check) => Some(check.working_directory.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(directories, vec!["", "crates/foo", "crates/foo"]);
}

#[test]
fn a_plan_revision_rejects_a_check_directory_outside_the_repository() {
    for directory in ["../outside", "/etc"] {
        let raw = format!(
            r#"[{{"id":"step_1","title":"Run","check":{{"command":"ls","workingDirectory":"{directory}","purpose":"why","required":true}}}}]"#
        );
        let error = parse_plan_revision(&raw, Path::new("/repo"))
            .expect_err("a directory outside the repository is refused");
        assert!(
            error.contains("outside the repository"),
            "{directory}: {error}"
        );
    }
}

#[test]
fn the_plan_event_carries_each_step_with_its_status_and_evidence() {
    // The panel has to distinguish a step confirmed by something Damaian
    // watched from one merely claimed, so the evidence has to cross the
    // wire — a status alone cannot tell the two apart (§5.3, §5.6).
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = CancelToken::new();
    std::thread::spawn(move || {
        let _ = sender.send(TurnEvent::Plan {
            plan: Box::new(sample_plan()),
            active: true,
        });
    });

    let mut out = FlakyWriter {
        written: Vec::new(),
        writes_before_failure: usize::MAX,
    };
    relay_turn_events(&mut out, &cancel, None, receiver, Duration::from_millis(25)).expect("relay");

    let text = String::from_utf8_lossy(&out.written);
    assert!(text.contains("event: plan"), "got {text:?}");
    assert!(text.contains("\"title\":\"Read the retry helper\""));
    assert!(text.contains("\"status\":\"completed\""));
    assert!(text.contains("\"status\":\"in_progress\""));
    assert!(
        text.contains("\"kind\":\"commandExit\""),
        "the evidence must reach the panel, not just the status: {text:?}"
    );
    // And the outcome the report will print, so the panel does not have to
    // re-derive "completed with nothing behind it" for itself.
    assert!(text.contains("\"outcome\":\"verified\""), "got {text:?}");
    assert!(text.contains("\"outcome\":\"outstanding\""), "got {text:?}");
}

#[test]
fn an_ended_turn_s_plan_is_not_active() {
    let mut plans = HashMap::new();
    plans.insert("task_1".to_string(), sample_plan());
    let mut statuses = HashMap::new();
    statuses.insert("task_1".to_string(), "complete".to_string());

    // `sample_plan` has one completed step and one in progress, so an ended
    // turn must say the second was never marked done rather than "running".
    let json = task_states_json(
        &statuses,
        &HashMap::new(),
        &plans,
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
    );
    assert!(json.contains("\"active\":false"), "{json}");
    assert!(
        json.contains("Turn ended with 1 step not marked done"),
        "{json}"
    );

    statuses.insert("task_1".to_string(), "running_tool".to_string());
    let json = task_states_json(
        &statuses,
        &HashMap::new(),
        &plans,
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
    );
    assert!(json.contains("\"active\":true"), "{json}");
    assert!(json.contains("1 step outstanding"), "{json}");
}

/// Spec 107 Task 1: a reloaded task waiting on a decision carries it in its
/// presentation, and its plan says `waiting`; a running task's does not.
#[test]
fn a_waiting_task_s_plan_and_presentation_carry_its_pending_decision() {
    let mut plans = HashMap::new();
    plans.insert("task_1".to_string(), sample_plan());
    let mut statuses = HashMap::new();
    statuses.insert("task_1".to_string(), "waiting_for_approval".to_string());
    let decision = workspace_engine::TurnPendingDecision {
        kind: workspace_engine::PendingDecisionKind::Patch,
        label: "Review the patch".to_string(),
        status_label: "Waiting for your review".to_string(),
        card_id: "patch_1".to_string(),
    };
    let mut pending = HashMap::new();
    pending.insert("task_1".to_string(), decision.clone());
    let json = task_states_json_with_outcomes(
        &statuses,
        &HashMap::new(),
        &plans,
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
        &HashMap::new(),
        &pending,
    );
    let tasks: serde_json::Value = serde_json::from_str(&format!("[{json}]")).unwrap();
    assert_eq!(tasks[0]["plan"]["waiting"], true, "{json}");
    assert_eq!(tasks[0]["plan"]["active"], true, "{json}");
    assert_eq!(
        tasks[0]["turnPresentation"]["pendingDecision"],
        serde_json::to_value(&decision).unwrap()
    );

    statuses.insert("task_1".to_string(), "running_tool".to_string());
    let json = task_states_json(
        &statuses,
        &HashMap::new(),
        &plans,
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
    );
    let tasks: serde_json::Value = serde_json::from_str(&format!("[{json}]")).unwrap();
    assert_eq!(tasks[0]["plan"]["waiting"], false, "{json}");
    assert!(
        tasks[0]["turnPresentation"]
            .get("pendingDecision")
            .is_none(),
        "{json}"
    );
}

#[test]
fn the_reminder_is_a_tagged_system_line_in_a_reloaded_session() {
    // The conversation view needs the tag to render the engine's nudge as a
    // system line rather than as something the user typed (spec 66 §5.5).
    let tagged = ChatMessage {
        id: "msg_1".to_string(),
        session_id: "session_1".to_string(),
        task_id: Some("task_1".to_string()),
        role: "user".to_string(),
        content: "Your plan still has open steps.".to_string(),
        kind: Some("plan_reminder".to_string()),
        created_at_ms: 5,
    };
    let json = message_json(1, &tagged, None);
    assert!(json.contains("\"kind\":\"plan_reminder\""), "{json}");

    let ordinary = ChatMessage {
        id: "msg_2".to_string(),
        session_id: "session_1".to_string(),
        task_id: None,
        role: "user".to_string(),
        content: "hello".to_string(),
        kind: None,
        created_at_ms: 6,
    };
    let json = message_json(2, &ordinary, None);
    assert!(!json.contains("\"kind\""), "{json}");
}

#[test]
fn the_plan_event_carries_whether_the_turn_is_active() {
    match turn_progress_event(TurnProgress::Plan(sample_plan())) {
        Some(TurnEvent::Plan { active, .. }) => assert!(active),
        _ => panic!("the plan update must become a live plan event"),
    }
    match turn_progress_event(TurnProgress::PlanFinished(sample_plan())) {
        Some(TurnEvent::Plan { active, .. }) => assert!(!active),
        _ => panic!("the finished plan must become a live plan event"),
    }
}

#[test]
fn a_plan_with_two_steps_in_progress_says_so_rather_than_picking_one() {
    // Requirement 2 is an invariant, and a panel that quietly rendered the
    // first of two in-progress steps would hide exactly the bug the
    // invariant exists to catch.
    let mut plan = sample_plan();
    plan.steps[0].status = StepStatus::InProgress;
    assert!(plan.violates_single_in_progress());

    let json = plan_json(&plan, true);
    assert!(
        json.contains("\"violatesSingleInProgress\":true"),
        "got {json}"
    );

    let json = plan_json(&sample_plan(), true);
    assert!(json.contains("\"violatesSingleInProgress\":false"));
}

#[test]
fn a_blocked_plan_is_never_summarised_as_complete_over_the_wire() {
    let mut plan = sample_plan();
    plan.steps[1].status = StepStatus::Blocked;

    let json = plan_json(&plan, true);
    assert!(json.contains("\"isComplete\":false"), "got {json}");
    assert!(
        !json.to_lowercase().contains("\"summary\":\"complete"),
        "got {json}"
    );
    assert!(json.contains("\"outcome\":\"blocked\""));
}

#[test]
fn the_chat_result_carries_a_question_put_up_for_an_answer() {
    // Spec 50 requirement 3, Task 6: the live card needs the question and
    // its options, and the payload must give no approval route anything to
    // act on. A `proposalId` here is how a question would become an
    // approval in the client.
    let json = question_proposal_json(Some(&AgentQuestionProposal {
        question_id: "question_7f".to_string(),
        question: "Did you mean the CLI or the shell?".to_string(),
        options: vec!["The CLI".to_string(), "The shell".to_string()],
        allow_free_text: true,
    }));
    let value: serde_json::Value = serde_json::from_str(&json).expect("question JSON");
    assert_eq!(value["questionId"], "question_7f");
    assert_eq!(value["question"], "Did you mean the CLI or the shell?");
    assert_eq!(value["options"][1], "The shell");
    assert_eq!(value["allowFreeText"], true);
    assert!(
        value.get("proposalId").is_none(),
        "a question has no proposal id to approve: {value}"
    );
    assert!(
        value.get("requiresApproval").is_none(),
        "a question is not an approval: {value}"
    );
    assert_eq!(question_proposal_json(None), "null");
}

#[test]
fn the_fetch_proposal_carries_the_verbatim_url_and_the_long_flag() {
    let long = format!("q={}", "a".repeat(198));
    let url = format!("http://127.0.0.1:9/docs?{long}");
    let json = fetch_proposal_json(Some(&AgentFetchProposal {
        id: "fetch_7f".to_string(),
        host: "127.0.0.1".to_string(),
        url: url.clone(),
        approval_key: "127.0.0.1:9".to_string(),
        long_components: vec![FetchComponentFlag {
            component: "query".to_string(),
            bytes: 200,
        }],
        query: Some(long.clone()),
        fragment: None,
    }));
    let value: serde_json::Value = serde_json::from_str(&json).expect("fetch JSON");
    assert_eq!(value["proposalId"], "fetch_7f");
    assert_eq!(value["host"], "127.0.0.1");
    assert_eq!(value["url"], url, "the dialog gets the URL unshortened");
    assert_eq!(value["approvalKey"], "127.0.0.1:9");
    assert_eq!(value["longComponents"][0]["component"], "query");
    assert_eq!(value["longComponents"][0]["bytes"], 200);
    assert_eq!(value["query"], long);
    assert_eq!(value["fragment"], serde_json::Value::Null);
    assert_eq!(fetch_proposal_json(None), "null");
}

#[test]
fn a_fetch_decision_refuses_another_pause_kind_with_a_stated_code() {
    isolated_data_dir();
    let repo = question_repo("fetch-decision-question");
    let paused = paused_on_question(&repo);
    let question_id = paused
        .question_proposal
        .as_ref()
        .expect("the question pauses")
        .question_id
        .clone();
    let form = form_fields(&[
        ("repo", repo.to_str().unwrap()),
        ("proposal_id", question_id.as_str()),
        ("decision", "once"),
        ("session_id", paused.session.id.as_str()),
    ]);
    let cancel = CancelToken::new();
    let (events, _discard) = std::sync::mpsc::channel();
    let mut adapter_for = |_config: &Config| {
        Ok(Box::new(MockModelAdapter::new("Done.")) as Box<dyn workspace_engine::ModelAdapter>)
    };
    let error = run_fetch_decision_request_with(&form, &cancel, &events, &mut adapter_for)
        .expect_err("a question is not a fetch approval");
    assert!(error.contains("is a question"), "{error}");
    assert!(error.contains("resume_after_answer"), "{error}");

    // The shell maps an engine refusal to a body the client can branch on.
    let code = fetch_decision_error_code(&error);
    assert_eq!(code, "not_fetch_pause");
    let body: serde_json::Value =
        serde_json::from_str(&fetch_decision_error_json(code, &error)).expect("error JSON");
    assert_eq!(body["code"], "not_fetch_pause");
    assert_eq!(body["error"], error);
    assert_eq!(body["message"], error);
}

/// A resume must use the provider the chat selected, not the saved
/// config's (spec 74). With the provider line gone from `user.conf`, every
/// approval after the first answer ran on the built-in default and failed
/// with "OPENAI_API_KEY is required".
#[test]
fn a_resume_uses_the_chats_selected_provider() {
    isolated_data_dir();
    let repo = question_repo("resume-chat-provider");
    let paused = paused_on_question(&repo);
    let question_id = paused
        .question_proposal
        .as_ref()
        .expect("the question pauses")
        .question_id
        .clone();
    let form = form_fields(&[
        ("repo", repo.to_str().unwrap()),
        ("proposal_id", question_id.as_str()),
        ("decision", "once"),
        ("session_id", paused.session.id.as_str()),
        ("model_provider", "deepseek"),
        ("model", "deepseek-v4-pro"),
    ]);
    let cancel = CancelToken::new();
    let (events, _discard) = std::sync::mpsc::channel();
    let mut used = None;
    let mut adapter_for = |config: &Config| {
        used = Some((config.model_provider.clone(), config.model_name.clone()));
        Ok(Box::new(MockModelAdapter::new("Done.")) as Box<dyn workspace_engine::ModelAdapter>)
    };
    let _ = run_fetch_decision_request_with(&form, &cancel, &events, &mut adapter_for);

    assert_eq!(
        used,
        Some(("deepseek".to_string(), "deepseek-v4-pro".to_string()))
    );
}

/// Spec 108 D5: the task record keeps the model it was created with, but
/// a resume on a switched model runs its calls on the new one. Both must
/// be recorded, the starting model first, and reach the payload.
#[test]
fn a_resume_on_a_switched_model_records_both_models() {
    isolated_data_dir();
    let repo = question_repo("resume-switched-model");
    let paused = paused_on_question(&repo);
    let starting = paused.task.model_name.clone();
    assert_ne!(
        starting, "deepseek-v4-pro",
        "the fixture must start elsewhere"
    );
    let question_id = paused
        .question_proposal
        .as_ref()
        .expect("the question pauses")
        .question_id
        .clone();
    // What the answer route does: an engine on the model the request
    // names, which here is not the one the task was created with.
    let form = form_fields(&[
        ("repo", repo.to_str().unwrap()),
        ("model_provider", "deepseek"),
        ("model", "deepseek-v4-pro"),
    ]);
    let switched = chat_engine(
        super::config_for_repo_with_model_options(repo.to_str().unwrap(), &form).expect("config"),
    );
    assert_eq!(switched.config.model_name, "deepseek-v4-pro");
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = turn_sink(&cancel, &mut on_token, &mut on_progress);
    let mut adapter = MockModelAdapter::new("Done.");
    switched
        .chat_orchestrator
        .resume_after_answer(&question_id, "The CLI", Some(0), &mut adapter, &mut sink)
        .expect("the resume runs");

    let engine = engine_for_question_test(&repo);
    let usage = engine
        .session_store
        .read_task_usage(&paused.session.id)
        .expect("usage reads");
    let total = usage.get(&paused.task.id).expect("the task recorded usage");
    let names: Vec<&str> = total
        .models
        .iter()
        .map(|model| model.name.as_str())
        .collect();
    assert_eq!(names, vec![starting.as_str(), "deepseek-v4-pro"]);
    assert_eq!(total.models[1].provider, "deepseek");
    // The task record itself is unchanged: it is the starting model.
    assert_eq!(paused.task.model_name, starting);

    let json = task_usage_json(Some(total), None);
    assert!(
        json.contains(&format!(
            "\"models\":[{{\"provider\":\"{}\",\"name\":\"{}\"}},{{\"provider\":\"deepseek\",\"name\":\"deepseek-v4-pro\"}}]",
            paused.task.model_provider, starting
        )),
        "{json}"
    );
}

fn form_fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// A repository whose engine loads the isolated test data directory, which
/// is the same directory a shell route reads when it builds its own engine.
fn question_repo(name: &str) -> PathBuf {
    isolated_data_dir();
    let repo = temp_path(name);
    fs::create_dir_all(&repo).expect("repository");
    fs::write(repo.join("README.md"), "# Questions\n").expect("readme");
    repo
}

fn engine_for_question_test(repo: &std::path::Path) -> WorkspaceEngine {
    let (mut config, report) =
        config_and_report_for_repo(repo.to_str().expect("repository path")).expect("config");
    // Fixtures are indexed once and discarded; the macOS watcher buys them
    // nothing and costs startup time.
    config.enable_index_watcher = false;
    // The test must not inherit a cap of 0 from the developer's own config.
    config.agent_max_questions_per_turn = 2;
    // `ask_user` is offered only through native tools. The developer's own
    // config may name a provider that does not support them, so the fixture
    // pins one that does rather than depending on the machine.
    let provider_id = config.model_provider.clone();
    let model = config.model_name.clone();
    config
        .model_providers
        .retain(|provider| provider.id != provider_id);
    config
        .model_providers
        .push(workspace_engine::ModelProviderConfig {
            id: provider_id.clone(),
            label: provider_id.clone(),
            base_url: "https://example.test".to_string(),
            api_key_env: "DAMAIAN_TEST_KEY".to_string(),
            models: vec![model],
            supports_native_tools: true,
            max_output_tokens: None,
            context_token_budget: None,
            context_window: None,
            provider_reports_usage: true,
            price_per_million_input_tokens: None,
            price_per_million_output_tokens: None,
            price_per_million_cached_input_tokens: None,
            supports_explicit_cache_breakpoints: false,
        });
    assert!(config.supports_native_tools());
    chat_engine((config, report))
}

fn turn_sink<'a>(
    cancel: &'a CancelToken,
    on_token: &'a mut dyn FnMut(&str),
    on_progress: &'a mut dyn FnMut(TurnProgress),
) -> TurnSink<'a> {
    TurnSink {
        on_token,
        on_progress,
        cancel,
        steering: None,
    }
}

/// A turn paused on a command that needs approval. The command is the
/// marker file every refusal test looks for.
fn paused_on_command(repo: &std::path::Path) -> ChatTurnResult {
    let engine = engine_for_question_test(repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![ToolCall {
            id: "call_marker".to_string(),
            name: "run_command".to_string(),
            arguments_json:
                r#"{"command":"touch should-not-run.txt","reason":"Write a marker file."}"#
                    .to_string(),
        }]],
    );
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = turn_sink(&cancel, &mut on_token, &mut on_progress);
    let result = engine
        .chat_orchestrator
        .ask_with_session_with_options(
            repo,
            "Write the marker",
            &[],
            None,
            &mut adapter,
            &mut sink,
            ChatTurnOptions::interactive(),
        )
        .expect("the command pauses for approval");
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    result
}

/// A turn paused on the model's question.
fn paused_on_question(repo: &std::path::Path) -> ChatTurnResult {
    let engine = engine_for_question_test(repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![ToolCall {
            id: "call_question".to_string(),
            name: "ask_user".to_string(),
            arguments_json:
                r#"{"question":"Did you mean the CLI or the shell?","options":["The CLI","The shell"],"allowFreeText":true}"#
                    .to_string(),
        }]],
    );
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = turn_sink(&cancel, &mut on_token, &mut on_progress);
    let result = engine
        .chat_orchestrator
        .ask_with_session_with_options(
            repo,
            "Which one?",
            &[],
            None,
            &mut adapter,
            &mut sink,
            ChatTurnOptions::interactive(),
        )
        .expect("the turn pauses on a question");
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    assert!(result.question_proposal.is_some());
    result
}

#[test]
fn a_reopened_session_carries_the_question_and_answer_messages_in_order() {
    // Requirement 3's reload half. `read_messages` already yields the two
    // roles; the card also needs the options it was asked with, which the
    // role alone cannot carry.
    isolated_data_dir();
    let engine = default_engine().expect("default engine");
    let session = engine
        .session_store
        .create_session("repo_question_transcript", "question transcript")
        .expect("session");
    let question = PendingQuestionRef::new();
    engine
        .session_store
        .record_question_asked(
            &session.id,
            "task_question",
            &question,
            &AskUserInput {
                question: "Did you mean the CLI or the shell?".to_string(),
                options: vec!["The CLI".to_string(), "The shell".to_string()],
                allow_free_text: true,
            },
        )
        .expect("question recorded");
    engine
        .session_store
        .record_question_answered(&session.id, question.question_id(), "The CLI", Some(0))
        .expect("answer recorded");

    let (port, token) = serve_for_test();
    let response = get_session_for_test(port, &token, &session.id);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let payload = json_of(&response);
    let messages = payload["messages"].as_array().expect("messages");
    let roles: Vec<&str> = messages
        .iter()
        .map(|message| message["role"].as_str().expect("role"))
        .collect();
    assert_eq!(
        roles,
        vec!["question", "answer"],
        "a reload must show the exchange in the order it happened: {payload}"
    );
    assert_eq!(messages[0]["id"], question.question_id());
    assert_eq!(messages[0]["content"], "Did you mean the CLI or the shell?");
    assert_eq!(messages[0]["options"][0], "The CLI");
    assert_eq!(messages[0]["options"][1], "The shell");
    assert_eq!(messages[0]["allowFreeText"], true);
    assert_eq!(messages[1]["content"], "The CLI");
}

#[test]
fn the_answer_route_refuses_a_command_proposal_id() {
    // Requirement 2 at the shell's answer route: a command proposal id can
    // never be answered as though an answer were consent. Nothing runs and
    // the command is still waiting for its own decision.
    let repo = question_repo("answer-route-command-id");
    let paused = paused_on_command(&repo);
    let proposal_id = paused
        .command_proposal
        .as_ref()
        .expect("a command proposal")
        .id
        .clone();
    let (events, _receiver) = std::sync::mpsc::channel::<TurnEvent>();
    let error = run_answer_question_request(
        &form_fields(&[
            ("repo", repo.to_str().expect("repository path")),
            ("question_id", &proposal_id),
            ("answer", "Yes"),
            ("selected_option", ""),
        ]),
        &CancelToken::new(),
        &events,
    )
    .expect_err("a command proposal id is not answerable");
    assert!(
        error.contains("not a question"),
        "the route must say what is wrong: {error}"
    );
    assert!(
        !repo.join("should-not-run.txt").exists(),
        "answering a command id must run nothing"
    );
    let engine = engine_for_repo(repo.to_str().expect("repository path")).expect("engine");
    assert!(
        engine
            .chat_orchestrator
            .has_pending_chat_command(&proposal_id),
        "the command is still waiting for its own decision"
    );
    fs::remove_dir_all(repo).ok();
}

#[test]
fn run_command_with_a_question_id_runs_nothing() {
    // Requirement 2's other direction: a question id reaching the command
    // route is not an approval, and there is no proposal behind it to run.
    let repo = question_repo("run-command-question-id");
    let paused = paused_on_question(&repo);
    let question_id = paused
        .question_proposal
        .as_ref()
        .expect("a question")
        .question_id
        .clone();
    let (port, token) = serve_for_test();
    let response = post_fields_for_test(
        port,
        &token,
        "/api/run-command",
        &[
            ("repo", repo.to_str().expect("repository path")),
            ("proposal_id", &question_id),
        ],
    );
    assert!(
        !response.starts_with("HTTP/1.1 200"),
        "a question id is not a command proposal: {response}"
    );
    assert!(
        !repo.join("should-not-run.txt").exists(),
        "nothing may run for a question id"
    );
    let engine = engine_for_repo(repo.to_str().expect("repository path")).expect("engine");
    assert!(
        !engine
            .chat_orchestrator
            .has_pending_chat_command(&question_id),
        "a question is never a pending command"
    );
    fs::remove_dir_all(repo).ok();
}

#[test]
fn the_chat_result_carries_a_plan_put_up_for_review() {
    // Task 11's gate stops the turn with a `plan_proposal`; without this
    // field the frontend has no way to know it happened, and the turn
    // reads as an ordinary answer that mysteriously did nothing.
    let json = plan_proposal_json(Some(&AgentPlanProposal {
        id: "planreview_1".to_string(),
        plan: sample_plan(),
        deferred_action: "Preparing a patch".to_string(),
        budget_review: None,
    }));
    assert!(
        json.contains("\"proposalId\":\"planreview_1\""),
        "got {json}"
    );
    assert!(json.contains("\"deferredAction\":\"Preparing a patch\""));
    assert!(json.contains("\"title\":\"Add a bounded backoff\""));
    // Spec 103 Task 2: an inline review offered no budget, so the key is
    // absent — exactly the pre-budget shape, not `budgetReview:null`.
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    let keys: Vec<&str> = value
        .as_object()
        .expect("the proposal is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec!["deferredAction", "plan", "proposalId"],
        "an inline review keeps its old JSON shape"
    );
    assert_eq!(plan_proposal_json(None), "null");
}

#[test]
fn the_plan_card_transports_the_engine_budget_review_verbatim() {
    // Spec 103 Task 2: the desktop is transport. The exact review object the
    // engine built, including labels and units, is what the card receives.
    let review = TaskBudgetReview::from_policy(&Config::default().task_budget_policy());
    let json = plan_proposal_json(Some(&AgentPlanProposal {
        id: "planreview_budget".to_string(),
        plan: sample_plan(),
        deferred_action: "Preparing a patch".to_string(),
        budget_review: Some(review.clone()),
    }));
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(
        value["budgetReview"],
        serde_json::to_value(&review).expect("the review serializes")
    );
    let keys: Vec<&str> = value
        .as_object()
        .expect("the proposal is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec!["budgetReview", "deferredAction", "plan", "proposalId"],
        "a worker review adds exactly the budget key"
    );
}

/// Spec 108 D6: the plan card shows the steps and their checks as text;
/// the plan is changed through the conversation. Decline opens the
/// question and sends nothing until it is answered.
#[test]
fn the_plan_card_is_read_only_and_decline_asks_how_to_change_it() {
    let card = app_js_function("createPlanReview");
    for editor in [
        "input.className = \"plan-review-title\"",
        "Move step",
        "Remove this step",
        "planCheckControl",
        "Add a required check",
        "steps: revision",
        "command-approval-risk",
        "\"↑\"",
        "\"↓\"",
        "\"×\"",
    ] {
        assert!(!card.contains(editor), "editing control left: {editor}");
    }
    assert!(
        !super::APP_JS.contains("function planCheckControl("),
        "the check editor is gone"
    );
    // Checks are text, the deferred action a quiet line.
    assert!(card.contains("planCheckTitle(step.check)"), "{card}");
    assert!(
        card.contains("`Next: ${proposal.deferredAction}`"),
        "{card}"
    );

    assert!(
        card.contains("\"How should the plan be changed?\""),
        "{card}"
    );
    assert!(card.contains("\"Send\""), "{card}");
    assert!(card.contains("\"Decline without changes\""), "{card}");
    assert!(
        card.contains("changes: approved ? \"\" : changesText"),
        "{card}"
    );
    // Decline only opens the question; a worker's card keeps the plain
    // decline because its route takes no changes.
    let decline = card
        .split("declineButton.addEventListener(\"click\"")
        .nth(1)
        .and_then(|rest| rest.split("});").next())
        .expect("the decline handler");
    assert!(decline.contains("changes.hidden = false"), "{decline}");
    assert!(decline.contains("if (decideInWorker)"), "{decline}");

    let css = include_str!("../static/style.css");
    for gone in [".plan-review-title", ".plan-review-check-input"] {
        assert!(!css.contains(gone), "{gone} is styled still");
    }
    // Its `display: grid` must not defeat `hidden` before Decline.
    assert!(css.contains(".plan-review-changes[hidden]"), "{css}");
}

/// Spec 108 D6: `changes` answers a decline. With an approval the request
/// would be two decisions at once, so it is refused before anything runs.
#[test]
fn the_resume_plan_route_refuses_changes_with_an_approval() {
    let form: HashMap<String, String> = [
        ("repo", "/nonexistent-damaian-repo"),
        ("proposal_id", "planreview_x"),
        ("approved", "true"),
        ("changes", "Skip the docs step"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    let (events, _discard) = std::sync::mpsc::channel();
    let error = super::run_resume_plan_request(&form, &CancelToken::new(), &events)
        .expect_err("changes with an approval");
    assert!(error.contains("sent with a decline"), "{error}");
}

#[test]
fn the_plan_card_renders_budget_rows_without_dimension_logic() {
    // The JS must not map dimensions to labels, units or policy. It shows
    // the rows the engine sends and posts the typed budget back untouched.
    let app = include_str!("../static/app.js");
    let card = app
        .split("function createPlanReview")
        .nth(1)
        .and_then(|rest| rest.split("function createPatchPreview").next())
        .expect("the plan review card");
    assert!(card.contains("budgetReview.rows"), "{card}");
    assert!(card.contains("row.label"), "{card}");
    assert!(card.contains("row.unit"), "{card}");
    assert!(
        card.contains("JSON.stringify(proposal.budgetReview.budget)"),
        "{card}"
    );
    for forbidden in [
        "action rounds",
        "exploration rounds",
        "actionRounds",
        "explorationRounds",
        "inputTokens",
        "outputTokens",
    ] {
        assert!(
            !card.contains(forbidden),
            "the plan card hard-codes budget policy: {forbidden}"
        );
    }
}

#[test]
fn the_relay_cancels_the_turn_once_the_client_stops_listening() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = CancelToken::new();
    // Sends for long enough that the write failure happens well before the
    // channel disconnects, which is what ends the relay here.
    std::thread::spawn(move || {
        for _ in 0..20 {
            if sender.send(TurnEvent::Token("x".to_string())).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });

    let mut out = FlakyWriter {
        written: Vec::new(),
        writes_before_failure: 2,
    };
    relay_turn_events(&mut out, &cancel, None, receiver, Duration::from_millis(25)).expect("relay");

    assert!(
        cancel.is_cancelled(),
        "a dead client must stop the turn, not just stop the writes"
    );
}

#[test]
fn legacy_run_scenario_receives_steps_without_actions() {
    let call = workspace_engine::WebDiagnosticCall::from_tool_call(
        "run_web_scenario",
        r##"{
                "url":"http://localhost:5001/",
                "viewport":{"width":1280,"height":720},
                "actions":[
                    {"action":"fill","selector":"#username","value":"tester"},
                    {"action":"click","selector":"#register"}
                ],
                "capture":{"screenshot":true}
            }"##,
    )
    .expect("valid scenario")
    .expect("web diagnostic call");

    let arguments = mcp_browser_arguments("run_scenario", &call).expect("arguments");
    let value: serde_json::Value = serde_json::from_str(&arguments).expect("JSON arguments");

    assert!(value.get("actions").is_none(), "got {value}");
    assert_eq!(
        value["steps"],
        serde_json::json!([
            {"action":"goto","url":"http://localhost:5001/"},
            {"action":"fill","selector":"#username","value":"tester"},
            {"action":"click","selector":"#register"}
        ])
    );
    assert_eq!(value["viewport"]["width"], 1280);
    assert_eq!(value["capture"]["screenshot"], true);
}

#[test]
fn native_run_web_scenario_keeps_the_damaian_actions_contract() {
    let call = workspace_engine::WebDiagnosticCall::from_tool_call(
        "run_web_scenario",
        r##"{"url":"http://localhost:5001/","actions":[{"action":"click","selector":"#register"}]}"##,
    )
    .expect("valid scenario")
    .expect("web diagnostic call");

    assert_eq!(
        mcp_browser_arguments("run_web_scenario", &call).expect("arguments"),
        call.arguments_json
    );
}

/// Answers every diagnostic with a companion report whose screenshot is a
/// real file under `/…/runs/`, shaped by the same function the MCP runner
/// uses, so the test sees exactly what the engine would.
struct CompanionShapedRunner {
    text: String,
    data_dir: PathBuf,
}

impl workspace_engine::WebDiagnosticsRunner for CompanionShapedRunner {
    fn inspect(
        &self,
        call: &workspace_engine::WebDiagnosticCall,
    ) -> workspace_engine::Result<workspace_engine::WebDiagnosticReport> {
        Ok(browser_report_from_tool_result(
            workspace_engine::WebDiagnosticReport::from_text(self.text.clone(), false),
            call,
            &self.data_dir,
            "MCP server `browser` tool `inspect_page`",
        ))
    }

    fn run_scenario(
        &self,
        call: &workspace_engine::WebDiagnosticCall,
    ) -> workspace_engine::Result<workspace_engine::WebDiagnosticReport> {
        self.inspect(call)
    }
}

#[test]
fn a_structured_browser_report_is_rendered_and_lists_the_materialised_artifact() {
    let repo = temp_path("browser-artifact-e2e");
    let runs = repo.join("runs");
    fs::create_dir_all(&runs).unwrap();
    fs::write(repo.join("README.md"), "# Web app\n").unwrap();
    let screenshot = runs.join("20260925-1-page.png");
    fs::write(&screenshot, b"\x89PNG\r\n\x1a\n").unwrap();
    let screenshot = screenshot.to_string_lossy().to_string();
    let text = format!(
        r#"{{"final_url": "http://localhost:5001/", "title": "Snake Game", "status": 200,
                "page_errors": ["ReferenceError: boom"],
                "artifacts": ["{screenshot}"],
                "artifact_metadata": [{{"kind": "screenshot", "path": "{screenshot}",
                  "mime_type": "image/png", "width": 1280, "height": 720}}]}}"#
    );

    let mut config = Config {
        data_dir: isolated_data_dir().to_path_buf(),
        enable_index_watcher: false,
        ..Config::default()
    };
    config
        .model_providers
        .push(workspace_engine::ModelProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            base_url: String::new(),
            api_key_env: String::new(),
            models: Vec::new(),
            supports_native_tools: true,
            max_output_tokens: None,
            context_token_budget: None,
            context_window: None,
            provider_reports_usage: true,
            price_per_million_input_tokens: None,
            price_per_million_output_tokens: None,
            price_per_million_cached_input_tokens: None,
            supports_explicit_cache_breakpoints: false,
        });
    let mut engine = WorkspaceEngine::new(config);
    engine.chat_orchestrator.set_web_diagnostics_runner(
        workspace_engine::WebDiagnosticsRunnerHandle::new(CompanionShapedRunner {
            text,
            data_dir: isolated_data_dir().to_path_buf(),
        }),
    );
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Found it.".to_string()],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "inspect_web_page".to_string(),
                arguments_json: r#"{"url":"http://localhost:5001/"}"#.to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};
    let session_id = engine
        .chat_orchestrator
        .ask(&repo, "Inspect the page.", &[], &mut adapter, &mut on_token)
        .unwrap()
        .session
        .id;

    let result = engine
        .session_store
        .read_messages(&session_id)
        .unwrap()
        .into_iter()
        .find(|message| message.role == "tool")
        .expect("the diagnostic's tool result")
        .content;
    assert!(
        result.starts_with("Browser diagnostic found 1 page error."),
        "{result}"
    );
    assert!(
        !result.contains("Browser diagnostic result via"),
        "{result}"
    );
    assert!(
        result.contains("- Source: MCP server `browser` tool `inspect_page`"),
        "{result}"
    );
    let listed = result
        .split("\n\nArtifacts:\n")
        .nth(1)
        .unwrap_or_else(|| panic!("an Artifacts: list in {result}"));
    let prefix = format!("- screenshot: web-diagnostics/{session_id}/");
    assert!(listed.starts_with(&prefix), "{listed}");
    assert!(
        listed.ends_with("/20260925-1-page.png (1280x720)"),
        "{listed}"
    );
    assert!(
        !result.contains(&screenshot),
        "the companion's path leaked: {result}"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn the_relay_reports_a_failed_turn_as_an_error_event() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = CancelToken::new();
    sender
        .send(TurnEvent::Failed("provider exploded".to_string()))
        .unwrap();
    drop(sender);

    let mut out = FlakyWriter {
        written: Vec::new(),
        writes_before_failure: usize::MAX,
    };
    relay_turn_events(&mut out, &cancel, None, receiver, Duration::from_millis(25)).expect("relay");

    let text = String::from_utf8_lossy(&out.written);
    assert!(text.contains("event: error"), "got {text:?}");
    assert!(text.contains("provider exploded"), "got {text:?}");
}

#[test]
fn decodes_forms() {
    let form = parse_form("repo=%2Ftmp%2Fapp&prompt=hello+world");
    assert_eq!(form.get("repo").unwrap(), "/tmp/app");
    assert_eq!(form.get("prompt").unwrap(), "hello world");
}

#[test]
fn parses_selected_patch_paths() {
    assert_eq!(
        parse_path_list("src/a.js\nsrc/b.js|src/c.js").unwrap(),
        vec!["src/a.js", "src/b.js", "src/c.js"]
    );
    assert!(parse_path_list(" \n ").is_err());
}

#[test]
fn serializes_generated_secret_warnings_for_the_patch_ui() {
    let json = generated_secret_warnings_json(&[GeneratedSecretWarning {
        path: "docs/\"README\".md".to_string(),
        categories: vec!["credential_assignment".to_string()],
        count: 2,
    }]);

    assert_eq!(
        json,
        "{\"path\":\"docs/\\\"README\\\".md\",\"count\":2,\"categories\":[\"credential_assignment\"]}"
    );
}

#[test]
fn renders_absent_allowlist_path_as_json_null() {
    // `null` rather than `""`, so the UI can tell "no permanent allowance
    // was granted" apart from "granted, path unknown".
    assert_eq!(json_optional_string(None), "null");
    assert_eq!(
        json_optional_string(Some(std::path::Path::new("/tmp/repo/.damaian/config.conf"))),
        "\"/tmp/repo/.damaian/config.conf\""
    );
}

#[test]
fn percent_decodes_invalid_hex_literally() {
    assert_eq!(percent_decode("a%zz"), "a%zz");
}

#[test]
fn percent_decodes_malformed_unicode_adjacent_escape_literally() {
    assert_eq!(percent_decode("%aé"), "%aé");
}

#[test]
fn validates_desktop_api_token_header() {
    let request = test_request_with_headers(&[("x-damaian-api-token", "secret")]);

    assert!(require_api_token(&request, "secret").is_ok());
    assert!(require_api_token(&request, "wrong").is_err());
}

#[test]
fn http_server_never_serves_desktop_api_token() {
    let options = ShellOptions::new(0, Some("/tmp/damaian-repo".to_string()));
    let token = options.api_token.clone();

    let bare_bootstrap_request = test_request("/api/bootstrap", &[]);
    assert!(api_request_requires_token(&bare_bootstrap_request.path));
    assert!(require_api_token(&bare_bootstrap_request, &token).is_err());

    let first_page = index_html();
    assert!(!first_page.contains(&token));
    assert!(!first_page.contains("data-api-token"));
    assert!(!first_page.contains("data-default-repo"));
    assert!(!first_page.contains("/tmp/damaian-repo"));

    let second_page = index_html();
    assert_eq!(first_page, second_page);
    assert!(!second_page.contains(&token));

    let authenticated_bootstrap_request =
        test_request("/api/bootstrap", &[("x-damaian-api-token", &token)]);
    assert!(require_api_token(&authenticated_bootstrap_request, &token).is_ok());
}

/// Actually launches Finder, so this is excluded from normal `cargo
/// test` runs (same reasoning as there being no test for
/// `open_in_vscode`/`launch_vscode`, which also has a real side effect).
/// Run manually with `cargo test -p desktop-shell -- --ignored
/// reveal_in_finder_endpoint_opens_the_requested_repository_root` to
/// verify end-to-end.
#[test]
#[ignore]
fn reveal_in_finder_endpoint_opens_the_requested_repository_root() {
    let options = ShellOptions::new(0, None);
    let token = options.api_token.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let port = listener.local_addr().expect("local addr").port();
    spawn_with_shell_stack("test-shell", move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let _ = handle_connection(&mut stream, &options);
        }
    })
    .expect("spawn the test shell");

    let repo = std::env::temp_dir();
    let body = format!("repo={}", repo.to_string_lossy());
    let request = format!(
        "POST /api/reveal-in-finder HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");

    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected response: {response}"
    );
}

/// Actually launches Finder, so this is excluded from normal `cargo
/// test` runs, like `reveal_in_finder_endpoint_opens_the_requested_repository_root`.
/// Run manually with `cargo test -p desktop-shell -- --ignored
/// reveal_web_diagnostic_artifact_endpoint_selects_the_file_in_finder` to
/// verify end-to-end. It writes only under the isolated temp data dir.
#[test]
#[ignore]
fn reveal_web_diagnostic_artifact_endpoint_selects_the_file_in_finder() {
    let data_dir = isolated_data_dir();
    let relative = "web-diagnostics/session_reveal/task_reveal/run-1/page.png";
    let file = data_dir.join(relative);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, b"png").unwrap();
    let (port, token) = serve_for_test();

    let repo = std::env::temp_dir();
    let body = format!("repo={}&path={relative}", repo.to_string_lossy());
    let response = send_for_test(
        port,
        format!(
            "POST /api/reveal-web-diagnostic-artifact HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ),
    );

    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected response: {response}"
    );
    assert!(response.contains("page.png"), "{response}");
}

/// The reported bug end to end: `apply selected` on flagged content used
/// to fail with a `policy_blocked` error the user could not get past. The
/// route must instead report *what* was found without writing anything,
/// then apply the same selection once the user consents.
#[test]
fn apply_patch_endpoint_warns_then_applies_when_the_user_accepts() {
    let repo = std::env::temp_dir().join(format!(
        "damaian-apply-secret-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(repo.join(".damaian")).unwrap();
    isolated_data_dir();
    fs::write(repo.join("config.js"), "export const token = \"\";\n").unwrap();

    let repo_arg = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
    let patch = engine
        .patch_engine
        .create_patch(
            &repo,
            &[workspace_engine::ProposedChange {
                path: "config.js".to_string(),
                new_content: "export const api_key = \"sk_live_9f8a7b6c5d4e3f2a1b0c\";\n"
                    .to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "add key",
        )
        .expect("create patch");
    engine.patch_store.save(&patch).expect("store patch");

    let options = ShellOptions::new(0, None);
    let token = options.api_token.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let port = listener.local_addr().expect("local addr").port();
    spawn_with_shell_stack("test-shell", move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let _ = handle_connection(&mut stream, &options);
        }
    })
    .expect("spawn the test shell");

    let post = |body: String| {
        let request = format!(
            "POST /api/apply-patch HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
        stream.write_all(request.as_bytes()).expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    };

    let base = format!(
        "repo={}&patch_id={}",
        percent_encode_for_test(&repo_arg),
        patch.id
    );

    // First attempt: warned, nothing written.
    let warned = post(base.clone());
    assert!(warned.starts_with("HTTP/1.1 200"), "{warned}");
    assert!(warned.contains("\"blockedBySecrets\""), "{warned}");
    assert!(warned.contains("config.js"), "{warned}");
    assert!(warned.contains("credential_assignment"), "{warned}");
    assert!(warned.contains("\"appliedFiles\":[]"), "{warned}");
    assert_eq!(
        fs::read_to_string(repo.join("config.js")).unwrap(),
        "export const token = \"\";\n",
        "a warned apply must not write anything"
    );
    // The response must name the category, never the matched value.
    assert!(!warned.contains("sk_live_9f8a7b6c5d4e3f2a1b0c"), "{warned}");

    // Second attempt: the user accepted.
    let accepted = post(format!("{base}&allow_secrets=1"));
    assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
    assert!(!accepted.contains("\"blockedBySecrets\""), "{accepted}");
    assert!(
        accepted.contains("\"appliedFiles\":[\"config.js\"]"),
        "{accepted}"
    );
    assert_eq!(
        fs::read_to_string(repo.join("config.js")).unwrap(),
        "export const api_key = \"sk_live_9f8a7b6c5d4e3f2a1b0c\";\n"
    );

    fs::remove_dir_all(&repo).ok();
}

/// A turn paused on a model question, for looking at spec 107's reload
/// placement: the question card below everything else, the composer line
/// and the disabled composer.
fn pending_question_session(engine: &WorkspaceEngine, repository_id: &str) -> String {
    let store = &engine.session_store;
    let session = store
        .create_session(repository_id, "Pick a retry policy")
        .unwrap();
    let task = store
        .create_task(&session.id, "add a retry policy", "mock", "mock")
        .unwrap();
    store
        .append_message(&session.id, Some(&task.id), "user", "add a retry policy")
        .unwrap();
    store
        .append_message(
            &session.id,
            Some(&task.id),
            "assistant",
            "Before I add it I need one decision from you.",
        )
        .unwrap();
    let question = PendingQuestionRef::new();
    store
        .record_question_asked(
            &session.id,
            &task.id,
            &question,
            &AskUserInput {
                question: "Exponential or fixed backoff?".to_string(),
                options: vec!["Exponential".to_string(), "Fixed".to_string()],
                allow_free_text: true,
            },
        )
        .unwrap();
    store.await_answer(&task, &question).unwrap();
    session.id
}

/// Spec 107 D3 (C): the worker suggestion is drawn only where the engine's
/// `workerSuggestion` names the card, with the engine's labels; the plan
/// card no longer offers spec 94's always-on **Run in a worker**.
#[test]
fn app_js_suggests_a_worker_only_where_the_engine_says() {
    let proposals = app_js_function("appendProposals");
    assert!(
        proposals.contains("payload.turnPresentation?.workerSuggestion"),
        "{proposals}"
    );
    assert!(
        proposals.contains("suggestionFor(\"plan\", payload.planProposal.proposalId)"),
        "{proposals}"
    );
    assert!(
        proposals.contains("suggestionFor(\"patch\", payload.patchProposal.patchId)"),
        "{proposals}"
    );
    assert!(
        proposals.contains("suggestion.cardId === cardId"),
        "{proposals}"
    );

    let plan = app_js_function("createPlanReview");
    assert!(
        !plan.contains("\"Run in a worker\""),
        "the plan card has no always-on worker button: {plan}"
    );
    assert!(
        plan.contains("workerSuggestion && !decideInWorker && workersEnabled"),
        "{plan}"
    );
    assert!(plan.contains("\"/api/plan-to-worker\""), "{plan}");

    let row = app_js_function("createWorkerSuggestion");
    for engine_label in [
        "suggestion.title",
        "suggestion.note",
        "suggestion.runLabel",
        "suggestion.continueLabel",
        "prompt: suggestion.prompt",
    ] {
        assert!(row.contains(engine_label), "{engine_label}: {row}");
    }
    assert!(row.contains("showNewWorkerDialog"), "{row}");
    assert!(
        !row.contains("Run in a worker") && !row.contains("Continue here"),
        "labels come from the engine: {row}"
    );
}

/// Spec 108 D7: one decision at a time. The plan card renders Approve and
/// Decline only; Approve with the engine's suggestion swaps the actions for
/// it (Continue here sends the approval, Run in a worker hands the plan
/// over named by its review), Approve without one resumes directly, and
/// Decline never shows it. At a patch the suggestion is its own card,
/// before the patch card, which is hidden until the user has answered.
#[test]
fn the_plan_card_asks_one_decision_at_a_time() {
    let card = app_js_function("createPlanReview");
    assert_eq!(
        card.matches("createWorkerSuggestion(").count(),
        1,
        "the suggestion is built in one place: {card}"
    );
    assert!(
        !card.contains("wrapper.append(suggestionRow")
            && !card.contains("if (suggestionRow) wrapper.append"),
        "never rendered with the card's own actions: {card}"
    );
    let approve = card
        .split("approveButton.addEventListener(\"click\"")
        .nth(1)
        .and_then(|rest| rest.split("\n  });").next())
        .expect("the approve handler");
    assert!(approve.contains("if (!offersWorker)"), "{approve}");
    assert!(approve.contains("run(true)"), "{approve}");
    assert!(
        approve.contains("createWorkerSuggestion(workerSuggestion"),
        "{approve}"
    );
    assert!(
        approve.contains("actions.replaceWith(suggestionRow)"),
        "{approve}"
    );
    assert!(
        approve.contains("onContinue: () => decide(true)"),
        "{approve}"
    );
    assert!(approve.contains("onRun: runInWorker"), "{approve}");
    for handler in ["declineButton", "sendButton", "declineOnlyButton"] {
        let body = card
            .split(&format!("{handler}.addEventListener(\"click\""))
            .nth(1)
            .and_then(|rest| rest.split("});").next())
            .expect("a decline handler");
        assert!(
            !body.contains("Suggestion") && !body.contains("suggestion"),
            "{handler} never offers a worker: {body}"
        );
    }
    assert!(
        card.contains("planProposalId: proposal.proposalId"),
        "the handoff names the approved review: {card}"
    );
    assert!(
        !card.contains("next.title"),
        "no tooltip over the Next line: {card}"
    );
    let dialog = app_js_function("showNewWorkerDialog");
    assert!(
        dialog.contains("plan_proposal_id: planProposalId"),
        "{dialog}"
    );

    let row = app_js_function("createWorkerSuggestion");
    assert!(row.contains("if (onContinue) await onContinue()"), "{row}");
    assert!(row.contains("onHanded?.(started)"), "{row}");

    let proposals = app_js_function("appendProposals");
    assert!(!proposals.contains("patchCard.prepend("), "{proposals}");
    assert!(proposals.contains("patchCard.hidden = true"), "{proposals}");
    assert!(
        proposals.contains("steps.append(suggestionCard, patchCard)"),
        "the suggestion precedes the patch card: {proposals}"
    );
    assert!(proposals.contains("onContinue: showPatch"), "{proposals}");
    assert!(proposals.contains("onHanded: showPatch"), "{proposals}");
    assert!(
        proposals.contains("place(\"patchProposal\", patchCard, steps)"),
        "the patch card stays the pending decision's card: {proposals}"
    );
    let css = include_str!("../static/style.css");
    assert!(css.contains(".worker-suggestion-steps > [hidden]"), "{css}");
}

/// Spec 112 requirement 8: one button scale and one icon-button class. A
/// class with no rule (`btn-secondary`) rendered as an unstyled browser
/// button, and an emoji drifted from the drawn icon set.
#[test]
fn static_assets_use_one_button_scale_and_drawn_icons() {
    for (name, text) in [
        ("app.js", super::APP_JS),
        ("index.html", super::INDEX_HTML),
        ("style.css", super::STYLE_CSS),
    ] {
        assert!(
            !text.contains("btn-secondary"),
            "{name} names btn-secondary"
        );
        assert!(!text.contains("icon-button"), "{name} names icon-button");
        assert!(
            !text.contains('📌'),
            "{name} labels a control with an emoji"
        );
    }
    assert!(
        !super::STYLE_CSS.contains("var(--accent-soft,"),
        "--accent-soft is defined, so a fallback can only drift off-theme"
    );
}

/// Spec 108 D3, the UI half: Archive in both session menus and the
/// worker's, archived sessions out of the sidebar list and into a
/// collapsed Archived (N) entry with Open and Unarchive, search labelling
/// archived hits, and the board hiding archived workers by default. The
/// route's own refusal message reaches the user through the toast.
#[test]
fn app_js_archives_sessions_out_of_the_sidebar() {
    let item = app_js_function("sessionArchiveMenuItem");
    assert!(
        item.contains("archived ? \"Unarchive\" : \"Archive\""),
        "{item}"
    );
    assert!(
        item.contains("setSessionArchived(projectPath, session, !archived)"),
        "{item}"
    );
    for menu in ["toggleSessionMenu", "toggleThreadMenu", "toggleWorkerMenu"] {
        let body = app_js_function(menu);
        assert!(body.contains("sessionArchiveMenuItem("), "{menu}: {body}");
        // Delete stays, and stays the destructive one.
        assert!(body.contains("action: \"delete\""), "{menu}: {body}");
    }
    assert!(
        app_js_function("runSessionAction").contains("toast(error.message)"),
        "an archive refusal is shown as the route's message"
    );

    let set = app_js_function("setSessionArchived");
    assert!(set.contains("\"/api/session-archive\""), "{set}");
    assert!(set.contains("\"/api/session-unarchive\""), "{set}");

    let list = app_js_function("renderProjectList");
    assert!(
        list.contains("sessions.filter((session) => !session.archived)"),
        "{list}"
    );
    assert!(
        list.contains("(workers || []).filter((row) => !row.archived)"),
        "{list}"
    );

    // Moved into the project's own ⋮ menu (2026-11-13): the Archived
    // entry no longer sits as its own row in the session list, even
    // collapsed. Opening one is still Open; unarchiving it is the thread
    // header's own menu, which `sessionArchiveMenuItem` already covers,
    // so the drilled-in row itself only opens the session.
    assert!(
        !list.contains("Archived ("),
        "the session list must not render the Archived entry inline: {list}"
    );
    let project_menu = app_js_function("toggleProjectMenu");
    assert!(
        project_menu.contains("archivedProjectMenuItem(projectPath)"),
        "{project_menu}"
    );
    let item = app_js_function("archivedProjectMenuItem");
    assert!(item.contains("`Archived (${entries.length})`"), "{item}");
    assert!(
        item.contains("hidden: entries.length === 0"),
        "an empty project shows no Archived row: {item}"
    );
    assert!(
        item.contains("if (!sessions && !workers) return { label: \"Archived\", hidden: true };"),
        "a project never expanded this session has no count to show: {item}"
    );
    let row = app_js_function("archivedProjectMenuRow");
    assert!(row.contains("loadSession(id)"), "{row}");
    assert!(
        !row.contains("\"Unarchive\""),
        "unarchiving an opened archived session is the thread menu's job, not this row's: {row}"
    );

    let search = app_js_function("renderSessionSearchResults");
    assert!(
        search.contains("archivedIds.has(hit.sessionId)"),
        "{search}"
    );
    assert!(search.contains("\"Archived\""), "{search}");

    let board = app_js_function("renderWorkersBoard");
    assert!(
        board.contains("workersBoardShowArchived ? shown : shown.filter((row) => !row.archived)"),
        "{board}"
    );
    assert!(
        board.contains("`Show archived (${archivedCount})`"),
        "{board}"
    );
    assert!(
        super::APP_JS.contains("let workersBoardShowArchived = false;"),
        "the board hides archived workers by default"
    );
}

/// The body of the top-level `function <name>(` in `app.js`, up to the
/// next top-level declaration.
/// The effort menu offers a provider exactly the levels its request
/// carries. A level the menu shows but the engine drops was the original
/// defect: DeepSeek offered five efforts and sent none of them.
#[test]
fn app_js_effort_levels_match_what_the_engine_sends() {
    let app = super::APP_JS;
    let start = app
        .find("const providerReasoningLevels = {")
        .expect("app.js has the providerReasoningLevels table");
    let table = &app[start..start + app[start..].find("\n};").unwrap()];
    for provider in ["openai", "openai-compatible", "deepseek", "openrouter"] {
        let key = if provider.contains('-') {
            format!("\"{provider}\": [")
        } else {
            format!("{provider}: [")
        };
        let line = table
            .lines()
            .find(|line| line.trim_start().starts_with(&key))
            .unwrap_or_else(|| panic!("app.js offers no levels for {provider}"));
        let listed = line[line.find('[').unwrap() + 1..line.find(']').unwrap()]
            .split(',')
            .map(|level| level.trim().trim_matches('"'))
            .filter(|level| !level.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(
            listed,
            workspace_engine::supported_reasoning_levels(provider),
            "{provider}"
        );
    }
}

/// Answering a decision clears it at once: the composer stopped saying
/// "Approve the command" only when the resumed turn ended, while the
/// approved command ran for minutes. Every resume goes through
/// `resumeDecision`, which settles the pending decision first.
#[test]
fn app_js_settles_a_decision_as_soon_as_it_is_answered() {
    assert!(
        app_js_function("resumeDecision").contains("settlePendingDecision();"),
        "resumeDecision must settle the pending decision before streaming"
    );
    let resumes = super::APP_JS
        .match_indices("\nasync function streamResume")
        .map(|(start, _)| {
            let name = &super::APP_JS[start + "\nasync function ".len()..];
            &name[..name.find('(').unwrap()]
        })
        .collect::<Vec<_>>();
    assert!(resumes.len() >= 8, "found {resumes:?}");
    for name in resumes {
        let body = app_js_function(name);
        assert!(
            body.contains("resumeDecision(") && !body.contains("streamRequest("),
            "{name} must resume through resumeDecision"
        );
    }
    let command = app_js_function("createCommandApprovalPreview");
    assert!(command.contains("actions.hidden = true;"));
    assert!(command.contains("actions.hidden = false;"));
}

/// The page itself never scrolls in the wide layout. `overflow: hidden` on
/// the body still let `scrollIntoView` scroll it whenever anything poked
/// past the window, which hid the header and the top of the sidebar
/// (2026-11-12). `.app` is pinned to the window and clips, and only the
/// narrow layout, which scrolls the page on purpose, undoes that.
#[test]
fn the_wide_layout_pins_the_app_to_the_window() {
    let css = super::STYLE_CSS;
    let start = css.find("\n.app {").expect("style.css has an .app rule");
    let rule = &css[start..start + css[start..].find('}').unwrap()];
    for declaration in ["position: fixed;", "inset: 0;", "overflow: clip;"] {
        assert!(
            rule.contains(declaration),
            "`.app` lacks `{declaration}`: {rule}"
        );
    }
    let narrow = &css[css.find("@media (max-width: 1100px)").unwrap()..];
    let narrow_app = &narrow[narrow.find(".app {").unwrap()..];
    let narrow_app = &narrow_app[..narrow_app.find('}').unwrap()];
    assert!(narrow_app.contains("position: static;"), "{narrow_app}");
    assert!(narrow_app.contains("overflow: visible;"), "{narrow_app}");
}

/// The commit panel is a full-viewport panel, not a small centered card;
/// a file's diff renders expanded and colored, through the same
/// `renderColoredDiff` a proposed patch uses, not a plain-text `<pre>`
/// behind a disclosure; hooks and checks are collapsed into one
/// secondary disclosure so they do not compete with the diffs for space;
/// the redaction note stays outside any disclosure (spec 35 §5.4).
#[test]
fn app_js_shows_the_commit_review_as_a_full_panel_with_expanded_diffs() {
    let show = app_js_function("showCommitDialog");
    assert!(
        show.contains("renderColoredDiff(candidate.diff)"),
        "a file's diff must use the colored renderer: {show}"
    );
    assert!(
        !show.contains("<details") || !show.contains("Diff (redacted)"),
        "a file's diff must not be collapsed behind its own disclosure: {show}"
    );
    assert!(
        show.contains(r#"<details class="commit-extra">"#),
        "hooks and checks must be one collapsed, secondary disclosure: {show}"
    );
    assert!(
        show.contains("commit-redaction") && !show.contains(r#"<details class="commit-redaction"#),
        "the redaction note must stay outside any disclosure: {show}"
    );

    let css = super::STYLE_CSS;
    let start = css
        .find(
            "
.app-dialog-commit {",
        )
        .expect("style.css should have an .app-dialog-commit rule");
    let rule = &css[start..start + css[start..].find('}').unwrap()];
    for declaration in ["position: fixed;", "inset: 0;"] {
        assert!(
            rule.contains(declaration),
            "the .app-dialog-commit rule lacks {declaration}: {rule}"
        );
    }
}

/// Spec 110 Task 3: a refused request — a 409 from any route included — shows
/// the route's own message. `api`, a refused stream and the export download
/// all read the body through one parser and throw its `error`, never the
/// JSON text.
#[test]
fn app_js_shows_a_refusals_message_never_its_json() {
    let parse = app_js_function("parseApiPayload");
    assert!(parse.contains("return { error: text };"), "{parse}");
    assert!(parse.contains("!Array.isArray(payload)"), "{parse}");
    let refusal = app_js_function("refusalError");
    assert!(refusal.contains("parseApiPayload(text)"), "{refusal}");
    assert!(refusal.contains("payload.error.trim()"), "{refusal}");
    assert!(
        refusal.contains("error.status = response.status;"),
        "{refusal}"
    );
    assert!(
        !refusal.contains("new Error(text"),
        "the body is never the message: {refusal}"
    );
    assert!(app_js_function("api").contains("throw refusalError(response, text);"));
    assert!(
        app_js_function("streamRequestError")
            .contains("return refusalError(response, await response.text());")
    );
    assert!(
        app_js_function("exportSessionDownload")
            .contains("throw refusalError(response, await response.text());")
    );
    assert!(
        !super::APP_JS.contains("throw new Error(text || response.statusText)"),
        "no route throws its raw body"
    );
}

/// Spec 110 Task 3: sidebar actions during a turn update the sidebar at
/// once. Sidebar fetches overlap while sessions work (the working-set
/// refresh, a turn's end, an archive, a rename); an answer older than one
/// already applied is dropped, so a list read before an archive never puts
/// the session back. Renaming another session keeps the open one open, and
/// an action on another project fetches its list even while a load of it is
/// in flight.
#[test]
fn app_js_sidebar_actions_during_a_turn_update_the_sidebar_at_once() {
    let fetch = app_js_function("fetchProjectSessions");
    assert!(
        fetch.contains("const fetchNumber = ++projectSessionsFetches;"),
        "{fetch}"
    );
    assert!(
        fetch.contains("if (fetchNumber > (projectSessionsApplied.get(projectPath) || 0))"),
        "{fetch}"
    );
    assert!(
        fetch.contains("projectSessionsByPath.set(projectPath, payload.sessions || []);"),
        "{fetch}"
    );
    for reader in [
        "loadSessions",
        "loadProjectSessions",
        "refreshSessionStates",
    ] {
        let body = app_js_function(reader);
        assert!(body.contains("fetchProjectSessions("), "{reader}: {body}");
        assert!(
            !body.contains("projectSessionsByPath.set("),
            "{reader} writes the list only through the ordered fetch: {body}"
        );
    }
    let rename = app_js_function("renameSessionForProject");
    assert!(
        rename.contains("await loadSessions(currentSessionId || \"\", false);"),
        "renaming another session must not select it: {rename}"
    );
    for action in [
        "setSessionArchived",
        "renameSessionForProject",
        "deleteSessionForProject",
    ] {
        let body = app_js_function(action);
        assert!(body.contains("fetchProjectSessions("), "{action}: {body}");
        assert!(!body.contains("loadProjectSessions("), "{action}: {body}");
        assert!(body.contains("renderProjectList();"), "{action}: {body}");
    }
}

/// Spec 110 Task 3: switching to another session while one works leaves the
/// first running, and returning shows its progress again. This window's turns
/// are tracked per session, so another session's composer is free and its
/// Stop never reaches the first; a stream names its session to the window
/// only while the window still shows it; a turn whose bubble left the page
/// draws no cards or decision into the open session, and returns its notes to
/// its own composer; loading the session puts its live bubble back.
#[test]
fn app_js_a_session_left_mid_turn_keeps_working_and_shows_it_on_return() {
    let app = super::APP_JS;
    for gone in ["chatSubmitting", "currentTurn", "stopCurrentTurn"] {
        assert!(!app.contains(gone), "app.js still names {gone}");
    }
    assert!(
        !app.contains("currentSessionId = payload.sessionId;"),
        "a turn's done must not take the window to its session"
    );
    assert!(
        app_js_function("sessionSubmitting").contains("windowTurnFor(currentSessionId)"),
        "busy means this session's turn"
    );
    let target = app_js_function("composerStopTarget");
    assert!(
        target.contains("const turn = windowTurnFor(currentSessionId);"),
        "{target}"
    );

    let follow = app_js_function("followTurnSession");
    assert!(
        follow.contains("const shown = !owner || owner.sessionId === currentSessionId;"),
        "{follow}"
    );
    assert!(
        follow.contains("owner?.adoptSession(sessionId);"),
        "{follow}"
    );
    assert!(follow.contains("if (shown) {"), "{follow}");
    // Every resume flow's `done` and the composer's `session` / `done` go
    // through it.
    assert!(
        app.matches("followTurnSession(indicator?.action, payload.sessionId);")
            .count()
            >= 7
    );
    for flow in ["sendChatPrompt", "askAgainFromExhausted"] {
        let body = app_js_function(flow);
        assert!(body.contains("beginWindowTurn()"), "{flow}");
        assert!(body.contains("followTurnSession(turn, "), "{flow}");
        assert!(
            body.contains("turn?.end()") || body.contains("turn.end()"),
            "{flow}"
        );
        assert!(body.contains("liveTurnShown(assistantMessage)"), "{flow}");
    }
    let send = app_js_function("sendChatPrompt");
    assert!(
        send.contains("if (!liveTurnShown(assistantMessage)) return true;"),
        "{send}"
    );
    assert!(
        send.contains("void refreshSessionStates([turnRepo]);"),
        "the left session's sidebar row follows its outcome: {send}"
    );

    let proposals = app_js_function("appendProposals");
    let guard = proposals
        .find("if (!liveTurnShown(message)) return;")
        .expect("a left turn draws nothing into the open session");
    assert!(guard < proposals.find("noteContextPayload(").unwrap());
    assert!(guard < proposals.find("setPendingDecision(").unwrap());

    let action = app_js_function("beginRunningAction");
    assert!(
        action.contains("return adopted || sessionAtStart"),
        "{action}"
    );
    assert!(
        app_js_function("startTurnIndicator").contains("running.liveMessage = target.message;")
    );
    let reattach = app_js_function("reattachLiveTurns");
    assert!(reattach.contains("log.append(bubble);"), "{reattach}");
    assert!(reattach.contains("observeStatusHome(home)"), "{reattach}");
    assert!(reattach.contains("waitingNotes"), "{reattach}");
    let load = app_js_function("loadSession");
    let restore = load.find("restorePendingDecision(").unwrap();
    let back = load
        .find("reattachLiveTurns(currentSessionId);")
        .expect("loading a session shows its running work again");
    assert!(restore < back, "{load}");
    assert!(
        load.contains("heldComposerText.get(currentSessionId)"),
        "{load}"
    );

    let refill = app_js_function("refillComposer");
    assert!(
        refill.contains("if (sessionId && sessionId !== currentSessionId)"),
        "{refill}"
    );
    assert!(
        app_js_function("whenTurnsIdle")
            .contains("!windowTurnFor(sessionId) && !runningTurnStream(sessionId)"),
        "another session's turn does not hold up a returned note"
    );
    assert!(
        app_js_function("processSseEvent").contains("turn.sessionId === currentSessionId"),
        "a left turn does not repoint the context panel"
    );
}

/// Observation 103: a session can be busy on work this window never
/// streamed (a crash-recovered command's own resumed turn). The poller
/// is the one thing that still says so, the banner shows it, and Send
/// refuses rather than repeating the 409 the shell already gave once.
#[test]
fn app_js_shows_work_this_window_never_streamed() {
    let poll = app_js_function("pollWorkingSessions");
    assert!(poll.contains(r#"api("/api/working-sessions")"#), "{poll}");
    assert!(poll.contains("syncComposerStatus();"), "{poll}");

    let busy = app_js_function("busyElsewhereWork");
    assert!(
        busy.contains("runningTurnStream(currentSessionId)"),
        "a stream this window already knows about must not double up with the slot: {busy}"
    );
    let sync = app_js_function("syncComposerStatus");
    assert!(
        sync.contains("void loadSession(currentSessionId);"),
        "the only way to learn what invisible work left behind is a reload: {sync}"
    );

    let send = app_js_function("sendChatPrompt");
    assert!(
        send.contains("workingSessionsById.has(currentSessionId)"),
        "Send must refuse rather than repeat a 409 the shell already gave: {send}"
    );

    let target = app_js_function("composerStopTarget");
    assert!(
        target.contains("workingSessionsById.get(currentSessionId)")
            && target.contains("stopWorkFromComposer(sessionId)"),
        "the composer's Stop must cancel the session the banner names: {target}"
    );
    assert!(
        app_js_function("stopWorkFromComposer").contains("await stopSessionWork(sessionId);"),
        "it cancels through the session-work route"
    );
}

/// Spec 109 D7/D8: every turn is stoppable, the composer line
/// and a check card's running row read the same running state, and the
/// composer stops claiming a decision once it has been answered
/// (observation 89).
#[test]
fn app_js_gives_every_running_action_one_state_and_a_stop() {
    let indicator = app_js_function("startTurnIndicator");
    assert!(
        indicator.contains("beginRunningAction({ stop: onStop || null })"),
        "a turn's line is a running action: {indicator}"
    );
    assert!(
        indicator.contains("if (onStop === false)")
            && indicator.contains("running.unstoppable = true;"),
        "only an explicit `false` makes a turn unstoppable: {indicator}"
    );
    // The one turn line without Stop is the legacy one-shot patch request.
    assert_eq!(
        super::APP_JS
            .matches("startTurnIndicator(assistantMessage, false)")
            .count(),
        1
    );
    let begin = app_js_function("beginRunningAction");
    assert!(
        begin.contains("void stopSessionWork(action.sessionId)"),
        "a stream's default Stop is the shell's session-work cancel: {begin}"
    );
    let stop = app_js_function("stopSessionWork");
    assert!(stop.contains(r#"api("/api/session-work-cancel""#), "{stop}");

    let entry = app_js_function("composerStatusEntry");
    assert!(
        entry.contains("runningActionFor(currentSessionId)"),
        "{entry}"
    );
    let decision = app_js_function("pendingDecisionApplies");
    assert!(
        decision.contains("&& !running"),
        "an answered decision gives way to what it started: {decision}"
    );

    let card = app_js_function("createCheckBatchCard");
    assert!(
        card.contains("card.append(createRunningRow(action));"),
        "checks that run show Cancel in place of the card's buttons: {card}"
    );
    let busy = app_js_function("busyElsewhereWork");
    assert!(
        busy.contains("runningActionFor(currentSessionId)"),
        "{busy}"
    );
}

/// Spec 112 requirement 3: the composer's Send is the one Stop. One function
/// decides what it stops — this window's stream (abort), else the running
/// action (a resumed stream, an answered question, a fetch decision), else
/// work only the shell knows about (session-work cancel) — and click and
/// Escape both go through it. The turn row and the composer lines build no
/// Stop; unstoppable work leaves a disabled Stop that says why.
#[test]
fn app_js_has_one_stop_in_the_composer() {
    let target = app_js_function("composerStopTarget");
    let stream = target
        .find("windowTurnFor(currentSessionId)")
        .expect("this session's stream first");
    let action = target
        .find("runningActionFor(currentSessionId)")
        .expect("then the running action");
    let work = target
        .find("workingSessionsById.get(currentSessionId)")
        .expect("then work this window never streamed");
    assert!(stream < action && action < work, "{target}");
    assert!(target.contains("action.stop()"), "{target}");
    assert!(target.contains("action.unstoppableReason"), "{target}");
    assert!(target.contains("if (!work.cancellable)"), "{target}");

    let sync = app_js_function("syncComposerStop");
    assert!(sync.contains("button.disabled = !target.stop;"), "{sync}");
    assert!(sync.contains("button.title = target.reason"), "{sync}");
    // The slot keeps the button in step; the running-state tick, the poll
    // and `setComposerBusy` all reach it.
    assert!(app_js_function("syncComposerStatus").contains("syncComposerStop();"));
    for caller in ["syncRunningState", "pollWorkingSessions", "setComposerBusy"] {
        assert!(
            app_js_function(caller).contains("syncComposerStatus();"),
            "{caller} must keep the button in step"
        );
    }

    // One Stop site: click and Escape share `stopFromComposer`, and nothing
    // else calls the stream's abort directly.
    let app = super::APP_JS;
    assert_eq!(app.matches("stopFromComposer();").count(), 1, "Escape");
    assert!(app.contains("if (stopFromComposer()) return;"), "click");
    assert_eq!(
        app.matches("stopWindowTurn(turn);").count(),
        0,
        "nothing calls the stream's abort directly"
    );
    assert!(
        target.contains("action ? action.stop() : stopWindowTurn(turn)"),
        "{target}"
    );
    // The question answer and fetch decision streams have running actions
    // with the default (cancel-route) stop, so the composer reaches them.
    for flow in ["createQuestionCard", "createFetchApproval"] {
        assert!(
            app_js_function(flow).contains("startTurnIndicator(assistantMessage);"),
            "{flow}"
        );
    }

    let indicator = app_js_function("startTurnIndicator");
    assert!(
        !indicator.contains("createElement(\"button\")"),
        "the turn row builds no button: {indicator}"
    );
    assert!(
        indicator.contains("runningListeners.add(showStopping)"),
        "{indicator}"
    );
    for gone in [
        "turn-indicator-stop",
        "composer-running-stop-btn",
        "composer-invisible-work-stop-btn",
    ] {
        assert!(!app.contains(gone), "app.js names {gone}");
        assert!(!super::INDEX_HTML.contains(gone), "index.html names {gone}");
        assert!(!super::STYLE_CSS.contains(gone), "style.css names {gone}");
    }

    // Steering is unchanged: Enter goes to `sendChatPrompt`, which delivers a
    // note into a running turn.
    assert!(
        app_js_function("sendChatPrompt").contains("return sendSteeringNote(working.sessionId);")
    );
}

/// Observation 103: a recovered command card names its session, so the
/// shell gives the request the session's lane (and its cancel token), and
/// the card shows it running with Stop, following the lane's label.
#[test]
fn app_js_makes_a_recovered_command_visible_and_stoppable() {
    let card = app_js_function("createCommandApprovalPreview");
    assert!(
        card.contains(r#"session_id: currentSessionId || "","#),
        "{card}"
    );
    assert!(
        card.contains("running.followsServerLabel = true;"),
        "{card}"
    );
    assert!(
        card.contains(r#"createRunningRow(running, { stopLabel: "Stop" })"#),
        "{card}"
    );
    let poll = app_js_function("pollWorkingSessions");
    assert!(poll.contains("action.followsServerLabel"), "{poll}");
    // `exitCode` is -1 for a timed-out run too; only the termination the
    // route returns says the user's Stop ended it.
    assert!(
        card.contains(r#"payload.termination === "cancelled""#),
        "{card}"
    );
}

/// A Settings toggle for spec 2's semantic search, same shape as the
/// other config-backed checkboxes: reads the effective value (default
/// off, the opposite reading of `conversation_auto_compact`'s default-on
/// one) and writes through `/api/config-set`.
#[test]
fn app_js_has_a_semantic_search_settings_toggle() {
    assert!(
        super::APP_JS.contains(r#"<input id="semantic-search-input""#)
            || super::INDEX_HTML.contains(r#"<input id="semantic-search-input""#),
        "the Settings page must have the checkbox"
    );
    let refresh = app_js_function("refreshSemanticSearchEnabled");
    assert!(
        refresh.contains(r#"entry.key === "enable_semantic_search""#),
        "{refresh}"
    );
    assert!(
        refresh.contains(r#"rule?.value === "true""#),
        "default off: an absent key must read as unchecked: {refresh}"
    );
    assert!(
        super::APP_JS.contains(
            r#"form({ key: "enable_semantic_search", value: enabled ? "true" : "false" })"#
        ),
        "the toggle must write through the same route every other config checkbox uses"
    );
    let load = app_js_function("loadConfigFile");
    assert!(
        load.contains("refreshSemanticSearchEnabled();"),
        "opening Settings must refresh the toggle: {load}"
    );
}

/// The cap on how large a repository semantic search embeds sits under the
/// toggle, reads the effective value, and writes only a positive whole number.
#[test]
fn settings_offer_the_semantic_search_size_cap() {
    assert!(
        super::INDEX_HTML.contains(r#"id="semantic-search-max-chunks-input""#)
            && super::INDEX_HTML.contains(r#"data-config-key="semantic_search_max_chunks""#),
        "the Settings page must have the cap field"
    );
    let refresh = app_js_function("refreshSemanticSearchEnabled");
    assert!(
        refresh.contains(r#"entry.key === "semantic_search_max_chunks""#),
        "{refresh}"
    );
    assert!(
        super::APP_JS.contains(r#"form({ key: "semantic_search_max_chunks", value })"#),
        "the cap must write through /api/config-set"
    );
    assert!(
        super::APP_JS.contains(r#"!/^[1-9]\d*$/.test(value)"#),
        "zero or a non-number must not be saved"
    );
}

/// Spec 112 requirement 4 (D7): after a restart the sweep's recovery card is
/// the one answerable copy of a question, and `restorePendingDecision` frames
/// it; the transcript's copy keeps the question's text with no answer
/// controls. A worker's transcript copy is left alone, because it answers
/// through the worker's socket rather than the shell's route.
#[test]
fn app_js_draws_a_reattached_question_answerable_once() {
    let recovery = app_js_function("renderRecoveryPrompts");
    assert!(recovery.contains("if (resumable) {"), "{recovery}");
    assert!(
        recovery.contains(r#"approval.kind === "question""#),
        "{recovery}"
    );
    assert!(
        recovery.contains(r#"querySelectorAll(".message.question > .question-card")"#),
        "only the transcript's copy, never the recovery card's: {recovery}"
    );
    assert!(
        recovery.contains("card.dataset.questionId === questionId"),
        "{recovery}"
    );
    assert!(
        recovery.contains("makeQuestionCardReadOnly(card)"),
        "{recovery}"
    );

    let read_only = app_js_function("makeQuestionCardReadOnly");
    for control in [
        ".question-card-options",
        ".question-card-free-text",
        ".question-card-status",
    ] {
        assert!(read_only.contains(control), "{control}: {read_only}");
    }
    assert!(
        read_only.contains(r#"card.dataset.readonly = "true";"#),
        "{read_only}"
    );
    // An answered card keeps its record.
    assert!(
        read_only.contains(r#"if (card.dataset.answered === "true") return;"#),
        "{read_only}"
    );

    // The framed copy is the recovery card whenever there is one, and the
    // recovery cards are drawn before the decision is restored.
    let restore = app_js_function("restorePendingDecision");
    let recovered = restore.find("recovered ||").expect("recovery card first");
    let transcript = restore
        .find(r#"querySelectorAll(".question-card")"#)
        .expect("transcript card as the fallback");
    assert!(recovered < transcript, "{restore}");
    let load = app_js_function("loadSession");
    let sweep = load
        .find("await renderRecoveryPrompts(currentSessionId, generation);")
        .unwrap();
    let pending = load
        .find("await restorePendingDecision(payload.tasks || [], current);")
        .unwrap();
    assert!(sweep < pending, "{load}");

    // Overlapping loads of one session drew the recovery card twice: a load a
    // later one has overtaken stops drawing, inside the sweep's await too.
    assert!(
        load.contains("const generation = ++sessionLoadGeneration;"),
        "{load}"
    );
    assert!(
        load.contains("await renderRecoveryPrompts(currentSessionId, generation);"),
        "{load}"
    );
    assert!(
        load.matches("if (!current()) return;").count() >= 4,
        "{load}"
    );
    assert!(
        recovery.contains("if (generation !== sessionLoadGeneration) return;"),
        "{recovery}"
    );

    assert!(
        super::STYLE_CSS.contains(".question-card[data-readonly=\"true\"]"),
        "a read-only copy looks settled, not clickable"
    );
}

/// Spec 112 requirement 6: an assistant message draws no bordered body until
/// it has content, so a starting turn is its status row alone.
#[test]
fn style_css_hides_an_empty_assistant_body() {
    let css = super::STYLE_CSS;
    let rule = &css[css
        .find(".message.assistant .message-body:empty {")
        .expect("the empty-body rule")..];
    let rule = &rule[..rule.find('}').unwrap()];
    assert!(rule.contains("display: none;"), "{rule}");
    // The live turns still open with an empty body and fill it on the first
    // token, which is what makes the rule apply until then.
    assert!(
        app_js_function("sendChatPrompt")
            .contains(r#"assistantMessage = appendChatMessage("assistant", "");"#)
    );
    assert!(app_js_function("updateChatMessage").contains("target.body.innerHTML ="));
}

/// Spec 112 requirement 6: Rewind is offered only for a checkpoint that
/// covers a file, and a live turn marks its own checkpoint — found by its
/// task id — not the session's newest unrestored one.
#[test]
fn app_js_offers_rewind_only_for_the_turns_own_files() {
    let mark = app_js_function("markMessageRewindable");
    let guard = mark
        .find("if (!checkpoint.fileCount) return;")
        .expect("no row for a checkpoint that covers no file");
    assert!(guard < mark.find("createElement").unwrap(), "{mark}");

    let send = app_js_function("sendChatPrompt");
    assert!(
        send.contains("sessionCheckpoints.find((entry) => entry.taskId === turnTaskId)"),
        "{send}"
    );
    assert!(
        send.contains("const turnTaskId = assistantMessage?.message.dataset.taskId;"),
        "{send}"
    );
    assert!(
        !send.contains("!entry.restoredAtMs"),
        "the newest unrestored checkpoint is not this turn's: {send}"
    );
    // The reload path matches the user message itself.
    assert!(
        app_js_function("renderMessages")
            .contains("sessionCheckpoints.find((entry) => entry.userMessageId === message.id)")
    );
}

/// Observation 106: the terminal panel's height is the user's. A separator on
/// its top edge sets `--terminal-panel-height`, clamped, persisted, and reset
/// by double-click; the hard 360px cap is gone; the xterm element is still
/// observed so a height change refits the terminal and resizes the pty.
#[test]
fn terminal_panel_has_a_persisted_resize_handle() {
    let html = super::INDEX_HTML;
    let panel = &html[html.find(r#"<section id="terminal-panel""#).expect("panel")..];
    let panel = &panel[..panel.find("</section>").unwrap()];
    assert!(panel.contains(r#"id="terminal-resize-handle""#), "{panel}");
    assert!(panel.contains(r#"role="separator""#), "{panel}");

    let css = super::STYLE_CSS;
    let rule = &css[css.find(".terminal-panel {").expect("panel rule")..];
    let rule = &rule[..rule.find('}').unwrap()];
    assert!(
        rule.contains("height: var(--terminal-panel-height,"),
        "{rule}"
    );
    assert!(
        !rule.contains("min(360px, 38vh)"),
        "the old hard cap: {rule}"
    );
    assert!(css.contains(".terminal-resize-handle {"));
    assert!(css.contains("cursor: row-resize;"));

    let wire = app_js_function("wireTerminalResize");
    for needle in [
        "localStorage.getItem(terminalHeightStorageKey)",
        "setPointerCapture(event.pointerId)",
        "drag.startHeight + (drag.startY - event.clientY)",
        "setTerminalHeight(panel.getBoundingClientRect().height, { persist: true })",
        r#"addEventListener("dblclick", () => setTerminalHeight(null, { persist: true }))"#,
        r#""ArrowUp""#,
    ] {
        assert!(wire.contains(needle), "wireTerminalResize lacks {needle}");
    }
    let set = app_js_function("setTerminalHeight");
    assert!(
        set.contains(
            r#"setProperty("--terminal-panel-height", `${clampTerminalHeight(height)}px`)"#
        )
    );
    assert!(set.contains("localStorage.setItem(terminalHeightStorageKey"));
    assert!(super::APP_JS.contains("\nwireTerminalResize();\n"));
    assert!(
        app_js_function("createTerminalInstance")
            .contains(r#"termResizeObserver.observe($("terminal-xterm"))"#),
        "a height change must still refit xterm"
    );
}

/// Observation 99: a patch card with no pending file is a record, not a
/// decision. It drops Apply and Reject instead of offering "Apply 0".
#[test]
fn app_js_settles_a_patch_card_with_no_pending_file() {
    let preview = app_js_function("createPatchPreview");
    assert!(
        preview
            .contains(r#"const settled = !state.files.some((file) => file.state === "pending");"#),
        "{preview}"
    );
    assert!(preview.contains("actions.hidden = settled;"));
    assert!(preview.contains(r#"wrapper.classList.toggle("settled", settled);"#));
    assert!(!preview.contains("No patch files left to review."));
    assert!(super::STYLE_CSS.contains(".patch-actions[hidden] {\n  display: none;"));
}

/// Spec 112 fix (2026-11-20): a working-sessions answer from a poll that
/// started before this window's own turn or action in the session ended still
/// lists that work. Read as "busy elsewhere", it flipped the slot on and, at
/// the next poll, off again — and going off reloads the session, which drew no
/// card for a pending command or plan: the card appeared and vanished about two
/// seconds later. The poll stamps when it was *sent*, every local end is
/// noted, and only a poll sent after the end can report work elsewhere.
#[test]
fn app_js_reads_busy_elsewhere_only_from_a_poll_sent_after_local_work_ended() {
    let poll = app_js_function("pollWorkingSessions");
    let sent = poll
        .find("workingSessionsRequestedAt")
        .expect("the poll records when it was sent");
    let fetched = poll
        .find("api(\"/api/working-sessions\")")
        .expect("the poll fetches the working sessions");
    assert!(
        poll[..fetched].contains("const requestedAt = Date.now()"),
        "the send time is taken before the request: {poll}"
    );
    assert!(sent > fetched, "it is applied only with the answer: {poll}");

    let busy = app_js_function("busyElsewhereWork");
    assert!(
        busy.contains("localWorkEndedAt.get(currentSessionId)")
            && busy.contains("workingSessionsRequestedAt"),
        "busy elsewhere ignores a poll sent before local work ended: {busy}"
    );

    let stream = app_js_function("streamRequest");
    let finally = &stream[stream.find("finally").expect("a finally block")..];
    assert!(
        finally.contains("noteLocalWorkEnded(turn.sessionId)"),
        "a stream's end is noted: {stream}"
    );
    let action = app_js_function("beginRunningAction");
    let end = &action[action.find("end() {").expect("an end method")..];
    assert!(
        end.contains("noteLocalWorkEnded(action.sessionId)"),
        "a running action's end is noted: {action}"
    );
}

/// Spec 107 note 2026-11-20: a reload drew no card for a command, patch or
/// plan paused while the app runs — only a question or a decision the
/// startup sweep reattached had one. With neither on the page, the card is
/// read from `/api/pending-card` and drawn by `appendProposals`, the live
/// turn's own builder, so it answers through the live resume route and never
/// the recovery close-out (`resolveReattachedApproval`, `detached`).
#[test]
fn app_js_redraws_a_live_pending_card_on_reload() {
    let restore = app_js_function("restorePendingDecision");
    let found = restore.find("const card =").expect("the page's own card");
    let read = restore
        .find("card ? null : await loadPendingCard(task.id, decision)")
        .expect("the card is read only when the page has none");
    assert!(found < read, "{restore}");
    assert!(
        restore.contains("if (!isCurrent()) return;"),
        "a load overtaken during the read draws nothing: {restore}"
    );
    assert!(
        restore.contains("appendProposals(body ? { body } : {}, live, repo());"),
        "the live builder draws it: {restore}"
    );
    for recovery_route in [
        "resolveReattachedApproval",
        "detached",
        "createReattachedApprovalCard",
    ] {
        assert!(
            !restore.contains(recovery_route),
            "{recovery_route}: {restore}"
        );
    }

    let load = app_js_function("loadPendingCard");
    assert!(load.contains("/api/pending-card?"), "{load}");
    assert!(
        load.contains(r#"decision.kind === "question""#),
        "a question keeps its transcript card: {load}"
    );
    assert!(
        load.contains("(payload.proposalId ?? payload.patchId) !== decision.cardId"),
        "the card must answer the decision the engine reports: {load}"
    );
    assert!(
        load.contains("turnPresentation: { pendingDecision: decision }, [field]: payload"),
        "shaped like the live turn's `done` payload: {load}"
    );
}

fn app_js_function(name: &str) -> &'static str {
    let app = super::APP_JS;
    let start = app
        .find(&format!("\nfunction {name}("))
        .or_else(|| app.find(&format!("\nasync function {name}(")))
        .unwrap_or_else(|| panic!("app.js has no function {name}"));
    let rest = &app[start + 1..];
    let end = rest.find("\n}\n").map_or(rest.len(), |end| end + 3);
    &rest[..end]
}

/// Spec 112 requirement 7: a session row draws the worker dot's look for
/// `working`, `needs_you` and `failed` in place of its time, named for screen
/// readers and on hover; any other session keeps its time. A collapsed
/// project names its "N need you". The working-sessions poll refetches the
/// sessions when the working set changes, and a local turn's end refetches
/// them.
#[test]
fn app_js_draws_session_state_dots_in_the_sidebar() {
    let app = super::APP_JS;
    let table = &app[app.find("const SESSION_STATE_DOTS = {").expect("the table")..];
    let table = &table[..table.find("};").unwrap()];
    for line in [
        r#"working: { look: "running", label: "Working" }"#,
        r#"needs_you: { look: "needs-you", label: "Needs you" }"#,
        r#"failed: { look: "failed", label: "Failed" }"#,
    ] {
        assert!(table.contains(line), "{line}: {table}");
    }
    assert_eq!(table.matches("look:").count(), 3, "three states: {table}");

    let dot = app_js_function("createSessionStateDot");
    assert!(dot.contains("if (!state) return null;"), "{dot}");
    for part in [
        "\"worker-dot session-state-dot\"",
        "dot.dataset.state = state.look;",
        "setAttribute(\"role\", \"img\")",
        "setAttribute(\"aria-label\", state.label)",
        "dot.title = state.label;",
    ] {
        assert!(dot.contains(part), "{part}: {dot}");
    }
    assert!(
        app_js_function("renderProjectSession")
            .contains("row.append(button, createSessionStateDot(session) || when, menuButton);")
    );
    // Worker rows keep the engine's dot.
    assert!(app_js_function("renderProjectWorker").contains("createWorkerDot(row)"));

    let list = app_js_function("renderProjectList");
    assert!(list.contains("sessionsNeedingYou(sessions)"), "{list}");
    assert!(list.contains("`${needYou} need you`"), "{list}");
    assert!(app_js_function("sessionsNeedingYou").contains("session.state === \"needs_you\""));

    let poll = app_js_function("pollWorkingSessions");
    assert!(
        poll.contains("if (workingSet !== sidebarWorkingSet)")
            && poll.contains("void refreshSessionStates();"),
        "{poll}"
    );
    let refresh = app_js_function("refreshSessionStates");
    assert!(refresh.contains("fetchProjectSessions(path)"), "{refresh}");
    assert!(app_js_function("fetchProjectSessions").contains("/api/sessions?repo="));
    assert!(
        refresh.contains("if (changed.some(Boolean)) renderProjectList();"),
        "{refresh}"
    );
    for turn in ["sendChatPrompt", "askAgainFromExhausted"] {
        let body = app_js_function(turn);
        let finally = &body[body.rfind("} finally {").expect(turn)..];
        // The turn's own project (spec 110 Task 3): the user may have
        // switched to another while it ran.
        assert!(
            finally.contains("void refreshSessionStates([turnRepo]);")
                || finally.contains("void refreshSessionStates([askRepo]);"),
            "{turn}: {finally}"
        );
    }

    let css = super::STYLE_CSS;
    assert!(css.contains(".project-need-you {"));
    assert!(css.contains(".session-state-dot {"));
}

/// Spec 107 Task 2, requirement 1, per kind: the card of every decision the
/// engine can report goes to the end of `#chat-log`, not into the bubble.
/// The match is exhaustive, so a new engine kind fails to compile here
/// until the UI says which proposal carries its card.
#[test]
fn app_js_places_every_pending_decision_kind_above_the_composer() {
    use workspace_engine::PendingDecisionKind as Kind;
    let kinds = [
        Kind::Question,
        Kind::Patch,
        Kind::Checks,
        Kind::Command,
        Kind::RoundBudget,
        Kind::Plan,
        Kind::Fallback,
        Kind::ContextReview,
        Kind::Fetch,
    ];
    let append = app_js_function("appendProposals");
    let fields = &super::APP_JS[super::APP_JS
        .find("const PENDING_DECISION_FIELDS = {")
        .expect("the kind-to-proposal map")..];
    let fields = &fields[..fields.find("};").unwrap()];
    for kind in kinds {
        let (field, create) = match kind {
            Kind::Question => ("questionProposal", "createQuestionCard"),
            Kind::Patch => ("patchProposal", "createPatchPreview"),
            Kind::Checks => ("checkBatch", "createCheckBatchCard"),
            Kind::Command => ("commandProposal", "createCommandApprovalPreview"),
            Kind::RoundBudget => ("roundBudgetProposal", "createRoundBudgetOffer"),
            Kind::Plan => ("planProposal", "createPlanReview"),
            Kind::Fallback => ("fallbackProposal", "createFallbackOffer"),
            Kind::ContextReview => ("contextReview", "createContextReview"),
            Kind::Fetch => ("fetchProposal", "createFetchApproval"),
        };
        assert!(
            fields.contains(&format!("  {}: \"{field}\",", kind.as_str())),
            "{} maps to {field}",
            kind.as_str()
        );
        assert!(
            append.contains(&format!("place(\"{field}\"")),
            "{field} is placed through the pending-decision rule"
        );
        assert!(
            !append.contains(&format!("message.body.append({create}(")),
            "{create} no longer goes straight into the bubble"
        );
    }
    // The kind comes from the engine, and the call clears a stale one.
    assert!(append.contains("payload.turnPresentation?.pendingDecision"));
    assert!(append.contains("setPendingDecision(message, pendingCard, decision, pendingMount)"));
    // A verification report is a result, not a decision: it stays put.
    assert!(append.contains("message.body.append(\n      createVerificationReportCard("));

    let set = app_js_function("setPendingDecision");
    assert!(set.contains("$(\"chat-log\").append(frame)"));
    assert!(set.contains("scrollIntoView({ block: \"nearest\" })"));
    // Spec 112 requirement 5: no pointer in the bubble; the slot is the
    // pointer while the card is out of view.
    assert!(set.contains("observeStatusHome(frame)"), "{set}");
    // The one-shot patch route has no turn, but its card follows the rule.
    let one_shot = app_js_function("proposePatchFromChat");
    assert!(one_shot.contains("setPendingDecision("));
    assert!(!one_shot.contains("body.append(createPatchPreview"));
}

/// Spec 107 Task 2: after a reload the decision comes from `/api/session`
/// and its card is moved below everything else the session draws; the
/// memory stack never lands under it.
#[test]
fn app_js_restores_the_pending_decision_last_on_reload() {
    let load = app_js_function("loadSession");
    let memory = load
        .find("await refreshMemoryProposals(currentSessionId)")
        .unwrap();
    let restore = load
        .find("restorePendingDecision(payload.tasks || [], current)")
        .expect("loadSession restores the pending decision");
    let recovery = load.find("await renderRecoveryPrompts(").unwrap();
    assert!(recovery < restore && memory < restore);
    let restore_fn = app_js_function("restorePendingDecision");
    assert!(restore_fn.contains("entry.turnPresentation?.pendingDecision"));
    assert!(restore_fn.contains("entry.dataset.questionId === decision.cardId"));
    assert!(restore_fn.contains(".recovery-card"));
    assert!(app_js_function("renderMessages").contains("resetPendingDecision()"));
    assert!(app_js_function("clearChat").contains("resetPendingDecision()"));
    assert!(app_js_function("renderMemoryProposals").contains("log.insertBefore(stack, pending)"));
}

/// Spec 107 Task 2, requirement 2 and D2, as spec 112 requirements 2 and 5
/// keep them: the slot names the decision from the engine's label, its
/// control focuses the card, and a pending question disables the main
/// composer instead of competing with the card's answer field — whether or
/// not the card is in view.
#[test]
fn app_js_composer_names_the_pending_decision_and_yields_to_a_question() {
    let entry = app_js_function("composerStatusEntry");
    assert!(entry.contains("decision.decision.label"), "{entry}");
    assert!(entry.contains("\"Needs your answer\""), "{entry}");
    assert!(
        entry.contains("question ? \"Answer\" : \"Show\""),
        "{entry}"
    );
    assert!(app_js_function("pendingDecisionApplies").contains("!sessionSubmitting()"));
    let lock = app_js_function("syncComposerLock");
    assert!(lock.contains("pendingDecisionApplies()"), "{lock}");
    assert!(
        !lock.contains("statusHome"),
        "the lock does not follow the card's visibility: {lock}"
    );
    assert!(lock.contains("field.disabled = state.locked"), "{lock}");
    assert!(lock.contains("button.disabled = state.locked"), "{lock}");
    assert!(super::APP_JS.contains("question: \"Answer the question\","));
    assert!(!super::APP_JS.contains("Answer the question above"));
    assert!(app_js_function("syncComposerStatus").contains("syncComposerLock();"));
    let focus = app_js_function("focusPendingDecision");
    assert!(focus.contains(".question-card-free-text input:not(:disabled)"));
    assert!(
        super::APP_JS.contains(
            "$(\"composer-status-btn\").addEventListener(\"click\", focusPendingDecision)"
        )
    );
}

/// Spec 113 requirement 1 and D1: with a plan review pending, Send (button or
/// Enter, both through `sendChatPrompt`) sends the composer's text as that
/// plan's decline with changes, through the card's own `decide` — the one
/// `/api/resume-plan-stream` call with `approved=false` — and never posts
/// `/api/ask-stream`. It never approves: the hook passes `false`. A worker's
/// card has no such hook, and a steering note is still checked first.
#[test]
fn app_js_typing_over_a_pending_plan_declines_it_with_changes() {
    let send = app_js_function("sendChatPrompt");
    let steering = send
        .find("return sendSteeringNote(working.sessionId)")
        .expect("steering notes go first");
    let worker = send.find("await sendWorkerComposer(").unwrap();
    let pending = send
        .find("const pending = pendingDecisionApplies();")
        .expect("sendChatPrompt reads the pending decision");
    let ask = send.find("\"/api/ask-stream\"").unwrap();
    assert!(
        worker < steering && steering < pending && pending < ask,
        "{send}"
    );
    // Only the composer's own text: a prompt a button supplies is not typing.
    assert!(send.contains("if (!promptOverride) {"), "{send}");
    assert!(send.contains("if (state?.locked) return false;"), "{send}");
    assert!(
        send.contains("return declinePendingPlanWithChanges(planDeclineFor(pending));"),
        "{send}"
    );

    let decline = app_js_function("declinePendingPlanWithChanges");
    assert!(decline.contains("field.value = \"\";"), "{decline}");
    assert!(decline.contains("await decline(text)"), "{decline}");
    assert!(!decline.contains("/api/"), "no route of its own: {decline}");

    let review = app_js_function("createPlanReview");
    let hook = &review[review
        .find("planReviewDeclines.set(wrapper,")
        .expect("the plan card registers its decline")..];
    let hook = &hook[..hook.find("});").unwrap()];
    assert!(hook.contains("decide(false, text)"), "{hook}");
    assert!(
        !hook.contains("decide(true"),
        "typing never approves: {hook}"
    );
    assert!(hook.contains("changesInput.value = text;"), "{hook}");
    assert!(review.contains("if (!decideInWorker) {"), "{review}");
    // `decide` is the card's Decline → Send: one route, `approved` false with
    // the typed changes.
    assert!(review.contains("approved: approved ? \"true\" : \"false\","));
    assert!(review.contains("changes: approved ? \"\" : changesText,"));
    assert!(app_js_function("streamResumePlanRequest").contains("\"/api/resume-plan-stream\""));

    let state = app_js_function("composerDecisionState");
    assert!(
        state.contains("if (!current?.frame) return null;"),
        "{state}"
    );
    assert!(
        state.contains("{ placeholder: PLAN_CHANGES_PLACEHOLDER, locked: false }"),
        "{state}"
    );
    assert!(
        super::APP_JS
            .contains("const PLAN_CHANGES_PLACEHOLDER = \"Describe changes to the plan\";")
    );
    // After a restart the plan review sits inside its recovery card.
    assert!(app_js_function("planDeclineFor").contains("querySelectorAll(\".plan-review\")"));
}

/// Spec 113 requirement 2 and D2: every pending kind but a plan disables the
/// composer and Send, with a placeholder naming the decision; a kind the
/// table does not know still locks. Answering the card settles the decision,
/// which runs `syncComposerStatus` and so unlocks the composer.
#[test]
fn app_js_other_pending_decisions_lock_the_composer() {
    let app = super::APP_JS;
    let table = &app[app
        .find("const COMPOSER_LOCK_PLACEHOLDERS = {")
        .expect("the table")..];
    let table = &table[..table.find("};").unwrap()];
    for line in [
        r#"question: "Answer the question","#,
        r#"command: "Approve or reject the command first","#,
        r#"patch: "Apply or reject the patch first","#,
        r#"checks: "Decide the checks first","#,
        r#"fetch: "Allow or decline the fetch first","#,
        r#"round_budget: "Answer the round budget first","#,
        r#"fallback: "Answer the provider fallback first","#,
        r#"context_review: "Send or decline the context first","#,
    ] {
        assert!(table.contains(line), "missing {line}: {table}");
    }
    assert!(!table.contains("plan:"), "a plan does not lock: {table}");
    // Every kind a card can be pending for is either in the table or a plan.
    let fields = &app[app.find("const PENDING_DECISION_FIELDS = {").unwrap()..];
    let fields = &fields[..fields.find("};").unwrap()];
    for line in fields.lines().skip(1) {
        let Some((kind, _)) = line.trim().split_once(':') else {
            continue;
        };
        assert!(
            kind == "plan" || table.contains(&format!("{kind}: ")),
            "{kind} has no lock placeholder"
        );
    }
    let state = app_js_function("composerDecisionState");
    assert!(
        state.contains("COMPOSER_LOCK_PLACEHOLDERS[kind] || \"Answer the card above first\""),
        "{state}"
    );
    assert!(
        state.contains("return { placeholder, locked: true };"),
        "{state}"
    );
    let lock = app_js_function("syncComposerLock");
    assert!(
        lock.contains("field.placeholder = composerDecisionLock;"),
        "{lock}"
    );
    assert!(lock.contains("field.disabled = false;"), "{lock}");
    assert!(app_js_function("settlePendingDecision").contains("syncComposerStatus();"));
}

/// Spec 112 requirements 1, 2 and 5: the three composer boxes are one slot,
/// `#composer-status`, derived by one function in the order Needs you →
/// Working → Busy elsewhere. An `IntersectionObserver` (viewport root,
/// clipped by `#chat-log`) hides Needs you while the pending frame is in view and Working while the
/// running action's status row is; the slot never holds a Stop, its text is
/// clipped to one line with the rest in `title`, and the in-message pointer
/// is gone.
#[test]
fn app_js_has_one_composer_status_slot() {
    let html = super::INDEX_HTML;
    let slot = &html[html.find(r#"id="composer-status""#).expect("the slot")..];
    let slot = &slot[..slot.find("</div>").unwrap()];
    for id in [
        "composer-status-label",
        "composer-status-clock",
        "composer-status-btn",
    ] {
        assert!(slot.contains(id), "{id}: {slot}");
    }
    assert_eq!(
        slot.matches("<button").count(),
        1,
        "one optional button: {slot}"
    );
    assert!(
        !slot.contains("Stop"),
        "the slot never holds a Stop: {slot}"
    );

    let entry = app_js_function("composerStatusEntry");
    let needs = entry.find("kind: \"needs-you\"").expect("Needs you");
    let working = entry.find("kind: \"working\"").expect("Working");
    let busy = entry.find("kind: \"busy\"").expect("Busy elsewhere");
    assert!(needs < working && working < busy, "priority: {entry}");
    assert!(
        entry.contains("!statusHomeShown(decision.frame)"),
        "{entry}"
    );
    assert!(
        entry.contains("[...action.homes].some(statusHomeShown)"),
        "{entry}"
    );
    assert!(entry.contains("title:"), "{entry}");

    let observe = app_js_function("observeStatusHome");
    assert!(observe.contains("new IntersectionObserver("), "{observe}");
    // The viewport root, clipped by `#chat-log`'s scrolling, holds in both
    // layouts; a `#chat-log` root would see every home in view below 1100px.
    assert!(!observe.contains("root:"), "{observe}");
    assert!(app_js_function("homeIsInView").contains("$(\"chat-log\")"));
    assert!(app_js_function("startTurnIndicator").contains("observeStatusHome(row);"));
    assert!(app_js_function("createRunningRow").contains("observeStatusHome(row)"));
    assert!(app_js_function("clipStatusText").contains("max = 60"));

    let sync = app_js_function("syncComposerStatus");
    assert!(sync.contains("clipStatusText(entry.text)"), "{sync}");
    assert!(sync.contains("slot.title = entry.title;"), "{sync}");
    assert!(
        !sync.contains("createElement(\"button\")") && !sync.contains("stop()"),
        "{sync}"
    );

    let app = super::APP_JS;
    for gone in [
        "composer-running",
        "composer-decision",
        "composer-invisible-work",
        "syncComposerRunning",
        "syncComposerDecision",
        "syncInvisibleWork",
        "appendPendingPointer",
        "pending-decision-pointer",
        "the card is below",
    ] {
        assert!(!app.contains(gone), "app.js names {gone}");
        assert!(!html.contains(gone), "index.html names {gone}");
        assert!(!super::STYLE_CSS.contains(gone), "style.css names {gone}");
    }
}

/// Spec 109 Task 3: a failed check's baseline asks on the ordinary check
/// card, with the engine's own heading and note saying why; a card without
/// them keeps the check card's wording.
#[test]
fn a_check_card_shows_the_engines_title_and_note_when_it_has_them() {
    let card = app_js_function("createCheckBatchCard");
    assert!(
        card.contains("batch.title || \"Checks need approval\""),
        "{card}"
    );
    assert!(card.contains("batch.note ||"), "{card}");
    let json = super::check_batch_json(Some(&workspace_engine::AgentCheckBatch {
        id: "checkbatch_1".to_string(),
        checks: Vec::new(),
        title: Some("A check failed: is the failure new?".to_string()),
        note: None,
    }));
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["title"], "A check failed: is the failure new?");
    assert!(value["note"].is_null(), "{value}");
}

/// Spec 107 Task 3, requirement 3: a turn paused on the user keeps its
/// status row, relabelled with the engine's `statusLabel`, until the
/// decision is settled or the next turn starts; the same row comes back on
/// reload. A pending state without a decision does not claim one.
#[test]
fn app_js_status_line_waits_with_the_engine_label_until_the_turn_ends() {
    let finish = app_js_function("startTurnIndicator");
    assert!(finish.contains("clearWaitingIndicator();"));
    assert!(finish.contains("state === \"pending\" && pendingDecision"));
    assert!(finish.contains("showWaitingIndicator(row, pendingDecision.decision.statusLabel"));
    let show = app_js_function("showWaitingIndicator");
    assert!(show.contains("row.dataset.state = \"waiting\""));
    assert!(show.contains("formatClock(Date.now() - entry.since)"));
    for settled in ["settlePendingDecision", "resetPendingDecision"] {
        assert!(
            app_js_function(settled).contains("clearWaitingIndicator()"),
            "{settled}"
        );
    }
    assert!(
        app_js_function("restorePendingDecision")
            .contains("restoreWaitingIndicator(body, decision.statusLabel")
    );
    let css = super::STYLE_CSS;
    assert!(css.contains(".turn-indicator[data-state=\"waiting\"]"));
}

/// Spec 107 Task 3, requirement 3: while working, the row names the
/// current activity with its own clock and the turn's total apart from
/// it. Thinking updates belong to the model call already timed; reasoning
/// shows only its size (spec 81), never its text.
#[test]
fn app_js_status_line_times_the_current_activity_apart_from_the_turn() {
    let indicator = app_js_function("startTurnIndicator");
    assert!(indicator.contains("turn-indicator-total"));
    assert!(indicator.contains("formatClock(now - stepStartedAt)"));
    assert!(indicator.contains("`turn ${formatClock(now - startedAt)}`"));
    assert!(indicator.contains("if (payload.phase !== \"thinking\") beginActivity();"));
    assert!(indicator.contains("formatCharCount(payload.reasoningChars)"));
    assert!(!indicator.contains("reasoningContent"));
    let clock = app_js_function("formatClock");
    assert!(clock.contains("`${minutes}m ${seconds % 60}s`"));
    assert!(super::APP_JS.contains("model: \"Waiting for the model\""));
}

/// Spec 107 Task 3, requirement 4: the open step of a task waiting on the
/// user says so, from `plan.waiting` on reload and from the pending
/// decision live (whose `plan` events always say `waiting: false`).
#[test]
fn app_js_plan_step_reads_waiting_for_you_while_a_decision_is_pending() {
    assert!(
        app_js_function("planStepLiveLabel")
            .contains("if (plan.waiting) return \"waiting for you\";")
    );
    assert!(app_js_function("renderPlanPanel").contains("planStepLiveLabel(plan)"));
    assert!(app_js_function("setPendingDecision").contains("markPlanWaiting(planMessage, true)"));
    assert!(
        app_js_function("settlePendingDecision")
            .contains("markPlanWaiting(pendingDecision.planMessage, false)")
    );
}

/// The done event of an SSE turn, whichever `data:` line carries it.
fn sse_done(response: &str) -> serde_json::Value {
    for line in response.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        if value.get("response").is_some() && value.get("sessionId").is_some() {
            return value;
        }
    }
    panic!("no done event in response: {response}");
}

/// Creates a session whose patch is applied and therefore verification is
/// due, and returns the session id.
fn due_session(repo: &std::path::Path, changes: &[(&str, &str)]) -> String {
    let repo_arg = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
    for (path, before) in changes {
        let file = repo.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, before).unwrap();
    }
    let session = engine
        .session_store
        .create_session(
            &workspace_engine::hash::repository_id_for_root(repo),
            "verification",
        )
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "patch", "provider", "model")
        .unwrap();
    let proposed = changes
        .iter()
        .map(|(path, _)| workspace_engine::ProposedChange {
            path: (*path).to_string(),
            new_content: format!("// fixed {path}\n"),
            status: None,
            allow_restricted: false,
        })
        .collect::<Vec<_>>();
    let mut patch = engine
        .patch_engine
        .create_patch(repo, &proposed, Some(&task.id), "test change")
        .unwrap();
    patch.session_id = session.id.clone();
    engine.patch_store.save(&patch).unwrap();
    engine
        .edit_orchestrator
        .apply_stored_patch(repo, &patch.id, None, None, "tester", false)
        .unwrap();
    session.id
}

fn temp_shell_repo(name: &str) -> std::path::PathBuf {
    let repo = std::env::temp_dir().join(format!(
        "damaian-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(repo.join(".damaian")).unwrap();
    isolated_data_dir();
    repo
}

#[test]
fn verify_endpoint_reports_no_checks_available() {
    // The route builds an adapter before the engine decides nothing is due.
    unsafe { std::env::set_var("DAMAIAN_MOCK_MODEL_RESPONSE", "No repair.") };
    let repo = temp_shell_repo("verify-endpoint");
    let session = due_session(&repo, &[("src/a.rs", "fn a() {}\n")]);
    let (port, token) = serve_for_test();
    let repo_arg = repo.to_string_lossy().to_string();

    let response = post_fields_for_test(
        port,
        &token,
        "/api/verify-stream",
        &[("repo", &repo_arg), ("session_id", &session)],
    );

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let done = sse_done(&response);
    let report = &done["verificationReport"];
    assert_eq!(report["outcome"], "no_checks_available", "{done}");
    assert!(
        done["response"]
            .as_str()
            .unwrap_or_default()
            .contains("No checks were available"),
        "{done}"
    );

    fs::remove_dir_all(&repo).ok();
}

/// Spec 102 Task 3: the report's provenance partitions, repair targets,
/// labels and actions are engine data. The shell serialises them and the
/// card prints them; neither derives a classification from a check outcome.
#[test]
fn verification_provenance_and_actions_survive_json() {
    let check = CheckId::new("", "cargo test");
    let finding = Finding::new(
        FindingDraft {
            source: FindingSource::Compiler,
            severity: Severity::Error,
            summary: "mismatched types".to_string(),
            details: None,
            range: Some(SourceRange {
                path: "src/a.rs".to_string(),
                start_line: 4,
                start_column: Some(1),
                end_line: None,
                end_column: None,
            }),
            code: Some("E0308".to_string()),
        },
        &SecretScanner::new(Vec::new()),
    );
    let evidence = FindingEvidence::from_finding(&finding);
    let fingerprint = FailureFingerprint::compute(
        &check,
        std::slice::from_ref(&evidence),
        &CheckTermination::Exited { exit_code: 1 },
    );
    let events = [
        VerificationEvent::Started {
            task_id: "task".to_string(),
            attempt: 0,
            checks: vec![check.clone()],
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check: check.clone(),
            outcome: CheckOutcome::Ran {
                exit_code: Some(1),
                marker_id: "marker".to_string(),
                termination: Some(CheckTermination::Exited { exit_code: 1 }),
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::Provenance {
            task_id: "task".to_string(),
            record: CheckProvenanceRecord {
                check: check.clone(),
                provenance: FailureProvenance::NewRegression,
                termination: CheckTermination::Exited { exit_code: 1 },
                fingerprint,
                findings: vec![evidence.clone()],
                introduced: vec![evidence],
            },
        },
        VerificationEvent::Finished {
            task_id: "task".to_string(),
            outcome: VerificationOutcome::Failed,
        },
    ];
    let report = VerificationReport::build(
        &fold_verification(events.iter()),
        &TaskPlan::new("task", 0),
        &[],
        &BTreeMap::new(),
    );
    let json = verification_report_json(Some(&report));

    let value: serde_json::Value = serde_json::from_str(&json).expect("valid report JSON");
    assert_eq!(value["provenance"][0]["provenance"], "new_regression");
    assert_eq!(
        value["provenance"][0]["checks"][0]["label"],
        "cargo test (repository root)"
    );
    assert_eq!(
        value["provenance"][0]["checks"][0]["reason"],
        "exit code 1; 1 finding"
    );
    assert_eq!(
        value["provenance"][0]["summary"],
        "These checks passed before the patch and fail now. Automatic repair receives only these findings."
    );
    assert_eq!(
        value["provenance"][0]["checks"][0]["evidenceLines"][0],
        "error: [E0308] mismatched types at src/a.rs:4:1"
    );
    assert_eq!(
        value["provenance"][0]["checks"][0]["evidence"][0]["summary"],
        "mismatched types"
    );
    assert_eq!(
        value["repairTargets"][0]["evidence"][0]["summary"],
        "mismatched types"
    );
    assert_eq!(value["actions"][0]["kind"], "review_introduced_failure");
    assert!(
        value["actions"][0]["label"]
            .as_str()
            .unwrap_or_default()
            .contains("Review the repair"),
        "{json}"
    );
    assert!(json.contains("Introduced regressions"), "{json}");
    assert!(!json.contains("raw output"), "{json}");
}

#[test]
fn an_environment_failure_serialises_with_an_action_and_no_repair_target() {
    let check = CheckId::new("", "cargo test");
    let fingerprint =
        FailureFingerprint::compute(&check, &[], &CheckTermination::MissingExecutable);
    let events = [
        VerificationEvent::Started {
            task_id: "task".to_string(),
            attempt: 0,
            checks: vec![check.clone()],
        },
        VerificationEvent::Check {
            task_id: "task".to_string(),
            check: check.clone(),
            outcome: CheckOutcome::Environment {
                termination: CheckTermination::MissingExecutable,
            },
            file_hashes: BTreeMap::new(),
            generation: 0,
        },
        VerificationEvent::Provenance {
            task_id: "task".to_string(),
            record: CheckProvenanceRecord {
                check: check.clone(),
                provenance: FailureProvenance::EnvironmentFailure,
                termination: CheckTermination::MissingExecutable,
                fingerprint,
                findings: Vec::new(),
                introduced: Vec::new(),
            },
        },
        VerificationEvent::Finished {
            task_id: "task".to_string(),
            outcome: VerificationOutcome::Failed,
        },
    ];
    let report = VerificationReport::build(
        &fold_verification(events.iter()),
        &TaskPlan::new("task", 0),
        &[],
        &BTreeMap::new(),
    );
    let value: serde_json::Value =
        serde_json::from_str(&verification_report_json(Some(&report))).unwrap();

    assert_eq!(value["provenance"][0]["provenance"], "environment_failure");
    assert_eq!(value["repairTargets"], serde_json::json!([]));
    assert_eq!(value["actions"][0]["kind"], "fix_environment");
    assert_eq!(value["actions"][0]["label"], "Fix the environment");
}

/// Spec 96: a reload offers the explicit action from the durable log, and
/// stops offering it once a newer repair task exists.
#[test]
fn session_payload_offers_continue_repair_only_at_the_latest_limit() {
    let repo = temp_shell_repo("continue-repair-offer");
    let repo_arg = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
    let session = engine
        .session_store
        .create_session(
            &workspace_engine::hash::repository_id_for_root(&repo),
            "limited",
        )
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "verify", "provider", "model")
        .unwrap();
    engine
        .session_store
        .update_task_status(&task, workspace_engine::TaskStatus::Failed, None)
        .unwrap();
    engine
        .session_store
        .append_verification_event(
            &session.id,
            &VerificationEvent::Started {
                task_id: task.id.clone(),
                attempt: 0,
                checks: Vec::new(),
            },
        )
        .unwrap();
    engine
        .session_store
        .append_verification_event(
            &session.id,
            &VerificationEvent::Finished {
                task_id: task.id.clone(),
                outcome: VerificationOutcome::RepairLimitExhausted,
            },
        )
        .unwrap();

    let offered = session_verification_json(&engine, &session.id);
    assert!(offered.contains(&task.id), "{offered}");
    assert!(offered.contains("\"canContinueRepair\":true"), "{offered}");

    let newer = engine
        .session_store
        .create_task(&session.id, "repair", "provider", "model")
        .unwrap();
    let not_offered = session_verification_json(&engine, &session.id);
    assert!(
        not_offered.contains("\"canContinueRepair\":false"),
        "a newer repair task is in progress: {not_offered}"
    );
    assert_ne!(task.id, newer.id);

    fs::remove_dir_all(&repo).ok();
}

/// Spec 102 Task 4: the reloaded session payload carries the same typed
/// stall, and it never offers Spec 96's explicit repair continuation.
#[test]
fn session_payload_renders_a_durable_repair_stall_without_offering_continue() {
    let repo = temp_shell_repo("repair-stall-offer");
    let repo_arg = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
    let session = engine
        .session_store
        .create_session(
            &workspace_engine::hash::repository_id_for_root(&repo),
            "stalled",
        )
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "verify", "provider", "model")
        .unwrap();
    engine
        .session_store
        .update_task_status(&task, workspace_engine::TaskStatus::Failed, None)
        .unwrap();
    let check = CheckId::new("", "cargo test");
    let entry = workspace_engine::RepairStateEntry {
        failure_fingerprint: FailureFingerprint::compute(
            &check,
            &[],
            &CheckTermination::Exited { exit_code: 1 },
        ),
        patch_digest: "sha256:patch".to_string(),
        task_diff_digest: "sha256:diff".to_string(),
    };
    let stall =
        workspace_engine::RepairStall::new(workspace_engine::RepairStallReason::RepeatedFailure);
    for event in [
        VerificationEvent::Started {
            task_id: task.id.clone(),
            attempt: 1,
            checks: vec![check],
        },
        VerificationEvent::RepairStalled {
            task_id: task.id.clone(),
            entry,
            stall: stall.clone(),
        },
    ] {
        engine
            .session_store
            .append_verification_event(&session.id, &event)
            .unwrap();
    }

    let offered = session_verification_json(&engine, &session.id);
    let value: serde_json::Value = serde_json::from_str(&offered).expect("valid payload");
    assert_eq!(value["taskId"], task.id);
    assert_eq!(value["outcome"], "repair_stalled");
    assert_eq!(
        value["canContinueRepair"], false,
        "a stall is not the numeric limit: {offered}"
    );
    assert_eq!(value["repairStall"]["label"], stall.label);
    assert_eq!(value["repairStall"]["explanation"], stall.explanation);
    assert_eq!(
        value["repairStall"]["action"]["kind"],
        "review_stalled_repair"
    );

    // A fresh store over the same log renders exactly the same stop.
    drop(engine);
    let reopened = engine_for_repo(&repo_arg).expect("engine for the same repo");
    let reloaded = session_verification_json(&reopened, &session.id);
    assert_eq!(reloaded, offered);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn resume_checks_endpoint_records_declined_checks() {
    unsafe { std::env::set_var("DAMAIAN_MOCK_MODEL_RESPONSE", "No repair.") };
    let repo = temp_shell_repo("resume-checks-endpoint");
    fs::create_dir_all(repo.join("packages/web/src")).unwrap();
    fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    fs::write(
        repo.join("packages/web/package.json"),
        "{\"scripts\":{\"test\":\"node test.js\"}}\n",
    )
    .unwrap();
    let session = due_session(
        &repo,
        &[
            ("src/a.rs", "fn a() {}\n"),
            ("packages/web/src/index.ts", "export {};\n"),
        ],
    );
    let (port, token) = serve_for_test();
    let repo_arg = repo.to_string_lossy().to_string();

    let paused = post_fields_for_test(
        port,
        &token,
        "/api/verify-stream",
        &[("repo", &repo_arg), ("session_id", &session)],
    );
    let paused = sse_done(&paused);
    let checks = paused["checkBatch"]["checks"]
        .as_array()
        .expect("both checks need approval");
    assert_eq!(checks.len(), 2, "{paused}");
    let decisions = checks
        .iter()
        .map(|check| {
            serde_json::json!({
                "proposalId": check["proposalId"],
                "approved": false,
            })
        })
        .collect::<Vec<_>>();
    let decisions = serde_json::Value::Array(decisions).to_string();

    let resumed = post_fields_for_test(
        port,
        &token,
        "/api/resume-checks-stream",
        &[
            ("repo", &repo_arg),
            (
                "proposal_id",
                paused["checkBatch"]["proposalId"]
                    .as_str()
                    .expect("batch id"),
            ),
            ("decisions", &decisions),
        ],
    );

    assert!(resumed.starts_with("HTTP/1.1 200"), "{resumed}");
    let done = sse_done(&resumed);
    let report = &done["verificationReport"];
    // Both checks were declined, so nothing ran: spec 78 records that
    // as `nothing_verified`, where this once pinned `passed`.
    assert_eq!(report["outcome"], "nothing_verified", "{done}");
    let skipped = report["skipped"].as_array().expect("skipped checks");
    let declined = skipped
        .iter()
        .filter(|item| item["reason"]["reason"] == "declined_by_user")
        .count();
    assert_eq!(declined, 2, "{done}");
    assert!(
        skipped.iter().all(|item| {
            let reason = &item["reason"]["reason"];
            reason == "declined_by_user" || reason == "no_running_target"
        }),
        "{done}"
    );

    fs::remove_dir_all(&repo).ok();
}

/// Spec 95: one Workers board for every project, opened from the header
/// ⋮, and no Workers entry in the sidebar. Checks the source shape.
#[test]
fn the_workers_board_is_one_panel_in_the_header_menu() {
    assert!(
        !super::APP_JS.contains("renderWorkersEntry"),
        "the sidebar entry is gone"
    );
    assert!(!super::APP_JS.contains("workersBoardProject"));
    let menu = &super::APP_JS[super::APP_JS.find("function toggleThreadMenu").unwrap()..];
    let menu = &menu[..menu.find("\n}\n").unwrap()];
    assert!(menu.contains("label: \"Workers\""), "{menu}");
    assert!(
        menu.contains("onSelect: () => openWorkersBoard()"),
        "{menu}"
    );
    assert!(
        super::APP_JS.contains("await api(\"/api/workers\");"),
        "the board polls every project"
    );
    assert!(
        super::APP_JS.contains("\"Project\""),
        "the board names each row's project"
    );
    assert!(super::APP_JS.contains("normalizeProjectPath(row.sourceRepository)"));
    assert!(!include_str!("../static/style.css").contains(".project-workers-entry"));
}

/// Spec 102 Task 5: the shared patch payload carries the engine's typed
/// repair-scope review, so a live turn, a worker and a reattached card use
/// one shape.
#[test]
fn the_shared_patch_payload_carries_the_engine_repair_scope() {
    let review = RepairScopeReview::compute(
        &["src/a.rs".to_string()],
        &["src/report.rs".to_string()],
        &["src/new.rs".to_string()],
    );
    let payload = patch_payload_json("patch_1", "repair", &[], Some(&review));
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(value["patchId"], "patch_1");
    assert_eq!(value["repairScope"]["oneShot"], true);
    assert_eq!(value["repairScope"]["expansionPaths"][0], "src/new.rs");
    assert_eq!(
        value["repairScope"]["actions"][0]["kind"],
        "approve_scope_expansion_once"
    );
    assert_eq!(value["repairScope"]["label"], review.label);
}

/// The desktop card prints the engine's repair-scope fields and keeps no
/// path classification or permission decision of its own. The engine
/// returns `repairScope` only for an expansion, so presence alone decides
/// rendering; the card never reads `oneShot`, a reason/action kind or a
/// decision field.
#[test]
fn the_patch_card_renders_only_engine_repair_scope_facts() {
    let card = super::APP_JS
        .split("function createPatchPreview(")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction ").next())
        .expect("the patch preview");
    let without_comments = |text: &str| {
        text.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    for field in [
        "payload.repairScope",
        "scope.label",
        "scope.explanation",
        "scope.reasonGroups",
        "group.paths",
        "scope.actions",
        "action.label",
        "action.detail",
    ] {
        assert!(card.contains(field), "{field} in {card}");
    }
    let renderer = card
        .split("function renderRepairScope()")
        .nth(1)
        .and_then(|rest| rest.split("\n  renderRepairScope();").next())
        .expect("the repair-scope renderer");
    assert!(
        renderer.contains("const scope = payload.repairScope;"),
        "{renderer}"
    );
    assert!(renderer.contains("if (!scope)"), "{renderer}");
    let renderer_code = without_comments(renderer);
    for forbidden in [
        "oneShot",
        "one_shot",
        "expansionPaths",
        "taskChangedPaths",
        "findingPaths",
        "reasonGroups.kind",
        "group.kind",
        "action.kind",
        "decision",
        "isProtected",
        "classify",
        "permission",
        "status",
    ] {
        assert!(
            !renderer_code.contains(forbidden),
            "the card must render on repairScope presence alone, not {forbidden}"
        );
    }
    let card_code = without_comments(card);
    for forbidden in [
        "oneShot",
        "taskChangedPaths",
        "findingPaths",
        "expansionPaths.includes",
        "isProtected",
        "classify",
    ] {
        assert!(
            !card_code.contains(forbidden),
            "the card must not classify paths itself: {forbidden}"
        );
    }
}

/// Spec 82: the declined-everything answer above is what a "Submit checks"
/// button over unticked boxes sent without saying so. The card now names
/// the outcome on its button, and its boxes still start unticked. This
/// checks the source shape, as the other JS-only assertions here do.
#[test]
fn the_check_card_button_says_what_it_will_do() {
    let app = include_str!("../static/app.js");
    let label = app
        .split("function checkSubmitLabel(runCount, total) {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("the submit label helper");
    for wording in [
        "\"Skip the check\"",
        "\"Skip all checks\"",
        "`Run ${total} check${total === 1 ? \"\" : \"s\"}`",
        "`Run ${runCount}, skip ${total - runCount}`",
    ] {
        assert!(label.contains(wording), "{wording} in {label}");
    }

    let card = app
        .split("function createCheckBatchCard(")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction ").next())
        .expect("the check card");
    assert!(!card.contains("Submit checks"), "{card}");
    assert!(card.contains("run.checked = reattached;"), "{card}");
    assert!(
        card.contains("checkSubmitLabel(ticked, rows.length)"),
        "{card}"
    );
    assert!(
        card.contains("run.addEventListener(\"change\", syncSubmit)"),
        "{card}"
    );
    assert!(card.contains("\"Run all\""), "{card}");
    // A single unticked check still gets a run button, and skipping is
    // never the primary one.
    assert!(card.contains("\"Run the check\""), "{card}");
    assert!(
        card.contains("runAll.hidden = ticked === rows.length;"),
        "{card}"
    );
    assert!(
        card.contains("submit.classList.toggle(\"btn-primary\", ticked > 0);"),
        "{card}"
    );
    let style = include_str!("../static/style.css");
    assert!(
        style.contains(".check-batch-run,"),
        "the checkbox keeps its size"
    );
}

/// A worker's process can be idle while a patch or fallback card still
/// needs a user decision. The session view must retain the durable task
/// result rather than presenting that state as unexplained idle.
#[test]
fn worker_session_explains_task_outcomes_and_stale_progress() {
    // Spec 101 Task 3: labels, dots, groups, stale-progress sentences and
    // the next command are engine data. The browser renders the returned
    // model and keeps no status-to-outcome table of its own.
    for removed in [
        "const WORKER_TASK_OUTCOMES",
        "const WORKER_END_LABELS",
        "const WORKER_COMPOSER_MODES",
        "const WORKER_WIRE_COMMANDS",
        "function workerViewOutcome",
        "function workerComposerMode",
        "function workerActivityDetail",
        "function chatCompletionState",
        "function markMessageToolBudgetExhausted",
        "function markMessageTokenBudgetExhausted",
        "function markMessageIncomplete",
    ] {
        assert!(!super::APP_JS.contains(removed), "{removed}");
    }
    for forbidden in [
        "taskStatus ===",
        "taskStatus ==",
        "taskControl.outcome ===",
        "taskControl.outcome ==",
        ".outcome === \"complete\"",
        ".outcome === \"incomplete\"",
        // A reload groups paused turns from the engine's presentation fact,
        // not by comparing the durable task status.
        "task.status === \"waiting_for_approval\"",
        "task.status == \"waiting_for_approval\"",
    ] {
        assert!(!super::APP_JS.contains(forbidden), "{forbidden}");
    }
    assert!(super::APP_JS.contains("task.turnPresentation?.paused"));
    assert!(super::APP_JS.contains("view.control"));
    assert!(super::APP_JS.contains("answer.control"));
    assert!(super::APP_JS.contains("status.headline"));
    assert!(super::APP_JS.contains("status.detail"));
    assert!(super::APP_JS.contains("status.actions"));
    assert!(super::APP_JS.contains("view.control?.composer"));
    assert!(super::APP_JS.contains("row.control?.boardAction"));
    assert!(super::APP_JS.contains("function renderTaskControlInto"));
    assert!(super::APP_JS.contains("/api/continue-task-stream"));
    assert!(super::APP_JS.contains("taskControl"));
    assert!(super::APP_JS.contains("turnPresentation"));
    assert!(super::APP_JS.contains("indicatorState"));
    assert!(super::APP_JS.contains("stopNotice"));
    assert!(super::APP_JS.contains("action.wireCommand"));
    assert!(super::APP_JS.contains("action.taskId"));
    // A failed stream is not a successful continuation: the card stays
    // and the button is restored instead of being removed as if it ran.
    assert!(super::APP_JS.contains("const succeeded = await runStreamTaskAction"));
    assert!(super::APP_JS.contains("if (!succeeded)"));
    let send = super::APP_JS
        .split("async function sendChatPrompt")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction ").next())
        .expect("sendChatPrompt");
    assert!(
        send.contains("if (sessionSubmitting()) return false;"),
        "{send}"
    );
    assert!(send.contains("return true;"), "{send}");
    assert!(send.contains("return false;"), "{send}");
}

/// Spec 100: the two worker-prompt actions and the result card are source
/// shapes here because the UI has no browser assertion in this suite.
#[test]
fn worker_prompts_offer_start_worker_and_work_here_instead() {
    let app = include_str!("../static/app.js");
    let actions = app
        .split("function attachWorkerPromptActions")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction ").next())
        .expect("the worker-prompt actions");
    assert!(actions.contains("\"Start worker\""), "{actions}");
    assert!(actions.contains("\"Work here instead\""), "{actions}");
    assert!(actions.contains("sendChatPrompt"), "{actions}");
    assert!(actions.contains("INLINE_WORK_INSTRUCTION"), "{actions}");
    // Both actions are absent in a worker session and on a block whose tag
    // is not exactly `worker-prompt`.
    assert!(actions.contains("isWorkerSession(sessionId)"), "{actions}");
    assert!(
        actions.contains("code.className !== \"language-worker-prompt\""),
        "{actions}"
    );
    assert!(actions.contains("\"assistant\""), "{actions}");
}

/// Spec 100: a durable worker return becomes a card in the parent, with the
/// explicit continuation and a link back to the worker.
#[test]
fn a_worker_result_card_offers_continue_with_result() {
    assert!(super::APP_JS.contains("function renderWorkerResultCards"));
    assert!(super::APP_JS.contains("worker-result-card"));
    assert!(super::APP_JS.contains("Continue with result"));
    assert!(super::APP_JS.contains("Open worker"));
    assert!(super::APP_JS.contains("\"This is a result, not an approval."));
    assert!(super::APP_JS.contains("payload.workerReturns"));
    assert!(super::APP_JS.contains("workerReturn.control?.headline"));
    assert!(
        super::APP_JS.contains("/api/continue-worker-result-stream"),
        "the action must consume the return through its own route"
    );
}

/// Spec 32 Task 6: a `before_patch_apply` hook's denial is the error text
/// the UI already shows, so its reason reaches the user, and a warning is
/// returned beside the applied files.
#[test]
fn apply_patch_endpoint_reports_hook_denials_and_warnings() {
    let repo = std::env::temp_dir().join(format!(
        "damaian-apply-hook-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(repo.join(".damaian")).unwrap();
    fs::write(repo.join("notes.txt"), "one\n").unwrap();

    let data_dir = isolated_data_dir().to_path_buf();
    let script = data_dir.join("patch-hook.sh");
    let deny = "cat <<'JSON'\n{\"hookApiVersion\":1,\"verdict\":\"deny\",\"reason\":\"migrations/ is applied by the release process\"}\nJSON\n";
    fs::write(&script, deny).unwrap();
    let user_config = data_dir.join("config").join("user.conf");
    fs::create_dir_all(user_config.parent().unwrap()).unwrap();
    let hook_line = format!(
        "hook.no_migrations.command=/bin/sh\nhook.no_migrations.args={}\nhook.no_migrations.events=before_patch_apply\n",
        script.to_string_lossy()
    );
    fs::write(&user_config, &hook_line).unwrap();

    let repo_arg = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
    let patch = engine
        .patch_engine
        .create_patch(
            &repo,
            &[workspace_engine::ProposedChange {
                path: "notes.txt".to_string(),
                new_content: "two\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            None,
            "hook apply",
        )
        .expect("create patch");
    engine.patch_store.save(&patch).expect("store patch");

    let (port, token) = serve_for_test();
    let fields = [("repo", repo_arg.as_str()), ("patch_id", patch.id.as_str())];
    let denied = post_fields_for_test(port, &token, "/api/apply-patch", &fields);
    assert!(denied.starts_with("HTTP/1.1 500"), "{denied}");
    assert!(denied.contains("no_migrations"), "{denied}");
    assert!(denied.contains("release process"), "{denied}");
    assert_eq!(
        fs::read_to_string(repo.join("notes.txt")).unwrap(),
        "one\n",
        "a denied apply wrote the file"
    );

    let warn = "cat <<'JSON'\n{\"hookApiVersion\":1,\"verdict\":\"warn\",\"reason\":\"the patch was checked by the hook\"}\nJSON\n";
    fs::write(&script, warn).unwrap();
    let applied = post_fields_for_test(port, &token, "/api/apply-patch", &fields);
    let _ = fs::remove_file(&user_config);
    assert!(applied.starts_with("HTTP/1.1 200"), "{applied}");
    assert!(
        applied.contains("\"hookWarnings\":[\"the patch was checked by the hook\"]"),
        "{applied}"
    );
    assert_eq!(fs::read_to_string(repo.join("notes.txt")).unwrap(), "two\n");

    fs::remove_dir_all(&repo).ok();
}

/// The two rewind routes over real HTTP, because the UI reaches them that
/// way and a route that compiles is not a route that answers.
#[test]
fn checkpoint_routes_list_and_rewind_over_http() {
    let repo = std::env::temp_dir().join(format!(
        "damaian-rewind-route-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(repo.join(".damaian")).unwrap();
    isolated_data_dir();
    fs::write(repo.join("app.js"), "export const a = 1;\n").unwrap();
    fs::write(repo.join("other.js"), "export const b = 1;\n").unwrap();

    let repo_arg = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
    let manifest = engine
        .checkpoint_store
        .create_checkpoint(
            &repo,
            workspace_engine::CheckpointRequest {
                session_id: "session_route",
                task_id: Some("task_route"),
                user_message_id: Some("msg_route"),
                summary: "Before: bump the constant",
                conversation: workspace_engine::CheckpointConversation {
                    last_event_seq: 1,
                    task_status: "running".to_string(),
                },
                pending_approvals: Vec::new(),
                paths: vec![
                    workspace_engine::CheckpointPath {
                        path: "app.js".to_string(),
                        origin: workspace_engine::CheckpointOrigin::Patch,
                    },
                    workspace_engine::CheckpointPath {
                        path: "other.js".to_string(),
                        origin: workspace_engine::CheckpointOrigin::Patch,
                    },
                ],
            },
        )
        .expect("create checkpoint");
    fs::write(repo.join("app.js"), "export const a = 2;\n").unwrap();
    fs::write(repo.join("other.js"), "export const b = 2;\n").unwrap();
    engine
        .checkpoint_store
        .seal_checkpoint(&repo, &manifest)
        .expect("seal checkpoint");

    let options = ShellOptions::new(0, None);
    let token = options.api_token.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let port = listener.local_addr().expect("local addr").port();
    spawn_with_shell_stack("test-shell", move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let _ = handle_connection(&mut stream, &options);
        }
    })
    .expect("spawn the test shell");

    let send = |request: String| {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
        stream.write_all(request.as_bytes()).expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    };

    let listed = send(format!(
        "GET /api/checkpoints?repo={}&session_id=session_route HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n",
        percent_encode_for_test(&repo_arg)
    ));
    assert!(listed.contains(&manifest.checkpoint_id), "{listed}");
    assert!(listed.contains("\"fileCount\":2"), "{listed}");
    assert!(listed.contains("Before: bump the constant"), "{listed}");
    assert!(
        listed.contains("\"path\":\"app.js\",\"origin\":\"patch\""),
        "{listed}"
    );

    let rewind = |extra: &str| {
        let body = format!(
            "repo={}&checkpoint_id={}&files=true&conversation=false{extra}",
            percent_encode_for_test(&repo_arg),
            manifest.checkpoint_id
        );
        send(format!(
            "POST /api/rewind HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ))
    };

    // One file: the fourth restore operation, over the wire.
    let single = rewind("&path=app.js");
    assert!(
        single.contains("\"restoredFiles\":[\"app.js\"]"),
        "{single}"
    );
    assert_eq!(
        fs::read_to_string(repo.join("app.js")).unwrap(),
        "export const a = 1;\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("other.js")).unwrap(),
        "export const b = 2;\n"
    );

    let both = rewind("");
    assert!(both.contains("\"other.js\""), "{both}");
    assert!(both.contains("\"conversationRestored\":false"), "{both}");
    assert_eq!(
        fs::read_to_string(repo.join("other.js")).unwrap(),
        "export const b = 1;\n"
    );

    let _ = fs::remove_dir_all(&repo);
}

/// Serves the real UI with a known API token, seeded with a session whose
/// turn can be rewound and with three tasks left mid-flight, so the
/// checkpoint controls and both shapes of the crash recovery card can
/// actually be looked at. The web UI takes its token from the Tauri
/// bootstrap, so a browser cannot otherwise authenticate — and a full Tauri
/// build is minutes.
///
/// `#[ignore]`d: it binds a port and serves until it is stopped. Run it by
/// hand and open the URL it prints:
///
/// ```sh
/// cargo test -p desktop-shell --lib -- --ignored --nocapture serves_the_ui
/// ```
///
/// It binds port 4899; set `DAMAIAN_UI_INSPECTION_PORT` to use another
/// when 4899 is taken by another checkout's server.
///
/// Then, in the browser console (the token gates every `/api/` route):
///
/// ```js
/// apiToken = "damaian-ui-inspection-token";
/// setRepository("<the repository path it printed>", false);
/// ```
#[test]
#[ignore]
fn serves_the_ui_for_manual_inspection() {
    let data_dir = isolated_data_dir().to_path_buf();
    let repo = std::env::temp_dir().join(format!(
        "damaian-ui-inspection-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("upload.rs"), "fn upload() {\n    todo!()\n}\n").unwrap();
    fs::write(repo.join("retry.rs"), "// no retry yet\n").unwrap();

    let repo_arg = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_arg).expect("engine");
    let repository_id = engine
        .indexer
        .repository_id_for_path(&repo_arg)
        .expect("repository id");
    let session = engine
        .session_store
        .create_session(&repository_id, "Add retry to the upload client")
        .unwrap();
    let task = engine
        .session_store
        .create_task(
            &session.id,
            "add retry to the upload client",
            "mock",
            "mock",
        )
        .unwrap();
    let user_message = engine
        .session_store
        .append_message(
            &session.id,
            Some(&task.id),
            "user",
            "add retry to the upload client",
        )
        .unwrap();
    engine
        .session_store
        .append_message(
            &session.id,
            Some(&task.id),
            "assistant",
            "Added a retry helper and wired it into `upload`.",
        )
        .unwrap();
    let manifest = engine
        .checkpoint_store
        .create_checkpoint(
            &repo,
            workspace_engine::CheckpointRequest {
                session_id: &session.id,
                task_id: Some(&task.id),
                user_message_id: Some(&user_message.id),
                summary: "Before: add retry to the upload client",
                conversation: workspace_engine::CheckpointConversation {
                    last_event_seq: 1,
                    task_status: "complete".to_string(),
                },
                pending_approvals: Vec::new(),
                paths: vec![
                    workspace_engine::CheckpointPath {
                        path: "upload.rs".to_string(),
                        origin: workspace_engine::CheckpointOrigin::Patch,
                    },
                    workspace_engine::CheckpointPath {
                        path: "retry.rs".to_string(),
                        origin: workspace_engine::CheckpointOrigin::Command,
                    },
                ],
            },
        )
        .unwrap();
    fs::write(
        repo.join("upload.rs"),
        "fn upload() {\n    with_retry(send)\n}\n",
    )
    .unwrap();
    fs::write(repo.join("retry.rs"), "pub fn with_retry() {}\n").unwrap();
    engine
        .checkpoint_store
        .seal_checkpoint(&repo, &manifest)
        .unwrap();

    // Both shapes of the crash recovery card
    // (`docs/specs/45_crash_recovery_prompt.md` §5.6) plus a reattached
    // approval, so the surface can be driven without staging a real crash.
    // The turn above stays non-terminal on purpose: its card is the one
    // with a checkpoint for `Inspect` to link to.
    let unknown = engine
        .session_store
        .create_task(&session.id, "run the database migration", "mock", "mock")
        .unwrap();
    let unknown = engine
        .session_store
        .update_task_status(&unknown, workspace_engine::TaskStatus::RunningTool, None)
        .unwrap();
    let _unknown_marker = engine
        .session_store
        .start_action(&unknown, "run_command", "psql -f migrate.sql", true)
        .unwrap();
    let interrupted = engine
        .session_store
        .create_task(&session.id, "explain the retry helper", "mock", "mock")
        .unwrap();
    // What spec 19's sweep records for a model call cut off by the crash,
    // so the card's spend line can be looked at (spec 45 §5.9).
    engine
        .session_store
        .record_task_usage_for_task_id(
            &session.id,
            &interrupted.id,
            None,
            "lost_marker_inspection",
            Some("marker_inspection"),
            workspace_engine::TokenUsage::estimated(4820, 0),
            None,
            Some("lost_to_crash"),
        )
        .unwrap();
    let interrupted = engine
        .session_store
        .update_task_status(
            &interrupted,
            workspace_engine::TaskStatus::PreparingContext,
            None,
        )
        .unwrap();
    let _interrupted_marker = engine
        .session_store
        .start_action(&interrupted, "read_file", "retry.rs", false)
        .unwrap();
    let awaiting = engine
        .session_store
        .create_task(&session.id, "list the crate", "mock", "mock")
        .unwrap();
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "ls -la", "Desktop command proposal")
        .unwrap();
    engine
        .session_store
        .await_approval(
            &awaiting,
            &workspace_engine::PendingApprovalRef {
                kind: "command".to_string(),
                proposal_id: proposal.id.clone(),
            },
        )
        .unwrap();

    // A plan review caught mid-crash: the plan in the log, its paused turn
    // on disk where the orchestrator parks one.
    let reviewing = engine
        .session_store
        .create_task(
            &session.id,
            "add retry to the upload client",
            "mock",
            "mock",
        )
        .unwrap();
    let mut plan = workspace_engine::TaskPlan::new(&reviewing.id, 1);
    for (id, title) in [
        ("step_1", "Read the upload client"),
        ("step_2", "Add a retry helper"),
        ("step_3", "Wire it into upload()"),
    ] {
        plan.steps.push(workspace_engine::PlanStep {
            id: id.to_string(),
            title: title.to_string(),
            detail: None,
            status: workspace_engine::StepStatus::Pending,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
            build: None,
        });
    }
    engine.session_store.create_plan(&reviewing, &plan).unwrap();
    let pending_dir = data_dir.join("chat").join("pending");
    fs::create_dir_all(&pending_dir).unwrap();
    fs::write(
        pending_dir.join("planprop_inspection.json"),
        r#"{"proposal_id":"planprop_inspection","plan_review":{"deferred_action":"apply a patch to upload.rs"}}"#,
    )
    .unwrap();
    engine
        .session_store
        .await_approval(
            &reviewing,
            &workspace_engine::PendingApprovalRef {
                kind: "plan".to_string(),
                proposal_id: "planprop_inspection".to_string(),
            },
        )
        .unwrap();

    // A second session left in Ask mode, holding a turn the mode refused
    // (spec 20 §5.6), so the mode control and a refusal can be looked at.
    let (refused_session, _) = refused_turn_in_ask_mode(&repo);

    let (diagnosed_session, expected_card_header) =
        recorded_web_diagnostic_session(&engine, &repository_id, &data_dir);
    let legacy_session = legacy_web_diagnostic_session(&engine, &repository_id, &data_dir);
    let findings_session = findings_to_review_session(&engine, &repository_id, &repo);
    let activity_session = activity_and_plan_session(&engine, &repository_id);
    let command_activity_session = command_activity_session(&engine, &repository_id, &repo_arg);
    let question_session = pending_question_session(&engine, &repository_id);

    // 4899 unless overridden, so two checkouts can be inspected at once
    // without stopping each other's server.
    let port = std::env::var("DAMAIAN_UI_INSPECTION_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4899);
    let options = ShellOptions {
        port,
        default_repo: Some(repo_arg.clone()),
        api_token: "damaian-ui-inspection-token".to_string(),
    };
    println!("UI inspection server: http://127.0.0.1:{}/", options.port);
    println!("  apiToken = \"{}\"", options.api_token);
    println!("  repository = {repo_arg}");
    println!("  session = {}", session.id);
    println!("  refused-in-Ask session = {refused_session}");
    println!("  recorded web diagnostic session = {diagnosed_session}");
    println!("    card header should read: {expected_card_header}");
    println!("  legacy web diagnostic session (regex thumbnails) = {legacy_session}");
    println!(
        "  findings session (the composer's repo bar should offer \"Fix 2 errors\") = {findings_session}"
    );
    println!(
        "  activity + plan session (collapsed Activity and Steps disclosures) = {activity_session}"
    );
    println!(
        "  command activity session (spec 68: command steps, approval requests, one usage row) = {command_activity_session}"
    );
    println!(
        "  pending question session (spec 107: the card last, the composer disabled) = {question_session}"
    );
    println!("  data_dir = {}", data_dir.display());
    run_server(options).expect("serve the UI");
}

/// A finished turn made of command steps in every stored shape spec 68
/// renders: ran, repeated, approval request, declined, timed out with a
/// hook note, a command containing a backtick and an em dash, a mixed
/// STDERR body, and a tool result that does not parse. Returns the
/// session id.
fn command_activity_session(engine: &WorkspaceEngine, repository_id: &str, root: &str) -> String {
    let store = &engine.session_store;
    let session = store
        .create_session(repository_id, "Command activity")
        .unwrap();
    let task = store
        .create_task(
            &session.id,
            "list the build output and check the crate",
            "mock",
            "mock",
        )
        .unwrap();
    let cause = workspace_engine::ApprovalCause::ShellControl {
        constructs: "`|`".into(),
    }
    .plain_words();
    let check = format!(
        "Command: cargo check 2>&1 | tail -5\nWorking directory: {root}\nExit code: 0\n\nSTDOUT:\nFinished dev\n\nSTDERR:\n"
    );
    let messages: Vec<(&str, String)> = vec![
        ("user", "List the build output and check the crate.".into()),
        ("assistant", "Ran `ls target` — See what the last build left.".into()),
        (
            "tool",
            format!(
                "Command: ls target\nWorking directory: {root}\nExit code: 0\n\nSTDOUT:\ndebug\n\nSTDERR:\n"
            ),
        ),
        ("assistant", format!("{cause}\n\n`cargo check 2>&1 | tail -5`")),
        ("assistant", "Ran `cargo check 2>&1 | tail -5` — Compile the crate.".into()),
        ("tool", check.clone()),
        ("assistant", "Ran `cargo check 2>&1 | tail -5` — Check again.".into()),
        ("tool", check),
        ("assistant", format!("{cause}\n\n`rm -rf target | cat`")),
        ("assistant", "Ran `rm -rf target | cat` — Clean the build.".into()),
        (
            "tool",
            "The user declined to run `rm -rf target | cat`. Do not request it again; answer using what you already know, noting the limitation if it matters.".into(),
        ),
        ("assistant", "Ran `sleep 999` — Wait for the server.".into()),
        (
            "tool",
            format!(
                "Command: sleep 999\nWorking directory: {root}/sub\nExit code: none (timed out)\n\nSTDOUT:\n\n\nSTDERR:\nslow\n\nHook `fmt-guard`: refused after the action."
            ),
        ),
        ("assistant", "Ran `echo \"a ` — b\"` — Quote test.".into()),
        (
            "tool",
            format!(
                "Command: echo \"a ` — b\"\nWorking directory: {root}\nExit code: 1\n\nSTDOUT:\nx\n\nSTDERR:\nHalf\n\nSTDERR:\nreal"
            ),
        ),
        ("assistant", "Read `src/lib.rs`.".into()),
        ("tool", "fn main() {}".into()),
        ("assistant", "The crate builds; the clean was declined.".into()),
    ];
    for (role, content) in &messages {
        store
            .append_message(&session.id, Some(&task.id), role, content)
            .unwrap();
    }
    store
        .record_task_usage_for_task_id(
            &session.id,
            &task.id,
            None,
            "run_command_activity",
            None,
            workspace_engine::TokenUsage::estimated(5200, 640),
            None,
            None,
        )
        .unwrap();
    store
        .update_task_status(&task, workspace_engine::TaskStatus::Complete, None)
        .unwrap();
    session.id
}

/// A finished turn with a plan and a run of intermediate action/result
/// pairs, so the collapsed Activity disclosure and the collapsed plan
/// steps (spec 67) can be inspected. Returns the session id.
fn activity_and_plan_session(engine: &WorkspaceEngine, repository_id: &str) -> String {
    let store = &engine.session_store;
    let session = store
        .create_session(repository_id, "Investigate the upload client")
        .unwrap();
    let task = store
        .create_task(&session.id, "investigate the upload client", "mock", "mock")
        .unwrap();
    store
        .append_message(
            &session.id,
            Some(&task.id),
            "user",
            "Investigate the upload client and report.",
        )
        .unwrap();
    for (action, result) in [
        ("Read `upload.rs`", "fn upload() {\n    todo!()\n}\n"),
        ("Searched for `retry`", "0 matches across 1 file\n"),
        (
            "Ran `cargo test`",
            "test result: FAILED. 0 passed; 1 failed; 0 ignored\n",
        ),
    ] {
        store
            .append_message(&session.id, Some(&task.id), "assistant", action)
            .unwrap();
        store
            .append_message(&session.id, Some(&task.id), "tool", result)
            .unwrap();
    }
    let plan = workspace_engine::TaskPlan {
        task_id: task.id.clone(),
        created_at_ms: 1,
        steps: vec![
            workspace_engine::PlanStep {
                id: "step_1".to_string(),
                title: "Read the upload client".to_string(),
                detail: None,
                status: workspace_engine::StepStatus::Completed,
                depends_on: Vec::new(),
                started_at_ms: Some(1),
                completed_at_ms: Some(2),
                evidence: vec![workspace_engine::Evidence::FileRead {
                    path: "upload.rs".to_string(),
                    hash: "sha256:0".to_string(),
                }],
                check: None,
                build: None,
            },
            workspace_engine::PlanStep {
                id: "step_2".to_string(),
                title: "Run the tests".to_string(),
                detail: None,
                status: workspace_engine::StepStatus::Blocked,
                depends_on: Vec::new(),
                started_at_ms: Some(2),
                completed_at_ms: Some(3),
                evidence: Vec::new(),
                check: None,
                build: None,
            },
            workspace_engine::PlanStep {
                id: "step_3".to_string(),
                title: "Report the finding".to_string(),
                detail: None,
                status: workspace_engine::StepStatus::Pending,
                depends_on: Vec::new(),
                started_at_ms: None,
                completed_at_ms: None,
                evidence: Vec::new(),
                check: None,
                build: None,
            },
        ],
    };
    store.create_plan(&task, &plan).unwrap();
    store
        .append_message(
            &session.id,
            Some(&task.id),
            "assistant",
            "## Finding\n\n`upload` never retries. The test suite fails before it can prove the fix.",
        )
        .unwrap();
    store
        .update_task_status(&task, workspace_engine::TaskStatus::Complete, None)
        .unwrap();
    session.id
}

/// A browser diagnostic turn as spec 12 Task 3 records it: the tool
/// message listing the materialised screenshot, plus the redacted record
/// the card renders. Returns the session id and `render_for_model`'s header
/// line, which the card must reproduce word for word.
fn recorded_web_diagnostic_session(
    engine: &WorkspaceEngine,
    repository_id: &str,
    data_dir: &std::path::Path,
) -> (String, String) {
    let store = &engine.session_store;
    let session = store
        .create_session(repository_id, "Why does the snake game not start?")
        .unwrap();
    let task = store
        .create_task(
            &session.id,
            "why does the snake game not start?",
            "mock",
            "mock",
        )
        .unwrap();
    let relative = format!("web-diagnostics/{}/{}/run-1/page.png", session.id, task.id);
    write_inspection_png(&data_dir.join(&relative), [163, 55, 55]);

    let warnings: Vec<String> = (1..=6)
        .map(|n| format!(r#"{{"type": "warning", "text": "Deprecated API call {n}"}}"#))
        .collect();
    let companion = format!(
        r#"{{"url": "http://localhost:5001/", "final_url": "http://localhost:5001/play",
                "title": "Snake Game", "status": 200,
                "page_errors": ["ReferenceError: Cannot access 'game' before initialization"],
                "console": [
                  {{"type": "log", "text": "booting"}},
                  {{"type": "error", "text": "Failed to load resource: 404",
                    "location": {{"url": "http://localhost:5001/js/app.js", "lineNumber": 41, "columnNumber": 7}}}},
                  {}
                ],
                "failed_requests": [
                  {{"url": "http://localhost:5001/api/me", "method": "GET", "status": 404,
                    "status_text": "Not Found"}},
                  {{"url": "http://localhost:5001/ws", "method": "GET",
                    "failure": "net::ERR_CONNECTION_REFUSED"}}
                ],
                "results": [
                  {{"step": 0, "action": "goto", "success": true}},
                  {{"step": 1, "action": "click", "success": false,
                    "error": "locator('#start') is not visible"}}
                ],
                "dom_summary": {{"forms": 1, "buttons": ["Start", "Log in"],
                  "status_text": "Loading…", "visible_text_excerpt": "Snake\nScore: 0\nStart"}}}}"#,
        warnings.join(",\n")
    );
    let mut report = WebDiagnosticReport::from_text(companion, false);
    report.artifacts = vec![workspace_engine::WebDiagnosticArtifact {
        kind: "screenshot".to_string(),
        path: relative.clone(),
        mime_type: Some("image/png".to_string()),
        width: Some(160),
        height: Some(90),
    }];
    report.via = Some("MCP server `browser` tool `run_scenario`".to_string());
    let header = report
        .render_for_model()
        .and_then(|text| text.lines().next().map(str::to_string))
        .expect("a structured report renders");

    store
        .append_message(
            &session.id,
            Some(&task.id),
            "user",
            "why does the snake game not start?",
        )
        .unwrap();
    store
        .append_message(
            &session.id,
            Some(&task.id),
            "assistant",
            "Running a browser scenario against the game.",
        )
        .unwrap();
    store
        .append_message(
            &session.id,
            Some(&task.id),
            "tool",
            &format!("{header}\n\nArtifacts:\n- screenshot: {relative} (160x90)"),
        )
        .unwrap();
    store
        .append_web_diagnostic(
            &session.id,
            &WebDiagnosticRecord {
                id: "webdiagrec_inspection".to_string(),
                task_id: task.id.clone(),
                tool: "run_web_scenario".to_string(),
                url: "http://localhost:5001/".to_string(),
                recorded_at_ms: 1_759_000_000_000,
                report: report.redacted(&engine.scanner),
            },
        )
        .unwrap();
    store
        .append_message(
            &session.id,
            Some(&task.id),
            "assistant",
            "`game` is read before it is declared in `app.js`, so the start button never renders.",
        )
        .unwrap();
    // Finished, so the crash recovery sweep leaves the turn alone.
    store
        .update_task_status(&task, workspace_engine::TaskStatus::Complete, None)
        .unwrap();
    (session.id, header)
}

/// The findings panel's fixture (spec 22 Task 11): two open errors, one
/// ranged and one not, an open warning, a stale lint finding and a
/// dismissed test failure, across four sources. Returns the session id.
fn findings_to_review_session(
    engine: &WorkspaceEngine,
    repository_id: &str,
    repo: &std::path::Path,
) -> String {
    let store = &engine.session_store;
    let session = store
        .create_session(repository_id, "Findings to review")
        .unwrap();
    let task = store
        .create_task(&session.id, "check the upload client", "mock", "mock")
        .unwrap();
    store
        .append_message(
            &session.id,
            Some(&task.id),
            "user",
            "check the upload client",
        )
        .unwrap();
    store
        .append_message(
            &session.id,
            Some(&task.id),
            "assistant",
            "`cargo check` and `npm test` both failed; the findings panel lists what they reported.",
        )
        .unwrap();

    let range = |path: &str, line: u32, column: u32| SourceRange {
        path: path.to_string(),
        start_line: line,
        start_column: Some(column),
        end_line: None,
        end_column: None,
    };
    let hashed = |path: &str| workspace_engine::hash::file_hash(repo.join(path)).unwrap();
    let record = |draft: FindingDraft, file_hash: Option<String>| {
        let mut finding = Finding::new(draft, &engine.scanner).with_task_id(&task.id);
        if let Some(file_hash) = file_hash {
            finding = finding.with_file_hash(file_hash);
        }
        store.record_finding(&session.id, &finding).unwrap();
        finding.id().to_string()
    };

    record(
        FindingDraft {
            source: FindingSource::Compiler,
            severity: Severity::Error,
            summary: "mismatched types".to_string(),
            details: Some(
                "error[E0308]: mismatched types\n --> upload.rs:3:5\n  |\n3 |     with_retry(send)\n  |     ^^^^^^^^^^^^^^^^ expected `Result<(), Error>`, found `()`"
                    .to_string(),
            ),
            range: Some(range("upload.rs", 3, 5)),
            code: Some("E0308".to_string()),
        },
        Some(hashed("upload.rs")),
    );
    record(
        FindingDraft {
            source: FindingSource::Command,
            severity: Severity::Error,
            summary: "npm test: 1 failing".to_string(),
            details: None,
            range: None,
            code: None,
        },
        None,
    );
    record(
        FindingDraft {
            source: FindingSource::Compiler,
            severity: Severity::Warning,
            summary: "unused variable: `retries`".to_string(),
            details: None,
            range: Some(range("upload.rs", 7, 9)),
            code: Some("unused_variables".to_string()),
        },
        Some(hashed("upload.rs")),
    );
    // A hash no file has, so the finding derives stale on every read.
    record(
        FindingDraft {
            source: FindingSource::Lint,
            severity: Severity::Warning,
            summary: "unneeded `return` statement".to_string(),
            details: None,
            range: Some(range("retry.rs", 2, 5)),
            code: Some("clippy::needless_return".to_string()),
        },
        Some("sha256:0".to_string()),
    );
    let dismissed = record(
        FindingDraft {
            source: FindingSource::Test,
            severity: Severity::Error,
            summary: "tests::retries_three_times failed".to_string(),
            details: None,
            range: Some(range("retry.rs", 10, 1)),
            code: None,
        },
        Some(hashed("retry.rs")),
    );
    store
        .set_finding_status(
            &session.id,
            &dismissed,
            workspace_engine::finding::FindingStatus::Dismissed,
        )
        .unwrap();

    // Finished, so the crash recovery sweep leaves the turn alone.
    store
        .update_task_status(&task, workspace_engine::TaskStatus::Complete, None)
        .unwrap();
    session.id
}

/// A session from before spec 12 Task 3: no recorded diagnostic, only a
/// tool message whose text names the screenshot, which the legacy regex
/// thumbnails must keep showing.
fn legacy_web_diagnostic_session(
    engine: &WorkspaceEngine,
    repository_id: &str,
    data_dir: &std::path::Path,
) -> String {
    let store = &engine.session_store;
    let session = store
        .create_session(
            repository_id,
            "Check the landing page (before recorded diagnostics)",
        )
        .unwrap();
    let task = store
        .create_task(&session.id, "check the landing page", "mock", "mock")
        .unwrap();
    let relative = format!("web-diagnostics/{}/{}/run-1/page.png", session.id, task.id);
    write_inspection_png(&data_dir.join(&relative), [161, 92, 24]);
    for (role, content) in [
        ("user", "check the landing page".to_string()),
        ("assistant", "Inspecting the page.".to_string()),
        (
            "tool",
            format!(
                "Browser diagnostic result via MCP server `browser` tool `inspect_page`:\nNo problems found.\n\nArtifacts:\n- screenshot: {relative} (160x90)"
            ),
        ),
        ("assistant", "The landing page loads cleanly.".to_string()),
    ] {
        store
            .append_message(&session.id, Some(&task.id), role, &content)
            .unwrap();
    }
    store
        .update_task_status(&task, workspace_engine::TaskStatus::Complete, None)
        .unwrap();
    session.id
}

/// Writes a real 160×90 PNG, a stand-in page with an `accent`-coloured
/// banner, so the thumbnails have something to show. Stored (uncompressed)
/// deflate keeps it free of an image dependency.
fn write_inspection_png(path: &std::path::Path, accent: [u8; 3]) {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 90;
    let mut pixels = Vec::new();
    for y in 0..HEIGHT {
        pixels.push(0); // filter type: none
        for x in 0..WIDTH {
            let rgb = if y < 14 {
                [23, 107, 93]
            } else if (26..40).contains(&y) && (12..148).contains(&x) {
                accent
            } else {
                [255, 255, 255]
            };
            pixels.extend_from_slice(&rgb);
        }
    }
    // zlib header, one stored block per 65535 bytes, then Adler-32.
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = pixels.chunks(65_535).collect();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &pixels {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&WIDTH.to_be_bytes());
    ihdr.extend_from_slice(&HEIGHT.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(&b"IHDR"[..], ihdr), (b"IDAT", zlib), (b"IEND", Vec::new())] {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = png.len();
        png.extend_from_slice(kind);
        png.extend_from_slice(&data);
        let crc = png_crc(&png[start..]);
        png.extend_from_slice(&crc.to_be_bytes());
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, png).unwrap();
}

fn png_crc(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn percent_encode_for_test(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => {
                character.to_string()
            }
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

#[test]
fn render_markdown_endpoint_returns_syntax_highlighted_html() {
    let options = ShellOptions::new(0, None);
    let token = options.api_token.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let port = listener.local_addr().expect("local addr").port();
    spawn_with_shell_stack("test-shell", move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let _ = handle_connection(&mut stream, &options);
        }
    })
    .expect("spawn the test shell");

    let body = "content=%23%20Title%0A%0A%60%60%60rust%0Afn%20main()%20%7B%7D%0A%60%60%60";
    let request = format!(
        "POST /api/render-markdown HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");

    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected response: {response}"
    );
    assert!(response.contains("<h1>Title</h1>"));
    assert!(response.contains("hl-"));
    assert!(!response.contains("<script>"));
}

/// Serves `handle_connection` on an ephemeral port and answers a failed
/// route the way `run_server` does, so a test can see an error response
/// rather than a closed socket. Returns the port and the API token.
fn serve_for_test() -> (u16, String) {
    let options = ShellOptions::new(0, None);
    let token = options.api_token.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let port = listener.local_addr().expect("local addr").port();
    spawn_with_shell_stack("test-shell", move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            if let Err(error) = handle_connection(&mut stream, &options) {
                let _ =
                    write_basic_response(&mut stream, 500, "application/json", &json_error(&error));
            }
        }
    })
    .expect("spawn the test shell");
    (port, token)
}

/// Spec 108 D1: the manual Protected paths control is gone from the main
/// window. The session payload no longer carries it, its routes are not
/// served, and the page has no panel or handlers — while a task's durable
/// protection (spec 98) is untouched and still read by the engine.
#[test]
fn the_protected_paths_control_is_gone_from_the_session_payload_and_page() {
    let repo = temp_shell_repo("no-protected-control");
    fs::write(repo.join("README.md"), "readme\n").unwrap();
    let repo_arg = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_arg).unwrap();
    let session = engine
        .session_store
        .create_session(
            &workspace_engine::repository_id_for_root(&repo),
            "protected paths",
        )
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "Do the work.", "provider", "model")
        .unwrap();
    engine
        .add_task_protected_paths(&repo, &session.id, &task.id, &["README.md".to_string()])
        .unwrap();
    let (port, token) = serve_for_test();

    let response = send_for_test(
        port,
        format!(
            "GET /api/session?repo={}&session_id={} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n",
            percent_encode_for_test(&repo_arg),
            percent_encode_for_test(&session.id)
        ),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let payload = response_json(&response);
    assert_eq!(payload["session"]["id"], session.id);
    assert!(
        payload.get("protectedPathsControl").is_none(),
        "the session payload must not carry the removed control: {payload}"
    );

    for path in ["/api/protected-paths-add", "/api/protected-paths-remove"] {
        let refused = post_fields_for_test(
            port,
            &token,
            path,
            &[
                ("repo", &repo_arg),
                ("session_id", &session.id),
                ("task_id", &task.id),
                ("paths", "README.md"),
            ],
        );
        assert!(refused.starts_with("HTTP/1.1 404"), "{path}: {refused}");
    }

    // Enforcement state is the engine's and is unchanged.
    let active = engine_for_repo(&repo_arg)
        .unwrap()
        .task_protected_paths(&session.id, &task.id)
        .unwrap();
    assert_eq!(active.path_list(), vec!["README.md".to_string()]);

    let html = include_str!("../static/index.html");
    assert!(
        !html.contains("protected-paths"),
        "index.html keeps the panel"
    );
    assert!(
        !html.contains("Protect paths"),
        "index.html keeps the button"
    );
    let css = include_str!("../static/style.css");
    assert!(
        !css.contains(".protected-paths-panel"),
        "style.css keeps the panel"
    );
    let app = include_str!("../static/app.js");
    for removed in [
        "/api/protected-paths",
        "renderProtectedPathsControl",
        "addProtectedPaths",
        "removeProtectedPath",
        "protected-paths-",
    ] {
        assert!(!app.contains(removed), "app.js keeps {removed}");
    }

    fs::remove_dir_all(&repo).ok();
}

/// The provider editor's placeholders show these numbers, so the endpoint
/// must answer per model with what a blank field resolves to, including
/// "no ceiling" for the output of a provider that sets none.
#[test]
fn provider_token_defaults_answers_per_model() {
    let (port, token) = serve_for_test();
    let get = |query: &str| {
        response_json(&send_for_test(
            port,
            format!(
                "GET /api/provider-token-defaults?{query} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n"
            ),
        ))
    };

    let deepseek = get("provider=deepseek&models=deepseek-v4-flash,deepseek-chat");
    assert_eq!(deepseek["models"][0]["model"], "deepseek-v4-flash");
    assert_eq!(deepseek["models"][0]["maxOutputTokens"], 65_536);
    assert_eq!(deepseek["models"][0]["contextTokenBudget"], 64_000);
    assert_eq!(deepseek["models"][1]["maxOutputTokens"], 8_192);
    assert_eq!(deepseek["models"][1]["contextTokenBudget"], 16_000);
    // Spec 79: the window is its own field, known only for V4.
    assert_eq!(deepseek["models"][0]["contextWindow"], 1_000_000);
    assert!(deepseek["models"][1]["contextWindow"].is_null());

    let unnamed = get("provider=&models=");
    assert_eq!(unnamed["models"][0]["model"], "");
    assert!(unnamed["models"][0]["maxOutputTokens"].is_null());
    assert_eq!(
        unnamed["models"][0]["contextTokenBudget"],
        workspace_engine::DEFAULT_CONTEXT_TOKEN_BUDGET
    );
}

fn send_for_test(port: u16, request: String) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    response
}

fn form_body_for_test(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{key}={}", percent_encode_for_test(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn response_json(response: &str) -> serde_json::Value {
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(response);
    serde_json::from_str(body).unwrap_or_else(|error| panic!("{error}: {body}"))
}

fn post_fields_for_test(port: u16, token: &str, path: &str, fields: &[(&str, &str)]) -> String {
    let body = form_body_for_test(fields);
    send_for_test(
        port,
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

/// A monorepo with two npm packages and a directory that only becomes a
/// root when the user adds it (no manifest of its own).
fn repository_map_fixture(name: &str) -> PathBuf {
    let repo = temp_path(name);
    for directory in ["packages/api/src", "packages/web/src", "tools/scripts"] {
        fs::create_dir_all(repo.join(directory)).unwrap();
    }
    let scripts = "{\"scripts\":{\"test\":\"node test.js\"}}";
    fs::write(repo.join("packages/api/package.json"), scripts).unwrap();
    fs::write(repo.join("packages/web/package.json"), scripts).unwrap();
    fs::write(repo.join("packages/api/src/index.ts"), "export {};\n").unwrap();
    fs::write(repo.join("packages/web/src/index.ts"), "export {};\n").unwrap();
    fs::write(repo.join("tools/scripts/run.ts"), "export {};\n").unwrap();
    repo
}

/// A Rust declaration at the repository root and one under an npm root, so
/// the symbol endpoints have both a provenance and a `rootPath` to report.
fn symbol_fixture(name: &str) -> PathBuf {
    isolated_data_dir();
    let repo = repository_map_fixture(name);
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/upload.rs"), "pub fn upload_retry() {}\n").unwrap();
    fs::write(
        repo.join("packages/api/src/retry.ts"),
        "export function uploadRetry() {}\n",
    )
    .unwrap();
    repo
}

fn find_root<'a>(payload: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    payload["map"]["roots"]
        .as_array()?
        .iter()
        .find(|root| root["path"] == path)
}

#[test]
fn repository_map_endpoint_serves_the_map_and_its_load_shape() {
    let repo = repository_map_fixture("repomap-shape");
    let (port, token) = serve_for_test();
    let url = format!(
        "/api/repository-map?repo={}",
        percent_encode_for_test(&repo.to_string_lossy())
    );
    let response = get_for_test(port, &token, &url);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    let payload = json_of(&response);
    assert_eq!(payload["load"]["outcome"], "built", "{payload}");
    assert_eq!(payload["map"]["schemaVersion"], 1, "{payload}");

    let api = find_root(&payload, "packages/api")
        .unwrap_or_else(|| panic!("packages/api root missing: {payload}"));
    assert_eq!(api["detectedBy"]["kind"], "manifest");
    assert_eq!(api["detectedBy"]["path"], "packages/api/package.json");
    let commands = api["commands"].as_array().expect("commands array");
    assert!(
        commands
            .iter()
            .any(|command| command["command"] == "npm test"),
        "{api}"
    );
    assert!(
        commands
            .iter()
            .all(|command| command["workingDirectory"] == "packages/api"),
        "{api}"
    );
}

#[test]
fn repository_roots_endpoint_writes_and_re_reads_an_added_root() {
    let repo = repository_map_fixture("repomap-add");
    let repo_str = repo.to_string_lossy().to_string();
    let (port, token) = serve_for_test();

    let response = post_fields_for_test(
        port,
        &token,
        "/api/repository-roots",
        &[
            ("repo", &repo_str),
            ("action", "add"),
            ("path", "tools/scripts"),
        ],
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let payload = json_of(&response);
    assert_eq!(
        payload["configPath"],
        repo.join(".damaian/config.conf").to_string_lossy().as_ref(),
        "{payload}"
    );

    let written = fs::read_to_string(repo.join(".damaian/config.conf")).unwrap();
    assert!(
        written.contains("project_roots_added=tools/scripts"),
        "{written}"
    );

    let added = find_root(&payload, "tools/scripts")
        .unwrap_or_else(|| panic!("added root missing: {payload}"));
    assert_eq!(added["detectedBy"]["kind"], "userOverride");
    assert_eq!(added["userOverride"], "added");

    let url = format!(
        "/api/repository-map?repo={}",
        percent_encode_for_test(&repo_str)
    );
    let reread = json_of(&get_for_test(port, &token, &url));
    assert!(
        find_root(&reread, "tools/scripts").is_some_and(|root| root["userOverride"] == "added"),
        "{reread}"
    );
}

#[test]
fn repository_roots_endpoint_reports_an_invalid_path_as_an_invalid_override() {
    let repo = repository_map_fixture("repomap-invalid");
    let repo_str = repo.to_string_lossy().to_string();
    let (port, token) = serve_for_test();

    let response = post_fields_for_test(
        port,
        &token,
        "/api/repository-roots",
        &[
            ("repo", &repo_str),
            ("action", "add"),
            ("path", "../outside"),
        ],
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let payload = json_of(&response);

    let excluded = payload["map"]["excluded"]
        .as_array()
        .expect("excluded array");
    assert!(
        excluded
            .iter()
            .any(|entry| entry["path"] == "../outside" && entry["reason"] == "invalidOverride"),
        "{payload}"
    );
    assert!(
        find_root(&payload, "../outside").is_none(),
        "an invalid override must not become a root: {payload}"
    );
}

#[test]
fn repository_roots_endpoint_refuses_a_foreign_repository() {
    let (port, token) = serve_for_test();
    let not_a_repository = temp_path("repomap-foreign");
    fs::write(&not_a_repository, "not a directory").unwrap();
    let path = not_a_repository.to_string_lossy().to_string();

    let response = post_fields_for_test(
        port,
        &token,
        "/api/repository-roots",
        &[
            ("repo", &path),
            ("action", "add"),
            ("path", "tools/scripts"),
        ],
    );
    assert!(response.starts_with("HTTP/1.1 500"), "{response}");
}

#[test]
fn a_root_override_is_never_written_outside_the_allowed_roots() {
    let allowed = temp_path("repomap-allowed");
    let inside = allowed.join("repo");
    let outside = temp_path("repomap-outside");
    fs::create_dir_all(&inside).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let engine = WorkspaceEngine::new(Config {
        data_dir: allowed.join(".damaian-data"),
        allowed_roots: vec![allowed.clone()],
        enable_index_watcher: false,
        ..Config::default()
    });

    let inside_str = inside.to_string_lossy().to_string();
    assert_eq!(
        repository_config_write_target(&engine, &inside_str).unwrap(),
        Config::repository_config_path(&inside_str)
    );
    let refused = repository_config_write_target(&engine, &outside.to_string_lossy());
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.contains("not allowed")),
        "{refused:?}"
    );
    assert!(!outside.join(".damaian").exists());
}

#[test]
fn symbols_endpoint_serves_matches_with_provenance_and_their_root() {
    let repo = symbol_fixture("symbols-shape");
    let (port, token) = serve_for_test();
    let url = format!(
        "/api/symbols?repo={}&q=upload_retry",
        percent_encode_for_test(&repo.to_string_lossy())
    );
    let response = get_for_test(port, &token, &url);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    let payload = json_of(&response);
    let matches = payload["matches"].as_array().expect("matches");
    assert_eq!(matches.len(), 1, "{payload}");
    assert_eq!(matches[0]["quality"], "exact");
    let symbol = &matches[0]["symbol"];
    assert_eq!(symbol["name"], "upload_retry");
    assert_eq!(symbol["kind"], "function");
    assert_eq!(symbol["path"], "src/upload.rs");
    assert_eq!(symbol["line"], 1);
    assert_eq!(symbol["rootPath"], "");
    assert_eq!(symbol["provenance"], "heuristic");

    // The root is the payload's, not something the UI infers.
    let rooted = format!(
        "/api/symbols?repo={}&q=uploadRetry",
        percent_encode_for_test(&repo.to_string_lossy())
    );
    let response = get_for_test(port, &token, &rooted);
    let payload = json_of(&response);
    assert_eq!(
        payload["matches"][0]["symbol"]["rootPath"], "packages/api",
        "{payload}"
    );
}

#[test]
fn symbol_references_endpoint_answers_without_a_language_server() {
    let repo = symbol_fixture("symbols-references");
    let (port, token) = serve_for_test();
    let url = format!(
        "/api/symbol-references?repo={}&path=src%2Fupload.rs&line=1&name=upload_retry",
        percent_encode_for_test(&repo.to_string_lossy())
    );
    let response = get_for_test(port, &token, &url);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    let payload = json_of(&response);
    assert_eq!(payload["kind"], "references");
    assert_eq!(payload["symbol"]["path"], "src/upload.rs");
    assert_eq!(payload["symbol"]["line"], 1);
    assert_eq!(payload["symbol"]["name"], "upload_retry");
    assert_eq!(payload["language"], "rust");
    assert_eq!(payload["available"], false);
    assert!(
        payload["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no language server for rust")),
        "{payload}"
    );
    assert_eq!(payload["relations"], serde_json::json!([]));
}

#[test]
fn symbol_endpoints_refuse_a_repository_that_is_not_a_directory() {
    let (port, token) = serve_for_test();
    let not_a_repository = temp_path("symbols-foreign");
    fs::write(&not_a_repository, "not a directory").unwrap();
    let repo = percent_encode_for_test(&not_a_repository.to_string_lossy());

    for path in [
        format!("/api/symbols?repo={repo}&q=upload_retry"),
        format!("/api/symbol-references?repo={repo}&path=x.rs&line=1&name=x"),
        format!("/api/language-servers?repo={repo}"),
    ] {
        let response = get_for_test(port, &token, &path);
        assert!(response.starts_with("HTTP/1.1 500"), "{path}: {response}");
    }
    fs::remove_file(not_a_repository).unwrap();
}

#[test]
fn symbol_repository_stays_inside_the_allowed_roots() {
    let allowed = temp_path("symbols-allowed");
    let inside = allowed.join("repo");
    let outside = temp_path("symbols-outside");
    fs::create_dir_all(&inside).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let engine = WorkspaceEngine::new(Config {
        data_dir: allowed.join(".damaian-data"),
        allowed_roots: vec![allowed.clone()],
        enable_index_watcher: false,
        ..Config::default()
    });

    assert_eq!(
        symbol_repository(&engine, &inside.to_string_lossy()).unwrap(),
        fs::canonicalize(&inside).unwrap()
    );
    let refused = symbol_repository(&engine, &outside.to_string_lossy());
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.contains("not allowed")),
        "{refused:?}"
    );
    fs::remove_dir_all(allowed).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn language_servers_endpoint_serves_the_hub_shape() {
    let repo = symbol_fixture("symbols-servers");
    let (port, token) = serve_for_test();
    let url = format!(
        "/api/language-servers?repo={}",
        percent_encode_for_test(&repo.to_string_lossy())
    );
    let response = get_for_test(port, &token, &url);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    // Nothing is configured in this test's user config, so the list is
    // empty rather than absent.
    assert_eq!(json_of(&response)["servers"], serde_json::json!([]));
}

#[test]
fn language_servers_json_lists_configured_servers_before_they_start() {
    let mut configured = BTreeMap::new();
    configured.insert("rust".to_string(), "rust-analyzer".to_string());
    configured.insert(
        "python".to_string(),
        "pyright-langserver --stdio".to_string(),
    );
    let statuses = vec![
        LanguageServerStatus {
            language: "rust".to_string(),
            command: "rust-analyzer".to_string(),
            state: LanguageServerState::Crashed { restarts: 1 },
            pid: None,
        },
        LanguageServerStatus {
            language: "python".to_string(),
            command: "pyright-langserver --stdio".to_string(),
            state: LanguageServerState::Unavailable {
                reason: "spawn failed: No such file or directory".to_string(),
            },
            pid: None,
        },
    ];

    let payload: serde_json::Value =
        serde_json::from_str(&language_servers_json(&configured, statuses)).unwrap();
    let servers = payload["servers"].as_array().expect("servers");
    assert_eq!(servers.len(), 2, "{payload}");
    assert_eq!(servers[0]["language"], "python");
    assert_eq!(servers[0]["state"], "unavailable");
    assert_eq!(
        servers[0]["reason"],
        "spawn failed: No such file or directory"
    );
    assert_eq!(servers[1]["language"], "rust");
    assert_eq!(servers[1]["state"], "crashed");
    assert_eq!(servers[1]["restarts"], 1);
    assert!(servers[1]["reason"].is_null(), "{payload}");
}

#[test]
fn a_configured_language_server_is_idle_until_a_lookup_starts_it() {
    let mut configured = BTreeMap::new();
    configured.insert("go".to_string(), "gopls".to_string());

    let payload: serde_json::Value =
        serde_json::from_str(&language_servers_json(&configured, Vec::new())).unwrap();
    let server = &payload["servers"][0];
    assert_eq!(server["language"], "go");
    assert_eq!(server["command"], "gopls");
    assert_eq!(server["state"], "idle");
    assert!(server["pid"].is_null());
}

#[test]
fn command_proposal_endpoint_serves_a_stored_proposals_directories() {
    let repo = repository_map_fixture("repomap-proposal");
    let repo_str = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_str).unwrap();
    let proposal = engine
        .validation_orchestrator
        .propose_command_at(
            &repo,
            repo.join("packages/api"),
            "npm test",
            "test proposal",
        )
        .unwrap();

    let (port, token) = serve_for_test();
    let url = format!(
        "/api/command-proposal?repo={}&proposal_id={}",
        percent_encode_for_test(&repo_str),
        proposal.id
    );
    let response = get_for_test(port, &token, &url);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let payload = json_of(&response);
    assert_eq!(
        payload["workingDirectory"],
        repo.join("packages/api").to_string_lossy().as_ref(),
        "{payload}"
    );
    assert_eq!(payload["repositoryRoot"], repo_str, "{payload}");
    // No command rule decided `npm test`: the card has no rule line.
    assert!(payload["rule"].is_null(), "{payload}");
    assert!(payload["ruleWarning"].is_null(), "{payload}");
}

/// Spec 111 D7: the card names the command rule that decided and its source,
/// and Damaian's built-in default when that blocked it.
#[test]
fn command_proposal_endpoint_names_the_deciding_rule_and_its_source() {
    let repo = repository_map_fixture("repomap-proposal-rule");
    fs::create_dir_all(repo.join(".damaian")).unwrap();
    fs::write(
        repo.join(".damaian/config.conf"),
        "command_rule=ask npm test *\n",
    )
    .unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_str).unwrap();
    let asked = engine
        .validation_orchestrator
        .propose_command(&repo, "npm test", "test proposal")
        .unwrap();
    let blocked = engine
        .validation_orchestrator
        .propose_command(&repo, "rm -rf /", "test proposal")
        .unwrap();

    let (port, token) = serve_for_test();
    let stored = |proposal_id: &str| {
        let response = get_for_test(
            port,
            &token,
            &format!(
                "/api/command-proposal?repo={}&proposal_id={proposal_id}",
                percent_encode_for_test(&repo_str)
            ),
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        json_of(&response)
    };

    let payload = stored(&asked.id);
    assert_eq!(payload["rule"], "ask npm test *", "{payload}");
    assert_eq!(payload["ruleSource"], "repository", "{payload}");
    assert_eq!(
        payload["ruleSummary"], "Asked by repository rule `ask npm test *` (repository config)",
        "{payload}"
    );
    assert!(payload["ruleWarning"].is_null(), "{payload}");

    let payload = stored(&blocked.id);
    assert_eq!(payload["ruleSource"], "default", "{payload}");
    assert_eq!(
        payload["ruleSummary"], "Damaian's default: recursive `rm` of `/`",
        "{payload}"
    );
}

fn get_session_for_test(port: u16, token: &str, session_id: &str) -> String {
    send_for_test(
        port,
        format!(
            "GET /api/session?session_id={session_id} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n"
        ),
    )
}

fn post_session_mode_for_test(port: u16, token: &str, session_id: &str, mode: &str) -> String {
    let body = format!("session_id={session_id}&mode={mode}");
    send_for_test(
        port,
        format!(
            "POST /api/session-mode HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn new_session_for_test(title: &str) -> String {
    isolated_data_dir();
    default_engine()
        .expect("default engine")
        .session_store
        .create_session("repo_session_mode_test", title)
        .expect("create session")
        .id
}

/// Requirement 7's default, made visible over the wire: a session nobody
/// has set a mode on reads as Code.
#[test]
fn session_json_includes_the_mode() {
    let session_id = new_session_for_test("mode default");
    let (port, token) = serve_for_test();

    let response = get_session_for_test(port, &token, &session_id);

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"mode\":\"code\""), "{response}");
}

/// The follow-up GET is what proves the write persisted: an endpoint that
/// only echoed the requested mode back would pass the first assertion.
#[test]
fn post_session_mode_changes_it_and_returns_the_updated_session() {
    let session_id = new_session_for_test("mode switch");
    let (port, token) = serve_for_test();

    let changed = post_session_mode_for_test(port, &token, &session_id, "ask");
    assert!(changed.starts_with("HTTP/1.1 200"), "{changed}");
    assert!(
        changed.contains(&format!("\"session\":{{\"id\":\"{session_id}\"")),
        "{changed}"
    );
    assert!(changed.contains("\"mode\":\"ask\""), "{changed}");

    let reread = get_session_for_test(port, &token, &session_id);
    assert!(reread.contains("\"mode\":\"ask\""), "{reread}");
}

/// An unknown mode must not fall back to Code: silently landing in the
/// most permissive mode is exactly the widening requirement 4 forbids.
#[test]
fn post_session_mode_rejects_an_unknown_mode_string() {
    let session_id = new_session_for_test("mode reject");
    let (port, token) = serve_for_test();
    post_session_mode_for_test(port, &token, &session_id, "review");

    let rejected = post_session_mode_for_test(port, &token, &session_id, "sideways");

    assert!(!rejected.starts_with("HTTP/1.1 200"), "{rejected}");
    assert!(rejected.contains("sideways"), "{rejected}");
    let reread = get_session_for_test(port, &token, &session_id);
    assert!(reread.contains("\"mode\":\"review\""), "{reread}");
}

/// A repository with one source file, an engine for it, and a session
/// that belongs to it.
fn findings_fixture(name: &str) -> (PathBuf, WorkspaceEngine, String) {
    isolated_data_dir();
    let repo = std::env::temp_dir().join(format!(
        "damaian-shell-findings-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(repo.join("src")).expect("repository");
    fs::write(repo.join("src/lib.rs"), "fn a() {}\n").expect("source file");
    let engine = engine_for_repo(repo.to_str().unwrap()).expect("engine");
    let repository_id = engine
        .indexer
        .repository_id_for_path(&repo)
        .expect("repository id");
    let session_id = engine
        .session_store
        .create_session(&repository_id, "Findings")
        .expect("session")
        .id;
    (repo, engine, session_id)
}

/// Records a finding. With a path, it is ranged and hashed the way
/// recording does it (spec 22 Task 8), so editing the file makes it stale.
fn record_finding_for_test(
    engine: &WorkspaceEngine,
    repo: &std::path::Path,
    session_id: &str,
    summary: &str,
    path: Option<&str>,
) -> String {
    let mut finding = Finding::new(
        FindingDraft {
            source: FindingSource::Compiler,
            severity: Severity::Error,
            summary: summary.to_string(),
            details: None,
            range: path.map(|path| SourceRange {
                path: path.to_string(),
                start_line: 1,
                start_column: None,
                end_line: None,
                end_column: None,
            }),
            code: None,
        },
        &SecretScanner::default(),
    );
    if let Some(path) = path {
        finding =
            finding.with_file_hash(workspace_engine::hash::file_hash(repo.join(path)).unwrap());
    }
    engine
        .session_store
        .record_finding(session_id, &finding)
        .expect("record finding");
    finding.id().to_string()
}

fn get_findings_for_test(
    port: u16,
    token: &str,
    repo: &std::path::Path,
    session_id: &str,
) -> String {
    send_for_test(
        port,
        format!(
            "GET /api/findings?repo={}&session_id={session_id} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n",
            repo.display()
        ),
    )
}

fn post_form_for_test(port: u16, token: &str, path: &str, body: &str) -> String {
    send_for_test(
        port,
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn json_of(response: &str) -> serde_json::Value {
    let body = response.split("\r\n\r\n").nth(1).expect("a body");
    serde_json::from_str(body).expect("JSON body")
}

fn status_of(json: &serde_json::Value, id: &str) -> String {
    json["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|finding| finding["id"] == id)
        .unwrap_or_else(|| panic!("{id} missing from {json}"))["status"]
        .as_str()
        .unwrap()
        .to_string()
}

fn get_sessions_for_test(
    port: u16,
    token: &str,
    repo: &std::path::Path,
    archived: Option<&str>,
) -> Vec<(String, bool)> {
    let filter = archived
        .map(|value| format!("&archived={value}"))
        .unwrap_or_default();
    let response = send_for_test(
        port,
        format!(
            "GET /api/sessions?repo={}{filter} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n",
            repo.display()
        ),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let mut sessions: Vec<(String, bool)> = json_of(&response)["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|session| {
            (
                session["id"].as_str().unwrap().to_string(),
                session["archived"].as_bool().expect("an archived flag"),
            )
        })
        .collect();
    sessions.sort();
    sessions
}

/// Spec 108 D3: archive and unarchive go through their routes, the
/// listing carries the flag and filters on it, and delete still deletes
/// an archived session.
#[test]
fn session_archive_routes_round_trip_and_filter_the_listing() {
    let (repo, engine, archived_id) = findings_fixture("archive");
    let repository_id = engine.indexer.repository_id_for_path(&repo).unwrap();
    let kept_id = engine
        .session_store
        .create_session(&repository_id, "Kept")
        .unwrap()
        .id;
    let (port, token) = serve_for_test();

    let response = post_form_for_test(
        port,
        &token,
        "/api/session-archive",
        &format!("session_id={archived_id}"),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert_eq!(json_of(&response)["session"]["archived"], true);

    let mut both = vec![(archived_id.clone(), true), (kept_id.clone(), false)];
    both.sort();
    assert_eq!(get_sessions_for_test(port, &token, &repo, None), both);
    assert_eq!(
        get_sessions_for_test(port, &token, &repo, Some("include")),
        both
    );
    assert_eq!(
        get_sessions_for_test(port, &token, &repo, Some("exclude")),
        vec![(kept_id.clone(), false)]
    );
    assert_eq!(
        get_sessions_for_test(port, &token, &repo, Some("only")),
        vec![(archived_id.clone(), true)]
    );

    let response = post_form_for_test(
        port,
        &token,
        "/api/session-unarchive",
        &format!("session_id={archived_id}"),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert_eq!(json_of(&response)["session"]["archived"], false);
    assert!(
        get_sessions_for_test(port, &token, &repo, Some("only")).is_empty(),
        "unarchived"
    );

    post_form_for_test(
        port,
        &token,
        "/api/session-archive",
        &format!("session_id={archived_id}"),
    );
    let response = post_form_for_test(
        port,
        &token,
        "/api/session-delete",
        &format!("session_id={archived_id}&repo={}", repo.display()),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert_eq!(
        get_sessions_for_test(port, &token, &repo, None),
        vec![(kept_id, false)]
    );
}

#[test]
fn session_archive_route_refuses_an_unknown_filter_and_session() {
    let (repo, _engine, _session_id) = findings_fixture("archive-refusals");
    let (port, token) = serve_for_test();
    let response = send_for_test(
        port,
        format!(
            "GET /api/sessions?repo={}&archived=maybe HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n",
            repo.display()
        ),
    );
    assert!(!response.starts_with("HTTP/1.1 200"), "{response}");

    let response = post_form_for_test(
        port,
        &token,
        "/api/session-archive",
        "session_id=session_missing",
    );
    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
}

/// A running worker's session is refused with 409 and stays unarchived.
/// The route asks the supervisor; this drives the answer it builds.
#[test]
fn archiving_a_running_worker_is_a_conflict() {
    let (_repo, engine, session_id) = findings_fixture("archive-running");
    let (status, body) = super::session_archive_answer(&engine, &session_id, true, true);
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("Stop the worker first"), "{body}");
    assert!(
        !engine
            .session_store
            .read_session(&session_id)
            .unwrap()
            .unwrap()
            .archived
    );

    let (status, body) = super::session_archive_answer(&engine, &session_id, true, false);
    assert_eq!(status, 200, "{body}");
}

/// Staleness is derived against `repo`'s files (spec 22 `context.md`
/// §16), so an edit shows up in the next GET. The untouched ranged finding
/// is what pins the root: read against any other directory its file is
/// missing or different, and it would be stale too.
#[test]
fn get_findings_returns_them_with_staleness_derived_against_the_repository() {
    let (repo, engine, session_id) = findings_fixture("get");
    fs::write(repo.join("src/kept.rs"), "fn kept() {}\n").expect("second file");
    let unranged = record_finding_for_test(&engine, &repo, &session_id, "no file", None);
    let ranged =
        record_finding_for_test(&engine, &repo, &session_id, "on a file", Some("src/lib.rs"));
    let untouched =
        record_finding_for_test(&engine, &repo, &session_id, "kept", Some("src/kept.rs"));
    fs::write(repo.join("src/lib.rs"), "fn a() { edited() }\n").unwrap();
    let (port, token) = serve_for_test();

    let response = get_findings_for_test(port, &token, &repo, &session_id);

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let json = json_of(&response);
    assert_eq!(json["findings"][0]["id"], unranged.as_str(), "record order");
    assert_eq!(status_of(&json, &unranged), "open");
    assert_eq!(status_of(&json, &ranged), "stale");
    assert_eq!(status_of(&json, &untouched), "open");
}

/// §16: reading a session against another checkout would judge
/// staleness on the wrong files.
#[test]
fn findings_endpoints_refuse_a_session_from_another_repository() {
    let (repo, engine, _) = findings_fixture("other-repo");
    let foreign = engine
        .session_store
        .create_session("repo_somewhere_else", "Foreign")
        .unwrap()
        .id;
    let (port, token) = serve_for_test();

    let response = get_findings_for_test(port, &token, &repo, &foreign);
    assert!(!response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("another repository"), "{response}");

    let body = format!("repo={}&session_id={foreign}&finding_ids=x", repo.display());
    let repair = post_form_for_test(port, &token, "/api/findings-repair", &body);
    assert!(!repair.starts_with("HTTP/1.1 200"), "{repair}");
}

/// The `language_server.rust` command line for this test binary's fake
/// server, `fake_language_server_for_findings_helper`. libtest prints on
/// stdout before the helper runs, so the helper speaks LSP on fd 3.
fn fake_language_server_command() -> String {
    format!(
        "/bin/sh -c \"export DAMAIAN_SHELL_FAKE_LSP=1; exec \\\"\\$0\\\" --ignored --exact \
             tests::fake_language_server_for_findings_helper 3>&1 1>/dev/null\" '{}'",
        std::env::current_exe().unwrap().display()
    )
}

/// Held by each test that uses the global hub, because each ends with
/// `shutdown_global`, which would stop the other's server under a
/// threaded `cargo test`.
static GLOBAL_HUB_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Opens `src/lib.rs` on the global hub, the one `GET /api/findings`
/// reads, and waits for a `documentSymbol` answer. The fake server
/// publishes on `didOpen`, before it reads the request, so once the
/// answer is in the hub holds the diagnostic.
fn publish_a_language_server_error(engine: &WorkspaceEngine, repo: &std::path::Path) {
    let mut config = engine.config.clone();
    config
        .language_servers
        .insert("rust".to_string(), fake_language_server_command());
    let launch = LanguageServerLaunch::from_config(&config, repo, "rust", "ses_shell")
        .expect("rust is configured");
    let hub = LanguageServerHub::global();
    hub.notify(&launch, |client, uris| {
        client.did_open(&uris.uri_for("src/lib.rs"), "rust", 1, "fn a() {}\n")
    })
    .expect("didOpen");
    hub.request(&launch, |client, uris| {
        client.document_symbol(&uris.uri_for("src/lib.rs"))
    })
    .expect("documentSymbol");
}

/// Spec 25 Task 7: the panel's GET records what the hub holds first.
#[test]
fn get_findings_returns_a_language_server_diagnostic() {
    let _hub = GLOBAL_HUB_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (repo, engine, session_id) = findings_fixture("language-server");
    publish_a_language_server_error(&engine, &repo);
    let (port, token) = serve_for_test();

    let response = get_findings_for_test(port, &token, &repo, &session_id);
    let again = get_findings_for_test(port, &token, &repo, &session_id);
    LanguageServerHub::shutdown_global();

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let json = json_of(&response);
    let findings = json["findings"].as_array().expect("findings");
    assert_eq!(findings.len(), 1, "{json}");
    let finding = &findings[0];
    assert_eq!(finding["source"], "language_server");
    assert_eq!(finding["severity"], "error");
    assert_eq!(finding["summary"], "the fake server says no");
    assert_eq!(finding["code"], "E1");
    assert_eq!(finding["taskId"], serde_json::Value::Null);
    assert_eq!(finding["range"]["path"], "src/lib.rs");
    assert_eq!(finding["range"]["startLine"], 1);
    assert_eq!(finding["range"]["startColumn"], 4);
    assert_eq!(finding["status"], "open");
    assert_eq!(
        json_of(&again)["findings"].as_array().unwrap().len(),
        1,
        "a second GET records nothing new"
    );
}

/// A recording failure is logged, and the panel still gets what the
/// session holds. Here the session log is read-only, so the append fails.
#[test]
fn get_findings_lists_what_it_has_when_recording_fails() {
    use std::os::unix::fs::PermissionsExt;

    let _hub = GLOBAL_HUB_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (repo, engine, session_id) = findings_fixture("language-server-failing");
    let kept = record_finding_for_test(&engine, &repo, &session_id, "kept", None);
    publish_a_language_server_error(&engine, &repo);
    let log = engine
        .config
        .data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    fs::set_permissions(&log, fs::Permissions::from_mode(0o444)).unwrap();
    let (port, token) = serve_for_test();

    let response = get_findings_for_test(port, &token, &repo, &session_id);
    fs::set_permissions(&log, fs::Permissions::from_mode(0o644)).unwrap();
    LanguageServerHub::shutdown_global();

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let json = json_of(&response);
    let ids: Vec<&str> = json["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .map(|finding| finding["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![kept.as_str()], "{json}");
}

/// The fake language server for the two tests above. Never run on its
/// own: they re-execute this test binary into it, and run directly it
/// returns at once. It answers `initialize`, `documentSymbol` (`[]`) and
/// `shutdown`, and on each `didOpen` publishes one error on the opened
/// file: line 0, characters 3–4, code `E1`.
#[test]
#[ignore = "the fake language server; re-executed by the findings tests"]
fn fake_language_server_for_findings_helper() {
    use std::os::fd::FromRawFd;
    use workspace_engine::{read_message, write_message};

    if std::env::var("DAMAIAN_SHELL_FAKE_LSP").is_err() {
        return;
    }
    // SAFETY: fd 3 is the pipe the command line moved there, and nothing
    // else in this process owns it.
    let mut output = unsafe { fs::File::from_raw_fd(3) };
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    loop {
        let Ok(Some(message)) = read_message(&mut input) else {
            return;
        };
        let id = message.get("id").cloned();
        let reply = match (message["method"].as_str().unwrap_or_default(), id) {
            ("initialize", Some(id)) => {
                serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": { "capabilities": {} } })
            }
            ("shutdown", Some(id)) => {
                serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null })
            }
            (_, Some(id)) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": [] }),
            ("exit", None) => return,
            ("textDocument/didOpen", None) => serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {
                    "uri": message["params"]["textDocument"]["uri"],
                    "diagnostics": [{
                        "range": {
                            "start": { "line": 0, "character": 3 },
                            "end": { "line": 0, "character": 4 },
                        },
                        "severity": 1,
                        "code": "E1",
                        "message": "the fake server says no",
                    }],
                },
            }),
            _ => continue,
        };
        write_message(&mut output, &reply).unwrap();
    }
}

/// The follow-up GET proves the write persisted. An endpoint that only
/// echoed the request would pass the first assertion.
#[test]
fn post_finding_status_dismisses_and_a_later_get_shows_it() {
    let (repo, engine, session_id) = findings_fixture("dismiss");
    let id = record_finding_for_test(&engine, &repo, &session_id, "waved away", None);
    let (port, token) = serve_for_test();

    let body = format!(
        "repo={}&session_id={session_id}&finding_id={id}&status=dismissed",
        repo.display()
    );
    let changed = post_form_for_test(port, &token, "/api/finding-status", &body);
    assert!(changed.starts_with("HTTP/1.1 200"), "{changed}");
    assert_eq!(status_of(&json_of(&changed), &id), "dismissed");

    let reread = get_findings_for_test(port, &token, &repo, &session_id);
    assert_eq!(status_of(&json_of(&reread), &id), "dismissed");
}

/// Rejected, never defaulted (`/api/session-mode`'s rule), and `stale`
/// is refused by the store (spec 22 `context.md` §13.2).
#[test]
fn post_finding_status_rejects_an_unknown_status_and_refuses_stale() {
    let (repo, engine, session_id) = findings_fixture("reject");
    let id = record_finding_for_test(&engine, &repo, &session_id, "x", None);
    let (port, token) = serve_for_test();

    for status in ["sideways", "stale"] {
        let body = format!(
            "repo={}&session_id={session_id}&finding_id={id}&status={status}",
            repo.display()
        );
        let rejected = post_form_for_test(port, &token, "/api/finding-status", &body);
        assert!(
            !rejected.starts_with("HTTP/1.1 200"),
            "{status}: {rejected}"
        );
    }
    let reread = get_findings_for_test(port, &token, &repo, &session_id);
    assert_eq!(status_of(&json_of(&reread), &id), "open");
}

#[test]
fn post_findings_repair_returns_the_request_and_its_prompt() {
    let (repo, engine, session_id) = findings_fixture("repair");
    let open = record_finding_for_test(&engine, &repo, &session_id, "fix me", None);
    let stale = record_finding_for_test(&engine, &repo, &session_id, "moved", Some("src/lib.rs"));
    fs::write(repo.join("src/lib.rs"), "fn a() { edited() }\n").unwrap();
    let (port, token) = serve_for_test();

    let body = format!(
        "repo={}&session_id={session_id}&finding_ids={open},{stale}",
        repo.display()
    );
    let response = post_form_for_test(port, &token, "/api/findings-repair", &body);

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let json = json_of(&response);
    assert_eq!(json["request"]["findings"][0]["id"], open.as_str());
    assert_eq!(json["request"]["findings"].as_array().unwrap().len(), 1);
    assert_eq!(json["request"]["excluded"][0]["findingId"], stale.as_str());
    assert_eq!(json["request"]["excluded"][0]["reason"], "stale");
    let prompt = json["prompt"].as_str().expect("a prompt");
    assert!(prompt.contains(&format!("(finding {open})")), "{prompt}");
    assert!(prompt.contains(&format!("- {stale}: stale")), "{prompt}");
}

/// spec 22 `context.md` §15: a prompt that fixes nothing is not sent.
#[test]
fn post_findings_repair_with_nothing_open_returns_a_null_prompt() {
    let (repo, engine, session_id) = findings_fixture("repair-empty");
    let id = record_finding_for_test(&engine, &repo, &session_id, "done", None);
    engine
        .session_store
        .set_finding_status(
            &session_id,
            &id,
            workspace_engine::finding::FindingStatus::Fixed,
        )
        .unwrap();
    let (port, token) = serve_for_test();

    let body = format!(
        "repo={}&session_id={session_id}&finding_ids={id}",
        repo.display()
    );
    let response = post_form_for_test(port, &token, "/api/findings-repair", &body);

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let json = json_of(&response);
    assert!(json["prompt"].is_null(), "{json}");
    assert_eq!(json["request"]["excluded"][0]["reason"], "fixed");
}

const REFUSED_TURN_ANSWER: &str =
    "Ask mode does not let me change files; switch to Code and I will.";

/// Runs a real turn through the orchestrator in which an Ask-mode session's
/// model calls `propose_patch`, and returns the session id and the adapter
/// (whose `requests` show what was replayed to the model). Shared by the
/// end-to-end refusal test and the UI inspection server.
fn refused_turn_in_ask_mode(repo: &std::path::Path) -> (String, MockModelAdapter) {
    let mut config = Config {
        data_dir: isolated_data_dir().to_path_buf(),
        enable_index_watcher: false,
        ..Config::default()
    };
    config
        .model_providers
        .push(workspace_engine::ModelProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            base_url: String::new(),
            api_key_env: String::new(),
            models: Vec::new(),
            supports_native_tools: true,
            max_output_tokens: None,
            context_token_budget: None,
            context_window: None,
            provider_reports_usage: true,
            price_per_million_input_tokens: None,
            price_per_million_output_tokens: None,
            price_per_million_cached_input_tokens: None,
            supports_explicit_cache_breakpoints: false,
        });
    let engine = WorkspaceEngine::new(config);
    let mut on_token = |_token: &str| {};
    let session_id = engine
        .chat_orchestrator
        .ask(
            repo,
            "warm up",
            &[],
            &mut MockModelAdapter::new("Ready."),
            &mut on_token,
        )
        .unwrap()
        .session
        .id;
    engine
        .session_store
        .set_session_mode(&session_id, SessionMode::Ask, "user")
        .unwrap();

    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), REFUSED_TURN_ANSWER.to_string()],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "propose_patch".to_string(),
                arguments_json:
                    r#"{"summary":"Add a file","files":[{"path":"new.txt","content":"hello\n"}]}"#
                        .to_string(),
            }],
            Vec::new(),
        ],
    );
    let cancel = CancelToken::new();
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(
            repo,
            "Add new.txt",
            &[],
            Some(&session_id),
            &mut adapter,
            &mut sink,
        )
        .unwrap();
    (session_id, adapter)
}

/// Spec 20 §5.6, "the turn says which mode blocked it", end to end: a
/// refusal needs no rendering of its own because it reaches the user by
/// two existing routes. It is replayed to the model as the refused call's
/// tool result, so the model's next answer can explain it, and the session
/// payload the UI renders on load carries that tool result too.
#[test]
fn a_refused_call_reaches_the_model_and_the_session_payload() {
    let repo = temp_path("mode-refusal-e2e");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("README.md"), "# Refusal\n").unwrap();
    let (session_id, adapter) = refused_turn_in_ask_mode(&repo);

    let refusal = "Refused: Ask mode does not allow this. Switch to Code mode to allow it.";
    let replayed = adapter.requests[1]
        .messages
        .iter()
        .find(|message| message.role == "tool")
        .expect("the refused call's result is replayed to the model");
    assert_eq!(replayed.tool_call_id.as_deref(), Some("call_1"));
    assert!(replayed.content.contains(refusal), "{}", replayed.content);

    let (port, token) = serve_for_test();
    let payload = get_session_for_test(port, &token, &session_id);
    assert!(payload.contains(REFUSED_TURN_ANSWER), "{payload}");
    assert!(payload.contains("\"role\":\"tool\""), "{payload}");
    assert!(payload.contains(refusal), "{payload}");
    assert!(!repo.join("new.txt").exists());

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn render_markdown_links_only_real_non_restricted_files() {
    let repo = temp_path("render-file-links");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/auth.rs"), "fn login() {}\n").unwrap();
    fs::write(repo.join(".env"), "SECRET=1\n").unwrap();
    let repo_str = repo.to_string_lossy().to_string();

    let content = "The fix is in src/auth.rs:12, not in src/missing.rs, and never .env.";
    let html = render_markdown_with_optional_file_links(content, Some(&repo_str));

    // Real, allowed file becomes a link with its line number.
    assert!(html.contains("class=\"file-reference\""));
    assert!(html.contains("data-path=\"src/auth.rs\""));
    assert!(html.contains("data-line=\"12\""));
    // Nonexistent path and the restricted .env are left as plain text.
    assert!(html.contains("src/missing.rs"));
    assert!(!html.contains("data-path=\"src/missing.rs\""));
    assert!(!html.contains(".env<"));
    assert!(!html.contains("data-path=\".env\""));

    // Without a repo, nothing is linked.
    let plain = render_markdown_with_optional_file_links(content, None);
    assert!(!plain.contains("file-reference"));

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn parses_keychain_api_key_references() {
    assert_eq!(
        keychain::account_from_reference("keychain:model-api-key"),
        Some("model-api-key")
    );
    assert_eq!(keychain::account_from_reference("OPENAI_API_KEY"), None);
    assert_eq!(keychain::account_from_reference("keychain:  "), None);
    assert_eq!(
        keychain::reference_for_account(" model-api-key ").unwrap(),
        "keychain:model-api-key"
    );
}

#[test]
fn rejects_invalid_keychain_account_names() {
    assert!(keychain::validate_account("").is_err());
    assert!(keychain::validate_account(" \n ").is_err());
    assert!(keychain::validate_account("model-api-key").is_ok());
}

#[test]
fn caches_model_api_keys_for_current_process() {
    let account = "test-process-cache-model-key";
    forget_model_api_key(account);

    assert_eq!(cached_model_api_key(account), None);
    remember_model_api_key(account, "sk-test-value");
    assert_eq!(
        cached_model_api_key(" test-process-cache-model-key "),
        Some("sk-test-value".to_string())
    );
    forget_model_api_key(account);
    assert_eq!(cached_model_api_key(account), None);
}

/// Local runtimes do not authenticate. A missing key must not stop a
/// request to a loopback endpoint, while a remote endpoint still fails
/// loudly instead of sending an unauthenticated request.
#[test]
fn a_missing_model_key_is_allowed_only_for_a_loopback_endpoint() {
    let mut config = Config {
        model_api_key_env: format!("DAMAIAN_UNSET_TEST_KEY_{}", std::process::id()),
        model_base_url: "http://localhost:11434".to_string(),
        ..Config::default()
    };
    assert_eq!(
        resolve_shell_model_api_key(&config),
        Ok(String::new()),
        "local Ollama serves without a key"
    );

    config.model_base_url = "https://api.example.test".to_string();
    assert!(
        resolve_shell_model_api_key(&config).is_err(),
        "a remote endpoint without a key is still a configuration error"
    );
}

#[test]
fn desktop_settings_config_path_is_user_only() {
    assert!(desktop_settings_config_path(None).is_ok());
    assert!(desktop_settings_config_path(Some("user")).is_ok());

    let repo_error = desktop_settings_config_path(Some("repo")).unwrap_err();
    assert!(repo_error.contains("desktop settings only write user config"));

    let unknown_error = desktop_settings_config_path(Some("admin")).unwrap_err();
    assert_eq!(unknown_error, "scope must be user");
}

/// The Settings task-budget fields (spec 103, 2026-11-11) save through
/// `/api/config-set`, which writes the user overlay with the engine's own
/// parser: a valid value lands, zero, text and anything above the product
/// ceiling are refused, and the file keeps the last good value.
#[test]
fn task_budget_settings_save_through_the_config_parser() {
    let dir = std::env::temp_dir().join(format!(
        "damaian-task-budget-settings-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("user.conf");

    super::update_config_overlay(path.clone(), "worker_task_budget_input_tokens", "20000000")
        .unwrap();
    super::update_config_overlay(path.clone(), "worker_task_budget_action_rounds", "120").unwrap();
    let ceiling = workspace_engine::ABSOLUTE_TASK_INPUT_TOKEN_CEILING;
    for refused in ["0", "lots", &(ceiling + 1).to_string()] {
        let error =
            super::update_config_overlay(path.clone(), "worker_task_budget_input_tokens", refused)
                .unwrap_err();
        assert!(
            error.contains("worker_task_budget_input_tokens"),
            "{refused}: {error}"
        );
    }
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        saved.contains("worker_task_budget_input_tokens=20000000"),
        "{saved}"
    );
    assert!(
        saved.contains("worker_task_budget_action_rounds=120"),
        "{saved}"
    );

    // Every field the page renders names a key the overlay accepts.
    for key in [
        "worker_task_budget_action_rounds",
        "worker_task_budget_exploration_rounds",
        "worker_task_budget_input_tokens",
        "worker_task_budget_output_tokens",
    ] {
        assert!(
            super::INDEX_HTML.contains(&format!("data-config-key=\"{key}\"")),
            "{key}"
        );
        super::update_config_overlay(path.clone(), key, "1").unwrap();
    }
    assert!(super::APP_JS.contains("\"/api/config-set\", form({ key: input.dataset.configKey"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_allows_tauri_cors_origins() {
    let tauri_request = test_request_with_headers(&[("origin", "http://tauri.localhost")]);
    let local_request = test_request_with_headers(&[("origin", "http://localhost:4765")]);
    let browser_request = test_request_with_headers(&[("origin", "https://example.test")]);
    let same_origin_request = test_request_with_headers(&[]);

    assert_eq!(
        allowed_cors_origin(&tauri_request),
        Some("http://tauri.localhost")
    );
    assert_eq!(
        allowed_cors_origin(&local_request),
        Some("http://localhost:4765")
    );
    assert_eq!(allowed_cors_origin(&browser_request), None);
    assert_eq!(allowed_cors_origin(&same_origin_request), None);
}

#[test]
fn saves_valid_config_file() {
    let path = temp_path("valid").join("config").join("user.conf");
    save_config_file(
        &path,
        "model_base_url=https://api.example.test\nmodel_name=test-model\n",
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "model_base_url=https://api.example.test\nmodel_name=test-model\n"
    );
}

// The Providers settings panel writes these two keys, so the save endpoint
// must accept them and the engine must read back what the form wrote.
// Blank fields are omitted entirely rather than written empty, which is
// what makes the engine fall back to its per-model defaults.
#[test]
fn saves_provider_token_settings_written_by_the_settings_panel() {
    let path = temp_path("token-settings").join("config").join("user.conf");
    let content = "model_provider=deepseek\n\
             model_name=deepseek-v4-flash\n\
             model_provider.deepseek.max_output_tokens=120000\n\
             model_provider.deepseek.context_token_budget=90000\n";
    save_config_file(&path, content).unwrap();

    let overlay = workspace_engine::ConfigOverlay::load(&path).expect("saved config must reload");
    let mut config = workspace_engine::Config::default();
    config.apply_overlay(overlay);
    assert_eq!(config.max_output_tokens(), Some(120_000));
    assert_eq!(config.context_token_budget(), 90_000);

    // Omitting the keys restores the built-in per-model defaults.
    let blanked = temp_path("token-settings-blank")
        .join("config")
        .join("user.conf");
    save_config_file(
        &blanked,
        "model_provider=deepseek\nmodel_name=deepseek-v4-flash\n",
    )
    .unwrap();
    let mut defaults = workspace_engine::Config::default();
    defaults.apply_overlay(workspace_engine::ConfigOverlay::load(&blanked).unwrap());
    assert_eq!(defaults.max_output_tokens(), Some(65_536));
    assert_eq!(defaults.context_token_budget(), 64_000);
}

#[test]
fn rejects_invalid_token_settings_without_writing() {
    let path = temp_path("token-settings-invalid").join("config.conf");
    let error =
        save_config_file(&path, "model_provider.deepseek.context_token_budget=0\n").unwrap_err();
    assert!(error.contains("between 1"), "unexpected error: {error}");
    assert!(!path.exists());
}

#[test]
fn rejects_invalid_config_file_without_writing() {
    let path = temp_path("invalid").join("config.conf");
    let error = save_config_file(&path, "unknown_key=value\n").unwrap_err();
    assert!(error.contains("Unknown config key"));
    assert!(!path.exists());
}

/// A checkout whose `.damaian/config.conf` holds `repository_config`.
fn policy_fixture(name: &str, repository_config: &str) -> PathBuf {
    isolated_data_dir();
    let repo = temp_path(name);
    fs::create_dir_all(repo.join(".damaian")).expect("repository");
    fs::write(repo.join(".damaian").join("config.conf"), repository_config)
        .expect("repository config");
    repo
}

fn get_effective_policy_for_test(port: u16, token: &str, query: &str) -> String {
    send_for_test(
        port,
        format!(
            "GET /api/effective-policy?{query} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n"
        ),
    )
}

/// Every refusal in the policy as `(key, class, by)`, from the rules and
/// from `otherRefused`.
fn refusals_of(json: &serde_json::Value) -> Vec<(String, String, String)> {
    let rules = json["rules"].as_array().expect("rules");
    rules
        .iter()
        .flat_map(|rule| rule["refused"].as_array().expect("refused").iter())
        .chain(json["otherRefused"].as_array().expect("otherRefused"))
        .map(|refused| {
            (
                refused["key"].as_str().unwrap().to_string(),
                refused["class"].as_str().unwrap().to_string(),
                refused["by"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn rule_of<'a>(json: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    json["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|rule| rule["key"] == key)
        .unwrap_or_else(|| panic!("{key} has no rule in {json}"))
}

/// Task 6's shape over the wire: per-entry sources, a refusal by key and
/// class, and the refused value nowhere in the response (`context.md` §8).
/// The rules must also be the lines of the text the provider syncing
/// parses, loaded for the same repository.
#[test]
fn get_effective_policy_serves_the_attributed_policy_without_a_refused_value() {
    let repo = policy_fixture(
        "policy-view",
        "restricted_patterns=secrets/**\n\
             require_approval_for_file_edits=false\n\
             shell=/tmp/evil-shell-task7\n",
    );
    let (port, token) = serve_for_test();

    let response = get_effective_policy_for_test(port, &token, &format!("repo={}", repo.display()));

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(!response.contains("evil-shell-task7"), "{response}");
    let json = json_of(&response);
    assert_eq!(json["profile"], "full");
    assert_eq!(json["profileSelected"], false);
    assert!(json["mode"].is_null(), "{json}");
    assert_eq!(json["header"], "Full repository development");
    let entries = rule_of(&json, "restricted_patterns")["entries"]
        .as_array()
        .expect("a list rule carries entries");
    assert!(
        entries
            .iter()
            .any(|entry| entry["value"] == "secrets/**" && entry["source"]["kind"] == "repository"),
        "{entries:?}"
    );
    let refusals = refusals_of(&json);
    for expected in [
        (
            "require_approval_for_file_edits",
            "restrict_only",
            "repository",
        ),
        ("shell", "forbidden", "repository"),
    ] {
        assert!(
            refusals
                .iter()
                .any(|(key, class, by)| (key.as_str(), class.as_str(), by.as_str()) == expected),
            "{expected:?} missing from {refusals:?}"
        );
    }
    let (text, error) = effective_policy_for_repo(repo.to_str().unwrap());
    assert!(error.is_empty(), "{error}");
    let lines: Vec<String> = json["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| {
            format!(
                "{}={}",
                rule["key"].as_str().unwrap(),
                rule["value"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(lines, text.lines().collect::<Vec<_>>());
}

/// Proposal §5.6 in the header: the session's mode, intersected. A session
/// from another checkout is refused rather than shown against this one.
#[test]
fn get_effective_policy_intersects_the_sessions_mode() {
    let repo = policy_fixture("policy-mode", "");
    let engine = engine_for_repo(repo.to_str().unwrap()).expect("engine");
    let repository_id = engine.indexer.repository_id_for_path(&repo).unwrap();
    let session_id = engine
        .session_store
        .create_session(&repository_id, "Policy")
        .unwrap()
        .id;
    engine
        .session_store
        .set_session_mode(&session_id, SessionMode::Ask, "user")
        .unwrap();
    let foreign = engine
        .session_store
        .create_session("repo_somewhere_else", "Foreign")
        .unwrap()
        .id;
    let (port, token) = serve_for_test();

    let response = get_effective_policy_for_test(
        port,
        &token,
        &format!("repo={}&session={session_id}", repo.display()),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let json = json_of(&response);
    assert_eq!(json["mode"], "ask");
    assert_eq!(json["header"], "Full repository development ∩ Ask mode");

    let refused = get_effective_policy_for_test(
        port,
        &token,
        &format!("repo={}&session={foreign}", repo.display()),
    );
    assert!(!refused.starts_with("HTTP/1.1 200"), "{refused}");
    assert!(refused.contains("another repository"), "{refused}");
}

/// The selection is written where `damaian profile-set` writes it, and
/// audited. The follow-up GET is what proves it persisted: an endpoint that
/// only answered with the requested profile would pass the first half.
#[test]
fn post_permission_profile_writes_audits_and_returns_the_new_policy() {
    let repo = policy_fixture("policy-select", "");
    let (port, token) = serve_for_test();
    let query = format!("repo={}", repo.display());

    let selected = post_form_for_test(
        port,
        &token,
        "/api/permission-profile",
        &format!("{query}&profile=safe_local"),
    );

    assert!(selected.starts_with("HTTP/1.1 200"), "{selected}");
    let json = json_of(&selected);
    assert_eq!(json["profile"], "safe_local");
    assert_eq!(json["profileSelected"], true);
    let edits = rule_of(&json, "require_approval_for_file_edits");
    assert_eq!(edits["value"], "true");
    assert_eq!(edits["sources"][0]["kind"], "profile");
    assert_eq!(rule_of(&json, "command_access")["value"], "local");

    let repository_id = workspace_engine::hash::repository_id_for_root(&repo);
    let user_config =
        fs::read_to_string(isolated_data_dir().join("config").join("user.conf")).unwrap();
    assert!(
        user_config.contains(&format!("permission_profile.{repository_id}=safe_local")),
        "{user_config}"
    );
    let audit = fs::read_to_string(isolated_data_dir().join("audit").join("events.jsonl")).unwrap();
    assert!(
        audit
            .lines()
            .any(|line| line.contains("permission_profile_set")
                && line.contains(&repository_id)
                && line.contains("safe_local")),
        "{audit}"
    );
    let reread = json_of(&get_effective_policy_for_test(port, &token, &query));
    assert_eq!(reread["profile"], "safe_local");

    let rejected = post_form_for_test(
        port,
        &token,
        "/api/permission-profile",
        &format!("{query}&profile=sideways"),
    );
    assert!(!rejected.starts_with("HTTP/1.1 200"), "{rejected}");
    assert!(rejected.contains("sideways"), "{rejected}");
    let reread = json_of(&get_effective_policy_for_test(port, &token, &query));
    assert_eq!(
        reread["profile"], "safe_local",
        "a refused name must not be written"
    );
}

fn get_for_test(port: u16, token: &str, path_and_query: &str) -> String {
    send_for_test(
        port,
        format!(
            "GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n"
        ),
    )
}

/// Spec 31 Task 8 in the shell: export a built-in, preview the import
/// (nothing written), import it under a new name, find it in the custom
/// listing and select it. The user's own `user.conf` lines survive, since
/// an import writes a profile file and never user config (Task 7,
/// deviation 6).
#[test]
fn a_built_in_exported_and_imported_under_a_new_name_can_be_selected() {
    let repo = policy_fixture("profile-import", "");
    let user_config = isolated_data_dir().join("config").join("user.conf");
    fs::create_dir_all(user_config.parent().unwrap()).unwrap();
    fs::write(
        &user_config,
        "max_file_bytes=4096\nignore_patterns=target/\n",
    )
    .unwrap();
    let (port, token) = serve_for_test();

    let exported = get_for_test(
        port,
        &token,
        "/api/permission-profile-export?profile=safe_local",
    );
    assert!(exported.starts_with("HTTP/1.1 200"), "{exported}");
    let text = json_of(&exported)["text"].as_str().unwrap().to_string();
    assert!(text.contains("command_access=local"), "{text}");
    let import_form = |extra: &str| {
        format!(
            "repo={}&name=my_safe&text={}{extra}",
            repo.display(),
            percent_encode_for_test(&text)
        )
    };
    let profile_file = isolated_data_dir()
        .join("config")
        .join("profiles")
        .join("my_safe.conf");
    // Read-only is in force here, but the import is compared with the
    // config a profile would narrow, so `local` is not called loosening.
    let narrowed = post_form_for_test(
        port,
        &token,
        "/api/permission-profile",
        &format!("repo={}&profile=read_only", repo.display()),
    );
    assert!(narrowed.starts_with("HTTP/1.1 200"), "{narrowed}");

    let preview = post_form_for_test(
        port,
        &token,
        "/api/permission-profile-import",
        &import_form("&preview=true"),
    );
    assert!(preview.starts_with("HTTP/1.1 200"), "{preview}");
    let preview = json_of(&preview);
    assert_eq!(preview["written"], false);
    assert_eq!(preview["exists"], false);
    assert!(
        preview["notCarried"].as_array().unwrap().is_empty(),
        "{preview}"
    );
    assert!(
        preview["loosening"].as_array().unwrap().is_empty(),
        "{preview}"
    );
    assert!(!profile_file.exists(), "a preview wrote the profile");

    let imported = post_form_for_test(
        port,
        &token,
        "/api/permission-profile-import",
        &import_form(""),
    );
    assert!(imported.starts_with("HTTP/1.1 200"), "{imported}");
    assert_eq!(json_of(&imported)["written"], true);
    assert!(profile_file.is_file());

    let listed = json_of(&get_for_test(port, &token, "/api/permission-profiles"));
    assert_eq!(listed["custom"], serde_json::json!(["my_safe"]));

    let selected = post_form_for_test(
        port,
        &token,
        "/api/permission-profile",
        &format!("repo={}&profile=my_safe", repo.display()),
    );
    assert!(selected.starts_with("HTTP/1.1 200"), "{selected}");
    let json = json_of(&selected);
    assert_eq!(json["profile"], "my_safe");
    assert_eq!(rule_of(&json, "command_access")["value"], "local");
    assert_eq!(
        rule_of(&json, "require_approval_for_file_edits")["value"],
        "true"
    );
    let user = fs::read_to_string(&user_config).unwrap();
    for line in ["max_file_bytes=4096", "ignore_patterns=target/", "=my_safe"] {
        assert!(user.contains(line), "{line} missing from {user}");
    }
}

/// The preview lists, by key and class, what a profile cannot carry and
/// what would loosen this checkout's config, and writes nothing. The
/// refused value is not in the response. A reserved name is refused at
/// preview, and an existing one at import unless replacing.
#[test]
fn a_profile_import_preview_lists_what_will_not_apply_without_its_values() {
    let repo = policy_fixture("profile-import-review", "command_access=read_only\n");
    let (port, token) = serve_for_test();
    let text = "shell=/tmp/evil-shell-task8\ncommand_access=all\nallow_file_edits=false\n";
    let post = |name: &str, extra: &str| {
        post_form_for_test(
            port,
            &token,
            "/api/permission-profile-import",
            &format!(
                "repo={}&name={name}&text={}{extra}",
                repo.display(),
                percent_encode_for_test(text)
            ),
        )
    };

    let preview = post("reviewed", "&preview=true");

    assert!(preview.starts_with("HTTP/1.1 200"), "{preview}");
    assert!(!preview.contains("evil-shell-task8"), "{preview}");
    let json = json_of(&preview);
    assert_eq!(
        json["notCarried"],
        serde_json::json!([{ "key": "shell", "class": "forbidden" }])
    );
    assert_eq!(
        json["loosening"],
        serde_json::json!([{ "key": "command_access", "class": "restrict_only" }])
    );
    assert_eq!(
        json["carried"],
        serde_json::json!(["allow_file_edits", "command_access"])
    );
    let profiles = isolated_data_dir().join("config").join("profiles");
    assert!(!profiles.join("reviewed.conf").exists());

    let reserved = post("full", "&preview=true");
    assert!(!reserved.starts_with("HTTP/1.1 200"), "{reserved}");
    assert!(reserved.contains("built-in"), "{reserved}");

    assert!(post("reviewed", "").starts_with("HTTP/1.1 200"));
    let again = post("reviewed", "");
    assert!(again.contains("already exists"), "{again}");
    let replaced = post("reviewed", "&replace=true");
    assert_eq!(json_of(&replaced)["replaced"], true, "{replaced}");
    let audit = fs::read_to_string(isolated_data_dir().join("audit").join("events.jsonl")).unwrap();
    assert!(audit.contains("permission_profile_imported"), "{audit}");
    assert!(!audit.contains("evil-shell-task8"), "{audit}");
}

/// Criterion 4 for a hand-edited custom profile (Task 3's note for Task 7):
/// the shell audits what the profile could not apply, by key and class.
/// The refused value reaches neither the response nor the audit log.
#[test]
fn a_custom_profiles_refused_keys_are_audited_when_the_shell_shows_it() {
    let repo = policy_fixture("policy-custom", "");
    let profiles = isolated_data_dir().join("config").join("profiles");
    fs::create_dir_all(&profiles).unwrap();
    fs::write(
        profiles.join("task7_custom.conf"),
        "command_access=read_only\nshell=/tmp/custom-shell-task7\n",
    )
    .unwrap();
    let (port, token) = serve_for_test();

    let selected = post_form_for_test(
        port,
        &token,
        "/api/permission-profile",
        &format!("repo={}&profile=task7_custom", repo.display()),
    );

    assert!(selected.starts_with("HTTP/1.1 200"), "{selected}");
    assert!(!selected.contains("custom-shell-task7"), "{selected}");
    let json = json_of(&selected);
    assert_eq!(rule_of(&json, "command_access")["value"], "read_only");
    assert!(
        refusals_of(&json).contains(&("shell".into(), "forbidden".into(), "profile".into())),
        "{json}"
    );
    let audit = fs::read_to_string(isolated_data_dir().join("audit").join("events.jsonl")).unwrap();
    assert!(
        audit
            .lines()
            .any(|line| line.contains("permission_profile_key_rejected")
                && line.contains("task7_custom")
                && line.contains("\"shell\"")),
        "{audit}"
    );
    assert!(!audit.contains("custom-shell-task7"), "{audit}");
}

/// Repository config is untrusted input, so a key it cannot parse is
/// skipped and reported rather than failing the load (spec 34 §7). This
/// panel therefore still shows the effective policy — a repository cannot
/// blank it out by shipping one bad line.
#[test]
fn effective_policy_survives_an_unparsable_repository_key() {
    let repo = temp_path("invalid-effective-policy");
    fs::create_dir_all(repo.join(".damaian")).unwrap();
    fs::write(
        repo.join(".damaian").join("config.conf"),
        "unknown_key=value\nignore_patterns=vendor/\n",
    )
    .unwrap();

    let (policy, error) = effective_policy_for_repo(repo.to_str().unwrap());

    assert!(error.is_empty(), "unexpected error: {error}");
    assert!(policy.contains("vendor/"), "{policy}");
    assert!(!policy.contains("unknown_key"), "{policy}");
}

// With no repository selected there is nothing to review, and the answer
// must not touch the user's real data directory to say so — every other
// path through this endpoint loads and writes user-scope state, which is
// why the engine-level behaviour is tested in
// `workspace-engine/tests/repository_config_trust.rs` instead.
#[test]
fn repository_config_review_is_empty_without_a_repository() {
    assert_eq!(
        repository_config_review_json("  ").unwrap(),
        "{\"rejectedKeys\":[],\"allowlistEntries\":[],\"hooks\":[]}"
    );
}

#[test]
fn validates_context_files_inside_repo() {
    let repo = temp_path("context-file");
    fs::create_dir_all(repo.join("src")).unwrap();
    let file = repo.join("src").join("main.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    let engine = WorkspaceEngine::new(Config::default());

    assert_eq!(
        validate_context_files(&engine, repo.to_str().unwrap(), "src/main.rs").unwrap(),
        vec!["src/main.rs"]
    );
    assert_eq!(
        validate_context_files(&engine, repo.to_str().unwrap(), file.to_str().unwrap()).unwrap(),
        vec!["src/main.rs"]
    );
}

#[test]
fn rejects_context_directories() {
    let repo = temp_path("context-directory");
    fs::create_dir_all(repo.join("src")).unwrap();
    let engine = WorkspaceEngine::new(Config::default());

    let error = validate_context_files(&engine, repo.to_str().unwrap(), "src").unwrap_err();

    assert_eq!(error, "context path must be a file");
}

#[test]
fn allows_context_files_outside_repo() {
    let repo = temp_path("context-outside");
    fs::create_dir_all(&repo).unwrap();
    let outside = repo.with_file_name(format!(
        "{}-outside.txt",
        repo.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&outside, "notes").unwrap();
    let engine = WorkspaceEngine::new(Config::default());

    let expected = fs::canonicalize(&outside)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    assert_eq!(
        validate_context_files(&engine, repo.to_str().unwrap(), outside.to_str().unwrap()).unwrap(),
        vec![expected]
    );
    fs::remove_file(outside).unwrap();
}

#[test]
fn rejects_restricted_context_files() {
    let repo = temp_path("context-restricted");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join(".env"), "API_KEY=secret\n").unwrap();
    let engine = WorkspaceEngine::new(Config::default());

    let error = validate_context_files(&engine, repo.to_str().unwrap(), ".env").unwrap_err();

    assert!(error.contains("restricted by policy"));
}

#[test]
fn rejects_restricted_context_files_outside_repo() {
    let repo = temp_path("context-restricted-outside-repo");
    fs::create_dir_all(&repo).unwrap();
    let outside_dir = temp_path("context-restricted-outside-dir");
    fs::create_dir_all(&outside_dir).unwrap();
    let outside = outside_dir.join("id_rsa");
    fs::write(&outside, "-----BEGIN PRIVATE KEY-----").unwrap();
    let engine = WorkspaceEngine::new(Config::default());

    let error = validate_context_files(&engine, repo.to_str().unwrap(), outside.to_str().unwrap())
        .unwrap_err();

    assert!(error.contains("restricted by policy"));
    fs::remove_dir_all(outside_dir).unwrap();
}

#[test]
fn validates_existing_working_folder() {
    let path = temp_path("working-folder");
    fs::create_dir_all(&path).unwrap();
    let expected = fs::canonicalize(&path).unwrap();
    assert_eq!(
        validate_working_folder(path.to_str().unwrap()).unwrap(),
        expected
    );
}

#[test]
fn rejects_file_as_working_folder() {
    let path = temp_path("working-file");
    fs::write(&path, "not a directory").unwrap();
    let error = validate_working_folder(path.to_str().unwrap()).unwrap_err();
    assert_eq!(error, "working folder must be a directory");
}

#[test]
fn rejects_missing_working_folder() {
    let path = temp_path("missing-folder");
    let error = validate_working_folder(path.to_str().unwrap()).unwrap_err();
    assert!(error.contains("working folder does not exist"));
}

#[test]
fn validates_workspace_path_inside_repo() {
    let repo = temp_path("workspace-path");
    fs::create_dir_all(repo.join("src")).unwrap();
    let file = repo.join("src").join("main.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    assert_eq!(
        validate_workspace_path(repo.to_str().unwrap(), "src/main.rs").unwrap(),
        fs::canonicalize(file).unwrap()
    );
}

#[test]
fn rejects_workspace_path_outside_repo() {
    let repo = temp_path("workspace-traversal");
    fs::create_dir_all(&repo).unwrap();
    let outside = repo.with_file_name(format!(
        "{}-outside",
        repo.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&outside, "secret").unwrap();
    let relative = format!("../{}", outside.file_name().unwrap().to_string_lossy());
    let error = validate_workspace_path(repo.to_str().unwrap(), &relative).unwrap_err();
    assert_eq!(
        error,
        "workspace path must stay inside the selected repository"
    );
    fs::remove_file(outside).unwrap();
}

#[test]
fn terminal_cwd_uses_selected_working_folder() {
    let repo = temp_path("terminal-cwd");
    fs::create_dir_all(&repo).unwrap();

    assert_eq!(
        terminal_cwd_for_repo(repo.to_str().unwrap()).unwrap(),
        fs::canonicalize(&repo).unwrap()
    );
}

#[test]
fn terminal_cd_updates_cwd_without_shelling_out() {
    let repo = temp_path("terminal-cd");
    fs::create_dir_all(repo.join("child")).unwrap();

    let result = run_terminal_command(repo.to_str().unwrap(), "cd child").unwrap();

    assert_eq!(result.cwd, fs::canonicalize(repo.join("child")).unwrap());
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.is_empty());
    assert!(result.stderr.is_empty());
}

// Spec 19: a reloaded conversation shows what each past turn spent, and
// says when the figure is an approximation.
#[test]
fn the_session_payload_carries_each_tasks_usage() {
    let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);
    let usage = HashMap::from([(
        "task_1".to_string(),
        TaskUsage {
            input_tokens: 1200,
            output_tokens: 340,
            source: UsageSource::Estimated,
            reported_cost: None,
            run_count: 1,
            cached_input_tokens: None,
            cache_reported_input_tokens: 0,
            runs_without_cache_report: 1,
            models: Vec::new(),
        },
    )]);

    let json = task_states_json(
        &statuses,
        &usage,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
    );

    assert!(json.contains("\"inputTokens\":1200"), "{json}");
    assert!(json.contains("\"outputTokens\":340"), "{json}");
    // The marker requirement 4 turns on: the shell has to be able to tell
    // the user this is an approximation rather than a measurement.
    assert!(json.contains("\"usageSource\":\"estimated\""), "{json}");
    assert!(json.contains("\"runCount\":1"), "{json}");
    // Nothing reported a cost, so no cost field at all — a zero would read
    // as "this turn was free".
    assert!(!json.contains("reportedCost"), "{json}");
    // Spec 49: this provider reported no cache split, so no cache fields
    // at all. A `0` here renders as "caching is broken" when the truth is
    // "we cannot see it".
    assert!(!json.contains("cachedInputTokens"), "{json}");
    assert!(!json.contains("cacheHitRate"), "{json}");
    // Spec 108 D5: always present beside the tokens, empty here because
    // this usage names no model.
    assert!(json.contains("\"models\":[]"), "{json}");
}

/// Spec 108 D5: a reloaded turn lists every model its calls ran on, the
/// starting model first.
#[test]
fn the_session_payload_carries_every_model_a_task_used() {
    let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);
    let model = |provider: &str, name: &str| workspace_engine::TaskModel {
        provider: provider.to_string(),
        name: name.to_string(),
    };
    let usage = HashMap::from([(
        "task_1".to_string(),
        TaskUsage {
            input_tokens: 10,
            output_tokens: 2,
            source: UsageSource::Measured,
            reported_cost: None,
            run_count: 2,
            cached_input_tokens: None,
            cache_reported_input_tokens: 0,
            runs_without_cache_report: 2,
            models: vec![
                model("deepseek", "deepseek-v4-flash"),
                model("deepseek", "deepseek-v4-pro"),
            ],
        },
    )]);

    let json = task_states_json(
        &statuses,
        &usage,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
    );

    assert!(
        json.contains(
            "\"models\":[{\"provider\":\"deepseek\",\"name\":\"deepseek-v4-flash\"},{\"provider\":\"deepseek\",\"name\":\"deepseek-v4-pro\"}]"
        ),
        "{json}"
    );
}

/// Spec 49 task 6: what the client is told about the cache.
mod cache_fields;

#[test]
fn a_reopened_session_carries_each_turns_notes() {
    let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);
    let notes = BTreeMap::from([(
        "task_1".to_string(),
        TurnNotes {
            text: "Approvals live in command_policy.rs.".to_string(),
            carried: true,
            chars: 38,
            total_chars: 38,
        },
    )]);

    let json = task_states_json(
        &statuses,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &notes,
        &HashMap::new(),
        &Config::default(),
    );

    assert!(
        json.contains(
            "\"notes\":{\"text\":\"Approvals live in command_policy.rs.\",\"carried\":true}"
        ),
        "{json}"
    );
}

#[test]
fn phases_carry_the_notes_size() {
    let phase = TurnPhase {
        kind: PhaseKind::Model,
        label: String::new(),
        round: 1,
        max_rounds: 8,
        exploration_round: 2,
        max_exploration_rounds: 24,
        notes_chars: 2_100,
        reasoning_chars: 0,
    };

    let json = phase_json(&phase);

    assert!(json.contains("\"explorationRound\":2"), "{json}");
    assert!(json.contains("\"notesChars\":2100"), "{json}");
}

#[test]
fn a_thinking_phase_carries_the_reasoning_count_and_no_text() {
    let phase = TurnPhase {
        kind: PhaseKind::Thinking,
        label: String::new(),
        round: 1,
        max_rounds: 8,
        exploration_round: 0,
        max_exploration_rounds: 24,
        notes_chars: 0,
        reasoning_chars: 1_234,
    };

    let json = phase_json(&phase);

    assert!(json.contains("\"phase\":\"thinking\""), "{json}");
    assert!(json.contains("\"reasoningChars\":1234"), "{json}");
    assert!(json.contains("\"label\":\"\""), "{json}");
}

#[test]
fn a_task_with_no_usage_reports_none_rather_than_zero() {
    // A session written before usage existed. Absent, not zero: a zero is
    // indistinguishable from a real measurement of nothing.
    let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);

    let json = task_states_json(
        &statuses,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
    );

    assert!(json.contains("\"id\":\"task_1\""), "{json}");
    assert!(!json.contains("inputTokens"), "{json}");
}

#[test]
fn a_reopened_session_still_carries_each_turns_plan() {
    // The completion report is the part a user comes back for, and it
    // would be the one thing a reload lost if the plan travelled only on
    // the live turn's SSE channel.
    let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);
    let plans = HashMap::from([("task_1".to_string(), sample_plan())]);

    let json = task_states_json(
        &statuses,
        &HashMap::new(),
        &plans,
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
    );

    assert!(json.contains("\"plan\":{"), "{json}");
    assert!(
        json.contains("\"title\":\"Read the retry helper\""),
        "{json}"
    );
    assert!(json.contains("\"outcome\":\"verified\""), "{json}");
}

#[test]
fn a_turn_that_never_planned_carries_no_plan_field() {
    // Absent, not an empty plan: a trivial turn and a plan that proposed
    // nothing are different facts, and the panel must not appear for the
    // first (§5.1).
    let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);

    let json = task_states_json(
        &statuses,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
    );

    assert!(!json.contains("\"plan\""), "{json}");
}

fn sample_web_diagnostic_record(task_id: &str) -> WebDiagnosticRecord {
    WebDiagnosticRecord {
        id: "webdiagrec_1".to_string(),
        task_id: task_id.to_string(),
        tool: "inspect_web_page".to_string(),
        url: "http://localhost:5001/".to_string(),
        recorded_at_ms: 1_758_800_000_000,
        report: WebDiagnosticReport::from_text(
            r#"{"final_url": "http://localhost:5001/", "status": 200,
                    "page_errors": ["ReferenceError: game is not defined"]}"#,
            false,
        ),
    }
}

/// Spec 12 `context.md` §3.3: the card must survive a reload, so the
/// recorded report rides on `/api/session`'s task, whole.
#[test]
fn a_reopened_session_carries_each_turns_web_diagnostics() {
    isolated_data_dir();
    let engine = default_engine().expect("default engine");
    let session = engine
        .session_store
        .create_session("repo_web_diagnostic_test", "diagnosed")
        .expect("create session");
    let task = engine
        .session_store
        .create_task(&session.id, "why is it blank?", "mock", "m")
        .expect("create task");
    engine
        .session_store
        .append_web_diagnostic(&session.id, &sample_web_diagnostic_record(&task.id))
        .expect("append record");
    let (port, token) = serve_for_test();

    let response = get_session_for_test(port, &token, &session.id);

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = response.split("\r\n\r\n").nth(1).expect("a body");
    let json: serde_json::Value = serde_json::from_str(body).expect("JSON body");
    let task_json = json["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .find(|entry| entry["id"] == task.id.as_str())
        .expect("the task");
    assert_eq!(
        task_json["webDiagnostics"][0]["report"]["details"]["page_errors"][0],
        "ReferenceError: game is not defined",
        "{body}"
    );
    assert_eq!(task_json["webDiagnostics"][0]["taskId"], task.id.as_str());
}

#[test]
fn a_turn_that_never_diagnosed_carries_no_web_diagnostics_field() {
    let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);
    let web_diagnostics = HashMap::from([(
        "task_2".to_string(),
        vec![sample_web_diagnostic_record("task_2")],
    )]);

    let json = task_states_json(
        &statuses,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &web_diagnostics,
        &BTreeMap::new(),
        &HashMap::new(),
        &Config::default(),
    );

    assert!(json.contains("\"id\":\"task_1\""), "{json}");
    assert!(!json.contains("webDiagnostics"), "{json}");
}

/// Recurses until 4 MiB of stack is in use, measured from the addresses
/// of the first and deepest frames' locals rather than assumed from a
/// frame size, which debug builds inflate. 4 MiB is twice a default
/// spawned thread's stack and half the shell's; on a 2 MiB thread this
/// aborts the test process with a stack overflow.
fn use_stack_down_to(top: usize) -> usize {
    let marker = std::hint::black_box([0u8; 1024]);
    let here = marker.as_ptr() as usize;
    // The stack grows down on every target this app builds for.
    if top - here >= 4 * 1024 * 1024 {
        return top - here;
    }
    use_stack_down_to(top).max(marker[0] as usize)
}

#[test]
fn the_shell_stack_holds_what_a_default_thread_cannot() {
    let handle = spawn_with_shell_stack("stack-test", || {
        let top = std::hint::black_box([0u8; 1]);
        use_stack_down_to(top.as_ptr() as usize)
    })
    .unwrap();
    let used = handle.join().expect("8 MiB holds 4 MiB of frames");
    assert!(used >= 4 * 1024 * 1024);
}

/// The shell never pauses a turn, so these never happen in a turn it
/// hosts; were one to arrive, it must not become an SSE event the UI does
/// not know. A note does stream (spec 110 D7).
#[test]
fn pause_progress_streams_nothing_and_a_note_streams() {
    for progress in [TurnProgress::Paused, TurnProgress::Resumed] {
        assert!(turn_progress_event(progress).is_none());
    }
    assert!(matches!(
        turn_progress_event(TurnProgress::NoteAdded {
            message_id: "msg_1".to_string(),
        }),
        Some(TurnEvent::Note(id)) if id == "msg_1"
    ));
}

#[test]
fn a_web_diagnostic_progress_event_streams_as_web_diagnostic() {
    let record = sample_web_diagnostic_record("task_1");

    let event = crate::turn_progress_event(TurnProgress::WebDiagnostic(Box::new(record.clone())));
    let Some(TurnEvent::WebDiagnostic(streamed)) = event else {
        panic!("expected TurnEvent::WebDiagnostic");
    };
    let mut out = Vec::new();
    write_sse_event(&mut out, "web_diagnostic", &web_diagnostic_json(&streamed)).unwrap();

    let text = String::from_utf8(out).unwrap();
    let data = text
        .strip_prefix("event: web_diagnostic\ndata: ")
        .and_then(|rest| rest.strip_suffix("\n\n"))
        .unwrap_or_else(|| panic!("not a web_diagnostic event: {text}"));
    let parsed: WebDiagnosticRecord = serde_json::from_str(data).expect("record JSON");
    assert_eq!(parsed, record);
    assert!(data.contains("\"recordedAtMs\":"), "{data}");
}

/// Spec 12 `context.md` §3.4: reveal reuses the artifact path check, so
/// nothing outside `<data-dir>/web-diagnostics/` can be shown in Finder.
#[test]
fn web_diagnostic_reveal_target_accepts_only_artifacts_under_the_data_dir() {
    let data_dir = std::env::temp_dir().join(format!(
        "damaian-reveal-target-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let run_dir = data_dir.join("web-diagnostics/session_1/task_1/run-1");
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(run_dir.join("page.png"), b"png").unwrap();
    fs::write(data_dir.join("config-secret.txt"), b"no").unwrap();
    let config = Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        ..Config::default()
    };

    let accepted =
        web_diagnostic_reveal_target(&config, "web-diagnostics/session_1/task_1/run-1/page.png")
            .expect("a real artifact is accepted");
    assert!(accepted.ends_with("run-1/page.png"), "{accepted:?}");

    for refused in [
        "config-secret.txt",
        "sessions/session_1.jsonl",
        "/etc/hosts",
        "web-diagnostics/../config-secret.txt",
    ] {
        assert!(
            web_diagnostic_reveal_target(&config, refused).is_err(),
            "{refused} must be refused"
        );
    }

    fs::remove_dir_all(data_dir).unwrap();
}

/// Spec 112 requirement 7: `/api/sessions` rows carry the state the sidebar
/// draws — `working` from the lanes, `needs_you` and `failed` from the newest
/// task — and no `state` at all otherwise.
#[test]
fn sessions_json_carries_each_sessions_sidebar_state() {
    let data_dir = std::env::temp_dir().join(format!(
        "damaian-session-state-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = workspace_engine::SessionStore::new(&data_dir);
    let session_with = |title: &str, status: Option<TaskStatus>| {
        let session = store.create_session("repo_1", title).unwrap();
        if let Some(status) = status {
            let task = store.create_task(&session.id, "go", "mock", "m").unwrap();
            store.update_task_status(&task, status, None).unwrap();
        }
        session
    };
    // Working wins over the task's status: the turn is running right now.
    let working = session_with("Working", Some(TaskStatus::WaitingForModel));
    let needs_you = session_with("Needs you", Some(TaskStatus::WaitingForApproval));
    let failed = session_with("Failed", Some(TaskStatus::Failed));
    let complete = session_with("Complete", Some(TaskStatus::Complete));
    let cancelled = session_with("Cancelled", Some(TaskStatus::Cancelled));
    let empty = session_with("Empty", None);
    let sessions = [&working, &needs_you, &failed, &complete, &cancelled, &empty]
        .map(|session| session.clone())
        .to_vec();
    let lanes = std::collections::HashSet::from([working.id.clone()]);

    let json = format!("[{}]", super::sessions_json(&store, &sessions, &lanes));
    let rows: serde_json::Value = serde_json::from_str(&json).unwrap();
    let state_of = |id: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .unwrap()
            .get("state")
            .cloned()
    };

    assert_eq!(state_of(&working.id), Some(serde_json::json!("working")));
    assert_eq!(
        state_of(&needs_you.id),
        Some(serde_json::json!("needs_you"))
    );
    assert_eq!(state_of(&failed.id), Some(serde_json::json!("failed")));
    for idle in [&complete, &cancelled, &empty] {
        assert_eq!(state_of(&idle.id), None, "{} has no state", idle.title);
    }
    assert_eq!(super::session_state(true, Some("failed")), Some("working"));
    assert_eq!(super::session_state(false, Some("incomplete")), None);

    fs::remove_dir_all(data_dir).unwrap();
}

// The list view needs the counts and the coverage flag, and must not grow
// a content field: the manifest deliberately has none.
#[test]
fn checkpoint_list_json_reports_what_the_list_view_shows() {
    let manifest = test_manifest();

    let json = checkpoint_list_json(&[manifest]);

    assert!(json.contains("\"checkpointId\":\"checkpoint_1\""));
    assert!(json.contains("\"summary\":\"Before: bump the version\""));
    assert!(json.contains("\"fileCount\":1"));
    assert!(json.contains("\"excludedCount\":1"));
    assert!(json.contains("\"commandEffectsCovered\":true"));
    assert!(json.contains("\"restoredAtMs\":null"));
    assert!(json.contains("\"userMessageId\":\"msg_1\""));
    // A live turn knows its task id, not its user message's id, so this is
    // how it finds its own checkpoint (spec 112 requirement 6).
    assert!(json.contains("\"taskId\":\"task_1\""));
    // The per-file list is what lets the UI offer "restore only this file".
    // Paths and origins only: the manifest has no content and this must not
    // become the place that acquires it.
    assert!(json.contains("\"files\":[{\"path\":\"src/a.rs\",\"origin\":\"patch\"}]"));
    assert!(json.contains("\"pendingApprovals\":[]"));
    assert!(!json.contains("hash"));
}

// A rewind that skipped a file and one that refused to overwrite it are
// different outcomes, and the UI has to be able to say which happened.
#[test]
fn checkpoint_restore_json_keeps_skipped_and_conflicted_apart() {
    let result = CheckpointRestoreResult {
        checkpoint_id: "checkpoint_1".to_string(),
        restored_files: vec!["src/a.rs".to_string()],
        deleted_files: Vec::new(),
        skipped_files: vec!["src/b.rs".to_string()],
        conflicted_files: vec!["src/c.rs".to_string()],
        conversation_restored: true,
        warnings: vec!["src/c.rs: changed since the checkpoint was taken".to_string()],
    };

    let json = checkpoint_restore_json(&result);

    assert!(json.contains("\"restoredFiles\":[\"src/a.rs\"]"));
    assert!(json.contains("\"skippedFiles\":[\"src/b.rs\"]"));
    assert!(json.contains("\"conflictedFiles\":[\"src/c.rs\"]"));
    assert!(json.contains("\"conversationRestored\":true"));
}

fn test_manifest() -> workspace_engine::CheckpointManifest {
    workspace_engine::CheckpointManifest {
        checkpoint_id: "checkpoint_1".to_string(),
        repository_id: "repo_1".to_string(),
        session_id: "session_1".to_string(),
        task_id: Some("task_1".to_string()),
        created_at_ms: 1_788_000_000_000,
        user_message_id: Some("msg_1".to_string()),
        summary: "Before: bump the version".to_string(),
        conversation: workspace_engine::CheckpointConversation {
            last_event_seq: 12,
            task_status: "running".to_string(),
        },
        pending_approvals: Vec::new(),
        tree_oid: "abc".to_string(),
        files: vec![workspace_engine::CheckpointFile {
            path: "src/a.rs".to_string(),
            hash: Some("hash".to_string()),
            existed: true,
            oid: Some("oid".to_string()),
            origin: workspace_engine::CheckpointOrigin::Patch,
            mode: 0o100644,
            expected_hash: None,
            expected_existed: None,
        }],
        excluded: vec![workspace_engine::CheckpointExclusion {
            path: ".env".to_string(),
            reason: "restricted_pattern".to_string(),
        }],
        restored_at_ms: None,
        command_effects_covered: true,
    }
}

// Startup must fail loudly on a data directory this build cannot read.
// The alternative — carrying on with an empty projects list — reads as data
// loss and invites the destructive recovery attempt the refusal exists to
// prevent.
#[test]
fn startup_refuses_a_data_directory_written_by_a_newer_schema() {
    let data_dir = temp_path("schema-newer");
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join("schema.conf"), "schema_version=999\n").unwrap();

    let error = verify_data_dir_schema_at(&data_dir).expect_err("startup should refuse");

    assert!(
        error.contains(&data_dir.display().to_string()),
        "refusal should name the data directory: {error}"
    );
    assert!(
        error.contains("999"),
        "refusal should name the version found: {error}"
    );
}

#[test]
fn startup_marks_a_fresh_data_directory_with_the_current_schema() {
    let data_dir = temp_path("schema-fresh");

    verify_data_dir_schema_at(&data_dir).expect("a fresh data directory should be accepted");

    assert_eq!(
        fs::read_to_string(data_dir.join("schema.conf")).expect("marker should be written"),
        "schema_version=1\n"
    );
}

// The launch sweep has to *decide* and record, not merely tidy the
// directory. An entry naming a pid that cannot exist, owned by an instance
// that cannot exist, is the shape a crashed instance leaves behind.
#[test]
fn startup_sweeps_an_entry_left_by_a_crashed_instance() {
    let data_dir = temp_path("startup-sweep");
    fs::create_dir_all(data_dir.join("processes")).unwrap();
    fs::write(
        data_dir.join("processes").join("4000002-111.json"),
        "{\"pid\":4000002,\"startTimeUs\":111,\"pgid\":4000002,\"kind\":\"command\",\
             \"sessionId\":\"ses_1\",\"registeredAtMs\":0,\"ownerPid\":4000001,\
             \"ownerStartTimeUs\":222}",
    )
    .unwrap();
    // Its own config rather than the machine's: whether this passes must
    // not depend on whether auditing happens to be enabled here.
    let config = workspace_engine::Config {
        data_dir: data_dir.clone(),
        ..workspace_engine::Config::default()
    };

    sweep_orphaned_processes(&config).expect("the sweep should run");

    assert!(
        fs::read_dir(data_dir.join("processes"))
            .unwrap()
            .next()
            .is_none(),
        "a spent entry is removed so it is not re-decided at every launch"
    );
    let log = fs::read_to_string(data_dir.join("audit").join("events.jsonl"))
        .expect("the sweep records what it decided");
    assert!(
        log.contains("orphan_process_already_exited"),
        "the sweep must record its decision, not just empty the directory: {log}"
    );
}

#[test]
fn terminal_rejects_missing_cwd() {
    let cwd = temp_path("terminal-missing");

    let error = run_terminal_command(cwd.to_str().unwrap(), "pwd").unwrap_err();

    assert!(error.contains("terminal cwd does not exist"));
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("damaian-desktop-shell-{name}-{stamp}"))
}

fn test_request_with_headers(headers: &[(&str, &str)]) -> Request {
    test_request("/api/test", headers)
}

fn test_request(path: &str, headers: &[(&str, &str)]) -> Request {
    Request {
        method: "GET".to_string(),
        path: path.to_string(),
        query: HashMap::new(),
        headers: headers
            .iter()
            .map(|(key, value)| (key.to_ascii_lowercase(), value.to_string()))
            .collect(),
        body: String::new(),
    }
}

/// End-to-end check of the real pty terminal driven directly through the
/// `terminal` module (the same entry points the desktop app's Tauri
/// commands call): open a shell, type a command and confirm the shell's
/// output comes back over the session's output channel, then resize and
/// close. Spawns a real login shell, so it is excluded from the default
/// run.
#[test]
#[ignore]
fn terminal_pty_round_trips_shell_output() {
    let cwd = super::terminal_cwd_for_repo("").expect("resolve terminal cwd");
    let data_dir = std::env::temp_dir().join(format!(
        "damaian-pty-registry-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&data_dir);
    let registry = workspace_engine::ProcessRegistry::open(&data_dir).expect("registry");
    let id = super::terminal::open(&cwd, 80, 24, &data_dir).expect("open pty session");
    assert_eq!(
        registry.entries().unwrap().len(),
        1,
        "a running shell must be recorded, or a crash leaks it"
    );
    assert_eq!(
        registry.entries().unwrap()[0].1.as_ref().unwrap().kind,
        workspace_engine::ProcessKind::Terminal.as_str()
    );
    let receiver = super::terminal::take_output(&id).expect("take output channel");

    let marker = "pty_marker_9931";
    super::terminal::write_input(&id, format!("echo {marker}\n").as_bytes()).expect("write to pty");

    let mut decoded = String::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < deadline && !decoded.contains(marker) {
        match receiver.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(chunk) => decoded.push_str(&String::from_utf8_lossy(&chunk)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(
        decoded.contains(marker),
        "expected shell output to contain {marker}, got: {decoded:?}"
    );

    super::terminal::resize(&id, 120, 40).expect("resize pty");
    assert!(
        super::terminal::close(&id).is_some(),
        "close should reap the shell"
    );
    assert!(
        registry.entries().unwrap().is_empty(),
        "requirement 4: closing a terminal is a clean exit and leaves no entry"
    );
}

/// Runs git in `repo` with a fixed identity, so a commit works on a
/// machine with no global `user.name`.
fn git_for_test(repo: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A repository on `main` with one commit, a modified tracked file and
/// an untracked one, and `origin` set to `remote`.
fn repo_bar_fixture(name: &str, remote: Option<&str>) -> PathBuf {
    let repo = temp_path(name);
    fs::create_dir_all(&repo).unwrap();
    git_for_test(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git_for_test(&repo, &["add", "a.txt"]);
    git_for_test(&repo, &["commit", "-q", "-m", "initial"]);
    // Replaces one line and appends one: +2 −1 against HEAD.
    fs::write(repo.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    // Untracked text counts as insertions: +3.
    fs::write(repo.join("new.txt"), "x\ny\nz\n").unwrap();
    if let Some(remote) = remote {
        git_for_test(&repo, &["remote", "add", "origin", remote]);
    }
    repo
}

fn git_status_for_test(repo: &std::path::Path) -> String {
    isolated_data_dir();
    let (port, token) = serve_for_test();
    get_for_test(
        port,
        &token,
        &format!(
            "/api/git-status?repo={}",
            percent_encode_for_test(&repo.to_string_lossy())
        ),
    )
}

/// Spec 58 requirement 11. The prefix assertion is what keeps the old
/// fields byte-identical: it is the exact string the route served before.
#[test]
fn git_status_adds_branch_head_counts_and_github_url() {
    let repo = repo_bar_fixture("repobar-github", Some("git@github.com:acme/widget.git"));
    let head = git_for_test(&repo, &["rev-parse", "--short", "HEAD"]);

    let response = git_status_for_test(&repo);

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = response.split("\r\n\r\n").nth(1).unwrap();
    assert!(
        body.starts_with(
            "{\"clean\":false,\"exitCode\":0,\"files\":[\
                 {\"path\":\"a.txt\",\"raw\":\" M a.txt\",\"untracked\":false,\"conflicted\":false},\
                 {\"path\":\"new.txt\",\"raw\":\"?? new.txt\",\"untracked\":true,\"conflicted\":false}],"
        ),
        "{body}"
    );
    let payload = json_of(&response);
    assert_eq!(payload["branch"], "main", "{payload}");
    assert_eq!(payload["head"], head.as_str(), "{payload}");
    assert_eq!(payload["insertions"], 5, "{payload}");
    assert_eq!(payload["deletions"], 1, "{payload}");
    assert_eq!(
        payload["githubUrl"], "https://github.com/acme/widget",
        "{payload}"
    );
}

/// Spec 108 Task 5 (D4). The route serves `raw` as status plus path, so
/// the card must print it alone: a deletion, a modification and an
/// untracked directory each name their path once.
#[test]
fn git_status_card_prints_each_path_once() {
    let repo = temp_path("git-status-card");
    fs::create_dir_all(&repo).unwrap();
    git_for_test(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("gone.md"), "x\n").unwrap();
    fs::write(repo.join("kept.md"), "one\n").unwrap();
    git_for_test(&repo, &["add", "."]);
    git_for_test(&repo, &["commit", "-q", "-m", "initial"]);
    fs::remove_file(repo.join("gone.md")).unwrap();
    fs::write(repo.join("kept.md"), "two\n").unwrap();
    fs::create_dir_all(repo.join("newdir")).unwrap();
    fs::write(repo.join("newdir/file.txt"), "n\n").unwrap();

    let payload = json_of(&git_status_for_test(&repo));
    let lines: Vec<String> = payload["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["raw"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        lines,
        [" D gone.md", " M kept.md", "?? newdir/"],
        "{payload}"
    );
    // Each record already ends with its path, which is why the card must
    // not append `path` again.
    for file in payload["files"].as_array().unwrap() {
        let (raw, path) = (
            file["raw"].as_str().unwrap(),
            file["path"].as_str().unwrap(),
        );
        assert!(
            raw.ends_with(path) && raw.matches(path).count() == 1,
            "{file}"
        );
    }

    let line = app_js_function("gitStatusLine");
    assert!(line.contains("file.raw || file.path"), "{line}");
    let render = app_js_function("renderGitStatusText");
    assert!(render.contains("gitStatusLine(file)"), "{render}");
    assert!(
        !render.contains("${file.raw") && !render.contains("file.path"),
        "the card prints the record alone: {render}"
    );
}

/// Requirement 12: an origin that is not github.com yields `null`, never
/// the raw remote.
#[test]
fn git_status_reports_a_non_github_origin_as_null() {
    let repo = repo_bar_fixture("repobar-gitlab", Some("https://gitlab.com/acme/widget.git"));

    let response = git_status_for_test(&repo);

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(!response.contains("gitlab"), "{response}");
    let payload = json_of(&response);
    assert_eq!(payload["branch"], "main", "{payload}");
    assert!(payload["githubUrl"].is_null(), "{payload}");
}

/// Requirement 14: a plain folder is the old payload plus `null`/0, not
/// an error.
#[test]
fn git_status_of_a_non_git_folder_has_null_repo_fields() {
    let folder = temp_path("repobar-plain");
    fs::create_dir_all(&folder).unwrap();

    let response = git_status_for_test(&folder);

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let payload = json_of(&response);
    assert_eq!(payload["clean"], false, "{payload}");
    assert_ne!(payload["exitCode"], 0, "{payload}");
    assert_eq!(payload["files"], serde_json::json!([]), "{payload}");
    assert!(payload["branch"].is_null(), "{payload}");
    assert!(payload["head"].is_null(), "{payload}");
    assert_eq!(payload["insertions"], 0, "{payload}");
    assert_eq!(payload["deletions"], 0, "{payload}");
    assert!(payload["githubUrl"].is_null(), "{payload}");
}

/// Requirement 13: the client never supplies the URL. A `url` field in
/// the form is ignored and the opener sees only the derived one.
#[test]
fn open_github_opens_the_derived_url_and_ignores_a_supplied_one() {
    isolated_data_dir();
    let repo = repo_bar_fixture("repobar-open", Some("https://github.com/acme/widget"));
    let form = HashMap::from([
        ("repo".to_string(), repo.to_string_lossy().to_string()),
        ("url".to_string(), "https://evil.example/".to_string()),
    ]);
    let mut opened = Vec::new();

    let url = super::open_github_for_form(&form, |url| {
        opened.push(url.to_string());
        Ok(())
    })
    .expect("open github");

    assert_eq!(url, "https://github.com/acme/widget");
    assert_eq!(opened, vec!["https://github.com/acme/widget".to_string()]);
}

/// No GitHub origin, no repository, or no folder: an error, and nothing
/// is opened.
#[test]
fn open_github_refuses_without_a_github_origin() {
    isolated_data_dir();
    let gitlab = repo_bar_fixture("repobar-open-gitlab", Some("https://gitlab.com/a/b"));
    let no_origin = repo_bar_fixture("repobar-open-none", None);
    let plain = temp_path("repobar-open-plain");
    fs::create_dir_all(&plain).unwrap();
    let missing = temp_path("repobar-open-missing");

    for folder in [&gitlab, &no_origin, &plain, &missing] {
        let form = HashMap::from([("repo".to_string(), folder.to_string_lossy().to_string())]);
        let mut opened = false;
        let result = super::open_github_for_form(&form, |_| {
            opened = true;
            Ok(())
        });
        assert!(result.is_err(), "{folder:?}: {result:?}");
        assert!(!opened, "{folder:?} opened something");
    }
}

/// The route exists and requires `repo`. Without one it fails before
/// anything could be opened, so this never launches a browser.
#[test]
fn open_github_route_requires_a_repo() {
    let (port, token) = serve_for_test();

    let response = post_fields_for_test(
        port,
        &token,
        "/api/open-github",
        &[("url", "https://evil.example/")],
    );

    assert!(response.starts_with("HTTP/1.1 500"), "{response}");
    assert!(response.contains("missing form field: repo"), "{response}");
}

/// Spec 57 Task 6: a worker's turns, hosted on this process by the same
/// turn thread `--worker` runs. Its data directory is the shell's
/// (`isolated_data_dir`), as it is in the app.
fn context_test_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "damaian-shell-context-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), "# Context\n").unwrap();
    dir
}

fn shell_session() -> String {
    isolated_data_dir();
    let engine = default_engine().expect("the shell engine should build");
    engine
        .session_store
        .create_session("repo_shell_context", "Context controls")
        .expect("a session")
        .id
}

#[test]
fn context_pin_routes_round_trip_and_session_json_carries_them() {
    let session_id = shell_session();
    let (port, token) = serve_for_test();

    let added = post_form_for_test(
        port,
        &token,
        "/api/context-pin",
        &form_body_for_test(&[
            ("session_id", &session_id),
            ("action", "add"),
            ("path", "src/lib.rs"),
            ("start", "2"),
            ("end", "8"),
        ]),
    );
    assert!(added.starts_with("HTTP/1.1 200"), "{added}");
    let json = json_of(&added);
    assert_eq!(json["pins"][0]["path"], "src/lib.rs");
    assert_eq!(json["pins"][0]["range"]["start"], 2);

    let session = json_of(&get_for_test(
        port,
        &token,
        &format!("/api/session?session_id={session_id}"),
    ));
    assert_eq!(session["contextState"]["pins"][0]["path"], "src/lib.rs");
    assert!(session["contextState"]["restrictedPaths"].is_null());

    let cleared = post_form_for_test(
        port,
        &token,
        "/api/context-pin",
        &form_body_for_test(&[("session_id", &session_id), ("action", "clear")]),
    );
    assert_eq!(json_of(&cleared)["pins"].as_array().unwrap().len(), 0);
}

#[test]
fn context_restriction_route_sets_and_clears() {
    let session_id = shell_session();
    let (port, token) = serve_for_test();

    let set = post_form_for_test(
        port,
        &token,
        "/api/context-restriction",
        &form_body_for_test(&[
            ("session_id", &session_id),
            ("paths", "src/lib.rs\nsrc/main.rs"),
        ]),
    );
    assert_eq!(
        json_of(&set)["restrictedPaths"].as_array().unwrap().len(),
        2
    );

    let cleared = post_form_for_test(
        port,
        &token,
        "/api/context-restriction",
        &form_body_for_test(&[("session_id", &session_id), ("clear", "true")]),
    );
    assert!(json_of(&cleared)["restrictedPaths"].is_null());
}

#[test]
fn a_context_pin_action_is_never_defaulted() {
    let session_id = shell_session();
    let (port, token) = serve_for_test();
    let response = post_form_for_test(
        port,
        &token,
        "/api/context-pin",
        &form_body_for_test(&[("session_id", &session_id), ("action", "unknown_action")]),
    );
    assert!(
        response.contains("Unknown context pin action"),
        "{response}"
    );
}

#[test]
fn a_context_review_decision_is_never_defaulted() {
    let repo = context_test_repo();
    let (port, token) = serve_for_test();
    let response = post_form_for_test(
        port,
        &token,
        "/api/resume-context-stream",
        &form_body_for_test(&[
            ("repo", &repo.to_string_lossy()),
            ("proposal_id", "contextreview_missing"),
            ("decision", "maybe"),
        ]),
    );
    assert!(
        response.contains("Unknown context review decision"),
        "{response}"
    );
}

#[test]
fn task_json_carries_the_manifest_and_no_item_content() {
    let mut statuses = HashMap::new();
    statuses.insert("task_1".to_string(), "complete".to_string());
    let mut contexts = HashMap::new();
    contexts.insert(
        "task_1".to_string(),
        workspace_engine::ContextManifest {
            repository_id: "repo".to_string(),
            task_id: "task_1".to_string(),
            assembled_at_ms: 0,
            limit: workspace_engine::EffectiveLimit::declared(100),
            total_tokens: 12,
            categories: Vec::new(),
            included: vec![workspace_engine::IncludedRef {
                category: workspace_engine::ContextCategory::FileRanges,
                kind: workspace_engine::ContextKind::ExplicitFile,
                path: Some("src/lib.rs".to_string()),
                range: None,
                tokens: 12,
                root_path: None,
                provenance: None,
                origin: None,
            }],
            excluded: Vec::new(),
            conversation_summary: None,
            symbol_provenance: None,
            memory: Vec::new(),
            conversation_tokens: None,
            system_prompt_tokens: None,
        },
    );

    let json = task_states_json(
        &statuses,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &contexts,
        &Config::default(),
    );

    assert!(json.contains("\"context\""), "{json}");
    assert!(json.contains("src/lib.rs"), "{json}");
    assert!(!json.contains("item content"), "{json}");
}

/// Spec 103 Task 6: a reloaded task carries the typed terminal outcome and
/// the same engine-owned control. The browser renders these fields; it has
/// no status-to-label or status-to-action table.
#[test]
fn a_reloaded_task_carries_its_typed_terminal_outcome_and_engine_control() {
    isolated_data_dir();
    let engine = WorkspaceEngine::new(Config {
        data_dir: isolated_data_dir().to_path_buf(),
        enable_index_watcher: false,
        ..Config::default()
    });
    let session = engine
        .session_store
        .create_session("repo_outcome", "Worker")
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "bounded", "mock", "m")
        .unwrap();
    let outcome = workspace_engine::TaskTerminalOutcome::new(
        &session.id,
        &task.id,
        workspace_engine::TaskTerminalOutcomeKind::NoProgress,
    );
    engine
        .session_store
        .settle_task_terminal_outcome(&task, TaskStatus::TaskNoProgress, &outcome)
        .unwrap();

    let statuses = HashMap::from([(
        task.id.clone(),
        TaskStatus::TaskNoProgress.as_str().to_string(),
    )]);
    let outcomes = HashMap::from([(task.id.clone(), outcome.clone())]);
    let json = task_states_json_with_outcomes(
        &statuses,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &BTreeMap::new(),
        &HashMap::new(),
        &engine.config,
        &outcomes,
        &HashMap::new(),
    );
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["terminalOutcome"]["kind"], "no_progress");
    assert_eq!(
        value["turnPresentation"]["terminalOutcome"]["kind"],
        "no_progress"
    );
    assert_eq!(value["turnPresentation"]["indicatorState"], "incomplete");
    assert_eq!(
        value["turnPresentation"]["stopNotice"]["label"],
        outcome.label()
    );

    let control = workspace_engine::TaskControlService::new(&engine.session_store, None)
        .session_control(&session.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        control
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind),
        Some(workspace_engine::TaskTerminalOutcomeKind::NoProgress)
    );
    assert!(control.primary_action.is_none());
    assert!(control.actions.is_empty());
}

/// Spec 103 Task 6: the parent's returned-worker JSON carries the typed
/// outcome and the same engine-owned returned control.
#[test]
fn a_parent_worker_return_carries_the_typed_outcome_and_control() {
    isolated_data_dir();
    let engine = WorkspaceEngine::new(Config {
        data_dir: isolated_data_dir().to_path_buf(),
        enable_index_watcher: false,
        ..Config::default()
    });
    let parent = engine
        .session_store
        .create_session("repo_parent", "Parent")
        .unwrap();
    let worker = engine
        .session_store
        .create_session("repo_worker", "Worker")
        .unwrap();
    let task = engine
        .session_store
        .create_task(&worker.id, "bounded", "mock", "m")
        .unwrap();
    let outcome = workspace_engine::TaskTerminalOutcome::new(
        &worker.id,
        &task.id,
        workspace_engine::TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
    )
    .with_dimension(workspace_engine::TaskBudgetDimension::ActionRounds);
    let task = engine
        .session_store
        .settle_task_terminal_outcome(&task, TaskStatus::TaskBudgetExhausted, &outcome)
        .unwrap();
    let returned = workspace_engine::WorkerReturn::new(
        &parent.id,
        &worker.id,
        &task.id,
        "branch",
        "/worktree",
        task.status.as_str(),
        "stopped",
    )
    .with_terminal_outcome(Some(outcome));

    let json = worker_returns_json(&engine, &[returned]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        value[0]["terminalOutcome"]["kind"],
        "approved_budget_exhausted"
    );
    assert_eq!(
        value[0]["control"]["terminalOutcome"]["kind"],
        "approved_budget_exhausted"
    );
    assert_eq!(value[0]["control"]["outcome"], "budget_exhausted");
    assert!(value[0]["control"]["primaryAction"].is_null());
}

/// Spec 103 Task 6: the browser must not grow a status-to-policy table for
/// the new stops. It draws the engine-owned control fields it already has.
#[test]
fn app_js_keeps_no_terminal_outcome_policy() {
    for forbidden in [
        "task_budget_exhausted",
        "task_compaction_cannot_fit",
        "task_no_progress",
        "approved_budget_exhausted",
        "compaction_cannot_fit",
    ] {
        assert!(
            !super::APP_JS.contains(forbidden),
            "app.js must not map terminal outcomes itself: {forbidden}"
        );
    }
    assert!(super::APP_JS.contains("taskControl"));
    assert!(super::APP_JS.contains("turnPresentation"));
}

#[test]
fn a_context_progress_event_becomes_the_sse_context_event() {
    let manifest = workspace_engine::ContextManifest {
        repository_id: "repo".to_string(),
        task_id: "task".to_string(),
        assembled_at_ms: 0,
        limit: workspace_engine::EffectiveLimit::declared(10),
        total_tokens: 3,
        categories: Vec::new(),
        included: Vec::new(),
        excluded: Vec::new(),
        conversation_summary: None,
        symbol_provenance: None,
        memory: Vec::new(),
        conversation_tokens: None,
        system_prompt_tokens: None,
    };
    let event = crate::turn_progress_event(TurnProgress::Context(Box::new(manifest)));
    assert!(matches!(event, Some(crate::TurnEvent::Context(_))));
}
mod worker_turns;

/// Spec 57 Task 7: the shell-side supervisor and its routes.
///
/// The supervisor starts a stub instead of the real worker: a shell script
/// that records its argv, its environment and the shape of its handover
/// line, then holds its stdin like a worker and exits when the shell lets
/// go of it. Where a test needs the socket protocol, it serves the worker's
/// socket itself (`fake_socket`). Neither is a real worker, and nothing
/// here kills a process: every stub ends by its lifeline. The tests that
/// start the real worker binary or signal a process are `#[ignore]`d in
/// `tests/workers.rs`.
mod worker_supervisor;
/// Spec 32 Task 9 over real HTTP: list, review, toggle and test a hook.
///
/// The review writes user config only — the endpoint has no path parameter,
/// and a `path` field naming the repository's own `.damaian/config.conf` is
/// ignored. A pending repository hook cannot be tested before it is
/// reviewed, and a stale approval hash is refused rather than adopting a
/// definition the user never saw. The test run reports redacted stderr.
#[test]
fn hook_routes_list_review_toggle_and_test_over_http() {
    let repo = std::env::temp_dir().join(format!(
        "damaian-hook-routes-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(repo.join(".damaian")).unwrap();
    fs::create_dir_all(repo.join("scripts")).unwrap();
    let lint_script = repo.join("scripts").join("lint-hook.sh");
    let lint_marker = repo.join("lint-ran");
    fs::write(
        &lint_script,
        format!(
            "#!/bin/sh\ntouch {}\necho '{{\"hookApiVersion\":1,\"verdict\":\"allow\"}}'\n",
            lint_marker.to_string_lossy()
        ),
    )
    .unwrap();
    let repository_config = Config::repository_config_path(&repo);
    fs::write(
        &repository_config,
        "hook.lint.command=/bin/sh\n\
             hook.lint.args=./scripts/lint-hook.sh\n\
             hook.lint.events=before_tool\n\
             hook.lint.mode=optional\n",
    )
    .unwrap();
    let repository_before = fs::read_to_string(&repository_config).unwrap();

    let data_dir = isolated_data_dir();
    let user_config = data_dir.join("config").join("user.conf");
    fs::create_dir_all(user_config.parent().unwrap()).unwrap();
    let noter_script = data_dir.join("noter.sh");
    fs::write(
        &noter_script,
        "cat > /dev/null\n\
             echo '{\"hookApiVersion\":1,\"verdict\":\"warn\",\"reason\":\"heads up\"}'\n\
             echo 'aws key AKIAIOSFODNN7EXAMPLE' >&2\n",
    )
    .unwrap();
    fs::write(
        &user_config,
        format!(
            "hook.noter.command=/bin/sh\nhook.noter.args={}\nhook.noter.events=before_tool\n",
            noter_script.to_string_lossy()
        ),
    )
    .unwrap();

    let repo_arg = repo.to_string_lossy().to_string();
    let (port, token) = serve_for_test();
    let hooks_path = format!("/api/hooks?repo={}", percent_encode_for_test(&repo_arg));

    // Both are listed; the repository hook is pending until reviewed.
    let listed = get_for_test(port, &token, &hooks_path);
    assert!(listed.starts_with("HTTP/1.1 200"), "{listed}");
    let listed = json_of(&listed);
    let rows = listed["hooks"].as_array().expect("a hooks list");
    assert_eq!(rows.len(), 2, "{listed}");
    let lint = rows.iter().find(|row| row["id"] == "lint").unwrap();
    assert_eq!(lint["activation"], "pending_review");
    assert_eq!(lint["source"], "repository");
    assert_eq!(lint["mode"], "optional");
    assert_eq!(lint["command"], "/bin/sh");
    assert_eq!(lint["events"][0], "before_tool");
    let hash = lint["hash"]
        .as_str()
        .expect("a definition hash")
        .to_string();
    let noter = rows.iter().find(|row| row["id"] == "noter").unwrap();
    assert_eq!(noter["activation"], "active");
    assert_eq!(noter["source"], "user");

    // A pending repository hook is not executed by a test run.
    let refused = post_fields_for_test(
        port,
        &token,
        "/api/hook-test",
        &[
            ("repo", repo_arg.as_str()),
            ("hook", "lint"),
            ("event", "before_tool"),
        ],
    );
    assert!(refused.starts_with("HTTP/1.1 500"), "{refused}");
    assert!(refused.contains("review"), "{refused}");
    assert!(!lint_marker.exists(), "an unreviewed hook ran");

    // A stale hash is refused, and nothing is written.
    let stale = post_fields_for_test(
        port,
        &token,
        "/api/hook-review",
        &[
            ("repo", repo_arg.as_str()),
            ("hook", "lint"),
            ("decision", "approve"),
            ("hash", "sha256:deadbeef"),
        ],
    );
    assert!(stale.starts_with("HTTP/1.1 500"), "{stale}");
    assert!(
        !fs::read_to_string(&user_config)
            .unwrap()
            .contains("hook_review."),
        "a refused review wrote user config"
    );

    // The real review: a `path` field naming the repository's own config is
    // ignored, because the endpoint has no such parameter.
    let reviewed = post_fields_for_test(
        port,
        &token,
        "/api/hook-review",
        &[
            ("repo", repo_arg.as_str()),
            ("hook", "lint"),
            ("decision", "approve"),
            ("hash", hash.as_str()),
            ("path", repository_config.to_str().unwrap()),
        ],
    );
    assert!(reviewed.starts_with("HTTP/1.1 200"), "{reviewed}");
    let reviewed = json_of(&reviewed);
    let lint = reviewed["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "lint")
        .unwrap();
    assert_eq!(lint["activation"], "active", "{reviewed}");
    assert_eq!(
        fs::read_to_string(&repository_config).unwrap(),
        repository_before,
        "the endpoint wrote the repository's own config"
    );
    let user = fs::read_to_string(&user_config).unwrap();
    assert!(user.contains("hook_review."), "{user}");
    assert!(user.contains(&hash), "{user}");

    // A user hook can be switched off, and the definition survives.
    let disabled = post_fields_for_test(
        port,
        &token,
        "/api/hook-enabled",
        &[
            ("repo", repo_arg.as_str()),
            ("hook", "noter"),
            ("enabled", "false"),
        ],
    );
    assert!(disabled.starts_with("HTTP/1.1 200"), "{disabled}");
    let disabled = json_of(&disabled);
    let noter = disabled["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "noter")
        .unwrap();
    assert_eq!(noter["activation"], "disabled", "{disabled}");
    assert_eq!(noter["enabled"], false);

    // A repository's hook is not the user's to switch off.
    let refused_toggle = post_fields_for_test(
        port,
        &token,
        "/api/hook-enabled",
        &[
            ("repo", repo_arg.as_str()),
            ("hook", "lint"),
            ("enabled", "false"),
        ],
    );
    assert!(
        refused_toggle.starts_with("HTTP/1.1 500"),
        "{refused_toggle}"
    );
    assert_eq!(
        fs::read_to_string(&repository_config).unwrap(),
        repository_before
    );

    // A test run reports the verdict and redacted stderr.
    let tested = post_fields_for_test(
        port,
        &token,
        "/api/hook-test",
        &[
            ("repo", repo_arg.as_str()),
            ("hook", "noter"),
            ("event", "before_tool"),
        ],
    );
    assert!(tested.starts_with("HTTP/1.1 200"), "{tested}");
    let tested = json_of(&tested);
    assert_eq!(tested["verdict"], "warn", "{tested}");
    assert!(tested["failure"].is_null(), "{tested}");
    let stderr = tested["stderr"].as_str().expect("stderr");
    assert!(stderr.contains("[REDACTED_AWS_ACCESS_KEY"), "{stderr}");
    assert!(!stderr.contains("AKIAIOSFODNN7EXAMPLE"), "{stderr}");

    // The reviewed repository hook now runs, and the settings listing shows
    // it active.
    let ran = post_fields_for_test(
        port,
        &token,
        "/api/hook-test",
        &[
            ("repo", repo_arg.as_str()),
            ("hook", "lint"),
            ("event", "before_tool"),
        ],
    );
    assert!(ran.starts_with("HTTP/1.1 200"), "{ran}");
    assert_eq!(json_of(&ran)["verdict"], "allow");
    assert!(lint_marker.exists(), "the reviewed hook did not run");

    let _ = fs::remove_dir_all(&repo);
}

struct CommitRouteFixture {
    repo: PathBuf,
    session_id: String,
    engine: WorkspaceEngine,
}

fn git_in(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A real repository with one accepted change and a Code-mode session. The
/// data directory is the test binary's isolated one, which the shell's own
/// `engine_for_repo` also resolves, so the route sees the same session and
/// patch the test created.
fn commit_route_fixture(name: &str) -> CommitRouteFixture {
    let data_dir = isolated_data_dir().to_path_buf();
    let repo = temp_path(name);
    std::fs::create_dir_all(&repo).unwrap();
    git_in(&repo, &["init", "--quiet", "--initial-branch=main"]);
    git_in(&repo, &["config", "user.email", "test@damaian.local"]);
    git_in(&repo, &["config", "user.name", "Damaian Test"]);
    git_in(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("a.txt"), "original\n").unwrap();
    git_in(&repo, &["add", "-A"]);
    git_in(&repo, &["commit", "--quiet", "-m", "initial"]);

    let engine = WorkspaceEngine::new(Config {
        data_dir,
        enable_index_watcher: false,
        ..Config::default()
    });
    let repository_id = workspace_engine::repository_id_for_root(&repo);
    let session = engine
        .session_store
        .create_session(&repository_id, "Commit routes")
        .unwrap();
    engine
        .session_store
        .append_message(&session.id, None, "user", "Add a file")
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "Add a file", "test", "test")
        .unwrap();
    std::fs::write(repo.join("a.txt"), "damaian\n").unwrap();
    let patch = engine
        .patch_engine
        .create_patch(
            &repo,
            &[workspace_engine::ProposedChange {
                path: "a.txt".to_string(),
                new_content: "damaian\n".to_string(),
                status: None,
                allow_restricted: false,
            }],
            Some(&task.id),
            "test",
        )
        .unwrap();
    let mut patch = patch;
    patch.session_id = session.id.clone();
    patch.origin_session_id = session.id.clone();
    engine.patch_store.save(&patch).unwrap();
    engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &patch.id, None, None, "tester", false)
        .unwrap();
    CommitRouteFixture {
        repo,
        session_id: session.id,
        engine,
    }
}

#[test]
fn commit_status_and_preparation_are_served() {
    let fixture = commit_route_fixture("commit-status");
    let (port, token) = serve_for_test();
    let repo = percent_encode_for_test(&fixture.repo.to_string_lossy());
    let status = get_for_test(
        port,
        &token,
        &format!(
            "/api/commit-status?repo={repo}&session={}",
            fixture.session_id
        ),
    );
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    let status = response_json(&status);
    assert_eq!(status["candidateCount"], 1);
    assert_eq!(status["selectedCount"], 1);
    assert_eq!(status["canCommit"], true);

    let preparation = post_fields_for_test(
        port,
        &token,
        &format!(
            "/api/commit-preparation?repo={repo}&session={}",
            fixture.session_id
        ),
        &[],
    );
    assert!(preparation.starts_with("HTTP/1.1 200"), "{preparation}");
    let preparation = response_json(&preparation);
    assert_eq!(preparation["diffRedacted"], true);
    assert!(preparation["hooks"]["hooks"].is_array());
    assert_eq!(preparation["candidates"][0]["path"], "a.txt");
    assert_eq!(preparation["candidates"][0]["selectedByDefault"], true);
    let _ = std::fs::remove_dir_all(&fixture.repo);
}

#[test]
fn a_commit_needs_its_own_approval() {
    let fixture = commit_route_fixture("commit-approval");
    let (port, token) = serve_for_test();
    let repo_text = fixture.repo.to_string_lossy().to_string();
    let repo = percent_encode_for_test(&repo_text);
    let preparation = response_json(&post_fields_for_test(
        port,
        &token,
        &format!(
            "/api/commit-preparation?repo={repo}&session={}",
            fixture.session_id
        ),
        &[],
    ));
    let prepare_id = preparation["prepareId"].as_str().unwrap();
    let before = git_in(&fixture.repo, &["rev-parse", "HEAD"]);

    let response = post_fields_for_test(
        port,
        &token,
        "/api/commit",
        &[
            ("repo", repo_text.as_str()),
            ("session_id", fixture.session_id.as_str()),
            ("prepare_id", prepare_id),
            ("paths", "a.txt"),
            ("message", "Commit a"),
        ],
    );
    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    let body = response_json(&response);
    assert_eq!(body["code"], "approval_required");
    assert_eq!(git_in(&fixture.repo, &["rev-parse", "HEAD"]), before);
    let _ = std::fs::remove_dir_all(&fixture.repo);
}

#[test]
fn a_commit_is_refused_in_ask_mode() {
    let fixture = commit_route_fixture("commit-ask");
    fixture
        .engine
        .session_store
        .set_session_mode(&fixture.session_id, SessionMode::Ask, "user")
        .unwrap();
    let (port, token) = serve_for_test();
    let repo_text = fixture.repo.to_string_lossy().to_string();
    let repo = percent_encode_for_test(&repo_text);
    let preparation = response_json(&post_fields_for_test(
        port,
        &token,
        &format!(
            "/api/commit-preparation?repo={repo}&session={}",
            fixture.session_id
        ),
        &[],
    ));
    let prepare_id = preparation["prepareId"].as_str().unwrap();
    assert_eq!(preparation["canCommit"], false);
    assert!(
        preparation["refusals"][0]
            .as_str()
            .unwrap()
            .contains("Code mode")
    );
    let response = post_fields_for_test(
        port,
        &token,
        "/api/commit",
        &[
            ("repo", repo_text.as_str()),
            ("session_id", fixture.session_id.as_str()),
            ("prepare_id", prepare_id),
            ("approval", "commit"),
            ("paths", "a.txt"),
            ("message", "Commit a"),
        ],
    );
    assert!(response.starts_with("HTTP/1.1 409"), "{response}");
    let body = response_json(&response);
    assert_eq!(body["code"], "commit_not_allowed");
    let _ = std::fs::remove_dir_all(&fixture.repo);
}

#[test]
fn a_commit_route_creates_the_commit_and_reports_the_sha() {
    let fixture = commit_route_fixture("commit-ok");
    let (port, token) = serve_for_test();
    let repo_text = fixture.repo.to_string_lossy().to_string();
    let repo = percent_encode_for_test(&repo_text);
    let preparation = response_json(&post_fields_for_test(
        port,
        &token,
        &format!(
            "/api/commit-preparation?repo={repo}&session={}",
            fixture.session_id
        ),
        &[],
    ));
    let prepare_id = preparation["prepareId"].as_str().unwrap();
    let response = post_fields_for_test(
        port,
        &token,
        "/api/commit",
        &[
            ("repo", repo_text.as_str()),
            ("session_id", fixture.session_id.as_str()),
            ("prepare_id", prepare_id),
            ("approval", "commit"),
            ("paths", "a.txt"),
            ("message", "Commit a"),
            ("allow_generated_secrets", "false"),
        ],
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = response_json(&response);
    let sha = body["commitSha"].as_str().expect("a SHA");
    assert_eq!(git_in(&fixture.repo, &["rev-parse", "HEAD"]), sha);
    assert_eq!(git_in(&fixture.repo, &["show", "HEAD:a.txt"]), "damaian");
    let _ = std::fs::remove_dir_all(&fixture.repo);
}

#[test]
fn a_rewritten_message_is_returned_with_a_code() {
    let fixture = commit_route_fixture("commit-rewrite");
    let hook = fixture.repo.join(".git/hooks/commit-msg");
    std::fs::write(
        &hook,
        "#!/bin/sh\ngrep -q '^Reviewed-by: Test$' \"$1\" || printf '\\nReviewed-by: Test\\n' >> \"$1\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&hook, permissions).unwrap();

    let (port, token) = serve_for_test();
    let repo_text = fixture.repo.to_string_lossy().to_string();
    let repo = percent_encode_for_test(&repo_text);
    let preparation = response_json(&post_fields_for_test(
        port,
        &token,
        &format!(
            "/api/commit-preparation?repo={repo}&session={}",
            fixture.session_id
        ),
        &[],
    ));
    let prepare_id = preparation["prepareId"].as_str().unwrap();
    let before = git_in(&fixture.repo, &["rev-parse", "HEAD"]);
    let response = post_fields_for_test(
        port,
        &token,
        "/api/commit",
        &[
            ("repo", repo_text.as_str()),
            ("session_id", fixture.session_id.as_str()),
            ("prepare_id", prepare_id),
            ("approval", "commit"),
            ("paths", "a.txt"),
            ("message", "Commit a"),
        ],
    );
    assert!(response.starts_with("HTTP/1.1 409"), "{response}");
    let body = response_json(&response);
    assert_eq!(body["code"], "commit_message_rewritten");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Reviewed-by: Test")
    );
    assert_eq!(git_in(&fixture.repo, &["rev-parse", "HEAD"]), before);
    let _ = std::fs::remove_dir_all(&fixture.repo);
}

/// Spec 102 Task 4: a reloaded stall card is the same engine data as the live
/// report. The shell places it; it does not inspect the reason.
#[test]
fn the_reloaded_stall_card_renders_engine_strings_without_classifying() {
    let app = include_str!("../static/app.js");
    let card = app
        .split("function renderRepairStallOffer(")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction ").next())
        .expect("the reloaded stall card");
    for engine_field in [
        "verification?.repairStall",
        "stall.label",
        "stall.explanation",
        "stall.action.label",
        "stall.action.detail",
    ] {
        assert!(
            card.contains(engine_field),
            "{engine_field} missing from {card}"
        );
    }
    for forbidden in [
        "identical_state",
        "repeated_failure",
        "equivalent_patch",
        "unchanged_task_diff",
        "oscillation",
        "canContinueRepair",
    ] {
        assert!(
            !card.contains(forbidden),
            "the stall card must not infer or offer {forbidden}: {card}"
        );
    }
}

#[test]
fn the_verification_card_renders_engine_provenance_without_classifying() {
    let app = include_str!("../static/app.js");
    let card = app
        .split("function createVerificationReportCard(")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction ").next())
        .expect("the verification report card");
    for engine_field in [
        "report.provenance",
        "group.label",
        "group.summary",
        "group.checks",
        "item.label",
        "item.reason",
        "item.evidenceLines",
        "report.actions",
        "action.label",
        "action.detail",
        "item.reasonLabel",
        "report.repairStall",
        "report.repairStall.label",
        "report.repairStall.explanation",
    ] {
        assert!(
            card.contains(engine_field),
            "{engine_field} missing from {card}"
        );
    }
    // The stall's action is flattened into report.actions, so rendering
    // report.repairStall.action here would print the same engine action twice.
    assert!(
        !card.contains("report.repairStall.action"),
        "the stall action must render once through report.actions: {card}"
    );
    // The provenance section renders the engine's own check label and
    // fully-formatted redacted evidence. Deriving either in JavaScript — from
    // a CheckId or from FindingEvidence fields — would put report business
    // logic back in the shell.
    let provenance = card
        .split("for (const group of report.provenance")
        .nth(1)
        .and_then(|rest| rest.split("What you can do").next())
        .expect("the provenance group renderer");
    assert!(
        !provenance.contains("checkLabel("),
        "the provenance section must use the engine's ProvenanceCheck.label: {provenance}"
    );
    assert!(
        !provenance.contains("finding.summary"),
        "the provenance section must render engine-formatted evidenceLines: {provenance}"
    );
    for forbidden in [
        "new_regression",
        "pre_existing",
        "unchanged_failure",
        "environment_failure",
        "unknown_provenance",
        "repairTargets",
        "repair_eligible",
        "reason.reason ===",
        "identical_state",
        "repeated_failure",
        "equivalent_patch",
        "unchanged_task_diff",
        "oscillation",
    ] {
        assert!(
            !card.contains(forbidden),
            "the card must render engine facts, not classify them: {forbidden}"
        );
    }
}

/// Spec 109 D9: Settings edits the user's `project_checks` for the selected
/// repository through `/api/project-checks`. The list lands in user config
/// keyed by the repository's id (never in the repository), reads back with its
/// effective form, refuses an entry the parser refuses or one a `|` would
/// split, and an empty list removes the key.
#[test]
fn project_checks_route_writes_user_config_and_reads_it_back() {
    let repo = temp_shell_repo("project-checks");
    let repo_str = repo.to_string_lossy().to_string();
    let (port, token) = serve_for_test();
    let repository_id = workspace_engine::repository_id_for_root(&repo);

    let saved = post_fields_for_test(
        port,
        &token,
        "/api/project-checks",
        &[
            ("repo", &repo_str),
            ("checks", "make test\n\n tools::just lint \n"),
        ],
    );
    assert!(saved.starts_with("HTTP/1.1 200"), "{saved}");
    let payload = json_of(&saved);
    assert_eq!(payload["repositoryId"], repository_id.as_str());
    assert_eq!(payload["checks"][0]["command"], "make test");
    assert_eq!(payload["checks"][1]["workingDirectory"], "tools");
    assert_eq!(payload["checks"][1]["entry"], "tools::just lint");
    assert_eq!(
        payload["effective"].as_array().unwrap().len(),
        2,
        "{payload}"
    );
    let config_path = payload["configPath"].as_str().unwrap().to_string();
    let written = fs::read_to_string(&config_path).unwrap();
    assert!(
        written.contains(&format!(
            "project_checks.{repository_id}=make test|tools::just lint"
        )),
        "{written}"
    );
    assert!(
        !repo.join(".damaian/config.conf").exists(),
        "nothing is written into the repository"
    );

    let url = format!(
        "/api/project-checks?repo={}",
        percent_encode_for_test(&repo_str)
    );
    let reread = json_of(&get_for_test(port, &token, &url));
    assert_eq!(reread["checks"], payload["checks"]);

    for refused in ["make test | tee log", "../up::make test", "tools::"] {
        let response = post_fields_for_test(
            port,
            &token,
            "/api/project-checks",
            &[("repo", &repo_str), ("checks", refused)],
        );
        assert!(
            !response.starts_with("HTTP/1.1 200"),
            "{refused}: {response}"
        );
    }
    assert_eq!(
        json_of(&get_for_test(port, &token, &url))["checks"],
        payload["checks"],
        "a refused save leaves the list as it was"
    );

    let cleared = post_fields_for_test(
        port,
        &token,
        "/api/project-checks",
        &[("repo", &repo_str), ("checks", "")],
    );
    assert!(json_of(&cleared)["checks"].as_array().unwrap().is_empty());
    let written = fs::read_to_string(&config_path).unwrap();
    assert!(!written.contains("project_checks."), "{written}");
}

/// Spec 109 D9: the Settings page has the Project checks editor, and app.js
/// loads and saves it through the route.
#[test]
fn app_js_edits_project_checks_in_settings() {
    for id in [
        "project-checks-section",
        "project-checks-editor",
        "project-checks-save-btn",
        "project-checks-status",
    ] {
        assert!(super::INDEX_HTML.contains(&format!("id=\"{id}\"")), "{id}");
    }
    assert!(super::APP_JS.contains("`/api/project-checks?repo=${encodeURIComponent(repoPath)}`"));
    assert!(super::APP_JS.contains("\"/api/project-checks\",\n    form({ repo: repoPath, checks: $(\"project-checks-editor\").value }),"));
}

/// Spec 111 Task 5: Settings › Commands lists Damaian's defaults and every
/// rule in force with its source, and edits only the user's own
/// `command_rule` lines, in user config. A repository's `ask`/`deny` is
/// listed but not editable, its `allow` is refused and reported by line, and
/// nothing is written into the repository. A list that changed since it was
/// read, or a line the parser refuses, leaves user config as it was.
#[test]
fn command_rules_route_lists_sources_and_edits_only_user_rules() {
    let repo = temp_shell_repo("command-rules");
    let repository_config = "command_rule=deny git push *\ncommand_rule=allow make deploy\n";
    fs::write(repo.join(".damaian/config.conf"), repository_config).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    let (port, token) = serve_for_test();
    let url = format!(
        "/api/command-rules?repo={}",
        percent_encode_for_test(&repo_str)
    );

    let read = json_of(&get_for_test(port, &token, &url));
    assert_eq!(read["defaults"].as_array().unwrap().len(), 7, "{read}");
    assert_eq!(read["defaults"][0]["name"], "recursive_remove", "{read}");
    let repository_rule = read["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["source"] == "repository")
        .cloned()
        .expect("the repository's deny is listed");
    assert_eq!(repository_rule["text"], "deny git push *", "{read}");
    assert_eq!(repository_rule["line"], 1, "{read}");
    assert!(repository_rule["userIndex"].is_null(), "{read}");
    assert!(
        read["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|rule| rule["effect"] != "allow" || rule["source"] != "repository"),
        "a repository allow never reaches the list: {read}"
    );
    assert_eq!(read["refused"][0]["source"], "repository", "{read}");
    assert_eq!(read["refused"][0]["line"], 2, "{read}");
    let previous = read["userRules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule.as_str().unwrap().to_string())
        .collect::<Vec<_>>();

    let mut added = previous.clone();
    added.push("allow cargo test *".to_string());
    added.push("ask npm publish * in ~/work".to_string());
    let saved = post_fields_for_test(
        port,
        &token,
        "/api/command-rules",
        &[
            ("repo", &repo_str),
            ("rules", &added.join("\n")),
            ("previous", &previous.join("\n")),
        ],
    );
    assert!(saved.starts_with("HTTP/1.1 200"), "{saved}");
    let payload = json_of(&saved);
    let config_path = payload["configPath"].as_str().unwrap().to_string();
    let written = fs::read_to_string(&config_path).unwrap();
    assert!(
        written.contains("command_rule=allow cargo test *"),
        "{written}"
    );
    assert!(
        written.contains("command_rule=ask npm publish * in ~/work"),
        "{written}"
    );
    assert_eq!(
        fs::read_to_string(repo.join(".damaian/config.conf")).unwrap(),
        repository_config,
        "nothing is written into the repository"
    );
    let rules = payload["rules"].as_array().unwrap();
    let position = |text: &str| {
        rules
            .iter()
            .position(|rule| rule["text"] == text)
            .unwrap_or_else(|| panic!("{text} is listed: {payload}"))
    };
    assert!(
        position("deny git push *") < position("allow cargo test *"),
        "repository rules come before the user's, as they are evaluated: {payload}"
    );
    let own = &rules[position("ask npm publish * in ~/work")];
    assert_eq!(own["source"], "user", "{payload}");
    assert_eq!(own["directory"], "~/work", "{payload}");
    assert_eq!(own["userIndex"], previous.len() + 1, "{payload}");

    let stale = post_fields_for_test(
        port,
        &token,
        "/api/command-rules",
        &[
            ("repo", &repo_str),
            ("rules", ""),
            ("previous", &previous.join("\n")),
        ],
    );
    assert!(!stale.starts_with("HTTP/1.1 200"), "{stale}");
    assert!(
        stale.contains("changed since Settings read them"),
        "{stale}"
    );
    let mut refused = added.clone();
    refused.push("permit ls".to_string());
    let invalid = post_fields_for_test(
        port,
        &token,
        "/api/command-rules",
        &[
            ("repo", &repo_str),
            ("rules", &refused.join("\n")),
            ("previous", &added.join("\n")),
        ],
    );
    assert!(!invalid.starts_with("HTTP/1.1 200"), "{invalid}");
    assert!(
        invalid.contains(&format!("Rule {}", refused.len())),
        "{invalid}"
    );
    assert_eq!(
        fs::read_to_string(&config_path).unwrap(),
        written,
        "a refused save leaves user config as it was"
    );

    // Reordering is the same write.
    let mut reordered = added.clone();
    let last = reordered.len() - 1;
    reordered.swap(last - 1, last);
    let moved = json_of(&post_fields_for_test(
        port,
        &token,
        "/api/command-rules",
        &[
            ("repo", &repo_str),
            ("rules", &reordered.join("\n")),
            ("previous", &added.join("\n")),
        ],
    ));
    assert_eq!(moved["userRules"][last - 1], "ask npm publish * in ~/work");

    let cleared = json_of(&post_fields_for_test(
        port,
        &token,
        "/api/command-rules",
        &[
            ("repo", &repo_str),
            ("rules", &previous.join("\n")),
            ("previous", &reordered.join("\n")),
        ],
    ));
    assert_eq!(
        cleared["userRules"].as_array().unwrap().len(),
        previous.len()
    );
}

/// Spec 111 Task 5: the Commands page exists, loads through the route, and
/// sends every edit with the list it replaces.
#[test]
fn app_js_lists_and_edits_command_rules_in_settings() {
    for id in [
        "settings-page-commands",
        "command-rules-status",
        "command-rules-notes",
        "command-rule-add-btn",
        "command-rule-form",
        "command-rule-effect",
        "command-rule-pattern",
        "command-rule-directory",
        "command-rule-save-btn",
        "command-rules-list",
        "command-defaults-list",
    ] {
        assert!(super::INDEX_HTML.contains(&format!("id=\"{id}\"")), "{id}");
    }
    assert!(super::INDEX_HTML.contains("data-settings-page=\"commands\""));
    assert!(super::APP_JS.contains("api(`/api/command-rules${query}`)"));
    assert!(
        super::APP_JS.contains("previous: (commandRulesPayload?.userRules || []).join(\"\\n\"),")
    );
    // Only the user's own rules get a ⋮ menu; Delete is in it, confirmed.
    assert!(
        super::APP_JS.contains("if (rule.userIndex !== null && rule.userIndex !== undefined) {")
    );
    assert!(super::APP_JS.contains("\"Delete command rule?\""));
}

/// Spec 109 D5: `/api/command-session-approval` records **Approve for this
/// session** for a stored proposal that qualifies and refuses one that does
/// not; `/api/session-approvals-revoke` ends the session's approvals and says
/// how many. Neither runs anything.
#[test]
fn session_approval_routes_record_refuse_and_revoke() {
    let repo = temp_shell_repo("session-approval-routes");
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q"]);
    let repo_str = repo.to_string_lossy().to_string();
    let engine = engine_for_repo(&repo_str).unwrap();
    let qualifies = engine
        .validation_orchestrator
        .propose_command(&repo, "stack test", "test proposal")
        .unwrap();
    let chain = engine
        .validation_orchestrator
        .propose_command(&repo, "stack test && stack build", "test proposal")
        .unwrap();
    let (port, token) = serve_for_test();

    let approved = post_fields_for_test(
        port,
        &token,
        "/api/command-session-approval",
        &[
            ("repo", &repo_str),
            ("session_id", "session_routes"),
            ("proposal_id", &qualifies.id),
        ],
    );
    assert!(approved.starts_with("HTTP/1.1 200"), "{approved}");
    assert_eq!(json_of(&approved)["command"], "stack test");
    let refused = post_fields_for_test(
        port,
        &token,
        "/api/command-session-approval",
        &[
            ("repo", &repo_str),
            ("session_id", "session_routes"),
            ("proposal_id", &chain.id),
        ],
    );
    assert!(refused.starts_with("HTTP/1.1 409"), "{refused}");
    assert!(
        json_of(&refused)["error"]
            .as_str()
            .unwrap()
            .contains("shell control")
    );
    assert_eq!(
        engine
            .session_store
            .command_session_approvals("session_routes")
            .unwrap()
            .len(),
        1
    );

    let revoked = post_fields_for_test(
        port,
        &token,
        "/api/session-approvals-revoke",
        &[("repo", &repo_str), ("session_id", "session_routes")],
    );
    assert!(revoked.starts_with("HTTP/1.1 200"), "{revoked}");
    assert_eq!(json_of(&revoked)["revoked"], 1);
    assert!(
        engine
            .session_store
            .command_session_approvals("session_routes")
            .unwrap()
            .is_empty()
    );
}

/// Spec 109 D5: the command card offers **Approve for this session** in its
/// approval overflow and a check or baseline card per check, only when the
/// payload says the command qualifies; both record the approval before the
/// ordinary approval runs it. The session ⋮ menus carry **Revoke session
/// approvals**.
#[test]
fn app_js_offers_approve_for_this_session_and_its_revoke() {
    let app = super::APP_JS;
    assert!(app.contains("if (proposal.sessionApprovable && !decide && !detached) {"));
    assert!(app.contains("label: \"Approve for this session\","));
    assert!(app.contains(
        "if (sessionApproval) await approveForSession(proposalRepo, proposal.proposalId);"
    ));
    assert!(app.contains("if (check.sessionApprovable && !decide) {"));
    assert!(app.contains(
        "for (const proposalId of forSession) await approveForSession(batchRepo, proposalId);"
    ));
    assert!(app.contains("\"/api/command-session-approval\","));
    assert!(app.contains("\"/api/session-approvals-revoke\","));
    assert!(app.contains("label: \"Revoke session approvals\","));
    assert_eq!(
        app.matches("        revokeSessionApprovalsMenuItem(projectPath, session")
            .count(),
        2,
        "both session ⋮ menus"
    );
}
