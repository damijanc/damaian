# Feature Spec: Provider Limits and Backpressure

Status: Not started
Order: 48 of 53
Plan: `docs/PLAN/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 7 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#17](17_durable_task_state_and_crash_recovery/proposal.md) (the
task state a refusal is recorded in) — built;
[#19](19_token_and_cost_accounting/proposal.md) (the usage a refusal must not
inflate) — built. Everything else named below is a cross-reference, not a
prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 11 (error
handling).
Related implementation specs:
[`08_stop_and_progress.md`](08_stop_and_progress.md) (owns the progress channel
a wait is reported through, and the cancellation that must interrupt it),
[`17_durable_task_state_and_crash_recovery/proposal.md`](17_durable_task_state_and_crash_recovery/proposal.md)
(owns the task states and the action markers a refused call must close),
[`19_token_and_cost_accounting/proposal.md`](19_token_and_cost_accounting/proposal.md)
(owns the usage a refused call must not inflate),
[`45_crash_recovery_prompt.md`](45_crash_recovery_prompt.md) (the card that must
be able to name this outcome), and
[`49_prompt_cache_accounting_and_reuse.md`](49_prompt_cache_accounting_and_reuse.md)
(the sibling work package on the same response metadata).

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
4xx. The adapter's own comment says so:

> `curl -sS` exits zero on a 4xx, so the status never reaches us.

So the two arms written for rate limiting are unreachable for the case they
name. They fire only if curl's own stderr happens to contain the words. The
retry budget that exists (`MAX_ATTEMPTS: u32 = 3`, `RETRY_BACKOFF_MS: [500,
1500]`) is spent entirely on connection-level failures.

Three further consequences follow from the same root cause — no HTTP status is
ever captured:

- **Classification is substring matching on prose.** `contains("connection")`
  matches a permanent authentication error whose message mentions a connection
  profile; `contains("429")` matches any body carrying a request id with those
  three digits. The classifier is as likely to retry a permanent failure as a
  transient one, and it decides the user-facing message too, so a wrong guess is
  shown as well as acted on.
- **Quota exhaustion and rate limiting are indistinguishable.** "You have run
  out of credit" and "you are going too fast" want opposite responses. Both
  arrive as `ClientError::Io` carrying a sentence.
- **The outcome has no name.** [Spec 17](17_durable_task_state_and_crash_recovery/proposal.md)
  classifies what happened to an interrupted task, and
  [spec 45](45_crash_recovery_prompt.md) writes a sentence about it. Neither can
  say "the provider refused this call", because nothing records that it did.

This is Must-tier for the same reason [spec 19](19_token_and_cost_accounting/proposal.md)
is: the failure is silent and systematically misreports. A user on a metered API
hits this on their first long task, and what they see is a turn that died with a
sentence from a vendor.

## 2. Current State

- **No status, no headers.** `CurlModelTransport::curl_args` is `["-sS",
  "--no-buffer", "--config", "-"]` and `curl_config` sets `connect-timeout`,
  `speed-limit`, `speed-time` and `max-time`. Nothing captures `%{http_code}`
  and nothing reads response headers, so `Retry-After` is discarded before it
  reaches Rust.
- **`ModelTransport::send_stream` returns `Result<String>`** — the body, or an
  error. The shape has no room for a status.
- **Connection-level retry exists and works.** `send_with_retries` retries up to
  three attempts with fixed backoff, and only while `!emitted_any` — retrying
  after tokens have been shown to the user would blend two responses. That rule
  is correct and this spec keeps it.
- **The usage probe already re-sends a request.** The loop in `stream_response`
  sends once asking for usage, and once without it if the provider rejects the
  field, deliberately not counting the probe as a retry. A second reason to
  re-send has to fit alongside it rather than beside it.
- **`ClientError` has no provider-refusal variant.** Everything from a provider
  is `Io(String)`; `code()` maps it to `"io_error"`.
- **Spec 19 writes an estimate before the call.** `start_action_with_estimate`
  records `model_request_json(&request).len().div_ceil(4)` onto the action
  marker *before* the request is sent, so a crash can still account for a call
  that may have been billed. A refused call therefore arrives with a
  non-zero estimate already recorded.
- **Spec 08 owns progress.** `PhaseKind::Model` phases are emitted per round
  with `round` and `max_rounds`, and `CancelToken::check` is polled in the
  transport. A wait has somewhere to be reported and something to interrupt it.

## 3. Requirements

1. Every model call captures its HTTP status and the response headers that
   govern retry, without contaminating the token stream the user is watching.
2. A provider response is classified into a typed refusal derived from **status
   first**, body second. Prose substring matching is never the primary signal
   and never the only one.
3. Rate limiting and transient overload are retried, honouring `Retry-After`
   where the provider sent one, under a bounded attempt count **and** a bounded
   total wall-clock ceiling. Authentication failures, malformed requests and
   quota exhaustion are not retried.
4. A retry wait is visible in the progress channel, states how long it is
   waiting and why, and is interrupted immediately by a stop.
5. A turn that exhausts the retry budget produces an outcome distinguishable
   from a generic failure, nameable by [spec 45](45_crash_recovery_prompt.md),
   and leaving plan state intact so the work can be resumed later.
6. A refused call reports the usage it actually incurred. An attempt the
   provider rejected before generating anything is recorded as measured zero,
   not as the pre-call estimate.
7. Switching to a different provider or model in response to a refusal requires
   the user's consent each time it happens, and is recorded.
8. Every refusal is audited through `AuditLog::record`, carrying the
   classification and the status — never the request body.

## 4. Non-goals

- **Client-side rate limiting.** Predicting a provider's limit locally means
  modelling a bucket the provider does not publish and Damaian cannot observe.
  This spec reacts to the limit; it does not try to stay under it.
- **A queue of pending turns.** One session, one turn. Backpressure delays a
  call; it does not build a scheduler. [Spec 39](39_coordination_and_conflict_handling.md)
  explicitly rules a general work queue out and this spec does not reintroduce
  one.
- **Automatic model downgrade.** Requirement 7 is a consented switch. Silently
  answering with a weaker model because the strong one was busy changes the
  result of the user's task without telling them, and model routing as a
  deliberate cost strategy is Phase 3 WP7.
- **Replacing curl with an HTTP client crate.** That is a larger change with its
  own trade-offs; §5.1 gets the status out of the transport already in use.
- **Provider-specific SDKs.** The adapter stays OpenAI-compatible.
- **Retrying tool calls, patches, or commands.** This spec covers the model
  call only.
- **A built-in table of per-provider rate limits.** Same reasoning as spec 19 §4
  on prices: it would go stale and then confidently mislead.

## 5. Design

### 5.1 Getting the status out of curl

Two options, and this spec takes the second.

1. **A sentinel in stdout.** `write-out = "\nDAMAIAN_HTTP_STATUS:%{http_code}\n"`
   appends the code after the body. Cheap, and wrong for a specific reason: that
   stream is model output shown to the user token by token, and the first time a
   model emits the sentinel — writing about this spec, for instance — the parser
   is fooled. A correctness hazard whose trigger is "the assistant discusses its
   own source" is not acceptable.
2. **Headers to a file.** curl's config gains
   `dump-header = "<data_dir>/tmp/<run_id>.headers"`, written by curl and read by
   Rust after `wait()`, then deleted. The token stream is untouched, the status
   line and every response header are available, and `Retry-After` comes along
   for free.

The header file lives under the data directory, is named by `run_id` so
concurrent calls cannot collide, and is removed on both the success and error
paths. `path_policy.rs` governs it like any other file Damaian writes.

The transport trait gains one method with a default implementation, so existing
implementations — `MockModelTransport` and every test double — compile
unchanged:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponseMeta {
    /// `None` when the transport cannot report one (a mock, or curl failing
    /// before a response). Never defaulted to 200.
    pub status: Option<u16>,
    /// Seconds to wait, parsed from `Retry-After` in either of its two forms.
    pub retry_after_secs: Option<u64>,
}

pub trait ModelTransport {
    // ... existing methods unchanged ...

    /// Metadata for the most recent `send_stream`. Default: nothing known.
    fn last_response_meta(&self) -> ResponseMeta {
        ResponseMeta::default()
    }
}
```

