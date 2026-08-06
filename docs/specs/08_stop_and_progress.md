# Feature Spec: Stoppable Chat Turns and Live Progress

Status: Not started
Order: 8 of 8
Related spec sections: `ai_coding_assistant_specification.md` §7.1 (Chat Interface — "Distinct UI states for thinking, waiting for approval, running command, applying patch, failed, and complete"), §7.5 (Model Adapter — `cancel(runId): void`, "Timeout and cancellation preserve user work", "Partial streamed output is clearly marked if incomplete"), §11 (Error Handling), §12.1 (Performance — "Chat streaming should begin as soon as provider streaming starts").

## 1. Motivation

A chat turn cannot be interrupted, and while it runs the user is told almost
nothing about what it is doing.

**Nothing can stop a turn.** `ChatOrchestrator::run_agentic_turn` (`chat.rs`)
runs up to `MAX_TOOL_ROUNDS = 6` model rounds, and each `CurlModelTransport`
call carries `max-time = 900` (`model.rs`). The worst case is roughly 90
minutes of unstoppable work. `ModelAdapter::cancel(run_id)` exists in the trait
exactly as §7.5 specifies, but it is dead code: no caller anywhere in the
workspace, and its only effect is to push a run id onto a `cancelled` vector
that marks the run `incomplete` *after* it has already finished. The interface
is present and the capability is not.

**Progress is a single static word.** `setChatStatus` (`app.js`) writes
`Thinking` when the request goes out and `Streaming` on the first token. Between
those two there is no signal at all — and that gap is precisely where the long
waits live. A user watching `Thinking` for ninety seconds cannot tell whether
the provider is slow, a tool is running, the turn is on model round 4 of 6, or
the process has hung. §7.1 requires distinct states for thinking, running a
command, and applying a patch; one text badge does not provide them.

**A stop leaves a wedged record.** `ask_with_session` (`chat.rs`) persists the
session, a `Running` task, and the user message *before* the first model call.
Any turn that does not reach its end leaves that task `Running` forever, since
`TaskStatus` has no terminal state for "the user stopped this" — only `Failed`
and `Complete`.

**A dead client does not stop the work.** `handle_ask_stream`
(`desktop-shell/src/lib.rs`) already notices a failed SSE write and records it
in `write_error`, but `on_token` is `&mut dyn FnMut(&str)` returning `()`, so
the callback has no way to signal upward. The turn runs to completion, billing
tokens into a closed socket.

### 1.1 The constraint that shapes the design

`run_server` (`desktop-shell/src/lib.rs`) accepts one connection and calls
`handle_connection` inline — no thread per connection. **The shell services
exactly one request at a time.** A `POST /api/cancel` sent during a chat turn
would sit in the accept queue until the turn it meant to cancel had already
finished.

This rules out the obvious design and points to the one below: cancellation is
driven by the *client closing the connection*, which needs no server round trip
and therefore works despite the serial accept loop.

## 2. Goals

- A user can stop an in-flight chat turn and have the underlying model request
  actually die, within a couple of seconds, including when no tokens have
  streamed yet.
- While a turn runs, the UI names the phase it is in and how long it has been
  going.
- A stopped turn preserves the user's work: the partial answer and the prompt
  both survive, and the session record says plainly that it was stopped.
- No orphaned `curl` process, reader thread, or `Running` task can outlive a
  stopped turn.

## 3. Non-Goals

- **Stopping anything other than a chat turn.** `/api/propose-edit`,
  approved shell commands, and indexing keep today's behaviour. See §7.
- **Threading the shell.** The serial accept loop stays. The rest of the UI
  remains blocked for the duration of a turn; that is a real problem and it is
  deliberately out of scope here (§7).
- **Cancelling inside an MCP tool call.** See §4.6.
- **Server-driven elapsed time.** The clock is client-side (§4.5).
- **A queue or a "stop after this round" soft stop.** Stop means stop.

## 4. Design

### 4.1 Cancellation token

New `crates/workspace-engine/src/cancel.rs`:

```rust
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self);
    pub fn is_cancelled(&self) -> bool;
    pub fn check(&self) -> Result<()>;   // Err(ClientError::Cancelled) if set
}
```

