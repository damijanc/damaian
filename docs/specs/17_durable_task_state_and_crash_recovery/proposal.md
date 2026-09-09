# Feature Spec: Durable Task State and Recovery Classification

Status: Done
Order: 17 of 19
Roadmap: `docs/ROADMAP/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Also in this spec: [`context.md`](context.md) (motivation and current state),
[`tasks.md`](tasks.md) (execution order and progress).
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.5 (model adapter cancellation), section 7.6 (tool
and action orchestrator), section 11 (error handling). Related implementation
specs: [`../08_stop_and_progress.md`](../08_stop_and_progress.md) (the
cancellation and UI-state work this extends),
[`../10_persistent_command_approval.md`](../10_persistent_command_approval.md),
[`../16_session_checkpoints_and_rewind.md`](../16_session_checkpoints_and_rewind.md)
(shares the session event log and the `seq` field),
[`../45_crash_recovery_prompt.md`](../45_crash_recovery_prompt.md) and
[`../46_process_registry_and_orphan_sweep.md`](../46_process_registry_and_orphan_sweep.md)
(split out of this spec — see §0).

## 0. Scope: this spec is the engine core

Spec 17 as originally written covered three separable areas. It was split before
implementation so the risk surface stays legible and the load-bearing part — the
guarantee that nothing with an unknown outcome is ever auto-retried — lands
first and can be tested headlessly.

| Area | Where it lives now |
|---|---|
| State machine, durable reads, action markers, recovery **classification**, migration, pending-approval reattach, audit | **This spec** |
| The recovery prompt and its four user choices in the desktop shell | [`../45_crash_recovery_prompt.md`](../45_crash_recovery_prompt.md) |
| Process registry for MCP stdio, `curl` and PTY, and the orphan sweep | [`../46_process_registry_and_orphan_sweep.md`](../46_process_registry_and_orphan_sweep.md) |

The split is along a real seam. Classification is a pure function of the session
log; the prompt is presentation over its output; the registry shares no code with
either and touches three unrelated subsystems. This spec defines the
classification *and* the operations a recovery decision performs
(resume, mark failed, abandon), so spec 45 adds a surface over an API that
already works and is already tested.

**Already satisfied when this spec was written.** §5.6's `seq` migration shipped
with [spec 16](../16_session_checkpoints_and_rewind.md): `numbered_events`,
`event_seq` and `latest_seq` in `session.rs` already number events without a
`seq` by line order, which is their append order. Verify before implementing;
do not rebuild it.

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
7. Each recovery decision is available as an engine operation — resume, mark
   failed, abandon — and the classification records which of them is safe for a
   given task. Presenting them is
   [spec 45](../45_crash_recovery_prompt.md); *deciding* what may be offered is
   this spec, because that decision is the central guarantee in requirement 5
   and must not live in a webview.
8. A pending approval survives restart with the `CommandProposal` or
   `ProposedPatch` it refers to, reattached to its task.
9. Existing sessions and configuration migrate. A session written by the
   current version loads after the upgrade with no data loss.
10. Recovery classifications and decisions, and their outcomes, are recorded
    through `AuditLog::record`.

Moved out of this spec: the recovery prompt and the `Inspect` view
([spec 45](../45_crash_recovery_prompt.md)), and cleaning up processes owned by
a crashed session ([spec 46](../46_process_registry_and_orphan_sweep.md)).

## 4. Non-goals

- Resuming a model call mid-stream. A model call whose stream was cut is a lost
  call; the task resumes by making a new one, and the cost of the lost call is
  reported by [spec 19](../19_token_and_cost_accounting.md), not hidden.
- Undoing what a crashed action did. That is rewind
  ([spec 16](../16_session_checkpoints_and_rewind.md)); this spec establishes what
  happened so the user can decide.
- Detecting whether an *external* side effect landed — whether a `docker push`
  reached the registry, or an MCP call mutated a remote system. Damaian records
  that the outcome is unknown and stops. Probing external systems to find out is
  out of scope and, for most tools, not possible.
- Crash reporting, telemetry, or automatic issue creation.
- Recovering from a corrupted data directory. That is schema-version handling in
  [spec 15](../15_install_and_update_verification.md).
- A live progress display of the new states. [Spec 08](../08_stop_and_progress.md)
  owns the progress UI; this spec feeds it more precise states and adds no UI of
  its own.
- Any user interface at all. The recovery prompt, the four choices and the
  `Inspect` view are [spec 45](../45_crash_recovery_prompt.md). This spec is
  engine-only and every acceptance criterion below is testable headlessly.
- Cleaning up processes owned by a crashed session —
  [spec 46](../46_process_registry_and_orphan_sweep.md).
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
replay it (`session.rs:237`), and [spec 16](../16_session_checkpoints_and_rewind.md)
appends rewind markers to the same log. Rewriting it to update state would
destroy the audit trail, break replay, and turn every status update into a
whole-file rewrite whose own crash window is far larger than the one it closes.

The atomicity unit for an append log is one event line:

- Each event is serialised, terminated with `\n`, and written with a single
  `write` to a file opened in append mode. Order is arrival order.
- Every event carries a monotonic `seq`, unique within the session.
  [Spec 16](../16_session_checkpoints_and_rewind.md) needs the same field; the two
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
if any, was `preparing_context` or a read-only model call, per requirement 6.

**Whether auto-resume is permitted is decided here, not by the caller.** The
classification carries the answer, so a UI cannot widen it by asking for a
resume that was never safe. Requirement 5 is a promise about what Damaian will
not do on its own, and a promise enforced only by a webview is not enforced.

In every other case a decision is required. Three of the four are engine
operations this spec provides:

| Operation | Effect | Owner |
|---|---|---|
| Resume | Continue from the last known-good point. Refused unless the classification marked it safe | This spec |
| Mark failed | Terminal `failed`, with a note that the outcome was unknown | This spec |
| Abandon | Terminal `cancelled`. The task is closed and the turn is not retried | This spec |
| Inspect | Show the task, its dangling action, the files it may have touched, and its checkpoint | [Spec 45](../45_crash_recovery_prompt.md) |

`Inspect` is presentation over data this spec already exposes: the
classification names the dangling action, and the task's checkpoint from
[spec 16](../16_session_checkpoints_and_rewind.md) says what the files looked
like before it. Spec 45 renders that and names the specific action rather than a
generic "session interrupted".

The recovery API is therefore a classification plus three operations, each of
which is testable headlessly. Spec 45 adds a surface over something that already
works.

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
([spec 10](../10_persistent_command_approval.md)) still applies, because it is
repository config rather than task state.

### 5.6 Migration

One migration, shared with [spec 16](../16_session_checkpoints_and_rewind.md):

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

### 5.7 Orphaned processes — moved to spec 46

Requirement 9 and this section moved wholesale to
[spec 46](../46_process_registry_and_orphan_sweep.md). It shares no code with
the state machine, touches three unrelated subsystems (`mcp.rs`, `model.rs`,
`terminal.rs`), and its correctness rests on a different question — whether a
recorded PID still belongs to the process that was recorded.

One conclusion from the original analysis belongs *here*, because it changes
what this spec must classify: **commands cannot orphan a process.**
`CommandRunner` uses `Command::output()` (`command_runner.rs:89-93`), which
blocks until the child exits and reaps it. A crash during a command leaves the
child parented to `launchd`, but Damaian never held a PID to kill and the
command's outcome is unknown regardless. That is `unknown_external_outcome` —
this spec's job — not a cleanup problem.


### 5.8 Documentation

`docs/TROUBLESHOOTING.md`: how to read the recovery events in a session log, how
to find a task's dangling action, and what `unknown_external_outcome` means.

The user-facing explanation — what happens after a crash, what the recovery
choices mean, and why Damaian will not retry an action on its own — belongs with
the prompt that presents them, in
[spec 45](../45_crash_recovery_prompt.md)'s `docs/USER_GUIDE.md` work. Writing it
here would document a screen that does not exist yet.

## 6. Acceptance Criteria

- Killing the application in each of the twelve states leaves a task that
  classifies correctly on restart, asserted by a test per state.
- A task killed during `applying_patch` or `running_tool` reports
  `unknown_external_outcome`, is marked not safe to auto-resume, and a `resume`
  call against it is **refused** rather than merely un-offered.
- A task killed during `preparing_context` resumes automatically.
- A torn final line in a session log is discarded, the rest of the log replays
  correctly, and the truncation is audited.
- A pending approval survives restart with its proposal reattached, and a
  missing or corrupt proposal file fails the task with a clear reason instead of
  reconstructing a card.
- The classification names the specific in-flight action and its `seq`, so
  [spec 45](../45_crash_recovery_prompt.md) can render "a patch application was
  in progress" without re-deriving it.
- Sessions written before this change load with no data loss, and a legacy
  `running` task classifies as `interrupted`.
- Spec 16's rewind tests still pass after the read path changes from
  `line.contains` to parse-first. They are the regression guard for a change
  that touches how every session log is read.
- The twelve-scenario deterministic tier from
  [spec 18](../18_local_evaluation_harness/proposal.md) still passes, and its
  thirteenth scenario (`resume_interrupted_session`) can have its
  `blocked_on = "spec-17"` removed and then passes.
- Recovery classifications and decisions appear in the audit log with their
  evidence.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

Written after implementation. Per-task detail is in
[`tasks.md`](tasks.md)'s progress table; this is what the spec asked to have
recorded.

### The kill matrix: every state automated, one manual test

**All thirteen states are covered by automated failure injection** — the twelve
of §5.1 plus `unknown_external_outcome`, which §5.1 lists as a state even
though it is assigned at recovery time. Each is crossed with the three shapes a
crash can leave on disk (no marker, a read-only marker, a side-effecting
marker), giving **39 cells, none of them manual**. Every cell asserts the
classification, whether auto-resume is permitted, and whether a human may
resume — the last because §5.4 and requirement 5 answer different questions and
conflating them would be a mistake in either direction.

Two rows cannot be produced from `TaskStatus::all()` and are covered by their
own tests rather than dropped:

- The legacy `running` string. Only the no-marker shape is reachable for it,
  because action markers did not exist in the version that wrote it.
- A status string from a *later* version. Treated as neither finished nor safe.

**One test is manual by design**, `#[ignore]`d per `AGENTS.md`:
`a_real_sigkill_mid_action_leaves_a_readable_log_and_an_unknown_outcome` spawns
a child process, waits until its action marker is genuinely on disk, then kills
it by PID and asserts the death was `SIGKILL` rather than a clean exit. It
proves what all 39 constructed cells assume and cannot check: that after a real
kill the log is *fully parsable* and the dangling marker survives, not just the
status. The division is deliberate — that test establishes the signature is
real, and the matrix establishes what is done about it.

