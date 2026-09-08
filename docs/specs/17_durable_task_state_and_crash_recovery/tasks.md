# Durable Task State and Recovery Classification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Started:** not yet

**Goal:** Make a crash classifiable. After this lands, every task interrupted by a crash is
recorded as a specific action with a known or unknown outcome, and nothing whose outcome is
unknown can be automatically repeated — enforced in the engine, not by a prompt.

**Architecture:** The session log stays append-only. Each consequential action brackets itself
with `action_started` / `action_finished` events carrying `sideEffecting`, so a dangling start
is the crash signature. At launch a classifier replays each log, assigns `interrupted` or
`unknown_external_outcome` to non-terminal tasks, records the evidence, and decides whether
auto-resume is permitted. Three operations — resume, mark failed, abandon — act on that
decision, and `resume` refuses a task the classifier did not mark safe.

**Tech Stack:** Rust 2024, `workspace-engine` only. `serde_json` is already a dependency and
replaces the hand-rolled substring JSON extraction on the read path. No new dependencies, no
UI, no new quality-gate command.

## Global Constraints

Every task's requirements implicitly include this section.

- **The central guarantee (requirement 5):** no command, MCP call, external write, or patch
  application whose previous outcome is unknown is ever automatically repeated. Where a choice
  exists, refuse rather than retry.
- **Enforcement lives in the engine.** `resume` on an unsafe task returns an error. A caller
  cannot widen this, because spec 45 will be a webview and a guarantee enforced only in a
  webview is not enforced.
- **Append, never rewrite.** The log is the audit trail and spec 16 replays it. No task in this
  plan rewrites a session log. Status changes are new events.
- **No UI.** Every acceptance criterion here is testable headlessly. The prompt is
  [spec 45](../45_crash_recovery_prompt.md).
- **`#[ignore]` anything with real side effects**, with a doc comment saying how to run it —
  per `AGENTS.md`. Some of the kill matrix is manual by design; say which.
- **Every quality-gate command from `AGENTS.md` passes** at the end of every task. Read that
  file for the list rather than relying on a remembered one.
- **Clippy warnings are errors.** `#[allow(...)]` needs a comment saying why.
- Commit messages: one subject line, no body, no trailer.

## Two findings from the pre-implementation survey

Verified against the code, not assumed. Both change what this plan does.

**1. §5.6's `seq` migration is already done — do not rebuild it.** Spec 16 shipped it on both
sides: `append_session_event` writes `seq` on every append (`session.rs:332`), and
`numbered_events` (`session.rs:400`) falls back to line order for events written before the
field existed, which is their append order. Task 1 verifies this and moves on.

**2. Appending is O(session length), and this spec multiplies appends.** `append_session_event`
calls `latest_event_seq`, which reads and scans the **entire log file** to compute the next
`seq` (`session.rs:299-304`, `:332`). Today that is tolerable. This spec adds two events per
action across six action types, so a long session's cost grows quadratically. Task 2 fixes it
before Task 5 makes it bite — deliberately ordered that way, because discovering it when the
kill matrix runs slowly would be a much worse place to find out.

## Progress

Update this table as tasks land, so anyone picking the work up mid-flight knows where it
stopped without reading the git log.

