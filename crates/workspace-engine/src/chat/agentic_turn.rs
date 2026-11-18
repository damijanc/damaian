use super::*;

impl ChatOrchestrator {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_agentic_turn(
        &self,
        repository_root: &Path,
        session: Session,
        mut task: Task,
        context_files: Vec<String>,
        mut messages: Vec<ModelMessage>,
        mut action_rounds: u32,
        mut exploration_rounds: u32,
        mut turn_notes: TurnNotesState,
        // Completed native tool rounds, carried across a pause so the next
        // request after a reload compacts from the same typed facts.
        mut completed_tool_rounds: CompletedToolTracker,
        // Evidence accrued before this resume, carried in the pending turn.
        // The turn starts empty; every pause seeds it (spec 66 §5.3).
        mut step_evidence: Vec<crate::plan::Evidence>,
        // Whether this turn already sent its one plan reminder. Carried so a
        // resume cannot remind twice (spec 66 §5.2).
        mut plan_reminder_sent: bool,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
        turn_options: ChatTurnOptions,
        // The open segment a paused turn left behind, validated against the
        // durable `segment_started` record before it is reused (spec 103 Task 5).
        segment_pause: Option<&TaskSegmentPauseState>,
    ) -> Result<ChatTurnResult> {
        // Every turn and inherited continuation gets the durable exclusions as
        // a reminder. This is never the enforcement boundary: proposal and
        // apply resolve the same record themselves (spec 98 §2.7). A resumed
        // turn may carry a reminder from its paused messages, so this replaces
        // that block rather than appending a second one. A read failure fails
        // closed: running without the reminder would silently drop a
        // user-owned constraint.
        let active_protected = self
            .session_store
            .task_protected_paths(&session.id, &task.id)?;
        // The worker writes this as its own progress line: the resolved typed
        // record, with the task id, so a board can show the active exclusions
        // without re-deriving them from a prompt or message text.
        sink.progress(TurnProgress::ProtectedPaths(active_protected.clone()));
        apply_protected_paths_reminder(&mut messages, &active_protected, &self.scanner);
        // Per-turn MCP runtime: connects lazily, caches tool lists and
        // connections for this turn, and tears everything down on drop.
        let mut mcp = self.build_mcp_runtime(&session.id);
        let browser_mcp_server_ids = self.browser_diagnostic_mcp_server_ids();
        // Layer 1 (`proposal.md` §5.2): the mode is captured once, here, before
        // any tool is offered, and never re-read later in the turn — a mode
        // change mid-turn must not take effect until the next turn (§5.4).
        // Every definition is filtered through the one permission matrix;
        // withholding any part of it here would be the second copy of the
        // matrix the Global Constraints forbid.
        let mode = self.session_store.session_mode(&session.id);
        // The profile's capabilities are captured with it: this engine loaded
        // config when the turn began (spec 31 `context.md` §6).
        let capabilities = self.config.profile_capabilities();
        // Spec 50 `context.md` §4: `ask_user` exists only where someone can
        // answer it and the cap allows at least one question. A cap of 0 means
        // "not offered", never "offered and always refused". Decided once, so
        // the offer and Layer 3's refusal below cannot disagree.
        let ask_user_offered = self.config.supports_native_tools()
            && turn_options.question_route()
            && self.config.agent_max_questions_per_turn > 0
            && permits(
                mode,
                &capabilities,
                &ToolAction::AskUser(AskUserInput {
                    question: String::new(),
                    options: Vec::new(),
                    allow_free_text: false,
                }),
                None,
                None,
            )
            .is_allowed();
        // Spec 51 §5.3 constraint 1: the first fetch of a host asks the user.
        // A turn with nobody to answer can only use a host the user already
        // approved, so the tool is withheld where `ask_user` is withheld. A cap
        // of 0 means "not offered", never "offered and always refused".
        let fetch_url_offered = self.config.supports_native_tools()
            && turn_options.synchronous_user_present
            && self.config.web_fetch_max_per_turn > 0
            && permits(
                mode,
                &capabilities,
                &ToolAction::FetchUrl { url: String::new() },
                None,
                None,
            )
            .is_allowed();
        let native_tools = self.config.supports_native_tools().then(|| {
            // `permits` decides per `ToolAction` variant, so each definition
            // is asked about a placeholder action of its kind — only the
            // variant matters for every class except `run_command` and MCP,
            // which get their own context below.
            let offered =
                |action: &ToolAction| permits(mode, &capabilities, action, None, None).is_allowed();

            // Whether the `run_command` *definition* is offered cannot depend
            // on one specific command, only on whether *any* command could pass
            // in this mode and profile, so ask about the most permissive
            // classification there is. Going through `permits` rather than
            // hand-coding `mode != Ask` keeps the matrix in one place, and
            // `command_access=none` withholds the tool the same way. The text
            // must be genuinely read-only, not empty: the matrix checks the
            // command itself, not just its risk, so an allowlist cannot widen
            // a mode.
            let permissive_command = CommandClassification {
                command: "pwd".to_string(),
                risk: CommandRisk::Low,
                blocked: false,
                requires_approval: false,
                reasons: Vec::new(),
                causes: Vec::new(),
                expected_effects: String::new(),
                may_use_network: false,
            };
            let run_command_permitted = permits(
                mode,
                &capabilities,
                &ToolAction::Command(CommandRequest {
                    command: String::new(),
                    reason: String::new(),
                    working_directory: None,
                }),
                Some(&permissive_command),
                None,
            )
            .is_allowed();

            let mut tools = Vec::new();
            if run_command_permitted {
                tools.push(run_command_tool_definition(&self.config.shell));
            }
            if offered(&ToolAction::ProposePatch(GeneratedEdit {
                summary: String::new(),
                changes: Vec::new(),
            })) {
                tools.push(propose_patch_tool_definition());
            }
            if offered(&ToolAction::ProposePlan(Vec::new())) {
                tools.push(propose_plan_tool_definition());
                tools.push(complete_step_tool_definition());
            }
            if ask_user_offered {
                tools.push(ask_user_tool_definition());
            }
            if fetch_url_offered {
                tools.push(fetch_url_tool_definition());
            }
            if offered(&ToolAction::ReadFile {
                path: String::new(),
                range: None,
                worktree: None,
            }) {
                tools.push(read_file_tool_definition());
            }
            if offered(&ToolAction::ListDirectory {
                dir: None,
                depth: None,
                worktree: None,
            }) {
                tools.push(list_directory_tool_definition());
            }
            if offered(&ToolAction::SearchContent {
                pattern: String::new(),
                path_glob: None,
                max_matches: None,
                worktree: None,
            }) {
                tools.push(search_content_tool_definition());
            }
            if offered(&ToolAction::EditFile {
                summary: String::new(),
                edits: Vec::new(),
            }) {
                tools.push(edit_file_tool_definition());
            }
            if offered(&ToolAction::SearchCodebase {
                query: String::new(),
                semantic: false,
                limit: 0,
            }) {
                tools.push(search_codebase_tool_definition());
            }
            if offered(&ToolAction::ReadGitStatus { worktree: None }) {
                tools.push(read_git_status_tool_definition());
            }
            if offered(&ToolAction::ReadGitDiff {
                staged: false,
                worktree: None,
                paths: Vec::new(),
                stat: false,
                base: None,
            }) {
                tools.push(read_git_diff_tool_definition());
            }
            if offered(&ToolAction::ListWorktrees { base: None }) {
                tools.push(list_worktrees_tool_definition());
            }
            if offered(&ToolAction::ReadGitLog {
                worktree: None,
                base: None,
                max_count: None,
            }) {
                tools.push(read_git_log_tool_definition());
            }
            // Spec 65 §5.1: offered wherever the read tools are and the bound
            // is above zero. Zero means "not offered", never "offered and
            // always refused".
            if self.config.agent_max_turn_notes_chars > 0
                && offered(&ToolAction::TurnNotes {
                    text: String::new(),
                    replace: false,
                })
            {
                tools.push(turn_notes_tool_definition());
            }
            if offered(&ToolAction::ProposeMemory {
                statement: String::new(),
                category: MemoryCategory::Fact,
                scope: None,
                source: None,
                evidence_path: None,
            }) {
                tools.push(propose_memory_tool_definition());
            }
            if self.web_diagnostics_runner.is_some()
                && offered(&ToolAction::WebDiagnostic(WebDiagnosticCall {
                    kind: WebDiagnosticKind::Inspect,
                    url: String::new(),
                    arguments_json: "{}".to_string(),
                    session_id: None,
                    task_id: None,
                }))
            {
                tools.push(inspect_web_page_tool_definition());
                tools.push(run_web_scenario_tool_definition());
            }
            // Best-effort: discovered MCP tools are namespaced
            // (mcp__<server>__<tool>) and appended; a server that fails to
            // connect is simply skipped. Browser-diagnostic servers are
            // dropped, and the rest are filtered by mode — a per-tool lookup,
            // because the flattened definition carries no read-only hint.
            tools.extend(mcp.tool_definitions().into_iter().filter(|tool| {
                let Some((server_id, tool_name)) = parse_namespaced_tool_name(&tool.name) else {
                    return true;
                };
                if browser_mcp_server_ids.contains(&server_id) {
                    return false;
                }
                let hint = mcp.tool_read_only_hint(&server_id, &tool_name);
                permits(
                    mode,
                    &capabilities,
                    &ToolAction::McpCall {
                        server_id,
                        tool_name,
                        arguments_json: String::new(),
                    },
                    None,
                    hint,
                )
                .is_allowed()
            }));
            tools
        });

        // Whatever the model has produced so far, carried across rounds so a
        // stop between them still has an answer to preserve.
        let mut partial_response = String::new();
        // Whether any model call in this turn has been accounted for, so a
        // stop between rounds does not record a zero on top of real spending.
        let mut recorded_any_usage = false;
        let mut web_debug_mode = prompt_enters_web_debug_mode(&task.user_prompt);
        let mut failed_browser_calls = HashMap::new();
        // The turn's plan, once the model has proposed one, and the evidence
        // accrued since the current step started.
        //
        // Held here rather than re-read from the log each round because the
        // *accrual* has no home on disk until the step closes: evidence belongs
        // to a step, and which step is open is a fact about this turn. The plan
        // itself is written through as it changes, so a crash loses at most the
        // evidence of the step that was still running — unless the turn was
        // paused, because a paused turn carries it in `PendingChatTurn` and the
        // resumed loop seeds it from there (spec 66 §5.3).
        // Seeded from the log rather than left empty: a resumed turn already
        // has a plan (see `carry_plan_from_the_previous_turn`), and starting this
        // at `None` would make `complete_step` report "there is no plan" while
        // the panel showed one.
        let mut plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)
            .unwrap_or_default();
        // A turn that arrives with a plan already in hand — resumed after a
        // token stop, or after the user approved or revised it — shows it
        // before doing any work. Without this the panel would stay empty until
        // the first `complete_step`, which is the longest stretch of the turn.
        if let Some(current) = plan.as_ref() {
            sink.plan(current);
        }
        // Read from the log for the same reason the plan is: the approval is a
        // decision the user took, and a turn resumed after a restart must not
        // ask for it again. It is only ever read here — the gate below is the
        // single place it decides anything.
        let mut plan_approved = self
            .session_store
            .read_plan_approved(&session.id, &task.id)
            .unwrap_or(false);
        // Spec 103 R17: a worker without an approved task budget still gets the
        // bounded pre-plan phase. Worker identity is durable, never a prompt or
        // a UI label. An inline session keeps today's round/Continue behaviour
        // exactly, because this flag is false for it.
        let worker_session = self.session_store.worker_record(&session.id)?.is_some();
        let preplan_plan_required = if worker_session {
            self.session_store
                .read_messages(&session.id)
                .unwrap_or_default()
                .iter()
                .any(|message| {
                    message.task_id.as_deref() == Some(task.id.as_str())
                        && message.kind.as_deref() == Some(PREPLAN_PLAN_REQUIRED_MESSAGE_KIND)
                })
        } else {
            false
        };
        // A turn resumed after the instruction was written gets exactly one more
        // model call, not another exploration pass. A turn that sends it now
        // sets this for the call that follows.
        let mut preplan_final_attempt = preplan_plan_required;