`Clone + Send + Sync`, so the shell keeps one handle while the worker holds
another. It reaches the orchestrator inside the `TurnSink` of §4.2, and is passed
as a plain `&CancelToken` to `ModelAdapter::stream_response` and
`ModelTransport::send_stream`, which have no use for the rest of the sink.

`Cancelled` is an internal control signal, not the turn's outcome. Every check
point raises it, and `run_agentic_turn` catches it at one place: on `Cancelled`
from any depth it runs the §4.7 persistence and returns
`Ok(ChatTurnResult { cancelled: true, .. })`. `Cancelled` never escapes
`run_agentic_turn` — a caller sees either a normal result, a result flagged
`cancelled`, or a genuine failure. Any other `Err` keeps today's behaviour and
marks the task `Failed`.

New `ClientError::Cancelled`, and `TaskStatus::Cancelled` (`as_str()` →
`"cancelled"`). `ModelAdapter::cancel(run_id)` and the `cancelled: Vec<String>`
field it feeds are **deleted** — the token replaces them. §7.5's cancellation
requirement is met by the token; keeping a second, non-functional spelling of
the same idea in the trait would be worse than removing it.

### 4.2 Worker thread behind the SSE handler

`handle_ask_stream` and `handle_resume_command_stream` stop running the turn
themselves. They spawn a worker and keep the socket:

```
handle_ask_stream (owns TcpStream)           worker thread
──────────────────────────────────           ─────────────
spawn worker, hold CancelToken   ──────────► run_chat_request(&form, &cancel, tx)
                                             builds engine, runs the turn
loop recv_timeout(1s):                       sends TurnEvent over mpsc
  Session/Phase/Token → write SSE   ◄──────
  Done/Failed         → write, break
  Timeout             → write ": keepalive"
                        write failed?
                          → cancel.cancel()
                            stop writing, keep draining
join worker
```

```rust
enum TurnEvent {
    Session(String),
    Phase(TurnPhase),
    Token(String),
    Done(Box<ChatTurnResult>),
    Failed(String),
}
```

`run_chat_request` already constructs its own `WorkspaceEngine`, so the worker
owns everything it touches and nothing non-`Send` crosses the boundary. The
callbacks keep running on the worker — they send instead of writing.

**The orchestrator needs a way to emit `session` and `phase`.** `on_token`
carries only token strings, so there is no channel for the §4.4 events. Rather
than growing the parameter list of `run_agentic_turn` — already carrying
`#[allow(clippy::too_many_arguments)]` — the `on_token` parameter is *replaced*
by one struct that groups the whole per-turn side channel:

```rust
pub enum TurnProgress {
    Session(String),
    Phase(TurnPhase),
}

pub struct TurnSink<'a> {
    pub on_token: &'a mut dyn FnMut(&str),
    pub on_progress: &'a mut dyn FnMut(TurnProgress),
    pub cancel: &'a CancelToken,
}
```

This lowers the argument count instead of raising it, and puts the three things
that are per-turn-and-not-per-request in one place.

`ChatOrchestrator::ask` (`chat.rs`) **keeps its current signature** and builds a
default sink internally — no-op progress, unset token. That matters: `ask` is the
entry point for `damaian-cli` and for roughly fifteen tests in
`workspace-engine/tests/foundation.rs`, none of which should need touching.
`ask_with_session` and `resume_after_command_decision` take the sink, and the
only callers of those are in `desktop-shell`.

Phases are emitted by the **orchestrator**, not the shell — it is the only layer
that knows when context assembly starts, which tool is running, and which round
it is on. The shell just forwards them.

**The keepalive is the disconnect detector.** A silent turn produces no writes,
so without it a closed socket goes unnoticed for as long as the model takes to
emit a first token. One write per second closes that hole.

**Drain, do not abandon.** Once a write has failed the handler stops writing but
keeps calling `recv_timeout`. Abandoning the channel would block the worker on a
full `mpsc` send, and it would never reach its next cancellation check.

### 4.3 Interruptible transport

`CurlModelTransport::send_stream` currently blocks in `stdout.read(&mut buffer)`,
which is why nothing can observe a cancellation during a model call. It gains
the same shape as §4.2: a reader thread pumps `child.stdout` into a channel, and
the calling thread does `recv_timeout(250ms)` so it can check the token while the
provider is silent.

