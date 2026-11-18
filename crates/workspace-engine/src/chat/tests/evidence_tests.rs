use super::*;
use crate::plan::Evidence;

/// Requirement 6, structurally. The rule is that a step's status is a
/// function of what Damaian observed, and the enum is where that is
/// enforced: there is no variant a model claim could be poured into.
///
/// Asserted against the source rather than the type system because the
/// failure mode is someone *adding* a variant under deadline, which no
/// type-level assertion can catch. This makes that a deliberate, visible
/// act — the test names the requirement it would be breaking.
///
/// Comments are stripped before the scan, and that is not a convenience:
/// the first version of this test failed on `plan.rs`'s own doc comment
/// explaining why the variant does not exist. A guard that cannot tell a
/// declaration from prose about the absence of one would be removed by the
/// next person who documents the rule, which is precisely the person it
/// exists to protect.
#[test]
fn evidence_has_no_variant_a_model_could_fill() {
    let code: String = include_str!("../../plan.rs")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("ModelAsserted"),
        "requirement 6: evidence is what Damaian observed, never what the model claimed"
    );
}

#[test]
fn a_failing_command_produces_evidence_carrying_its_code_and_directory() {
    assert_eq!(
        evidence_for(
            &ActionOutcome::CommandExit(Some(1), CommandPurpose::Check),
            "action_1",
            Some("cargo test"),
            Some("crates/engine")
        ),
        Some(Evidence::CommandExit {
            marker_id: "action_1".to_string(),
            exit_code: Some(1),
            purpose: CommandPurpose::Check,
            command_hash: Some(crate::hash::sha256("cargo test")),
            working_directory: "crates/engine".to_string(),
        })
    );
}

#[test]
fn a_command_with_no_exit_code_still_produces_evidence() {
    // Not `None`. "We ran it and learned nothing" is a different fact from
    // "we did not run it", and the status rule needs the difference to
    // block the step rather than complete it unverified.
    assert_eq!(
        evidence_for(
            &ActionOutcome::CommandExit(None, CommandPurpose::Other),
            "action_1",
            None,
            None
        ),
        Some(Evidence::CommandExit {
            marker_id: "action_1".to_string(),
            exit_code: None,
            purpose: CommandPurpose::Other,
            command_hash: None,
            working_directory: String::new(),
        })
    );
}

#[test]
fn a_failed_exploration_still_produces_evidence_carrying_its_purpose() {
    // Spec 63 §5.3: the exit stays evidence. What changes is only how the
    // status rule reads it, so the purpose has to reach the step.
    assert_eq!(
        evidence_for(
            &ActionOutcome::CommandExit(Some(1), CommandPurpose::Exploration),
            "action_1",
            None,
            None
        ),
        Some(Evidence::CommandExit {
            marker_id: "action_1".to_string(),
            exit_code: Some(1),
            purpose: CommandPurpose::Exploration,
            command_hash: None,
            working_directory: String::new(),
        })
    );
}

#[test]
fn a_read_only_tool_that_succeeded_produces_no_evidence() {
    // A `read_file` that worked says nothing about whether the step it
    // served is done. Manufacturing a success record from it is exactly
    // the letter-not-spirit failure requirement 6 guards against — it
    // would make every step look verified.
    assert_eq!(
        evidence_for(&ActionOutcome::Ok, "action_1", None, None),
        None
    );
}

#[test]
fn a_tool_that_reported_failure_produces_no_false_success() {
    // `Failed` carries no exit code, so there is nothing to record that
    // would not be invented. The step's own status comes from the absence
    // of confirming evidence, not from a fabricated failure record.
    assert_eq!(
        evidence_for(&ActionOutcome::Failed, "action_1", None, None),
        None
    );
}

/// Requirement 5: a command that was killed reports why it stopped. `-1` is
/// what `exit_code.unwrap_or(-1)` prints, and a killed command reads as a
/// failed one if its termination is not named.
#[test]
fn a_timed_out_command_reports_its_termination_not_a_negative_exit_code() {
    let execution = CommandExecution {
        id: "cmd_1".to_string(),
        command: "sleep 30".to_string(),
        working_directory: "/tmp".to_string(),
        risk: crate::command_policy::CommandRisk::Low,
        approved_by: None,
        started_at_ms: 0,
        completed_at_ms: 1,
        exit_code: None,
        termination: crate::command_runner::CommandTermination::TimedOut,
        stdout: String::new(),
        stderr: String::new(),
        purpose: crate::command_purpose::CommandPurpose::Other,
    };

    let context = sandbox_command_context(&execution);

    assert!(context.contains("timed out"), "{context}");
    assert!(
        !context.contains("-1"),
        "a killed command must not read as exit -1: {context}"
    );
}