        // Extra rounds a turn may take past its configured segment: granted by
        // a token ceiling (requirement 6) or by a round-budget *Continue*
        // (spec 61 §5.2), which seeds this from the pending turn's running
        // `rounds_granted`. Bounded by the absolute round cap so a provider
        // that reports no usage cannot loop forever.
        let mut round_budget_extension: u32 = turn_options.round_budget_extension;
        // The per-turn read allowance (spec 59 §5.2). It is fixed for the turn
        // and never extended by a continuation, so it is computed once here.
        let mut max_exploration_rounds = self.default_exploration_round_limit();
        // A notes record that survived without its call list (a hand-written or
        // partially written pending file) rebuilds the list from the messages
        // it saved, so the notice and the repeat rule still work.
        if turn_notes.calls.is_empty() && !turn_notes.text.is_empty() {
            turn_notes.calls = self.rebuild_call_list(repository_root, &messages);
        }
        // The turn's already-dispatched calls (spec 59 §5.3, spec 65 §5.6),
        // keyed by the canonical form below. A resumed turn rebuilds the set
        // from the persisted list, taking entries since the last round that
        // wrote — a write changes what a read reports, so reads before it are
        // verification history rather than a loop. A turn written before spec
        // 65 has no list and starts empty, as #59 accepted.
        let mut seen_read_calls: HashSet<Vec<String>> = {
            let since_write = turn_notes
                .calls
                .iter()
                .rposition(|call| call.writes)
                .map(|index| index + 1)
                .unwrap_or(0);
            turn_notes.calls[since_write..]
                .iter()
                // A read whose result became a manifest is repeatable again,
                // including after a reload (spec 103 Task 4).
                .filter(|call| !call.compacted)
                .filter_map(|call| call.key.clone())
                .collect()
        };
        // The answer that triggered the one plan reminder, if one was sent. On a
        // true completion it is the turn's response, and the post-reminder
        // answer follows it as a second message (spec 66 `context.md` D4). A
        // pause or budget stop after the reminder keeps its own response.
        let mut pre_reminder_answer: Option<String> = None;
        // Automatic worker segments (spec 103 Task 5). `None` for every inline
        // task, every worker without an approved budget and every historical
        // pending turn, which keeps the explicit round-budget path exactly.
        let (mut segment_tracker, segment_started_new) = match self.task_segment_run_state(
            &session,
            &task,
            action_rounds,
            exploration_rounds,
            plan.as_ref(),
            &step_evidence,
            &completed_tool_rounds,
            &turn_notes,
            self.tool_round_limit(web_debug_mode) + round_budget_extension,
            max_exploration_rounds,
            segment_pause,
        )? {
            Some((tracker, started_new)) => (Some(tracker), started_new),
            None => (None, false),
        };
        if segment_started_new {
            // The planning rounds before the budget approval are the new
            // segment's baseline, not its first counters.
            action_rounds = 0;
            exploration_rounds = 0;
        }
        if let Some(tracker) = &segment_tracker {
            max_exploration_rounds = max_exploration_rounds.min(tracker.exploration_allowance);
        }
        // Spec 107 D3 (A): an ordinary Code-mode turn crosses its round
        // checkpoint the way a worker segment does — while the progress digest
        // advances and the default task budget allows — instead of pausing on
        // **Continue** every segment. Ask, Plan and Review keep the pause, and
        // a worker keeps its own approved-budget tracker above. A repair turn
        // keeps its pause too: it is bounded by its attempt limit and the
        // stagnation rules (spec 102), not by task progress. A configured token
        // ceiling already continues past the checkpoint under its own bound
        // (spec 47), so it keeps that path.
        let mut ordinary_continuation = (!worker_session
            && segment_tracker.is_none()
            && mode == SessionMode::Code
            && !turn_options.verification_turn
            && self.config.agent_max_task_tokens.is_none())
        .then(|| {
            OrdinaryContinuation::new(
                self.config.task_budget_policy().default,
                self.segment_progress_digest(
                    plan.as_ref(),
                    &step_evidence,
                    &completed_tool_rounds,
                    &turn_notes,
                ),
            )
        });
        // A request prepared for an automatic boundary is reused for the very
        // next model call, so one threshold crossing records one compaction
        // attempt rather than preparing the same request twice.
        let mut boundary_request: Option<PreparedTurnRequest> = None;
        let (mut final_run, mut response, proposals, stop_reason) = loop {
            // Spec 57 §5.4, in its order: a pause holds here, before the
            // cancel check, so a stop that arrives while paused ends the turn
            // through the same path as any other stop.
            if let Some(steering) = sink.steering
                && steering.is_paused()
                && !sink.cancel.is_cancelled()
            {
                let running = task.status.clone();
                task = self
                    .session_store
                    .update_task_status(&task, TaskStatus::Paused, None)?;
                sink.progress(TurnProgress::Paused);
                if steering.wait_while_paused(sink.cancel) {
                    task = self
                        .session_store
                        .update_task_status(&task, running, None)?;
                    sink.progress(TurnProgress::Resumed);
                }
            }

            // Checked before each round rather than only mid-stream: stopping
            // here is what saves a whole model call, and it is the only point
            // that catches a stop arriving during context assembly or a tool.
            if sink.cancel.is_cancelled() {
                let cancelled_run = ModelRun::cancelled_before_start(
                    &self.config.model_provider,
                    &self.config.model_name,
                );
                // Only when nothing was called: a stop between rounds already
                // has its earlier rounds accounted, and adding a zero on top
                // would claim a model call that never happened.
                if !recorded_any_usage {
                    self.session_store.record_task_usage(
                        &task,
                        Some(CallModel::configured(&self.config)),
                        &cancelled_run.run_id,
                        None,
                        TokenUsage::measured_zero(),
                        None,
                        None,
                    )?;
                }
                return self.finish_cancelled_turn(
                    repository_root,
                    session,
                    task,
                    context_files,
                    &partial_response,
                    &turn_notes,
                    cancelled_run,
                    plan,
                    step_evidence,
                    plan_reminder_sent,
                    sink,
                );
            }

            // Notes join the conversation as ordinary user messages, before
            // the request they should reach. The log keeps the user's text as
            // written, like every user message; what goes to the model is
            // redacted (spec 57 correction C3).
            if let Some(steering) = sink.steering {
                for note in steering.drain_notes() {
                    let message = self.session_store.append_message(
                        &session.id,
                        Some(&task.id),
                        "user",
                        &note,
                    )?;
                    self.session_store
                        .record_steering_note(&session.id, &message.id)?;
                    messages.push(ModelMessage::user(self.scanner.redact(&note).text));
                    sink.progress(TurnProgress::NoteAdded {
                        message_id: message.id,
                    });
                    // The prepared request predates this note, so it is no
                    // longer the exact next request.
                    boundary_request = None;
                }
            }

            // Before the request is built, not after the response arrives.
            // `force_final` — the round budget's shape — deliberately spends
            // one more model call on crossing, which is right for a bound on
            // rounds and backwards for a bound on money: context grows across
            // rounds, so that call is the most expensive of the turn. A ceiling
            // whose enforcement action is to spend more than the ceiling is not
            // a ceiling. `context.md` §3.3.
            if let Some((ceiling, spent)) = self.token_ceiling_spend(&session.id, &task.id)? {
                // Absent usage is zero here, not "unknown". `read_task_usage`
                // omits a task with no events because "not recorded" and "used
                // nothing" differ for a *report*; for a *ceiling* nothing spent
                // is nothing spent, and treating absence as unenforceable would
                // disable the bound for the first call of every turn — the only
                // call some turns make. `context.md` §3.9.
                //
                // An `Estimated` total is checked like any other: declining to
                // enforce on one would make the ceiling inoperative for every
                // provider that does not report usage (§5.4).
                if spent >= ceiling {
                    let response = token_budget_exhausted_response(ceiling, spent, plan.as_ref());
                    let mut stopped = ModelRun::cancelled_before_start(
                        &self.config.model_provider,
                        &self.config.model_name,
                    );
                    stopped.content = response.clone();
                    break (
                        stopped,
                        response,
                        TurnProposals::default(),
                        StopReason::TokenBudget,
                    );
                }
            }

            // A budgeted task reads its durable usage once for this iteration.
            // The approved output cap is applied before `build_turn_request`
            // constructs and measures the request, so Task 4 sees exactly the
            // shape the adapter will send; the post-preparation check then
            // measures that same object without changing it.
            let task_usage = if segment_tracker.is_some() {
                // The approved budget covers the whole continuation chain:
                // a verification, repair or continuation is a new task that
                // carries it (spec 106 Task 4).
                self.session_store
                    .read_budget_usage(&session.id, &task.id)?
            } else {
                None
            };

            let mut max_rounds = (self.tool_round_limit(web_debug_mode) + round_budget_extension)
                .min(ABSOLUTE_TURN_ROUND_CAP);
            if let Some(tracker) = &segment_tracker {
                max_rounds = max_rounds.min(tracker.action_allowance);
            }
            if let Some(ordinary) = &ordinary_continuation
                && let Some(allowance) = ordinary.action_allowance
            {
                max_rounds = max_rounds.min(allowance);
            }
            let force_final = action_rounds >= max_rounds;
            // Spec 61 §5.1: at the round budget, with no token ceiling to
            // continue under, pause before the forced final call instead of
            // spending it on text nobody sees. The forced call is made only
            // when the user answers *Answer now*. #47's continuation still
            // applies when a ceiling is set and the absolute cap is not yet
            // reached, so the forced call proceeds on that path.
            //
            // Spec 103 Task 5: a worker whose plan carries an approved task
            // budget crosses this checkpoint internally instead, provided every
            // approved total still allows progress, the engine-owned progress
            // digest advanced, and Task 4 can fit the exact next request. The
            // segment is committed only after that request is prepared, so one
            // threshold crossing records one compaction attempt.
            if force_final && !turn_options.round_budget_answer_now {
                // A pre-plan tracker has no approved task budget to spend once
                // a plan exists. The pre-plan phase is over at that point, so
                // it cannot decide the boundary: fall back to the ordinary
                // unbudgeted path (spec 103 requirement 4) rather than charge
                // the spent pre-plan total as an approved-budget stop.
                if plan.is_some()
                    && segment_tracker
                        .as_ref()
                        .is_some_and(|tracker| tracker.preplan)
                {
                    segment_tracker = None;
                }
                // Spec 103 R17: a worker with no plan and no approved budget
                // starts its bounded pre-plan phase at its first round
                // boundary. Starting lazily keeps a worker that proposes a plan
                // immediately on today's path, with no extra segment record.
                if segment_tracker.is_none() && worker_session && plan.is_none() {
                    let tracker = self.start_preplan_segment(
                        &session,
                        &task,
                        action_rounds,
                        exploration_rounds,
                        plan.as_ref(),
                        &step_evidence,
                        &completed_tool_rounds,
                        &turn_notes,
                        self.tool_round_limit(web_debug_mode) + round_budget_extension,
                        self.default_exploration_round_limit(),
                    )?;
                    max_exploration_rounds = self
                        .default_exploration_round_limit()
                        .min(tracker.exploration_allowance);
                    action_rounds = 0;
                    exploration_rounds = 0;
                    round_budget_extension = 0;
                    segment_tracker = Some(tracker);
                    continue;
                }
                if let Some(tracker) = segment_tracker.as_mut() {
                    let totals = SegmentTotals {
                        action_rounds: tracker.total_action_rounds(action_rounds),
                        exploration_rounds: tracker.total_exploration_rounds(exploration_rounds),
                    };
                    let usage = task_usage.as_ref();
                    let digest = self.segment_progress_digest(
                        plan.as_ref(),
                        &step_evidence,
                        &completed_tool_rounds,
                        &turn_notes,
                    );
                    // Pre-plan exhaustion is not an approved-budget stop: it
                    // asks for the plan the phase exists to produce. The
                    // instruction is durable and the next model call is the
                    // final chance; a worker that still has no plan settles the
                    // typed outcome below.
                    if tracker.preplan && plan.is_none() {
                        let exhausted = totals.exploration_rounds
                            >= tracker.budget.exploration_rounds
                            || totals.action_rounds >= tracker.budget.action_rounds;
                        if exhausted {
                            let last_segment = self.live_segment_snapshot(
                                tracker,
                                action_rounds,
                                exploration_rounds,
                                usage,
                                digest.clone(),
                            );
                            if preplan_final_attempt {
                                let outcome = self.preplan_exhausted_outcome(
                                    &session,
                                    &task,
                                    &tracker.budget,
                                    totals,
                                    usage,
                                    Some(last_segment),
                                )?;
                                let (run, response, stop) = self.task_terminal_stop(outcome)?;
                                break (run, response, TurnProposals::default(), stop);
                            }
                            self.session_store.append_message_with_kind(
                                &session.id,
                                Some(&task.id),
                                "user",
                                PREPLAN_PLAN_REQUIRED_NOTE,
                                Some(PREPLAN_PLAN_REQUIRED_MESSAGE_KIND),
                            )?;
                            messages
                                .push(ModelMessage::user(PREPLAN_PLAN_REQUIRED_NOTE.to_string()));
                            self.audit_log.record(
                                "preplan_plan_required",
                                &[
                                    ("actor", "system".to_string()),
                                    ("sessionId", session.id.clone()),
                                    ("taskId", task.id.clone()),
                                    ("segment", tracker.segment.to_string()),
                                    ("totalActionRounds", totals.action_rounds.to_string()),
                                    (
                                        "totalExplorationRounds",
                                        totals.exploration_rounds.to_string(),
                                    ),
                                    (
                                        "explorationLimit",
                                        tracker.budget.exploration_rounds.to_string(),
                                    ),
                                ],
                            )?;
                            preplan_final_attempt = true;
                            // One ordinary segment is enough to call
                            // `propose_plan`, which asks for no approval and
                            // runs no tool. The tracker's own allowance was
                            // sized for the old segment and would otherwise keep
                            // `force_final` true, stopping before the chance to
                            // answer.
                            round_budget_extension += self.default_tool_round_limit();
                            tracker.action_allowance = tracker
                                .action_allowance
                                .max(action_rounds.saturating_add(self.default_tool_round_limit()));
                            continue;
                        }
                    }
                    let decision = decide_segment_continuation(&SegmentContinuationInput {
                        budget: &tracker.budget,
                        totals,
                        usage,
                        normal_action_rounds: self.tool_round_limit(web_debug_mode)
                            + round_budget_extension,
                        normal_exploration_rounds: self.default_exploration_round_limit(),
                        progress_advanced: digest != tracker.start_digest,
                    });
                    // Every stop below describes the segment this turn actually
                    // ran, not its durable zeroed `segment_started` record.
                    let last_segment = self.live_segment_snapshot(
                        tracker,
                        action_rounds,
                        exploration_rounds,
                        usage,
                        digest.clone(),
                    );
                    match decision {
                        SegmentContinuationDecision::Continue {
                            action_rounds: action_allowance,
                            exploration_rounds: exploration_allowance,
                        } => {
                            // The approved output cap is part of the request
                            // bytes, so apply it before Task 4 prepares and
                            // measures the request. The measured shape is then
                            // exactly what the adapter sends.
                            let output_cap = match task_request_output_cap(
                                &tracker.budget,
                                usage,
                                self.config.max_output_tokens(),
                            ) {
                                Ok(cap) => cap,
                                Err(failure) => {
                                    let outcome = self.task_authority_outcome(
                                        &session,
                                        &task,
                                        &tracker.budget,
                                        usage,
                                        &failure,
                                        Some(last_segment.clone()),
                                    )?;
                                    let (run, response, stop) = self.task_terminal_stop(outcome)?;
                                    break (run, response, TurnProposals::default(), stop);
                                }
                            };
                            // Prepared before the boundary is written: a
                            // request that cannot fit must leave the segment
                            // open and send nothing.
                            let PreparedTurnRequest {
                                request,
                                compacted_call_ids,
                            } = match self.build_turn_request(
                                repository_root,
                                &session,
                                &task,
                                &messages,
                                &turn_notes,
                                &completed_tool_rounds,
                                plan.as_ref(),
                                &step_evidence,
                                &active_protected,
                                native_tools.clone(),
                                Some(output_cap),
                            )? {
                                TurnRequestDecision::Prepared(prepared) => prepared,
                                TurnRequestDecision::CannotFit(attempt) => {
                                    // No boundary is written and no request is
                                    // sent: the segment stays open and the task
                                    // settles with its own typed outcome.
                                    let outcome = self.cannot_fit_outcome(
                                        &session,
                                        &task,
                                        &attempt,
                                        usage,
                                        Some(last_segment.clone()),
                                    )?;
                                    let (run, response, stop) = self.task_terminal_stop(outcome)?;
                                    break (run, response, TurnProposals::default(), stop);
                                }
                            };
                            // The round totals can still allow a segment while
                            // the exact next request would overspend an approved
                            // token total. Prove the already-prepared request
                            // without changing it; on failure nothing is sent
                            // and no segment is started.
                            let prepared =
                                match authorize_task_request(&request, &tracker.budget, usage) {
                                    Ok(()) => PreparedTurnRequest {
                                        request,
                                        compacted_call_ids,
                                    },
                                    Err(failure) => {
                                        let outcome = self.task_authority_outcome(
                                            &session,
                                            &task,
                                            &tracker.budget,
                                            usage,
                                            &failure,
                                            Some(last_segment.clone()),
                                        )?;
                                        let (run, response, stop) =
                                            self.task_terminal_stop(outcome)?;
                                        break (run, response, TurnProposals::default(), stop);
                                    }
                                };
                            let tokens = SegmentTokenTotals::from_usage(usage);
                            let finished = TaskSegmentFinished {
                                format_version: TASK_SEGMENT_FORMAT_VERSION,
                                session_id: session.id.clone(),
                                task_id: task.id.clone(),
                                segment: tracker.segment,
                                segment_action_rounds: action_rounds,
                                segment_exploration_rounds: exploration_rounds,
                                total_action_rounds: totals.action_rounds,
                                total_exploration_rounds: totals.exploration_rounds,
                                tokens,
                                start_digest: tracker.start_digest.clone(),
                                progress_digest: digest.clone(),
                            };
                            let started = TaskSegmentStarted {
                                format_version: TASK_SEGMENT_FORMAT_VERSION,
                                session_id: session.id.clone(),
                                task_id: task.id.clone(),
                                segment: tracker.segment + 1,
                                segment_action_rounds: 0,
                                segment_exploration_rounds: 0,
                                total_action_rounds: totals.action_rounds,
                                total_exploration_rounds: totals.exploration_rounds,
                                tokens,
                                start_digest: digest.clone(),
                            };
                            self.session_store.append_task_segment_boundary(
                                &session.id,
                                &finished,
                                &started,
                            )?;
                            self.audit_log
                                .record("segment_finished", &finished.audit_fields())?;
                            self.audit_log
                                .record("segment_started", &started.audit_fields())?;

                            tracker.prior_action_rounds = totals.action_rounds;
                            tracker.prior_exploration_rounds = totals.exploration_rounds;
                            tracker.segment += 1;
                            tracker.start_digest = digest;
                            tracker.action_allowance = action_allowance;
                            tracker.exploration_allowance = exploration_allowance;
                            action_rounds = 0;
                            exploration_rounds = 0;
                            round_budget_extension = 0;
                            max_exploration_rounds = exploration_allowance;
                            boundary_request = Some(prepared);
                            continue;
                        }
                        SegmentContinuationDecision::Declined(reason) => {
                            self.audit_log.record(
                                "segment_continuation_declined",
                                &[
                                    ("actor", "system".to_string()),
                                    ("sessionId", session.id.clone()),
                                    ("taskId", task.id.clone()),
                                    ("segment", tracker.segment.to_string()),
                                    ("reason", reason.as_str().to_string()),
                                    ("totalActionRounds", totals.action_rounds.to_string()),
                                    (
                                        "totalExplorationRounds",
                                        totals.exploration_rounds.to_string(),
                                    ),
                                    (
                                        "inputTokens",
                                        usage.map_or_else(
                                            || "unavailable".to_string(),
                                            |usage| usage.input_tokens.to_string(),
                                        ),
                                    ),
                                    (
                                        "outputTokens",
                                        usage.map_or_else(
                                            || "unavailable".to_string(),
                                            |usage| usage.output_tokens.to_string(),
                                        ),
                                    ),
                                ],
                            )?;
                            let outcome = self.segment_decline_outcome(
                                &session,
                                &task,
                                reason,
                                &tracker.budget,
                                totals,
                                usage,
                                Some(last_segment),
                            )?;
                            let (run, response, stop) = self.task_terminal_stop(outcome)?;
                            break (run, response, TurnProposals::default(), stop);
                        }
                    }
                }
                // Spec 107 D3 (A): the same pure predicate as a worker segment,
                // under the default task budget. A decline is not a terminal
                // stop here: it falls through to the existing round-budget card,
                // so the user still decides how an ordinary session goes on.
                // Nothing durable is written for a continued checkpoint beyond
                // its audit line; the card's pause carries the counters as
                // before.
                if segment_tracker.is_none()
                    && let Some(ordinary) = ordinary_continuation.as_mut()
                {
                    let totals = ordinary.totals(action_rounds, exploration_rounds);
                    let usage = self
                        .session_store
                        .read_budget_usage(&session.id, &task.id)?;
                    let digest = self.segment_progress_digest(
                        plan.as_ref(),
                        &step_evidence,
                        &completed_tool_rounds,
                        &turn_notes,
                    );
                    let decision = decide_segment_continuation(&SegmentContinuationInput {
                        budget: &ordinary.budget,
                        totals,
                        usage: usage.as_ref(),
                        normal_action_rounds: self.tool_round_limit(web_debug_mode),
                        normal_exploration_rounds: self.default_exploration_round_limit(),
                        progress_advanced: digest != ordinary.start_digest,
                    });
                    let (event, reason) = match decision {
                        SegmentContinuationDecision::Continue { .. } => {
                            ("ordinary_continuation", "progress".to_string())
                        }
                        SegmentContinuationDecision::Declined(reason) => (
                            "ordinary_continuation_declined",
                            reason.as_str().to_string(),
                        ),
                    };
                    self.audit_log.record(
                        event,
                        &[
                            ("actor", "system".to_string()),
                            ("sessionId", session.id.clone()),
                            ("taskId", task.id.clone()),
                            ("reason", reason),
                            ("totalActionRounds", totals.action_rounds.to_string()),
                            (
                                "totalExplorationRounds",
                                totals.exploration_rounds.to_string(),
                            ),
                        ],
                    )?;
                    if let SegmentContinuationDecision::Continue {
                        action_rounds: action_allowance,
                        exploration_rounds: exploration_allowance,
                    } = decision
                    {
                        ordinary.advance(totals, digest, action_allowance);
                        action_rounds = 0;
                        exploration_rounds = 0;
                        round_budget_extension = 0;
                        max_exploration_rounds = self
                            .default_exploration_round_limit()
                            .min(exploration_allowance);
                        continue;
                    }
                }
                if segment_tracker.is_some()
                    || self.config.agent_max_task_tokens.is_none()
                    || action_rounds >= ABSOLUTE_TOOL_ROUND_CAP
                {
                    return self.pause_for_round_budget(
                        repository_root,
                        session,
                        task,
                        context_files,
                        messages,
                        action_rounds,
                        exploration_rounds,
                        turn_notes,
                        completed_tool_rounds.clone(),
                        step_evidence.clone(),
                        plan_reminder_sent,
                        max_rounds,
                        round_budget_extension,
                        turn_options,
                        segment_tracker.as_ref(),
                    );
                }
            }
            let tools = if force_final {
                None
            } else {
                native_tools.clone()
            };
            // One engine-owned request-preparation path. Below the window share
            // this is exactly the legacy bounded request; at or above it, the
            // safety-first selection and Task 3's reducer decide what is sent
            // (spec 103 Task 4). A request prepared for an automatic boundary
            // is reused rather than prepared a second time.
            let PreparedTurnRequest {
                request,
                compacted_call_ids,
            } = match boundary_request.take() {
                Some(prepared) => prepared,
                None => {
                    // Apply the approved output cap before Task 4 constructs
                    // and measures the request. The field is part of the
                    // request bytes, so doing it afterwards would let Task 4
                    // judge a different shape from the one the adapter sends.
                    let output_cap = match segment_tracker.as_ref() {
                        Some(tracker) => match task_request_output_cap(
                            &tracker.budget,
                            task_usage.as_ref(),
                            self.config.max_output_tokens(),
                        ) {
                            Ok(cap) => Some(cap),
                            Err(failure) => {
                                let last_segment = self.live_segment_snapshot(
                                    tracker,
                                    action_rounds,
                                    exploration_rounds,
                                    task_usage.as_ref(),
                                    self.segment_progress_digest(
                                        plan.as_ref(),
                                        &step_evidence,
                                        &completed_tool_rounds,
                                        &turn_notes,
                                    ),
                                );
                                let outcome = self.task_authority_outcome(
                                    &session,
                                    &task,
                                    &tracker.budget,
                                    task_usage.as_ref(),
                                    &failure,
                                    Some(last_segment),
                                )?;
                                let (run, response, stop) = self.task_terminal_stop(outcome)?;
                                break (run, response, TurnProposals::default(), stop);
                            }
                        },
                        // An unbudgeted task lets the request builder apply
                        // the configured output ceiling exactly as before.
                        None => None,
                    };
                    match self.build_turn_request(
                        repository_root,
                        &session,
                        &task,
                        &messages,
                        &turn_notes,
                        &completed_tool_rounds,
                        plan.as_ref(),
                        &step_evidence,
                        &active_protected,
                        tools,
                        output_cap,
                    )? {
                        TurnRequestDecision::Prepared(prepared) => prepared,
                        TurnRequestDecision::CannotFit(attempt) => {
                            // No model call and no segment boundary. The task
                            // settles with its own cannot-fit outcome. An
                            // unbudgeted task has no segment at all.
                            let last_segment = segment_tracker.as_ref().map(|tracker| {
                                self.live_segment_snapshot(
                                    tracker,
                                    action_rounds,
                                    exploration_rounds,
                                    task_usage.as_ref(),
                                    self.segment_progress_digest(
                                        plan.as_ref(),
                                        &step_evidence,
                                        &completed_tool_rounds,
                                        &turn_notes,
                                    ),
                                )
                            });
                            let outcome = self.cannot_fit_outcome(
                                &session,
                                &task,
                                &attempt,
                                task_usage.as_ref(),
                                last_segment,
                            )?;
                            let (run, response, stop) = self.task_terminal_stop(outcome)?;
                            break (run, response, TurnProposals::default(), stop);
                        }
                    }
                }
            };
            // A read whose body became a manifest may be performed again
            // through the ordinary tool path. Identity is the typed canonical
            // key recorded at dispatch, never a text heuristic.
            for key in completed_tool_rounds.compacted_read_keys(&compacted_call_ids) {
                seen_read_calls.remove(&key);
                for call in &mut turn_notes.calls {
                    if call.key.as_ref() == Some(&key) {
                        call.compacted = true;
                    }
                }
            }
            // A budgeted task has no untracked model call. The request was
            // already built with its approved output cap, so this check only
            // measures the exact prepared input and never changes the request.
            // The first request after approval is checked here too, so planning
            // spend cannot authorise another call.
            if let Some(tracker) = segment_tracker.as_ref()
                && let Err(failure) =
                    authorize_task_request(&request, &tracker.budget, task_usage.as_ref())
            {
                let last_segment = self.live_segment_snapshot(
                    tracker,
                    action_rounds,
                    exploration_rounds,
                    task_usage.as_ref(),
                    self.segment_progress_digest(
                        plan.as_ref(),
                        &step_evidence,
                        &completed_tool_rounds,
                        &turn_notes,
                    ),
                );
                let outcome = self.task_authority_outcome(
                    &session,
                    &task,
                    &tracker.budget,
                    task_usage.as_ref(),
                    &failure,
                    Some(last_segment),
                )?;
                let (run, response, stop) = self.task_terminal_stop(outcome)?;
                break (run, response, TurnProposals::default(), stop);
            }

            let token_estimate: usize = request
                .messages
                .iter()
                .map(|message| message.content.len())
                .sum::<usize>()
                .div_ceil(4);
            self.audit_log.record(
                "model_request_prepared",
                &[
                    ("actor", "system".to_string()),
                    ("sessionId", session.id.clone()),
                    ("taskId", task.id.clone()),
                    ("repositoryId", session.repository_id.clone()),
                    ("contextFiles", context_files.join(",")),
                    ("tokenEstimate", token_estimate.to_string()),
                    ("toolRound", action_rounds.to_string()),
                    ("toolRoundLimit", max_rounds.to_string()),
                    // Spec 59 §5.6: both budgets, so a reader of the audit trail
                    // can see what the turn had read as well as what it had
                    // acted on.
                    ("explorationRounds", exploration_rounds.to_string()),
                    ("explorationLimit", max_exploration_rounds.to_string()),
                    // Spec 65 correction C6: the request records how big the
                    // notes are, never their text (that is in the session log).
                    ("notesChars", turn_notes.text.chars().count().to_string()),
                ],
            )?;

            sink.phase(
                PhaseKind::Model,
                "",
                action_rounds,
                max_rounds,
                exploration_rounds,
                max_exploration_rounds,
                turn_notes.text.chars().count(),
            );
            // `sideEffecting: false` — §4 treats a cut stream as a lost call:
            // the task resumes by making a new one, and spec 19 reports the cost
            // of the lost one rather than hiding it. So a marker left dangling
            // on the cancellation path classifies as `interrupted`, which is the
            // correct answer per §5.1 ("a call may have been billed").
            // The estimate rides on the marker because it must outlive the
            // request: after a crash the request is gone, and recovery would
            // otherwise have only a zero to account a billed call with.
            // Spec 19 §5.5.
            let model_marker = self.session_store.start_action_with_estimate(
                &task,
                "model_call",
                &self.config.model_name,
                false,
                Some(model_request_json(&request).len().div_ceil(4) as u64),
            )?;
            let model_marker_id = model_marker.id().to_string();

            // This round's output, kept here as well as streamed. A stop
            // mid-stream returns `Err(Cancelled)` and no run at all, and these
            // tokens are then the only record that the provider generated —
            // and billed — anything. Spec 19 §5.5.
            let mut round_output = String::new();
            let stream_result = {
                let on_token = &mut *sink.on_token;
                // Shared by the refusal wait and the reasoning count below.
                let on_progress = std::cell::RefCell::new(&mut *sink.on_progress);
                let mut accumulate = |token: &str| {
                    round_output.push_str(token);
                    on_token(token);
                };
                // The refusal wait is out-of-band: the token stream stays
                // clean, and the phase carries the remaining seconds so the
                // user watching "retrying in 47s" can stop it. Spec 48 §5.3.
                let mut on_wait = |seconds: u64| {
                    (on_progress.borrow_mut())(TurnProgress::Phase(TurnPhase::new(
                        PhaseKind::Model,
                        format!("Provider refused — retrying in {seconds}s"),
                        action_rounds,
                        max_rounds,
                        exploration_rounds,
                        max_exploration_rounds,
                        turn_notes.text.chars().count(),
                    )));
                };
                // Reasoning arrives a few characters per chunk; one phase per
                // chunk would be most of a worker's progress file. Spec 81 §4.
                let mut last_thinking: Option<std::time::Instant> = None;
                let mut on_reasoning = |reasoning_chars: usize| {
                    if last_thinking.is_some_and(|last| last.elapsed() < THINKING_PROGRESS_INTERVAL)
                    {
                        return;
                    }
                    last_thinking = Some(std::time::Instant::now());
                    let mut phase = TurnPhase::new(
                        PhaseKind::Thinking,
                        "",
                        action_rounds,
                        max_rounds,
                        exploration_rounds,
                        max_exploration_rounds,
                        turn_notes.text.chars().count(),
                    );
                    phase.reasoning_chars = reasoning_chars;
                    (on_progress.borrow_mut())(TurnProgress::Phase(phase));
                };
                model_adapter.stream_response_with_reasoning(
                    &request,
                    sink.cancel,
                    &mut accumulate,
                    &mut on_wait,
                    &mut on_reasoning,
                )
            };
            let model_run = match stream_result {
                Ok(model_run) => model_run,
                // A stop is not a failure. The transport raises `Cancelled`
                // when it killed the request mid-flight, and it must not be
                // recorded as a provider error.
                Err(ClientError::Cancelled) => {
                    let cancelled_run = ModelRun::cancelled_before_start(
                        &self.config.model_provider,
                        &self.config.model_name,
                    );
                    // The request was sent and the answer was cut short, so
                    // this is not the free case: estimate from what went out
                    // and what came back before the stop.
                    self.session_store.record_task_usage(
                        &task,
                        Some(CallModel::configured(&self.config)),
                        &cancelled_run.run_id,
                        Some(&model_marker_id),
                        TokenUsage::estimated(
                            model_request_json(&request).len().div_ceil(4) as u64,
                            round_output.len().div_ceil(4) as u64,
                        ),
                        None,
                        Some("stopped_mid_stream"),
                    )?;
                    return self.finish_cancelled_turn(
                        repository_root,
                        session,
                        task,
                        context_files,
                        &partial_response,
                        &turn_notes,
                        cancelled_run,
                        plan,
                        step_evidence,
                        plan_reminder_sent,
                        sink,
                    );
                }
                Err(ClientError::Provider(refusal, message)) => {
                    // A refusal is a named failure: the marker closes "refused"
                    // and the pre-call estimate is overridden, so a call the
                    // provider rejected before generating anything cannot
                    // inflate the task's reported spend. A refusal that arrived
                    // *mid-stream* is different — the provider streamed and
                    // billed something, so it is estimated from what went out
                    // and what came back, exactly as a cancelled run is
                    // (spec 48 §5.5).
                    self.session_store.finish_action(model_marker, "refused")?;
                    let usage = if round_output.is_empty() {
                        TokenUsage::measured_zero()
                    } else {
                        TokenUsage::estimated(
                            model_request_json(&request).len().div_ceil(4) as u64,
                            round_output.len().div_ceil(4) as u64,
                        )
                    };
                    self.session_store.record_task_usage(
                        &task,
                        Some(CallModel::configured(&self.config)),
                        &create_id("modelrun"),
                        Some(&model_marker_id),
                        usage,
                        None,
                        Some("refused"),
                    )?;
                    // Requirement 8: every refusal is audited with its
                    // classification — never the request body or the key, so
                    // the fields are ids, names, the typed kind, and the
                    // provider's own message, nothing that was sent.
                    self.audit_log.record(
                        "provider_refusal",
                        &[
                            ("actor", "system".to_string()),
                            ("sessionId", session.id.clone()),
                            ("taskId", task.id.clone()),
                            ("provider", self.config.model_provider.clone()),
                            ("model", self.config.model_name.clone()),
                            ("classification", refusal.code().to_string()),
                            ("message", message.clone()),
                        ],
                    )?;
                    // Spec 56: where a different configured provider could
                    // answer, the refusal becomes a declined-by-default offer
                    // rather than a failure. Everything above — the marker, the
                    // measured zero, the audit — has already happened either
                    // way; only the task's outcome waits on the user.
                    if let Some(candidate) = self
                        .config
                        .fallback_provider_for(&refusal)
                        .filter(|_| !turn_options.provider_fallback_used)
                    {
                        let offer = PendingProviderFallback {
                            refused_provider: self.config.model_provider.clone(),
                            refused_model: self.config.model_name.clone(),
                            fallback_provider: candidate.id.clone(),
                            fallback_model: candidate.models.first().cloned().unwrap_or_default(),
                            refusal_code: refusal.code().to_string(),
                            message,
                        };
                        return self.pause_for_provider_fallback(
                            repository_root,
                            session,
                            task,
                            context_files,
                            messages,
                            action_rounds,
                            exploration_rounds,
                            turn_notes,
                            completed_tool_rounds.clone(),
                            step_evidence.clone(),
                            plan_reminder_sent,
                            turn_options,
                            segment_tracker.as_ref(),
                            offer,
                        );
                    }
                    // A provider failure is a true end, so the open step keeps
                    // its accrued evidence (spec 66 §5.3). Best-effort: an
                    // unwritable log must not hide the provider's error.
                    let _ = self.settle_plan_at_turn_end(
                        &session,
                        &task,
                        &mut plan,
                        &mut step_evidence,
                        plan_reminder_sent,
                        "failed",
                    );
                    let _ = self.session_store.update_task_status_with_kind(
                        &task,
                        TaskStatus::Failed,
                        Some(message.as_str()),
                        Some(refusal.code()),
                    );
                    if let Some(current) = plan.as_ref() {
                        sink.plan_finished(current);
                    }
                    return Err(ClientError::Provider(refusal, message));
                }
                Err(error) => {
                    let _ = self.settle_plan_at_turn_end(
                        &session,
                        &task,
                        &mut plan,
                        &mut step_evidence,
                        plan_reminder_sent,
                        "failed",
                    );
                    let _ = self.session_store.update_task_status(
                        &task,
                        TaskStatus::Failed,
                        Some(&error.to_string()),
                    );
                    if let Some(current) = plan.as_ref() {
                        sink.plan_finished(current);
                    }
                    return Err(error);
                }
            };
            self.session_store.finish_action(model_marker, "ok")?;

            self.session_store.record_task_usage(
                &task,
                Some(CallModel::configured(&self.config)),
                &model_run.run_id,
                Some(&model_marker_id),
                model_run.usage,
                model_run.reported_cost,
                None,
            )?;
            // Requirement 5: an attempt that reached the provider was billed
            // for its input even though its answer never arrived. Same input,
            // no output. Over-counting a connection that failed before the
            // body went out is the deliberate direction of error —
            // under-reporting makes Damaian look cheaper than it is.
            for attempt in 0..model_run.retry_count {
                self.session_store.record_task_usage(
                    &task,
                    Some(CallModel::configured(&self.config)),
                    &format!("{}_retry{}", model_run.run_id, attempt + 1),
                    Some(&model_marker_id),
                    TokenUsage::estimated(model_run.usage.input_tokens, 0),
                    None,
                    Some("retried_attempt"),
                )?;
            }
            recorded_any_usage = true;

            // The adapter observes this once per provider per process, and
            // has no `AuditLog` of its own — see `ModelRun`. Recording it here
            // is what lets a user set `provider_reports_usage=false` and stop
            // paying for a probe on every process start.
            if model_run.usage_reporting_unsupported {
                self.audit_log.record(
                    "model_usage_reporting_unsupported",
                    &[
                        ("actor", "system".to_string()),
                        ("sessionId", session.id.clone()),
                        ("taskId", task.id.clone()),
                        ("provider", self.config.model_provider.clone()),
                        ("model", self.config.model_name.clone()),
                    ],
                )?;
            }
            model_run.record_quirks(&self.audit_log, &session.id, &task.id)?;

            let redacted = self.scanner.redact(&model_run.content).text;

            // An adapter that streamed part of an answer before noticing the
            // stop returns `Ok` with partial content, so the flag has to be
            // re-checked here as well as at the top of the loop.
            if sink.cancel.is_cancelled() {
                let partial = model_run.content.clone();
                return self.finish_cancelled_turn(
                    repository_root,
                    session,
                    task,
                    context_files,
                    &partial,
                    &turn_notes,
                    model_run,
                    plan,
                    step_evidence,
                    plan_reminder_sent,
                    sink,
                );
            }
            partial_response = redacted.clone();

            if force_final {
                if model_output_requests_tool(&model_run, &redacted) {
                    // Requirement 6: with a token ceiling set, the round cap is
                    // not the end. The ceiling is the explicit, bounded budget,
                    // so the turn takes another round segment and records that
                    // it did rather than stopping dead. With no ceiling there is
                    // no bound to continue under, so today's `ToolBudget` stop
                    // stands. `action_rounds < ABSOLUTE_TOOL_ROUND_CAP` is the
                    // safety net for a provider whose usage never reaches the
                    // ceiling.
                    if self.config.agent_max_task_tokens.is_some()
                        && action_rounds < ABSOLUTE_TOOL_ROUND_CAP
                    {
                        self.audit_log.record(
                            "turn_continued",
                            &[
                                ("actor", "system".to_string()),
                                ("sessionId", session.id.clone()),
                                ("taskId", task.id.clone()),
                                ("round", action_rounds.to_string()),
                                ("roundLimit", max_rounds.to_string()),
                                // Spec 59 §5.6: this record's round fields are
                                // `round`/`roundLimit`, so the exploration
                                // pair is named explicitly rather than shared.
                                ("explorationRounds", exploration_rounds.to_string()),
                                ("explorationLimit", max_exploration_rounds.to_string()),
                                ("notesChars", turn_notes.text.chars().count().to_string()),
                            ],
                        )?;
                        messages.push(
                            ModelMessage::assistant(redacted.clone())
                                .with_reasoning_content(model_run.reasoning_content.clone()),
                        );
                        messages.push(ModelMessage::user(
                            "Your tool-round segment ended and the turn continued under its token ceiling. Continue the task; the ceiling will stop it when it is reached."
                                .to_string(),
                        ));
                        round_budget_extension += self.default_tool_round_limit();
                        // The forced-final call spent a round but dispatched
                        // nothing, so it belongs to the action budget (§5.2).
                        action_rounds += 1;
                        continue;
                    }
                    let response = tool_budget_exhausted_response(max_rounds, exploration_rounds);
                    let mut exhausted_run = model_run;
                    exhausted_run.content = response.clone();
                    break (
                        exhausted_run,
                        response,
                        TurnProposals::default(),
                        StopReason::ToolBudget,
                    );
                }
                // Spec 32 `context.md` §4: a hook that denies a completion
                // does not fail the turn — the reason goes back as a user-role
                // note and the model gets another round. The action budget
                // bounds the re-entries: a call that dispatched nothing is an
                // action round, exactly as the forced-final call is (§5.2).
                //
                // *Answer now* is the user's own decision to end the turn at
                // the round budget (spec 61 §5.2). The hook does not run
                // there: §4's correction needs another round, and this path
                // deliberately has none left — re-entering it would loop
                // without a bound, because the pause that would stop it is
                // skipped for exactly this turn.
                if !turn_options.round_budget_answer_now
                    && let Some(note) =
                        self.before_task_complete(repository_root, &session, &task, action_rounds)?
                {
                    messages.push(
                        ModelMessage::assistant(redacted.clone())
                            .with_reasoning_content(model_run.reasoning_content.clone()),
                    );
                    messages.push(ModelMessage::user(note));
                    action_rounds += 1;
                    continue;
                }
                // The reminder runs after the hook allows, so the hook sees the
                // answer and the plan as they stand (spec 66 `context.md` D1).
                // At a forced-final call the two-round gate is already false,
                // because `action_rounds >= max_rounds`; kept here so both
                // completion points share one rule.
                if let Some((reminder, open_steps)) = self.plan_reminder_note(
                    &session.id,
                    &task.id,
                    plan.as_ref(),
                    plan_reminder_sent,
                    action_rounds,
                    max_rounds,
                    turn_options.round_budget_answer_now,
                )? {
                    self.send_plan_reminder(
                        &session,
                        &task,
                        &redacted,
                        &model_run,
                        &reminder,
                        open_steps,
                        &mut messages,
                        &mut pre_reminder_answer,
                    )?;
                    plan_reminder_sent = true;
                    action_rounds += 1;
                    continue;
                }
                break (
                    model_run,
                    redacted,
                    TurnProposals::default(),
                    StopReason::Answered,
                );
            }

            let (mut calls_this_round, undecodable_calls) =
                decodable_tool_actions(&model_run.tool_calls, model_run.truncated);
            // Spec 103 R17: after the pre-plan instruction, the model either
            // proposes the plan it was asked for or the task stops. A tool call
            // made instead is deliberately not dispatched, and an ordinary text
            // answer does not become a completion: both are the same failure to
            // produce the plan, and neither may become a generic Continue.
            if preplan_final_attempt
                && plan.is_none()
                && !calls_this_round
                    .iter()
                    .any(|(_, action)| matches!(action, ToolAction::ProposePlan(_)))
                && let Some(tracker) = segment_tracker.as_ref()
            {
                let totals = SegmentTotals {
                    action_rounds: tracker.total_action_rounds(action_rounds),
                    exploration_rounds: tracker.total_exploration_rounds(exploration_rounds),
                };
                let last_segment = self.live_segment_snapshot(
                    tracker,
                    action_rounds,
                    exploration_rounds,
                    task_usage.as_ref(),
                    self.segment_progress_digest(
                        plan.as_ref(),
                        &step_evidence,
                        &completed_tool_rounds,
                        &turn_notes,
                    ),
                );
                let outcome = self.preplan_exhausted_outcome(
                    &session,
                    &task,
                    &tracker.budget,
                    totals,
                    task_usage.as_ref(),
                    Some(last_segment),
                )?;
                let (run, response, stop) = self.task_terminal_stop(outcome)?;
                break (run, response, TurnProposals::default(), stop);
            }
            // The text envelope is the fallback only when no native call
            // decoded, exactly as before: one native call and the envelope is
            // ignored.
            if calls_this_round.is_empty()
                && let Some(command) = parse_command_request(&redacted)
            {
                calls_this_round.push((None, ToolAction::Command(command)));
            }

            if calls_this_round.is_empty() {
                // The model asked for a tool but the request couldn't be
                // decoded. The usual cause is the provider stopping at its
                // output-token ceiling and cutting the `arguments` JSON off
                // mid-string. This used to end the turn silently: the user saw
                // a lead-in like "Let me create all the necessary files:", no
                // patch, and a task marked complete. Feed the failure back
                // instead so the model can retry within the remaining rounds —
                // the same recovery the restricted-path patch arm uses.
                let Some((undecodable, note)) = undecodable_calls.first() else {
                    // The model answered with no tool call: a completion, so
                    // `before_task_complete` sees it (spec 32 `context.md` §4).
                    if let Some(note) =
                        self.before_task_complete(repository_root, &session, &task, action_rounds)?
                    {
                        messages.push(
                            ModelMessage::assistant(redacted.clone())
                                .with_reasoning_content(model_run.reasoning_content.clone()),
                        );
                        messages.push(ModelMessage::user(note));
                        action_rounds += 1;
                        continue;
                    }
                    // Only after the hook allows: the reminder is the last
                    // thing between the turn and its end, not a second engine
                    // note beside the hook's (spec 66 `context.md` D1).
                    if let Some((reminder, open_steps)) = self.plan_reminder_note(
                        &session.id,
                        &task.id,
                        plan.as_ref(),
                        plan_reminder_sent,
                        action_rounds,
                        max_rounds,
                        turn_options.round_budget_answer_now,
                    )? {
                        self.send_plan_reminder(
                            &session,
                            &task,
                            &redacted,
                            &model_run,
                            &reminder,
                            open_steps,
                            &mut messages,
                            &mut pre_reminder_answer,
                        )?;
                        plan_reminder_sent = true;
                        action_rounds += 1;
                        continue;
                    }
                    break (
                        model_run,
                        redacted,
                        TurnProposals::default(),
                        StopReason::Answered,
                    );
                };
                let summary = format!(
                    "Attempted to call `{}`, but the request could not be decoded.",
                    undecodable.name
                );
                self.session_store.append_message(
                    &session.id,
                    Some(&task.id),
                    "assistant",
                    &summary,
                )?;
                self.session_store
                    .append_message(&session.id, Some(&task.id), "tool", note)?;
                // Deliberately not echoed back as an `assistant` tool_calls /
                // `tool` pair: the malformed arguments would have to be
                // replayed verbatim, and providers reject a tool result whose
                // call didn't parse. Plain text carries the correction safely.
                messages.push(
                    ModelMessage::assistant(if redacted.trim().is_empty() {
                        summary
                    } else {
                        redacted.clone()
                    })
                    .with_reasoning_content(model_run.reasoning_content.clone()),
                );
                messages.push(ModelMessage::user(note.clone()));
                // A round whose only calls failed to decode is not known to be
                // reads, so it is an action round (§5.1).
                action_rounds += 1;
                continue;
            }

            // §5.1: a round is exploration when every decoded call is a read,
            // nothing in it failed to decode, none is a repeat (§5.3), and the
            // allowance has room. Classification uses `action_effect` and
            // nothing else, so a new `ToolAction` variant's effect is also its
            // budget decision.
            //
            // The canonical keys are computed once here and remembered for the
            // dispatch below, so a call is not resolved twice and a batch and a
            // sequential round agree on what counts as the same call.
            let call_keys: Vec<Option<Vec<String>>> = calls_this_round
                .iter()
                .map(|(_, action)| self.call_key(repository_root, action))
                .collect();
            let repeats_a_seen_call = {
                // A duplicate inside this round is a repeat too: two identical
                // calls in one round make it an action round.
                let mut this_round = HashSet::new();
                call_keys
                    .iter()
                    .flatten()
                    .any(|key| seen_read_calls.contains(key) || !this_round.insert(key))
            };
            let round_is_exploration = !calls_this_round.is_empty()
                && undecodable_calls.is_empty()
                && calls_this_round.iter().all(|(_, action)| {
                    matches!(
                        action_effect(action),
                        ActionEffect::ReadOnly | ActionEffect::Notes
                    )
                })
                && !repeats_a_seen_call
                && exploration_rounds < max_exploration_rounds;

            // Every decoded call runs, in the order the model made them. A
            // terminal arm (a command needing approval, a patch ready for
            // review) ends the turn from inside the loop; a non-terminal one
            // records its result and the loop continues. The break value is
            // carried out in `terminal` because a `break` inside the `for`
            // would only leave the loop, not the turn.
            // An all-read-only round is dispatched concurrently; any other round
            // falls back to sequential, in-order dispatch — which is also what
            // runs the mutating calls one at a time (requirement 8). A single
            // call is never batched: there is nothing to overlap.
            // A stop that arrived before the round starts must not begin a
            // batch of reads; the top-of-loop check finishes the turn.
            //
            // `before_tool` runs per call before the batch, not inside it: a
            // hook is a process, and spawning several in parallel would make
            // their order — and therefore a deny's short-circuit — depend on
            // scheduling. A call the mode, profile or a hook refuses is taken
            // out of the batch; its refusal is recorded by the sequential
            // loop below, which keeps one result per model call.
            let mut precomputed_permissions: Vec<Option<Permission>> =
                vec![None; calls_this_round.len()];
            let mut precomputed_before: Vec<Option<HookOutcome>> =
                vec![None; calls_this_round.len()];
            let precomputed = if !sink.cancel.is_cancelled()
                && calls_this_round.len() > 1
                && calls_this_round
                    .iter()
                    .all(|(_, action)| action_is_batchable_read_only(action))
            {
                let mut batch_actions: Vec<DecodedCall> = Vec::new();
                let mut batch_indices: Vec<usize> = Vec::new();
                for (index, (_, action)) in calls_this_round.iter().enumerate() {
                    let permission = self.action_permission(mode, repository_root, action, &mcp);
                    precomputed_permissions[index] = Some(permission);
                    if !permission.is_allowed() {
                        continue;
                    }
                    let payload = tool_event_payload(&self.scanner, action, None, None);
                    let outcome = self.dispatch_tool_event(
                        HookEvent::BeforeTool,
                        &session,
                        &task,
                        repository_root,
                        payload,
                    );
                    let allowed =
                        before_tool_effect(action, outcome.verdict) == BeforeToolEffect::Proceed;
                    precomputed_before[index] = Some(outcome);
                    // A notes call may share a round with reads, but its
                    // mutation belongs on the loop thread after the batch
                    // returns — never inside the batch worker (spec 65 §5.1).
                    if allowed && matches!(action_effect(action), ActionEffect::ReadOnly) {
                        batch_indices.push(index);
                        batch_actions.push(calls_this_round[index].clone());
                    }
                }
                let mut results: Vec<Option<(String, String, ActionOutcome)>> =
                    vec![None; calls_this_round.len()];
                if !batch_actions.is_empty() {
                    let computed = self.run_read_only_batch(
                        repository_root,
                        &session,
                        &task,
                        &batch_actions,
                        sink.cancel,
                    );
                    for (index, result) in batch_indices.into_iter().zip(computed) {
                        results[index] = Some(result);
                    }
                }
                Some(results)
            } else {
                None
            };
            let mut terminal: Option<(ModelRun, String, TurnProposals, StopReason)> = None;
            // Whether this round ran something that changes what a read
            // reports, so the seen-call set is cleared once the round completes
            // (spec 59 §5.3).
            let mut wrote_this_round = false;
            // Index in `messages` of this round's one assistant message, once
            // its first call has run. Later calls join it (spec 03 §7): a
            // response is one assistant turn, whatever number of calls it made.
            let mut round_message: Option<usize> = None;
            // The calls that complete in this round, kept together so a batched
            // assistant round stays one tracker round (spec 103 Task 4).
            let mut round_completed: Vec<TrackedCompletedToolCall> = Vec::new();
            // Kept out of `model_run`, which a terminal arm moves.
            let round_reasoning = model_run.reasoning_content.clone();
            for (index, (matched_tool_call, tool_action)) in
                calls_this_round.into_iter().enumerate()
            {
                // A stop between calls of a round ends the batch: no further
                // call starts, and the top of the loop finishes the turn. A
                // read already in flight is not interrupted — it is a bounded
                // local read, and the tools take no cancel token — but none
                // begins after this point.
                if sink.cancel.is_cancelled() {
                    break;
                }
                // Layer 3 (`proposal.md` §5.2): the mode captured at the top of
                // the turn is asked about every action before anything else can
                // act on it. Before the review gate, so no plan review is raised
                // for a step the mode is about to refuse; before dispatch, so a
                // refused command leaves no stored proposal and no approval card
                // behind. One site for every class rather than one per arm, so a
                // new `ToolAction` variant cannot reach its arm unchecked.
                let permission = precomputed_permissions[index].unwrap_or_else(|| {
                    self.action_permission(mode, repository_root, &tool_action, &mcp)
                });

                // The review gate (§5.5). Checked before the action is bracketed,
                // let alone dispatched: there is no marker to finish and nothing
                // to undo, because nothing has happened yet. That is the whole
                // point of putting it here rather than beside the approval card
                // the action would have raised on its own — the user is being
                // asked about the plan, not about this one step, and asking after
                // the first edit had already been prepared would be asking too
                // late to redirect.
                //
                // A turn with no plan is not gated. The gate exists to review a
                // plan, and inventing one to have something to approve would put a
                // panel in front of every trivial question (§5.1).
                // A plan holding only the verification loop's own `verify_`
                // steps is not one the model proposed, so there is nothing to
                // review: a planless task continuing after a verified patch
                // (spec 107 D3) carries exactly that.
                if let Some(current) = plan.as_ref()
                    && has_primary_steps(current)
                    && !plan_approved
                    && !turn_options.verification_turn
                    && permission.is_allowed()
                    && action_awaits_plan_review(&tool_action, |command| {
                        self.validation_orchestrator
                            .command_needs_approval(repository_root, command)
                    })
                {
                    let deferred_action = tool_action_label(&tool_action);
                    let budget_review = self.worker_task_budget_review(&session.id)?;
                    let response = plan_review_response(current, &deferred_action);
                    let proposal_id = create_id("planreview");
                    completed_tool_rounds.push_round(std::mem::take(&mut round_completed));
                    self.pending_commands.save(&PendingChatTurn {
                        proposal_id: proposal_id.clone(),
                        session: session.clone(),
                        task: task.clone(),
                        repository_root: repository_root.to_string_lossy().to_string(),
                        context_files: context_files.clone(),
                        round: action_rounds,
                        exploration_rounds,
                        turn_notes: turn_notes.text.clone(),
                        turn_notes_carried: turn_notes.carried,
                        calls: turn_notes.calls.clone(),
                        completed_tool_rounds: completed_tool_rounds.rounds.clone(),
                        // A pause carries the open step's evidence and the
                        // reminder flag; neither is attached or reset here.
                        step_evidence: step_evidence.clone(),
                        plan_reminder_sent,
                        messages: messages.clone(),
                        // The deferred call is deliberately dropped rather than
                        // stored: it was never dispatched, and the resumed turn
                        // asks the model again rather than replaying a request the
                        // user may have just revised out of the plan.
                        matched_tool_call: None,
                        last_content: redacted.clone(),
                        turn_options,
                        reasoning_content: model_run.reasoning_content.clone(),
                        round_calls_fed_back: round_message.is_some(),
                        mcp_call: None,
                        web_diagnostic_call: None,
                        plan_review: Some(PendingPlanReview {
                            deferred_action: deferred_action.clone(),
                            budget: budget_review.clone(),
                        }),
                        provider_fallback: None,
                        round_budget: None,
                        segment: segment_tracker
                            .as_ref()
                            .map(|tracker| tracker.pause_state(action_rounds, exploration_rounds)),
                        question: None,
                        check_batch: None,
                        failure_baseline: None,
                        fetch_call: None,
                        context_review: None,
                    })?;
                    self.note_pending_approvals(
                        &session,
                        &task,
                        vec![PendingApproval {
                            kind: "plan".to_string(),
                            proposal_id: proposal_id.clone(),
                        }],
                    );
                    let mut proposal_run = model_run;
                    proposal_run.content = response.clone();
                    terminal = Some((
                        proposal_run,
                        response,
                        TurnProposals {
                            plan: Some(AgentPlanProposal {
                                id: proposal_id,
                                plan: current.clone(),
                                deferred_action,
                                budget_review,
                            }),
                            plan_changes_code: action_changes_code(&tool_action)
                                || current.steps.iter().any(|step| step.check.is_some()),
                            ..Default::default()
                        },
                        StopReason::Answered,
                    ));
                    break;
                }

                // Spec 32 `context.md` §3: after the mode/profile check and
                // the plan-review gate, before the action marker. A refusal
                // here is a tool result like a mode refusal, but nothing was
                // dispatched, so it leaves no marker, proposal or card.
                let before_tool = precomputed_before[index].take().unwrap_or_else(|| {
                    let payload = tool_event_payload(&self.scanner, &tool_action, None, None);
                    self.dispatch_tool_event(
                        HookEvent::BeforeTool,
                        &session,
                        &task,
                        repository_root,
                        payload,
                    )
                });
                let before_effect = before_tool_effect(&tool_action, before_tool.verdict);
                if before_effect == BeforeToolEffect::Refuse {
                    let summary = format!(
                        "Attempted to call `{}`.",
                        tool_action_marker(&tool_action).0
                    );
                    let tool_result = hook_refusal(&before_tool);
                    self.session_store.append_message(
                        &session.id,
                        Some(&task.id),
                        "assistant",
                        &summary,
                    )?;
                    self.session_store.append_message(
                        &session.id,
                        Some(&task.id),
                        "tool",
                        &tool_result,
                    )?;
                    if let Some(call) = &matched_tool_call {
                        match round_message {
                            Some(index) => messages[index].tool_calls.push(call.clone()),
                            None => {
                                round_message = Some(messages.len());
                                messages.push(ModelMessage::assistant_with_tool_calls(
                                    redacted.clone(),
                                    vec![call.clone()],
                                    round_reasoning.clone(),
                                ));
                            }
                        }
                        messages.push(ModelMessage::tool(call.id.clone(), tool_result));
                    } else {
                        messages.push(
                            ModelMessage::assistant(redacted.clone())
                                .with_reasoning_content(model_run.reasoning_content.clone()),
                        );
                        messages.push(ModelMessage::user(format!(
                            "Command result:\n{tool_result}"
                        )));
                    }
                    continue;
                }
                if before_effect == BeforeToolEffect::SatisfiedApproval {
                    self.record_satisfied_approval(&session.id, &task.id, &before_tool);
                }
                let force_approval = before_effect == BeforeToolEffect::ForceApproval;
                let force_approval_cause =
                    force_approval.then(|| hook_cause(&before_tool)).flatten();

                if matches!(tool_action, ToolAction::WebDiagnostic(_)) {
                    web_debug_mode = true;
                }
                let mut max_rounds = (self.tool_round_limit(web_debug_mode)
                    + round_budget_extension)
                    .min(ABSOLUTE_TURN_ROUND_CAP);
                if let Some(tracker) = &segment_tracker {
                    max_rounds = max_rounds.min(tracker.action_allowance);
                }
                sink.phase(
                    PhaseKind::Tool,
                    tool_action_label(&tool_action),
                    action_rounds,
                    max_rounds,
                    exploration_rounds,
                    max_exploration_rounds,
                    turn_notes.text.chars().count(),
                );

                // Which plan step was open when this call ran. Captured before
                // dispatch because `complete_step` may advance the plan from
                // inside the same round (spec 103 Task 4).
                let open_step_id = plan.as_ref().and_then(|current| {
                    current
                        .steps
                        .iter()
                        .find(|step| step.status == StepStatus::InProgress)
                        .map(|step| step.id.clone())
                });
                // Bracket the dispatch, not the leaf call: `ValidationOrchestrator`
                // and `McpClient` hold no `SessionStore` and receive no `Task`, so
                // marking inside them would mean threading session state through
                // two modules that have nothing to do with it. The window is
                // slightly wider than the leaf action — it includes proposal and
                // setup — which errs toward reporting an unknown outcome rather
                // than missing one, the safe direction for requirement 5.
                let (marker_action, marker_ref, marker_side_effecting) =
                    tool_action_marker(&tool_action);
                let action_marker = self.session_store.start_action(
                    &task,
                    marker_action,
                    &marker_ref,
                    marker_side_effecting,
                )?;

                // Each non-terminal arm below produces the (assistant summary,
                // tool result, outcome) triple to persist and feed back to the
                // model. Terminal outcomes (a command needing approval, or a patch
                // ready for review) `break` the loop directly instead, since
                // both always require the human before anything continues.
                //
                // The third element is what the tool *reported*, which is not the
                // same as whether dispatching it worked. Every arm used to converge
                // on `finish_action(marker, "ok")`, so a command that exited
                // non-zero was indistinguishable in the log from one that passed —
                // and spec 18's `tool_and_model_error_rate` consequently read 0.000
                // by construction. Spec 21 requirement 6 reads a step's status from
                // this value, so it has to be the tool's answer, not the
                // dispatcher's.
                //
                // A sandbox command's findings leave its arm through this local
                // rather than through `ActionOutcome` (spec 22 `context.md` §14).
                let mut command_findings: Option<crate::plan::Evidence> = None;
                let mut command_termination: Option<CommandTermination> = None;
                // The directory the command actually ran in, repository-relative
                // for the evidence (spec 105's check identity). Carried out of
                // the arm like the termination: `CommandExecution` is not
                // persisted.
                let mut command_directory: Option<String> = None;
                // Kept for `after_tool`; the dispatch match below consumes the
                // action's fields.
                let after_tool_action = tool_action.clone();
                // A working directory the policy refuses fails the tool result
                // before anything is proposed or run (spec 24 Task 7). It is
                // resolved once here so the dispatch arm proposes in the same
                // directory the classifier saw.
                let refused_directory = match &tool_action {
                    ToolAction::Command(request) => resolve_command_directory(
                        &self.path_policy,
                        repository_root,
                        request.working_directory.as_deref().unwrap_or_default(),
                    )
                    .err(),
                    _ => None,
                };
                // Captured before the dispatch match consumes `tool_action`.
                // A denied call never dispatched, so it is not a seen call or a
                // recorded call; a read that failed on its own (a missing file,
                // a refused path) did, and still counts. A refused working
                // directory and a question nobody can answer are refused before
                // dispatch, so neither is recorded either.
                let dispatched = permission.is_allowed()
                    && refused_directory.is_none()
                    && !(matches!(tool_action, ToolAction::AskUser(_)) && !ask_user_offered);
                let dispatched_call_key = if dispatched {
                    call_keys[index].clone()
                } else {
                    None
                };
                // The notice and warning read this line, so it is redacted and
                // truncated here, once, rather than on every request.
                let call_line = dispatched
                    .then(|| self.render_call_line(repository_root, &tool_action))
                    .flatten()
                    .map(|line| {
                        truncate_for_prompt(
                            &single_line(&self.scanner.redact(&line).text),
                            CALL_LINE_MAX_CHARS,
                        )
                    });
                let round_number = action_rounds + exploration_rounds + 1;
                let writes_this_call = permission.is_allowed()
                    && matches!(action_effect(&tool_action), ActionEffect::WritesRepository);
                let (assistant_summary, mut tool_result_text, action_outcome) = if !permission
                    .is_allowed()
                {
                    // A failed tool result rather than an error, so the
                    // model can tell the user why in its next message
                    // (§5.6) instead of the turn just ending.
                    (
                        format!(
                            "Attempted to call `{}`.",
                            tool_action_marker(&tool_action).0
                        ),
                        refusal_message(permission),
                        ActionOutcome::Failed,
                    )
                } else if let Some(error) = refused_directory {
                    (
                        format!("Attempted to run `{}`.", tool_action_marker(&tool_action).1),
                        error.to_string(),
                        ActionOutcome::Failed,
                    )
                } else if matches!(tool_action, ToolAction::AskUser(_)) && !ask_user_offered {
                    // Layer 3 for spec 50: a model that asks anyway, with
                    // nobody to answer, is told to proceed rather than left
                    // waiting on a reply that cannot come.
                    (
                        "Attempted to ask the user a question.".to_string(),
                        NO_USER_CAN_ANSWER.to_string(),
                        ActionOutcome::Failed,
                    )
                } else if matches!(tool_action, ToolAction::FetchUrl { .. }) && !fetch_url_offered {
                    // Layer 3 for spec 51: the offer is withheld when nobody
                    // can approve a host or the cap is 0, and a model that
                    // calls it anyway is told why instead of reading a network
                    // error.
                    (
                        "Attempted to fetch a URL.".to_string(),
                        NO_FETCH_IN_THIS_TURN.to_string(),
                        ActionOutcome::Failed,
                    )
                } else if matches!(action_effect(&tool_action), ActionEffect::ReadOnly) {
                    match precomputed.as_ref() {
                        // Precomputed by the concurrent batch, indexed by the
                        // model's call order.
                        Some(results) => match results[index].clone() {
                            Some(result) => result,
                            None => self.dispatch_read_only_action(
                                repository_root,
                                &session,
                                &task,
                                &tool_action,
                            ),
                        },
                        None => self.dispatch_read_only_action(
                            repository_root,
                            &session,
                            &task,
                            &tool_action,
                        ),
                    }
                } else {
                    match tool_action {
                        ToolAction::Command(command_request) => {
                            let working_directory =
                                self.command_working_directory(repository_root, &command_request);
                            let extra_causes = force_approval_cause
                                .as_ref()
                                .map(|(hook, reason)| {
                                    vec![ApprovalCause::HookRequested {
                                        hook: hook.clone(),
                                        reason: reason.clone(),
                                    }]
                                })
                                .unwrap_or_default();
                            let proposal = self
                                .validation_orchestrator
                                .propose_command_at_with_causes(
                                    repository_root,
                                    &working_directory,
                                    &command_request.command,
                                    &command_request.reason,
                                    extra_causes,
                                )?;

                            if proposal.requires_approval || proposal.blocked {
                                let response = command_proposal_response(&proposal);
                                completed_tool_rounds
                                    .push_round(std::mem::take(&mut round_completed));
                                self.pending_commands.save(&PendingChatTurn {
                                    proposal_id: proposal.id.clone(),
                                    session: session.clone(),
                                    task: task.clone(),
                                    repository_root: repository_root.to_string_lossy().to_string(),
                                    context_files: context_files.clone(),
                                    round: action_rounds,
                                    exploration_rounds,
                                    turn_notes: turn_notes.text.clone(),
                                    turn_notes_carried: turn_notes.carried,
                                    calls: turn_notes.calls.clone(),
                                    completed_tool_rounds: completed_tool_rounds.rounds.clone(),
                                    step_evidence: step_evidence.clone(),
                                    plan_reminder_sent,
                                    messages: messages.clone(),
                                    matched_tool_call: matched_tool_call.clone(),
                                    last_content: redacted.clone(),
                                    turn_options,
                                    reasoning_content: model_run.reasoning_content.clone(),
                                    round_calls_fed_back: round_message.is_some(),
                                    mcp_call: None,
                                    web_diagnostic_call: None,
                                    plan_review: None,
                                    provider_fallback: None,
                                    round_budget: None,
                                    segment: segment_tracker.as_ref().map(|tracker| {
                                        tracker.pause_state(action_rounds, exploration_rounds)
                                    }),
                                    question: None,
                                    check_batch: None,
                                    failure_baseline: None,
                                    fetch_call: None,
                                    context_review: None,
                                })?;
                                self.note_pending_approvals(
                                    &session,
                                    &task,
                                    vec![PendingApproval {
                                        kind: "command".to_string(),
                                        proposal_id: proposal.id.clone(),
                                    }],
                                );
                                // A clean stop for a human decision, not a crash: the
                                // action is finished so the classifier does not read a
                                // dangling marker as an unknown outcome.
                                self.session_store
                                    .finish_action(action_marker, "awaiting_approval")?;
                                let mut proposal_run = model_run;
                                proposal_run.content = response.clone();
                                terminal = Some((
                                    proposal_run,
                                    response,
                                    TurnProposals {
                                        command: Some(agent_command_proposal(
                                            &self.config,
                                            &proposal,
                                        )),
                                        ..Default::default()
                                    },
                                    StopReason::Answered,
                                ));
                                break;
                            }

                            sink.phase(
                                PhaseKind::Tool,
                                running_command_label(&proposal.command),
                                action_rounds,
                                max_rounds,
                                exploration_rounds,
                                max_exploration_rounds,
                                turn_notes.text.chars().count(),
                            );
                            let cancel = sink.cancel;
                            // Borrow only the progress field, so `cancel` can be read
                            // for the same call without two conflicting borrows of the
                            // sink.
                            let on_progress = &mut *sink.on_progress;
                            let mut on_output = |line: &str| {
                                on_progress(TurnProgress::Phase(TurnPhase::new(
                                    PhaseKind::Output,
                                    line,
                                    action_rounds,
                                    max_rounds,
                                    exploration_rounds,
                                    max_exploration_rounds,
                                    turn_notes.text.chars().count(),
                                )));
                            };
                            let record = self.validation_orchestrator.run_proposal(
                                &proposal.id,
                                false,
                                "policy",
                                Some(&task.id),
                                cancel,
                                &mut on_output,
                            )?;
                            command_findings = self.record_command_findings(
                                repository_root,
                                &session,
                                &task,
                                &record.execution,
                            )?;
                            let command_context = sandbox_command_context(&record.execution);
                            // The exit code is in hand right here, one statement before
                            // the marker is finished. Nothing downstream can recover it
                            // — `CommandExecution` is never persisted to the session log
                            // — so it is carried out of the arm rather than looked up.
                            let exit_code = record.execution.exit_code;
                            command_termination = Some(record.execution.termination);
                            command_directory = Some(
                                relative_location(
                                    repository_root,
                                    Path::new(&record.execution.working_directory),
                                )
                                .unwrap_or_else(|| record.execution.working_directory.clone()),
                            );
                            (
                                tool_call_summary(&command_request),
                                command_context,
                                ActionOutcome::CommandExit(exit_code, record.execution.purpose),
                            )
                        }
                        ToolAction::ProposePatch(generated_edit) => {
                            match self
                                .refuse_edit_in_worktree(
                                    repository_root,
                                    generated_edit
                                        .changes
                                        .iter()
                                        .map(|change| change.path.as_str()),
                                )
                                .and_then(|()| {
                                    self.patch_engine.create_patch(
                                        repository_root,
                                        &generated_edit.changes,
                                        Some(&task.id),
                                        &generated_edit.summary,
                                    )
                                })
                                .and_then(|patch| {
                                    self.ensure_patch_not_protected(&session.id, &task.id, &patch)
                                        .map(|()| patch)
                                }) {
                                Ok(patch) => {
                                    // See `ProposedPatch::session_id`: the engine does
                                    // not know the session, this orchestrator does.
                                    let mut patch = patch;
                                    patch.session_id = session.id.clone();
                                    // Spec 102 Task 5: the scope review is part of
                                    // the patch before its first save, so a crash
                                    // can never recover a repair card without it.
                                    let repair_scope = self.repair_scope_review_for_patch(
                                        repository_root,
                                        &session,
                                        &task,
                                        turn_options,
                                        &patch,
                                    )?;
                                    patch.repair_scope = repair_scope;
                                    self.patch_store.save(&patch)?;
                                    let response = patch_proposal_response(&patch);
                                    // A patch waiting for review is a clean stop, not
                                    // a crash — finish the marker before breaking.
                                    self.session_store
                                        .finish_action(action_marker, "awaiting_review")?;
                                    let after = self.dispatch_tool_event(
                                        HookEvent::AfterTool,
                                        &session,
                                        &task,
                                        repository_root,
                                        tool_event_payload(
                                            &self.scanner,
                                            &after_tool_action,
                                            Some(&ActionOutcome::Ok),
                                            None,
                                        ),
                                    );
                                    self.record_after_refusal(
                                        &session.id,
                                        &task.id,
                                        HookEvent::AfterTool,
                                        &after,
                                    );
                                    let proposal = agent_patch_proposal(&patch);
                                    let mut proposal_run = model_run;
                                    proposal_run.content = response.clone();
                                    terminal = Some((
                                        proposal_run,
                                        response,
                                        TurnProposals {
                                            patch: Some(proposal),
                                            ..Default::default()
                                        },
                                        StopReason::Answered,
                                    ));
                                    break;
                                }
                                // Fed back as a tool result rather than aborting the
                                // turn, so the model can see why (e.g. a restricted
                                // or out-of-repo path) and correct itself within the
                                // remaining rounds instead of the turn just failing.
                                Err(error) => (
                                    format!(
                                        "Attempted to propose a patch: {}",
                                        generated_edit.summary
                                    ),
                                    format!("Cannot propose that patch: {error}"),
                                    ActionOutcome::Failed,
                                ),
                            }
                        }
                        ToolAction::ProposePlan(steps) => {
                            // A plan an earlier turn made, carried in and not yet
                            // started on, is the one exception (spec 83 §5.4): the
                            // user may have moved on to other work, and nothing
                            // this turn observed is tied to it yet. It stays
                            // recorded under the task that made it.
                            //
                            // A continuation is different (spec 101 review fix
                            // R9). The user asked to continue this exact
                            // incomplete task, and its carried plan is what
                            // still keeps it from completing. Replacing it here
                            // would drop those open or blocked steps, or its
                            // required check, without the user ever seeing the
                            // replacement, so the proposal is refused while any
                            // remain. A carried plan with nothing outstanding has
                            // no such work to lose and may still be replaced,
                            // which the ordinary review gate covers before its
                            // first mutating action.
                            let continue_protected = turn_options.plan_already_carried
                                && plan.as_ref().is_some_and(carried_plan_has_outstanding_work);
                            // Spec 108 D6: the user reviewed this plan and asked
                            // for changes, so its replacement is exactly what
                            // they asked for, carried or not. It still meets the
                            // review gate before any change.
                            let changes_requested = plan.is_some()
                                && self
                                    .session_store
                                    .plan_changes_requested(&session.id, &task.id)
                                    .unwrap_or(false);
                            let replaceable = plan.is_some()
                                && (changes_requested
                                    || (!continue_protected
                                        && self
                                            .session_store
                                            .carried_plan_untouched(&session.id, &task.id)
                                            .unwrap_or(false)));
                            if plan.is_some() && !replaceable {
                                // Refused rather than replaced. Steps already carry
                                // evidence tied to a state of the repository, and
                                // rewriting the plan underneath that evidence produces
                                // a history that no longer describes what happened —
                                // the same reason §5.5 rules out mid-execution edits.
                                let guidance = if continue_protected {
                                    "This task is continuing a plan carried from the task you were asked to continue, and that plan still has open or blocked work or a required check. Work through the carried steps with complete_step and satisfy the check. Only the user can change which carried steps survive, through plan review. This proposal was not recorded.".to_string()
                                } else {
                                    "This turn already has a plan. Work through its remaining steps with complete_step, or stop and start a new turn if the plan is wrong.".to_string()
                                };
                                (
                                    "Attempted to propose a second plan.".to_string(),
                                    guidance,
                                    ActionOutcome::Failed,
                                )
                            } else {
                                let now = now_millis();
                                let mut proposed = crate::plan::TaskPlan::new(&task.id, now);
                                for (index, step) in steps.iter().enumerate() {
                                    proposed.steps.push(crate::plan::PlanStep {
                                        id: format!("step_{}", index + 1),
                                        // Model-authored text, redacted like any other:
                                        // a title is rendered in the panel and written
                                        // to the log, so a secret echoed into one must
                                        // not survive there.
                                        title: self.scanner.redact(&step.title).text,
                                        detail: step
                                            .detail
                                            .as_ref()
                                            .map(|detail| self.scanner.redact(detail).text),
                                        // The engine's to set, not the model's.
                                        status: if index == 0 {
                                            crate::plan::StepStatus::InProgress
                                        } else {
                                            crate::plan::StepStatus::Pending
                                        },
                                        depends_on: Vec::new(),
                                        started_at_ms: (index == 0).then_some(now),
                                        completed_at_ms: None,
                                        evidence: Vec::new(),
                                        check: normalized_proposed_check(
                                            repository_root,
                                            step.check.clone(),
                                        ),
                                        build: normalized_proposed_check(
                                            repository_root,
                                            step.build.clone(),
                                        ),
                                    });
                                }
                                self.session_store.create_plan(&task, &proposed)?;
                                // A replaced plan's approval was for the old steps.
                                plan_approved = false;
                                // Spec 108 D7: the plan the user approved before
                                // handing it to this worker is not reviewed twice.
                                let handed_note =
                                    self.approve_handed_plan(&session, &task, &proposed)?;
                                if handed_note.is_some() {
                                    plan_approved = true;
                                }
                                sink.plan(&proposed);
                                let summary = format!("Planned {} steps.", proposed.steps.len());
                                let first = proposed.steps[0].title.clone();
                                plan = Some(proposed);
                                let mut result = format!(
                                    "Plan recorded. The current step is: {first}. Call complete_step when its work is done."
                                );
                                if let Some(note) = handed_note {
                                    result.push_str("\n\n");
                                    result.push_str(&note);
                                }
                                (summary, result, ActionOutcome::Ok)
                            }
                        }
                        ToolAction::CompleteStep => match plan.as_mut() {
                            None => (
                                "Attempted to complete a step.".to_string(),
                                "There is no plan for this turn, so there is no step to complete."
                                    .to_string(),
                                ActionOutcome::Failed,
                            ),
                            Some(current) => {
                                // A step can only be closed while it is the one
                                // running, or re-judged while it is the one
                                // blocked. A second `complete_step` after a block
                                // used to find no open step, leave `status` at its
                                // default `Completed`, and start the next pending
                                // step — laundering the block into a handoff
                                // (found in a session on 2026-10-26). It then
                                // became a refusal with no way out, so a block
                                // stood for the rest of the turn even after the
                                // model fixed it (2026-10-29). A blocked step is
                                // now judged again on everything observed since,
                                // by the same rule: it clears only when the
                                // command that failed has since passed.
                                let running = current.steps.iter().position(|step| {
                                    step.status == crate::plan::StepStatus::InProgress
                                });
                                let blocked = current.steps.iter().position(|step| {
                                    step.status == crate::plan::StepStatus::Blocked
                                });
                                match running.or(blocked) {
                                    None => (
                                        "Attempted to complete a step.".to_string(),
                                        "No step is in progress, so there is nothing to complete. Do not call complete_step again.".to_string(),
                                        ActionOutcome::Failed,
                                    ),
                                    Some(open_index) => {
                                        let was_blocked = current.steps[open_index].status
                                            == crate::plan::StepStatus::Blocked;
                                        // Spec 106 Task 6 (live run 3): a step that
                                        // declares a required check does not close
                                        // before that check has run on it, or been
                                        // declined, or been run by the verification
                                        // loop. Closing it anyway left the gate a
                                        // missing check nothing would ever run (it is
                                        // the model's job, review follow-up to R16),
                                        // and the worker stopped. Refusing here is the
                                        // one point where the model can still act.
                                        let unrun_check = if was_blocked {
                                            None
                                        } else {
                                            self.unrun_required_check(
                                                &session.id,
                                                &task.id,
                                                &current.steps[open_index],
                                                &step_evidence,
                                            )?
                                        };
                                        if let Some(check) = unrun_check {
                                            let title = current.steps[open_index].title.clone();
                                            (
                                                "Attempted to complete a step.".to_string(),
                                                format!(
                                                    "Step \"{title}\" declares the required check `{}` ({}), and it has not run on this step. Run exactly that command in that directory with run_command, then call complete_step again. The step stays in progress.",
                                                    check.command,
                                                    if check.working_directory.is_empty() {
                                                        "repository root".to_string()
                                                    } else {
                                                        check.working_directory.clone()
                                                    }
                                                ),
                                                ActionOutcome::Failed,
                                            )
                                        } else {
                                        // The model asked to move on; it does not get
                                        // to say how the step ended. §5.3: the status
                                        // is a function of the evidence, and this is
                                        // the only place a step reaches a terminal
                                        // status.
                                        let accrued = std::mem::take(&mut step_evidence);
                                        let observed_more = !accrued.is_empty();
                                        let now = now_millis();
                                        let open = &mut current.steps[open_index];
                                        // Extends rather than replaces. A step can
                                        // already carry evidence this turn never saw:
                                        // a patch applied after the turn that proposed
                                        // it appends through the log (`edit.rs`), and
                                        // a resumed plan arrives with everything its
                                        // earlier turns recorded. Assigning here would
                                        // silently drop both, and the step would then
                                        // be judged on a fraction of what is known
                                        // about it.
                                        open.evidence.extend(accrued);
                                        let status =
                                            crate::plan::status_from_evidence(&open.evidence);
                                        let finished_title = open.title.clone();
                                        if was_blocked && status == crate::plan::StepStatus::Blocked {
                                            // Still blocked: nothing hands off, and the
                                            // block keeps its time. What was observed
                                            // since is kept, so the next re-check
                                            // weighs it too.
                                            if observed_more {
                                                let kept = open.clone();
                                                self.session_store.update_plan_step(&task, &kept)?;
                                                sink.plan(current);
                                            }
                                            (
                                                "Attempted to complete a step.".to_string(),
                                                format!(
                                                    "No step is in progress. Step \"{finished_title}\" is still blocked: a command it ran failed, and that same command has not succeeded since. Fix the cause, run exactly the same command again (a different command does not clear it), then call complete_step. The remaining steps are still pending."
                                                ),
                                                ActionOutcome::Failed,
                                            )
                                        } else {
                                            open.status = status;
                                            open.completed_at_ms = Some(now);
                                            let closed = open.clone();
                                            self.session_store.update_plan_step(&task, &closed)?;

                                            // A blocked step does not hand off: the next
                                            // step's prerequisite failed, and starting it
                                            // anyway would build on work that did not
                                            // happen.
                                            let mut next_title = None;
                                            if status == crate::plan::StepStatus::Completed
                                                && let Some(next) =
                                                    current.steps.iter_mut().find(|step| {
                                                        step.status
                                                            == crate::plan::StepStatus::Pending
                                                    })
                                            {
                                                next.status = crate::plan::StepStatus::InProgress;
                                                next.started_at_ms = Some(now);
                                                next_title = Some(next.title.clone());
                                                let started = next.clone();
                                                self.session_store
                                                    .update_plan_step(&task, &started)?;
                                            }
                                            // Once, after the handoff rather than after
                                            // each of its two writes: between them the
                                            // closing step is already terminal and the
                                            // next has not opened, so a panel updated
                                            // mid-handoff would blink through a state
                                            // with no current step. The log keeps both
                                            // writes; the panel does not need them.
                                            sink.plan(current);

                                            let done = if was_blocked {
                                                format!(
                                                    "Step \"{finished_title}\" is no longer blocked: the command that failed has since succeeded."
                                                )
                                            } else {
                                                format!("Step \"{finished_title}\" is complete.")
                                            };
                                            let result = match (status, &next_title) {
                                                (crate::plan::StepStatus::Blocked, _) => format!(
                                                    "Step \"{finished_title}\" is blocked: a command it ran did not succeed. Fix the cause and run that same command again; complete_step then checks the step again. The remaining steps are still pending."
                                                ),
                                                (_, Some(next)) => {
                                                    format!("{done} The current step is now: {next}.")
                                                }
                                                (_, None) => format!("{done} That was the last step."),
                                            };
                                            (
                                                format!("Finished: {finished_title}"),
                                                result,
                                                ActionOutcome::Ok,
                                            )
                                        }
                                        }
                                    }
                                }
                            }
                        },
                        ToolAction::EditFile { summary, edits } => {
                            match self
                                .refuse_edit_in_worktree(
                                    repository_root,
                                    edits.iter().map(|edit| edit.path.as_str()),
                                )
                                .and_then(|()| {
                                    region_edits_to_changes(
                                        repository_root,
                                        &self.path_policy,
                                        &edits,
                                    )
                                })
                                .and_then(|changes| {
                                    self.patch_engine.create_patch(
                                        repository_root,
                                        &changes,
                                        Some(&task.id),
                                        &summary,
                                    )
                                })
                                .and_then(|patch| {
                                    self.ensure_patch_not_protected(&session.id, &task.id, &patch)
                                        .map(|()| patch)
                                }) {
                                Ok(patch) => {
                                    // See `ProposedPatch::session_id`: the engine does
                                    // not know the session, this orchestrator does.
                                    let mut patch = patch;
                                    patch.session_id = session.id.clone();
                                    // Spec 102 Task 5: the scope review is part of
                                    // the patch before its first save, so a crash
                                    // can never recover a repair card without it.
                                    let repair_scope = self.repair_scope_review_for_patch(
                                        repository_root,
                                        &session,
                                        &task,
                                        turn_options,
                                        &patch,
                                    )?;
                                    patch.repair_scope = repair_scope;
                                    self.patch_store.save(&patch)?;
                                    let response = patch_proposal_response(&patch);
                                    // A patch waiting for review is a clean stop, not
                                    // a crash — finish the marker before breaking.
                                    self.session_store
                                        .finish_action(action_marker, "awaiting_review")?;
                                    let after = self.dispatch_tool_event(
                                        HookEvent::AfterTool,
                                        &session,
                                        &task,
                                        repository_root,
                                        tool_event_payload(
                                            &self.scanner,
                                            &after_tool_action,
                                            Some(&ActionOutcome::Ok),
                                            None,
                                        ),
                                    );
                                    self.record_after_refusal(
                                        &session.id,
                                        &task.id,
                                        HookEvent::AfterTool,
                                        &after,
                                    );
                                    let proposal = agent_patch_proposal(&patch);
                                    let mut proposal_run = model_run;
                                    proposal_run.content = response.clone();
                                    terminal = Some((
                                        proposal_run,
                                        response,
                                        TurnProposals {
                                            patch: Some(proposal),
                                            ..Default::default()
                                        },
                                        StopReason::Answered,
                                    ));
                                    break;
                                }
                                // Fed back as a tool result rather than aborting the
                                // turn, so the model can see why (a stale anchor, a
                                // restricted path) and correct itself within the
                                // remaining rounds.
                                Err(error) => (
                                    format!("Attempted to edit files: {summary}"),
                                    format!("Cannot apply that edit: {error}"),
                                    ActionOutcome::Failed,
                                ),
                            }
                        }
                        ToolAction::FetchUrl { url } => {
                            let cancel = sink.cancel;
                            match self.dispatch_fetch_url(
                                repository_root,
                                &session,
                                &task,
                                &url,
                                &turn_notes.calls,
                                &messages,
                                cancel,
                            )? {
                                FetchDispatch::Result(summary, content, outcome) => {
                                    (summary, content, outcome)
                                }
                                // Spec 51 §5.3 constraint 1: an unapproved host
                                // pauses before the request is built. The turn
                                // ends cleanly, not as a failure — nothing has
                                // left the machine and the user has a decision
                                // to make.
                                FetchDispatch::NeedsApproval(inspection) => {
                                    let host = inspection.url.host.clone();
                                    let response = format!(
                                        "Fetching from {host} needs your approval before \
                                         anything is requested."
                                    );
                                    let proposal_id = create_id("fetch");
                                    let proposal = AgentFetchProposal {
                                        id: proposal_id.clone(),
                                        host: host.clone(),
                                        url: inspection.url.raw.clone(),
                                        approval_key: inspection.url.approval_key(),
                                        long_components: inspection
                                            .flags
                                            .long_components
                                            .iter()
                                            .map(|flag| FetchComponentFlag {
                                                component: flag.component.as_str().to_string(),
                                                bytes: flag.bytes,
                                            })
                                            .collect(),
                                        query: inspection.url.query.clone(),
                                        fragment: inspection.url.fragment.clone(),
                                    };
                                    completed_tool_rounds
                                        .push_round(std::mem::take(&mut round_completed));
                                    self.pending_commands.save(&PendingChatTurn {
                                        proposal_id: proposal_id.clone(),
                                        session: session.clone(),
                                        task: task.clone(),
                                        repository_root: repository_root
                                            .to_string_lossy()
                                            .to_string(),
                                        context_files: context_files.clone(),
                                        round: action_rounds,
                                        exploration_rounds,
                                        turn_notes: turn_notes.text.clone(),
                                        turn_notes_carried: turn_notes.carried,
                                        calls: turn_notes.calls.clone(),
                                        completed_tool_rounds: completed_tool_rounds.rounds.clone(),
                                        step_evidence: step_evidence.clone(),
                                        plan_reminder_sent,
                                        messages: messages.clone(),
                                        matched_tool_call: matched_tool_call.clone(),
                                        last_content: redacted.clone(),
                                        turn_options,
                                        reasoning_content: model_run.reasoning_content.clone(),
                                        round_calls_fed_back: round_message.is_some(),
                                        mcp_call: None,
                                        web_diagnostic_call: None,
                                        plan_review: None,
                                        provider_fallback: None,
                                        round_budget: None,
                                        segment: segment_tracker.as_ref().map(|tracker| {
                                            tracker.pause_state(action_rounds, exploration_rounds)
                                        }),
                                        question: None,
                                        check_batch: None,
                                        failure_baseline: None,
                                        fetch_call: Some(PendingFetchCall {
                                            host: host.clone(),
                                            url: proposal.url.clone(),
                                            approval_key: proposal.approval_key.clone(),
                                            long_components: proposal.long_components.clone(),
                                            query: proposal.query.clone(),
                                            fragment: proposal.fragment.clone(),
                                        }),
                                        context_review: None,
                                    })?;
                                    self.note_pending_approvals(
                                        &session,
                                        &task,
                                        vec![PendingApproval {
                                            kind: "fetch".to_string(),
                                            proposal_id: proposal_id.clone(),
                                        }],
                                    );
                                    // A clean stop for a human decision, not a
                                    // crash: nothing was dispatched, so the
                                    // marker is finished rather than left
                                    // dangling.
                                    self.session_store
                                        .finish_action(action_marker, "awaiting_approval")?;
                                    let mut proposal_run = model_run;
                                    proposal_run.content = response.clone();
                                    terminal = Some((
                                        proposal_run,
                                        response,
                                        TurnProposals {
                                            fetch: Some(proposal),
                                            ..Default::default()
                                        },
                                        StopReason::Answered,
                                    ));
                                    break;
                                }
                            }
                        }
                        ToolAction::WebDiagnostic(call) => {
                            let call = call.with_context(&session.id, &task.id);
                            // Spec 12 §5.4: the session grant never reaches a
                            // remote URL.
                            let session_approved = if call.is_low_risk() || !call.targets_loopback()
                            {
                                false
                            } else {
                                self.session_store
                                    .browser_diagnostics_allowed_for_session(&session.id)?
                            };
                            if force_approval || (!call.is_low_risk() && !session_approved) {
                                let proposal_id = create_id("webdiag");
                                let proposal = web_diagnostic_approval_proposal(
                                    &proposal_id,
                                    &call,
                                    force_approval_cause.as_ref(),
                                );
                                let response = proposal.prompt.clone();
                                completed_tool_rounds
                                    .push_round(std::mem::take(&mut round_completed));
                                self.pending_commands.save(&PendingChatTurn {
                                    proposal_id,
                                    session: session.clone(),
                                    task: task.clone(),
                                    repository_root: repository_root.to_string_lossy().to_string(),
                                    context_files: context_files.clone(),
                                    round: action_rounds,
                                    exploration_rounds,
                                    turn_notes: turn_notes.text.clone(),
                                    turn_notes_carried: turn_notes.carried,
                                    calls: turn_notes.calls.clone(),
                                    completed_tool_rounds: completed_tool_rounds.rounds.clone(),
                                    step_evidence: step_evidence.clone(),
                                    plan_reminder_sent,
                                    messages: messages.clone(),
                                    matched_tool_call: matched_tool_call.clone(),
                                    last_content: redacted.clone(),
                                    turn_options,
                                    reasoning_content: model_run.reasoning_content.clone(),
                                    round_calls_fed_back: round_message.is_some(),
                                    mcp_call: None,
                                    web_diagnostic_call: Some(PendingWebDiagnosticCall { call }),
                                    plan_review: None,
                                    provider_fallback: None,
                                    round_budget: None,
                                    segment: segment_tracker.as_ref().map(|tracker| {
                                        tracker.pause_state(action_rounds, exploration_rounds)
                                    }),
                                    question: None,
                                    check_batch: None,
                                    failure_baseline: None,
                                    fetch_call: None,
                                    context_review: None,
                                })?;
                                self.note_pending_approvals(
                                    &session,
                                    &task,
                                    vec![PendingApproval {
                                        kind: "browser_diagnostic".to_string(),
                                        proposal_id: proposal.id.clone(),
                                    }],
                                );
                                // A clean stop for a human decision, not a crash: the
                                // action is finished so the classifier does not read a
                                // dangling marker as an unknown outcome.
                                self.session_store
                                    .finish_action(action_marker, "awaiting_approval")?;
                                let mut proposal_run = model_run;
                                proposal_run.content = response.clone();
                                terminal = Some((
                                    proposal_run,
                                    response,
                                    TurnProposals {
                                        command: Some(proposal),
                                        ..Default::default()
                                    },
                                    StopReason::Answered,
                                ));
                                break;
                            }
                            if session_approved {
                                self.audit_log.record(
                                    "browser_diagnostic_session_approval_used",
                                    &[
                                        ("actor", "system".to_string()),
                                        ("sessionId", session.id.clone()),
                                        ("taskId", task.id.clone()),
                                        ("tool", call.name().to_string()),
                                        ("url", call.url.clone()),
                                    ],
                                )?;
                            }

                            let signature = web_diagnostic_signature(&call);
                            let retry_limit = self.config.agent_tool_retry_limit;
                            let (content, outcome) = if failed_browser_calls
                                .get(&signature)
                                .copied()
                                .unwrap_or_default()
                                >= retry_limit
                            {
                                // Refused rather than attempted, because the same call
                                // has already failed its retry limit. Still a failure:
                                // the tool produced no diagnostic.
                                (browser_retry_limit_note(retry_limit), ActionOutcome::Failed)
                            } else {
                                // A page that throws is not a failed call
                                // (spec 12 `context.md` §3.2).
                                let (content, failed) = self.run_and_record_web_diagnostic(
                                    repository_root,
                                    &call,
                                    sink,
                                )?;
                                if failed {
                                    *failed_browser_calls.entry(signature).or_insert(0) += 1;
                                }
                                let outcome = if failed {
                                    ActionOutcome::Failed
                                } else {
                                    ActionOutcome::Ok
                                };
                                (content, outcome)
                            };
                            (web_diagnostic_summary(&call), content, outcome)
                        }
                        ToolAction::McpCall {
                            server_id,
                            tool_name,
                            arguments_json,
                        } => {
                            // MCP tools reach an external service and can have side
                            // effects, so unless the server is marked no-approval we
                            // pause the turn exactly like a command needing approval:
                            // persist state keyed by a fresh proposal id and hand the
                            // user a proposal to accept or decline.
                            if force_approval || mcp.requires_approval(&server_id) {
                                let proposal_id = create_id("mcp");
                                let proposal = mcp_approval_proposal(
                                    &proposal_id,
                                    &server_id,
                                    &tool_name,
                                    &arguments_json,
                                    force_approval_cause.as_ref(),
                                );
                                let response = proposal.prompt.clone();
                                completed_tool_rounds
                                    .push_round(std::mem::take(&mut round_completed));
                                self.pending_commands.save(&PendingChatTurn {
                                    proposal_id,
                                    session: session.clone(),
                                    task: task.clone(),
                                    repository_root: repository_root.to_string_lossy().to_string(),
                                    context_files: context_files.clone(),
                                    round: action_rounds,
                                    exploration_rounds,
                                    turn_notes: turn_notes.text.clone(),
                                    turn_notes_carried: turn_notes.carried,
                                    calls: turn_notes.calls.clone(),
                                    completed_tool_rounds: completed_tool_rounds.rounds.clone(),
                                    step_evidence: step_evidence.clone(),
                                    plan_reminder_sent,
                                    messages: messages.clone(),
                                    matched_tool_call: matched_tool_call.clone(),
                                    last_content: redacted.clone(),
                                    turn_options,
                                    reasoning_content: model_run.reasoning_content.clone(),
                                    round_calls_fed_back: round_message.is_some(),
                                    mcp_call: Some(PendingMcpCall {
                                        server_id,
                                        tool_name,
                                        arguments_json,
                                    }),
                                    web_diagnostic_call: None,
                                    plan_review: None,
                                    provider_fallback: None,
                                    round_budget: None,
                                    segment: segment_tracker.as_ref().map(|tracker| {
                                        tracker.pause_state(action_rounds, exploration_rounds)
                                    }),
                                    question: None,
                                    check_batch: None,
                                    failure_baseline: None,
                                    fetch_call: None,
                                    context_review: None,
                                })?;
                                self.note_pending_approvals(
                                    &session,
                                    &task,
                                    vec![PendingApproval {
                                        kind: "mcp_tool".to_string(),
                                        proposal_id: proposal.id.clone(),
                                    }],
                                );
                                let mut proposal_run = model_run;
                                proposal_run.content = response.clone();
                                terminal = Some((
                                    proposal_run,
                                    response,
                                    TurnProposals {
                                        command: Some(proposal),
                                        ..Default::default()
                                    },
                                    StopReason::Answered,
                                ));
                                break;
                            }

                            // No approval required: run it now and feed the result back.
                            let summary = mcp_call_summary(&server_id, &tool_name);
                            let (content, outcome) =
                                match mcp.call_tool(&server_id, &tool_name, &arguments_json) {
                                    Ok(result) => {
                                        let text = self.scanner.redact(&result.text).text;
                                        // `is_error` is the server's own verdict on its
                                        // call. Reaching the server is not the same as
                                        // the call working, and only the server knows
                                        // which happened.
                                        if result.is_error {
                                            (
                                                format!("MCP tool reported an error:\n{text}"),
                                                ActionOutcome::Failed,
                                            )
                                        } else {
                                            (text, ActionOutcome::Ok)
                                        }
                                    }
                                    // Spec 109 D8: stopped part-way, not failed.
                                    Err(ClientError::Cancelled) => (
                                        "The user stopped this MCP call; its outcome is \
                                         unknown."
                                            .to_string(),
                                        ActionOutcome::Cancelled,
                                    ),
                                    Err(error) => (
                                        format!("MCP tool call failed: {error}"),
                                        ActionOutcome::Failed,
                                    ),
                                };
                            (summary, content, outcome)
                        }
                        // Spec 65 §5.1: notes mutate turn state that only this
                        // loop owns, so they are dispatched here rather than in
                        // the concurrent batch. The text is redacted before it
                        // is measured, stored, logged, sent or counted.
                        ToolAction::TurnNotes { text, replace } => {
                            let limit = self.config.agent_max_turn_notes_chars;
                            if limit == 0 {
                                (
                                    "Attempted to write notes.".to_string(),
                                    "turn_notes is not available in this session.".to_string(),
                                    ActionOutcome::Failed,
                                )
                            } else {
                                let redacted = self.scanner.redact(&text).text;
                                let chars = redacted.chars().count();
                                let current = turn_notes.text.chars().count();
                                let separator = if turn_notes.text.is_empty() { 0 } else { 2 };
                                let fits = if replace {
                                    chars <= limit
                                } else {
                                    current + separator + chars <= limit
                                };
                                if !fits {
                                    let detail = if replace {
                                        format!(
                                            "The replacement is {chars} characters, over the \
                                             {limit}-character limit; shorten it."
                                        )
                                    } else {
                                        format!(
                                            "Adding {chars} more characters to {current} would \
                                             exceed the {limit}-character limit; rewrite the \
                                             notes shorter with replace."
                                        )
                                    };
                                    (
                                        "Attempted to write notes.".to_string(),
                                        format!("notes: {current} / {limit} characters. {detail}"),
                                        ActionOutcome::Failed,
                                    )
                                } else {
                                    if replace {
                                        turn_notes.text = redacted;
                                        turn_notes.carried = false;
                                    } else {
                                        if !turn_notes.text.is_empty() {
                                            turn_notes.text.push_str("\n\n");
                                        }
                                        turn_notes.text.push_str(&redacted);
                                    }
                                    let total = turn_notes.text.chars().count();
                                    // The per-change log event carries the text
                                    // (already redacted); the audit carries only
                                    // sizes, never text (requirement 11).
                                    self.session_store.record_turn_notes(
                                        &session.id,
                                        &task.id,
                                        replace,
                                        turn_notes.carried,
                                        &turn_notes.text,
                                        chars,
                                    )?;
                                    self.audit_log.record(
                                        "turn_notes_recorded",
                                        &[
                                            ("actor", "model".to_string()),
                                            ("sessionId", session.id.clone()),
                                            ("taskId", task.id.clone()),
                                            ("replace", replace.to_string()),
                                            ("carried", turn_notes.carried.to_string()),
                                            ("chars", chars.to_string()),
                                            ("totalChars", total.to_string()),
                                        ],
                                    )?;
                                    (
                                        if replace {
                                            "Updated notes.".to_string()
                                        } else {
                                            "Wrote notes.".to_string()
                                        },
                                        format!("notes: {total} / {limit} characters."),
                                        ActionOutcome::Ok,
                                    )
                                }
                            }
                        }
                        // Spec 29 §5.11: a suggestion is a proposal at most.
                        // The gate screens it; the user confirms through a
                        // separate route. A refusal here is not an error the
                        // model can fix by rephrasing a credential.
                        ToolAction::ProposeMemory {
                            statement,
                            category,
                            scope,
                            source,
                            evidence_path,
                        } => {
                            let origin = if evidence_path.is_some() {
                                ProposalOrigin::RepositoryObservation {
                                    session_id: session.id.clone(),
                                }
                            } else if let Some(source) = source {
                                ProposalOrigin::ExternalContent {
                                    session_id: session.id.clone(),
                                    source,
                                }
                            } else {
                                ProposalOrigin::ModelSuggestion {
                                    session_id: session.id.clone(),
                                    task_id: task.id.clone(),
                                }
                            };
                            let evidence = evidence_path
                                .as_deref()
                                .map(|path| vec![MemoryEvidence::new(path, String::new())])
                                .unwrap_or_default();
                            let proposal = MemoryProposal::new(
                                scope.unwrap_or(MemoryScope::Project),
                                category,
                                statement,
                                origin,
                                evidence,
                                Some(task.id.clone()),
                            );
                            match self
                                .memory_store
                                .propose_with_evidence(repository_root, proposal)
                            {
                                Ok(Some(_)) => (
                                    "Suggested a memory.".to_string(),
                                    "A memory proposal was created for the user to confirm. It \
                                     is not stored until they confirm it."
                                        .to_string(),
                                    ActionOutcome::Ok,
                                ),
                                Ok(None) => (
                                    "Attempted to suggest a memory.".to_string(),
                                    "Damaian did not propose that memory: the memory policy \
                                     refused it, so the user sees no card."
                                        .to_string(),
                                    ActionOutcome::Failed,
                                ),
                                Err(error) => (
                                    "Attempted to suggest a memory.".to_string(),
                                    format!("The proposal could not be recorded: {error}"),
                                    ActionOutcome::Failed,
                                ),
                            }
                        }
                        ToolAction::AskUser(input) => {
                            // Requirement 5: the cap is read from the log, so a
                            // resumed turn counts what it already asked and an
                            // in-memory counter cannot reset it.
                            let asked = self
                                .session_store
                                .questions_asked_in_task(&session.id, &task.id)?;
                            let cap = self.config.agent_max_questions_per_turn as usize;
                            if asked >= cap {
                                (
                                    "Attempted to ask the user a question.".to_string(),
                                    format!(
                                        "A question was not asked: this turn has already asked \
                                         {asked} of {cap} questions. Proceed without asking more, \
                                         and say in your answer what you assumed and why."
                                    ),
                                    ActionOutcome::Failed,
                                )
                            } else {
                                // Requirement 7: the question is model-authored
                                // and may be quoting repository content, so it
                                // is redacted before it is stored, shown or
                                // sent anywhere.
                                let redacted_input = AskUserInput {
                                    question: self.scanner.redact(&input.question).text,
                                    options: input
                                        .options
                                        .iter()
                                        .map(|option| self.scanner.redact(option).text)
                                        .collect(),
                                    allow_free_text: input.allow_free_text,
                                };
                                if let Err(error) = redacted_input.validate() {
                                    // Redaction can lengthen a question enough
                                    // to break a bound. That is still the
                                    // model's to fix, not a reason to fail the
                                    // turn.
                                    (
                                        "Attempted to ask the user a question.".to_string(),
                                        format!("The question was not asked: {error}"),
                                        ActionOutcome::Failed,
                                    )
                                } else {
                                    let question = PendingQuestionRef::new();
                                    self.session_store.record_question_asked(
                                        &session.id,
                                        &task.id,
                                        &question,
                                        &redacted_input,
                                    )?;
                                    completed_tool_rounds
                                        .push_round(std::mem::take(&mut round_completed));
                                    self.pending_commands.save(&PendingChatTurn {
                                        proposal_id: question.question_id().to_string(),
                                        session: session.clone(),
                                        task: task.clone(),
                                        repository_root: repository_root
                                            .to_string_lossy()
                                            .to_string(),
                                        context_files: context_files.clone(),
                                        round: action_rounds,
                                        exploration_rounds,
                                        turn_notes: turn_notes.text.clone(),
                                        turn_notes_carried: turn_notes.carried,
                                        calls: turn_notes.calls.clone(),
                                        completed_tool_rounds: completed_tool_rounds.rounds.clone(),
                                        step_evidence: step_evidence.clone(),
                                        plan_reminder_sent,
                                        messages: messages.clone(),
                                        matched_tool_call: matched_tool_call.clone(),
                                        last_content: redacted.clone(),
                                        turn_options,
                                        reasoning_content: model_run.reasoning_content.clone(),
                                        round_calls_fed_back: round_message.is_some(),
                                        mcp_call: None,
                                        web_diagnostic_call: None,
                                        plan_review: None,
                                        provider_fallback: None,
                                        round_budget: None,
                                        segment: segment_tracker.as_ref().map(|tracker| {
                                            tracker.pause_state(action_rounds, exploration_rounds)
                                        }),
                                        question: Some(PendingQuestion {
                                            question_id: question.question_id().to_string(),
                                            kind: PendingQuestionKind::Agent,
                                        }),
                                        check_batch: None,
                                        failure_baseline: None,
                                        fetch_call: None,
                                        context_review: None,
                                    })?;
                                    // A clean stop for a human decision, not a
                                    // crash: the marker says what it waits on.
                                    self.session_store
                                        .finish_action(action_marker, "awaiting_answer")?;
                                    let response = QUESTION_PAUSED_RESPONSE.to_string();
                                    let mut proposal_run = model_run;
                                    proposal_run.content = response.clone();
                                    terminal = Some((
                                        proposal_run,
                                        response,
                                        TurnProposals {
                                            question: Some(AgentQuestionProposal {
                                                question_id: question.question_id().to_string(),
                                                question: redacted_input.question,
                                                options: redacted_input.options,
                                                allow_free_text: redacted_input.allow_free_text,
                                            }),
                                            ..Default::default()
                                        },
                                        StopReason::Answered,
                                    ));
                                    break;
                                }
                            }
                        }
                        other => {
                            unreachable!("read-only action reached the sequential match: {other:?}")
                        }
                    }
                };
                // A stop that reached the action itself (spec 109 D8): an MCP
                // call or a hook ended part-way. What it did is unknown, so the
                // marker says `unknown` (spec 17), nothing is retried, and the
                // turn ends cancelled.
                if action_outcome == ActionOutcome::Cancelled {
                    self.session_store
                        .finish_action_stopped_by_user(action_marker)?;
                    break;
                }
                // §5.3: a dispatched read joins the turn's seen-call set, so an
                // identical call in a later round is a repeat. A write marks the
                // round to clear the set once it completes: checking Git state
                // again after changing it is verification, not a loop.
                if let Some(key) = dispatched_call_key.clone() {
                    seen_read_calls.insert(key);
                }
                if writes_this_call {
                    wrote_this_round = true;
                }
                // Typed tracker metadata for the next request's safety pins and
                // manifests. Captured before `action_outcome` is consumed below.
                let tracked_completed = matched_tool_call.as_ref().and_then(|call| {
                    tracked_completed_call(
                        call,
                        &after_tool_action,
                        &action_outcome,
                        open_step_id.clone(),
                        dispatched_call_key.clone(),
                    )
                });
                // Accrued before the marker is consumed, since the marker id is
                // what ties this evidence back to the action in the log. Evidence
                // belongs to whichever step is open; with no plan there is nothing
                // to attach it to and this is a no-op.
                if plan.is_some()
                    && let Some(evidence) = evidence_for(
                        &action_outcome,
                        action_marker.id(),
                        action_command(&after_tool_action),
                        command_directory.as_deref(),
                    )
                {
                    step_evidence.push(evidence);
                }
                // After the exit it explains (spec 22 `context.md` §14).
                if plan.is_some()
                    && let Some(findings) = command_findings.take()
                {
                    step_evidence.push(findings);
                }
                // One finish per dispatch, on the tool's own answer. A command goes
                // through `finish_command_action` so the outcome is derived from
                // the exit code rather than passed beside it — the two cannot then
                // disagree in the log.
                match action_outcome {
                    // Spec 109 D8: a command the Stop killed part-way is named
                    // in the stopped turn's report; one that timed out is not.
                    ActionOutcome::CommandExit(_, _)
                        if command_termination == Some(CommandTermination::Cancelled) =>
                    {
                        self.session_store
                            .finish_action_stopped_by_user(action_marker)?
                    }
                    ActionOutcome::CommandExit(exit_code, _) => self
                        .session_store
                        .finish_command_action(action_marker, exit_code)?,
                    // A successful read is an `ok` dispatch like any other; the
                    // path and hash it carries are for the step's evidence, not
                    // for the marker.
                    ActionOutcome::Ok | ActionOutcome::FileRead { .. } => {
                        self.session_store.finish_action(action_marker, "ok")?
                    }
                    ActionOutcome::Failed => {
                        self.session_store.finish_action(action_marker, "failed")?
                    }
                    // Unreachable after the early break above, kept so the
                    // match stays exhaustive if that guard ever changes.
                    ActionOutcome::Cancelled => self
                        .session_store
                        .finish_action(action_marker, "cancelled")?,
                }

                // Spec 32 `context.md` §3: after the action's marker is
                // finished. A mode/profile refusal dispatched nothing and has
                // no event; a hook refusal returned before the marker.
                if permission.is_allowed() {
                    let payload = tool_event_payload(
                        &self.scanner,
                        &after_tool_action,
                        Some(&action_outcome),
                        command_termination,
                    );
                    let after = self.dispatch_tool_event(
                        HookEvent::AfterTool,
                        &session,
                        &task,
                        repository_root,
                        payload,
                    );
                    if let Some(note) = hook_after_note(&after) {
                        tool_result_text.push_str("\n\n");
                        tool_result_text.push_str(&note);
                    }
                    self.record_after_refusal(&session.id, &task.id, HookEvent::AfterTool, &after);
                }

                // Persist the tool call and its result so later turns in this
                // session can still see it (previously this context was
                // discarded once the turn finished).
                self.session_store.append_message(
                    &session.id,
                    Some(&task.id),
                    "assistant",
                    &assistant_summary,
                )?;
                self.session_store.append_message(
                    &session.id,
                    Some(&task.id),
                    "tool",
                    &tool_result_text,
                )?;

                // Feed this call back before the next one in the round, so a
                // terminal break still carries everything that already ran.
                // The round is one assistant message carrying the model's prose
                // and reasoning once, and it lists only calls that ran: each
                // call joins it as its result lands, so `messages` is a valid
                // request at every break. Splitting it per call dropped the
                // reasoning from all but the first, and DeepSeek's thinking mode
                // rejects the next request over that (spec 47 §7).
                if let Some(call) = &matched_tool_call {
                    match round_message {
                        Some(index) => messages[index].tool_calls.push(call.clone()),
                        None => {
                            round_message = Some(messages.len());
                            messages.push(ModelMessage::assistant_with_tool_calls(
                                redacted.clone(),
                                vec![call.clone()],
                                round_reasoning.clone(),
                            ));
                        }
                    }
                    messages.push(ModelMessage::tool(
                        call.id.clone(),
                        tool_result_text.clone(),
                    ));
                } else {
                    // Only reachable for `ToolAction::Command` via the
                    // `DAMAIAN_COMMAND_V1` text-envelope fallback — every other
                    // action only exists as a native tool call.
                    messages.push(
                        ModelMessage::assistant(redacted.clone())
                            .with_reasoning_content(model_run.reasoning_content.clone()),
                    );
                    messages.push(ModelMessage::user(format!(
                        "Command result:\n{tool_result_text}"
                    )));
                }
                if let Some(tracked) = tracked_completed {
                    round_completed.push(tracked);
                }
                // Spec 65 §5.3: the durable per-turn record the notice, the
                // warning and a resumed turn's seen-call set read. A call that
                // opened no assistant message (the text-envelope command) has
                // no stable index to record against.
                if dispatched && let Some(message_index) = round_message {
                    turn_notes.calls.push(RecordedCall {
                        round: round_number,
                        message_index,
                        line: call_line.unwrap_or_default(),
                        key: dispatched_call_key,
                        writes: writes_this_call,
                        compacted: false,
                    });
                }
            }
            // A terminal arm may already have moved this round into the pause;
            // an ordinary round reaches here with the calls still to publish.
            completed_tool_rounds.push_round(std::mem::take(&mut round_completed));

            if let Some(finished) = terminal {
                break finished;
            }

            // A call in the same round that could not be decoded is reported
            // after the ones that ran, as plain text: its arguments cannot be
            // replayed and providers reject a tool result whose call never
            // parsed.
            if let Some((undecodable, note)) = undecodable_calls.first() {
                let summary = format!(
                    "Attempted to call `{}`, but the request could not be decoded.",
                    undecodable.name
                );
                self.session_store.append_message(
                    &session.id,
                    Some(&task.id),
                    "assistant",
                    &summary,
                )?;
                self.session_store
                    .append_message(&session.id, Some(&task.id), "tool", note)?;
                // Still this response's turn, so it carries the reasoning:
                // thinking mode rejects any assistant message that lost it.
                messages
                    .push(ModelMessage::assistant(summary).with_reasoning_content(round_reasoning));
                messages.push(ModelMessage::user(note.clone()));
            }

            // §5.3: a write changes the state Git and file reads report, so
            // checking it again in a later round is the ordinary verify step.
            // A terminal round breaks above and never reaches here; every
            // resume starts with an empty set anyway.
            if wrote_this_round {
                seen_read_calls.clear();
            }

            // §5.2: the round is charged to whichever budget classification
            // chose. `round_is_exploration` already required every decoded call
            // to be a read, no undecodable call, no repeat, and room in the
            // allowance, so this is the whole charging rule.
            if round_is_exploration {
                exploration_rounds += 1;
            } else {
                action_rounds += 1;
            }
        };

