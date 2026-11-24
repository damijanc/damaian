use super::*;

impl ChatOrchestrator {
    /// Builds the per-turn MCP runtime from the active server config, resolving
    /// each HTTP server's auth token up front. Returns an inert runtime when no
    /// servers are active (the common case), so there's zero overhead and no
    /// behavior change for users who don't configure MCP.
    pub(super) fn build_mcp_runtime(&self, session_id: &str, repository_id: &str) -> McpRuntime {
        let servers: Vec<McpServerRuntime> = self
            .config
            .active_mcp_servers()
            .into_iter()
            .map(|server| {
                let auth_token = if server.transport == McpTransport::Http {
                    self.resolve_mcp_token(&server.auth_token_env)
                } else {
                    None
                };
                McpServerRuntime {
                    config: server.clone(),
                    auth_token,
                }
            })
            .collect();
        if servers.is_empty() {
            McpRuntime::disabled()
        } else {
            McpRuntime::new(
                servers,
                self.audit_log.clone(),
                self.config.data_dir.clone(),
                session_id,
                repository_id,
                McpTimeouts {
                    connect: std::time::Duration::from_millis(self.config.mcp_connect_timeout_ms),
                    list: std::time::Duration::from_millis(self.config.mcp_list_timeout_ms),
                    call: std::time::Duration::from_millis(self.config.mcp_call_timeout_ms),
                },
            )
        }
    }

    pub(super) fn browser_diagnostic_mcp_server_ids(&self) -> Vec<String> {
        if self.web_diagnostics_runner.is_none() {
            return Vec::new();
        }
        self.config
            .active_mcp_servers()
            .into_iter()
            .filter(|server| looks_like_browser_diagnostics_mcp_server(server))
            .map(|server| server.id.clone())
            .collect()
    }

    fn resolve_mcp_token(&self, reference: &str) -> Option<String> {
        let reference = reference.trim();
        if reference.is_empty() {
            return None;
        }
        if let Some(resolver) = &self.mcp_token_resolver {
            return resolver.resolve(reference);
        }
        // Fallback with no injected resolver: only plain env vars can be read;
        // keychain references require the desktop shell's resolver.
        if reference.starts_with("keychain:") {
            return None;
        }
        std::env::var(reference).ok()
    }

    fn run_web_diagnostic_report(&self, call: &WebDiagnosticCall) -> Result<WebDiagnosticReport> {
        let Some(runner) = &self.web_diagnostics_runner else {
            return Err(ClientError::InvalidInput(
                "Browser diagnostics are not configured for this Damaian session.".to_string(),
            ));
        };
        match call.kind {
            WebDiagnosticKind::Inspect => runner.inspect(call),
            WebDiagnosticKind::Scenario => runner.run_scenario(call),
        }
    }

    /// Runs a browser diagnostic, records what it found, and returns the
    /// model-facing text plus whether the *tool* failed (`context.md` §3.2).
    /// Both dispatch sites — the agentic turn and the approval resume — go
    /// through here, so neither can forget to persist or redact.
    pub(super) fn run_and_record_web_diagnostic(
        &self,
        repository_root: &Path,
        call: &WebDiagnosticCall,
        sink: &mut TurnSink<'_>,
    ) -> Result<(String, bool)> {
        let report = self.run_web_diagnostic_report(call);
        // No report at all — connection or configuration failure — is a
        // failed call too.
        let failed = report
            .as_ref()
            .map_or(true, WebDiagnosticReport::tool_failed);
        if let (Ok(report), Some(session_id), Some(task_id)) =
            (&report, call.session_id.as_deref(), call.task_id.as_deref())
        {
            // `webdiag` already names the approval proposal; a distinct prefix
            // keeps the two id kinds apart.
            let record = WebDiagnosticRecord {
                id: create_id("webdiagrec"),
                task_id: task_id.to_string(),
                tool: call.name().to_string(),
                url: self.scanner.redact(&call.url).text,
                recorded_at_ms: now_millis(),
                report: report.redacted(&self.scanner),
            };
            self.session_store
                .append_web_diagnostic(session_id, &record)?;
            self.record_web_findings(repository_root, session_id, &record)?;
            sink.web_diagnostic(record);
        }
        Ok((self.format_web_diagnostic_result(report), failed))
    }

    fn format_web_diagnostic_result(&self, report: Result<WebDiagnosticReport>) -> String {
        match report {
            Ok(report) => {
                let mut text = match report.render_for_model() {
                    // The renderer already chose "found …" or "failed: …".
                    Some(rendered) => self.scanner.redact(&rendered).text,
                    None => {
                        let mut text = self.scanner.redact(&report.text).text;
                        if report.is_error && !text.starts_with("Browser diagnostic failed") {
                            text = format!("Browser diagnostic failed:\n{text}");
                        }
                        text
                    }
                };
                if !report.artifacts.is_empty() {
                    text.push_str("\n\nArtifacts:");
                    for artifact in report.artifacts {
                        text.push_str("\n- ");
                        text.push_str(&artifact.kind);
                        text.push_str(": ");
                        text.push_str(&self.scanner.redact(&artifact.path).text);
                        if let (Some(width), Some(height)) = (artifact.width, artifact.height) {
                            text.push_str(&format!(" ({width}x{height})"));
                        }
                    }
                }
                text
            }
            Err(error) => format!("Browser diagnostic failed: {error}"),
        }
    }
}

pub(super) fn mcp_call_summary(server_id: &str, tool_name: &str) -> String {
    format!("Called MCP tool `{tool_name}` on server `{server_id}`")
}

