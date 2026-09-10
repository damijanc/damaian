# Token and Cost Accounting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) · background and corrections in [`context.md`](context.md)
**Started:** not yet

**Goal:** Record what every model call cost in tokens, per task, distinguishing a
provider-reported figure from a local estimate, so a user can see afterwards what
a turn spent and Phase 2 has real numbers to enforce a ceiling against.

**Architecture:** `TokenUsage` rides on `ModelRun`, filled by the adapter —
measured when the provider's `usage` object arrives, estimated from
`payload.len()/4` when it does not. The chat and edit orchestrators append one
`task_usage_recorded` event per call to the append-only session log, and
`SessionStore::read_task_usage` sums them per task the same way
`read_task_statuses` replays status. Every consumer — the shell, the eval
harness, and later spec 23's completion report — reads that one function rather
than counting anything itself.

**Tech Stack:** Rust 2024 (workspace edition), no new dependencies. `serde_json`
for reading the provider's `usage` object; everything else is already in
`workspace-engine`.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) §3 first.** Five of the proposal's design
  statements describe behaviour the code does not have. Each task below already
  accounts for its correction; do not "fix" a task back to the proposal's
  wording.
- **An estimate is never presented as measured.** Requirement 4. This is a data
  rule *and* a display rule: every surface marks an estimated figure, and any
  aggregate containing one estimated term is estimated overall.
- **Under-reporting is the failure mode to avoid** (proposal §5.5). Where it is
  genuinely unknown whether a call was billed, count it and mark it estimated.
- **The session log is append-only.** Spec 17's rule. Usage is appended, never
  rewritten, and never back-patched onto an earlier event.
- **No API key, prompt text, or repository content in any usage record.**
  Requirement 7. Usage payloads carry numbers and ids only.
- **Requirement 6: never break a provider that rejects the addition.** The
  `stream_options` field is gated by config *and* by a runtime probe.
- **No new dependencies.** No tokenizer crate — the estimate is deliberately the
  existing `payload.len().div_ceil(4)` (proposal §5.3).
- **Clippy warnings are errors.** Fix rather than suppress; an `#[allow(...)]`
  needs a comment saying why.
- **Every quality-gate command from `AGENTS.md` must pass** at the end of every
  task. That is seven commands and the list in `AGENTS.md` is authoritative —
  read it, do not rely on a remembered list. `typos` and
  `node --check crates/desktop-shell/static/app.js` are the two that get missed.
- **Commit messages:** one subject line, no body, no `Co-Authored-By`. Rationale
  belongs in this plan and the proposal. Never cite commit SHAs in
  documentation.
- **Never switch branches.** Other processes write to this tree concurrently.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `TokenUsage` on `ModelRun`, estimated everywhere | Done | 2 tests, gate green at 440. Used the existing `test_request()` helper rather than the plan's inline literals, which localizes Task 3's new field to one place. **One plan error:** the usage estimate cannot live in the `ModelRun` literal — `content` is moved by an earlier field, so both adapters compute it into a local first. `Eq` dropped from `ModelRun`, `ChatTurnResult` and `EditProposalResult` as planned; nothing used them as a map key |
| 2 · Read the provider's reported usage | Done | `extract_usage` + `usage_payloads`, **6 tests, not the planned 5** — added `a_half_reported_usage_object_is_not_treated_as_measured`, because a usage object carrying only `prompt_tokens` would otherwise have become a measured figure with a fabricated zero in it, which is exactly the fabrication requirement 4 forbids. Gate green at 446. Called `extract_usage` once rather than the plan's twice, since the result is needed for both the usage and the cost field |
| 3 · Ask for usage, and the capability probe | Done | `request_usage` on the request, `provider_reports_usage` in config, `MockModelTransport::sequence`, and the probe. **8 tests, gate green at 454.** Two deviations from the plan, both to avoid churn: (1) the send-with-retries loop was extracted to `send_with_retries` so the probe re-sends through one definition rather than a labelled double loop; (2) **the adapter does not take an `AuditLog`** — it reports the observation as `ModelRun.usage_reporting_unsupported`, true exactly once per provider per process, and the orchestrator audits it. That follows the precedent `session.rs` already sets for not threading an `AuditLog` through many construction sites for a diagnostic. Also found, unrelated and filed separately: **an unknown key in a repository config fails the entire config load** (`config.rs:399`, `ConfigOverlay::load(path)?`) instead of being reported as a rejected key, so a cloned repo can break config loading for itself |
| 4 · The usage event and per-task aggregation | Done | `TaskUsage`, `record_task_usage`, `record_task_usage_for_task_id` and `read_task_usage`. **6 tests, not the planned 5** — added `cost_is_reported_when_every_run_carried_one` as the other half of the all-or-nothing cost rule, because asserting only the `None` case would pass if cost were never reported at all. Gate green at 460. The by-id sibling was added here rather than in Task 6: recovery holds ids and never rebuilds a `Task`, and discovering that in Task 6 would have meant editing this file twice |
| 5 · Record usage from both model call sites | Not started | |
| 6 · A lost call is counted at recovery | Not started | |
| 7 · The task-state surface | Not started | |
| 8 · Cost from user-configured rates | Not started | |
| 9 · The eval harness reads real figures | Not started | |
| 10 · Documentation and closing the spec | Not started | |

## File Structure

| File | Responsibility |
|---|---|
| `crates/workspace-engine/src/model.rs` | `UsageSource`, `TokenUsage`, the fields on `ModelRun`, usage parsing, the `stream_options` field and the probe. |
| `crates/workspace-engine/src/session.rs` | The `task_usage_recorded` event, `TaskUsage`, `read_task_usage`, and the pre-call estimate on the action marker. |
| `crates/workspace-engine/src/chat.rs` | Recording usage per round, per retry, and on a cancelled turn. |
| `crates/workspace-engine/src/edit.rs` | Recording usage for the patch-proposal model call. |
| `crates/workspace-engine/src/recovery.rs` | Appending an estimated usage event for a call lost to a crash. |
| `crates/workspace-engine/src/config.rs` | `provider_reports_usage` and the two optional price rates. |
| `crates/desktop-shell/src/lib.rs` | Usage on the `/api/session` payload and the chat turn response. |
| `crates/desktop-shell/static/app.js` | The per-turn usage line. |
| `crates/eval-harness/src/runner.rs` | Reading `read_task_usage` into the run record. |
| `crates/workspace-engine/tests/token_accounting.rs` | New integration test file for §5.4, §5.5 and the recovery case. |

## Interface reference

Verified against the tree on 2026-09-10. Every signature a later task depends
on, so no task has to go looking.

| Symbol | Where | Shape |
|---|---|---|
| `ModelRun` | `model.rs:172` | `#[derive(Debug, Clone, PartialEq, Eq)]` — the `Eq` is removed in Task 1 |
| `ModelRun::cancelled_before_start` | `model.rs:197` | `(provider: &str, model: &str) -> Self` |
| `ModelAdapter::estimate_tokens` | `model.rs:223` | `(&self, payload: &str) -> usize`, `payload.len().div_ceil(4)` |
| `model_request_json` | `model.rs:770` | `(request: &ModelRequest) -> String` |
| `extract_error_message` | used at `model.rs:691` | returns `Option<String>` for a body carrying an `error` object |
| `OpenAICompatibleAdapter::stream_response` | `model.rs:619` | builds `body` once at line 630, retries in a loop, returns the whole stream as `raw` |
| `MockModelTransport` | `model.rs:555` | one `response` for every call — Task 3 adds a sequence |
| `SessionStore::append_session_event` | `session.rs:706` | private; `(session_id, event_type, payload_json) -> Result<()>` |
| `SessionStore::start_action` | `session.rs:565` | `(&Task, action, reference, side_effecting) -> Result<ActionMarker>` |
| `SessionStore::dangling_actions` | `session.rs:621` | `(session_id) -> Result<Vec<DanglingAction>>`, paired by `markerId` |
| `DanglingAction` | `session.rs:185` | `{ task_id, action, reference, side_effecting, seq }` |
| `active_events` / `parsed_events` | `session.rs:888` / `861` | rewind-aware / everything; both yield `SessionEvent { seq, event_type, payload }` with `.text(field)` |
| `AuditLog::record` | `audit.rs:42` | `(event_type: &str, fields: &[(&str, String)]) -> Result<String>`, every value redacted |
| `classify_session` | `recovery.rs:46` | `(store, audit, session_id) -> Result<Vec<RecoveredTask>>` |
| chat model call | `chat.rs:957-991` | `start_action("model_call", model_name, false)` → `stream_response` → `finish_action(marker, "ok")` |
| edit model call | `edit.rs:310` | `stream_response`, with **no** action marker |
| eval `Tokens` | `eval-harness/src/record.rs:6` | `{ input, output, measured }`, hardcoded to zero at `runner.rs:427` |

