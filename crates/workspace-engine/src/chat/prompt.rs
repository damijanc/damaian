use super::*;

/// `system_prompt` plus the handoff paragraph when the session is offered
/// handoffs. `system_prompt` itself never changes, which is how the setting
/// being off stays byte-for-byte off (spec 57 requirement 1).
pub(super) fn system_prompt_for_session(
    mode: SessionMode,
    options: &ChatTurnOptions,
    offer_handoffs: bool,
    shell: &str,
) -> String {
    let prompt = system_prompt_with_capabilities(
        mode,
        options.question_route(),
        options.synchronous_user_present,
        shell,
    );
    if offer_handoffs {
        format!("{prompt}\n\n{WORKTREE_TASK_PARAGRAPH}\n\n{WORKER_HANDOFF_PARAGRAPH}")
    } else {
        prompt
    }
}

#[cfg(test)]
pub(super) fn system_prompt(mode: SessionMode, user_present: bool, shell: &str) -> String {
    system_prompt_with_capabilities(mode, user_present, user_present, shell)
}

/// Spec 113 requirement 4: a choice written into a plan or an answer reaches
/// the user as prose, and typing an answer to it starts a new task.
pub(super) const USER_CHOICE_PARAGRAPH: &str = "When the user must make a choice before you can go on, ask it with `ask_user` (2 to 4 short options) before proposing a plan. Never write options for the user to pick from into a plan or an answer.";

pub(super) fn system_prompt_with_capabilities(
    mode: SessionMode,
    question_route: bool,
    synchronous_user_present: bool,
    shell: &str,
) -> String {
    let prompt = mode_system_prompt(mode, shell);
    if synchronous_user_present {
        // Only the modes that propose plans need telling how to ask a choice
        // instead of writing options into one (spec 113 requirement 4).
        match mode {
            SessionMode::Code | SessionMode::Plan => format!("{prompt}\n\n{USER_CHOICE_PARAGRAPH}"),
            SessionMode::Ask | SessionMode::Review => prompt,
        }
    } else if question_route {
        format!("{prompt}\n\n{WORKER_QUESTION_PARAGRAPH}")
    } else {
        format!("{prompt}\n\n{HEADLESS_PARAGRAPH}")
    }
}

fn mode_system_prompt(mode: SessionMode, shell: &str) -> String {
    const PREFIX: &str = "You are a local-first coding assistant. Answer using only the provided repository context when possible. Cite relevant file paths. Do not request or expose secrets.\n\nRepository context sections named `agent_instruction` contain AGENTS.md instructions for this repository. Follow them when they apply to the files you discuss or edit. More specific nested AGENTS.md instructions override broader ones. The user's request and Damaian's safety policy take precedence over repository instructions.";
    match mode {
        // Ask is reads-and-explanations only: no command envelope at all, so
        // there is nothing to invite a command the mode would refuse.
        SessionMode::Ask => PREFIX.to_string(),
        SessionMode::Code => {
            format!(
                "{PREFIX}\n\n{}",
                command_envelope_paragraph_unrestricted(shell)
            )
        }
        // Plan and Review permit read-only commands but refuse anything that
        // would need approval outright rather than raising an approval card
        // (`proposal.md` §5.3), so the third paragraph matches that behavior.
        SessionMode::Plan | SessionMode::Review => {
            format!(
                "{PREFIX}\n\n{}",
                command_envelope_paragraph_read_only(shell)
            )
        }
    }
}

/// The basename of the configured shell — `zsh`, `bash`, … — the only part of
/// `config.shell` the model needs to be told (spec 63 requirement 7). The full
/// path is the user's own setting and adds nothing.
fn shell_basename(shell: &str) -> &str {
    shell.rsplit('/').next().unwrap_or(shell)
}

/// The sentence that tells the model which shell runs its commands, plus the
/// zsh-only glob-quoting warning: zsh's `nomatch` option fails a command whose
/// unquoted glob matches nothing, which is a look-around failure, not a code
/// problem (spec 63 proposal §5.4).
pub(super) fn shell_guidance(shell: &str) -> String {
    let name = shell_basename(shell);
    if name == "zsh" {
        format!(
            "Commands run in {name} -lc. Quote glob patterns such as '*.rs'; zsh fails a command whose unquoted glob matches nothing."
        )
    } else {
        format!("Commands run in {name} -lc.")
    }
}

