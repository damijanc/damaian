use super::*;

#[test]
fn a_pending_turn_written_before_the_completed_tool_tracker_still_loads() {
    let pending = PendingChatTurn {
        proposal_id: "proposal".to_string(),
        session: Session {
            id: "session".to_string(),
            repository_id: "repository".to_string(),
            title: "title".to_string(),
            created_at_ms: 0,
            updated_at_ms: 0,
            summary: String::new(),
            origin: "user".to_string(),
            archived: false,
        },
        task: Task {
            id: "task".to_string(),
            session_id: "session".to_string(),
            status: TaskStatus::WaitingForApproval,
            user_prompt: "prompt".to_string(),
            model_provider: "mock".to_string(),
            model_name: "mock".to_string(),
            created_at_ms: 0,
            completed_at_ms: None,
        },
        repository_root: "/tmp".to_string(),
        context_files: Vec::new(),
        round: 0,
        exploration_rounds: 0,
        turn_notes: String::new(),
        turn_notes_carried: false,
        calls: Vec::new(),
        completed_tool_rounds: Vec::new(),
        step_evidence: Vec::new(),
        plan_reminder_sent: false,
        messages: Vec::new(),
        matched_tool_call: None,
        last_content: String::new(),
        turn_options: ChatTurnOptions::default(),
        reasoning_content: None,
        round_calls_fed_back: false,
        mcp_call: None,
        web_diagnostic_call: None,
        plan_review: None,
        provider_fallback: None,
        round_budget: None,
        segment: None,
        question: None,
        check_batch: None,
        failure_baseline: None,
        fetch_call: None,
        context_review: None,
    };
    let mut value = serde_json::to_value(&pending).expect("a pending turn serializes");
    value
        .as_object_mut()
        .expect("a pending turn is an object")
        .remove("completed_tool_rounds");
    let loaded: PendingChatTurn =
        serde_json::from_value(value).expect("a historical pending turn must load");
    assert!(loaded.completed_tool_rounds.is_empty());
}