/// Requirement 6's second half: the within-turn array is bounded, and the
/// elision is stated. A request that grows with every round is what reaches
/// the token ceiling on array size rather than on work.
#[test]
fn a_long_turn_is_clamped_and_says_what_it_cut() {
    let mut messages = vec![ModelMessage::system("system"), ModelMessage::user("prompt")];
    for index in 0..20 {
        messages.push(ModelMessage::assistant(format!("assistant {index}")));
        messages.push(ModelMessage::tool(
            format!("call_{index}"),
            format!("tool {index}"),
        ));
    }

    let bounded = bounded_messages(&messages, 10, &NotesWindow::none());

    assert!(
        bounded.len() <= 10,
        "the request must be bounded: {}",
        bounded.len()
    );
    assert_eq!(bounded[0].content, "system");
    assert_eq!(bounded[1].content, "prompt");
    assert!(
        bounded[2].content.contains("elided"),
        "the cut must be stated: {}",
        bounded[2].content
    );
    assert!(
        bounded[3].content.starts_with("assistant"),
        "a retained tool result must keep the call before it: {}",
        bounded[3].content
    );
}

/// A batched round is one assistant message and a result per call, so a
/// cut by pairs could keep a result whose call was dropped. The cut must
/// land on a round's start instead.
#[test]
fn a_clamp_never_splits_a_batched_round() {
    let mut messages = vec![ModelMessage::system("system"), ModelMessage::user("prompt")];
    for round in 0..6 {
        let calls: Vec<ToolCall> = (0..3)
            .map(|call| ToolCall {
                id: format!("call_{round}_{call}"),
                name: "read_file".to_string(),
                arguments_json: "{}".to_string(),
            })
            .collect();
        messages.push(ModelMessage::assistant_with_tool_calls(
            format!("round {round}"),
            calls.clone(),
            Some("reasoning".to_string()),
        ));
        for call in calls {
            messages.push(ModelMessage::tool(call.id, "result"));
        }
    }

    let last_round = &messages[messages.len() - 4..];
    for max in 3..messages.len() {
        let bounded = bounded_messages(&messages, max, &NotesWindow::none());
        let kept_only_the_latest_round = bounded[3..] == *last_round;
        assert!(
            bounded.len() <= max || kept_only_the_latest_round,
            "bounded to {max}: {}",
            bounded.len()
        );
        let listed: Vec<&str> = bounded
            .iter()
            .flat_map(|message| &message.tool_calls)
            .map(|call| call.id.as_str())
            .collect();
        for message in bounded.iter().filter(|message| message.role == "tool") {
            let id = message.tool_call_id.as_deref().unwrap();
            assert!(
                listed.contains(&id),
                "bounded to {max}: `{id}` lost its call"
            );
        }
    }
}

#[test]
fn a_turn_inside_the_window_is_left_alone() {
    let messages = vec![ModelMessage::system("system"), ModelMessage::user("prompt")];
    let bounded = bounded_messages(&messages, 24, &NotesWindow::none());
    assert_eq!(bounded.len(), 2);
    assert!(
        !bounded
            .iter()
            .any(|message| message.content.contains("elided"))
    );
}

/// Spec 65's strongest available "byte-identical to before": the
/// pre-change algorithm, copied verbatim, compared against the real
/// function at every window and length in a representative matrix.
#[test]
fn with_notes_off_every_request_is_byte_identical_to_todays() {
    fn bounded_messages_before_65(
        messages: &[ModelMessage],
        max_messages: usize,
    ) -> Vec<ModelMessage> {
        if messages.len() <= max_messages {
            return messages.to_vec();
        }
        let head = messages[..2].to_vec();
        let tail = &messages[2..];
        let budget = max_messages.saturating_sub(3);
        let mut drop_count = tail.len().saturating_sub(budget);
        if drop_count < tail.len() && tail[drop_count].role != "assistant" {
            let is_round_start = |message: &ModelMessage| message.role == "assistant";
            drop_count = match tail[drop_count..].iter().position(is_round_start) {
                Some(offset) => drop_count + offset,
                None => tail[..drop_count]
                    .iter()
                    .rposition(is_round_start)
                    .unwrap_or(0),
            };
        }
        let results = tail[..drop_count]
            .iter()
            .filter(|message| message.role != "assistant")
            .count();
        let mut bounded = head;
        bounded.push(ModelMessage::system(format!(
            "Earlier rounds of this turn were elided to bound the request: {drop_count} messages ({results} call results) omitted. The session log still holds them; repeat a read if you need it."
        )));
        bounded.extend_from_slice(&tail[drop_count..]);
        bounded
    }

    for rounds in 0..14 {
        let mut messages = vec![ModelMessage::system("system"), ModelMessage::user("prompt")];
        for round in 0..rounds {
            messages.push(ModelMessage::assistant(format!("assistant {round}")));
            for call in 0..(round % 3 + 1) {
                messages.push(ModelMessage::tool(
                    format!("call_{round}_{call}"),
                    format!("result {round} {call}"),
                ));
            }
        }
        for max_messages in [3, 4, 5, 10, 24, 40] {
            assert_eq!(
                bounded_messages(&messages, max_messages, &NotesWindow::none()),
                bounded_messages_before_65(&messages, max_messages),
                "rounds={rounds} max={max_messages}"
            );
        }
    }
}