The matrix earned its place by finding a hole rather than confirming the code.
The auto-resume gate had three conditions and none covered a task whose
*stored* status is already `unknown_external_outcome`. Nothing writes that
today, but `TaskStatus::parse` accepts it, so the first thing to persist a
classification — spec 45 keeping a recovery list across a second crash — would
have made a status that says "the outcome is unknown" re-derive as resumable
whenever its marker was out of reach. It is now a fourth condition.

### Append cost, measured before and after (§5.2, Task 2)

`append_session_event` computed the next `seq` by rescanning the whole log.
This spec adds two events per action across six action types, so the cost was
quadratic in session length. Per-append, before and after the `Arc<Mutex<..>>`
sequence cache:

| Session length | Before | After |
|---|---|---|
| 500 events | 2.3 ms | 0.067 ms |
| 1000 events | 4.4 ms | 0.063 ms |
| 2000 events | 8.7 ms | 0.065 ms |

**17.5 s to append 2000 events became 129 ms — 135×.** The shape matters more
than the factor: per-append cost was *doubling* with length and is now flat, so
this was linear-versus-quadratic rather than a constant-factor win.

`Arc` turned out to be mandatory, not stylistic. `SessionStore` derives `Clone`
and is cloned into both the chat and edit orchestrators, so a per-clone cache
has each clone handing out the same next `seq` — mutation-tested, producing
`[1, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7]`. Since [spec
16](../16_session_checkpoints_and_rewind.md) resolves a rewind by `seq`, that
log would have rewound to the wrong place.

