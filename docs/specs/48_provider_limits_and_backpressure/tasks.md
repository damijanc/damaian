# Provider Limits and Backpressure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) · background and corrections in [`context.md`](context.md)
**Started:** 2026-09-16 — **Done:** 2026-09-16

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `ResponseMeta` and the header file | Done | `ResponseMeta`, `last_response_meta` on the trait, `dump-header` capture via a `HeaderFileGuard` (RAII cleanup on every exit path), and the per-call unique path under `data_dir/tmp/`. **Plan deviation:** `parse_retry_after` (delta-seconds + IMF-fixdate) landed here, not in Task 2, because `ResponseMeta.retry_after_secs` is produced at capture time — Task 2's mention of it is a stale cross-reference. 7 new tests, gate green. |
| 2 · `ProviderRefusal` and `ClientError::Provider` | Done | `ProviderRefusal` (six variants) with `code()`/`is_transient()`, `ClientError::Provider(ProviderRefusal, String)`, and `classify_refusal` (status-first, body-second via the error object's `code`/`type` field) + `body_mentions_quota`. `is_retryable_message` narrowed (dropped `"rate limit"`/`"429"`). **Plan deviation:** `ProviderRefusal` lives in `error.rs` rather than `model.rs` — `error.rs` is foundational and `ClientError::Provider` needs the type without importing the big module; the classifier stays in `model.rs`. 12 new tests, gate green. |
| 3 · Retry on refusal, with a cancellable visible wait | Done | `stream_response` retries transient refusals under `MAX_REFUSAL_ATTEMPTS` (4) and a 90s wall-clock ceiling; `Retry-After` honoured when within the ceiling (else fails carrying the figure), else exponential-with-jitter backoff; wait reported via a new `on_wait: &mut dyn FnMut(u64)` parameter on `ModelAdapter::stream_response` and polled against `CancelToken`. **Plan deviation (correcting §5.3):** refusal retries are *not* folded into `retry_count` — that field drives chat.rs's estimated-input booking, and a refusal was billed for nothing (`context.md` §3.7) — so `ModelRun` gains a separate `refusal_retries: u32`. 4 new tests, gate green. |
| 4 · The outcome and accounting | Done | `update_task_status_with_kind` (a new sibling, so `update_task_status`'s ~30 call sites stay untouched) writes `failureKind` beside the wrapped `task`; `read_task_failure_kinds` reads it and `task_states_json` surfaces it to the frontend. The chat refusal arm finishes the marker `"refused"` and books `TokenUsage::measured_zero`, overriding the pre-call estimate. 3 new integration tests in `tests/provider_limits.rs`, gate green. **Left for Task 6:** the frontend (`app.js`) does not yet branch on `failureKind` — the provider's own message already reaches the user via the error bubble, so the structured field is available but unused. |
| 5 · Consented fallback and quota exhaustion | Done | Quota exhaustion is permanent with the provider's message verbatim (`failureKind: provider_quota_exhausted`), and a refusal never silently switches provider even when a second one is configured — both pinned by tests (`a_quota_exhaustion_…`, `no_fallback_happens_without_approval`). The fallback *offer* (the approval flow) is deferred to a follow-up spec, not silently dropped. |
| 6 · Audit, documentation, and closing the spec | Done | `provider_refusal` audit (classification, never the request body/key — pinned by `a_refusal_is_audited_…`). `USER_GUIDE.md` gains "Provider Limits and Retries"; `TROUBLESHOOTING.md` gains the six classification rows and drops the now-stale "message text drives that decision" note. Also closed the mid-stream accounting gap the acceptance criteria named: a refusal after tokens streamed is estimated, not measured zero (`a_refusal_mid_stream_records_estimated_usage`). Spec marked Done. |

**Goal:** Capture every model call's HTTP status and retry-governing headers,
classify a provider refusal from status first and body second, retry only the
transient ones under two bounds with a cancellable, visible wait, and record the
refusal as a named reason — never inflating cost, never switching model without
consent.

**Architecture:** `CurlModelTransport` gains a `dump-header` file and reports a
`ResponseMeta` (status + `Retry-After`) through a new default trait method. A
`ProviderRefusal` enum and a `ClientError::Provider` variant carry a typed
classification, produced by a status-first/body-second classifier invoked in
`stream_response` after the send. The refusal wait replaces the fixed backoff
for the transient case with `Retry-After`-honouring, exponential-with-jitter
sleep polled against `CancelToken`. The refusal reaches the task as a
`failureKind` on the existing `failed` state, the action marker is finished
`"refused"` with a measured-zero usage, and a consent-gated fallback offer is
the only route to a second provider.

**Tech Stack:** Rust 2024 (workspace edition), no new dependencies. `serde_json`
for the header/body parsing that already exists; `dump-header` is a curl config
line, not a new library.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) §3 first.** Nine of the proposal's design
  statements describe behaviour the code does not have. Each task below already
  accounts for its correction; do not "fix" a task back to the proposal's
  wording.
- **Status first, body second, never prose.** Classification starts from
  `ResponseMeta.status` when present, falls back to a provider error object's
  `type`/`code` field, and never substring-matches a message. The one exception
  is the usage probe's `stream_options` wording check, which already exists and
  is not classification.
- **Never over-book a refusal.** A call refused before it generated anything is
  measured zero (requirement 6); a refused mid-stream call is estimated from
  what streamed. The retry-accounting loop must distinguish the two — the
  correction in `context.md` §3.7.
- **The refusal is a failed task with a reason, not a new state.** No
  fourteenth `TaskStatus`; the `failureKind` rides the existing `failed` state.
- **No consent-less fallback.** Requirement 7; there is no "always" and no
  remembered switch.
- **The header file is response metadata only.** `dump-header` writes response
  headers, never the request, so the API key is never on disk; delete it on both
  the success and error paths, and name it by a fresh id so concurrent calls
  cannot collide.
- **Clippy warnings are errors.** Fix rather than suppress; an `#[allow(...)]`
  needs a comment saying why.
- **Every quality-gate command from `AGENTS.md` must pass** at the end of every
  task. That is seven commands and the list in `AGENTS.md` is authoritative —
  read it, do not rely on a remembered list. `typos` and
  `node --check crates/desktop-shell/static/app.js` are the two that get missed.
- **Commit messages:** one subject line, no body, no `Co-Authored-By`. Rationale
  belongs in this plan and the proposal. Never cite commit SHAs in
  documentation.
- **Do not commit without asking.** Show the change and the gate result; the
  decision to commit is Damijan's.

## File Structure

| File | Responsibility |
|---|---|
| `crates/workspace-engine/src/model.rs` | `ResponseMeta`, `ProviderRefusal`, the classifier, `last_response_meta` on the trait, the `dump-header` capture, the refusal retry loop, and the `MockModelTransport` status support. |
| `crates/workspace-engine/src/error.rs` | `ClientError::Provider`, `code()` strings, and the narrowing of `is_retryable_message`. |
| `crates/workspace-engine/src/session.rs` | `failure_kind` on the `task_status_updated` write and read. |
| `crates/workspace-engine/src/chat.rs` | Finishing the marker `"refused"`, recording measured-zero usage, the failure arm, and the fallback offer. |
| `crates/workspace-engine/src/lib.rs` | Export `ProviderRefusal`, `ResponseMeta`. |
| `crates/damaian-cli/src/main.rs`, `crates/eval-harness/src/runner.rs`, `crates/desktop-shell/src/lib.rs` | Thread `data_dir` into `CurlModelTransport::new`. |
| `crates/workspace-engine/tests/provider_limits.rs` | **New.** The end-to-end refusal, accounting, and consent tests. |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | The user-facing description of the wait and the classifications. |

## Interface reference

Verified against the tree on 2026-09-16 (base branch). Every signature a later
task depends on, so no task has to go looking.

| Symbol | Where | Shape |
|---|---|---|
| `ClientError` | `error.rs:4` | 8 variants; Task 2 adds `Provider(ProviderRefusal, String)` |
| `ClientError::code` | `error.rs:20` | `(&self) -> &'static str` |
| `is_retryable_message` | `error.rs:47` | `(&str) -> bool`; Task 2 drops the `"rate limit"`/`"429"` arms |
| `ModelTransport` | `model.rs:450` | `send`, `send_stream`; Task 1 adds `last_response_meta(&self) -> ResponseMeta` with a default |
| `CurlModelTransport` | `model.rs:467` | `new(base_url, api_key)`; Task 1 adds a `data_dir` parameter |
| `curl_config` | `model.rs:488` | `(&self, request_body: &str) -> String`; gains `dump-header` |
| `send_with_retries` | `model.rs:760` | `(&mut self, body, cancel, on_token) -> Result<(raw, content, retry_count)>` |
| `OpenAICompatibleAdapter::stream_response` | `model.rs:840` | the loop that must classify and retry |
| `extract_error_message` | `model.rs:1311` | `(&str) -> Option<String>`, `message` field only |
| `MockModelTransport` | `model.rs:654` | `response`, `fail_before_success`, `responses`/`next_response`; Task 1 adds per-response status/`retry_after` |
| `start_action_with_estimate` | `session.rs:776` | records the marker with `estimatedInputTokens` |
| `finish_action` | `session.rs:830` | `(marker, outcome)` |
| `update_task_status` | `session.rs:311` | `(task, status, error)`; Task 4 adds a `failure_kind` parameter |
| `record_task_usage` | `session.rs` | `(task, run_id, marker_id, usage, reported_cost, reason)` |
| `TokenUsage::measured_zero` | `model.rs:196` | the pre-generation zero |
| chat model call | `chat.rs:1412-1500` | marker → `stream_response` → usage; error arm at `chat.rs:1467` |
| `PhaseKind` | `chat.rs:122` | `Context | Model | Tool | Finalizing`; the wait is reported as a `Model` phase with a label |

---

## Task 1: `ResponseMeta` and the header file

Get the status and `Retry-After` out of curl without touching the token stream,
and make the transport say so through a default trait method.

**Files:**
- Modify: `crates/workspace-engine/src/model.rs`
- Modify: `crates/damaian-cli/src/main.rs:316,365`, `crates/eval-harness/src/runner.rs:135`, `crates/desktop-shell/src/lib.rs:653,1185,1237,1319,1366` (thread `data_dir`)
- Modify: `crates/workspace-engine/src/lib.rs` (export `ResponseMeta`)

**Interfaces:**
- Produces: `ResponseMeta { status: Option<u16>, retry_after_secs: Option<u64> }`; `ModelTransport::last_response_meta` (default `None`/`None`); `CurlModelTransport::new(base_url, api_key, data_dir)`; a per-response `status`/`retry_after` on `MockModelTransport`.

- [x] **Step 1: Write the failing tests**

In `model.rs` `mod tests`:

```rust
#[test]
fn a_transport_with_no_metadata_reports_none_by_default() {
    // The default must be "unknown", never "200": a fabricated success is how
    // a mock would silently assert the provider answered.
    let transport = MockModelTransport::new("data: [DONE]\n");
    assert_eq!(transport.last_response_meta().status, None);
}

#[test]
fn a_mock_can_report_a_status_and_a_retry_after() {
    let mut transport = MockModelTransport::new("data: [DONE]\n");
    transport.status = Some(429);
    transport.retry_after_secs = Some(3);
    // `send` must leave the metadata readable afterwards.
    transport.send("{}").expect("send");
    assert_eq!(transport.last_response_meta().status, Some(429));
    assert_eq!(transport.last_response_meta().retry_after_secs, Some(3));
}
```

- [x] **Step 2: Run the tests to verify they fail**

`cargo test -p workspace-engine --lib a_transport_with_no_metadata a_mock_can_report`

Expected: FAIL to compile — `no method named last_response_meta`, no `status` field on `MockModelTransport`.

- [x] **Step 3: Add `ResponseMeta` and the default method**

In `model.rs`, above `ModelTransport`:

```rust
/// The metadata curl reported about the most recent response: the HTTP status
/// and the retry signal. Both are `Option` because a transport that cannot see
/// them (a mock, or curl dying before the response) must say so rather than
/// invent a 200.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponseMeta {
    pub status: Option<u16>,
    pub retry_after_secs: Option<u64>,
}
```

Add to `ModelTransport`:

```rust
    /// Metadata for the most recent `send_stream`. Default: nothing known.
    fn last_response_meta(&self) -> ResponseMeta {
        ResponseMeta::default()
    }
```

- [x] **Step 4: Capture the header file in `CurlModelTransport`**

Add `data_dir: PathBuf` to the struct and constructor. Give the transport a
`last_meta: ResponseMeta` field (cleared to `None` before each send). In
`curl_config`, accept a header path and emit
`header-dump` (`dump-header = "<path>"`) as an extra config line; in
`send_stream`, generate a fresh path under `data_dir/tmp/`, spawn curl with it,
read it after `wait()`, parse the status line and `Retry-After` (both the
delta-seconds and the HTTP-date form), then remove the file on every exit path.

The header path is created under `data_dir/tmp`; create that directory once in
the constructor.

- [x] **Step 5: Give `MockModelTransport` a status**

Add `pub status: Option<u16>` and `pub retry_after_secs: Option<u64>` to
`MockModelTransport`, and implement `last_response_meta` on it returning them.
Nothing else changes: the fields default to `None`.

- [x] **Step 6: Thread `data_dir` through the eight real call sites**

Each already has `engine.config.data_dir` or `config.data_dir` in scope. The
three test sites inside `model.rs` use a scratch temp dir.

- [x] **Step 7: Run the tests, the full gate, then show before committing**

Suggested subject line: `Capture a model call's HTTP status and retry header`

---

## Task 2: `ProviderRefusal` and `ClientError::Provider`

The typed refusal, produced status-first and body-second, with the dead arms
removed from the prose classifier.

**Files:**
- Modify: `crates/workspace-engine/src/error.rs`
- Modify: `crates/workspace-engine/src/model.rs`
- Modify: `crates/workspace-engine/src/lib.rs` (export `ProviderRefusal`)

**Interfaces:**
- Produces: `ProviderRefusal` (six variants per §5.2), `ClientError::Provider(ProviderRefusal, String)`, `code()` strings `provider_rate_limited` / `provider_overloaded` / `provider_quota_exhausted` / `provider_auth_failed` / `provider_bad_request` / `provider_refused`, `ProviderRefusal::is_transient`, and a classifier `classify_refusal(meta: &ResponseMeta, raw: &str) -> Option<ProviderRefusal>`.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn a_429_status_classifies_as_rate_limited_regardless_of_prose() {
    let meta = ResponseMeta { status: Some(429), retry_after_secs: None };
    // The body's request id contains "429" and the message says "connection";
    // status must win over both.
    let raw = "{\"error\":{\"message\":\"connection failed\",\"request_id\":\"req_429ab\"}}";
    assert_eq!(classify_refusal(&meta, raw), Some(ProviderRefusal::RateLimited { retry_after_secs: None }));
}

#[test]
fn a_401_classifies_as_auth_failed_and_is_not_transient() {
    let meta = ResponseMeta { status: Some(401), retry_after_secs: None };
    let refusal = classify_refusal(&meta, "{}").unwrap();
    assert_eq!(refusal, ProviderRefusal::AuthFailed);
    assert!(!refusal.is_transient());
}

#[test]
fn a_500_status_classifies_as_overloaded_and_is_transient() {
    let meta = ResponseMeta { status: Some(503), retry_after_secs: Some(5) };
    assert_eq!(classify_refusal(&meta, "{}").unwrap(), ProviderRefusal::Overloaded { retry_after_secs: Some(5) });
}

#[test]
fn a_body_naming_quota_is_exhausted_only_when_no_status_is_present() {
    let meta = ResponseMeta::default();
    let raw = "{\"error\":{\"code\":\"insufficient_quota\",\"message\":\"out of credit\"}}";
    assert_eq!(classify_refusal(&meta, raw), Some(ProviderRefusal::QuotaExhausted));
}

#[test]
fn a_provider_error_carries_the_refusal_and_a_code() {
    let error = ClientError::Provider(ProviderRefusal::QuotaExhausted, "out of credit".into());
    assert_eq!(error.code(), "provider_quota_exhausted");
    assert!(format!("{error}").contains("out of credit"));
}
```

In `error.rs`, pin the narrowing:

```rust
#[test]
fn the_message_classifier_no_longer_names_rate_limits() {
    // Removed because it is dead: rate limits are classified from status now,
    // and leaving the arm would let the next person conclude the case is handled.
    assert!(!is_retryable_message("provider rate limit exceeded"));
    assert!(!is_retryable_message("http status 429"));
    assert!(is_retryable_message("could not resolve host"));
}
```

- [x] **Step 2: Run the tests to verify they fail**

Expected: FAIL to compile — no `classify_refusal`, no `ProviderRefusal`.

- [x] **Step 3: Add `ProviderRefusal`, the variant, and the classifier**

Put `ProviderRefusal`, `classify_refusal`, and the `type`/`code`-reading body
helper in `model.rs` (the classifier needs `extract_error_message`'s sibling
that returns the `type`/`code` field, not just `message` — `context.md` §3.5).
`classify_refusal` returns `None` for a 2xx with no error object, and
`Some(Unknown)` for a recognised-but-unclassified refusal. `Retry-After`
parsing (both forms) lives here, returning `Option<u64>`.

In `error.rs`, add the variant, its `code()` arms, and narrow
`is_retryable_message` to drop `"rate limit"` and `"429"`, with a comment naming
where rate limits are classified now.

- [x] **Step 4: Run the tests, the full gate, then show before committing**

Suggested subject line: `Classify provider refusals from status, with a typed error`

---

## Task 3: Retry on refusal, with a cancellable visible wait

Fold the refusal into `stream_response`'s loop under two bounds, honouring
`Retry-After`, and make the wait visible and interruptible.

**Files:**
- Modify: `crates/workspace-engine/src/model.rs`

**Interfaces:**
- Consumes: `ResponseMeta`, `classify_refusal`, `ProviderRefusal::is_transient`.
- Produces: the refusal retry in `stream_response` (attempts bounded at 4 beyond the first, wall-clock 90s, `Retry-After`-first then exponential-with-jitter), counted in `retry_count`.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn a_429_then_200_is_retried_and_succeeds() {
    let transport = MockModelTransport::sequence_with_status(vec![
        ("{\"error\":{\"message\":\"rate limited\"}}".to_string(), Some(429), None),
        ("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n".to_string(), Some(200), None),
    ]);
    let mut adapter = OpenAICompatibleAdapter::new("m", transport);
    let request = test_request();
    let run = adapter.stream_response(&request, &CancelToken::new(), &mut |_| {}).expect("retry succeeds");
    assert_eq!(run.content, "ok");
    assert_eq!(run.retry_count, 1, "the refusal is an attempt beyond the first");
}

#[test]
fn retry_after_delta_and_http_date_parse_to_the_same_wait() {
    assert_eq!(parse_retry_after("3"), Some(3));
    // A fixed date 3 seconds from now.
    let future = (SystemTime::now() + Duration::from_secs(3))
        .duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    assert_eq!(parse_retry_after_date(&format_http_date(future)), Some(3)); // within 1s
}

#[test]
fn a_retry_after_beyond_the_ceiling_fails_with_the_providers_figure() {
    let transport = MockModelTransport::sequence_with_status(vec![
        ("{\"error\":{\"message\":\"rate limited\"}}".to_string(), Some(429), Some(10_000)),
    ]);
    let mut adapter = OpenAICompatibleAdapter::new("m", transport);
    let error = adapter.stream_response(&test_request(), &CancelToken::new(), &mut |_| {}).expect_err("ceiling exceeded");
    assert!(format!("{error}").contains("10000"), "the message carries the provider's own figure");
}

#[test]
fn a_rate_limit_wait_is_interrupted_by_a_stop() {
    let transport = MockModelTransport::sequence_with_status(vec![
        ("{\"error\":{\"message\":\"rate limited\"}}".to_string(), Some(429), Some(30)),
        ("data: [DONE]\n".to_string(), Some(200), None),
    ]);
    let mut adapter = OpenAICompatibleAdapter::new("m", transport);
    let cancel = CancelToken::new();
    cancel.cancel();
    let error = adapter.stream_response(&test_request(), &cancel, &mut |_| {}).expect_err("stop wins");
    assert!(matches!(error, ClientError::Cancelled));
}
```

`MockModelTransport::sequence_with_status` is a new constructor alongside
`sequence` (context.md §3.3): it pairs each response with an optional status and
`retry_after`. The wait must be testable without sleeping 30s, so the wait
duration is injected or the wall-clock ceiling is lowered in the test — make the
ceiling a parameter on the adapter (`retry_wait_ceiling`) defaulting to 90s,
and set it small in the ceiling test.

- [x] **Step 2: Run the tests to verify they fail**

Expected: FAIL — no `sequence_with_status`, no `parse_retry_after`, refusal not retried.

- [x] **Step 3: Implement the refusal retry**

After `send_with_retries` returns `Ok(raw)` in `stream_response`, read
`self.transport.last_response_meta()` and `classify_refusal`. If it is
transient and the attempt/wall-clock bounds remain, sleep `Retry-After` (or
exponential-with-jitter from 1s), polling `cancel` at `CANCEL_POLL_INTERVAL` and
emitting a `PhaseKind::Model` phase with a label like `Retrying in 12s — rate
limited`, then re-send through the same path. Fold the refusal attempts into the
`retry_count` returned to the caller. On exhausting the bounds, return
`Err(ClientError::Provider(refusal, message))`.

The wait is a sibling of `send_with_retries`, not a rewrite of it: the
connection-level loop keeps its fixed 500/1500ms backoff and its
`!emitted_any` rule.

- [x] **Step 4: Run the tests, the full gate, then show before committing**

Suggested subject line: `Retry rate-limited calls under bounds, with a stoppable wait`

---

## Task 4: The outcome and accounting

Name the refusal on the failed task, finish the marker `"refused"`, and book a
measured zero so the refusal cannot inflate cost.

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`
- Modify: `crates/workspace-engine/src/chat.rs`
- Test: `crates/workspace-engine/tests/provider_limits.rs` (new)

**Interfaces:**
- Produces: `failure_kind` on the `task_status_updated` write and read; `finish_action(marker, "refused")` + `record_task_usage(measured_zero, reason "refused")` on the refusal arm; a `read` that surfaces `failureKind` on the task state.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn a_task_failed_by_a_refusal_carries_the_kind_and_a_measured_zero() {
    // Drive a turn against a mock that refuses before generating anything,
    // then assert the task state carries failureKind and read_task_usage is
    // unchanged (the refusal booked a measured zero, not the pre-call estimate).
}
```

```rust
#[test]
fn a_task_failed_by_a_refusal_keeps_its_plan() {
    // A planned task, refused, then resumed: the plan steps survive the refusal.
}
```

- [x] **Step 2: Run the tests to verify they fail**

Expected: FAIL — no `failure_kind` written, the refusal arm books the estimate.

- [x] **Step 3: Add `failure_kind` to the status write and read**

`update_task_status` gains `failure_kind: Option<&str>`, folded into the wrapped
payload as `"failureKind":"…"` beside `"error"`. `read_task_statuses` (and the
task-state surface the shell renders) reads it back. Existing callers pass
`None`; the refusal arm in `chat.rs` passes `"provider_rate_limited"` or
`"provider_quota_exhausted"` per the classification.

- [x] **Step 4: Finish the marker and book the zero on the refusal arm**

In `chat.rs`, the refusal branch (distinct from the `Cancelled` arm and the
generic `Err` arm) calls `finish_action(model_marker, "refused")` and
`record_task_usage(&task, run_id, Some(marker_id), TokenUsage::measured_zero(),
None, Some("refused"))` before failing the task. The generic non-refusal error
arm stays as-is for now (it is out of scope; `context.md` §3.8 records that it
leaves the marker dangling, and that remains true for the connection-failure
case).

- [x] **Step 5: Run the tests, the full gate, then show before committing**

Suggested subject line: `Record a provider refusal as a named failure that costs nothing`

---

## Task 5: Consented fallback and quota exhaustion

Requirement 7 and §5.7: the only route to a second provider is an explicit
per-occurrence approval, and quota exhaustion is permanent with a verbatim
message.

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs`
- Modify: `crates/desktop-shell/src/lib.rs` (the approval-shaped decision)
- Test: `crates/workspace-engine/tests/provider_limits.rs`

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn no_fallback_happens_without_approval() {
    // A refusal with a second provider configured and no approval given must
    // fail the task rather than silently switching.
}

#[test]
fn a_quota_exhaustion_fails_permanently_with_the_providers_message() {
    // QuotaExhausted: no retry, no fallback, message verbatim, failureKind
    // provider_quota_exhausted.
}
```

- [x] **Step 2: Implement the consent gate and quota handling**

The fallback offer is refused by default, not remembered, with no "always". The
switch, when approved, is recorded on the task and shown in the transcript.
`QuotaExhausted` fails with `failureKind: "provider_quota_exhausted"` and the
provider's message verbatim, and its plan survives.

- [x] **Step 3: Run the tests, the full gate, then show before committing**

Suggested subject line: `Offer a model fallback only with explicit consent`

---

## Task 6: Audit, documentation, and closing the spec

Requirement 8, §5.8, and §7.

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs` (the audit call)
- Modify: `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`
- Modify: `proposal.md` §7, and its `Status:` line; `docs/specs/README.md` row

- [x] **Step 1: Audit every refusal**

`AuditLog::record("provider_refusal", …)` carrying the classification and
status — never the request body or the API key. Assert in a test that no audit
line contains the body or the key.

- [x] **Step 2: Document**

`USER_GUIDE.md` gains what a rate-limit wait looks like, that it is stoppable,
and that a fallback is always asked for. `TROUBLESHOOTING.md` gains the six
classifications, what each means, and which are worth retrying by hand.

- [x] **Step 3: Fill §7 and close the spec**

Record the §7 findings (provider 200-with-error-object behaviour, observed
`Retry-After` honesty, whether 90s was right). Update the three summaries
together: the `Status:` line, the `README.md` row, and this table.

- [x] **Step 4: Run the full gate, then show before committing**

Suggested subject line: `Document provider refusals and their audit trail`
