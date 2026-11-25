use super::*;
use crate::mcp_inventory::{
    MCP_TOOL_SEARCH_MAX_MATCHES, McpDeferralDecision, McpToolInventory, McpToolSurface,
    decide_deferral, search_inventory,
};
use crate::profile::ProfileCapabilities;

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
    /// The MCP tools the turn's messages already name — a resumed turn's
    /// calls and search matches — whose schemas the offer includes (spec 33
    /// §5.1 stage 2).
    pub(super) named_mcp_tools: BTreeSet<McpToolKey>,
}

/// The mode and profile layers for one MCP tool. The turn's offer and the
/// deferral decision both ask this, so the cost the ceiling is measured
/// against is the cost of exactly the tools the turn would offer.
fn mcp_tool_permitted(
    mode: SessionMode,
    capabilities: &ProfileCapabilities,
    server_id: &str,
    tool_name: &str,
    read_only_hint: Option<bool>,
) -> bool {
    permits(
        mode,
        capabilities,
        &ToolAction::McpCall {
            server_id: server_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments_json: String::new(),
        },
        None,
        read_only_hint,
    )
    .is_allowed()
}

/// An MCP tool by `(server id, tool name)`.
pub(super) type McpToolKey = (String, String);

/// Spec 33 §5.1 stage 2 (D3 as built): the MCP tools this turn has named so
/// far — every one the model called, and every match an `mcp_tool_search`
/// result returned. Their schemas are sent from the next round onward.
///
/// Read from the turn's messages rather than kept beside them, so a turn
/// resumed after an approval pause, a reload or a budget stop still sends
/// them: the pending turn already persists the messages.
pub(super) fn mcp_tools_named_in(messages: &[ModelMessage]) -> BTreeSet<McpToolKey> {
    let mut named = BTreeSet::new();
    let mut search_call_ids = HashSet::new();
    for message in messages {
        for call in &message.tool_calls {
            if call.name == MCP_TOOL_SEARCH_TOOL_NAME {
                search_call_ids.insert(call.id.as_str());
            } else if let Some(key) = parse_namespaced_tool_name(&call.name) {
                named.insert(key);
            }
        }
        let is_search_result = message.role == "tool"
            && message
                .tool_call_id
                .as_deref()
                .is_some_and(|id| search_call_ids.contains(id));
        if !is_search_result {
            continue;
        }
        let Ok(result) = serde_json::from_str::<McpToolSearchResult>(&message.content) else {
            continue;
        };
        named.extend(
            result
                .matches
                .iter()
                .filter_map(|hit| parse_namespaced_tool_name(&hit.name)),
        );
    }
    named
}

/// What `mcp_tool_search` returns to the model, as JSON. The same shape is
/// read back by [`mcp_tools_named_in`].
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct McpToolSearchResult {
    matches: Vec<crate::mcp_inventory::McpToolMatch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    unreachable_servers: Vec<String>,
    /// Servers latched `Failed` this session (§5.3): not searched, so the
    /// model learns why their tools are missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    failed_servers: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    note: String,
}

impl ChatOrchestrator {
    /// The active servers whose tools a turn may offer or search: browser
    /// diagnostic servers are the web tools' own and never appear as MCP
    /// tools.
    fn mcp_tool_server_ids(&self) -> Vec<String> {
        let browser_mcp_server_ids = self.browser_diagnostic_mcp_server_ids();
        self.config
            .active_mcp_servers()
            .iter()
            .map(|server| server.id.clone())
            .filter(|id| !browser_mcp_server_ids.contains(id))
            .collect()
    }

    /// The tools of `server_id` the user pinned (and left enabled): always
    /// sent, whatever the ceiling decided (§5.1). Read from config alone, so
    /// a server with nothing pinned is not contacted to find out.
    fn pinned_mcp_tools(&self, server_id: &str) -> BTreeSet<String> {
        self.config
            .mcp_tools
            .iter()
            .filter(|(_, settings)| settings.pinned && settings.enabled)
            .filter_map(|(key, _)| parse_namespaced_tool_name(key))
            .filter(|(server, _)| server == server_id)
            .map(|(_, tool)| tool)
            .collect()
    }