On cancellation, in this order: `child.kill()` → `child.wait()` (reap) → join
the reader thread → `Err(ClientError::Cancelled)`. A `Drop` guard on the child
covers a panic on the calling thread, which would otherwise skip the kill and
leak `curl` for the remainder of its 900-second `max-time`.

A reader thread is used rather than `O_NONBLOCK` + `libc`: no `unsafe`, no new
dependency, no `deny.toml` change, and it matches the threading already in
`index_cache.rs`.

### 4.4 SSE protocol additions

`processSseEvent` (`app.js`) leaves `event` as `"message"` for anything it does
not recognise and fires no handler, so these are backward compatible.

| Event | Payload | Purpose |
|-------|---------|---------|
| `session` | `{"sessionId":"…"}` | Emitted immediately after the session and task are created, **before** context assembly. |
| `phase` | `{"phase":"context\|model\|tool\|finalizing","label":"Reading src/auth.rs","round":2,"maxRounds":6}` | Drives the indicator. `label` is server-supplied so the frontend never needs to know tool names. `round` is **1-based on the wire** — `run_agentic_turn`'s counter starts at 0, so the conversion happens once, at emit, rather than being re-derived in the frontend. `maxRounds` is `MAX_TOOL_ROUNDS`. |
| `: keepalive` | SSE comment | Disconnect detection (§4.2). |

`session` is not a convenience. On a brand-new session the id is generated
server-side and only reaches the client in `done` — which a stopped client never
receives. Without this event, stopping the first turn of a new session orphans
it: the record exists on disk but the UI cannot name it, list it, or reload it.

### 4.5 Frontend

**Stop affordance.** `#ask-btn` stays enabled during a turn instead of being
disabled (`app.js`), swapping to a stop glyph with
`aria-label="Stop generating"`. `Escape` also stops, when a turn is in flight
and no dialog or popover is open.

Stopping is an `AbortController` passed to the `fetch` in `streamRequest`, held
on a module-level `currentTurn`. `controller.abort()` closes the socket; the
server side follows from §4.2. No request is sent — which is what makes Stop
work at all under the serial accept loop (§1.1).

**Indicator.** One factory owning its DOM row, its interval, and its teardown:

```js
const indicator = startTurnIndicator(assistantMessage);
indicator.phase({ phase: "tool", label: "Reading src/auth.rs", round: 2, maxRounds: 6 });
indicator.streaming();          // first token arrived
indicator.finish("stopped");    // | "complete" | "incomplete" | "failed"
```

`finish()` is idempotent so the error path can call it unconditionally.

| Signal | Displayed |
|--------|-----------|
| `phase: context` | `Assembling context… 2s` |
| `phase: model` | `Waiting for model… 14s` |
| first `token` | `Streaming… 21s` — client-side flip, no server event |
| `phase: tool` | `Reading src/auth.rs… · round 2/6` |
| `phase: finalizing` | `Finalizing…` |

The elapsed clock is a client-side `setInterval` from submit. It needs no round
trip and keeps ticking honestly when the server goes quiet — exactly when the
user most needs it.

**Two accessibility constraints.** `#chat-log` is `aria-live="polite"`
(`index.html`), so a per-second counter inside it would be announced every
second. The elapsed span is therefore `aria-hidden="true"`, and only the phase
label announces, only on change. The pulse animation goes behind
`@media (prefers-reduced-motion: reduce)`, degrading to a static dot with the
text intact. New rules follow the existing `.status-badge[data-state="…"]`
pattern in `style.css`.

`#chat-status` gains a `Stopped` / `warn` state so the header badge and the
bubble agree.

**Composer.** The prompt is cleared **on submit** rather than after success, and
**restored on stop or failure**.

Today it is cleared only after the turn succeeds, while the user message is
echoed into the log immediately — so the same text sits in two places for the
whole turn, and the composer looks like it is holding something unsent. Next to
a Stop button that reads worse still.

Restoring on failure is the more important half: a long prompt must not
evaporate because a provider timed out. Guard: if the composer is non-empty when
the turn ends, leave it alone — the user typed something new, and the original is
in the session history either way.