### Task 1's audit ripple: deferred to the classifier, deliberately

`SessionStore` has no `AuditLog`, and threading one through it would touch 15
construction sites — 13 of them tests, each then also needing a
`SecretScanner`. That is heavy churn for a diagnostic, and it would put an
audit dependency inside the storage layer.

So the discarded-line *count* is exposed by
`SessionStore::unreadable_event_count`, and `recovery::classify_session` records
it as `session_log_truncated_tail`. The caller that cares does the auditing.
This is better than the original plan rather than merely cheaper: a torn tail is
only meaningful as evidence of the crash being classified, and the classifier is
where that context exists.

### What spec 17 deliberately leaves uncalled

Requirement 4 says every incomplete task is detected and classified on launch.
`recovery::classify_all` **is** that sweep, and it is tested — but nothing on a
launch path calls it, because per §0 the desktop surface belongs to
[spec 45](../45_crash_recovery_prompt.md). The engine side is complete and
exercised end to end by spec 18's `resume_interrupted_session`; what is missing
is a caller.

Two consequences, stated so they are not discovered as bugs:

- **A crashed task keeps its last-written status until spec 45 lands.** Markers
  are written from now on, so today's crashes are already classifiable when
  that surface arrives — nothing has to be back-filled.
- **The stale-approval migration has not happened yet either.** The 24
  `waiting_for_approval` tasks described below get failed when
  `reattach_pending_approvals` is first invoked, not on the next launch.

`docs/TROUBLESHOOTING.md` says this too, in the section on reading recovery
events, so an operator does not go looking for a sweep that has not run.

### Two things worth knowing about the upgrade

Verified against 30 real session logs, structure only:

- **777 of 828 real events carry no `seq`.** The line-order fallback is the
  majority path, not an edge case.
- **`task_status_updated` really does appear in two shapes** — flat, and wrapped
  as `{"task":…,"error":…}` by `await_approval`. Both are current, and any
  reader of that event must handle both.

Of 74 real tasks, 25 were non-terminal at their latest status: 24
`waiting_for_approval` and one `running`. The 24 are marked `failed` with a
stated reason on first launch, because no earlier version recorded *which*
proposal a task was waiting on and §5.5 forbids rebuilding an approval card
from partial data. A `task_id` fallback was considered and rejected:
`CommandProposal` carries no task id, neither store can enumerate, and
decisively, failing a stale approval *task* destroys nothing — all 46 stored
patches remain on disk and stay applicable.