`status: Option<u16>` rather than `u16` is the load-bearing choice. A transport
that does not know the status must say so, because a default of 200 would make
every mock silently assert success and every classification below would be
derived from a fact nobody established.

### 5.2 The classification

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderRefusal {
    /// 429. Retry after `retry_after_secs` when the provider named one.
    RateLimited { retry_after_secs: Option<u64> },
    /// 500, 502, 503, 504 — the provider is unwell, not the request.
    Overloaded { retry_after_secs: Option<u64> },
    /// 402, or a 4xx whose body names an exhausted balance or quota.
    /// Permanent until the user does something outside Damaian.
    QuotaExhausted,
    /// 401, 403.
    AuthFailed,
    /// 400, 404, 422 — the request will not succeed if repeated.
    BadRequest,
    /// A refusal that was recognised as one but not classified further.
    /// Treated as permanent: guessing "transient" retries an error that will
    /// never clear.
    Unknown,
}
```

`ClientError` gains one variant, `Provider(ProviderRefusal, String)`, carrying
the classification and the provider's own message. `code()` returns
`"provider_rate_limited"`, `"provider_quota_exhausted"` and so on, so the audit
log and the UI can branch on a value instead of a sentence.

Classification order, and it is not negotiable:

1. **Status, when the transport reported one.** The table above, exhaustively.
2. **Body, only when the status is absent.** A mock, or a provider returning 200
   with an error object — which some do. The body check looks at a provider
   error object's `type`/`code` field, not at free prose.
3. **Nothing else.** No substring search over the message.

`is_retryable_message` stays, narrowed to what it is actually reachable for:
curl's own transport-level stderr. Its `"rate limit"` and `"429"` arms are
removed, with a comment saying where rate limits are now classified, because
leaving dead arms in a classifier is how the next person concludes the case is
handled.

Retryable is then a method on the refusal rather than a guess about a string:

```rust
impl ProviderRefusal {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Overloaded { .. })
    }
}
```

### 5.3 Waiting

A transient refusal is retried under two independent bounds:

| Bound | Default | Why |
|---|---|---|
| Attempts | 4 beyond the first | Enough to cross a per-minute window |
| Total wait | 90s per call | A ceiling a user will tolerate without wondering whether it hung |

`Retry-After` wins when the provider sent one and it fits inside the remaining
wall-clock ceiling. A `Retry-After` larger than the ceiling is not slept
through — the call fails with the refusal and the provider's own figure in the
message, because sleeping for eleven minutes is indistinguishable from a hang.
Where no `Retry-After` is given, backoff is exponential from 1s with jitter, so
two Damaian windows that hit the same limit do not resynchronise on the retry.

The wait is reported through [spec 08](08_stop_and_progress.md)'s existing
phase channel as a distinct phase carrying the remaining seconds, and polls
`CancelToken::check` on a short interval rather than sleeping the whole
duration. A stop during a rate-limit wait must take effect during the wait; a
user watching "retrying in 47s" and unable to cancel it is the worst version of
this feature.

Retries under this section are counted in `ModelRun::retry_count` alongside the
connection-level ones, because from the accounting side they are the same thing:
attempts beyond the first. The classification is what distinguishes them, and
that is audited.

### 5.4 The outcome is a reason, not a fourteenth state

[Spec 17](17_durable_task_state_and_crash_recovery/proposal.md)'s kill matrix
covers every task state by three crash shapes, and `TaskStatus::all()` exists so
that a variant added later fails the state tests until it has been given a
terminality and a side-effect answer. Adding a state is therefore three new
matrix cells and a re-reasoning of the recovery classifier — for a distinction
the state machine does not need. A task refused by the provider is *failed*, and
it is failed for a reason worth naming.

So the existing failure state gains a reason, recorded on the task event:

```json
{"seq":412,"eventType":"task_status_changed","taskId":"task_…",
 "status":"failed","failureKind":"provider_rate_limited",
 "detail":"Rate limited after 4 attempts over 90s"}