| Task | State | Notes |
|---|---|---|
| 1 · Parse-first session log reads | Done | `SessionEvent` + `parse_event`/`parsed_events` replace `line.contains` and the five substring helpers (now deleted). 3 tests, gate green at **382**; spec 16's 5 rewind tests and spec 18's 12 scenarios both still pass. **§5.6's `seq` migration confirmed already done** (written at `:332`, line-order fallback at `numbered_events`) — not rebuilt. **Found a real defect, not just a spec gap:** the old reader did not merely mishandle a torn line, it *fabricated records from one* — `read_messages` returned a phantom third message (`got ["first", "second", "torn"]`). Note the first version of that test passed against the old code by accident, because the substring reader drops the final character and my torn line happened to end in the `createdAtMs` digit; the tail now ends after a sacrificial field so the test actually distinguishes the two readers. **Audit deferred by design** (plan Step 6 sanctioned this): `SessionStore` has no `AuditLog` and threading one through its 15 construction sites — 13 of them tests, each then needing a `SecretScanner` — is heavy churn for a diagnostic, so `unreadable_event_count` exposes the number and Task 6's classifier audits it |
| 2 · Cache the append sequence | Done | `last_seq: Arc<Mutex<HashMap<..>>>` + `next_seq`; 2 assertions and 1 `#[ignore]`d timing probe, gate green at **384 passed, 6 ignored**. **Measured, not assumed** — per-append cost before: 2.3ms at 500 events, 4.4ms at 1000, 8.7ms at 2000 (17.5s for that batch); after: 0.067 / 0.063 / 0.065ms (129ms for 2000). **135× at 2000 events, and per-append is now flat rather than doubling** — the change is linear-vs-quadratic, not just faster. **`Arc` turned out to be mandatory, not stylistic:** `SessionStore` derives `Clone` and is cloned into *both* the chat and edit orchestrators (`workspace_engine.rs:97`, `:112`), so per-clone caches would each hand out the same next `seq`. Mutation-tested by giving `Clone` a fresh cache: `got duplicates in [1, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7]` — and rewind resolves by `seq`, so that log would rewind to the wrong place. Note the plan's own test (two independently constructed stores) would **not** have caught this, because a cache miss correctly falls back to the file; the clone test was added after reading how the store is actually cloned |
| 3 · Twelve task states | Done | 13 variants (12 live + the classifier's two), `all()`/`parse`/`is_terminal`/`may_have_side_effect_in_flight`; 5 tests, gate green at **389**. `Running` removed; `parse("running")` maps to `Interrupted` per §5.6. Call sites were **6, not the plan's 23** — the earlier count included every `TaskStatus::` mention rather than only `::Running`. All six became `PreparingContext`, each **verified rather than defaulted**. **Corrected the plan's own advice:** it said "where unclear, `PreparingContext` is the safe default: it is read-only, so a crash there is resumable" — but *resumable* is the permissive direction, and mislabelling an in-flight command as read-only would let the classifier auto-resume it, violating requirement 5. The safe default is whichever state *blocks* auto-resume. `chat.rs:698` turned out to be genuinely read-only (the approved command has already run — its result is the `tool` message appended just above), established by reading the surrounding code. UI boundary checked: `app.js` branches only on `cancelled` and `tool_budget_exhausted`, both preserved; the `"running"` seen near `setChatStatus` is a CSS class, not a status. The two `"running"` literals in `desktop-shell` tests are a stringly-typed `CheckpointConversation.task_status` and now double as legacy-value coverage. The four finer in-flight states exist but are not yet *reached* — Task 5 wires them at the action sites |
| 4 · Action markers | Done | `ActionMarker`, `DanglingAction`, `start_action`/`finish_action`/`dangling_actions`; **5 tests** (plan said 3), gate green at **393**. No `Drop` impl, deliberately — an automatic finish-on-drop would erase the crash signal — and `finish_action` consumes the marker so a forgotten finish reads as an unused binding. Paired by `markerId`, not action name, with a test running the same action twice. **One design decision the plan did not settle:** `dangling_actions` reads **all** events, not `active_events`. A rewind moves the *conversation* back, but whether an action completed is a fact about the world — filtering by active events let a rewind conceal a dangling `rm -rf build`, mutation-tested at `left: 0` vs `right: 1`. That is a requirement-5 hole, so the choice has its own test. Also dropped `side_effecting` from `ActionMarker` after clippy flagged it unread: the flag is already durable on the `action_started` event, and a second in-memory copy is state that can disagree with the log |
| 5 · Instrument the six action sites | Done | All six instrumented; 2 tests added, gate green at **395**. Bracketed at the dispatch layer per the design correction above — `tool_action_marker` derives name/reference/`sideEffecting` in one place, and all three clean-stop `break` exits finish with `awaiting_approval`/`awaiting_review` so stopping for a human is never read as an unknown outcome. Model call is `sideEffecting: false` (§4: a cut stream is a lost call, so a dangling model marker classifying as `interrupted` is correct). **Patch application resolved via option (a)**, chosen after measuring the cost: `ProposedPatch` gained `session_id`, the stored format went `V1` → `V2`, and the reader accepts both — no conversion, no file rewritten, and only 2 patch files existed on disk. `read_field` is name-checked, so a version mismatch fails closed rather than silently reading the next field. `create_patch` was left alone (19 callers, 17 of them tests) because `PatchEngine` has no business knowing about sessions; the two orchestrators that own one set it before saving. **The conflict path finishes its marker** — a conflict is detected in `prepare_files` before any write, so its outcome *is* known, and leaving it dangling would report a false unknown outcome on the most common failure there is; any other apply error is left dangling on purpose, since `apply_patch` writes files one at a time. Verified in real logs: `preserve_user_modified` (a deliberate conflict) shows `apply_patch started=1 finished=1`, and a passing run shows `model_call`/`propose_patch`/`read_file` all balanced. Option (a) also unblocks Task 7 — §5.5's pending-patch reattachment needs the same `session_id`. |
| 6 · Recovery classifier | Done | New `recovery.rs`: `RecoveredTask`, `classify_session`, `classify_all`; **10 tests** (plan said 6), gate green at **405**. Implements §5.4's three rules and closes Task 1's deferred audit — `session_log_truncated_tail` is now recorded here, by the caller that actually cares. **Two safety guards the plan did not specify, both mutation-tested.** (1) *Status backstop:* a task left in `running_tool` with **no marker at all** is classified `Interrupted` by rule 2, and rule 2 alone would auto-resume it — but absence of a marker is not evidence of safety, it is absence of evidence. Removing the guard makes `a_side_effecting_status_with_no_marker_is_still_not_auto_resumed` fail. (2) *Legacy `running`:* §5.6 maps it to `Interrupted`, and `Interrupted` normally permits auto-resume — but a legacy `running` task carries exactly the information this spec exists to eliminate, so resumable cannot be concluded from it. Removing that guard fails its test too. Both would have been silent requirement-5 holes. Also: an **unrecognised** status string is treated as non-terminal and not auto-resumable, since a status this version does not understand is precisely where guessing is unsafe. Note `waiting_for_approval` is deliberately *not* a recovered task (§5.4 rule 3) — Task 7 reattaches its proposal instead. |
| 7 · Pending approval reattach | Done | `PendingApprovalRef`, `SessionStore::await_approval` / `pending_approval_for`, `recovery::reattach_pending_approvals`; **4 tests** (plan said 3), gate green at **409**. Both sites that set `WaitingForApproval` now record *which* proposal (`chat.rs` for a command or patch, `edit.rs` for a patch). A proposal that is missing, corrupt, or has no recorded link marks the task **terminal `failed` with a stated reason** and audits `pending_approval_unavailable` — never a card rebuilt from whatever happened to parse, per §5.5. **Task 5's option (a) paid off here as predicted:** the patch reattachment needs `ProposedPatch.session_id`, so option (b) would have hit the same wall one task later. Two details worth keeping: `PendingApproval` was already taken by `checkpoint.rs`, and `Ref` is the better name anyway since this is a pointer to a stored proposal rather than the approval itself; and `pending_approval_for` lets a *later* status event without a link **clear** an earlier one, because a stale link would reattach a proposal the user has already decided about. Added a fourth test for the no-recorded-link case — a task whose approval predates this change has nothing to reattach and nothing to guess from, so it fails too |
| 8 · Recovery operations | Not started | |
| 9 · Legacy migration | Not started | |
| 10 · Twelve-state kill matrix | Not started | |
| 11 · Unblock spec 18's resume scenario | Not started | |
| 12 · Docs and spec closure | Not started | |

Record the workspace test count as each task lands. The baseline at the start of this plan is
**379 passed, 0 failed, 5 ignored**. Treat the count as a tripwire: if it moves by an amount
you did not intend, find out why before continuing.

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/workspace-engine/src/session.rs` | Session log: append, replay, task status | Heavily modified — parse-first reads, seq cache, new states, markers |
| `crates/workspace-engine/src/recovery.rs` | **New.** Classifier and the three recovery operations | Created in Task 6 |
| `crates/workspace-engine/src/chat.rs` | Turn orchestration | Marker instrumentation, finer statuses |
| `crates/workspace-engine/src/edit.rs` | Patch proposal and apply | Markers around apply |
| `crates/workspace-engine/src/validation.rs` | Stored command execution | Markers around execution |
| `crates/workspace-engine/src/mcp.rs` | MCP tool calls | Markers around calls |
| `crates/workspace-engine/src/lib.rs` | Public surface | Export recovery types |
| `crates/workspace-engine/tests/crash_recovery.rs` | **New.** The twelve-state kill matrix | Created in Task 10 |

`recovery.rs` is a new module rather than more of `session.rs` because classification is a pure
function of a replayed log and deserves its own test surface. `session.rs` is already 500+
lines and owns storage; recovery is policy over it.

## Interface reference

Verified against the current code. Read once before Task 1.

```rust
// crates/workspace-engine/src/session.rs
pub enum TaskStatus { Created, Running, WaitingForApproval, Failed, Complete,
                      Cancelled, ToolBudgetExhausted }          // :20  — seven today
impl TaskStatus { pub fn as_str(&self) -> &'static str }        // :32

pub struct Task { pub id, session_id: String, pub status: TaskStatus,
                  pub user_prompt, model_provider, model_name: String,
                  pub created_at_ms: u128, pub completed_at_ms: Option<u128> }  // :46

impl SessionStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self                              // :73
    pub fn create_task(&self, …) -> Result<Task>                                // :93
    pub fn update_task_status(&self, task: &Task, status: TaskStatus,
                              error: Option<&str>) -> Result<Task>              // :114
    pub fn append_message(&self, …) -> Result<ChatMessage>                      // :143
    pub fn read_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>>   // :219
    pub fn read_task_statuses(&self, session_id: &str)
        -> Result<HashMap<String, String>>                                      // :239
    pub fn latest_event_seq(&self, session_id: &str) -> Result<u64>             // :299
    pub fn rewind_conversation(&self, session_id: &str, through: u64)-> Result<()> // :310
    fn append_session_event(&self, session_id, event_type, payload) -> Result<()>  // :322
}

// Private helpers the read path uses today — Task 1 replaces the substring ones.
fn numbered_events(content: &str) -> impl Iterator<Item = (u64, &str)>   // :400
fn event_seq(line: &str) -> Option<u64>                                  // :407
fn active_events(content: &str) -> Vec<&str>                             // :422
fn json_string_field(raw: &str, field: &str) -> Option<String>           // :472
fn json_number_field(raw: &str, field: &str) -> Option<u128>             // :489
fn json_bool_field(raw: &str, field: &str) -> Option<bool>               // :499
```

**The event wire shape**, from `append_session_event`:

```json
{"eventId":"evt_…","seq":412,"timestampMs":1788…,"eventType":"task_status_updated",
 "payload":{ … }}
```

Note the payload is **nested**, but `read_task_statuses` reads `id` and `status` by substring
across the whole line — it reaches into the payload without descending into it. That is the
fragility Task 1 removes.

**`TaskStatus::` call sites outside `session.rs`** — 23 in total, all of which must still
compile after Task 3: `chat.rs` 8, `edit.rs` 4, `cancel.rs` 1, `tests/foundation.rs` 10.

**Status strings crossing into the UI:** `app.js` matches `"cancelled"` (`:3168`),
`"tool_budget_exhausted"` (`:3171`, `:3509`, `:4857`). The `"running"` argument to
`setChatStatus` is a CSS class, **not** a `TaskStatus` — do not rename it chasing this change.

---

### Task 1: Parse-first session log reads

The riskiest change in the plan, done first and alone so its blast radius is visible. It
changes how every session log is read, and spec 16's rewind tests plus spec 18's twelve
scenarios are the regression guard.

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`
- Modify: `crates/workspace-engine/tests/session_rewind.rs` (add the torn-line test)

**Interfaces:**
- Produces: `fn parse_event(line: &str) -> Option<SessionEvent>` where
  `SessionEvent { seq: u64, event_type: String, payload: serde_json::Value }`.
  Every read path goes through it.

- [ ] **Step 1: Verify the `seq` migration is already done, before assuming it**

```bash
grep -n 'seq' crates/workspace-engine/src/session.rs | head -20
```

Expected: `append_session_event` writes `"seq":{}` (`:336`) and `numbered_events` (`:400`)
falls back to line order. If both hold, §5.6's `seq` work is complete and this plan does not
touch it. Record the finding in the Progress table.

- [ ] **Step 2: Write the failing test**

Add to `crates/workspace-engine/tests/session_rewind.rs`:

```rust
/// Requirement 3: a partially written event is never readable as a valid state.
/// A crash can only tear the final line, so discarding it loses exactly the
/// event whose action has an unknown outcome — which the preceding
/// `action_started` marker already records.
#[test]
fn a_torn_final_line_is_discarded_and_the_rest_replays() {
    let (store, session) = session_with_messages(&["first", "second"]);

    // Simulate a crash mid-write: append a truncated JSON object with no newline.
    let path = session_log_path(&store, &session.id);
    let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    use std::io::Write;
    write!(file, "{{\"eventId\":\"evt_x\",\"seq\":99,\"eventType\":\"message_app").unwrap();
    drop(file);

    let messages = store.read_messages(&session.id).expect("log should still read");
    assert_eq!(
        messages.len(),
        2,
        "the two complete messages must survive a torn tail"
    );
}

/// A line that is valid JSON but carries no `seq` is still readable — events
/// written before spec 16 added the field are numbered by line order.
#[test]
fn an_event_without_a_seq_is_still_readable() {
    let (store, session) = session_with_messages(&["only"]);
    let path = session_log_path(&store, &session.id);
    let content = std::fs::read_to_string(&path).unwrap();
    let stripped: String = content
        .lines()
        .map(|line| line.replacen(&format!("\"seq\":{},", extract_seq(line)), "", 1))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, format!("{stripped}\n")).unwrap();

    assert_eq!(
        store.read_messages(&session.id).expect("read").len(),
        1,
        "a pre-seq event must still replay"
    );
}
```

The helpers `session_with_messages`, `session_log_path` and `extract_seq` do not exist yet —
write them at the top of the test file from the existing tests' setup, which already builds a
`SessionStore` over a temp dir.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p workspace-engine --locked --test session_rewind`
Expected: FAIL. The torn-line case is the one that matters — today `line.contains(…)` matches
the substring inside the torn line and `parse_message_event` returns `None`, so it may
*accidentally* pass. **If it passes, do not move on**: change the torn tail to one that
`contains` matches and parsing does not, e.g. cut the line after `"eventType":"message_appended"`,
so the test genuinely distinguishes the two implementations.

- [ ] **Step 4: Add the parsed event type**

In `session.rs`:

```rust
/// One parsed log line. Every read path goes through this rather than matching
/// substrings, so a torn line is structurally unreadable rather than a
/// coincidence of what text happens to be present.
struct SessionEvent {
    seq: u64,
    event_type: String,
    payload: serde_json::Value,
}

/// `None` for a line that is not a complete JSON object with an `eventType`.
/// A torn final line is the expected cause; `seq` falls back to the caller's
/// line ordering for events written before spec 16 added the field.
fn parse_event(line: &str, fallback_seq: u64) -> Option<SessionEvent> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let object = value.as_object()?;
    let event_type = object.get("eventType")?.as_str()?.to_string();
    let seq = object
        .get("seq")
        .and_then(|seq| seq.as_u64())
        .unwrap_or(fallback_seq);
    let payload = object
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Some(SessionEvent { seq, event_type, payload })
}
```

- [ ] **Step 5: Route every read through it**

Rewrite `numbered_events`, `active_events`, `read_messages`, `read_task_statuses`,
`browser_diagnostics_allowed_for_session`, `parse_session_log` and `parse_session_event` to
parse first and match on `event_type`. `read_task_statuses` reads `id` and `status` from the
**payload**, not from the whole line:

```rust
    for event in active_events(&content) {
        if event.event_type != "task_created" && event.event_type != "task_status_updated" {
            continue;
        }
        // `task_status_updated` may wrap the task as {"task":…,"error":…}, so
        // look in both shapes rather than assuming one.
        let task = event.payload.get("task").unwrap_or(&event.payload);
        if let (Some(id), Some(status)) = (
            task.get("id").and_then(|value| value.as_str()),
            task.get("status").and_then(|value| value.as_str()),
        ) {
            statuses.insert(id.to_string(), status.to_string());
        }
    }
