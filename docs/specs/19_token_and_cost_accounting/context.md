# Token and Cost Accounting — Context

Background for [`proposal.md`](proposal.md): why this work exists, what the code
looks like today, and the corrections found while planning it. The proposal is
the decision; this is the ground it stands on.

## 1. Motivation

Damaian spends the user's money and does not tell them how much.

A chat turn can make many model calls — the agent loop runs up to
`agent_max_tool_rounds` rounds, each with the assembled context resent — and the
only bound on any of it is a round count. A user who runs a long task on a
frontier model has no way to know afterwards whether it cost two cents or four
dollars, and no way to compare a cheap approach against an expensive one, because
nothing is recorded.

The gap is total rather than partial. `ModelRun` has no usage fields at all, the
streaming parser extracts message content and discards everything else, and
`Task` has nowhere to put a number. The only token figure in the codebase is a
`payload.len() / 4` estimate used for context budgeting, not reporting.

This work package only measures. The enforced per-task ceiling lands in Phase 2
alongside the plan machinery, because stopping a task cleanly needs it — but a
ceiling cannot be built on numbers that do not exist, and
[spec 18](../18_local_evaluation_harness/proposal.md)'s metric set has two rows
that report `notApplicable: "spec-19"` until this ships.

## 2. Current state

Verified against the tree on 2026-09-10, after specs 16, 17 and 18 landed. Line
numbers are a reading aid and drift; the symbol names are the durable part.

- **`ModelRun` carries no usage data** (`model.rs:172-191`): `run_id`, provider,
  model, timestamps, content, `incomplete`, `retry_count`, `tool_calls`,
  `truncated`, `reasoning_content`. No token counts, no cost.
- **The SSE reader discards everything but content.** `extract_model_tokens`
  (`model.rs:837`) walks `data:` lines, skips `[DONE]`, and passes each payload
  to `extract_content_values`. A provider's `usage` object is parsed for nothing.
- **The request body does not ask for usage.** `model_request_json`
  (`model.rs:770`) emits `model`, `messages`, `stream`, and optionally
  `temperature`, `max_tokens`, `reasoning_effort`, and `tools`. For
  OpenAI-compatible streaming APIs, usage is omitted from the stream unless
  `stream_options: {"include_usage": true}` is sent.
- **An input-side estimate already exists.** `ContextBundle.token_estimate`
  (`context_manager.rs:34`) accumulates the same `len / 4` approximation, and
  `chat.rs:945` already audits a per-round `tokenEstimate` on
  `model_request_prepared`.
- **Retries are counted but not costed.** `ModelRun.retry_count` is set at
  `model.rs:689` from the attempt counter. Each attempt that reached the provider
  is a billed call.
- **`Task` is replayed, not stored.** `read_task_statuses` (`session.rs:386`) and
  `read_tasks` (`session.rs:429`) rebuild tasks from `task_created` and
  `task_status_updated` events. Adding fields to the struct would store nothing.
- **Work is bounded by rounds, not tokens**: `agent_max_tool_rounds`,
  `agent_tool_retry_limit`.
- **No pricing information exists anywhere** in config or code.

## 3. Corrections found while planning

Five things the proposal assumed that the code does not do. Each changes what
the implementation has to be, and all five are already reflected in
[`tasks.md`](tasks.md).

**1. A provider 4xx is not a transport error.** §5.2 describes the
`stream_options` probe as triggered by "a request that fails with a 4xx". It
will not be. `curl` is invoked with `-sS` (`model.rs:381`), so an HTTP 400 exits
zero, its body is returned as a successful read, and the error surfaces only
when `extract_error_message(&raw)` finds an `error` object (`model.rs:691`). The
probe must therefore branch on the *parsed body*, not on a transport failure —
which also means `MockModelTransport::failing` is the wrong test tool for it, as
that raises `ClientError::Io`. §6's acceptance criterion names that helper, and
is wrong for the same reason.

**2. `MockModelTransport` returns one response for every call.** It carries a
single `response` field (`model.rs:556`), so no existing test double can express
"400 on the first call, 200 on the second" — the exact shape the probe needs. A
sequenced response is new work the probe task has to build.

**3. A cancelled mid-stream turn currently keeps nothing.** §5.5 asks for the
partial usage of a stopped turn. Today `pump_stream` discards its accumulated
body on cancellation (`model.rs:513`, `outcome.map(|()| raw)` on an `Err`), the
adapter propagates `ClientError::Cancelled`, and `chat.rs:969` substitutes
`ModelRun::cancelled_before_start` — a synthetic zero run. So the two cases §6
asks to distinguish, cancelled-before-the-call and cancelled-mid-stream, are
presently the same object. The tokens were streamed to the sink, so the chat
loop can accumulate them itself; that is cheaper and less risky than changing
what the adapter returns on cancel, which spec 08's stop path depends on.

**4. Recovery cannot estimate a lost call from the request, because the request
is gone.** §5.5 says recovery "appends a usage event estimated from the request",
and §6 requires it. After a crash the request object no longer exists, and the
`action_started` marker for a model call (`chat.rs:957`) records only the action
name and the model name. The estimate has to be made *before* the call and
written durably, or recovery has nothing to append but a zero — which is the
under-reporting §5.5 exists to prevent. The marker is the right place: it is
already written before the call, already read back by `dangling_actions`, and
already the evidence recovery classifies from.

**5. The "completion report" of §5.6 does not exist yet.** It is defined by
[spec 23](../23_verification_loop.md) §5.7, which is Not started. Requirement 2
therefore lands as the task-state surface — a per-turn usage line that survives
reload — and spec 23's report reads `read_task_usage` when it is built, rather
than growing a second accounting path.

## 4. What this does not touch

`agent_max_tool_rounds` stays the only bound on a turn. Nothing here stops,
warns, or throttles; a task that costs four dollars still costs four dollars and
now says so afterwards. The ceiling is Phase 2 WP2, which consumes these fields.
