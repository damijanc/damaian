//! Turn progress as server-sent events: the event type, the relay from the
//! engine's progress channel, and the streamed turn itself.

use super::*;

/// What the worker running a turn reports back to the handler holding the socket.
pub(crate) enum TurnEvent {
    Session(String),
    Phase(TurnPhase),
    /// Boxed because a `TaskPlan` is much larger than the other variants, and
    /// an unboxed one would make every `TurnEvent` — one per streamed token —
    /// carry its footprint. `active` is false only for the final event of a
    /// turn that ended (spec 66 §5.4).
    Plan {
        plan: Box<TaskPlan>,
        active: bool,
    },
    /// Boxed for the same reason as `Plan`: a record carries a whole report.
    WebDiagnostic(Box<WebDiagnosticRecord>),
    /// What the turn is about to send (spec 27 §5.1). Boxed because a manifest
    /// carries every included ref and exclusion.
    Context(Box<ContextManifest>),
    Token(String),
    /// A note the user sent while the turn worked entered the conversation
    /// as this user message (spec 110 D7).
    Note(String),
    /// Notes the turn ended without delivering, oldest first, sent just before
    /// its `Done` or `Failed` so the window can send them as the next turn or
    /// put them back in the composer (D7). A turn that paused carries them
    /// to the turn that resumes it instead, and sends none.
    NotesReturned(Vec<String>),
    Done(Box<ChatTurnResult>),
    Failed(String),
}