/// The command-envelope paragraph for Code — today's wording verbatim,
/// including the invitation to request a side-effecting command. Code is the
/// only mode where such a request can become an approval card, so it is the
/// only mode that should teach it.
pub(super) fn command_envelope_paragraph_unrestricted(shell: &str) -> String {
    format!(
        "If the user asks about current Git state, recent commits, latest changes, uncommitted changes, repository history, or another fact that requires a local command, your entire response must be exactly one command request envelope. Do not add prose before or after the envelope:\nDAMAIAN_COMMAND_V1\nCOMMAND: git log -1 --stat --oneline\nREASON: Inspect the latest commit for the user's question.\nEND_COMMAND\n\nPrefer read-only commands such as git status, git log, ls, and pwd when they are sufficient, and the read_git_diff and read_git_log tools for git diffs and commit content; git diff, git show and git log -p need approval in the shell. Damaian runs a short list of read-only commands without asking and needs your approval before running anything else, including any command that uses shell control. {} When the user's task requires a command with side effects, network access, Docker access, shell control, or unknown risk, request the command and Damaian will pause for user approval before running it.",
        shell_guidance(shell),
    )
}

/// The command-envelope paragraph for Plan and Review. The envelope stays
/// (read-only commands are permitted in both modes) but the Code-only
/// invitation to request a side-effecting command is replaced with what will
/// actually happen: it is refused, not queued for approval.
pub(super) fn command_envelope_paragraph_read_only(shell: &str) -> String {
    format!(
        "If the user asks about current Git state, recent commits, latest changes, uncommitted changes, repository history, or another fact that requires a local command, your entire response must be exactly one command request envelope. Do not add prose before or after the envelope:\nDAMAIAN_COMMAND_V1\nCOMMAND: git log -1 --stat --oneline\nREASON: Inspect the latest commit for the user's question.\nEND_COMMAND\n\nPrefer read-only commands such as git status, git log, ls, and pwd when they are sufficient, and the read_git_diff and read_git_log tools for git diffs and commit content; git diff, git show and git log -p need approval in the shell. Damaian runs a short list of read-only commands without asking and refuses every other command outright in this mode, including any command that uses shell control. {} In this mode only read-only commands that need no approval are available; a command with side effects, network access, Docker access, shell control, or unknown risk is refused outright, not queued for approval. Do not request one.",
        shell_guidance(shell),
    )
}

/// The conversation half of a user message: the #55 summary and the recent
/// messages, already redacted (spec 27 §5.1).
///
/// Split out so a paused pre-send review can re-render the request after an
/// item removal without re-reading the transcript. `build_model_prompt` still
/// composes the same two pieces and produces byte-identical text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PromptConversation {
    /// The rendered, redacted summary, if the turn sends one.
    pub(super) summary: Option<String>,
    /// The omitted-message notice and the messages; `None` when the block would
    /// be empty, which is what decides whether its heading appears.
    pub(super) body: Option<String>,
}

impl PromptConversation {
    /// The same conversation with the #55 summary removed for this turn.
    ///
    /// The model is told what it lost rather than silently receiving a
    /// shorter block: the summary's whole job is to make an omission visible.
    pub(super) fn without_summary(&self, messages: usize) -> Self {
        let notice = format!(
            "[Damaian: the user removed the conversation summary covering {messages} earlier \
             message{} from this request.]\n",
            if messages == 1 { "" } else { "s" }
        );
        Self {
            summary: None,
            body: Some(format!("{notice}{}", self.body.clone().unwrap_or_default())),
        }
    }
}

/// Records on the manifest what a turn sends beside the assembled context:
/// the conversation messages (the summary is already its own category) and
/// the system prompt, at `len / 4` like prose. The context panel measures
/// these against the model's window (spec 79).
pub(super) fn note_turn_overhead(
    manifest: &mut crate::context_manager::ContextManifest,
    conversation: &PromptConversation,
    messages: &[ModelMessage],
) {
    manifest.conversation_tokens = Some(
        conversation
            .body
            .as_deref()
            .map_or(0, |body| body.len().div_ceil(4)),
    );
    manifest.system_prompt_tokens = messages
        .iter()
        .find(|message| message.role == "system")
        .map(|message| message.content.len().div_ceil(4));
}