---

### Task 1: `TokenUsage` on `ModelRun`, estimated everywhere

Every run gets a figure from this task on. It is an estimate for now — Task 2
adds the measured path — so the type exists and is honest before anything reads
it.

**Files:**
- Modify: `crates/workspace-engine/src/model.rs`
- Modify: `crates/workspace-engine/src/chat.rs:168` (drop `Eq`)
- Modify: `crates/workspace-engine/src/edit.rs:29` (drop `Eq`)
- Modify: `crates/workspace-engine/src/lib.rs` (export the new types)
- Test: `crates/workspace-engine/src/model.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `UsageSource::{Measured, Estimated}`, `TokenUsage { input_tokens: u64, output_tokens: u64, source: UsageSource }`, `ModelRun.usage: TokenUsage`, `ModelRun.reported_cost: Option<f64>`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `model.rs`:

```rust
#[test]
fn a_run_with_no_reported_usage_is_estimated_from_the_request_and_the_content() {
    let transport = MockModelTransport::new("data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n");
    let mut adapter = OpenAICompatibleAdapter::new("m", transport);
    let request = ModelRequest {
        provider: "openai-compatible".to_string(),
        model: "m".to_string(),
        messages: vec![ModelMessage::user("count my tokens")],
        temperature: None,
        reasoning_level: None,
        stream: true,
        tools: None,
        max_tokens: None,
    };
    let run = adapter
        .stream_response(&request, &CancelToken::new(), &mut |_| {})
        .expect("the mock stream should produce a run");

    assert_eq!(run.usage.source, UsageSource::Estimated);
    // The estimate is over the serialised request, not the prompt alone, so it
    // is larger than the user's text and never zero.
    let body_estimate = model_request_json(&request).len().div_ceil(4) as u64;
    assert_eq!(run.usage.input_tokens, body_estimate);
    assert_eq!(run.usage.output_tokens, "hello".len().div_ceil(4) as u64);
    assert_eq!(run.reported_cost, None);
}

#[test]
fn a_turn_cancelled_before_the_provider_was_called_is_a_measured_zero() {
    // The one genuinely free case (proposal §5.5): nothing was sent, so nothing
    // was billed, and that is a fact rather than an estimate.
    let run = ModelRun::cancelled_before_start("openai-compatible", "m");
    assert_eq!(run.usage.source, UsageSource::Measured);
    assert_eq!(run.usage.input_tokens, 0);
    assert_eq!(run.usage.output_tokens, 0);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p workspace-engine --lib model::tests::a_run_with_no_reported_usage_is_estimated_from_the_request_and_the_content model::tests::a_turn_cancelled_before_the_provider_was_called_is_a_measured_zero`

Expected: FAIL — `no field 'usage' on type 'ModelRun'`, `cannot find type 'UsageSource'`.

- [ ] **Step 3: Add the types**

In `model.rs`, above `ModelRequest`:

```rust
/// Whether a token figure came from the provider or from a local
/// approximation. Per run rather than per task, because one task mixes them:
/// a provider that reports usage on a completed call reports nothing for a
/// call whose stream was cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageSource {
    /// Reported by the provider for this call.
    Measured,
    /// Derived locally from payload size. Never presented as measured.
    Estimated,
}

impl UsageSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Estimated => "estimated",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "measured" => Some(Self::Measured),
            "estimated" => Some(Self::Estimated),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub source: UsageSource,
}

impl TokenUsage {
    /// The only zero that is a fact rather than a guess: no request was sent.
    pub fn measured_zero() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            source: UsageSource::Measured,
        }
    }

    pub fn estimated(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            source: UsageSource::Estimated,
        }
    }
}
```

- [ ] **Step 4: Add the fields and drop the `Eq` that `f64` forbids**

`Option<f64>` is not `Eq`, and three structs derive it transitively. Nothing
uses them as a map key or in a set — checked — so `PartialEq` is enough, and
that is what `assert_eq!` needs.

In `model.rs`, change `#[derive(Debug, Clone, PartialEq, Eq)]` above
`pub struct ModelRun` to `#[derive(Debug, Clone, PartialEq)]` and add:

```rust
    /// What this call cost in tokens. Always populated: measured when the
    /// provider said, estimated when it did not (proposal §5.1).
    pub usage: TokenUsage,
    /// Cost as the provider reported it. `None` is the normal case and means
    /// "this provider did not tell us", never "free".
    pub reported_cost: Option<f64>,
```

Do the same to `pub struct ChatTurnResult` (`chat.rs:168`) and
`pub struct EditProposalResult` (`edit.rs:29`).

- [ ] **Step 5: Populate every construction site**

There are three. In `ModelRun::cancelled_before_start`:

```rust
            usage: TokenUsage::measured_zero(),
            reported_cost: None,
```

In `MockModelAdapter::stream_response`'s `Ok(ModelRun { … })` (`model.rs:335`),
before the closing brace:

```rust
            usage: TokenUsage::estimated(
                model_request_json(request).len().div_ceil(4) as u64,
                content.len().div_ceil(4) as u64,
            ),
            reported_cost: None,
```

In `OpenAICompatibleAdapter::stream_response`'s `Ok(ModelRun { … })`
(`model.rs:701`) — `body` is already in scope from line 630, and `content` is
the accumulated stream:

```rust
            usage: TokenUsage::estimated(
                self.estimate_tokens(&body) as u64,
                self.estimate_tokens(&content) as u64,
            ),
            reported_cost: None,
```

- [ ] **Step 6: Export the types**

In `crates/workspace-engine/src/lib.rs`, add `TokenUsage` and `UsageSource` to
the existing `pub use model::{…}` list.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p workspace-engine --lib model::tests::a_run_with_no_reported_usage_is_estimated_from_the_request_and_the_content model::tests::a_turn_cancelled_before_the_provider_was_called_is_a_measured_zero`

Expected: PASS, 2 tests.

- [ ] **Step 8: Run the full quality gate**

All seven commands from `AGENTS.md`. Expect the whole workspace suite green —
this task changes no behaviour, only adds fields.

- [ ] **Step 9: Commit**

```bash
git add crates/workspace-engine/src
git commit -m "Give every model run a token figure, estimated until the provider reports one"
```

---

### Task 2: Read the provider's reported usage

**Files:**
- Modify: `crates/workspace-engine/src/model.rs`
- Test: `crates/workspace-engine/src/model.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `TokenUsage`, `UsageSource` from Task 1.
- Produces: `fn extract_usage(raw: &str) -> Option<(u64, u64, Option<f64>)>` — input tokens, output tokens, and the provider's cost when it reports one.

The whole stream body is already in `raw` (`pump_stream` accumulates it and
`extract_tool_calls(&raw)` and `response_was_truncated(&raw)` already read it),
so this is a sibling of those, not a change to the incremental parser.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn usage_is_read_from_the_final_chunk_of_a_stream() {
    let raw = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11902,\"completion_tokens\":812}}\n\n",
        "data: [DONE]\n"
    );
    assert_eq!(extract_usage(raw), Some((11902, 812, None)));
}

#[test]
fn usage_accepts_the_input_output_naming_some_providers_use() {
    let raw = "data: {\"choices\":[],\"usage\":{\"input_tokens\":7,\"output_tokens\":3}}\n";
    assert_eq!(extract_usage(raw), Some((7, 3, None)));
}

#[test]
fn a_reported_cost_is_carried_when_the_provider_sends_one() {
    let raw = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"cost\":0.00031}}\n";
    assert_eq!(extract_usage(raw), Some((5, 2, Some(0.00031))));
}

#[test]
fn a_stream_without_a_usage_object_reports_nothing_rather_than_zero() {
    // The distinction requirement 4 rests on: absent is not the same as zero,
    // and a zero here would be presented as measured.
    let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n";
    assert_eq!(extract_usage(raw), None);
}

