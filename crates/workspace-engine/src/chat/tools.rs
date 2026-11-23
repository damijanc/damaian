use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandRequest {
    pub(crate) command: String,
    pub(crate) reason: String,
    /// Repository-relative directory the model asked the command to run in.
    /// `None` means the repository root, which is what every request before
    /// spec 24 meant.
    pub(crate) working_directory: Option<String>,
}

/// One step as the model proposed it. Only a title and an optional detail: the
/// status, the timings and the evidence are the engine's to write, and letting
/// the model supply them would hand it the very field requirement 6 exists to
/// keep out of its reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProposedStep {
    pub(super) title: String,
    pub(super) detail: Option<String>,
    pub(super) check: Option<PlannedCheck>,
    /// The step's build check (spec 109 D10), proposed for review like `check`.
    pub(super) build: Option<PlannedCheck>,
}

/// A planned check from a `propose_plan` step's `check` or `build` object. A
/// check is optional, and a proposed check authorises nothing on its own (spec
/// 105 §2): it only becomes authoritative once the user approves the plan. A
/// check without a command is not a check, so it is dropped the same way a
/// step with no title is. `required_default` is what a missing `required`
/// means; a build check passes `false` and ignores the key.
pub(super) fn planned_check_from_json(
    value: &serde_json::Value,
    required_default: bool,
) -> Option<PlannedCheck> {
    let command = value
        .get("command")
        .and_then(|command| command.as_str())
        .map(str::trim)
        .filter(|command| !command.is_empty())?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|text| text.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    Some(PlannedCheck {
        version: PLANNED_CHECK_VERSION,
        command: command.to_string(),
        working_directory: text("workingDirectory"),
        purpose: text("purpose"),
        required: required_default
            && value
                .get("required")
                .and_then(|required| required.as_bool())
                .unwrap_or(true),
    })
}

/// What a matched tool call (native `tools`/`tool_calls`, or the
/// `DAMAIAN_COMMAND_V1` text envelope for `Command`) asked the client to do.
/// `run_agentic_turn` dispatches on this rather than on raw tool names so
/// the text-envelope fallback and native tool calls funnel through the same
/// handling per variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolAction {
    Command(CommandRequest),
    ProposePatch(GeneratedEdit),
    /// The model's plan for this turn. §5.1: a turn is non-trivial when the
    /// model proposes more than one step for it, so this call *is* the
    /// triviality decision rather than a guess made before the turn starts.
    ProposePlan(Vec<ProposedStep>),
    /// The model asks to move on from the current step. It does not say how
    /// the step ended — the engine derives that from the evidence accrued
    /// while the step was in progress (§5.3).
    CompleteStep,
    // `worktree` on the five read tools is the argument as the model gave it,
    // resolved at dispatch (spec 60 §5.3); `None` reads the session's checkout.
    ReadFile {
        path: String,
        range: Option<LineRange>,
        worktree: Option<String>,
    },
    ListDirectory {
        dir: Option<String>,
        depth: Option<usize>,
        worktree: Option<String>,
    },
    SearchContent {
        pattern: String,
        path_glob: Option<String>,
        max_matches: Option<usize>,
        worktree: Option<String>,
    },
    EditFile {
        summary: String,
        edits: Vec<RegionEdit>,
    },
    SearchCodebase {
        query: String,
        semantic: bool,
        limit: usize,
    },
    ReadGitStatus {
        worktree: Option<String>,
    },
    ReadGitDiff {
        staged: bool,
        worktree: Option<String>,
        paths: Vec<String>,
        stat: bool,
        base: Option<String>,
    },
    /// Lists the repository's reachable worktrees (spec 60 §5.2). No
    /// `worktree`: it is the tool that discovers them.
    ListWorktrees {
        base: Option<String>,
    },
    /// `base..HEAD` as one-line summaries (spec 60 §5.2).
    ReadGitLog {
        worktree: Option<String>,
        base: Option<String>,
        max_count: Option<usize>,
    },
    WebDiagnostic(WebDiagnosticCall),
    McpCall {
        server_id: String,
        tool_name: String,
        arguments_json: String,
    },
    /// Spec 33 §5.1 stage 1: searches the MCP tool inventory by capability.
    /// Calls no MCP tool; it may list a server whose cost is still unknown,
    /// which is what records that server's inventory.
    McpToolSearch {
        query: String,
    },
    /// One bounded question for the user (spec 50). Already validated against
    /// the bounds when it is decoded; not yet redacted.
    AskUser(AskUserInput),
    /// One page the model asked to read (spec 51 §5.3). The URL is checked
    /// against the egress guard and the host allowlist before a request is
    /// built, and the request itself carries no credential.
    FetchUrl {
        url: String,
    },
    /// A model-written note about what it has learned this turn (spec 65).
    ///
    /// Not a read and not a turn-shaping call: it changes only the turn's
    /// notes, so it may run in the same round as the read that produced the
    /// finding, and a round of reads and notes is exploration (requirement 8).
    TurnNotes {
        text: String,
        replace: bool,
    },
    /// A candidate memory the model suggests (spec 29). It cannot store: the
    /// dispatch screens it and at most appends a proposal, which the user must
    /// confirm through a separate route.
    ProposeMemory {
        statement: String,
        category: MemoryCategory,
        scope: Option<MemoryScope>,
        source: Option<String>,
        evidence_path: Option<String>,
    },
}

