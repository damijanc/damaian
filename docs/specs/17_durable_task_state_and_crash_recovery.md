# Feature Spec: Durable Task State and Crash Recovery

Status: Not started
Order: 17 of 19
Roadmap: `docs/ROADMAP/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.5 (model adapter cancellation), section 7.6 (tool
and action orchestrator), section 11 (error handling). Related implementation
specs: [`08_stop_and_progress.md`](08_stop_and_progress.md) (the cancellation and
UI-state work this extends),
[`10_persistent_command_approval.md`](10_persistent_command_approval.md),
[`16_session_checkpoints_and_rewind.md`](16_session_checkpoints_and_rewind.md)
(shares the session event log and the `seq` migration).

## 1. Motivation

`TaskStatus::Running` covers context preparation, the model call, tool
execution, patch application, and validation, indiscriminately
(`crates/workspace-engine/src/session.rs:21-29`). A task killed while `Running`
therefore carries no information about what was in flight — and the question
that matters after a crash is exactly that: did the patch apply? did the command
run? was the model charged for a call whose answer was lost?

The consequence is worse than a poor status string. Without knowing whether an
action completed, there are only two options at restart, and both are wrong:
retry the action, which may apply a patch twice or run `npm publish` a second
time, or drop the task silently, which leaves the user with a half-applied
change and no record. Damaian currently does the second: nothing reconciles
incomplete tasks at launch, so a task that was `Running` when the app died stays
`Running` in the log forever and the UI shows a turn that never finishes.

[Spec 08](08_stop_and_progress.md) made a turn stoppable by the user. This work
package makes a turn survivable when the stop was not the user's idea.

## 2. Current State

- **`TaskStatus` has seven variants**: `Created`, `Running`,
  `WaitingForApproval`, `Failed`, `Complete`, `Cancelled`,
  `ToolBudgetExhausted` (`session.rs:21-29`), with string forms in
  `TaskStatus::as_str` (`session.rs:32-42`).
- **`Task` is a thin record**: id, session id, status, user prompt, provider,
  model, created and completed timestamps (`session.rs:46-56`). Nothing records
  what phase the work reached or what action was in flight.
- **Sessions are one append-only JSONL event log.** `SessionStore`
  (`session.rs:68`) appends `task_created`, `task_status_updated`, and
  `message_appended` events. Nothing is ever rewritten in place.
- **Tasks are replayed, not stored.** `read_task_statuses` (`session.rs:237`)
  scans the log with `line.contains("\"eventType\":\"task_created\"")` and
  `json_string_field`, letting a later event overwrite an earlier one. Its doc
  comment records this design explicitly.
- **Events have no sequence number.** Order is line order, and there is no
  monotonic identifier to reconcile against.
- **Readers are substring-based and torn-line-tolerant only by accident.**
  `read_messages` (`session.rs:219`) and `read_task_statuses` filter lines with
  `contains` and parse fields individually, so a truncated final line is not
  detected as truncated — it is parsed for whatever fields survive.
- **Cancellation exists.** `CancelToken` in
  `crates/workspace-engine/src/cancel.rs`, wired through the chat loop by
  [spec 08](08_stop_and_progress.md).
- **Command proposals already persist.** `CommandStore::save_proposal` and
  `load_proposal` (`crates/workspace-engine/src/validation.rs:49-61`) write and
  read a proposal by id under the data directory. Patch proposals persist under
  `<data_dir>/patches/` (`crates/workspace-engine/src/edit.rs:66`,
  `edit.rs:132`). What is missing is not the proposal — it is any record linking
  an interrupted task to the proposal it was waiting on.
- **Commands cannot orphan a process.** `CommandRunner` runs the configured
  shell with `Command::output()` (`crates/workspace-engine/src/command_runner.rs:89-93`),
  which blocks until exit and reaps the child.
- **Three things can orphan a process**: MCP stdio servers, spawned at
  `crates/workspace-engine/src/mcp.rs:323` with no kill-on-drop guard; the
  `curl` child used for model calls, wrapped in `KillOnDrop`
  (`crates/workspace-engine/src/model.rs:400-406`), which protects a graceful
  drop but not a `SIGKILL`; and PTY sessions held in a process-global map in
  `crates/desktop-shell/src/terminal.rs:31`.
- **The audit log records events with redacted fields**
  (`crates/workspace-engine/src/audit.rs:42`).

## 3. Requirements

1. `TaskStatus` distinguishes `created`, `preparing_context`,
   `waiting_for_model`, `running_tool`, `waiting_for_approval`,
   `applying_patch`, `validating`, `completed`, `failed`, `cancelled`,
   `interrupted`, and `unknown_external_outcome`, and preserves
   `tool_budget_exhausted`.
2. Every consequential action records a durable marker before it starts and
   after it finishes, so a crash between the two is detectable as a specific
   action with an unknown outcome.
3. A partially written event is never readable as a valid state.
4. On launch, every incomplete task is detected and classified.
5. **No command, MCP call, external write, or patch application whose previous
   outcome is unknown is ever automatically repeated.** This is the central
   guarantee.
6. Model-only and read-only work resumes automatically when enough state exists
   to do so safely.
7. The user is offered recovery choices: resume, inspect, mark failed, or
   abandon.
8. A pending approval survives restart with the `CommandProposal` or
   `ProposedPatch` it refers to, reattached to its task.
9. Processes owned by a crashed session are cleaned up. PIDs are tracked at
   spawn and killed by PID — never by process name, per `AGENTS.md`.
10. Existing sessions and configuration migrate. A session written by the
    current version loads after the upgrade with no data loss.
11. Recovery decisions and their outcomes are recorded through
    `AuditLog::record`.

## 4. Non-goals

- Resuming a model call mid-stream. A model call whose stream was cut is a lost
  call; the task resumes by making a new one, and the cost of the lost call is
  reported by [spec 19](19_token_and_cost_accounting.md), not hidden.
- Undoing what a crashed action did. That is rewind
  ([spec 16](16_session_checkpoints_and_rewind.md)); this spec establishes what
  happened so the user can decide.
- Detecting whether an *external* side effect landed — whether a `docker push`
  reached the registry, or an MCP call mutated a remote system. Damaian records
  that the outcome is unknown and stops. Probing external systems to find out is
  out of scope and, for most tools, not possible.
- Crash reporting, telemetry, or automatic issue creation.
- Recovering from a corrupted data directory. That is schema-version handling in
  [spec 15](15_install_and_update_verification.md).
- A live progress display of the new states. [Spec 08](08_stop_and_progress.md)
  owns the progress UI; this spec feeds it more precise states and adds only the
  recovery prompt.
- Background or long-running processes as a feature — Phase 2 WP5.

## 5. Design

### 5.1 State machine

| State | Meaning | Crash here means |
|---|---|---|
| `created` | Task recorded, nothing started | Nothing happened. Resume freely |
| `preparing_context` | Indexing, retrieval, context assembly | Read-only. Resume freely |
| `waiting_for_model` | Request sent, awaiting or streaming a response | A call may have been billed. Resume with a new call |
| `running_tool` | A tool or command is executing | **Unknown outcome.** Never auto-retry |
| `waiting_for_approval` | Awaiting the user's decision | Safe. Reattach the proposal |
| `applying_patch` | Writing files to disk | **Unknown outcome.** Never auto-retry |
| `validating` | Running validation commands | **Unknown outcome** if the command is not known read-only |
| `completed` / `failed` / `cancelled` / `tool_budget_exhausted` | Terminal | Nothing to do |
| `interrupted` | Crash in a state with no side effect in flight | Offer resume |
| `unknown_external_outcome` | Crash with a side-effecting action in flight | Offer inspect. Never auto-retry |

`interrupted` and `unknown_external_outcome` are assigned at recovery time, not
during normal operation. They are the classifier's output.

`validating` splits on what is being validated: a validation command that
`CommandPolicy` classifies as sandbox-safe read-only is resumable, and anything
else is not. Reusing the existing classification means the resume rule and the
approval rule cannot drift apart.

### 5.2 Durability: append, do not rewrite

The roadmap prescribes write-then-rename. **That is the wrong mechanism here and
must not be used.** The session log is a single append-only file whose readers
replay it (`session.rs:237`), and [spec 16](16_session_checkpoints_and_rewind.md)
appends rewind markers to the same log. Rewriting it to update state would
destroy the audit trail, break replay, and turn every status update into a
whole-file rewrite whose own crash window is far larger than the one it closes.

The atomicity unit for an append log is one event line:

- Each event is serialised, terminated with `\n`, and written with a single
  `write` to a file opened in append mode. Order is arrival order.
- Every event carries a monotonic `seq`, unique within the session.
  [Spec 16](16_session_checkpoints_and_rewind.md) needs the same field; the two
  specs share one migration (§5.6).
- A crash can leave a torn final line. Requirement 3 is met on the read side:
  every line must parse as a complete JSON object and carry a `seq`, and a line
  that does not is discarded with a `session_log_truncated_tail` audit event. A
  torn line is always the last one, so discarding it loses at most the event
  that was being written when the process died — which is precisely the event
  whose action has an unknown outcome, and is recovered from the preceding
  `_started` marker.
- This replaces the current `line.contains(...)` filtering in `read_messages`
  and `read_task_statuses`, which cannot distinguish a torn line from a valid
  one. Parse first, then match on the parsed `eventType`.

### 5.3 Before-and-after markers

Requirement 2 in an append log is a pair of events:

```json
{"seq":412,"eventType":"action_started","taskId":"task_…",
 "action":"apply_patch","ref":"patch_…","sideEffecting":true}
{"seq":413,"eventType":"action_finished","taskId":"task_…",
 "action":"apply_patch","ref":"patch_…","outcome":"ok"}
