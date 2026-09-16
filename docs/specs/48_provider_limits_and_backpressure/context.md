# Provider Limits and Backpressure — Context

Background for [`proposal.md`](proposal.md): why this work exists, what the code
looks like today, and the corrections found while planning it. The proposal is
the decision; this is the ground it stands on.

## 1. Motivation

A provider rate limit ends the turn, and the code that was written to prevent
that cannot see it.

`OpenAICompatibleAdapter::stream_response` parses the response body, and when
`extract_error_message` finds anything it is not specifically probing for, it
returns immediately:

```rust
return Err(ClientError::Io(format!("Model provider error: {message}")));
```

(`crates/workspace-engine/src/model.rs`, inside the usage-probe loop.) There is
no retry on that path, no backoff, and no reading of `Retry-After`. An HTTP 429
is handled exactly like a malformed request.

Meanwhile `is_retryable_message` (`crates/workspace-engine/src/error.rs`) names
rate limits first:

```rust
lower.contains("rate limit")
    || lower.contains("429")
    || lower.contains("timeout")
    ...
```

That classifier is consulted in one place — `send_with_retries`, on the error
returned by `ModelTransport::send_stream`. `CurlModelTransport` only returns an
error there when **curl itself** exits non-zero, and `curl -sS` exits zero on a
4xx. So the two arms written for rate limiting are unreachable for the case they
name; they fire only if curl's own stderr happens to contain the words. The
retry budget that exists (`MAX_ATTEMPTS = 3`, `RETRY_BACKOFF_MS = [500, 1500]`)
is spent entirely on connection-level failures.

Three further consequences follow from the same root cause — no HTTP status is
ever captured:

- **Classification is substring matching on prose.** `contains("connection")`
  matches a permanent authentication error whose message mentions a connection
  profile; `contains("429")` matches any body carrying a request id with those
  three digits. The classifier is as likely to retry a permanent failure as a
  transient one, and it decides the user-facing message too.
- **Quota exhaustion and rate limiting are indistinguishable.** "You have run
  out of credit" and "you are going too fast" want opposite responses. Both
  arrive as `ClientError::Io` carrying a sentence.
- **The outcome has no name.** Spec 17 classifies what happened to an
  interrupted task, and spec 45 writes a sentence about it. Neither can say "the
  provider refused this call", because nothing records that it did.

This is Must-tier for the same reason spec 19 is: the failure is silent and
systematically misreports. A user on a metered API hits this on their first long
task, and what they see is a turn that died with a sentence from a vendor.

## 2. Current state

Verified against the tree on 2026-09-16, on the base branch
(`origin/main`, before any process-registry work landed). Line numbers are a
reading aid and drift; the symbol names are the durable part.

- **No status, no headers.** `CurlModelTransport::curl_args` is
  `["-sS", "--no-buffer", "--config", "-"]` (`model.rs:480`) and `curl_config`
  (`model.rs:488`) sets `connect-timeout`, `speed-limit`, `speed-time` and
  `max-time`. Nothing captures `%{http_code}` and nothing reads response
  headers, so `Retry-After` is discarded before it reaches Rust.
- **`ModelTransport::send_stream` returns `Result<String>`** (`model.rs:453`) —
  the body, or an error. `send` returns the same shape. The shape has no room
  for a status.
- **Connection-level retry exists and works.** `send_with_retries`
  (`model.rs:760`) retries up to three attempts with fixed backoff, and only
  while `!emitted_any` — retrying after tokens have been shown would blend two
  responses. That rule is correct and this spec keeps it.
- **The usage probe already re-sends a request.** The loop in `stream_response`
  (`model.rs:839`) sends once asking for usage, and once without it if the
  provider rejects the field, deliberately not counting the probe as a retry. A
  second reason to re-send has to fit alongside it rather than beside it.
- **`ClientError` has no provider-refusal variant.** Everything from a provider
  is `Io(String)`; `code()` maps it to `"io_error"` (`error.rs:20`).
- **Spec 19 writes an estimate before the call.** `start_action_with_estimate`
  (`session.rs:776`) records `model_request_json(&request).len().div_ceil(4)`
  onto the action marker *before* the request is sent, so a crash can still
  account for a call that may have been billed. A refused call therefore arrives
  with a non-zero estimate already recorded.
- **Spec 08 owns progress.** `PhaseKind::Model` phases are emitted per round
  (`chat.rs:1402`) with `round` and `max_rounds`, and `CancelToken::check` is
  polled in the transport. A wait has somewhere to be reported and something to
  interrupt it — though the current retry sleep does not use the latter.

## 3. Corrections found while planning

Nine things the proposal assumed that the code does not do. Each changes what
the implementation has to be, and all nine are reflected in
[`tasks.md`](tasks.md).