    /// Spec 33 §5.1: the MCP schemas a round sends. Per server, from the
    /// deferral decision: every tool when the configuration is under the
    /// ceiling; otherwise only pinned tools and those `named` this turn. A
    /// server with nothing to send is not contacted, and a server not yet
    /// listed has its tools offered from the inventory's recorded schemas, so
    /// a turn that uses no MCP tool starts no server (§6) and only the server
    /// actually called (or searched while of unknown cost) starts (D3, Task
    /// 6); a record without a wanted tool or schema lists the server instead.
    /// Every
    /// tool sent passes its per-tool `enabled` setting — a disabled tool is
    /// absent, not refused — and the mode and profile layers with the claim
    /// on record; a call re-asks with the claim the server lists then.
    ///
    /// Pinned tools count against the ceiling (`context.md` D10): the ceiling
    /// bounds the request's whole MCP schema cost, so pinning narrows what
    /// else fits rather than spending past it.
    pub(super) fn mcp_tool_definitions(
        &self,
        mode: SessionMode,
        repository_id: &str,
        mcp: &mut McpRuntime,
        named: &BTreeSet<McpToolKey>,
    ) -> Vec<ToolDefinition> {
        let capabilities = self.config.profile_capabilities();
        let server_ids = self.mcp_tool_server_ids();
        let ids: Vec<&str> = server_ids.iter().map(String::as_str).collect();
        let inventory = McpToolInventory::load(&self.config.data_dir, repository_id, &ids);
        let surface = self.mcp_deferral(&inventory, &ids, mode);
        let offered = |server_id: &str, tool: &crate::mcp::McpTool| {
            self.config.mcp_tool_settings(server_id, &tool.name).enabled
                && mcp_tool_permitted(
                    mode,
                    &capabilities,
                    server_id,
                    &tool.name,
                    tool.read_only_hint,
                )
        };
        let mut definitions = Vec::new();
        for server_id in &server_ids {
            // A `Failed` server's tools are not offered for the rest of the
            // session (§5.3); a call naming one is told why.
            if !mcp.has_server(server_id) || mcp.is_failed(server_id) {
                continue;
            }
            let direct = surface.surface_for(server_id) == McpToolSurface::Direct;
            let pinned = self.pinned_mcp_tools(server_id);
            let named_here: BTreeSet<&str> = named
                .iter()
                .filter(|(server, _)| server == server_id)
                .map(|(_, tool)| tool.as_str())
                .collect();
            if !direct && pinned.is_empty() && named_here.is_empty() {
                continue;
            }
            // Lazy startup (spec 33 Task 6, §5.3): a server not yet listed
            // this turn is offered from its record whenever the record holds
            // every wanted tool with its schema — direct, pinned and named
            // alike — so offering starts nothing; only a search of an
            // unknown-cost server or a call does. A record missing a wanted
            // tool or a schema (an older record) lists the server instead.
            let recorded: Option<Vec<crate::mcp::McpTool>> = (!mcp.is_listed(server_id))
                .then(|| inventory.servers.get(server_id))
                .flatten()
                .and_then(|record| {
                    let has = |name: &str| record.tools.iter().any(|tool| tool.name == name);
                    let tools: Vec<_> = record
                        .tools
                        .iter()
                        .filter(|tool| {
                            direct
                                || pinned.contains(&tool.name)
                                || named_here.contains(tool.name.as_str())
                        })
                        .collect();
                    let complete = pinned.iter().all(|name| has(name))
                        && named_here.iter().all(|name| has(name))
                        && tools.iter().all(|tool| !tool.input_schema_json.is_empty());
                    complete.then(|| tools.iter().map(|tool| tool.to_mcp_tool()).collect())
                });
            let tools = match recorded {
                Some(tools) => tools,
                None => mcp.list_server(server_id).to_vec(),
            };
            for tool in tools {
                let wanted = direct
                    || pinned.contains(&tool.name)
                    || named_here.contains(tool.name.as_str());
                if wanted && offered(server_id, &tool) {
                    definitions.push(tool.to_tool_definition(server_id, &self.scanner));
                }
            }
        }
        definitions
    }