```

Delete `json_string_field`, `json_nullable_string_field`, `json_number_field`,
`json_bool_field` and `parse_json_string_at` once nothing calls them. If something outside
`session.rs` calls them, stop and report it rather than making them public.

- [ ] **Step 6: Audit the discard**

When a line fails to parse, record it once per read:

```rust
// Audited rather than silent: a discarded line means a crash happened here, and
// the operator needs to be able to see that in the log rather than infer it.
self.audit_log.record(
    "session_log_truncated_tail",
    &[("actor", "system".to_string()),
      ("sessionId", session_id.to_string()),
      ("discardedLines", discarded.to_string())],
)?;
```

`SessionStore` has no `AuditLog` today. Adding one changes `SessionStore::new`'s signature and
every construction site (`workspace_engine.rs:82` and the tests). **If that ripple is larger
than it looks, stop and report it** — an acceptable alternative is for the classifier in Task 6
to do the auditing, since it is the caller that cares. Decide with the real call-site count in
front of you, and record which you chose.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p workspace-engine --locked --test session_rewind`
Expected: PASS, including the 5 pre-existing rewind tests.

- [ ] **Step 8: Run the regression guards explicitly**

```bash
cargo test --workspace --locked
```

```bash
cargo run -p eval-harness -- run --tier deterministic
```