**1. The outcome event is `task_status_updated`, not `task_status_changed`, and
it wraps the reason.** §5.4's JSON example shows a flat event with an
`eventType` of `task_status_changed`. The code writes `task_status_updated`
(`session.rs:335`), and the failure reason travels as a **wrapped** payload —
`{"task":{…},"error":"…"}` (`session.rs:327-334`) — or a flat `task_json` with
no error at all. `failureKind` therefore needs a home in `update_task_status` /
the wrapped shape, and `read_task_statuses` (`session.rs:429-491`, which already
parses both shapes) must read it back. The spec's example is illustrative of the
*fields*, not of the current wire shape.

**2. `CurlModelTransport` knows neither the data directory nor a run id.** It is
built as `CurlModelTransport::new(base_url, api_key)` (`model.rs:472`), with
eight call sites plus three test sites. §5.1 places the header file under
`<data_dir>/tmp/<id>.headers`, but `run_id` is created one level up in
`stream_response` (`model.rs:846`) and the transport has no `data_dir`. The data
directory must be threaded into the transport constructor, and the per-call
uniqueness the spec names "run id" must come from a fresh id the transport
generates itself (`create_id`), since the transport cannot see the adapter's
`run_id`.

**3. `MockModelTransport` cannot report a status.** It returns `Ok(String)` only
(`model.rs:701-714`); `fail_before_success` simulates a connection failure, not
a provider response. Acceptance criterion 1 needs "a transport double returning
429 then 200", which is a per-response *status* plus `Retry-After` — a new field
(or parallel sequence) on the double. Without it, `last_response_meta()`'s
default of `None` makes every mock classify by body alone and the status-first
path is never exercised by a test.

**4. The connection-level retry sleeps without polling cancellation.** The
retry loop calls `std::thread::sleep(RETRY_BACKOFF_MS[...])` (`model.rs:818`),
not `CancelToken::check`. The existing 500/1500 ms waits are short enough that
this has never mattered, but requirement 4's 90-second ceiling makes the wait
mechanism load-bearing: the refusal wait must poll `cancel` at
`CANCEL_POLL_INTERVAL` (250 ms, `model.rs:14`) rather than sleep the whole
duration.

**5. `extract_error_message` reads only the `message` field.** It returns
`extract_string_field(raw, "message")` (`model.rs:1311-1316`). §5.2's
body-based classification step looks at a provider error object's `type`/`code`
field — which nothing extracts today. The body classifier needs a richer parse
of the error object than the single-field extractor the usage probe leans on.

**6. The refusal arrives as `Ok(raw)`, not `Err`.** This is the proposal's
central claim, confirmed in the code: on a 4xx, `curl -sS` exits zero, so
`CurlModelTransport::send_stream` returns `Ok(raw)` carrying the error body, and
`stream_response` turns it into `Err(ClientError::Io(format!("Model provider
error: {message}")))` at `model.rs:872` — *after* `send_with_retries` has
already returned `Ok`. The retry budget never sees the refusal. Classification
therefore has to happen in `stream_response` (or the loop around it), reading
`transport.last_response_meta()` after the send, not inside `send_with_retries`'
connection-failure arm.

**7. The retry-accounting loop assumes every retry was a billed connection
failure.** `chat.rs:1491-1500` records one `record_task_usage` per
`model_run.retry_count` with `TokenUsage::estimated(input, 0)` and reason
`"retried_attempt"`. If rate-limit retries are folded into `retry_count`
(§5.3), a refused attempt would be booked as an estimated input that the
provider never processed — the exact over-counting requirement 6 forbids. The
accounting must distinguish "connection-level retry that sent the body"
(estimated) from "refused before generating anything" (measured zero), which the
current loop cannot express.

**8. The non-cancel error path neither finishes the marker nor records usage.**
On `Err(error)` the turn calls `update_task_status(Failed, …)` and returns
(`chat.rs:1467-1474`), leaving the `action_started` marker from
`start_action_with_estimate` dangling and writing no usage event at all.
Requirement 6 needs the refusal arm to `finish_action(marker, "refused")` and
`record_task_usage(…, measured_zero, …, Some("refused"))` — new behaviour on a
path that today records nothing.

**9. `update_task_status` carries a single free-text `error`, no kind.** Its
signature is `(task, status, error: Option<&str>)` (`session.rs:311`). §5.4's
`failureKind` is a typed code, not prose, so the status write (and its wrapped
payload) must gain a `failure_kind` parameter, or the refusal path must carry
the kind through the existing `error` slot in a way the read side can recover —
the former is what `tasks.md` implements, because a UI that branches on a code
must not parse it back out of a sentence.

## 4. What this does not touch

`send_with_retries`' connection-level policy (three attempts, no retry after
tokens have streamed) is preserved; the refusal path is a sibling of it, not a
rewrite. The usage probe's two-pass shape is preserved; the refusal
classification is another decision made after the send, not another probe. And
nothing here changes what is sent to the provider — only what Damaian does with
the answer.