    /// Adds, from this round on, the schemas of MCP tools the turn named since
    /// the last round — a tool the last round called or a search matched (D3
    /// as built). `named_mcp_tools` is the set already considered, so a round
    /// with nothing new contacts nothing.
    pub(super) fn offer_named_mcp_tools(&self, st: &mut TurnState<'_>) {
        if st.native_tools.is_none() {
            return;
        }
        let named = mcp_tools_named_in(&st.messages);
        if named == st.named_mcp_tools {
            return;
        }
        st.named_mcp_tools = named;
        let definitions = self.mcp_tool_definitions(
            st.mode,
            &st.session.repository_id,
            &mut st.mcp,
            &st.named_mcp_tools,
        );
        let Some(tools) = st.native_tools.as_mut() else {
            return;
        };
        for definition in definitions {
            if !tools.iter().any(|tool| tool.name == definition.name) {
                tools.push(definition);
            }
        }
    }

    /// Before an MCP call is asked about, lists its server if this turn has
    /// not, so the permission check reads the claim the server makes now (D3:
    /// a deferred tool is reached by calling it). A tool the user disabled is
    /// not worth a connection: the call is refused before it is dispatched.
    /// Every other action is left alone.
    pub(super) fn prepare_mcp_call(&self, mcp: &mut McpRuntime, action: &ToolAction) {
        let ToolAction::McpCall {
            server_id,
            tool_name,
            ..
        } = action
        else {
            return;
        };
        if self.config.mcp_tool_settings(server_id, tool_name).enabled && mcp.has_server(server_id)
        {
            mcp.list_server(server_id);
        }
    }

    /// Whether the model may call this MCP tool at all: the user has not
    /// disabled it. Mode, profile and approval are asked separately, as for
    /// any call; an unknown server still fails where it always did, on
    /// connecting.
    pub(super) fn mcp_tool_callable(&self, server_id: &str, tool_name: &str) -> bool {
        self.config.mcp_tool_settings(server_id, tool_name).enabled
    }

    /// `mcp_tool_search` (§5.1 stage 1): matches the query against the
    /// persisted inventory of every server the turn may use. It lists a
    /// server only when its cost is still unknown — no inventory yet — which
    /// is how such a server becomes known; a server with an inventory is not
    /// contacted. Only tools the turn could offer are returned: enabled, and
    /// permitted by mode and profile under the recorded claim.
    pub(super) fn search_mcp_tools(
        &self,
        mode: SessionMode,
        repository_id: &str,
        mcp: &mut McpRuntime,
        query: &str,
    ) -> (String, ActionOutcome) {
        let server_ids = self.mcp_tool_server_ids();
        if server_ids.is_empty() {
            return (
                "No MCP server is enabled for this session.".to_string(),
                ActionOutcome::Failed,
            );
        }
        let ids: Vec<&str> = server_ids.iter().map(String::as_str).collect();
        let mut inventory = McpToolInventory::load(&self.config.data_dir, repository_id, &ids);
        let mut unreachable_servers = Vec::new();
        let failed_servers: Vec<String> = ids
            .iter()
            .filter(|server_id| mcp.is_failed(server_id))
            .map(|server_id| (*server_id).to_string())
            .collect();
        let ids: Vec<&str> = ids
            .into_iter()
            .filter(|server_id| !failed_servers.iter().any(|failed| failed == server_id))
            .collect();
        for server_id in &ids {
            if inventory.servers.contains_key(*server_id) || !mcp.has_server(server_id) {
                continue;
            }
            let listed = mcp.list_server(server_id).to_vec();
            match McpToolInventory::load_server(&self.config.data_dir, repository_id, server_id) {
                Some(record) => {
                    inventory.servers.insert((*server_id).to_string(), record);
                }
                // Listed this turn, but the record could not be written: the
                // search still sees what the server said.
                None if !listed.is_empty() => {
                    inventory.servers.insert(
                        (*server_id).to_string(),
                        crate::mcp_inventory::McpServerInventory {
                            server_id: (*server_id).to_string(),
                            refreshed_at_ms: 0,
                            tools: listed
                                .iter()
                                .map(|tool| {
                                    crate::mcp_inventory::McpInventoryTool::from_tool(tool, 0)
                                })
                                .collect(),
                        },
                    );
                }
                None => unreachable_servers.push((*server_id).to_string()),
            }
        }
        let capabilities = self.config.profile_capabilities();
        let matches = search_inventory(
            &inventory,
            &ids,
            query,
            MCP_TOOL_SEARCH_MAX_MATCHES,
            |server_id, tool| {
                self.config.mcp_tool_settings(server_id, &tool.name).enabled
                    && mcp_tool_permitted(
                        mode,
                        &capabilities,
                        server_id,
                        &tool.name,
                        tool.read_only_hint,
                    )
            },
        );
        let mut note = if matches.is_empty() {
            "No MCP tool matched. Try other words for what the tool should do.".to_string()
        } else {
            String::new()
        };
        if !failed_servers.is_empty() {
            if !note.is_empty() {
                note.push(' ');
            }
            note.push_str(
                "The servers in failed_servers failed repeatedly and are not retried \
                 automatically in this session; their tools are not available until the user \
                 retries them from the MCP settings.",
            );
        }
        let result = McpToolSearchResult {
            matches,
            unreachable_servers,
            failed_servers,
            note,
        };
        let text = serde_json::to_string(&result).unwrap_or_default();
        // Tool names and descriptions are server-supplied text.
        (self.scanner.redact(&text).text, ActionOutcome::Ok)
    }