/// What dispatching a tool can change beyond its own result.
///
/// The one place that question is answered. Spec 17's recovery reads the
/// [`ActionEffect::WritesRepository`] answer through [`tool_action_marker`]'s
/// boolean; requirement 8's batching reads [`ActionEffect::ReadOnly`] through
/// [`action_is_batchable_read_only`]; and spec 59's round classification (§5.1)
/// reads it to decide whether a round is charged to the exploration allowance
/// or the action budget. A tool's effect is therefore also its budget decision,
/// so adding a variant forces that decision here. All come off one exhaustive
/// match, so a new tool cannot become batchable, side-effecting or exploration
/// by omission, and there is no second list to keep in sync with spec 17.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionEffect {
    /// Reads repository or session state and returns it. No write, no turn
    /// control, so it is safe to run beside another read.
    ReadOnly,
    /// Can leave a side effect outside the session log. A crash mid-flight
    /// leaves an unknown outcome, and it must not run concurrently.
    WritesRepository,
    /// Writes nothing outside the session log, but advances or ends the turn:
    /// planning, completing a step, proposing a patch or an edit. Safe for
    /// crash recovery, not a candidate for batching.
    ShapesTurn,
    /// Writes the turn's own notes (spec 65). It is not `WritesRepository`
    /// because nothing outside the session log changes, and it is not
    /// `ShapesTurn` because recording a finding in the same round as the read
    /// that found it is exactly what the model should do. Its budget decision
    /// is exploration: a round of reads and notes is charged to the read
    /// allowance (requirement 8).
    Notes,
}

pub(super) fn action_effect(action: &ToolAction) -> ActionEffect {
    match action {
        ToolAction::Command(_)
        | ToolAction::WebDiagnostic(_)
        | ToolAction::McpCall { .. }
        // A fetch leaves the machine, so it is side-effecting and serialised:
        // a crash mid-request is an unknown external outcome (spec 51 D3).
        | ToolAction::FetchUrl { .. } => ActionEffect::WritesRepository,
        ToolAction::ProposePatch(_)
        | ToolAction::ProposePlan(_)
        | ToolAction::CompleteStep
        | ToolAction::EditFile { .. }
        // Writes only the session log, and waits on the user rather than
        // running beside another call (spec 50 Task 4 pauses the turn here).
        | ToolAction::AskUser(_) => ActionEffect::ShapesTurn,
        ToolAction::ReadFile { .. }
        | ToolAction::ListDirectory { .. }
        | ToolAction::SearchContent { .. }
        | ToolAction::SearchCodebase { .. }
        | ToolAction::ReadGitStatus { .. }
        | ToolAction::ReadGitDiff { .. }
        | ToolAction::ListWorktrees { .. }
        | ToolAction::ReadGitLog { .. } => ActionEffect::ReadOnly,
        // A proposal writes only the session log and waits for a separate user
        // action. It is charged like notes so a round of reads plus one
        // suggestion is exploration, not an action round.
        ToolAction::TurnNotes { .. } | ToolAction::ProposeMemory { .. } => ActionEffect::Notes,
        // A search reads the local inventory and calls no tool, so it is
        // charged as exploration. It is not `ReadOnly` because it is never
        // batched: it needs the turn's MCP runtime, which only the loop
        // thread holds, to list a server of unknown cost.
        ToolAction::McpToolSearch { .. } => ActionEffect::Notes,
    }
}