pub(super) fn build_conversation(
    prior_messages: &[ChatMessage],
    summary: Option<&ConversationSummary>,
    window_tokens: usize,
    scanner: &SecretScanner,
) -> PromptConversation {
    // A plan reminder is the engine's nudge, not the user's words: seeding a
    // later turn with it would present an instruction as something the user
    // said (spec 66 `context.md` D10).
    let conversation_messages =
        crate::compaction::sendable_messages(&prior_messages.iter().collect::<Vec<_>>());

    // Spec 55 §5.6: the summary is delimited recorded data, inside the
    // conversation block. It is never merged into the system prompt and never
    // placed where repository instructions live. A stored summary is local
    // data, like the transcript; the objective is quoted byte for byte in it.
    // Redact at the boundary so a secret in an earlier message cannot leave the
    // machine through the summary after being caught in the message itself.
    let summary = summary.map(|summary| scanner.redact(&summary.render_for_model()).text);

    // Spec 55 §5.1: the window is a token budget, not eight messages, so a
    // conversation of short messages keeps more turns than one of long ones.
    // The summary is part of the block and is charged to the same budget: it is
    // what the conversation may spend in total, not what the messages may spend
    // with the summary free. Without this a compacted request would carry the
    // summary *plus* a full window and exceed the threshold that triggered the
    // compaction.
    let summary_tokens = summary
        .as_ref()
        .map(|rendered| TokenEstimate::Prose.tokens(rendered))
        .unwrap_or(0);
    let recent_messages = crate::compaction::window_slice(
        &conversation_messages,
        window_tokens.saturating_sub(summary_tokens),
    );
    let omitted = conversation_messages.len() - recent_messages.len();

    let mut body = String::new();
    // §5.5: with no active summary covering the omitted messages, the loss is
    // stated rather than left silent. Messages the summary covers are not in
    // `prior_messages` at all, so `omitted` counts only those outside both the
    // summary and the window.
    if omitted > 0 {
        body.push_str(&format!(
            "[Damaian: {omitted} earlier message{} in this conversation were not sent to you \
             and were not summarised. The full transcript is still stored in the session and \
             is readable in the app.]\n",
            if omitted == 1 { "" } else { "s" }
        ));
    }
    for message in recent_messages {
        body.push_str(&message.role);
        body.push_str(": ");
        // Stored messages are verbatim by spec 16's design, so a secret typed
        // in an earlier turn is still in the transcript. Redact before
        // truncating: truncating first could leave half a secret straddling the
        // cut with nothing left to recognise it (C17).
        let redacted = scanner.redact(&message.content).text;
        body.push_str(&truncate_for_prompt(&redacted, 2_000));
        body.push('\n');
    }

    PromptConversation {
        summary,
        body: (!body.is_empty()).then_some(body),
    }
}

#[cfg(test)]
pub(super) fn build_model_prompt(
    plan: &ContextPlan,
    prior_messages: &[ChatMessage],
    summary: Option<&ConversationSummary>,
    window_tokens: usize,
    scanner: &SecretScanner,
    command_context: Option<&str>,
) -> String {
    let conversation = build_conversation(prior_messages, summary, window_tokens, scanner);
    build_model_prompt_with_conversation(plan, &conversation, command_context)
}