        // A turn that ends awaiting a decision records *which* proposal, so a
        // restart reattaches it instead of rebuilding a card from partial data
        // (§5.5).
        let pending = proposals
            .command
            .as_ref()
            .map(|proposal| crate::session::PendingApprovalRef {
                kind: "command".to_string(),
                proposal_id: proposal.id.clone(),
            })
            .or_else(|| {
                proposals
                    .patch
                    .as_ref()
                    .map(|proposal| crate::session::PendingApprovalRef {
                        kind: "patch".to_string(),
                        proposal_id: proposal.patch_id.clone(),
                    })
            })
            .or_else(|| {
                proposals
                    .plan
                    .as_ref()
                    .map(|proposal| crate::session::PendingApprovalRef {
                        kind: "plan".to_string(),
                        proposal_id: proposal.id.clone(),
                    })
            })
            .or_else(|| {
                proposals
                    .fetch
                    .as_ref()
                    .map(|proposal| crate::session::PendingApprovalRef {
                        kind: "fetch".to_string(),
                        proposal_id: proposal.id.clone(),
                    })
            });
        let pending_question = proposals
            .question
            .as_ref()
            .map(|proposal| PendingQuestionRef::recorded(proposal.question_id.clone()));
        let truly_completed = pending.is_none() && pending_question.is_none();