/// Whether a tool may be dispatched alongside another in the same round.
/// Derived from [`action_effect`] rather than declared beside it, so there is
/// no second list to drift. `Notes` is batchable with reads but is not a read:
/// its mutation happens on the loop thread after a batch returns, never inside
/// the batch worker (spec 65 §5.1).
pub(super) fn action_is_batchable_read_only(action: &ToolAction) -> bool {
    matches!(
        action_effect(action),
        ActionEffect::ReadOnly | ActionEffect::Notes
    )
}

/// What a `before_tool` verdict does to the call about to be dispatched
/// (`context.md` §4).
///
/// `RequestApproval` is not one thing for every tool: a command, MCP call or
/// web diagnostic has an approval card to move to, while `propose_patch` and
/// `edit_file` are already reviewed before they apply. Everything else has no
/// approval step, including `ask_user` — an answer is not an approval, so a
/// question is refused rather than turned into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BeforeToolEffect {
    Proceed,
    ForceApproval,
    /// Proceed, and record the request as a warning because the existing
    /// review step already satisfies it.
    SatisfiedApproval,
    Refuse,
}

pub(super) fn before_tool_effect(action: &ToolAction, verdict: Verdict) -> BeforeToolEffect {
    match verdict {
        Verdict::Allow | Verdict::Warn => BeforeToolEffect::Proceed,
        Verdict::Deny => BeforeToolEffect::Refuse,
        Verdict::RequestApproval => match action {
            ToolAction::Command(_) | ToolAction::McpCall { .. } | ToolAction::WebDiagnostic(_) => {
                BeforeToolEffect::ForceApproval
            }
            ToolAction::ProposePatch(_) | ToolAction::EditFile { .. } => {
                BeforeToolEffect::SatisfiedApproval
            }
            _ => BeforeToolEffect::Refuse,
        },
    }
}

/// The `(hook id, reason)` a denial or approval request is shown with.
pub(super) fn hook_cause(outcome: &HookOutcome) -> Option<(String, String)> {
    let hook_id = outcome.deciding_hook.as_deref()?;
    let reason = outcome
        .reasons
        .iter()
        .find(|(id, _)| id == hook_id)
        .map(|(_, reason)| reason.clone())
        .unwrap_or_default();
    Some((hook_id.to_string(), reason))
}

/// What an `after_tool` denial appends to the result the model sees
/// (`context.md` §4).
pub(super) fn hook_after_note(outcome: &HookOutcome) -> Option<String> {
    if outcome.verdict != Verdict::Deny {
        return None;
    }
    let (hook_id, reason) = hook_cause(outcome)?;
    Some(if reason.is_empty() {
        format!("Hook `{hook_id}` refused after the action.")
    } else {
        format!("Hook `{hook_id}`: {reason}")
    })
}