/// Forwards a turn's events to the client as SSE, and stops the turn if the
/// client goes away.
///
/// Runs on the thread that owns the socket while the turn itself runs on a
/// worker, because the turn spends most of its time blocked on the provider and
/// could not otherwise notice a disconnect.
pub(crate) fn relay_turn_events<W: Write>(
    out: &mut W,
    cancel: &CancelToken,
    notes: Option<&std::sync::Arc<lanes::NoteQueue>>,
    events: std::sync::mpsc::Receiver<TurnEvent>,
    keepalive: Duration,
) -> Result<(), String> {
    let mut client_gone = false;
    loop {
        match events.recv_timeout(keepalive) {
            Ok(event) => {
                let finished = matches!(event, TurnEvent::Done(_) | TurnEvent::Failed(_));
                // Spec 110 D3: the lane says what the session is doing, and a
                // turn that created its session claims that lane now.
                match &event {
                    TurnEvent::Session(session_id) => {
                        lanes::adopt_current(session_id, routes::Route::AskStream.work_label());
                        lanes::with_current(|lane| {
                            lane.set_cancel(cancel);
                            if let Some(notes) = notes {
                                lane.set_notes(notes);
                            }
                        });
                    }
                    TurnEvent::Phase(phase) if !phase.label.trim().is_empty() => {
                        lanes::with_current(|lane| lane.set_label(&phase.label));
                    }
                    _ => {}
                }
                if !client_gone {
                    let written = match &event {
                        TurnEvent::Session(session_id) => write_sse_event(
                            out,
                            "session",
                            &format!("{{\"sessionId\":\"{}\"}}", escape_json(session_id)),
                        ),
                        TurnEvent::Phase(phase) => {
                            write_sse_event(out, "phase", &phase_json(phase))
                        }
                        TurnEvent::Plan { plan, active } => {
                            write_sse_event(out, "plan", &plan_json(plan, *active))
                        }
                        TurnEvent::WebDiagnostic(record) => {
                            write_sse_event(out, "web_diagnostic", &web_diagnostic_json(record))
                        }
                        TurnEvent::Context(manifest) => write_sse_event(
                            out,
                            "context",
                            &serde_json::to_string(manifest).unwrap_or_default(),
                        ),
                        TurnEvent::Token(token) => write_sse_event(
                            out,
                            "token",
                            &format!("{{\"token\":\"{}\"}}", escape_json(token)),
                        ),
                        TurnEvent::Note(message_id) => {
                            let text = notes.and_then(|notes| notes.delivered());
                            write_sse_event(
                                out,
                                "note",
                                &serde_json::json!({ "messageId": message_id, "text": text })
                                    .to_string(),
                            )
                        }
                        TurnEvent::NotesReturned(returned) => write_sse_event(
                            out,
                            "notes_returned",
                            &serde_json::json!({ "notes": returned }).to_string(),
                        ),
                        TurnEvent::Done(result) => {
                            write_sse_event(out, "done", &chat_result_json(result))
                        }
                        TurnEvent::Failed(error) => {
                            write_sse_event(out, "error", &json_error(&friendly_chat_error(error)))
                        }
                    };
                    if written.is_err() {
                        client_gone = true;
                        cancel.cancel();
                    }
                }
                if finished {
                    return Ok(());
                }
            }
            // Nothing to send: poke the socket so a departed client is noticed.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if client_gone {
                    continue;
                }
                // An SSE comment. The client ignores it; the kernel does not.
                if out
                    .write_all(b": keepalive\n\n")
                    .and_then(|_| out.flush())
                    .is_err()
                {
                    client_gone = true;
                    cancel.cancel();
                }
            }
            // The worker dropped its sender, so the turn is over either way.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Runs `turn` on a worker thread and relays its events to `stream`.
pub(crate) fn stream_turn<F>(stream: &mut TcpStream, turn: F) -> Result<(), String>
where
    F: FnOnce(&CancelToken, &std::sync::mpsc::Sender<TurnEvent>) -> Result<ChatTurnResult, String>
        + Send
        + 'static,
{
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    // Spec 110 D7: the turn takes notes sent while it works. Notes a paused
    // turn of this session left are queued first, so the turn that resumes it
    // delivers them at its first boundary.
    let notes = lanes::NoteQueue::new();
    let data_dir = effective_data_dir().ok();
    if let (Some(session_id), Some(data_dir)) = (lanes::current_session(), data_dir.as_deref()) {
        for note in lanes::take_carried_notes(data_dir, &session_id) {
            notes.queue(note);
        }
    }
    let worker_notes = std::sync::Arc::clone(&notes);
    let worker = spawn_with_shell_stack("damaian-turn", move || {
        // Spec 109 D8: hooks, MCP calls and browser diagnostics reach the
        // turn's Stop through the thread's scoped token.
        let outcome = worker_cancel
            .scope(|| lanes::with_turn_notes(&worker_notes, || turn(&worker_cancel, &sender)));
        let left = worker_notes.close();
        let returned = settle_left_notes(&outcome, left, data_dir.as_deref());
        if !returned.is_empty() {
            let _ = sender.send(TurnEvent::NotesReturned(returned));
        }
        let event = match outcome {
            Ok(result) => TurnEvent::Done(Box::new(result)),
            Err(error) => TurnEvent::Failed(error),
        };
        let _ = sender.send(event);
        // `sender` drops here, which is what disconnects the channel and lets
        // the relay finish even if the terminal event could not be delivered.
    })
    .map_err(|error| format!("start the turn thread: {error}"))?;

    lanes::with_current(|lane| {
        lane.set_cancel(&cancel);
        lane.set_notes(&notes);
    });
    let outcome = relay_turn_events(stream, &cancel, Some(&notes), receiver, KEEPALIVE_INTERVAL);
    // Joined unconditionally: the worker owns the curl child, and leaving it
    // unreaped is how a stopped turn would keep billing tokens.
    let _ = worker.join();
    outcome
}

/// Where the notes a turn ended without delivering go (spec 110 D7). A turn
/// paused for an approval or a question carries them to the turn that resumes
/// it, which delivers them at its first boundary; the window gets them back
/// otherwise: completed or failed, it sends them as the next turn; stopped, it
/// puts them back in the composer. If they cannot be carried, they are
/// returned rather than lost.
fn settle_left_notes(
    outcome: &Result<ChatTurnResult, String>,
    left: Vec<String>,
    data_dir: Option<&Path>,
) -> Vec<String> {
    if left.is_empty() {
        return left;
    }
    match (outcome, data_dir) {
        (Ok(result), Some(data_dir)) if turn_is_paused(result) => {
            match lanes::carry_notes(data_dir, &result.session.id, &left) {
                Ok(()) => Vec::new(),
                Err(_) => left,
            }
        }
        _ => left,
    }
}

/// A turn waiting on a decision that resumes the same turn. A patch proposal
/// is not one: the turn is over, and applying the patch starts no resume.
fn turn_is_paused(result: &ChatTurnResult) -> bool {
    !result.cancelled
        && (result
            .command_proposal
            .as_ref()
            .is_some_and(|proposal| proposal.requires_approval)
            || result.plan_proposal.is_some()
            || result.question_proposal.is_some()
            || result.fetch_proposal.is_some()
            || result.fallback_proposal.is_some()
            || result.round_budget_proposal.is_some()
            || result.check_batch.is_some()
            || result.context_review.is_some())
}