#[test]
fn a_measured_run_carries_the_providers_figures_not_the_estimate() {
    let transport = MockModelTransport::new(concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":41,\"completion_tokens\":9}}\n\n",
        "data: [DONE]\n"
    ));
    let mut adapter = OpenAICompatibleAdapter::new("m", transport);
    let request = ModelRequest {
        provider: "openai-compatible".to_string(),
        model: "m".to_string(),
        messages: vec![ModelMessage::user("hi")],
        temperature: None,
        reasoning_level: None,
        stream: true,
        tools: None,
        max_tokens: None,
    };
    let run = adapter
        .stream_response(&request, &CancelToken::new(), &mut |_| {})
        .expect("the mock stream should produce a run");
    assert_eq!(run.usage.source, UsageSource::Measured);
    assert_eq!(run.usage.input_tokens, 41);
    assert_eq!(run.usage.output_tokens, 9);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p workspace-engine --lib model::tests::usage model::tests::a_reported_cost model::tests::a_stream_without_a_usage model::tests::a_measured_run`

Expected: FAIL — `cannot find function 'extract_usage'`.

- [ ] **Step 3: Implement the parser**

Next to `extract_model_tokens` in `model.rs`:

```rust
/// The provider's own token figures, when it reported any.
///
/// Reads the whole body rather than hooking the incremental reader: usage
/// arrives on a final chunk whose `choices` array is empty, which the token
/// extractor already passes over, and `raw` holds the complete stream by the
/// time this is called. The last `usage` object wins, so a provider that
/// repeats it per chunk reports its final total rather than its first
/// partial one.
///
/// `prompt_tokens`/`completion_tokens` is the OpenAI naming;
/// `input_tokens`/`output_tokens` is accepted as an alias because providers
/// differ and a missed alias reports an estimate as if nothing was measured.
pub fn extract_usage(raw: &str) -> Option<(u64, u64, Option<f64>)> {
    let mut found = None;
    for payload in usage_payloads(raw) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) else {
            continue;
        };
        let input = usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(serde_json::Value::as_u64);
        let output = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(serde_json::Value::as_u64);
        // Both or neither: a half-read usage object would be a measured figure
        // with a fabricated zero in it.
        if let (Some(input), Some(output)) = (input, output) {
            let cost = usage.get("cost").and_then(serde_json::Value::as_f64);
            found = Some((input, output, cost));
        }
    }
    found
}

/// The JSON payloads of a body, whether it is an SSE stream or a single
/// non-streaming response.
fn usage_payloads(raw: &str) -> Vec<String> {
    if !raw.contains("data:") {
        return vec![raw.to_string()];
    }
    raw.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("data:"))
        .map(|line| line.trim_start_matches("data:").trim().to_string())
        .filter(|payload| payload != "[DONE]")
        .collect()
}
```

- [ ] **Step 4: Use it in the adapter**

In `OpenAICompatibleAdapter::stream_response`, replace the `usage` and
`reported_cost` fields added in Task 1 with:

```rust
            usage: match extract_usage(&raw) {
                Some((input_tokens, output_tokens, _)) => TokenUsage {
                    input_tokens,
                    output_tokens,
                    source: UsageSource::Measured,
                },
                None => TokenUsage::estimated(
                    self.estimate_tokens(&body) as u64,
                    self.estimate_tokens(&content) as u64,
                ),
            },
            reported_cost: extract_usage(&raw).and_then(|(_, _, cost)| cost),
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p workspace-engine --lib model::tests::usage model::tests::a_reported_cost model::tests::a_stream_without_a_usage model::tests::a_measured_run`

Expected: PASS, 5 tests. The Task 1 estimate test must still pass — its stream
carries no `usage` object.

- [ ] **Step 6: Run the full quality gate, then commit**

```bash
git add crates/workspace-engine/src/model.rs
git commit -m "Read the provider's token figures when it reports them"
```

---

### Task 3: Ask for usage, and the capability probe

Nothing above yields a measured figure yet, because an OpenAI-compatible
streaming API omits `usage` unless asked. This task asks — without breaking a
provider that rejects the asking.

**Read [`context.md`](context.md) §3.1 and §3.2 before starting.** A provider
400 arrives here as a *successful* read whose body carries an `error` object,
not as a transport failure, so the probe branches on `extract_error_message`.
`MockModelTransport::failing` — which the proposal's §6 names — raises
`ClientError::Io` and is the wrong tool.

**Files:**
- Modify: `crates/workspace-engine/src/model.rs`
- Modify: `crates/workspace-engine/src/config.rs`
- Modify: `crates/workspace-engine/src/chat.rs` (populate the new request field)
- Modify: `crates/workspace-engine/src/edit.rs` (populate the new request field)
- Test: `crates/workspace-engine/src/model.rs`, `crates/workspace-engine/src/config.rs`, `crates/workspace-engine/tests/repository_config_trust.rs`

**Interfaces:**
- Consumes: `extract_usage` from Task 2.
- Produces: `ModelRequest.request_usage: bool`; `ModelProviderConfig.provider_reports_usage: bool` (default `true`); `Config::provider_reports_usage(&self) -> bool`; `MockModelTransport::sequence(Vec<String>)`.

- [ ] **Step 1: Write the failing tests**

In `model.rs`:

```rust
#[test]
fn a_streaming_request_asks_for_usage_when_the_provider_supports_it() {
    let request = ModelRequest {
        provider: "openai-compatible".to_string(),
        model: "m".to_string(),
        messages: vec![ModelMessage::user("hi")],
        temperature: None,
        reasoning_level: None,
        stream: true,
        tools: None,
        max_tokens: None,
        request_usage: true,
    };
    assert!(model_request_json(&request).contains("\"stream_options\":{\"include_usage\":true}"));
}

#[test]
fn a_non_streaming_request_never_asks_for_usage() {
    // `stream_options` is meaningless without a stream and 400s on some
    // providers, so the flag alone must not be enough to emit it.
    let request = ModelRequest {
        provider: "openai-compatible".to_string(),
        model: "m".to_string(),
        messages: vec![ModelMessage::user("hi")],
        temperature: None,
        reasoning_level: None,
        stream: false,
        tools: None,
        max_tokens: None,
        request_usage: true,
    };
    assert!(!model_request_json(&request).contains("stream_options"));
}

#[test]
fn a_provider_that_rejects_stream_options_is_retried_once_without_it() {
    let transport = MockModelTransport::sequence(vec![
        "{\"error\":{\"message\":\"Unrecognized request argument supplied: stream_options\"}}"
            .to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n".to_string(),
    ]);
    let mut adapter = OpenAICompatibleAdapter::new("m", transport);
    let request = ModelRequest {
        provider: "openai-compatible".to_string(),
        model: "m".to_string(),
        messages: vec![ModelMessage::user("hi")],
        temperature: None,
        reasoning_level: None,
        stream: true,
        tools: None,
        max_tokens: None,
        request_usage: true,
    };
    let run = adapter
        .stream_response(&request, &CancelToken::new(), &mut |_| {})
        .expect("the second attempt should succeed");

    assert_eq!(run.content, "hello");
    // A probe is not a failed call: counting it would inflate the retry figure
    // and, once Task 5 lands, bill the user for a request that never ran.
    assert_eq!(run.retry_count, 0);
    assert_eq!(run.usage.source, UsageSource::Estimated);
    assert!(!adapter.probe_supports_usage());
}

#[test]
fn the_probe_happens_once_rather_than_on_every_turn() {
    let transport = MockModelTransport::sequence(vec![
        "{\"error\":{\"message\":\"Unrecognized request argument supplied: stream_options\"}}"
            .to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\ndata: [DONE]\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\ndata: [DONE]\n".to_string(),
    ]);
    let mut adapter = OpenAICompatibleAdapter::new("m", transport);
    let request = ModelRequest {
        provider: "openai-compatible".to_string(),
        model: "m".to_string(),
        messages: vec![ModelMessage::user("hi")],
        temperature: None,
        reasoning_level: None,
        stream: true,
        tools: None,
        max_tokens: None,
        request_usage: true,
    };
    adapter
        .stream_response(&request, &CancelToken::new(), &mut |_| {})
        .expect("first call succeeds after the probe");
    adapter
        .stream_response(&request, &CancelToken::new(), &mut |_| {})
        .expect("second call succeeds directly");

    // Three sent bodies would mean the second turn probed again.
    assert_eq!(adapter.sent_bodies().len(), 3);
    assert!(adapter.sent_bodies()[0].contains("stream_options"));
    assert!(!adapter.sent_bodies()[1].contains("stream_options"));
    assert!(!adapter.sent_bodies()[2].contains("stream_options"));
}