Expected: 379 + 2 new tests, 0 failed; and the eval tier still reports 12 passing scenarios.
**The eval tier is the guard that matters here** — it reads real session logs end to end, which
no unit test does.

- [ ] **Step 9: Run the full quality gate**

Every command in `AGENTS.md`'s `## Quality gate`.

---

### Task 2: Cache the append sequence

Fixes the quadratic append found in the survey, before Task 5 multiplies the number of appends.

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`

**Interfaces:**
- Consumes: Task 1's `parse_event`.
- Produces: no public API change. `SessionStore` gains an internal per-session seq cache.

- [ ] **Step 1: Measure first, so the fix has a number attached**

Write an `#[ignore]`d benchmark-style test that appends 2,000 events to one session and prints
the elapsed time. Run it and record the figure. Without a before number, the after number
means nothing.

```bash
cargo test -p workspace-engine --locked -- --ignored append_cost
```

- [ ] **Step 2: Write the failing test**

```rust
/// The next `seq` must not require re-reading the whole log. Asserted through
/// behaviour rather than timing: two stores over the same directory must still
/// agree, so the cache cannot be a stale per-process guess.
#[test]
fn a_second_store_over_the_same_session_continues_the_sequence() {
    let dir = temp_dir("seq-cache");
    let first = SessionStore::new(&dir);
    let session = first.create_session("repo", "title").unwrap();
    first.append_message(&session.id, None, "user", "one").unwrap();

    let second = SessionStore::new(&dir);
    second.append_message(&session.id, None, "user", "two").unwrap();

    let seqs = recorded_seqs(&dir, &session.id);
    assert!(
        seqs.windows(2).all(|pair| pair[1] > pair[0]),
        "sequence numbers must stay strictly increasing across stores, got {seqs:?}"
    );
}
```