```

A dangling `action_started` with no matching `action_finished` is the signal the
classifier needs, and `sideEffecting` on the start event is what decides between
`interrupted` and `unknown_external_outcome` — recorded when the action begins,
so the decision does not depend on re-deriving the action's nature after a
crash, when the code path that knew it is gone.

Actions that get markers: model call, tool call, command execution, MCP call,
patch application, and each validation command.

### 5.4 Recovery at launch

For each session, replay the log and, for every task whose latest status is not
terminal:

1. If there is a dangling `action_started` with `sideEffecting: true` →
   `unknown_external_outcome`.
2. If there is a dangling `action_started` with `sideEffecting: false`, or a
   non-terminal status with no dangling action → `interrupted`.
3. If the latest status is `waiting_for_approval` → keep it, and reattach the
   proposal (§5.5).

Classification appends a `task_recovered` event with the classification and the
evidence — the dangling action and its `seq` — so a recovery decision is
auditable rather than a conclusion the UI reached once and forgot.

Auto-resume is permitted **only** for `interrupted` tasks whose dangling action,
if any, was `preparing_context` or a read-only model call, per requirement 6. In
every other case the user chooses:

| Choice | Effect |
|---|---|
| Resume | Continue from the last known-good point. Offered only when safe |
| Inspect | Show the task, its dangling action, the files it may have touched, and its checkpoint |
| Mark failed | Terminal `failed`, with a note that the outcome was unknown |
| Abandon | Terminal `cancelled`. The task is closed and the turn is not retried |

The recovery prompt names the specific action — "a patch application was in
progress and its outcome is unknown" — not a generic "session interrupted".
Requirement 5 is a promise about what Damaian will not do on its own; the prompt
is how the user learns what they are deciding about.

`Inspect` links to the task's checkpoint from
[spec 16](16_session_checkpoints_and_rewind.md), which is the mechanism for
actually recovering: the checkpoint says what the files looked like before the
action, so the user can compare and rewind if they choose.

### 5.5 Pending approvals

Proposals already persist (`validation.rs:49`, `edit.rs:66`). What is missing is
the link. Add the proposal reference to the `task_status_updated` event that
sets `waiting_for_approval`:

```json
{"seq":300,"eventType":"task_status_updated","id":"task_…",
 "status":"waiting_for_approval",
 "pendingApproval":{"kind":"command","proposalId":"cmdprop_…"}}