#[test]
fn a_provider_error_that_is_not_about_usage_is_still_an_error() {
    // The probe must not swallow real failures by retrying everything.
    let transport = MockModelTransport::sequence(vec![
        "{\"error\":{\"message\":\"Insufficient balance\"}}".to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n".to_string(),
    ]);
    let mut adapter = OpenAICompatibleAdapter::new("m", transport);
    let request = ModelRequest {
        provider: "openai-compatible".to_string(),
        model: "m".to_string(),
        messages: vec![ModelMessage::user("hi")],
        temperature: None,
        reasoning_level: None,
        stream: true,
        tools: None,
        max_tokens: None,
        request_usage: true,
    };
    let error = adapter
        .stream_response(&request, &CancelToken::new(), &mut |_| {})
        .expect_err("a balance error must not be retried as a capability probe");
    assert!(format!("{error}").contains("Insufficient balance"));
}
```

In `config.rs` `mod tests`:

```rust
#[test]
fn provider_usage_reporting_defaults_on_and_can_be_turned_off() {
    let mut config = Config::default();
    assert!(config.provider_reports_usage());
    config
        .apply_overlay(overlay_from(
            "model_provider.deepseek.provider_reports_usage=false",
        ))
        .unwrap();
    config.model_provider = "deepseek".to_string();
    assert!(!config.provider_reports_usage());
}
```

In `crates/workspace-engine/tests/repository_config_trust.rs`, mirroring the
existing provider-key cases:

```rust
#[test]
fn a_repository_cannot_turn_off_usage_reporting() {
    // Not a security hole on its own, but `model_provider.*` is forbidden to
    // repository scope wholesale (spec 34) and a new key must not become the
    // exception that proves an overlay is scope-blind again.
    let report = apply_repository_config("model_provider.deepseek.provider_reports_usage=false");
    assert!(
        report
            .rejected_keys
            .iter()
            .any(|key| key.key == "model_provider.deepseek"),
        "rejected keys were {:?}",
        report.rejected_keys
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p workspace-engine model::tests::a_streaming_request_asks model::tests::a_non_streaming_request model::tests::a_provider_that_rejects model::tests::the_probe_happens_once model::tests::a_provider_error_that_is_not config::tests::provider_usage_reporting a_repository_cannot_turn_off`

Expected: FAIL — missing field `request_usage`, no `MockModelTransport::sequence`, no `provider_reports_usage`.

- [ ] **Step 3: Add the request field and emit `stream_options`**

In `model.rs`, add to `ModelRequest`:

```rust
    /// Ask the provider to report token usage on the stream. Only meaningful
    /// with [`Self::stream`]; see spec 19 §5.2.
    pub request_usage: bool,
```

In `model_request_json`, after the `max_tokens` block:

```rust
    // Streaming only: `stream_options` is meaningless on a non-streaming call
    // and rejected outright by some providers.
    if request.request_usage && request.stream {
        body.push_str(",\"stream_options\":{\"include_usage\":true}");
    }
```

Fix every other `ModelRequest` construction site the compiler names — `chat.rs`
(around line 921) and `edit.rs` — with `request_usage:
self.config.provider_reports_usage(),` and each test literal with
`request_usage: false,` unless the test is about usage.

- [ ] **Step 4: Give the mock transport a sequence**

In `model.rs`, add to `MockModelTransport`:

```rust
    /// Responses handed out in order, one per call, the last one repeating.
    /// Needed because the usage probe is defined by what the *second* call
    /// returns, which a single `response` cannot express.
    pub responses: Vec<String>,
    pub next_response: usize,
```

```rust
    pub fn sequence(responses: Vec<String>) -> Self {
        Self {
            responses,
            next_response: 0,
            ..Self::new(String::new())
        }
    }
```

and in its `ModelTransport::send`/`send_stream`, take from `responses` when it
is non-empty, advancing `next_response` but holding at the last entry, and fall
back to `response` when it is empty so existing callers are untouched.

- [ ] **Step 5: Implement the probe**

Add to `OpenAICompatibleAdapter`:

```rust
    /// `None` until the provider has been observed, then the answer for the
    /// rest of the process. Spec 19 §5.2: the probe happens once, not on every
    /// turn. Phase 1 WP3's capability profile is the durable home for this;
    /// this field is shaped so WP3 can adopt the observation.
    supports_usage: Option<bool>,
```

with accessors the tests use:

```rust
    pub fn probe_supports_usage(&self) -> bool {
        self.supports_usage.unwrap_or(true)
    }
```

In `stream_response`, replace the single `let body = model_request_json(request);`
with a body that respects the observation, and wrap the send loop so a
usage-shaped rejection retries once:

```rust
        let mut ask_for_usage = request.request_usage && self.supports_usage.unwrap_or(true);
        let mut body = model_request_json(&ModelRequest {
            request_usage: ask_for_usage,
            ..request.clone()
        });
```

and after the existing `if let Some(message) = extract_error_message(&raw)`
check, before it returns:

```rust
        if let Some(message) = extract_error_message(&raw) {
            // A provider that does not know `stream_options` says so in the
            // body of a 200-shaped error, because curl -sS exits zero on a 4xx.
            // Retry once without the field: this is a capability probe, not a
            // failed call, so it does not count as an attempt.
            if ask_for_usage && mentions_unsupported_usage_option(&message) {
                self.supports_usage = Some(false);
                self.audit_unsupported_usage();
                ask_for_usage = false;
                body = model_request_json(&ModelRequest {
                    request_usage: false,
                    ..request.clone()
                });
                // Re-run the send loop once with the narrowed body; `attempt`
                // is reset so `retry_count` reports zero for a clean probe.
                …
            }
            return Err(ClientError::Io(format!("Model provider error: {message}")));
        }
```

Structure this as an outer `for probe_pass in 0..2` around the existing retry
loop rather than duplicating the send code, so there is one definition of how a
request is sent. On a successful send set `self.supports_usage = Some(true)`
when `ask_for_usage` was true and `extract_usage(&raw)` returned `Some`.

```rust
/// The provider is telling us it does not understand the usage request, as
/// opposed to any of the other things a provider says no to.
fn mentions_unsupported_usage_option(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("stream_options") || lowered.contains("include_usage")
}
```

`audit_unsupported_usage` records
`model_usage_reporting_unsupported` with the provider and model, so the user can
set the flag permanently. The adapter has no `AuditLog`; give
`OpenAICompatibleAdapter` an `Option<AuditLog>` set by the constructor the
orchestrators use, and leave it `None` for the bare `new`.

- [ ] **Step 6: Add the config key**

In `config.rs`: add `pub provider_reports_usage: bool` to `ModelProviderConfig`
and `Option<bool>` to `ModelProviderConfigOverlay`; parse
`"provider_reports_usage" => provider.provider_reports_usage = Some(parse_bool(field, value)?)`
in `set_model_provider_config` (`config.rs:1400`); default it to `true` in
`upsert_model_provider`'s push and in every `ModelProviderConfig` literal the
compiler names; and add the resolver next to `supports_native_tools`
(`config.rs:728`):

```rust
    /// Whether to ask this provider for token usage. Defaults to true: the
    /// field is standard for OpenAI-compatible APIs, and §5.2's probe handles
    /// the ones that reject it without the user having to know.
    pub fn provider_reports_usage(&self) -> bool {
        self.model_provider_config(&self.model_provider)
            .map(|provider| provider.provider_reports_usage)
            .unwrap_or(true)
    }
```

No scope work is needed: `model_provider.*` is already rejected wholesale from
repository scope at `config.rs:529-536`. The test in Step 1 pins that.

- [ ] **Step 7: Run the tests to verify they pass**

Run the Step 2 command. Expected: PASS, 7 tests.

- [ ] **Step 8: Run the full quality gate, then commit**

```bash
git add crates/workspace-engine/src crates/workspace-engine/tests
git commit -m "Ask providers for token usage, and probe once for the ones that refuse"
```

---

### Task 4: The usage event and per-task aggregation

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`
- Modify: `crates/workspace-engine/src/lib.rs` (export `TaskUsage`)
- Test: `crates/workspace-engine/tests/token_accounting.rs` (new)

**Interfaces:**
- Consumes: `TokenUsage`, `UsageSource` from Task 1.
- Produces: `SessionStore::record_task_usage(&self, task: &Task, run_id: &str, marker_id: Option<&str>, usage: TokenUsage, reported_cost: Option<f64>, reason: Option<&str>) -> Result<()>` and `SessionStore::read_task_usage(&self, session_id: &str) -> Result<HashMap<String, TaskUsage>>`, with `pub struct TaskUsage { pub input_tokens: u64, pub output_tokens: u64, pub source: UsageSource, pub reported_cost: Option<f64>, pub run_count: u32 }`.

- [ ] **Step 1: Write the failing tests**

Create `crates/workspace-engine/tests/token_accounting.rs`:

```rust
//! Per-task token accounting, per `docs/specs/19_token_and_cost_accounting/`.
//!
//! The rule these tests hold is requirement 4: an estimate is never presented
//! as measured, and an aggregate is only as trustworthy as its weakest term.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{SessionStore, Task, TokenUsage, UsageSource};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_data_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-usage-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn store_with_task(name: &str) -> (SessionStore, Task) {
    let store = SessionStore::new(temp_data_dir(name));
    let session = store.create_session("repo_1", "Usage").unwrap();
    let task = store
        .create_task(&session.id, "do the thing", "mock", "m")
        .unwrap();
    (store, task)
}

fn measured(input: u64, output: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        source: UsageSource::Measured,
    }
}

#[test]
fn a_tasks_usage_is_the_sum_of_its_runs() {
    let (store, task) = store_with_task("sum");
    store
        .record_task_usage(&task, "modelrun_1", None, measured(100, 10), None, None)
        .unwrap();
    store
        .record_task_usage(&task, "modelrun_2", None, measured(200, 20), None, None)
        .unwrap();

    let usage = store.read_task_usage(&task.session_id).unwrap();
    let total = usage.get(&task.id).expect("the task should have usage");
    assert_eq!(total.input_tokens, 300);
    assert_eq!(total.output_tokens, 30);
    assert_eq!(total.run_count, 2);
    assert_eq!(total.source, UsageSource::Measured);
}

#[test]
fn one_estimated_run_makes_the_whole_total_estimated() {
    let (store, task) = store_with_task("mixed");
    store
        .record_task_usage(&task, "modelrun_1", None, measured(100, 10), None, None)
        .unwrap();
    store
        .record_task_usage(
            &task,
            "modelrun_2",
            None,
            TokenUsage::estimated(50, 5),
            None,
            None,
        )
        .unwrap();

    let usage = store.read_task_usage(&task.session_id).unwrap();
    let total = &usage[&task.id];
    assert_eq!(total.input_tokens, 150);
    assert_eq!(
        total.source,
        UsageSource::Estimated,
        "a total containing an estimate is an estimate"
    );
}

#[test]
fn cost_is_summed_only_when_every_run_reported_one() {
    // A partial sum would understate the bill while looking like a real figure.
    let (store, task) = store_with_task("cost");
    store
        .record_task_usage(&task, "modelrun_1", None, measured(1, 1), Some(0.01), None)
        .unwrap();
    store
        .record_task_usage(&task, "modelrun_2", None, measured(1, 1), None, None)
        .unwrap();

    let usage = store.read_task_usage(&task.session_id).unwrap();
    assert_eq!(usage[&task.id].reported_cost, None);
}

#[test]
fn a_session_written_before_this_change_reports_no_runs_rather_than_a_zero() {
    // Requirement: absence is distinguishable from a task that used nothing.
    let (store, task) = store_with_task("legacy");
    let usage = store.read_task_usage(&task.session_id).unwrap();
    assert!(
        usage.get(&task.id).is_none(),
        "a task with no usage events must be absent, not zero"
    );
}

#[test]
fn a_usage_record_carries_no_prompt_or_file_content() {
    // Requirement 7, asserted against the bytes on disk rather than the type.
    let (store, task) = store_with_task("no-content");
    store
        .record_task_usage(&task, "modelrun_1", None, measured(100, 10), None, None)
        .unwrap();
    let log = fs::read_to_string(
        store
            .session_log_path_for_test(&task.session_id)
    )
    .unwrap();
    let usage_line = log
        .lines()
        .find(|line| line.contains("task_usage_recorded"))
        .expect("the usage event should be on disk");
    assert!(!usage_line.contains("do the thing"));
}
```

`session_log_path_for_test` does not exist; use the path directly —
`data_dir.join("sessions").join(format!("{session_id}.jsonl"))` — matching what
`crash_recovery.rs` does, and keep the `data_dir` from `temp_data_dir` in the
fixture tuple to build it.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p workspace-engine --test token_accounting`

Expected: FAIL — `no method named 'record_task_usage'`.

- [ ] **Step 3: Add `TaskUsage` and the writer**

In `session.rs`:

```rust
/// A task's total usage, summed from its `task_usage_recorded` events.
///
/// Not stored as a record for the same reason `Task` is not: the event log is
/// where task facts live, and a crash mid-task then loses at most the run that
/// was in flight while the completed runs stay correct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TaskUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `Estimated` when any contributing run was estimated (proposal §5.3).
    pub source: UsageSource,
    /// `Some` only when every contributing run reported a cost; a partial sum
    /// would understate the bill while looking authoritative.
    pub reported_cost: Option<f64>,
    pub run_count: u32,
}
```

```rust
    /// Appends one run's usage. One event per call that reached the provider.
    ///
    /// `marker_id` ties the event to the `action_started` marker for that call,
    /// so recovery can tell a call it already accounted for from one it has
    /// not (Task 6). `reason` explains a non-obvious estimate — a lost call, a
    /// stopped stream — and is a fixed harness-set string, never free text
    /// from a model or a file.
    pub fn record_task_usage(
        &self,
        task: &Task,
        run_id: &str,
        marker_id: Option<&str>,
        usage: TokenUsage,
        reported_cost: Option<f64>,
        reason: Option<&str>,
    ) -> Result<()> {
        let cost = match reported_cost {
            Some(cost) => format!("{cost}"),
            None => "null".to_string(),
        };
        let marker = match marker_id {
            Some(marker_id) => format!(",\"markerId\":\"{}\"", escape_json(marker_id)),
            None => String::new(),
        };
        let reason = match reason {
            Some(reason) => format!(",\"reason\":\"{}\"", escape_json(reason)),
            None => String::new(),
        };
        self.append_session_event(
            &task.session_id,
            "task_usage_recorded",
            &format!(
                "{{\"taskId\":\"{}\",\"runId\":\"{}\"{}, \"inputTokens\":{},\"outputTokens\":{},\"source\":\"{}\",\"reportedCost\":{}{}}}",
                escape_json(&task.id),
                escape_json(run_id),
                marker,
                usage.input_tokens,
                usage.output_tokens,
                usage.source.as_str(),
                cost,
                reason
            ),
        )
    }
```

- [ ] **Step 4: Add the reader**

```rust
    /// Every task's summed usage, keyed by task id. A task with no usage events
    /// is absent from the map rather than present with a zero — the difference
    /// between "not recorded" and "used nothing", which matters for sessions
    /// written before usage existed.
    ///
    /// Reads **all** events rather than only the active conversation, for the
    /// same reason `dangling_actions` does: a rewind moves the conversation,
    /// but what was billed was billed.
    pub fn read_task_usage(&self, session_id: &str) -> Result<HashMap<String, TaskUsage>> {
        let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
            return Ok(HashMap::new());
        };
        let mut totals: HashMap<String, TaskUsage> = HashMap::new();
        let mut every_run_reported_cost: HashMap<String, bool> = HashMap::new();
        for event in parsed_events(&content).0 {
            if event.event_type != "task_usage_recorded" {
                continue;
            }
            let Some(task_id) = event.text("taskId") else {
                continue;
            };
            let input = event
                .payload
                .get("inputTokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let output = event
                .payload
                .get("outputTokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            // An unparsable or absent source is treated as an estimate. The
            // safe default is the weaker claim.
            let source = event
                .text("source")
                .and_then(|value| UsageSource::parse(&value))
                .unwrap_or(UsageSource::Estimated);
            let cost = event
                .payload
                .get("reportedCost")
                .and_then(|value| value.as_f64());
            let all_costed = every_run_reported_cost.entry(task_id.clone()).or_insert(true);
            *all_costed = *all_costed && cost.is_some();
            let total = totals.entry(task_id).or_insert(TaskUsage {
                input_tokens: 0,
                output_tokens: 0,
                source: UsageSource::Measured,
                reported_cost: None,
                run_count: 0,
            });
            total.input_tokens += input;
            total.output_tokens += output;
            total.run_count += 1;
            if source == UsageSource::Estimated {
                total.source = UsageSource::Estimated;
            }
            total.reported_cost = Some(total.reported_cost.unwrap_or(0.0) + cost.unwrap_or(0.0));
        }
        for (task_id, total) in totals.iter_mut() {
            if !every_run_reported_cost.get(task_id).copied().unwrap_or(false) {
                total.reported_cost = None;
            }
        }
        Ok(totals)
    }
```

- [ ] **Step 5: Export and run the tests**

Add `TaskUsage` to the `pub use session::{…}` list in `lib.rs`.

Run: `cargo test -p workspace-engine --test token_accounting`

Expected: PASS, 5 tests.

- [ ] **Step 6: Run the full quality gate, then commit**

```bash
git add crates/workspace-engine/src crates/workspace-engine/tests/token_accounting.rs
git commit -m "Sum a task's token usage from append-only per-run events"
```

---

### Task 5: Record usage from both model call sites

Nothing writes a usage event yet. This task connects the adapter's figure to the
log, at both places a model is called, and handles the two cases requirement 5
names: a retried call and a stopped one.

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs:957-1007`
- Modify: `crates/workspace-engine/src/edit.rs:310`
- Test: `crates/workspace-engine/tests/token_accounting.rs`

**Interfaces:**
- Consumes: `record_task_usage`/`read_task_usage` (Task 4), `ModelRun.usage` (Tasks 1-2).

- [ ] **Step 1: Write the failing tests**

Append to `token_accounting.rs`, using the `foundation.rs` chat helpers as the
model for driving a turn (`chat_turn_with_adapter` and friends — read that file
rather than inventing a second harness):

```rust
#[test]
fn a_completed_turn_records_one_usage_event_per_model_call() {
    let harness = chat_harness("per-round");
    // Two scripted rounds: a tool call, then the final answer.
    let result = harness.ask_with_rounds(2);
    let usage = harness
        .store
        .read_task_usage(&result.session.id)
        .unwrap();
    assert_eq!(usage[&result.task.id].run_count, 2);
    assert!(usage[&result.task.id].input_tokens > 0);
}

#[test]
fn every_retried_attempt_is_counted() {
    // `retry_count` means the provider was called more than once, and the
    // input was transmitted each time. Counting only the successful attempt
    // would make Damaian look cheaper than it is (proposal §5.5).
    let harness = chat_harness("retries");
    let result = harness.ask_with_transport_failures(2);
    let usage = harness.store.read_task_usage(&result.session.id).unwrap();
    assert_eq!(
        usage[&result.task.id].run_count, 3,
        "one successful attempt plus two that reached the provider"
    );
    assert_eq!(usage[&result.task.id].source, UsageSource::Estimated);
}

#[test]
fn a_turn_stopped_mid_stream_records_what_it_streamed() {
    let harness = chat_harness("stopped-mid");
    let result = harness.ask_and_stop_after_first_token();
    let usage = harness.store.read_task_usage(&result.session.id).unwrap();
    let total = &usage[&result.task.id];
    assert_eq!(total.source, UsageSource::Estimated);
    assert!(
        total.input_tokens > 0,
        "the request was sent and therefore billed"
    );
}

#[test]
fn a_turn_stopped_before_the_call_records_a_measured_zero() {
    let harness = chat_harness("stopped-before");
    let result = harness.ask_with_cancel_already_set();
    let usage = harness.store.read_task_usage(&result.session.id).unwrap();
    let total = &usage[&result.task.id];
    assert_eq!(total.source, UsageSource::Measured);
    assert_eq!(total.input_tokens, 0);
    assert_eq!(total.output_tokens, 0);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p workspace-engine --test token_accounting`

Expected: FAIL — no usage events are written, so `usage[&task.id]` panics on a
missing key.

- [ ] **Step 3: Accumulate this round's streamed output in the chat loop**

Per [`context.md`](context.md) §3.3, a mid-stream stop loses the round's output
because `pump_stream` drops its buffer and the tokens went straight to the sink.
Wrap the sink so the loop keeps its own copy. In `chat.rs`, just before the
`stream_response` call (line 963):

```rust
            // Kept here rather than taken from the run, because a stop returns
            // `Err(Cancelled)` and no run at all — and those tokens were still
            // generated and billed (spec 19 §5.5).
            let mut round_output = String::new();
            let mut accumulate = |token: &str| {
                round_output.push_str(token);
                (sink.on_token)(token);
            };
```

and pass `&mut accumulate` to `stream_response` in place of `&mut *sink.on_token`.
The borrow of `sink.on_token` ends before `finish_cancelled_turn` needs `sink`,
so scope `accumulate` to the call itself if the borrow checker objects.

- [ ] **Step 4: Record usage on each path**

After `self.session_store.finish_action(model_marker, "ok")?;` (line 991):

```rust
            self.session_store.record_task_usage(
                &task,
                &model_run.run_id,
                Some(&model_marker_id),
                model_run.usage,
                model_run.reported_cost,
                None,
            )?;
            // Requirement 5: an attempt that reached the provider was billed
            // even though its answer never arrived. Same input, no output.
            for attempt in 0..model_run.retry_count {
                self.session_store.record_task_usage(
                    &task,
                    &format!("{}_retry{}", model_run.run_id, attempt + 1),
                    Some(&model_marker_id),
                    TokenUsage::estimated(model_run.usage.input_tokens, 0),
                    None,
                    Some("retried_attempt"),
                )?;
            }
```

`model_marker_id` is `model_marker.id.clone()` captured before
`finish_action` consumes the marker.

In the `Err(ClientError::Cancelled)` arm (line 969), before
`finish_cancelled_turn`:

```rust
                        // The request was sent; only the answer was cut short.
                        let cancelled_run = ModelRun::cancelled_before_start(
                            &self.config.model_provider,
                            &self.config.model_name,
                        );
                        self.session_store.record_task_usage(
                            &task,
                            &cancelled_run.run_id,
                            Some(&model_marker_id),
                            TokenUsage::estimated(
                                model_request_json(&request).len().div_ceil(4) as u64,
                                round_output.len().div_ceil(4) as u64,
                            ),
                            None,
                            Some("stopped_mid_stream"),
                        )?;
```

The top-of-loop cancellation (line 900) is the measured-zero case and needs
`record_task_usage` with `TokenUsage::measured_zero()` and no reason — nothing
was sent.

In `edit.rs`, after the `stream_response` at line 310, record the run's usage
the same way with `marker_id: None` — that call site has no action marker.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p workspace-engine --test token_accounting`

Expected: PASS, 9 tests (5 from Task 4, 4 new).

- [ ] **Step 6: Run the full quality gate, then commit**

Watch for existing chat tests that assert on the session log's event count or
sequence — a new event per round moves `seq` numbers. Fix the assertions rather
than the events.

```bash
git add crates/workspace-engine/src crates/workspace-engine/tests
git commit -m "Record what every model call spent, including retries and stopped turns"
```

---

### Task 6: A lost call is counted at recovery

**Read [`context.md`](context.md) §3.4 first.** After a crash the request object
is gone, so the estimate must be written *before* the call or recovery has
nothing but a zero to append.

**Files:**
- Modify: `crates/workspace-engine/src/session.rs` (`start_action`, `DanglingAction`)
- Modify: `crates/workspace-engine/src/chat.rs` (pass the estimate)
- Modify: `crates/workspace-engine/src/recovery.rs`
- Test: `crates/workspace-engine/tests/token_accounting.rs`

**Interfaces:**
- Produces: `SessionStore::start_action_with_estimate(&Task, action, reference, side_effecting, estimated_input_tokens: Option<u64>) -> Result<ActionMarker>`; `DanglingAction.estimated_input_tokens: Option<u64>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_call_lost_to_a_crash_is_counted_at_recovery() {
    let fixture = crash_fixture("lost-call");
    // The signature a kill leaves: the marker is written, the call never
    // finished, and no usage event was ever appended.
    let task = fixture.task_with_model_call_in_flight(4200);
    classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    let usage = fixture.store.read_task_usage(&fixture.session_id).unwrap();
    let total = usage.get(&task.id).expect("a lost call is still billed");
    assert_eq!(total.run_count, 1);
    assert_eq!(total.input_tokens, 4200);
    assert_eq!(total.source, UsageSource::Estimated);
}

#[test]
fn classifying_twice_does_not_bill_the_lost_call_twice() {
    // The sweep runs at every launch. Without a guard, one crash would grow
    // the reported spend every time the app opens.
    let fixture = crash_fixture("idempotent");
    let task = fixture.task_with_model_call_in_flight(4200);
    classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();
    classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    let usage = fixture.store.read_task_usage(&fixture.session_id).unwrap();
    assert_eq!(usage[&task.id].run_count, 1);
}

#[test]
fn a_completed_call_is_not_re_billed_at_recovery() {
    let fixture = crash_fixture("completed");
    let task = fixture.task_with_model_call_completed(4200);
    classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    let usage = fixture.store.read_task_usage(&fixture.session_id).unwrap();
    assert_eq!(usage[&task.id].run_count, 1, "the call already accounted");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p workspace-engine --test token_accounting recovery`

Expected: FAIL — no `estimated_input_tokens` and no append at recovery.

- [ ] **Step 3: Carry the pre-call estimate on the marker**

In `session.rs`, add the optional field to the `action_started` payload and to
`DanglingAction`:

```rust
    /// The input-token estimate for a model call, written before the call so a
    /// crash mid-flight still has a figure to account for (spec 19 §5.5).
    /// `None` for every other action.
    pub estimated_input_tokens: Option<u64>,
```

Keep `start_action`'s signature and add
`start_action_with_estimate`, with `start_action` delegating to it with `None`,
so the eight existing call sites are untouched. In `dangling_actions`, read the
field with `event.payload.get("estimatedInputTokens").and_then(|v| v.as_u64())`.

In `chat.rs`, replace the `start_action` at line 957 with
`start_action_with_estimate(&task, "model_call", &self.config.model_name, false,
Some(model_request_json(&request).len().div_ceil(4) as u64))`.

- [ ] **Step 4: Append the lost call's usage at classification**

In `recovery.rs`'s `classify_session`, after the dangling actions are read and
before the loop over statuses, for each dangling `model_call`:

```rust
    // A model call that started and never finished was billed and its answer
    // is gone. Requirement 5 counts it; §5.5 marks it estimated with a reason
    // so a crash does not silently reduce the reported spend.
    //
    // Guarded by marker id rather than by a flag, because the launch sweep runs
    // on every start and an unguarded append would grow the bill each time.
    let already_accounted = store.usage_marker_ids(session_id)?;
    for action in dangling.iter().filter(|action| action.action == "model_call") {
        if already_accounted.contains(&action.marker_id) {
            continue;
        }
        let Some(estimate) = action.estimated_input_tokens else {
            continue;
        };
        store.record_task_usage_for_task_id(
            session_id,
            &action.task_id,
            &format!("lost_{}", action.marker_id),
            Some(&action.marker_id),
            TokenUsage::estimated(estimate, 0),
            None,
            Some("lost_to_crash"),
        )?;
        audit.record(
            "task_usage_recorded_for_lost_call",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("taskId", action.task_id.clone()),
                ("estimatedInputTokens", estimate.to_string()),
            ],
        )?;
    }
```

This needs two additions to `SessionStore`: `usage_marker_ids(session_id) ->
Result<HashSet<String>>`, reading `markerId` off every `task_usage_recorded`
event, and `record_task_usage_for_task_id`, the by-id sibling of
`record_task_usage` — recovery has a task id, not a `Task`. Add `marker_id` to
`DanglingAction` at the same time; it is parsed already and simply not kept.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p workspace-engine --test token_accounting --test crash_recovery`

Expected: PASS. `crash_recovery.rs` must stay green — the new field is
additive and its tests pass `None`.

- [ ] **Step 6: Run the full quality gate, then commit**

```bash
git add crates/workspace-engine/src crates/workspace-engine/tests
git commit -m "Count the model call a crash interrupted instead of losing its cost"
```

---

### Task 7: The task-state surface

Requirement 2 asks for task state *and* the completion report. The report does
not exist yet — it is spec 23 §5.7, Not started ([`context.md`](context.md)
§3.5) — so this task delivers the surface that does exist, and spec 23 reads
`read_task_usage` when it builds the report.

**Files:**
- Modify: `crates/desktop-shell/src/lib.rs` (`/api/session`, `chat_result_json`)
- Modify: `crates/desktop-shell/static/app.js` (`renderMessages`, a new `markMessageUsage`)
- Test: `crates/desktop-shell/src/lib.rs` tests

**Interfaces:**
- Consumes: `read_task_usage` (Task 4).

- [ ] **Step 1: Write the failing test**

In the desktop-shell test module, following the existing endpoint tests:

```rust
#[test]
fn the_session_payload_carries_each_tasks_usage() {
    let fixture = shell_fixture("session-usage");
    let task = fixture.task_with_usage(1200, 340, UsageSource::Estimated);
    let payload = fixture.get(&format!("/api/session?session_id={}", task.session_id));
    assert!(payload.contains("\"inputTokens\":1200"));
    assert!(payload.contains("\"outputTokens\":340"));
    // The marker requirement 4 turns on: the shell must be able to tell the
    // user this is an approximation.
    assert!(payload.contains("\"usageSource\":\"estimated\""));
    assert!(payload.contains("\"runCount\":1"));
}

#[test]
fn a_task_with_no_usage_reports_none_rather_than_zero() {
    let fixture = shell_fixture("session-no-usage");
    let task = fixture.task_without_usage();
    let payload = fixture.get(&format!("/api/session?session_id={}", task.session_id));
    assert!(!payload.contains("\"inputTokens\""));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p desktop-shell the_session_payload_carries_each_tasks_usage a_task_with_no_usage_reports_none`

Expected: FAIL — the payload has no usage fields.

- [ ] **Step 3: Widen the session payload**

In `lib.rs`'s `GET /api/session` arm (line 445), read usage alongside statuses:

```rust
            let task_usage = engine
                .session_store
                .read_task_usage(&session_id)
                .map_err(|error| error.to_string())?;
```

and change `task_statuses_json(&task_statuses)` to
`task_states_json(&task_statuses, &task_usage)`, a widened version of the
existing function that emits the usage fields only for a task that has an
entry — absent, not zero:

```rust
fn task_states_json(
    statuses: &HashMap<String, String>,
    usage: &HashMap<String, TaskUsage>,
) -> String {
    let mut entries: Vec<&String> = statuses.keys().collect();
    // Sorted so the payload is stable between requests.
    entries.sort();
    entries
        .iter()
        .map(|id| {
            let usage_json = match usage.get(*id) {
                Some(total) => format!(
                    ",\"inputTokens\":{},\"outputTokens\":{},\"usageSource\":\"{}\",\"runCount\":{}{}",
                    total.input_tokens,
                    total.output_tokens,
                    total.source.as_str(),
                    total.run_count,
                    match total.reported_cost {
                        Some(cost) => format!(",\"reportedCost\":{cost}"),
                        None => String::new(),
                    }
                ),
                None => String::new(),
            };
            format!(
                "{{\"id\":\"{}\",\"status\":\"{}\"{}}}",
                escape_json(id),
                escape_json(&statuses[*id]),
                usage_json
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}
```

Do the same for the turn response in `chat_result_json` so a just-finished turn
shows its figure without a reload.

- [ ] **Step 4: Render it**

In `app.js`, add next to `markMessageStopped` (line 3812), reusing the existing
`.turn-indicator` pattern rather than inventing a style:

```javascript
// Per docs/specs/19_token_and_cost_accounting/. An estimated figure is always
// marked: the total is only as trustworthy as its weakest term, so one
// estimated call makes the whole turn's number an approximation.
function markMessageUsage(target, usage) {
  if (!usage || typeof usage.inputTokens !== "number") return;
  const row = document.createElement("p");
  row.className = "turn-indicator";
  row.dataset.state = "usage";
  const label = document.createElement("span");
  label.className = "turn-indicator-label";
  const estimated = usage.usageSource === "estimated";
  const total = usage.inputTokens + usage.outputTokens;
  const calls = usage.runCount === 1 ? "1 model call" : `${usage.runCount} model calls`;
  label.textContent = `${estimated ? "~" : ""}${total.toLocaleString()} tokens · ${calls}${
    estimated ? " (estimated)" : ""
  }`;
  if (typeof usage.reportedCost === "number") {
    label.textContent += ` · ${usage.reportedCost.toFixed(4)}`;
  }
  row.append(label);
  target.body.after(row);
}
```

and call it from `renderMessages` for assistant messages, joining by `taskId`
the same way the cancelled set does:

```javascript
  const usageByTask = new Map(
    tasks.filter((task) => typeof task.inputTokens === "number").map((task) => [task.id, task]),
  );
```

Add `[data-state="usage"]` to the `.turn-indicator` rules in the stylesheet,
using the muted metadata treatment from `docs/UI_STYLE_GUIDE.md` rather than a
new colour.

- [ ] **Step 5: Verify in the running app**

Static assets are `include_str!`-embedded, so a rebuild and restart is required
before the browser sees a change. Rebuild, restart, open a session that has a
completed turn, and confirm the line renders and reads correctly. Never
`pkill -f damaian-desktop-shell` — the user's own app shares the binary name;
kill by PID.

- [ ] **Step 6: Run the tests and the full quality gate**

Run: `cargo test -p desktop-shell` and then all seven gate commands.
`node --check crates/desktop-shell/static/app.js` and `npm run lint:web` are the
two this task can break.

- [ ] **Step 7: Commit**

```bash
git add crates/desktop-shell
git commit -m "Show what each turn spent, marked when the figure is an estimate"
```

---

### Task 8: Cost from user-configured rates

Optional, opt-in, and never attributed to the provider. Without rates, cost is
`None` and nothing is displayed.

**Files:**
- Modify: `crates/workspace-engine/src/config.rs`
- Modify: `crates/workspace-engine/src/session.rs` or the recording sites
- Test: `crates/workspace-engine/src/config.rs`, `crates/workspace-engine/tests/token_accounting.rs`

**Interfaces:**
- Produces: `ModelProviderConfig::{price_per_million_input_tokens, price_per_million_output_tokens}: Option<f64>`; `Config::estimated_cost(&self, usage: &TokenUsage) -> Option<f64>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn no_configured_rates_means_no_cost_figure() {
    let config = Config::default();
    assert_eq!(
        config.estimated_cost(&TokenUsage::estimated(1_000_000, 1_000_000)),
        None
    );
}

#[test]
fn configured_rates_produce_a_cost_from_the_users_own_numbers() {
    let mut config = Config::default();
    config
        .apply_overlay(overlay_from(concat!(
            "model_provider.deepseek.price_per_million_input_tokens=0.27\n",
            "model_provider.deepseek.price_per_million_output_tokens=1.10\n"
        )))
        .unwrap();
    config.model_provider = "deepseek".to_string();
    let cost = config
        .estimated_cost(&TokenUsage::estimated(2_000_000, 1_000_000))
        .expect("configured rates should produce a figure");
    assert!((cost - (0.27 * 2.0 + 1.10)).abs() < 1e-9);
}

#[test]
fn a_computed_cost_is_never_recorded_as_a_provider_reported_one() {
    // §5.6: a figure from the user's own rates is labelled estimated and
    // attributed to them. Writing it into `reportedCost` would launder a guess
    // into a provider fact.
    let (store, task, data_dir) = store_with_task_and_dir("computed-cost");
    store
        .record_task_usage(&task, "modelrun_1", None, TokenUsage::estimated(10, 1), None, None)
        .unwrap();
    let log = read_session_log(&data_dir, &task.session_id);
    assert!(log.contains("\"reportedCost\":null"));
}
```

- [ ] **Step 2: Run to verify they fail, then implement**

Add the two `Option<f64>` fields to `ModelProviderConfig` and its overlay, parse
them in `set_model_provider_config` with a new `parse_price` helper that rejects
a negative or non-numeric value with the same `InvalidInput` shape
`parse_token_count` uses, and add:

```rust
    /// Cost from the user's own configured rates, or `None` when they have set
    /// none. Deliberately not a built-in price table: prices change, and a
    /// stale table reports confident wrong numbers (proposal §4).
    ///
    /// Both rates must be set. One alone would produce a figure that silently
    /// omits half the bill.
    pub fn estimated_cost(&self, usage: &TokenUsage) -> Option<f64> {
        let provider = self.model_provider_config(&self.model_provider)?;
        let input_rate = provider.price_per_million_input_tokens?;
        let output_rate = provider.price_per_million_output_tokens?;
        Some(
            (usage.input_tokens as f64 / 1_000_000.0) * input_rate
                + (usage.output_tokens as f64 / 1_000_000.0) * output_rate,
        )
    }
```

Surface it in the shell as a separate field from `reportedCost` — name it
`estimatedCost` — and render it with "estimated, your rates" rather than as a
bare figure. `reportedCost` stays reserved for what the provider said.

- [ ] **Step 3: Run the tests, the full quality gate, then commit**

```bash
git add crates/workspace-engine crates/desktop-shell
git commit -m "Let a user price their own tokens without shipping a price table"
```

---

### Task 9: The eval harness reads real figures

**Files:**
- Modify: `crates/eval-harness/src/runner.rs:427`
- Modify: `crates/eval-harness/tests/harness.rs`
- Modify: `evals/baseline.json` (regenerated, reviewed before commit)
- Modify: `docs/specs/18_local_evaluation_harness/proposal.md` (§5.6's token row)

**Interfaces:**
- Consumes: `read_task_usage` (Task 4).

- [ ] **Step 1: Write the failing test**

In `crates/eval-harness/tests/harness.rs`:

```rust
#[test]
fn a_deterministic_run_reports_the_token_figures_the_session_recorded() {
    // Spec 18 §5.6's token row was `notApplicable: "spec-19"` until this. The
    // mock provider reports no usage, so the figure is an estimate — and the
    // harness must say so rather than presenting it as measured.
    let run = run_one_scenario("one_file_patch");
    assert!(run.record.tokens.input > 0);
    assert!(run.record.tokens.output > 0);
    assert!(
        !run.record.tokens.measured,
        "the mock adapter reports no usage, so every figure here is an estimate"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p eval-harness a_deterministic_run_reports_the_token_figures`

Expected: FAIL — `tokens.input` is hardcoded to 0 at `runner.rs:427`.

- [ ] **Step 3: Read the recorded usage**

Replace the hardcoded block in `runner.rs`:

```rust
    // Spec 19's `read_task_usage` is the one definition of what a task spent;
    // the harness reads it rather than counting model calls itself, so an eval
    // figure and a real session's figure cannot drift apart.
    let recorded = engine.session_store.read_task_usage(&session.id)?;
    let total = recorded.get(&task.id);
    run_record.tokens = Tokens {
        input: total.map(|usage| usage.input_tokens).unwrap_or(0),
        output: total.map(|usage| usage.output_tokens).unwrap_or(0),
        measured: total
            .map(|usage| usage.source == UsageSource::Measured)
            .unwrap_or(false),
    };
    run_record.cost = total.and_then(|usage| usage.reported_cost);
```

Update the `Tokens.measured` doc comment, which currently says `ModelRun`
carries no usage fields — it does now.

- [ ] **Step 4: Update spec 18**

In `docs/specs/18_local_evaluation_harness/proposal.md` §5.6, the token row says
the figures come from spec 19 and are "Zero and `measured: false` in the
deterministic tier". Correct it: they are now non-zero estimates in the
deterministic tier, and `measured: true` only against a provider that reports
usage. Note in that spec's §7 that the row is no longer `notApplicable`.

- [ ] **Step 5: Regenerate the baseline — and stop for review**

```bash
cargo run -q -p eval-harness --bin damaian-eval -- --tier deterministic --format json > evals/baseline.json
```

**This is a human checkpoint, not a step to click through.** Spec 18 §7 records
that the first generated baseline leaked the seeded credential and was rejected.
Read the new file metric by metric before committing it, and grep it for
`AKIAIOSFODNN7EXAMPLE`. Commit the baseline on its own, after the review.

- [ ] **Step 6: Run the full quality gate, then commit**

```bash
git add crates/eval-harness docs/specs/18_local_evaluation_harness/proposal.md
git commit -m "Report real token figures in the eval harness"
```

```bash
git add evals/baseline.json
git commit -m "Rebaseline the eval metrics now that token figures are measured"
```

---

### Task 10: Documentation and closing the spec

**Files:**
- Modify: `docs/USER_GUIDE.md`
- Modify: `docs/TROUBLESHOOTING.md`
- Modify: `docs/specs/19_token_and_cost_accounting/proposal.md`
- Modify: `docs/specs/README.md`

- [ ] **Step 1: User guide**

Add a section on where to see what a turn used, what "estimated" means and why
a figure may be estimated (no provider report, a stopped turn, a lost call), and
how to set `price_per_million_input_tokens` / `price_per_million_output_tokens`
for a cost figure — stating plainly that the figure is the user's own arithmetic
and not a bill.

- [ ] **Step 2: Troubleshooting**

Add: why usage may be missing for a provider, what the `stream_options` probe
does and what the `model_usage_reporting_unsupported` audit event means, and how
to set `model_provider.<id>.provider_reports_usage=false` permanently.

- [ ] **Step 3: Fill in §7 of the proposal**

The proposal's §7 asks for two things by name, and both are measurements rather
than prose:

1. Which providers were tested, and whether `stream_options` was accepted.
2. **Measured versus estimated counts for the same request**, so the accuracy of
   `len / 4` is known rather than assumed. Run one live-tier scenario against a
   provider that reports usage, put both numbers in §7, and state the error as a
   percentage. If it is badly wrong, say so — a later work package can improve
   the estimate, but nobody should discover the gap by accident.

This needs credentials and a real provider. It is also exactly what spec 18 §7's
open item needs, so do both in one session.

- [ ] **Step 4: Close the spec**

Set the proposal's `Status:` line to `Done` with the measured figures, fill in
this file's progress table with what each task actually found, and update the
row for #19 in `docs/specs/README.md` — including that it is now a folder spec,
and that spec 18's token rows are no longer `notApplicable`.

- [ ] **Step 5: Run the full quality gate, then commit**

```bash
git add docs
git commit -m "Document token accounting and close spec 19"
```

## Self-review

Checked after writing, against [`proposal.md`](proposal.md):

- **Requirement 1** (recorded per task, stored in `SessionStore`) — Tasks 4, 5.
- **Requirement 2** (task state and completion report) — Task 7, with the
  completion report deferred to spec 23 for the reason in `context.md` §3.5.
  **This is the one requirement not fully closed by this plan**, and it is
  closed as far as an existing surface allows.
- **Requirement 3** (feeds the harness) — Task 9.
- **Requirement 4** (estimates labelled, never presented as measured) — Task 1
  (the type), Task 4 (aggregate weakest-term rule), Task 7 (the display rule),
  Task 9 (`measured: false`).
- **Requirement 5** (every billed call, including retries and lost ones) —
  Task 5 (retries, stops), Task 6 (crash).
- **Requirement 6** (never break a provider) — Task 3, config flag plus probe.
- **Requirement 7** (no key, prompt, or file content) — Task 4's on-disk
  assertion; every payload field is a number or an id.
- **§5.7 documentation** — Task 10.
- **§6 acceptance criteria** — every bullet maps to a named test above, except
  the one naming `MockModelTransport::failing`, which `context.md` §3.1 corrects
  to a sequenced body-level rejection.