/// The payload for `before_tool` and `after_tool` (`context.md` §6): names and
/// paths, never file content. A command's text and a URL are redacted here
/// because the dispatcher redacts the response, not the envelope it sends.
pub(super) fn tool_event_payload(
    scanner: &SecretScanner,
    action: &ToolAction,
    outcome: Option<&ActionOutcome>,
    termination: Option<CommandTermination>,
) -> serde_json::Value {
    let (tool, mut paths) = match action {
        ToolAction::ReadFile { path, .. } => ("read_file", vec![path.clone()]),
        ToolAction::ListDirectory { dir, .. } => (
            "list_directory",
            vec![dir.clone().unwrap_or_else(|| ".".to_string())],
        ),
        ToolAction::SearchContent { path_glob, .. } => {
            ("search_content", path_glob.clone().into_iter().collect())
        }
        ToolAction::SearchCodebase { .. } => ("search_codebase", Vec::new()),
        ToolAction::ReadGitStatus { .. } => ("read_git_status", Vec::new()),
        ToolAction::ReadGitDiff { paths, .. } => ("read_git_diff", paths.clone()),
        ToolAction::ListWorktrees { .. } => ("list_worktrees", Vec::new()),
        ToolAction::ReadGitLog { .. } => ("read_git_log", Vec::new()),
        ToolAction::ProposePatch(edit) => (
            "propose_patch",
            edit.changes
                .iter()
                .map(|change| change.path.clone())
                .collect(),
        ),
        ToolAction::EditFile { edits, .. } => (
            "edit_file",
            edits.iter().map(|edit| edit.path.clone()).collect(),
        ),
        ToolAction::ProposePlan(_) => ("propose_plan", Vec::new()),
        ToolAction::CompleteStep => ("complete_step", Vec::new()),
        ToolAction::AskUser(_) => ("ask_user", Vec::new()),
        ToolAction::FetchUrl { .. } => ("fetch_url", Vec::new()),
        ToolAction::TurnNotes { .. } => ("turn_notes", Vec::new()),
        ToolAction::ProposeMemory { .. } => ("propose_memory", Vec::new()),
        ToolAction::Command(_) => ("run_command", Vec::new()),
        ToolAction::WebDiagnostic(_) => ("web_diagnostic", Vec::new()),
        ToolAction::McpCall { .. } => ("mcp_call", Vec::new()),
        ToolAction::McpToolSearch { .. } => ("mcp_tool_search", Vec::new()),
    };
    paths = paths
        .into_iter()
        .map(|path| scanner.redact(&path).text)
        .collect();
    let mut payload = serde_json::json!({"tool": tool, "paths": paths});
    match action {
        ToolAction::Command(request) => {
            payload["command"] = serde_json::json!(scanner.redact(&request.command).text);
        }
        ToolAction::McpCall {
            server_id,
            tool_name,
            ..
        } => {
            payload["server"] = serde_json::json!(scanner.redact(server_id).text);
            payload["toolName"] = serde_json::json!(scanner.redact(tool_name).text);
        }
        ToolAction::WebDiagnostic(call) => {
            payload["url"] = serde_json::json!(scanner.redact(&call.url).text);
        }
        // The URL leaves the machine, so the hook sees it; the dispatcher
        // redacts it here because the envelope is built before that.
        ToolAction::FetchUrl { url } => {
            payload["url"] = serde_json::json!(scanner.redact(url).text);
        }
        _ => {}
    }
    if let Some(outcome) = outcome {
        let outcome_name = match outcome {
            ActionOutcome::Ok | ActionOutcome::FileRead { .. } => "ok",
            ActionOutcome::CommandExit(Some(0), _) => "ok",
            ActionOutcome::CommandExit(_, _) => "failed",
            ActionOutcome::Failed => "failed",
            ActionOutcome::Cancelled => "refused",
        };
        payload["outcome"] = serde_json::json!(outcome_name);
        if let ActionOutcome::CommandExit(exit_code, _) = outcome {
            if let Some(code) = exit_code {
                payload["exitCode"] = serde_json::json!(code);
            }
            if let Some(termination) = termination {
                payload["termination"] = serde_json::json!(termination.as_str());
            }
        }
    }
    payload
}

/// What a stopped turn says about the action the stop ended part-way (spec
/// 109 D8). Its outcome is unknown and nothing retries it, so the user is
/// told to check rather than assume either way.
pub(super) fn stopped_action_note(action: &str, reference: &str) -> String {
    let what = match action {
        "run_command" => format!("the command `{reference}`"),
        "mcp_call" => format!("the MCP call `{reference}`"),
        _ if reference.is_empty() => format!("`{action}`"),
        _ => format!("`{action}` (`{reference}`)"),
    };
    format!(
        "Stopped by you while running {what}. It was ended part-way, so its outcome is \
         unknown; Damaian did not retry it. Check its effect before running it again."
    )
}

/// The action marker parameters for a dispatched tool: a stable name, a
/// reference identifying *which* invocation, and whether a crash mid-flight
/// could have left a side effect.
///
/// Derived here rather than at each match arm so the side-effect answer for a
/// tool exists in exactly one place. `ProposePatch` is **not** side-effecting:
/// proposing writes nothing to the repository. Applying one is, and is bracketed
/// separately in `edit.rs`.
pub(super) fn tool_action_marker(action: &ToolAction) -> (&'static str, String, bool) {
    // Spec 17's answer, unchanged in value: turn-shaping writes are confined to
    // the append-only session log, so a crash leaves `interrupted`, not
    // `unknown_external_outcome`.
    let side_effecting = matches!(action_effect(action), ActionEffect::WritesRepository);
    // Spec 60 §5.5: a worktree read's reference is `<worktree>:<reference>`,
    // so the log says where it happened and the same path in two worktrees is
    // two references. Without `worktree` the reference is unchanged.
    let (name, reference, side_effecting) = tool_action_marker_unscoped(action, side_effecting);
    match read_tool_worktree(action) {
        Some(worktree) => (name, format!("{worktree}:{reference}"), side_effecting),
        None => (name, reference, side_effecting),
    }
}