No paired restore for context chips. `dismissContextChips` only hides them;
`pinnedContextFiles` survives.

### 4.6 Where cancellation takes effect

Checked at three boundaries where stopping is safe:

1. **Top of the agentic loop** (`chat.rs`), before each model round — stopping
   before round 3 of 6 saves a whole model call.
2. **Inside `send_stream`** (§4.3), killing `curl` mid-response.
3. **After a tool returns**, before its result is fed back to the model.

Deliberately *not* inside a tool. The native tools are safe to let finish:
`read_file`, `search_codebase`, `read_git_status` and `read_git_diff` are fast
and read-only, `propose_patch` only produces a proposal the user must still
approve, and `run_command` pauses the turn for approval so nothing dangerous is
ever mid-flight.

**Known limitation.** MCP tool calls are only approval-gated when
`mcp.requires_approval(&server_id)` is true (`chat.rs`). A server configured
without approval executes inline — a third-party network call that may be slow
and have real side effects. Stop does not interrupt one; it takes effect at the
next check point after it returns. The UI has already said `Stopped`, so the
user's mental model stays correct, and the audit log records what actually
happened. Interrupting these requires cancellation support in the MCP client
layer and is out of scope.

### 4.7 Persistence on cancel

- Partial assistant text, if any, goes through `self.scanner.redact()` before
  `append_message` — the same treatment as the completion path. **The redaction
  guarantee holds on this path too.** If nothing streamed, no assistant message
  is appended.
- `update_task_status(&task, TaskStatus::Cancelled, None)`, converting the
  orphaned `Running` task (§1) into an honest record.
- A `chat_turn_cancelled` audit entry alongside the existing
  `model_request_prepared` records.
- Returns `Ok(ChatTurnResult { cancelled: true, .. })`, not `Err`. An `Err`
  would discard the session and task the caller needs, and it would collapse
  "the user stopped this" into the same bucket as "the provider failed" — a
  distinction both the status badge and the task history depend on.
  `chat_result_json` gains `cancelled`.

### 4.8 Stopped turns on reload

`/api/session` returns only `session` and `messages`, so reopening a session
containing a stopped turn renders the truncated answer as a complete, oddly
short reply — actively misleading, and a direct miss against §7.5's "partial
streamed output is clearly marked if incomplete".

`message_json` already carries `taskId`, so the fix is small: add
`tasks: [{id, status}]` to that payload and have `renderMessages` tag any
assistant message whose task is `cancelled`. The same `Stopped` treatment
applies live and on reload.

### 4.9 Error handling

| Case | Behaviour |
|------|-----------|
| Socket dies, worker still running | Handler stops writing, keeps draining, joins (§4.2). |
| Panic on the transport's calling thread | `Drop` guard kills and reaps the child (§4.3). |
| Stop pressed after the turn finished | `finish()` is idempotent, `currentTurn` is nulled on completion — a late `Escape` cannot abort the *next* turn. |
| Rapid submit after stop | Fresh `AbortController` per turn. The previous worker may take ~2s to wind down and the server is serial, so the new request waits in the accept queue. `Assembling context… 1s` while that resolves is honest. |
| Genuine network drop mid-turn | Trips the same keepalive detector and cancels the turn — server-side, correctly identical to a stop. The **client** distinguishes them: it called `abort()` or it did not. `AbortError` is filtered out of `sendChatPrompt`'s `catch`, or every stop would toast "Failed" and write an error into the bubble. |
| Cancel before the model is reached | Caught at the top of the loop. Task is `Cancelled` with no assistant message; §4.8 renders it as a stopped turn with an empty answer. |
| Non-streaming fallback (`/api/ask`) | No stream, no cancellation. Indicator shows a phase-less `Working…` and the button stays disabled. A dead path in the shipped webview, but it must not crash the indicator. |

**Detection latency, stated honestly.** On macOS the first write to a socket
whose peer has closed usually succeeds into the kernel buffer; the *second*
returns `EPIPE`. A stop is therefore detected in roughly one to two keepalive
ticks. The UI stops instantly because it is client-side; the model request dies
~1–2s later. Rust ignores `SIGPIPE` at startup, so this surfaces as an `Err`
rather than killing the shell.