    /// Spec 33 §5.1 (D2, D4): whether this repository's MCP tools go to the
    /// model directly or wait behind search, decided from the persisted
    /// inventory alone — it connects to nothing and starts no process. A
    /// server never listed for this repository is unknown cost and deferred.
    ///
    /// The cost counted is that of every tool the turn would offer: active,
    /// non-browser-diagnostic servers; tools whose
    /// `mcp_server.<server>.tool.<tool>.enabled` is not `false`; and the mode
    /// and profile layers, read with the inventory's recorded read-only claim.
    pub fn mcp_tool_surface(&self, repository_id: &str, mode: SessionMode) -> McpDeferralDecision {
        let server_ids = self.mcp_tool_server_ids();
        let ids: Vec<&str> = server_ids.iter().map(String::as_str).collect();
        let inventory = McpToolInventory::load(&self.config.data_dir, repository_id, &ids);
        self.mcp_deferral(&inventory, &ids, mode)
    }

    /// [`Self::mcp_tool_surface`] over an inventory already loaded.
    fn mcp_deferral(
        &self,
        inventory: &McpToolInventory,
        server_ids: &[&str],
        mode: SessionMode,
    ) -> McpDeferralDecision {
        let capabilities = self.config.profile_capabilities();
        decide_deferral(
            inventory,
            server_ids,
            self.config.mcp_deferral_ceiling_tokens,
            |server_id, tool| {
                self.config.mcp_tool_settings(server_id, &tool.name).enabled
                    && mcp_tool_permitted(
                        mode,
                        &capabilities,
                        server_id,
                        &tool.name,
                        tool.read_only_hint,
                    )
            },
        )
    }

    /// Filters every tool definition through the one permission matrix for
    /// the mode captured at the top of the turn. Withholding a tool anywhere
    /// else would be a second copy of the matrix.
    pub(super) fn tool_offer(
        &self,
        mode: SessionMode,
        turn_options: ChatTurnOptions,
        session: &Session,
        messages: &[ModelMessage],
        mcp: &mut McpRuntime,
    ) -> ToolOffer {
        // A resumed turn already named some MCP tools — by calling one or by
        // a search match — and keeps sending their schemas (spec 33 §5.1).
        let named_mcp_tools = mcp_tools_named_in(messages);
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
                rule: None,
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
            // Spec 33 §5.1: the search tool is present whenever a server
            // could supply a tool, and the schemas follow the deferral
            // surface — a deferred server is not contacted here. Discovered
            // tools are namespaced (mcp__<server>__<tool>); a server that
            // fails to connect contributes none.
            if !self.mcp_tool_server_ids().is_empty()
                && offered(&ToolAction::McpToolSearch {
                    query: String::new(),
                })
            {
                tools.push(mcp_tool_search_tool_definition());
            }
            tools.extend(self.mcp_tool_definitions(
                mode,
                &session.repository_id,
                mcp,
                &named_mcp_tools,
            ));
            tools
        });
        ToolOffer {
            ask_user_offered,
            fetch_url_offered,
            native_tools,
            named_mcp_tools,
        }
    }
}