fn notes_window<'a>(notes: &'a str, calls: &'a [RecordedCall], carried: bool) -> NotesWindow<'a> {
    NotesWindow {
        notes,
        carried,
        calls,
        enabled: true,
        notes_limit: 8_000,
        notice_cap: ELISION_NOTICE_LINE_CAP,
        warning_cap: PRE_CUT_LINE_CAP,
    }
}

fn call(index: usize, line: &str) -> RecordedCall {
    RecordedCall {
        round: index as u32 + 1,
        message_index: 2 + index * 2,
        line: line.to_string(),
        key: None,
        writes: false,
        compacted: false,
    }
}

#[test]
fn the_notes_block_sits_between_the_seed_and_the_notice() {
    let mut messages = vec![ModelMessage::system("system"), ModelMessage::user("prompt")];
    for index in 0..10 {
        messages.push(ModelMessage::assistant(format!("assistant {index}")));
        messages.push(ModelMessage::tool(
            format!("call_{index}"),
            format!("result {index}"),
        ));
    }
    let calls: Vec<RecordedCall> = (0..10)
        .map(|index| call(index, &format!("read_file src/{index}.rs")))
        .collect();
    let bounded = bounded_messages(&messages, 10, &notes_window("a finding", &calls, false));

    assert_eq!(bounded[0].role, "system");
    assert_eq!(bounded[1].role, "user");
    assert!(
        bounded[2].content.starts_with(NOTES_HEADER),
        "the notes are the first block after the seed: {}",
        bounded[2].content
    );
    assert!(
        bounded[3].content.contains("elided"),
        "the notice follows the notes: {}",
        bounded[3].content
    );
    assert!(
        bounded
            .last()
            .unwrap()
            .content
            .contains("The window is full."),
        "the warning is last: {}",
        bounded.last().unwrap().content
    );
}

#[test]
fn the_notes_block_does_not_shrink_the_round_budget() {
    let mut messages = vec![ModelMessage::system("system"), ModelMessage::user("prompt")];
    for index in 0..10 {
        messages.push(ModelMessage::assistant(format!("assistant {index}")));
        messages.push(ModelMessage::tool(
            format!("call_{index}"),
            format!("result {index}"),
        ));
    }
    let calls: Vec<RecordedCall> = (0..10)
        .map(|index| call(index, &format!("read_file src/{index}.rs")))
        .collect();
    for max_messages in [5, 10, 24] {
        let without = bounded_messages(&messages, max_messages, &NotesWindow::none());
        let with = bounded_messages(
            &messages,
            max_messages,
            &notes_window("a finding", &calls, false),
        );
        // One notes block and one warning at most; the retained rounds
        // are exactly the same messages.
        let without_rounds: Vec<&ModelMessage> = without
            .iter()
            .filter(|message| message.role == "assistant" || message.role == "tool")
            .collect();
        let with_rounds: Vec<&ModelMessage> = with
            .iter()
            .filter(|message| message.role == "assistant" || message.role == "tool")
            .collect();
        assert_eq!(
            without_rounds, with_rounds,
            "max={max_messages}: the round budget must be unchanged"
        );
    }
}

#[test]
fn the_carried_header_is_used_while_carried_text_remains() {
    let messages = vec![ModelMessage::system("system"), ModelMessage::user("prompt")];
    let bounded = bounded_messages(&messages, 24, &notes_window("a finding", &[], true));
    assert!(
        bounded[2].content.starts_with(CARRIED_NOTES_HEADER),
        "{}",
        bounded[2].content
    );
}

// -----------------------------------------------------------------
// Requirement 8's concurrent batch, at the level the turn calls it
// -----------------------------------------------------------------

fn batch_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("damaian-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(dir.join("src/b.rs"), "pub fn beta() {}\n").unwrap();
    dir
}

fn batch_engine(repo: &Path) -> crate::workspace_engine::WorkspaceEngine {
    crate::workspace_engine::WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    })
}

fn batch_session() -> Session {
    Session {
        id: "session-1".to_string(),
        repository_id: "repo-1".to_string(),
        title: "batch".to_string(),
        created_at_ms: 0,
        updated_at_ms: 0,
        summary: String::new(),
        origin: "user".to_string(),
        archived: false,
    }
}

