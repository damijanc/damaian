# Feature Spec: Bounded Worker Continuation and Within-Turn Compaction

Status: Not started
Order: 103 of 106
Plan: none. This is a capability gap exposed by the 2026-10-31 single-worker
attempt and `docs/PLAN/OBSERVATIONS.md` observation 42.
Depends on: [#17](17_durable_task_state_and_crash_recovery/proposal.md)
(durable task state), [#47](47_agent_working_capability/proposal.md) (agent
loop), [#55](55_conversation_compaction/proposal.md) (between-turn
compaction), [#59](59_exploration_rounds/proposal.md) (round classes),
[#101](101_closed_task_completion.md) (truthful completion), and
[#102](102_repair_provenance_and_stagnation.md) (progress stops) — #17, #47,
#55, #59, #101 and #102 are built. Everything else named below is a
cross-reference, not a prerequisite.

## 1. Motivation

The default worker segment permits sixteen action rounds and twenty-four
exploration rounds. Measured delegated specification tasks used 64–150 combined
rounds. A worker therefore cannot normally finish such a task without generic
**Continue** decisions.

Raising hard caps alone is unsafe. Every model call resends the growing
within-turn tool history, so cost grows faster than the round count and the
request reaches the provider's context window before a large cap becomes
useful. #55 explicitly left within-turn tool-result compaction out of scope.

## 2. Requirements

1. A plan review for a worker may include an explicit task execution budget:
   maximum action rounds, exploration rounds and model input/output tokens.
   The user sees and approves the values with the plan.
2. The approved task budget may span several internal segments. Reaching the
   ordinary per-segment limit automatically starts the next segment while the
   task remains within budget and #102 reports progress.
3. Automatic segment continuation is not an approval for patches, commands,
   checks, browser access, MCP tools or questions. Every existing safety pause
   remains.
4. Repository configuration may lower a task budget but cannot raise a user or
   administrator ceiling. A worker with no approved task budget keeps today's
   explicit **Continue** behaviour.
5. Before a request would exceed a configurable share of the provider context
   window, completed within-turn tool history is compacted. Full tool calls and
   results remain in the session/audit stores; only the next model request is
   reduced.
6. Compaction preserves verbatim: the system instructions, current user
   request, approved plan and budget, pending action, latest result for every
   open step, active failure findings, protected paths, action identifiers,
   file hashes needed for safety, and the bounded turn notes.
7. Older completed reads become deterministic manifests containing tool kind,
   path or command identity, range, result hash, outcome and a bounded redacted
   excerpt when needed. The model may reread a file through the normal tool if
   content was removed.
8. Compaction never converts untrusted tool output into instructions and never
   drops an unresolved error, pending proposal or unknown external outcome.
9. Each compaction and segment boundary is durable and auditable, including
   counts and token estimates but no new unredacted content.
10. The task stops with a specific outcome when the approved task budget is
    exhausted, compaction cannot fit the required state, or #102 detects no
    progress. It never silently falls back to `complete`.

## 3. Non-goals

- Unlimited unattended execution. The approved budget is finite and existing
  approval boundaries remain.
- Replacing #55's conversation summaries. This spec handles the tool-round
  array within one task run.
- Letting a repository choose a larger spend for its clone.
- Treating model-written prose as the authoritative record of actions, hashes,
  approvals or plan state.

## 4. Design

Add a durable `TaskExecutionBudget` approved alongside a worker plan and a
`segment_started`/`segment_finished` event pair. Segment limits remain small
checkpoints inside the task budget; the worker host resumes the same task and
plan without a synthetic user message when continuation is pre-authorized.

A deterministic request compactor walks completed tool rounds oldest-first.
It replaces eligible results with structured manifests until the next request
fits the window share. Safety-critical and current-step records are pinned.
The stored transcript is append-only and unchanged, matching #55's rule that
compaction changes what is sent rather than what is remembered.

## 5. Acceptance

- A scripted worker requiring more than sixteen action rounds completes across
  at least three internal segments after one task-budget approval.
- The same script without a task budget pauses at the existing round boundary.
- A patch and a command still pause for their own decisions inside an approved
  task budget.
- A long sequence of file reads stays below the configured request-window
  share, and the model can reread compacted content.
- Pending approvals, protected paths, current findings and base hashes are
  byte-identical before and after compaction.
- No-progress and total-budget exhaustion stop with distinct non-success
  outcomes that survive worker restart.