        // Spec 103 Task 6 with spec 102 precedence: settle the typed outcome
        // before its rendering is written anywhere. When a repair stall for this
        // exact task already won the atomic settlement race, the store keeps
        // that result; the response, audit status, control and presentation must
        // then describe the stall, not the outcome this turn was about to claim.
        let mut task_terminal_outcome: Option<TaskTerminalOutcome> = None;
        let mut terminal_settled: Option<Task> = None;
        let mut terminal_verification_report = None;
        if let StopReason::TaskTerminal(outcome) = &stop_reason {
            let settlement = self.session_store.settle_task_terminal_outcome_checked(
                &task,
                outcome.task_status(),
                outcome,
            )?;
            if settlement.preserved_repair_stall {
                let (stall_response, report) = self.verification_stall_presentation(
                    repository_root,
                    &session,
                    &settlement.task,
                )?;
                response = stall_response;
                final_run.content = response.clone();
                terminal_verification_report = Some(report);
                task = CompletionGateService::new(&self.session_store, repository_root)
                    .settle(&settlement.task, CompletionSettlement::VerificationFailed)?
                    .task;
                terminal_settled = Some(task.clone());
            } else {
                self.audit_log
                    .record("task_terminal_outcome", &outcome.audit_fields())?;
                task_terminal_outcome = self
                    .session_store
                    .read_task_outcome(&session.id, &settlement.task.id)?;
                task = settlement.task;
                terminal_settled = Some(task.clone());
            }
        }