fn batch_task() -> Task {
    Task {
        id: "task-1".to_string(),
        session_id: "session-1".to_string(),
        status: TaskStatus::PreparingContext,
        user_prompt: "batch".to_string(),
        model_provider: "mock".to_string(),
        model_name: "mock".to_string(),
        created_at_ms: 0,
        completed_at_ms: None,
    }
}

fn two_reads() -> Vec<DecodedCall> {
    vec![
        (
            None,
            ToolAction::ReadFile {
                path: "src/a.rs".to_string(),
                range: None,
                worktree: None,
            },
        ),
        (
            None,
            ToolAction::ReadFile {
                path: "src/b.rs".to_string(),
                range: None,
                worktree: None,
            },
        ),
    ]
}

/// Requirement 8: the concurrent batch must be byte-identical to the same
/// calls dispatched one after another, in the model's order. This is the
/// "comparing a concurrent run against a sequential one" half of the
/// acceptance criterion, and it is exact rather than timing-based.
#[test]
fn a_read_only_batch_matches_a_sequential_dispatch() {
    let repo = batch_repo("batch-equivalence");
    let engine = batch_engine(&repo);
    let orchestrator = &engine.chat_orchestrator;
    let session = batch_session();
    let task = batch_task();
    let actions = two_reads();

    let sequential: Vec<_> = actions
        .iter()
        .map(|(_, action)| orchestrator.dispatch_read_only_action(&repo, &session, &task, action))
        .collect();
    let concurrent =
        orchestrator.run_read_only_batch(&repo, &session, &task, &actions, &CancelToken::new());

    assert_eq!(
        sequential, concurrent,
        "concurrent results must equal sequential ones, in the same order"
    );
}

/// Requirement 8: a stop cancels every call in the batch that has not
/// started. Deterministic here because the token is set before the batch
/// runs, so every thread checks it first; no `file_read` audit entry may be
/// written. An already in-flight read is still not interruptible, which the
/// method's comment states rather than hides.
#[test]
fn a_cancelled_batch_dispatches_nothing() {
    let repo = batch_repo("batch-cancelled");
    let engine = batch_engine(&repo);
    let orchestrator = &engine.chat_orchestrator;
    let session = batch_session();
    let task = batch_task();
    let actions = two_reads();

    let cancel = CancelToken::new();
    cancel.cancel();
    let results = orchestrator.run_read_only_batch(&repo, &session, &task, &actions, &cancel);

    assert_eq!(
        results.len(),
        2,
        "one result slot per call, so the turn can abandon them"
    );
    assert!(
        results
            .iter()
            .all(|(_, _, outcome)| *outcome == ActionOutcome::Cancelled),
        "every call must report cancelled: {results:?}"
    );
    let audit = fs::read_to_string(repo.join(".damaian/audit/events.jsonl")).unwrap_or_default();
    assert!(
        !audit.contains("file_read"),
        "nothing may be read once the batch is cancelled: {audit}"
    );
}

/// The timing half of requirement 8's first clause. It is `#[ignore]`d
/// because a wall-clock comparison is exactly the kind of assertion that
/// flakes under load, and the exact sequential-equals-concurrent test above
/// is the real gate. Run it by hand:
///
/// ```sh
/// cargo test -p workspace-engine --lib -- --ignored --exact \
///   chat::evidence_tests::a_concurrent_batch_is_faster_than_a_sequential_search
/// ```
#[test]
#[ignore]
fn a_concurrent_batch_is_faster_than_a_sequential_search() {
    let repo = batch_repo("batch-timing");
    for index in 0..2000 {
        fs::write(
            repo.join("src").join(format!("f{index}.rs")),
            "let needle = 1;\n",
        )
        .unwrap();
    }
    let engine = batch_engine(&repo);
    let orchestrator = &engine.chat_orchestrator;
    let session = batch_session();
    let task = batch_task();
    let actions: Vec<DecodedCall> = (0..4)
        .map(|_| {
            (
                None,
                ToolAction::SearchContent {
                    pattern: "needle".to_string(),
                    path_glob: None,
                    max_matches: None,
                    worktree: None,
                },
            )
        })
        .collect();

    let sequential_start = std::time::Instant::now();
    for (_, action) in &actions {
        orchestrator.dispatch_read_only_action(&repo, &session, &task, action);
    }
    let sequential = sequential_start.elapsed();

    let concurrent_start = std::time::Instant::now();
    orchestrator.run_read_only_batch(&repo, &session, &task, &actions, &CancelToken::new());
    let concurrent = concurrent_start.elapsed();

    eprintln!("sequential {sequential:?}, concurrent {concurrent:?}");
    assert!(
        concurrent < sequential,
        "four concurrent searches should beat four sequential ones: \
             concurrent {concurrent:?} vs sequential {sequential:?}"
    );
}