## 5. Acceptance Criteria

- Pressing Stop 5s into a turn that has produced no tokens ends the turn, and
  the `curl` child is gone within 3s. Verified by PID, not by the UI.
- Pressing Stop mid-stream keeps the partial answer in the bubble, tagged
  `Stopped`, and the task is persisted as `cancelled`.
- Stopping the first turn of a brand-new session still lists that session
  without a reload — the `session` event arrived before the stop.
- Reopening a session containing a stopped turn shows the partial answer tagged
  `Stopped`, not as a complete reply.
- A partial answer containing a credential is redacted in the persisted message.
- During a turn the indicator moves through `Assembling context` →
  `Waiting for model` → `Streaming`, and shows a tool label with a round counter
  when the model calls a tool. Elapsed seconds advance throughout.
- With `prefers-reduced-motion: reduce`, the indicator animates not at all and
  still reads correctly.
- A screen reader announces phase changes and does **not** announce the elapsed
  count.
- Stopping produces no "Failed" toast and no error text in the bubble.
- A provider error still produces one, and still leaves the prompt recoverable.
- The prompt is cleared on submit, restored on stop and on failure, and never
  overwrites text typed during the turn.
- No `TaskStatus::Running` task survives a stopped turn.
- `ModelAdapter::cancel` no longer exists.
- `ChatOrchestrator::ask`'s signature is unchanged: `damaian-cli` and the
  existing `foundation.rs` tests compile without edits.
- The first model round reports `round 1/6`, not `round 0/6`.

## 6. Testing

`DAMAIAN_MOCK_MODEL_RESPONSE` and `DAMAIAN_DATA_DIR=.damaian` cover the
engine-side cases without an API key, a network, or touching real user data.

**`workspace-engine`**

- A token cancelled before the first round returns `Cancelled` with zero model
  calls (assert against `MockModelTransport::requests`).
- A token cancelled between rounds stops the loop; requests recorded is less
  than `MAX_TOOL_ROUNDS`.
- A cancelled turn persists the partial assistant message and a `cancelled`
  task; an empty partial persists no assistant message.
- A partial containing a credential is redacted in the stored message.
- `CancelToken::check` is `Ok` when unset, `Err(Cancelled)` when set.
- A transport whose reader stalls, then is cancelled, returns `Cancelled`
  rather than blocking — this is the regression test for §4.3, and it needs a
  stub that mimics a silent provider.

**`desktop-shell`**

- HTTP round trip against `/api/ask-stream` asserting `session` arrives before
  any `token`, and that a `phase` event precedes the first token.
- Closing the connection mid-stream causes the worker to exit and the task to
  land `cancelled` — the end-to-end proof of §4.2, using a mock model slow
  enough to stop within.
- `/api/session` includes `tasks` with statuses.

**Manual, `#[ignore]`d per the convention for real side effects**

- Real provider, real Stop, `pgrep -f 'curl.*chat/completions'` empty within 3s.
  Spawns a real network request, so it does not belong in `cargo test`.

**Frontend** has no test harness in this repo, so the UI criteria in §5 are
verified manually and recorded in §8.

## 7. Follow-ups (explicitly not in this spec)

1. **The rest of the UI is frozen during a chat turn.** The serial accept loop
   (§1.1) blocks session switching, file browsing and settings for the whole
   turn. Arguably a worse day-to-day irritation than the missing Stop button.
   Fixing it means threading `run_server`, which makes every handler concurrent
   at once and requires re-examining command policy, patch hash verification and
   secret redaction under concurrency. `index_cache.rs` is already
   `Mutex`-guarded and spawns its own threads, so the shared state is closer to
   ready than the accept loop suggests. Its own spec.
2. **`/api/propose-edit` cannot be stopped.** A plain JSON POST with no stream to
   abort, so during patch generation the button stays disabled and the user can
   still get stuck with no way out — the one remaining path with that problem.
   Converting the route to SSE would reuse this entire mechanism.
3. **Approved shell commands have no timeout at all** (`command_runner.rs`). A
   hung command wedges the serial shell indefinitely. Independent of this work
   and arguably more urgent.
4. **MCP call cancellation**, per §4.6.