- [ ] **Step 3: Implement the cache**

A `Mutex<HashMap<String, u64>>` on `SessionStore`, populated lazily from `latest_event_seq` on
first append for a session and incremented in memory thereafter. **A cache miss must fall back
to reading the file**, which is what makes the test above pass with two stores.

`SessionStore` derives `Clone` and is cloned into several orchestrators — the cache must be
shared, so it is an `Arc<Mutex<…>>`, not a plain field. Check how `SessionStore` is cloned in
`workspace_engine.rs` before choosing.

- [ ] **Step 4: Run the tests, then re-measure**

Run the ignored benchmark again and record the new figure next to the old one in the Progress
table. If it did not improve materially, the cache is not on the path you thought — investigate
rather than keeping a change that bought nothing.

- [ ] **Step 5: Full quality gate**

---

### Task 3: Twelve task states

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`
- Modify: `crates/workspace-engine/src/chat.rs`, `edit.rs`, `cancel.rs`
- Modify: `crates/workspace-engine/tests/foundation.rs`

**Interfaces:**
- Produces:

```rust
pub enum TaskStatus {
    Created, PreparingContext, WaitingForModel, RunningTool, WaitingForApproval,
    ApplyingPatch, Validating, Complete, Failed, Cancelled, ToolBudgetExhausted,
    Interrupted, UnknownExternalOutcome,
}
impl TaskStatus {
    pub fn as_str(&self) -> &'static str;
    pub fn parse(value: &str) -> Option<Self>;   // legacy "running" maps in Task 9
    pub fn is_terminal(self) -> bool;
    /// Whether a crash in this state may have left a side effect in flight.
    /// `Validating` is the interesting one — see the note below.
    pub fn may_have_side_effect_in_flight(self) -> bool;
}
```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn every_state_round_trips_through_its_string_form() {
    for status in TaskStatus::all() {
        assert_eq!(
            TaskStatus::parse(status.as_str()),
            Some(status),
            "{} must round-trip",
            status.as_str()
        );
    }
}

/// §5.1's table, encoded. A crash in these states may have left something
/// half-done; a crash in the others cannot have.
#[test]
fn the_states_that_can_leave_a_side_effect_in_flight_are_exactly_these() {
    let flagged: Vec<&str> = TaskStatus::all()
        .into_iter()
        .filter(|status| status.may_have_side_effect_in_flight())
        .map(|status| status.as_str())
        .collect();
    assert_eq!(flagged, vec!["running_tool", "applying_patch", "validating"]);
}

#[test]
fn terminal_states_are_exactly_these() {
    let terminal: Vec<&str> = TaskStatus::all()
        .into_iter()
        .filter(|status| status.is_terminal())
        .map(|status| status.as_str())
        .collect();
    assert_eq!(
        terminal,
        vec!["complete", "failed", "cancelled", "tool_budget_exhausted"]
    );
}
```