/// Join a plan's sections with an already-built conversation block.
///
/// The split exists so a paused review can re-render the request from an
/// amended plan without the transcript (spec 27 §5.1). The output is exactly
/// what `build_model_prompt` produced before the split.
pub(super) fn build_model_prompt_with_conversation(
    plan: &ContextPlan,
    conversation: &PromptConversation,
    command_context: Option<&str>,
) -> String {
    let mut output = String::new();
    let mut request = String::new();
    for section in plan.sections() {
        match section.name {
            PromptSectionName::RepositoryContext => {
                output.push_str(section.name.heading());
                output.push('\n');
                output.push_str(&section.text);
            }
            PromptSectionName::UserRequest => request = section.text,
            // The conversation block belongs to the caller: the plan holds no
            // messages. It is written between these two below.
            PromptSectionName::RecentConversation => {}
        }
    }

    if conversation.summary.is_some() || conversation.body.is_some() {
        output.push('\n');
        output.push_str(PromptSectionName::RecentConversation.heading());
        output.push('\n');
        if let Some(summary) = &conversation.summary {
            output.push_str(summary);
            output.push('\n');
        }
        if let Some(body) = &conversation.body {
            output.push_str(body);
        }
    }

    // The request comes only from the plan's redacted `Required` item. The
    // caller's prompt is never rendered, and there is deliberately no fallback
    // to it: a fallback would reopen the leak the moment the item was absent
    // (Correction C17).
    output.push('\n');
    output.push_str(PromptSectionName::UserRequest.heading());
    output.push('\n');
    output.push_str(&request);
    output.push('\n');

    if let Some(command_context) = command_context {
        output.push_str("\n--- command_result ---\n");
        output.push_str(command_context);
        if !command_context.ends_with('\n') {
            output.push('\n');
        }
    }
    output
}