pub(super) fn web_diagnostic_summary(call: &WebDiagnosticCall) -> String {
    match call.kind {
        WebDiagnosticKind::Inspect => format!("Inspected web page `{}`", call.url),
        WebDiagnosticKind::Scenario => format!("Ran web scenario against `{}`", call.url),
    }
}

pub(super) fn web_diagnostic_signature(call: &WebDiagnosticCall) -> String {
    format!(
        "{}:{}",
        call.name(),
        normalize_tool_arguments(&call.arguments_json)
    )
}

pub(super) fn browser_retry_limit_note(retry_limit: u32) -> String {
    format!(
        "This browser diagnostic has already failed {retry_limit} time(s) with substantially similar arguments. Change approach: inspect the relevant source files, simplify the scenario, or ask for a different page state instead of repeating the same call."
    )
}

pub(super) fn prompt_enters_web_debug_mode(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let has_url = lower.contains("http://localhost")
        || lower.contains("https://localhost")
        || lower.contains("http://127.0.0.1")
        || lower.contains("https://127.0.0.1")
        || lower.contains("http://[::1]")
        || lower.contains("https://[::1]");
    let has_web_symptom = [
        "web page", "web app", "website", "browser", "frontend", "button",
    ]
    .iter()
    .any(|term| lower.contains(term));
    has_url && has_web_symptom
}

fn looks_like_browser_diagnostics_mcp_server(server: &crate::config::McpServerConfig) -> bool {
    let haystack = format!(
        "{} {} {} {} {}",
        server.id,
        server.label,
        server.command,
        server.args.join(" "),
        server.url
    )
    .to_ascii_lowercase();
    haystack.contains("playwright")
        || haystack.contains("browser")
        || haystack.contains("web-diagnostic")
}

/// Builds the approval proposal surfaced to the user for a pending MCP tool
/// call, reusing the same [`AgentCommandProposal`] shape the command-approval
/// UI already renders. The arguments are truncated so a large payload can't
/// blow up the prompt.
pub(super) fn mcp_approval_proposal(
    proposal_id: &str,
    server_id: &str,
    tool_name: &str,
    arguments_json: &str,
    hook_cause: Option<&(String, String)>,
) -> AgentCommandProposal {
    let arguments = truncate_for_prompt(arguments_json.trim(), 500);
    let prompt = format!(
        "The assistant wants to call the MCP tool `{tool_name}` on server `{server_id}` with arguments:\n{arguments}\n\nThis runs outside the local sandbox and may have side effects. Approve to run it, or decline."
    );
    AgentCommandProposal {
        id: proposal_id.to_string(),
        command: format!("{server_id}/{tool_name}"),
        prompt,
        risk: "mcp".to_string(),
        requires_approval: true,
        blocked: false,
        // Not a local command: the command policy produced no cause. When a
        // `before_tool` hook forced this card, its cause is the honest one to
        // show; otherwise the card already says what the call is in `prompt`.
        cause: hook_cause
            .map(|(hook, reason)| {
                ApprovalCause::HookRequested {
                    hook: hook.clone(),
                    reason: reason.clone(),
                }
                .plain_words()
            })
            .unwrap_or_default(),
        reason: String::new(),
        // An MCP tool call is not a shell command, so `command_allowlist` has
        // nothing to say about it. Per-server `require_approval` in the MCP
        // config is the knob for making these stop prompting.
        allow_always: false,
        allow_browser_diagnostics_for_session: false,
        session_approvable: false,
        session_approval_refusal: String::new(),
        directory: String::new(),
    }
}

pub(super) fn web_diagnostic_approval_proposal(
    proposal_id: &str,
    call: &WebDiagnosticCall,
    hook_cause: Option<&(String, String)>,
) -> AgentCommandProposal {
    let arguments = truncate_for_prompt(call.arguments_json.trim(), 500);
    // Spec 12 §5.4: anything outside loopback is high risk, whatever the tool.
    let risk = if call.is_low_risk() {
        "browser-low"
    } else if call.targets_loopback() && matches!(call.kind, WebDiagnosticKind::Scenario) {
        "browser-medium"
    } else {
        "browser-high"
    };
    let origin = call.origin();
    let prompt = format!(
        "The assistant wants to run `{}` against `{}`.\nTarget origin: `{origin}`\n\nArguments:\n{arguments}\n\nBrowser diagnostics may navigate pages, use current browser state, or interact with forms. Approve to run it, or decline.",
        call.name(),
        call.url,
    );
    AgentCommandProposal {
        id: proposal_id.to_string(),
        command: format!("{} {}", call.name(), call.url),
        prompt,
        risk: risk.to_string(),
        requires_approval: true,
        blocked: false,
        // Not a local command: its policy cause does not exist. A hook that
        // forced this card supplies the one cause there is to name.
        cause: hook_cause
            .map(|(hook, reason)| {
                ApprovalCause::HookRequested {
                    hook: hook.clone(),
                    reason: reason.clone(),
                }
                .plain_words()
            })
            .unwrap_or_default(),
        reason: String::new(),
        allow_always: false,
        // The session grant covers loopback only, so a remote URL is never
        // offered it: each remote diagnostic is approved on its own.
        allow_browser_diagnostics_for_session: !call.is_low_risk() && call.targets_loopback(),
        session_approvable: false,
        session_approval_refusal: String::new(),
        directory: String::new(),
    }
}

/// Whether a report has a console problem with a location, the only thing
/// that needs the repository's file list (spec 22 `context.md` §14).
pub(super) fn has_located_console_problem(report: &WebDiagnosticReport) -> bool {
    report.details.as_ref().is_some_and(|details| {
        details
            .console
            .iter()
            .any(|entry| entry.is_problem() && entry.location.is_some())
    })
}