/// The `worktree` argument of a read tool, if it was given one.
pub(super) fn read_tool_worktree(action: &ToolAction) -> Option<&str> {
    match action {
        ToolAction::ReadFile { worktree, .. }
        | ToolAction::ListDirectory { worktree, .. }
        | ToolAction::SearchContent { worktree, .. }
        | ToolAction::ReadGitStatus { worktree }
        | ToolAction::ReadGitDiff { worktree, .. }
        | ToolAction::ReadGitLog { worktree, .. } => worktree.as_deref(),
        _ => None,
    }
}

/// " in worktree `x`" for a worktree read, nothing otherwise, so a read
/// without `worktree` says exactly what it always said.
pub(super) fn in_worktree(worktree: Option<&str>) -> String {
    worktree
        .map(|worktree| format!(" in worktree `{worktree}`"))
        .unwrap_or_default()
}

/// [`tool_action_marker`] before the worktree prefix.
fn tool_action_marker_unscoped(
    action: &ToolAction,
    side_effecting: bool,
) -> (&'static str, String, bool) {
    match action {
        ToolAction::Command(request) => ("run_command", request.command.clone(), side_effecting),
        ToolAction::ProposePatch(edit) => ("propose_patch", edit.summary.clone(), side_effecting),
        ToolAction::ProposePlan(steps) => ("propose_plan", steps.len().to_string(), side_effecting),
        ToolAction::CompleteStep => ("complete_step", String::new(), side_effecting),
        // The reference stays empty: the question is model-authored and not
        // yet redacted when the marker is written.
        ToolAction::AskUser(_) => ("ask_user", String::new(), side_effecting),
        // The host alone, not the URL: the reference is written to the action
        // log, and a query string is exactly where a value the guard might miss
        // would sit. An unparsable URL is refused before dispatch, so the empty
        // fallback only exists to keep the marker total.
        ToolAction::FetchUrl { url } => (
            "fetch_url",
            ExternalUrl::parse(url)
                .map(|url| url.host)
                .unwrap_or_default(),
            side_effecting,
        ),
        // The statement is model-authored and may contain a secret the gate
        // will refuse; the marker carries no reference for it.
        ToolAction::ProposeMemory { .. } => ("propose_memory", String::new(), side_effecting),
        // Only the size: the notes themselves are in the session log, and a
        // reference must not leak model-written text into the action log.
        ToolAction::TurnNotes { text, .. } => (
            "turn_notes",
            text.chars().count().to_string(),
            side_effecting,
        ),
        ToolAction::ReadFile { path, .. } => ("read_file", path.clone(), side_effecting),
        ToolAction::ListDirectory { dir, .. } => (
            "list_directory",
            dir.clone().unwrap_or_default(),
            side_effecting,
        ),
        ToolAction::SearchContent { pattern, .. } => {
            ("search_content", pattern.clone(), side_effecting)
        }
        ToolAction::EditFile { summary, .. } => ("edit_file", summary.clone(), side_effecting),
        ToolAction::SearchCodebase { query, .. } => {
            ("search_codebase", query.clone(), side_effecting)
        }
        ToolAction::ReadGitStatus { .. } => ("read_git_status", String::new(), side_effecting),
        ToolAction::ReadGitDiff {
            staged,
            paths,
            stat,
            base,
            ..
        } => {
            // The options join the reference only when given, so a plain diff's
            // reference is unchanged and two differently scoped diffs differ.
            let mut reference = staged.to_string();
            if let Some(base) = base {
                reference.push_str(&format!(" base={base}"));
            }
            if *stat {
                reference.push_str(" stat");
            }
            if !paths.is_empty() {
                reference.push_str(&format!(" paths={}", paths.join(",")));
            }
            ("read_git_diff", reference, side_effecting)
        }
        ToolAction::ListWorktrees { base } => (
            "list_worktrees",
            base.clone().unwrap_or_default(),
            side_effecting,
        ),
        ToolAction::ReadGitLog {
            base, max_count, ..
        } => {
            let mut reference = String::new();
            if let Some(base) = base {
                reference.push_str(&format!(" base={base}"));
            }
            if let Some(max_count) = max_count {
                reference.push_str(&format!(" n={max_count}"));
            }
            ("read_git_log", reference, side_effecting)
        }
        ToolAction::WebDiagnostic(call) => ("web_diagnostic", call.url.clone(), side_effecting),
        ToolAction::McpCall {
            server_id,
            tool_name,
            ..
        } => (
            "mcp_call",
            format!("{server_id}/{tool_name}"),
            side_effecting,
        ),
        // Empty for the reason `ask_user`'s is: the query is model-authored
        // and not yet redacted when the marker is written.
        ToolAction::McpToolSearch { .. } => ("mcp_tool_search", String::new(), side_effecting),
    }
}