/// Clamp the within-turn message array to a bounded window, stating what was
/// elided.
///
/// The first two messages are the initial system prompt and the user request
/// and are always kept. The rest are rounds — an assistant message and its
/// results, one per call — and are dropped from the oldest end in whole rounds,
/// so a tool result is never left without the call it answers. The session log
/// still holds every round; only the request is bounded. Spec 47 requirement 6,
/// the half `OBSERVATIONS.md` entry 6 names.
///
/// Spec 65 adds up to three blocks *beside* the bounded tail: the turn's notes
/// after the seed, the notes-aware elision notice, and a warning naming the
/// oldest round still in view. The round budget stays exactly
/// `max_messages - 3`, so the request may exceed `max_messages` by the blocks'
/// size and the number of rounds kept is unchanged. With notes off
/// (`NotesWindow::none()`) the result is byte-for-byte today's.
pub(super) fn bounded_messages(
    messages: &[ModelMessage],
    max_messages: usize,
    window: &NotesWindow<'_>,
) -> Vec<ModelMessage> {
    let head_len = messages.len().min(HEAD_MESSAGES);
    let head = messages[..head_len].to_vec();
    let tail = &messages[head_len..];
    // The head and the notice are never dropped, so the effective floor is
    // three messages whatever a repository sets.
    let budget = max_messages.saturating_sub(3);
    let over = messages.len() > max_messages;
    let mut drop_count = if over {
        tail.len().saturating_sub(budget)
    } else {
        0
    };
    // Whole rounds: every round opens with its assistant message, and a
    // batched round's one message is followed by a result per call, so the
    // cut lands on a round's start. Stopping inside a round would leave a
    // tool result without its call, which providers reject. The cut moves
    // forward to the next round — unless there is none, because dropping the
    // latest round would take away the results the model just asked for.
    // Then it moves back to that round's start, and the request exceeds the
    // bound by the part of one round that does not fit.
    if over && drop_count < tail.len() && tail[drop_count].role != "assistant" {
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
    if window.enabled && !window.notes.is_empty() {
        let header = if window.carried {
            CARRIED_NOTES_HEADER
        } else {
            NOTES_HEADER
        };
        bounded.push(ModelMessage::system(format!("{header}\n{}", window.notes)));
    }
    if over {
        bounded.push(ModelMessage::system(if window.enabled {
            elision_notice(drop_count, results, window)
        } else {
            format!(
                "Earlier rounds of this turn were elided to bound the request: {drop_count} messages ({results} call results) omitted. The session log still holds them; repeat a read if you need it."
            )
        }));
    }
    bounded.extend_from_slice(&tail[drop_count..]);
    if window.enabled
        && tail.len() >= budget
        && drop_count < tail.len()
        && let Some(warning) = pre_cut_warning(drop_count, window)
    {
        bounded.push(ModelMessage::system(warning));
    }
    bounded
}

/// The request selected for one model call, plus the call ids Task 4 compacted.
pub(super) struct PreparedTurnRequest {
    pub(super) request: ModelRequest,
    pub(super) compacted_call_ids: BTreeSet<String>,
}

impl PreparedTurnRequest {
    pub(super) fn unchanged(request: ModelRequest) -> Self {
        Self {
            request,
            compacted_call_ids: BTreeSet::new(),
        }
    }
}

/// What one request-preparation call decided (spec 103 Task 4 and Task 6).
///
/// A request that cannot fit is a typed terminal outcome rather than a
/// `ClientError`: the engine must settle a task status that names the refusal,
/// not hand a generic preparation failure to a caller. Invalid compaction
/// metadata is deliberately not this variant; it remains a fail-closed error.
pub(super) enum TurnRequestDecision {
    Prepared(PreparedTurnRequest),
    CannotFit(WithinTurnCompactionAttempt),
}

impl TurnRequestDecision {
    pub(super) fn prepared(request: ModelRequest) -> Self {
        Self::Prepared(PreparedTurnRequest::unchanged(request))
    }
}

/// The safety-first selection used only once the request would cross the
/// window share (spec 103 Task 4).
///
/// Precedence, in order: the system/user head, every system/user message, and
/// every completed round holding a pinned result are kept; the configured
/// message budget then bounds the remaining unpinned completed rounds, oldest
/// first. This never splits an assistant round from its results, and it cannot
/// send fewer pinned values than the legacy cut because it only ever keeps more.
pub(super) fn bounded_messages_preserving_safety(
    messages: &[ModelMessage],
    max_messages: usize,
    window: &NotesWindow<'_>,
    pinned_call_ids: &BTreeSet<String>,
) -> Vec<ModelMessage> {
    let head_len = messages.len().min(HEAD_MESSAGES);
    let mut protected = vec![false; messages.len()];
    for (index, message) in messages.iter().enumerate() {
        // System and user text is never elided. A plain assistant answer is not
        // part of a completed tool round either, so it stays too; only an
        // assistant tool-call message is a round declaration that may be cut.
        if index < head_len
            || message.role == "system"
            || message.role == "user"
            || (message.role == "assistant" && message.tool_calls.is_empty())
        {
            protected[index] = true;
        }
    }
    for (index, message) in messages.iter().enumerate() {
        if message.role == "tool"
            && let Some(call_id) = &message.tool_call_id
            && pinned_call_ids.contains(call_id)
        {
            protected[index] = true;
        }
        if message.role == "assistant"
            && message
                .tool_calls
                .iter()
                .any(|call| pinned_call_ids.contains(&call.id))
        {
            protected[index] = true;
            // A pinned declaration keeps its whole round, so no tool result is
            // left without the call it answers.
            let mut next = index + 1;
            while next < messages.len() && messages[next].role != "assistant" {
                protected[next] = true;
                next += 1;
            }
        }
    }

    let mut dropped = vec![false; messages.len()];
    let budget = max_messages.saturating_sub(3);
    let mut kept_tail = messages.len().saturating_sub(head_len);
    if messages.len() > max_messages {
        let mut index = head_len;
        while index < messages.len() {
            if messages[index].role != "assistant" {
                index += 1;
                continue;
            }
            let start = index;
            let mut end = index + 1;
            while end < messages.len() && messages[end].role != "assistant" {
                end += 1;
            }
            let group_len = end - start;
            let group_protected = (start..end).any(|position| protected[position]);
            if !group_protected && kept_tail > budget {
                for slot in dropped.iter_mut().take(end).skip(start) {
                    *slot = true;
                }
                kept_tail = kept_tail.saturating_sub(group_len);
            }
            index = end;
        }
    }

    let dropped_messages = dropped.iter().filter(|dropped| **dropped).count();
    let mut bounded = messages[..head_len].to_vec();
    if window.enabled && !window.notes.is_empty() {
        let header = if window.carried {
            CARRIED_NOTES_HEADER
        } else {
            NOTES_HEADER
        };
        bounded.push(ModelMessage::system(format!("{header}\n{}", window.notes)));
    }
    if dropped_messages > 0 {
        bounded.push(ModelMessage::system(safety_elision_notice(
            messages, &dropped, window,
        )));
    }
    for (index, message) in messages.iter().enumerate().skip(head_len) {
        if !dropped[index] {
            bounded.push(message.clone());
        }
    }
    if window.enabled
        && kept_tail >= budget
        && let Some(warning) = safety_pre_cut_warning(messages, &dropped, window)
    {
        bounded.push(ModelMessage::system(warning));
    }
    bounded
}

fn safety_elision_notice(
    messages: &[ModelMessage],
    dropped: &[bool],
    window: &NotesWindow<'_>,
) -> String {
    let message_count = dropped.iter().filter(|dropped| **dropped).count();
    let results = messages
        .iter()
        .zip(dropped)
        .filter(|(message, dropped)| **dropped && message.role == "tool")
        .count();
    let mut notice = format!(
        "Earlier rounds of this turn were elided to bound the request: {message_count} messages ({results} call results) omitted."
    );
    let elided = window
        .calls
        .iter()
        .filter(|call| dropped.get(call.message_index).copied().unwrap_or(false));
    let (lines, more) = rendered_call_lines(elided, window.notice_cap);
    if !lines.is_empty() {
        notice.push_str(" Calls already made, oldest first:\n");
        notice.push_str(&lines.join("\n"));
        if more > 0 {
            notice.push_str(&format!("\n  …and {more} more"));
        }
    }
    notice.push_str("\nThe session log still holds every result. Your notes are above.");
    notice
}

fn safety_pre_cut_warning(
    messages: &[ModelMessage],
    dropped: &[bool],
    window: &NotesWindow<'_>,
) -> Option<String> {
    let oldest = window.calls.iter().find(|call| {
        !dropped.get(call.message_index).copied().unwrap_or(false)
            && messages
                .get(call.message_index)
                .is_some_and(|message| message.role != "tool")
    })?;
    let (lines, more) = rendered_call_lines(
        window.calls.iter().filter(|call| {
            call.round == oldest.round && !dropped.get(call.message_index).copied().unwrap_or(false)
        }),
        window.warning_cap,
    );
    let mut warning = format!(
        "The window is full. With your next request, round {} leaves it:",
        oldest.round
    );
    for line in lines {
        warning.push('\n');
        warning.push_str(&line);
    }
    if more > 0 {
        warning.push_str(&format!("\n…and {more} more in that round."));
    }
    Some(warning)
}

/// The path whose hash a later patch or finding safety check rests on, when the
/// evidence carries one. Only typed evidence variants are read.
pub(super) fn collect_safety_path(evidence: &Evidence, paths: &mut BTreeSet<String>) {
    match evidence {
        // A historical read is not a body pin. Its durable path and hash stay in
        // the step evidence, and the latest result for an open step is pinned by
        // call id, so pinning every earlier read too would leave a planned task
        // with no eligible result to compact. Patch paths are different: they
        // carry the base hashes a later safety decision verifies.
        Evidence::FileRead { .. } => {}
        Evidence::PatchApplied { files, .. } => {
            for file in files {
                paths.insert(file.path.clone());
            }
        }
        Evidence::CommandExit { .. }
        | Evidence::CommandDeclined { .. }
        | Evidence::Findings { .. } => {}
    }
}

/// Keeps only completed rounds whose every call still has both an assistant
/// declaration and a tool result in the selected request.
///
/// This is a structural presence check over ids, not a reading of message text.
/// The safety selection keeps whole rounds, so a round is normally all present
/// or all absent; a partially present round is left out rather than described to
/// the reducer as a pair that does not exist.
pub(super) fn present_completed_rounds(
    messages: &[ModelMessage],
    rounds: &[CompletedToolRound],
) -> Vec<CompletedToolRound> {
    let declarations: BTreeSet<&str> = messages
        .iter()
        .filter(|message| message.role == "assistant")
        .flat_map(|message| message.tool_calls.iter().map(|call| call.id.as_str()))
        .collect();
    let results: BTreeSet<&str> = messages
        .iter()
        .filter(|message| message.role == "tool")
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect();
    rounds
        .iter()
        .filter(|round| {
            round.calls.iter().all(|call| {
                declarations.contains(call.call_id.as_str())
                    && results.contains(call.call_id.as_str())
            })
        })
        .cloned()
        .collect()
}

/// What `bounded_messages` may pin into a request (spec 65 §5.2).
///
/// `none()` is the key = 0 shape: no notes block, no warning, and today's
/// notice literal, so the request is exactly what it was before this spec.
pub(super) struct NotesWindow<'a> {
    pub(super) notes: &'a str,
    pub(super) carried: bool,
    pub(super) calls: &'a [RecordedCall],
    pub(super) enabled: bool,
    pub(super) notes_limit: usize,
    pub(super) notice_cap: usize,
    pub(super) warning_cap: usize,
}