```

`failureKind` is optional and absent on every existing event, so the migration
is a read-side default and no rewrite of existing logs is needed.
[Spec 45](45_crash_recovery_prompt.md)'s card reads it and names the outcome —
"The provider rate-limited this task after 4 attempts" — and offers Resume,
which is the correct and only useful action here. Plan state is untouched by a
refusal, so [spec 21](21_task_plan_progress_and_budget/proposal.md)'s resumed
turn carries the plan across exactly as it does after a stop.

### 5.5 A refused call must not inflate cost

[Spec 19](19_token_and_cost_accounting/proposal.md) writes an estimated token
count onto the action marker before the request is sent, so that a call lost to
a crash can still be accounted for. A refusal takes the same path and would
leave that estimate standing.

It must not. A 429 is rejected at the edge and generates nothing, so its true
usage is `TokenUsage::measured_zero()` — spec 19's own words for it are "the
only zero that is a fact rather than a guess: no request was sent, so nothing
was billed", and a refused request is the second case that qualifies.

So the marker is finished with an explicit outcome of `"refused"` and a measured
zero, overriding the pre-call estimate. Two cases are deliberately distinguished:

| Case | Usage recorded |
|---|---|
| Refused before any token (every 4xx, and a 5xx with an empty body) | Measured zero |
| Refused mid-stream after tokens were emitted | Estimated from what was sent and what arrived, exactly as a cancelled run is |

The second case is rare and real: a provider that starts streaming and then
emits an error event has billed something. Reporting it as zero would be the
same under-reporting spec 19 exists to prevent.

### 5.6 Falling back requires consent, every time

Requirement 7. On a terminal refusal, and only where the user has configured a
second provider, Damaian offers the switch as an approval-shaped decision naming
both models. It is refused by default, it is not remembered, and there is no
"always". `Allow Always` exists for commands whose risk is known and repeatable
([spec 10](10_persistent_command_approval.md)); "answer with a different model
whenever the good one is busy" is not that — it silently changes the quality of
every future answer.

The switch is recorded on the task and shown in the transcript, so a session
whose second half was answered by a weaker model says so.

### 5.7 Quota exhaustion

`QuotaExhausted` is permanent within the session: no retry, no fallback offer
unless a different provider is configured with its own credentials, and the
provider's own message is shown verbatim because it is the only place the user
learns what to top up. The task fails with `failureKind:
"provider_quota_exhausted"`, and its plan survives, so resuming after the user
has fixed their account is one click rather than a re-run.

### 5.8 Documentation

`docs/USER_GUIDE.md`: what a rate-limit wait looks like, that it can be
stopped, and that a fallback is always asked for. `docs/TROUBLESHOOTING.md`: the
refusal classifications, what each means, and which are worth retrying by hand.

## 6. Acceptance Criteria

- A 429 response is classified as `RateLimited`, retried, and succeeds on a
  later attempt — asserted with a transport double returning 429 then 200.
- `Retry-After` in delta-seconds form and in HTTP-date form both parse to the
  same wait.
- A `Retry-After` exceeding the wall-clock ceiling is not slept through; the
  call fails and the message carries the provider's figure.
- A 401 and a 400 are not retried — asserted by attempt count, not by timing.
- A provider error body containing the word "connection" or the digits "429" in
  a request id is classified from its status, not its prose, and is not retried
  when the status is permanent.
- A rate-limit wait is interrupted by a stop within one poll interval, and the
  turn reports cancelled rather than failed.
- A refused call records measured-zero usage, and the task's total is unchanged
  by it — asserted against `read_task_usage`.
- A refusal that arrived mid-stream records estimated, non-zero usage.
- A task failed by a refusal carries `failureKind`, and
  [spec 45](45_crash_recovery_prompt.md)'s card names the provider outcome
  rather than a generic failure.
- A task failed by a refusal retains its plan, and a resumed turn carries it
  across.
- No fallback switch happens without an explicit per-occurrence approval —
  asserted by running a refusal with a second provider configured and no
  approval given.
- The header file is deleted on both the success and the error path, and two
  concurrent calls do not collide — asserted by `run_id`-named paths.
- No audit record contains the request body or the API key.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation. Record:

- Whether any configured provider returns an error object with HTTP 200, which
  would make §5.2's second classification step load-bearing rather than a
  fallback for mocks.
- The observed `Retry-After` behaviour of each provider tested — whether one is
  sent at all, and whether it is honest.
- Whether the wall-clock ceiling of 90s proved too short or too long in
  practice, measured against real limits rather than chosen.