```

On recovery, load the proposal by id through the existing store. A proposal file
that is missing or fails to deserialise makes the task `failed` with a clear
reason — never an approval card reconstructed from partial data, since the user
would be approving a command Damaian is guessing at.

Re-presenting an approval card after restart does not re-approve anything. A
`command_allowlist` entry written by `Allow Always`
([spec 10](10_persistent_command_approval.md)) still applies, because it is
repository config rather than task state.

### 5.6 Migration

One migration, shared with [spec 16](16_session_checkpoints_and_rewind.md):

- **`seq`**: existing events have none. On read, events without `seq` are
  numbered by line order — which is their append order, so the numbering is
  correct rather than merely consistent. No file is rewritten.
- **Statuses**: the seven existing string forms all survive as-is. `running`
  becomes a legacy value that maps to `interrupted` at recovery time, since a
  `running` task in a log written before this change carries exactly the
  information this work package exists to eliminate: something was in flight and
  nothing recorded what.
- **`Task`**: no new required fields, so existing records deserialise unchanged.

Requirement 10 is verified by a test that loads a session log fixture captured
from the current version.

### 5.7 Orphaned processes

Requirement 9 needs re-aiming: **commands cannot orphan a process.**
`CommandRunner` uses `Command::output()` (`command_runner.rs:89-93`), which
blocks until the child exits and reaps it. A crash during a command leaves the
child parented to `launchd`, but Damaian never held a PID to kill and the
command's outcome is unknown regardless — that is `unknown_external_outcome`,
not a cleanup problem.

The three real sources are:

| Source | Current state | Change |
|---|---|---|
| MCP stdio servers (`mcp.rs:323`) | Spawned, no kill-on-drop | Record PID in a session-scoped process registry file at spawn; kill by PID at recovery |
| `curl` for model calls (`model.rs:400`) | `KillOnDrop` — safe on graceful drop, orphaned on `SIGKILL` | Same registry, same recovery sweep |
| PTY sessions (`terminal.rs:31`) | Process-global map, lost on crash | Same registry |

The registry is a file under the data directory holding PID, spawn time, and the
owning session, written at spawn and removed on clean exit. At launch, a
recorded PID is killed **only** when it is still alive and its start time
matches what was recorded — a PID is reused, and killing a stranger's process
because Damaian crashed is a worse bug than the leak. This is the mechanical
form of the `AGENTS.md` rule against matching by name.

### 5.8 Documentation

`docs/USER_GUIDE.md`: what happens after a crash, what the recovery choices
mean, and why Damaian will not retry an action on its own.
`docs/TROUBLESHOOTING.md`: how to read the recovery events in a session log, how
to find a task's dangling action, what `unknown_external_outcome` means, and
where the process registry lives.

## 6. Acceptance Criteria

- Killing the application in each of the twelve states leaves a task that
  classifies correctly on restart, asserted by a test per state.
- A task killed during `applying_patch` or `running_tool` reports
  `unknown_external_outcome`, offers inspection, and is never automatically
  retried.
- A task killed during `preparing_context` resumes automatically.
- A torn final line in a session log is discarded, the rest of the log replays
  correctly, and the truncation is audited.
- A pending approval survives restart with its proposal reattached, and a
  missing or corrupt proposal file fails the task with a clear reason instead of
  reconstructing a card.
- The recovery prompt names the specific in-flight action.
- Sessions written before this change load with no data loss, and a legacy
  `running` task classifies as `interrupted`.
- A recorded PID whose start time no longer matches is not killed — asserted by
  test.
- No MCP server, `curl`, or PTY process recorded by a crashed session is still
  running after recovery.
- Recovery classifications and user decisions appear in the audit log with their
  evidence.
- The five quality-gate commands from `AGENTS.md` pass.

## 7. Implementation Notes

To be completed during implementation.

The twelve-state kill matrix is the load-bearing test and the one most likely to
be quietly reduced to "a few representative states". Record which states were
exercised by an automated failure-injection test and which, if any, were only
checked by hand — per `AGENTS.md`, anything that spawns a real shell or kills a
real process is `#[ignore]`d with instructions, so some of this matrix will be
manual by design. Say which.