impl NotesWindow<'_> {
    pub(super) fn none() -> NotesWindow<'static> {
        NotesWindow {
            notes: "",
            carried: false,
            calls: &[],
            enabled: false,
            notes_limit: 0,
            notice_cap: 0,
            warning_cap: 0,
        }
    }
}

/// The calls already made, oldest first, capped with "…and N more".
fn rendered_call_lines<'a>(
    calls: impl IntoIterator<Item = &'a RecordedCall>,
    cap: usize,
) -> (Vec<String>, usize) {
    let calls: Vec<&RecordedCall> = calls.into_iter().collect();
    let shown = calls.len().min(cap);
    let lines = calls[..shown]
        .iter()
        .map(|call| format!("  {}", call.line))
        .collect();
    (lines, calls.len().saturating_sub(shown))
}

/// The notice that replaces today's literal when notes are on and rounds were
/// cut. It names the calls that left the window, not just how many messages
/// did, so the model can tell what it has already looked at.
fn elision_notice(drop_count: usize, results: usize, window: &NotesWindow<'_>) -> String {
    let mut notice = format!(
        "Earlier rounds of this turn were elided to bound the request: {drop_count} messages ({results} call results) omitted."
    );
    let boundary = HEAD_MESSAGES + drop_count;
    let elided = window
        .calls
        .iter()
        .filter(|call| call.message_index < boundary);
    let (lines, more) = rendered_call_lines(elided, window.notice_cap);
    if !lines.is_empty() {
        notice.push_str(" Calls already made, oldest first:\n");
        notice.push_str(&lines.join("\n"));
        if more > 0 {
            notice.push_str(&format!("\n  …and {more} more"));
        }
    }
    notice.push_str("\nThe session log still holds every result. Your notes are above.");
    notice
}