`TaskStatus::all()` is a new associated function returning every variant — needed so these
tests enumerate rather than list, which is what makes a forgotten variant fail.

- [ ] **Step 2: Run to verify failure, then implement**

Extend the enum and `as_str`. String forms are exactly §5.1's: `created`, `preparing_context`,
`waiting_for_model`, `running_tool`, `waiting_for_approval`, `applying_patch`, `validating`,
`complete`, `failed`, `cancelled`, `tool_budget_exhausted`, `interrupted`,
`unknown_external_outcome`.

Note `Complete` serialises as `"complete"`, not `"completed"` — match the existing value
(`session.rs:38`) or every stored session breaks.

`Validating` is flagged as possibly-side-effecting even though §5.1 says it "splits on what is
being validated". The split needs the *command*, which the status alone does not carry, so the
conservative answer is encoded here and the classifier refines it in Task 6 using the dangling
marker's `sideEffecting` flag. Erring toward "may have a side effect" is the safe direction for
requirement 5.

- [ ] **Step 3: Update the 23 call sites**

`chat.rs` 8, `edit.rs` 4, `cancel.rs` 1, `tests/foundation.rs` 10. Most are
`TaskStatus::Running`, which now needs a more specific state — use the one matching what the
code is about to do. Where genuinely unclear, `PreparingContext` is the safe default: it is
read-only, so a crash there is resumable.

- [ ] **Step 4: Run the full suite, then the eval tier**

Expected: all pass. The eval tier's `resume_interrupted_session` is still blocked, so it still
reports skipped.

- [ ] **Step 5: Full quality gate**

---

### Task 4: Action markers

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`

**Interfaces:**
- Produces:

```rust
pub struct ActionMarker { /* session_id, task_id, action, ref, side_effecting, seq */ }
impl SessionStore {
    /// Records `action_started` and returns a marker that MUST be finished.
    pub fn start_action(&self, task: &Task, action: &str, reference: &str,
                        side_effecting: bool) -> Result<ActionMarker>;
    pub fn finish_action(&self, marker: ActionMarker, outcome: &str) -> Result<()>;
}
```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_finished_action_leaves_no_dangling_marker() {
    let (store, task) = store_with_task();
    let marker = store.start_action(&task, "apply_patch", "patch_1", true).unwrap();
    store.finish_action(marker, "ok").unwrap();

    assert!(store.dangling_actions(&task.session_id).unwrap().is_empty());
}

/// The crash signature: a start with no matching finish.
#[test]
fn an_unfinished_action_is_reported_as_dangling_with_its_side_effect_flag() {
    let (store, task) = store_with_task();
    let marker = store.start_action(&task, "apply_patch", "patch_1", true).unwrap();
    std::mem::forget(marker); // stand in for the process dying here

    let dangling = store.dangling_actions(&task.session_id).unwrap();
    assert_eq!(dangling.len(), 1);
    assert_eq!(dangling[0].action, "apply_patch");
    assert!(
        dangling[0].side_effecting,
        "sideEffecting is recorded at start, so the classifier does not have to \
         re-derive the action's nature after the code that knew it is gone"
    );
}

/// A second action started after the first finished must not resurrect it.
#[test]
fn only_the_unfinished_action_is_dangling() {
    let (store, task) = store_with_task();
    let first = store.start_action(&task, "read_file", "src/x.rs", false).unwrap();
    store.finish_action(first, "ok").unwrap();
    let second = store.start_action(&task, "run_command", "cmd_1", true).unwrap();
    std::mem::forget(second);

    let dangling = store.dangling_actions(&task.session_id).unwrap();
    assert_eq!(dangling.len(), 1);
    assert_eq!(dangling[0].action, "run_command");
}
```

- [ ] **Step 2: Implement, matching each finish to its start**

Pair them by a marker id written on both events, not by action name — the same action can run
twice in a turn, and matching by name would let the second start cancel out the first.

Do **not** implement `Drop` on `ActionMarker` to auto-finish. A dropped marker is exactly the
crash case this spec exists to detect, and an automatic "finished" on drop would erase the
signal. `finish_action` consumes the marker so forgetting it is visible in review as a marker
that goes out of scope unused.

- [ ] **Step 3: Run, then full quality gate**

---

### Task 5: Instrument the six action sites

**Files — corrected from the plan's original list, see below:**
- Modify: `chat.rs` (model call, and every `ToolAction` arm: tool, command, MCP, validation)
- Modify: `edit.rs` (patch application)
- **Not** `validation.rs` or `mcp.rs`

**Interfaces:**
- Consumes: Task 4's `start_action` / `finish_action`.

**Design correction, made after reading the call sites.** The plan named
`validation.rs` and `mcp.rs` as instrumentation points. Neither
`ValidationOrchestrator` (`validation.rs:118`) nor `McpClient` (`mcp.rs:133`)
holds a `SessionStore`, and neither receives a `Task` — so instrumenting inside
them means threading session state through two modules that have nothing to do
with session state, purely to record a marker.

All of command execution, MCP calls and validation are *dispatched from*
`chat.rs`'s `ToolAction` match, and `run_agentic_turn` (`chat.rs:850`) takes
`mut task: Task` — so the task is in scope at every arm. Bracketing there covers
four of the six action types with no change to those modules.