        // Spec 66 `context.md` D4: on a true completion the answer that
        // triggered the reminder is the turn's response, and the post-reminder
        // answer follows it as a second message. Only a genuine answer does
        // this: a pause, a round/token budget stop or a Task 6 terminal outcome
        // keeps its own response, because replacing it with the earlier answer
        // would hide the card or the engine-owned stop the user is being shown.
        // The earlier answer is already in the transcript: `send_plan_reminder`
        // wrote it before the stop, so it is preserved without becoming the
        // turn's response.
        match pre_reminder_answer.take() {
            Some(earlier) if truly_completed && matches!(stop_reason, StopReason::Answered) => {
                if !response.trim().is_empty() {
                    self.session_store.append_message(
                        &session.id,
                        Some(&task.id),
                        "assistant",
                        &response,
                    )?;
                }
                response = earlier;
            }
            _ => {
                self.session_store.append_message(
                    &session.id,
                    Some(&task.id),
                    "assistant",
                    &response,
                )?;
            }
        }

        if truly_completed {
            // Attaching the buffer is the true end's job; a pause carried it
            // instead (spec 66 §5.3). Nothing here changes a step's status.
            // A Task 6 stop that lost to a spec 102 repair stall records the
            // authoritative status, not the kind it never settled.
            let stop_label = match &task_terminal_outcome {
                Some(outcome) => outcome.kind.as_str(),
                None if matches!(stop_reason, StopReason::TaskTerminal(_)) => task.status.as_str(),
                None => stop_reason.as_str(),
            };
            self.settle_plan_at_turn_end(
                &session,
                &task,
                &mut plan,
                &mut step_evidence,
                plan_reminder_sent,
                stop_label,
            )?;
            if let Some(current) = plan.as_ref() {
                sink.plan_finished(current);
            }
        } else if proposals.patch.is_some() {
            // A patch proposal is the only pending approval that saves no
            // paused turn: both stops that raise one — `propose_patch` and
            // `edit_file` — break with a patch in `proposals`. The turn is
            // pausing, so attach what it observed before the patch's own
            // `PatchApplied` can land, and attach only: no audit, no reminder,
            // no status change. Without this the failing command a repair turn
            // ran just before proposing its fix is lost, and the step closes
            // on evidence it never saw.
            self.attach_step_evidence(&session, &task, &mut plan, &mut step_evidence);
            if let Some(current) = plan.as_ref() {
                sink.plan(current);
            }
        }