/// The warning that names the oldest round still in view once the tail fills
/// the window (spec 65 §5.4). The model gets one round to record what it still
/// needs before those results are gone.
fn pre_cut_warning(drop_count: usize, window: &NotesWindow<'_>) -> Option<String> {
    let boundary = HEAD_MESSAGES + drop_count;
    let oldest = window
        .calls
        .iter()
        .find(|call| call.message_index >= boundary)?;
    let (lines, more) = rendered_call_lines(
        window
            .calls
            .iter()
            .filter(|call| call.round == oldest.round && call.message_index >= boundary),
        window.warning_cap,
    );
    let mut warning = format!(
        "The window is full. With your next request, round {} leaves it:",
        oldest.round
    );
    for line in lines {
        warning.push('\n');
        warning.push_str(&line);
    }
    if more > 0 {
        warning.push_str(&format!("\n  …and {more} more"));
    }
    warning.push_str("\nRecord in turn_notes anything from these results you still need.");
    warning.push_str(&format!(
        "\nNotes: {} / {} characters.",
        window.notes.chars().count(),
        window.notes_limit
    ));
    Some(warning)
}

/// Render one canonical call line from the segmented identity [`ChatOrchestrator::call_key`]
/// produced. Reading the identity rather than re-resolving a path is what
/// keeps the elision notice and the repeat rule from drifting apart.
pub(super) fn render_call_key(key: &[String], repository_root: &Path) -> Option<String> {
    let tool = key.first()?.as_str();
    let segment_after = |tag: &str| {
        key.iter()
            .position(|segment| segment == tag)
            .and_then(|index| key.get(index + 1))
    };
    let mut line = match tool {
        "read_file" => {
            let path = key.get(2)?;
            let mut line = format!("read_file {path}");
            if let Some(index) = key.iter().position(|segment| segment == "range") {
                line.push_str(&format!(" {}–{}", key.get(index + 1)?, key.get(index + 2)?));
            }
            line
        }
        "list_directory" => {
            let path = key.get(2)?;
            let mut line = format!("list_directory {path}");
            if let Some(depth) = segment_after("depth") {
                line.push_str(&format!(" depth {depth}"));
            }
            line
        }
        "search_content" => {
            let pattern = key.get(2)?;
            let mut line = format!("search_content \"{pattern}\"");
            if let Some(glob) = segment_after("glob") {
                line.push_str(&format!(" in {glob}"));
            }
            if let Some(max) = segment_after("max") {
                line.push_str(&format!(" max {max}"));
            }
            line
        }
        "search_codebase" => {
            let query = key.get(1)?;
            let mut line = format!("search_codebase \"{query}\"");
            if key.get(2).map(String::as_str) == Some("true") {
                line.push_str(" semantic");
            }
            if let Some(limit) = key.get(3) {
                line.push_str(&format!(" limit {limit}"));
            }
            line
        }
        "read_git_status" => "read_git_status".to_string(),
        "read_git_diff" => {
            let mut line = "read_git_diff".to_string();
            if key.get(2).map(String::as_str) == Some("true") {
                line.push_str(" staged");
            }
            if key.get(3).map(String::as_str) == Some("true") {
                line.push_str(" stat");
            }
            if let Some(base) = segment_after("base") {
                line.push_str(&format!(" base {base}"));
            }
            if let Some(index) = key.iter().position(|segment| segment == "paths") {
                let paths = &key[index + 1..];
                if !paths.is_empty() {
                    line.push_str(&format!(" paths {}", paths.join(",")));
                }
            }
            line
        }
        "list_worktrees" => {
            let mut line = "list_worktrees".to_string();
            if let Some(base) = segment_after("base") {
                line.push_str(&format!(" base {base}"));
            }
            line
        }
        "read_git_log" => {
            let mut line = "read_git_log".to_string();
            if let Some(base) = segment_after("base") {
                line.push_str(&format!(" base {base}"));
            }
            if let Some(max) = segment_after("max") {
                line.push_str(&format!(" n {max}"));
            }
            line
        }
        "fetch_url" => format!("fetch_url {}", key.get(1)?),
        // `turn_notes` and every non-read call have no canonical row; the
        // caller falls back to `tool_action_marker`.
        _ => return None,
    };
    // The checkout is segment 1 for every read tool that has one.
    let checkout = match tool {
        "search_codebase" | "list_worktrees" | "turn_notes" | "fetch_url" => None,
        _ => key.get(1),
    };
    if let Some(tag) = checkout
        && let Some(display) = checkout_display(tag, repository_root)
    {
        line.push_str(&format!(" (worktree {display})"));
    }
    Some(line)
}

/// How a `call_key` checkout tag is named in a rendered line: nothing for the
/// session's own checkout, the given argument for one that could not be
/// resolved, and the repository-relative path for a registered worktree.
fn checkout_display(tag: &str, repository_root: &Path) -> Option<String> {
    if tag == "checkout:session" {
        return None;
    }
    if let Some(given) = tag.strip_prefix("checkout-as-given:") {
        return Some(given.to_string());
    }
    let path = tag.strip_prefix("checkout:")?;
    let path = Path::new(path);
    Some(
        path.strip_prefix(repository_root)
            .map(|relative| relative.display().to_string())
            .unwrap_or_else(|_| path.display().to_string()),
    )
}

/// Collapse every run of whitespace to one space, so a marker's reference can
/// never add a line to the notice.
pub(super) fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A stable 64-bit FNV-1a hash, as hex. `DefaultHasher` is not guaranteed
/// stable across Rust versions, and a notes key is persisted with a paused
/// turn, so the repeat rule must not change when the binary is rebuilt.
pub(super) fn stable_text_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
