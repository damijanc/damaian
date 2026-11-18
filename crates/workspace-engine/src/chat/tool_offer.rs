use super::*;

/// The tools one turn offers the model, decided once before its first
/// request (spec 20 Layer 1, `proposal.md` §5.2).
pub(super) struct ToolOffer {
    /// Whether `ask_user` is offered. Layer 3 refuses a call where it is not,
    /// reading this same value, so the offer and the refusal cannot disagree.
    pub(super) ask_user_offered: bool,
    /// Whether `fetch_url` is offered, read by Layer 3 the same way.
    pub(super) fetch_url_offered: bool,
    /// `None` for a provider without native tool calls.
    pub(super) native_tools: Option<Vec<ToolDefinition>>,
}

impl ChatOrchestrator {
    /// Filters every tool definition through the one permission matrix for
    /// the mode captured at the top of the turn. Withholding a tool anywhere
    /// else would be a second copy of the matrix.
    pub(super) fn tool_offer(
        &self,
        mode: SessionMode,
        turn_options: ChatTurnOptions,
        mcp: &mut McpRuntime,
    ) -> ToolOffer {
        let browser_mcp_server_ids = self.browser_diagnostic_mcp_server_ids();
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
        ToolOffer {
            ask_user_offered,
            fetch_url_offered,
            native_tools,
        }
    }
}