        let mut completion_verification = false;
        task = match (&pending, &pending_question) {
            (Some(pending), None) => self.session_store.await_approval(&task, pending)?,
            // A question link goes under its own status key through
            // `await_answer`; never through `await_approval` (spec 50 §5.2).
            (None, Some(question)) => self.session_store.await_answer(&task, question)?,
            (None, None) => {
                match &stop_reason {
                    // Spec 101: an answer is settled through the engine gate, so
                    // `complete` cannot be asserted by the model or reached with
                    // open plan work, a due verification or pending evidence.
                    StopReason::Answered => {
                        let settled =
                            CompletionGateService::new(&self.session_store, repository_root)
                                .settle(&task, CompletionSettlement::Gate)?;
                        // Review follow-up to R16: the task-completion floor's
                        // enclosing roots are not run per change, and a plan
                        // that declares any required check must not suppress
                        // the floor. Ask the gate service which floor checks
                        // are missing or stale and are not plan-declared; run
                        // exactly those through the ordinary check policy. A
                        // plan-declared check stays the model's job.
                        let gate = CompletionGateService::new(&self.session_store, repository_root);
                        let mut needed_floor_checks =
                            gate.floor_checks_needing_verification(&session.id, &settled.task.id)?;
                        // Spec 106 Task 6: once no plan work is open, a finished
                        // step's stale required check is rerun the same way, or
                        // the task would stop on evidence nothing the model can
                        // run is able to refresh. Mid-plan, the next patch's
                        // verification reruns it instead. A check that never ran
                        // stays the model's job (review follow-up to R16).
                        let plan_work_open = settled.report.reason_codes().iter().any(|code| {
                            matches!(
                                code,
                                CompletionReasonCode::OpenPlanSteps
                                    | CompletionReasonCode::BlockedPlanSteps
                            )
                        });
                        if !plan_work_open {
                            for check in
                                gate.plan_checks_needing_rerun(&session.id, &settled.task.id)?
                            {
                                if !needed_floor_checks.contains(&check) {
                                    needed_floor_checks.push(check);
                                }
                            }
                        }
                        if !needed_floor_checks.is_empty() {
                            let paths = self
                                .session_store
                                .task_touched_paths(&session.id, &settled.task.id)?;
                            self.session_store.append_verification_event(
                                &session.id,
                                &VerificationEvent::Due {
                                    task_id: settled.task.id.clone(),
                                    patch_id: String::new(),
                                    paths,
                                    checks: needed_floor_checks,
                                },
                            )?;
                            completion_verification = true;
                        }
                        settled.task
                    }
                    StopReason::ToolBudget => self.session_store.update_task_status(
                        &task,
                        TaskStatus::ToolBudgetExhausted,
                        None,
                    )?,
                    StopReason::TokenBudget => self.session_store.update_task_status(
                        &task,
                        TaskStatus::TokenBudgetExhausted,
                        None,
                    )?,
                    // Spec 103 Task 6: the status and the typed outcome are one
                    // event, already settled above so a spec 102 repair stall
                    // keeps the result it recorded first.
                    StopReason::TaskTerminal(outcome) => match terminal_settled.take() {
                        Some(settled) => settled,
                        None => {
                            let settled = self.session_store.settle_task_terminal_outcome(
                                &task,
                                outcome.task_status(),
                                outcome,
                            )?;
                            task_terminal_outcome = self
                                .session_store
                                .read_task_outcome(&session.id, &settled.id)?;
                            settled
                        }
                    },
                }
            }
            (Some(_), Some(_)) => {
                unreachable!("a turn cannot pause for an approval and a question at once")
            }
        };
        self.audit_log.record(
            "model_response_completed",
            &[
                ("actor", "model".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("provider", final_run.provider.clone()),
                ("model", final_run.model.clone()),
                (
                    "status",
                    if proposals.command.is_some() {
                        "command_approval_required".to_string()
                    } else if proposals.patch.is_some() {
                        "patch_proposal_ready".to_string()
                    } else if proposals.plan.is_some() {
                        "plan_review_required".to_string()
                    } else if proposals.question.is_some() {
                        "question_pending".to_string()
                    } else if proposals.fetch.is_some() {
                        "fetch_approval_required".to_string()
                    } else if stop_reason == StopReason::ToolBudget {
                        "tool_budget_exhausted".to_string()
                    } else if stop_reason == StopReason::TokenBudget {
                        "token_budget_exhausted".to_string()
                    } else if let Some(outcome) = &task_terminal_outcome {
                        outcome.kind.as_str().to_string()
                    } else if matches!(stop_reason, StopReason::TaskTerminal(_)) {
                        // Spec 102's repair stall won the settlement race. The
                        // durable status is the result; no Task 6 outcome was
                        // written, so the audit may not name its kind.
                        task.status.as_str().to_string()
                    } else if task.status == TaskStatus::Incomplete {
                        // The persisted verdict, not the fact that the model
                        // answered, is what the audit records (spec 101).
                        "incomplete".to_string()
                    } else if action_rounds > 0 {
                        "complete_with_command".to_string()
                    } else {
                        "complete".to_string()
                    },
                ),
            ],
        )?;
        // A turn waiting on a command decision is not over: it resumes through
        // `resume_after_command_decision` and seals then. A plan review, a
        // question and a fetch approval are the same shape — nothing has
        // happened yet and the turn continues through
        // `resume_after_plan_decision`, `resume_after_answer` or
        // `resume_after_fetch_decision`. Anything else has left the repository
        // in the state a rewind must compare against.
        if proposals.command.is_none()
            && proposals.plan.is_none()
            && proposals.question.is_none()
            && proposals.fetch.is_none()
        {
            self.seal_turn_checkpoint(repository_root, &session, &task);
        }
        if completion_verification {
            // The answer already settled `incomplete` for the missing floor
            // check; this continuation is what runs it and records the real
            // verdict. Keeping the answer in front of the verification's report
            // preserves what the model said without letting it replace the
            // evidence (the same shape as a command-made verification due).
            let answer = response.clone();
            let mut verified =
                self.verify_due(repository_root, &session.id, model_adapter, sink)?;
            if !answer.trim().is_empty() {
                verified.response = format!("{answer}\n\n{}", verified.response);
            }
            return Ok(verified);
        }
        // From the log, not from a running total: the number the client shows
        // now must be the number it shows after a reload.
        let usage = self
            .session_store
            .read_task_usage(&session.id)
            .ok()
            .and_then(|usage| usage.get(&task.id).cloned());
        let estimated_cost = usage.as_ref().and_then(|usage| {
            self.config.estimated_cost(&TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                source: usage.source,
            })
        });
        self.offer_worker_once(&session, &task, &proposals)?;
        // Spec 107 D3: an ordinary agentic Code turn that pauses on a patch
        // with no plan of its own does not end there. Recorded once per task
        // chain, before the patch can be applied and verified.
        if proposals.patch.is_some()
            && mode == SessionMode::Code
            && !turn_options.verification_turn
            && plan.as_ref().is_none_or(|plan| !has_primary_steps(plan))
            && !self
                .session_store
                .planless_continues(&session.id, &task.id)?
        {
            self.session_store
                .record_planless_continuation(&session.id, &task.id)?;
        }
        let task_control = self.task_control_for(&task);
        let turn_presentation = self.turn_presentation_for(
            &task,
            final_run.incomplete,
            false,
            task_terminal_outcome.as_ref(),
        );
        Ok(ChatTurnResult {
            session,
            task,
            model_run: final_run,
            context_files,
            response,
            command_proposal: proposals.command,
            patch_proposal: proposals.patch,
            plan_proposal: proposals.plan,
            question_proposal: proposals.question,
            fetch_proposal: proposals.fetch,
            fallback_proposal: None,
            round_budget_proposal: None,
            check_batch: proposals.checks,
            context_review: None,
            verification_report: terminal_verification_report,
            turn_notes: turn_notes.view(),
            cancelled: false,
            usage,
            estimated_cost,
            task_control,
            turn_presentation,
        })
    }

    /// Closes out a turn the user stopped.
    ///
    /// The single place `ClientError::Cancelled` is turned back into a result:
    /// it persists whatever was generated, marks the task terminal so no
    /// `Running` record is left wedged, and reports the stop as an outcome
    /// rather than a failure — a stop and a provider error need to stay
    /// distinguishable in the badge and in the task history alike.
    #[allow(clippy::too_many_arguments)]
    fn finish_cancelled_turn(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        context_files: Vec<String>,
        partial: &str,
        turn_notes: &TurnNotesState,
        model_run: ModelRun,
        mut plan: Option<TaskPlan>,
        mut step_evidence: Vec<crate::plan::Evidence>,
        plan_reminder_sent: bool,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        // Through the same scanner as a completed turn: stopping does not
        // suspend the redaction guarantee.
        let mut response = self.scanner.redact(partial).text;
        if !response.is_empty() {
            self.session_store.append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                &response,
            )?;
        }
        // Spec 109 D8: the turn's report names what the stop cut short.
        if let Some(note) = self
            .session_store
            .action_stopped_by_user(&session.id, &task.id)?
            .map(|(action, reference)| stopped_action_note(&action, &reference))
        {
            let note = self.scanner.redact(&note).text;
            self.session_store
                .append_message(&session.id, Some(&task.id), "assistant", &note)?;
            if !response.is_empty() {
                response.push_str("\n\n");
            }
            response.push_str(&note);
        }
        // A stop is a true end: the step stays open, with what it ran attached
        // (spec 66 §5.3). It never changes the step's status.
        self.settle_plan_at_turn_end(
            &session,
            &task,
            &mut plan,
            &mut step_evidence,
            plan_reminder_sent,
            "cancelled",
        )?;
        if let Some(current) = plan.as_ref() {
            sink.plan_finished(current);
        }
        let task = self
            .session_store
            .update_task_status(&task, TaskStatus::Cancelled, None)?;
        self.audit_log.record(
            "chat_turn_cancelled",
            &[
                ("actor", "user".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("partialLength", response.len().to_string()),
            ],
        )?;
        // A stopped turn may still have applied a patch or run a command
        // before it stopped, so what it left behind is what a rewind has to
        // compare against.
        self.seal_turn_checkpoint(repository_root, &session, &task);
        let usage = self
            .session_store
            .read_task_usage(&session.id)
            .ok()
            .and_then(|usage| usage.get(&task.id).cloned());
        let estimated_cost = usage.as_ref().and_then(|usage| {
            self.config.estimated_cost(&TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                source: usage.source,
            })
        });
        let task_control = self.task_control_for(&task);
        let turn_presentation = self.turn_presentation_for(&task, false, true, None);
        Ok(ChatTurnResult {
            session,
            task,
            model_run,
            context_files,
            response,
            command_proposal: None,
            patch_proposal: None,
            plan_proposal: None,
            question_proposal: None,
            fetch_proposal: None,
            fallback_proposal: None,
            round_budget_proposal: None,
            check_batch: None,
            context_review: None,
            verification_report: None,
            turn_notes: turn_notes.view(),
            cancelled: true,
            usage,
            estimated_cost,
            task_control,
            turn_presentation,
        })
    }
}