**The trade-off, stated rather than hidden.** A marker at the dispatch layer
brackets "the engine asked for this action" rather than "the leaf process
started", so the window is slightly *wider* — it includes proposal and setup
work. That is the conservative direction: a crash in the wider window reports an
unknown outcome for an action that might not have started yet. Erring toward
"unknown" is correct for requirement 5; erring the other way — a narrow window
that misses a real side effect — is the bug this spec exists to prevent.

- [ ] **Step 1: Write the failing test**

One test per action type asserting a completed action leaves no dangling marker, and that the
`sideEffecting` flag matches §5.3's list: model call `false`, read-only tool `false`, command
`true`, MCP call `true`, patch apply `true`, validation command `true` unless
`CommandPolicy` classifies it sandbox-safe read-only.

Reuse `CommandPolicy::classify` for the validation case rather than a second list — §5.1: "the
resume rule and the approval rule cannot drift apart."

- [ ] **Step 2: Instrument each site, then run the eval tier**

```bash
cargo run -p eval-harness -- run --tier deterministic --format json
```

The twelve scenarios drive all six action types. Check `modelCalls` and the trace event counts
have not changed shape, and that runtime has not regressed materially from **2.8s** — Task 2's
cache is what should keep it flat, and this is where you find out whether it did.

- [ ] **Step 3: Full quality gate**

---

### Task 6: Recovery classifier

**Files:**
- Create: `crates/workspace-engine/src/recovery.rs`
- Modify: `crates/workspace-engine/src/lib.rs`

**Interfaces:**
- Produces:

```rust
pub struct RecoveredTask {
    pub task_id: String,
    pub session_id: String,
    pub classification: TaskStatus,        // Interrupted | UnknownExternalOutcome
    pub dangling: Option<DanglingAction>,  // action, ref, side_effecting, seq
    pub auto_resume_permitted: bool,
}
pub fn classify_session(store: &SessionStore, session_id: &str)
    -> Result<Vec<RecoveredTask>>;
pub fn classify_all(store: &SessionStore) -> Result<Vec<RecoveredTask>>;
```

- [ ] **Step 1: Write the failing tests — §5.4's three rules**

```rust
#[test]
fn a_dangling_side_effecting_action_classifies_as_unknown_external_outcome() { … }

#[test]
fn a_dangling_read_only_action_classifies_as_interrupted() { … }

#[test]
fn a_non_terminal_task_with_no_dangling_action_classifies_as_interrupted() { … }

#[test]
fn a_terminal_task_is_not_recovered_at_all() { … }

/// Requirement 6, and the boundary that matters most: auto-resume is permitted
/// for read-only work and nothing else.
#[test]
fn auto_resume_is_permitted_only_for_read_only_dangling_actions() {
    // preparing_context  → permitted
    // a model call       → permitted
    // run_command        → NOT permitted
    // apply_patch        → NOT permitted
}

/// The classification is evidence, not a verdict to be taken on trust.
#[test]
fn classification_records_the_dangling_action_and_its_seq() { … }
```

- [ ] **Step 2: Implement, and append `task_recovered`**

The event carries the classification, the dangling action and its `seq`. Recording it makes a
recovery decision auditable rather than a conclusion something reached once and forgot.

- [ ] **Step 3: Run, then full quality gate**

---

### Task 7: Pending approval reattach

**Files:**
- Modify: `session.rs` (the `pendingApproval` field on the status event), `recovery.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_pending_approval_survives_restart_with_its_proposal() { … }

/// §5.5: never an approval card reconstructed from partial data — the user
/// would be approving a command Damaian is guessing at.
#[test]
fn a_missing_proposal_file_fails_the_task_with_a_reason() { … }

#[test]
fn a_corrupt_proposal_file_fails_the_task_rather_than_reconstructing_a_card() { … }
```

- [ ] **Step 2: Implement, then full quality gate**

---

### Task 8: Recovery operations

**Files:**
- Modify: `crates/workspace-engine/src/recovery.rs`

**Interfaces:**
- Produces: `resume(store, &RecoveredTask)`, `mark_failed(...)`, `abandon(...)`.

- [ ] **Step 1: Write the failing tests — the central guarantee**

```rust
/// Requirement 5, enforced where it cannot be widened. Spec 45 will be a
/// webview; a guarantee that lives only there is not a guarantee.
#[test]
fn resume_is_refused_for_a_task_whose_outcome_is_unknown() {
    let recovered = /* classified UnknownExternalOutcome */;
    let error = recovery::resume(&store, &recovered)
        .expect_err("resuming an unknown-outcome task must be refused");
    assert!(format!("{error:?}").contains("unknown"));
}

#[test]
fn resume_is_permitted_for_an_interrupted_read_only_task() { … }

#[test]
fn mark_failed_is_terminal_and_notes_the_unknown_outcome() { … }

#[test]
fn abandon_is_terminal_and_does_not_retry_the_turn() { … }

/// Requirement 10.
#[test]
fn every_recovery_decision_is_audited_with_its_evidence() { … }
```

- [ ] **Step 2: Implement, then full quality gate**

---

### Task 9: Legacy migration

**Files:**
- Create: `crates/workspace-engine/tests/fixtures/legacy_session.jsonl`
- Modify: `session.rs` (`TaskStatus::parse` legacy mapping), `recovery.rs`

- [ ] **Step 1: Capture a real fixture, do not hand-write one**

Generate a session log with the **current released** code path and commit it verbatim. A
hand-written fixture tests your idea of the old format; a captured one tests the old format.

- [ ] **Step 2: Write the failing tests**