/// Whether the action a plan review defers changes code (spec 107 D3): an
/// edit or patch touching anything but documentation, or a command. Other
/// deferred actions are not evidence of a code change, so they suggest
/// nothing — a planning or documentation task is never offered a worker.
pub(super) fn action_changes_code(action: &ToolAction) -> bool {
    match action {
        ToolAction::ProposePatch(edit) => edit
            .changes
            .iter()
            .any(|change| !crate::verification::is_documentation(&change.path)),
        ToolAction::EditFile { edits, .. } => edits
            .iter()
            .any(|edit| !crate::verification::is_documentation(&edit.path)),
        ToolAction::Command(_) => true,
        _ => false,
    }
}

/// The progress label while a command actually runs, as opposed to
/// [`tool_action_label`]'s "Proposing" for the call that asked for it.
pub(super) fn running_command_label(command: &str) -> String {
    format!("Running `{command}`")
}

/// What to show the user while a tool runs. Lives here rather than in the UI so
/// the frontend never has to map tool names to prose.
pub(super) fn tool_action_label(action: &ToolAction) -> String {
    match action {
        ToolAction::Command(request) => format!("Proposing `{}`", request.command),
        ToolAction::ProposePatch(_) => "Preparing a patch".to_string(),
        ToolAction::ProposePlan(steps) => format!("Planning {} steps", steps.len()),
        ToolAction::CompleteStep => "Finishing a step".to_string(),
        ToolAction::AskUser(_) => "Asking you a question".to_string(),
        ToolAction::FetchUrl { url } => match ExternalUrl::parse(url) {
            Ok(url) => format!("Fetch URL: {}", url.host),
            Err(_) => "Fetch URL".to_string(),
        },
        ToolAction::ProposeMemory { .. } => "Suggesting a memory".to_string(),
        ToolAction::TurnNotes { replace, .. } => {
            if *replace {
                "Updating notes".to_string()
            } else {
                "Writing notes".to_string()
            }
        }
        ToolAction::ReadFile {
            path,
            range,
            worktree,
        } => {
            // Spec 107 requirement 3: "Reading config.rs 1200–1600", so a
            // second read of the same file is told apart from the first.
            let lines = range
                .map(|range| format!(" {}–{}", range.start, range.end))
                .unwrap_or_default();
            format!("Reading {path}{lines}{}", in_worktree(worktree.as_deref()))
        }
        ToolAction::ListDirectory { dir, worktree, .. } => match (dir, worktree) {
            (Some(dir), _) => format!("Listing {dir}{}", in_worktree(worktree.as_deref())),
            (None, Some(worktree)) => format!("Listing worktree `{worktree}`"),
            (None, None) => "Listing the repository".to_string(),
        },
        ToolAction::SearchContent {
            pattern,
            path_glob,
            worktree,
            ..
        } => {
            format!(
                "Searching for \"{pattern}\"{}{}",
                path_glob
                    .as_deref()
                    .map(|glob| format!(" in `{glob}`"))
                    .unwrap_or_default(),
                in_worktree(worktree.as_deref())
            )
        }
        ToolAction::EditFile { summary, .. } => format!("Preparing an edit: {summary}"),
        ToolAction::SearchCodebase { query, .. } => format!("Searching for \"{query}\""),
        ToolAction::ReadGitStatus { worktree } => {
            format!("Reading git status{}", in_worktree(worktree.as_deref()))
        }
        ToolAction::ReadGitDiff {
            staged, worktree, ..
        } => {
            if *staged {
                format!(
                    "Reading the staged diff{}",
                    in_worktree(worktree.as_deref())
                )
            } else {
                format!(
                    "Reading the working diff{}",
                    in_worktree(worktree.as_deref())
                )
            }
        }
        ToolAction::ListWorktrees { .. } => "Listing worktrees".to_string(),
        ToolAction::ReadGitLog { worktree, .. } => {
            format!("Reading git log{}", in_worktree(worktree.as_deref()))
        }
        ToolAction::WebDiagnostic(call) => match call.kind {
            WebDiagnosticKind::Inspect => format!("Inspecting {}", call.url),
            WebDiagnosticKind::Scenario => format!("Running web scenario on {}", call.url),
        },
        ToolAction::McpCall {
            server_id,
            tool_name,
            ..
        } => format!("Calling {tool_name} on {server_id}"),
        ToolAction::McpToolSearch { .. } => "Searching MCP tools".to_string(),
    }
}