```rust
#[test]
fn a_session_written_before_this_change_loads_with_no_data_loss() { … }

/// §5.6: a legacy `running` task carries exactly the information this work
/// package exists to eliminate — something was in flight and nothing recorded
/// what.
#[test]
fn a_legacy_running_task_classifies_as_interrupted() { … }
```

- [ ] **Step 3: Implement, then full quality gate**

---

### Task 10: Twelve-state kill matrix

§7 names this as the load-bearing test and the one most likely to be quietly reduced to "a few
representative states". Do not reduce it. Where a state cannot be reached by an automated
failure injection, say so explicitly rather than dropping the row.

**Files:**
- Create: `crates/workspace-engine/tests/crash_recovery.rs`

- [ ] **Step 1: Build the injection harness**

Rather than killing a real process, construct a session log ending in each state — with and
without a dangling marker — and run the classifier over it. That covers every state
deterministically and in-process.

A real `SIGKILL` test is a *different* test: it proves the log on disk is readable after an
actual kill, which the constructed logs assume. Write one, `#[ignore]` it with instructions
per `AGENTS.md`, and say in the Progress table that it is manual.

- [ ] **Step 2: One assertion per state**

A table-driven test over all thirteen `TaskStatus` variants, asserting the classification and
`auto_resume_permitted` for each. Use `TaskStatus::all()` so a state added later fails this
test until it is classified — that is the mechanism that stops the matrix quietly shrinking.

- [ ] **Step 3: Full quality gate, and record which states are automated**

---

### Task 11: Unblock spec 18's resume scenario

The concrete proof this spec works, and the reason it was sequenced ahead of specs 45 and 46.

**Files:**
- Modify: `crates/eval-harness/scenarios/resume_interrupted_session.toml`
- Modify: `crates/eval-harness/tests/harness.rs`

- [ ] **Step 1: Remove `blocked_on` and the comment explaining it**

- [ ] **Step 2: Make the scenario pass**

Its assertions are written for this world: `command_executed = false` and
`files_changed_outside_patch = 0`. The scenario may need turns that reach a killable state —
work out what it needs from the classifier's API rather than weakening the assertions.

- [ ] **Step 3: Update the counts the harness pins**

`twelve_scenarios_run_and_exactly_one_is_blocked` now expects **thirteen** running and **zero**
blocked. `the_deterministic_tier_runs_every_scenario_and_passes` expects zero `notApplicable`
records. `metrics.rs`' `recovery_success` row stops being `notApplicable: "spec-17"` and
becomes a real value — update it and its test.

- [ ] **Step 4: Regenerate and re-review the baseline**

`evals/baseline.json` changes: `recovery_success` gains a value and a scenario moves out of
skipped. Per §5.7 of spec 18, that is a **human review gate** — present the diff and wait.

- [ ] **Step 5: Full quality gate**

---

### Task 12: Docs and spec closure

**Files:**
- Modify: `docs/TROUBLESHOOTING.md`, `AGENTS.md`, `proposal.md`, `tasks.md`,
  `docs/specs/README.md`

- [ ] **Step 1: `docs/TROUBLESHOOTING.md`**

How to read recovery events in a session log, how to find a task's dangling action, and what
`unknown_external_outcome` means. **Not** `docs/USER_GUIDE.md` — that is spec 45's, and writing
it here would document a screen that does not exist.

- [ ] **Step 2: Close the spec**

`Status: Done` with what was measured. Fill §7: which of the twelve states are covered by
automated injection and which are manual; the append-cost figures before and after Task 2; and
whether Task 1's audit ripple was taken in `SessionStore` or deferred to the classifier.

- [ ] **Step 3: Update `docs/specs/README.md`'s row 17, and note that 45 and 46 are now unblocked**

- [ ] **Step 4: Final full quality gate**

---

## Self-review

Checked against `proposal.md` after writing.

**Requirement coverage.** (1) Task 3; (2) Tasks 4-5; (3) Task 1; (4) Task 6; (5) Task 8, which
is where it is *enforced* rather than merely respected; (6) Task 6's auto-resume rule; (7) Task
8; (8) Task 7; (9) Task 9; (10) Tasks 6 and 8. §5.6's `seq` half is verified in Task 1 Step 1
rather than built, per the survey.

**Ordering rationale.** Task 1 is first because it is the riskiest and everything after it
appends events that must be readable. Task 2 precedes Task 5 because Task 5 multiplies appends
and would otherwise mask a regression as "the harness got slower". Task 11 is late because it
is the end-to-end proof, and needs 6, 7 and 8 to exist.

**Three places this plan expects to be wrong**, each with a stop-and-report instruction rather
than a guess: Task 1 Step 6's `AuditLog` ripple through `SessionStore::new`; Task 2 Step 3's
`Arc` question, which depends on how `SessionStore` is actually cloned; and Task 11 Step 2,
where the scenario may need reshaping once the classifier's API is real. Spec 18's
implementation corrected something in eight of fifteen tasks — the same rate here would be
unsurprising, and the tasks most likely to need it are the ones touching code this plan read
but did not run.

**Deliberately not specified.** Tasks 6 through 10 give test names and interfaces rather than
full implementations. Their shape depends on what Tasks 1 to 5 actually produce, and spec 18
showed that code written against an unread interface is the main source of plan defects. The
interfaces above are verified; the bodies are for the implementer to write against real types.
