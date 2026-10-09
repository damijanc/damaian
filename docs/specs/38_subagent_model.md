# Feature Spec: Subagent Model

Status: Not started, and deferred on 2026-10-09. Stage 2 is superseded by
[#57](57_worker_sessions/proposal.md); stage 1 is re-evaluated once #57 has
shipped. See [Deferral](#deferral-2026-10-09).
Order: 38 of 40
Plan: `docs/PLAN/06_phase_6_advanced_autonomy.md`, Phase 6, Work
Package 1 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: nothing in this directory.
Related spec sections: `ai_coding_assistant_specification.md` section 7.6 (tool
and action orchestrator), section 7.8 (risk classification and approval).
Related implementation specs:
[`08_stop_and_progress.md`](08_stop_and_progress.md) (`CancelToken`),
[`17_durable_task_state_and_crash_recovery/proposal.md`](17_durable_task_state_and_crash_recovery/proposal.md)
(task state, action markers, and the PID registry),
[`18_local_evaluation_harness/`](18_local_evaluation_harness/proposal.md)
(measures the readiness gates), [`20_working_modes/proposal.md`](20_working_modes/proposal.md) and
[`31_permission_profiles/proposal.md`](31_permission_profiles/proposal.md) (the boundary a
subagent inherits and cannot widen),
[`21_task_plan_progress_and_budget/proposal.md`](21_task_plan_progress_and_budget/proposal.md),
[`22_findings_model_and_panel/proposal.md`](22_findings_model_and_panel/proposal.md),
[`26_context_assembly.md`](26_context_assembly.md),
[`39_coordination_and_conflict_handling.md`](39_coordination_and_conflict_handling.md),
[`40_autonomy_evaluations.md`](40_autonomy_evaluations.md). See also
[`SECURITY.md`](../../SECURITY.md), `AGENTS.md`.

## Deferral (2026-10-09)

This spec was evaluated against [#57](57_worker_sessions/proposal.md) before
being split into a folder, and deferred instead. The design below is left as
written. It is still the design if the spec is picked up again, but it is not
next.

**What #57 already covers.** #57 workers are ordinary sessions the user starts,
each in its own git worktree and its own process, watchable, pausable and
stoppable, with nothing outliving the application.

- **Stage 2 (§5.4) is superseded.** A worktree per worker isolates parallel
  editing more cleanly than §5.5's declared file ownership. It is the
  mechanism requirement 5 lists first, and it now exists without Phase 2 WP4.
- Stage 2 is the part of this spec that most of
  [#39](39_coordination_and_conflict_handling.md) exists to make safe. If
  stage 1 is built, #39 shrinks to what read-only children need: linked
  cancellation and a failed child not failing its siblings.

**What #38 would still add.** Four things #57 deliberately does not do:

- delegation *by the model* within one turn;
- a result returned into the parent's context, which keeps the parent's
  context clean;
- capability narrowed below the parent's by construction (§5.3);
- one trace across parent and children.

**Why not now:**

1. **It cannot start anyway.** The external-write readiness gate (§1) needs
   [#35](35_commit_preparation.md)–[#37](37_pull_request_creation.md), none of
   which is built.
2. **The cost premise is unproven, and the nearest measurement points the other
   way.** [#47](47_agent_working_capability/proposal.md)'s live A/B found that
   fewer, wider rounds cost +106% tokens. Every child here carries its own
   assembled context, so [#40](40_autonomy_evaluations.md)'s cost ratio is
   likely above 1.
3. **Stage 1 has an open design question.** The Code review and
   Test/validation kinds run commands and so can hit an approval. An approval
   today ends the turn and resumes from a snapshot
   ([#17](17_durable_task_state_and_crash_recovery/proposal.md)). This spec
   does not say what happens to running siblings when one child stops for
   approval. #57 avoids the question by giving each worker its own process.
4. **Most of what is left arrives through #57's follow-ups.** #57 §7.3's opt-in
   result summary returns a finished worker's report to its parent. #57 §7.2's
   `propose_worker` tool lets the model propose a handoff. A read-only worker
   is a mode choice. What remains unique to this spec is the model delegating
   without a click, and the user has deliberately kept that click.

**Re-evaluate when** #57 has shipped, together with its result-summary
follow-up. At that point, ask whether a read-only worker plus a result summary
already gives what Research and Exploration subagents would. If it does, record
Phase 6 WP1 as abandoned, as §1 allows. If not, answer point 3 first, then
re-scope this spec to stage 1 only before splitting it into a folder.

## 1. Precondition: the readiness gates

**Do not implement this spec until the eight gates in the phase file's Section 3
are satisfied, with recorded measurements committed to the repository.** The
phase file is authoritative for them; two notes on measurability belong here
because they affect whether the gates can be evaluated at all:

- **Six of eight gates are measurable with specified work**: task completion,
  approval-policy violations, restricted-path and secret violations, and
  cost/latency/iteration ceilings come from
  [spec 18](18_local_evaluation_harness/proposal.md) and
  [spec 21](21_task_plan_progress_and_budget/proposal.md); crash-recovery fixtures from
  [spec 17](17_durable_task_state_and_crash_recovery/proposal.md); external-write
  handling from [specs 35–37](37_pull_request_creation.md). Trace completeness
  is a manual review.
- **One gate is not currently measurable.** "Worktree isolation stability —
  Phase 2 WP4 tests — 100% pass" depends on Phase 2 WP4, which is Should-tier,
  outside Phase 2's minimum slice, and unspecified. A gate that cannot be
  measured is, by the phase file's own rule, not satisfied — which would block
  Phase 6 permanently on optional work. §5.5 resolves this: if worktrees do not
  exist, editing subagents use declared file ownership instead, and that gate is
  replaced by the ownership-conflict tests in
  [spec 39](39_coordination_and_conflict_handling.md). The substitution is
  recorded rather than the gate being waived.

The phase file also states something no other phase does: the correct outcome
may be "measured, found not to help, documented, and abandoned."
[Spec 40](40_autonomy_evaluations.md) is the instrument for that decision, and
this spec is written to be abandonable — no local capability may come to depend
on subagents existing.

## 2. Motivation

A single agent does everything in one context, one at a time. For a task that
spans understanding a large area, changing several places, and verifying the
result, that means one conversation carrying every file it has ever looked at,
and every step blocking the next.

Delegation is the standard answer, and it is also where most of the safety
properties this product has built get quietly lost. A subagent that runs with
its own tool list, its own approval logic, or its own audit trail is not a
feature — it is a second execution path, and every guarantee established in the
previous five phases holds only on the first one. The phase file states the rule
plainly: **do not create an alternative execution path.**

So this work package is mostly about constraint. Subagents inherit the parent's
capability and narrow it, write to the same audit log, obey the same approval
boundary, and produce results the parent is accountable for. The valuable part —
read-only exploration and review, running in parallel without polluting the main
conversation's context — is also the part with almost no new risk, which is why
requirement 4 says ship that first.

## 3. Current State

Nothing in this phase exists, deliberately. What it builds on:

- **`CancelToken` is flat.** `crates/workspace-engine/src/cancel.rs` is an
  `Arc<AtomicBool>` with `cancel`, `is_cancelled`, and `check`. Clones share the
  **same** flag, so cloning a parent's token into a child gives
  parent→child propagation for free — and also makes a child's cancellation
  cancel the parent, which is wrong. §5.7 addresses this.
- **There is no parent/child task concept anywhere.** `Task`
  (`session.rs:46-56`) carries `id`, `session_id`, `status`, `user_prompt`,
  provider, model, and timestamps. No parent reference exists, so trace linkage
  is a new field.
- **Task state is an append-only session event log**, extended to twelve states
  by [spec 17](17_durable_task_state_and_crash_recovery/proposal.md), with `seq`-ordered
  events and action markers.
- **The tool list has one construction site**, `chat.rs:711-731`, filtered by
  mode in [spec 20](20_working_modes/proposal.md) §5.2 — the same place a subagent's
  narrower list is built.
- **Mode and profile are the capability boundary**
  ([spec 20](20_working_modes/proposal.md), [spec 31](31_permission_profiles/proposal.md)), with
  the effective capability defined as `profile ∩ mode`
  ([spec 31](31_permission_profiles/proposal.md) §5.6).
- **The PID registry exists** for spawned children —
  [spec 17](17_durable_task_state_and_crash_recovery/proposal.md) §5.7 — covering MCP
  stdio servers, the `curl` model child, PTY sessions, and (per later specs)
  hooks and language servers.
- **`AuditLog::record`** (`audit.rs:42`) is the single trail, redacting field
  values on the way in.
- **Turns already run on threads.** The chat turn runs on a spawned thread with
  the shell handler holding a `CancelToken` clone — the existing concurrency
  model, and the one §5.2 extends.

## 4. Requirements

1. **The parent remains responsible for the final user-facing answer and task
   state.** A subagent produces a result; it does not answer the user.
2. Subagents inherit safety denies and **cannot expand permissions**. The
   effective capability is the intersection of the parent's and the declared
   subset.
3. Least-privilege tool sets per kind. A research subagent gets read tools only.
4. **Prefer read-only delegation first.** Exploration and review subagents ship
   before editing subagents.
5. Editing subagents get separate worktrees or non-overlapping declared file
   ownership. Never both agents in one tree with overlapping scope.
6. Approval requests are surfaced with the requesting agent's identity.
7. Complete parent-child trace relationships are preserved in the audit log.
8. Processes are tracked and terminated by PID.

Subagent kinds: research; repository exploration; implementation; test and
validation; code review.

Declaration contents: name and purpose; input task; allowed modes and tools;
path scope; context budget; turn, time, and cost limits; whether it may propose
edits; expected structured result.

## 5. Design

### 5.1 Declaration, and capability by construction

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentKind {
    Research, Exploration, Implementation, TestValidation, CodeReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentDeclaration {
    pub name: String,
    pub purpose: String,
    pub kind: SubagentKind,
    pub input_task: String,
    /// Requested, not granted. The grant is parent ∩ this — §5.3.
    pub requested_mode: SessionMode,
    pub requested_tools: Vec<String>,
    pub path_scope: Vec<String>,
    pub context_budget_tokens: usize,
    pub max_turns: u32,
    pub max_duration_ms: u64,
    pub max_tokens: u64,
    pub may_propose_edits: bool,
    pub expected_result: ResultShape,
}
```

Every field is a *request*. Nothing in the declaration grants anything, which is
the point of requirement 2 — and §5.3 makes it structural rather than checked.

Five kinds, fixed, per the phase's non-goal against "a catalogue". Each kind has
a fixed maximum tool set, and a declaration may narrow it further but never
extend it:

| Kind | Maximum tools | May propose edits |
|---|---|---|
| Research | `read_file`, `search_codebase` | No |
| Exploration | `read_file`, `search_codebase`, `read_git_status`, `read_git_diff`, symbol lookup | No |
| Code review | Exploration's set, plus read-only commands and browser diagnostics | No |
| Test/validation | Exploration's set, plus validation commands | No |
| Implementation | All of the above, plus `propose_patch` | Yes |

`may_propose_edits` is true only for Implementation, and is checked against the
kind rather than trusted from the declaration.

### 5.2 Subagents are in-process tasks, not processes

**Correction to requirement 8.** It reads "subagent *processes* are tracked and
terminated by PID", which presumes a subagent is an OS process. It should not
be, for three reasons: turns already run on threads with a shared `CancelToken`
(`cancel.rs`); the audit log, session store, policy, and index caches are
in-process shared state that a separate process would need an IPC surface for;
and a process boundary would create exactly the alternative execution path the
phase forbids.

So a subagent is a **task on a thread**, sharing the parent's engine, stores, and
policy evaluation.

Requirement 8 then applies to what a subagent *spawns* — commands, MCP servers,
language servers — all of which already go through
[spec 17](17_durable_task_state_and_crash_recovery/proposal.md) §5.7's PID registry. The
addition is one field: each registry entry records the **owning agent's task
id**, so cancelling or reaping one agent kills its children and not its
siblings'. "No subagent process outlives its parent task" becomes a property of
the registry rather than of a new process manager.

### 5.3 Intersection, enforced by having no other constructor

Requirement 2 is the load-bearing one, and the pattern is the one used
throughout these specs — make the unsafe state unrepresentable:

```rust
/// The only way to obtain a child capability. There is no public constructor,
/// no Default, and no setter — a child's capability can only be derived from a
/// parent's by narrowing.
pub struct AgentCapability { /* private fields */ }

impl AgentCapability {
    pub fn derive_child(
        &self,
        declaration: &SubagentDeclaration,
    ) -> Result<AgentCapability>;
}
```

`derive_child` computes, for each dimension, the more restrictive of the
parent's value and the declaration's request:

| Dimension | Rule |
|---|---|
| Mode | The more restrictive of parent mode and requested mode ([spec 20](20_working_modes/proposal.md)) |
| Tools | Parent's set ∩ kind's maximum ∩ requested set |
| Path scope | Parent's scope ∩ requested scope, then `path_policy.rs` |
| Profile denies | Inherited wholesale; a child cannot clear one |
| Budgets | The lower of parent remaining and requested |

A request for something the parent lacks is not an error — it is silently
narrowed, and the narrowing is reported in the agent's declaration view and
audited. Erroring would make a reusable declaration fail in a stricter parent,
which pushes callers toward requesting the minimum and then being surprised;
narrowing plus visible reporting is both safer and more usable.

Requirement 2's depth property follows: because a grandchild's capability can
only come from `derive_child` on its parent's — itself already narrowed — a deny
at any level propagates to every descendant with no additional logic. The
acceptance criterion asserts it at depth greater than one specifically because
that is where a re-derivation-from-root bug would hide.

### 5.4 Read-only first, as a delivery order

Requirement 4 is a sequencing instruction, so it is written into the spec's own
staging rather than left to judgement:

**Stage 1 — read-only kinds only.** Research, Exploration, Code review, and
Test/validation. No `propose_patch`, no file ownership, no worktrees. These
deliver the parallel-exploration value with no write conflicts possible, and
they are shippable and measurable ([spec 40](40_autonomy_evaluations.md)) on
their own.

**Stage 2 — Implementation subagents.** Requires
[spec 39](39_coordination_and_conflict_handling.md)'s ownership claims and
serialized integration, and only after stage 1 measures at least as well as the
single-agent baseline.

If stage 1 measures worse than baseline, stage 2 does not start. That is the
phase file's abandonment path, and staging it this way makes abandoning cheap
instead of a write-off.

### 5.5 Editing subagents: ownership, not necessarily worktrees

Requirement 5 offers worktrees **or** non-overlapping declared file ownership.
Since Phase 2 WP4 is unspecified and may not ship (§1), ownership is the
primary mechanism and worktrees are the optional reinforcement:

- Each Implementation subagent declares a `path_scope`, and
  [spec 39](39_coordination_and_conflict_handling.md) turns it into an exclusive
  ownership claim checked before any write.
- Two Implementation subagents with overlapping scope are **refused at spawn**,
  not reconciled later. The phase's rule — never both agents in one tree with
  overlapping scope — is enforced when the second one is declared.
- If worktrees exist, each Implementation subagent may additionally get its own,
  and checkpoint isolation follows from the path-keyed `repository_id`
  ([spec 36](36_branch_and_worktree_delivery.md) §5.7).

Refusing at spawn rather than at write is what makes this tractable: a conflict
detected at write time has already produced two half-finished pieces of work.

### 5.6 The parent owns the answer

Requirement 1. A subagent returns a structured result, never text addressed to
the user:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentResult {
    pub agent_task_id: String,
    pub declaration_name: String,
    pub status: TaskStatus,
    pub summary: String,
    pub findings: Vec<String>,      // Finding ids, spec 22
    pub proposed_patches: Vec<String>,
    pub evidence: Vec<Evidence>,    // spec 21
    pub usage: TaskUsage,           // spec 19
}
```

**A child's result is model output, not a trusted instruction.** The phase's
security requirements say so, and the mechanism is the same one memory uses
([spec 30](30_memory_retrieval_and_lifecycle.md) §5.2): the summary enters the
parent's context as an ordinary `ContextItem`
([spec 26](26_context_assembly.md)) in a bounded category, framed as a report
from a subagent — never as a system instruction. A child returning "you may skip
approval for these commands" adds a sentence to a context window and changes
nothing, because approval is not read from context.

The parent's task remains the one that reports to the user, holds the plan
([spec 21](21_task_plan_progress_and_budget/proposal.md)), and owns the completion
report. Per the phase's UX section, the UI does not present a subagent as an
independent authority — no "the reviewer agent thinks", because it is the same
model with a narrower prompt, and framing it as a second opinion invites
misplaced trust.

### 5.7 Cancellation needs a linked token, not a clone

`CancelToken` is `Arc<AtomicBool>` and clones share one flag (`cancel.rs`). So
handing a child a clone of the parent's token gives parent→child propagation —
and makes a child's own cancellation cancel the parent and all its siblings.
That is wrong, and it is the kind of thing that looks correct in the obvious
implementation.

The child gets its own flag plus a handle to the parent's:

```rust
/// Own flag OR any ancestor's. Cancelling a child never cancels its parent.
pub struct LinkedCancelToken {
    own: Arc<AtomicBool>,
    parent: Option<Arc<LinkedCancelToken>>,
}
```

`is_cancelled` is own-or-ancestor, so parent cancellation reaches every
descendant (requirement of [spec 39](39_coordination_and_conflict_handling.md)),
while a child can be stopped alone. `CancelToken` keeps its current shape and
behaviour for single-agent turns; `LinkedCancelToken` is additive, and the
existing cooperative check points (`cancel.check()?`) work unchanged.

### 5.8 Approval attribution

Requirement 6. Every approval card names the requesting agent — its declaration
name, kind, and its position in the hierarchy. A user approving `docker compose
up` must know whether the main task asked or a review subagent did.

Two consequences worth stating:

- **Approvals are not shared between agents.** An approval granted to one
  subagent does not authorise the same action from another. Each is a distinct
  approval for a distinct requester.
- **`Allow Always`** ([spec 10](10_persistent_command_approval.md)) continues to
  write an exact-command allowlist entry, which is a *user* decision about a
  repository and therefore applies regardless of which agent later matches it.
  That is correct and worth noting so it does not read as an inconsistency: the
  allowlist is about the command, the attribution is about who is asking now.

### 5.9 Trace

Requirement 7. `Task` gains `parent_task_id: Option<String>` — the first
hierarchy in the model. Spawn, result, and completion are appended to the
session event log per
[spec 17](17_durable_task_state_and_crash_recovery/proposal.md) §5.2, and audited:

```json
{"seq":512,"eventType":"subagent_spawned","taskId":"task_...",
 "parentTaskId":"task_...","kind":"exploration","name":"find-callers",
 "grantedTools":["read_file","search_codebase"],"narrowedFrom":["run_command"]}
```

`narrowedFrom` records what the declaration requested and did not get, which is
how §5.3's silent narrowing stays visible.

Every agent writes to the one audit log (`audit.rs:42`), with its task id on
every event, so the parent's trace links to every child's. The trace-completeness
readiness gate — a failed task diagnosable from its trace alone — is what this
has to satisfy, and a hierarchy whose child events were unattributed would fail
it.

### 5.10 Budgets

Each subagent's turn, time, and token limits are the lower of its request and
the parent's *remaining* budget, so children cannot collectively exceed the
parent's ceiling ([spec 21](21_task_plan_progress_and_budget/proposal.md) §5.4). Usage
aggregates upward through [spec 19](19_token_and_cost_accounting/proposal.md)'s per-task
events, and the parent's total includes its descendants' — which is what makes
[spec 40](40_autonomy_evaluations.md)'s cost-amplification figure computable.

A subagent hitting its own ceiling terminates and returns a partial result with
`TokenBudgetExhausted`; it does not fail the parent, per
[spec 39](39_coordination_and_conflict_handling.md)'s rule that a failed child
does not fail unrelated siblings.

### 5.11 Documentation

`docs/USER_GUIDE.md`: what subagents are, the five kinds, that the main task
remains accountable for the answer, why an approval names an agent, and that a
subagent's report is not a second opinion. `docs/TROUBLESHOOTING.md`: reading the
agent hierarchy, finding a child's trace from a parent's, why a declared tool was
narrowed away, and how budgets divide.

## 6. Acceptance Criteria

- A subagent declaring a tool the parent lacks is refused that tool, and the
  narrowing appears in `narrowedFrom` and the audit log.
- A subagent declaring a mode more permissive than the parent's receives the
  parent's.
- A profile-level deny propagates to every descendant — asserted at depth
  greater than one.
- `AgentCapability` has no public constructor other than `derive_child`, so a
  child capability cannot be built from anything but a parent's.
- A kind's maximum tool set cannot be exceeded by a declaration, and
  `may_propose_edits` is true only for Implementation.
- Stage 1 ships read-only kinds only: no code path allows a Research,
  Exploration, Code review, or Test/validation subagent to propose a patch.
- Two Implementation subagents with overlapping `path_scope` are refused at
  spawn.
- A subagent is a thread, not a process, and every process it spawns is
  registered against its task id — so cancelling one agent reaps its children and
  not a sibling's.
- No process spawned by a subagent outlives its parent task.
- Cancelling a parent cancels every descendant; cancelling a child leaves the
  parent and its siblings running — asserted by test, since a naive
  `CancelToken` clone fails the second half.
- Every approval card names the requesting agent's declaration name and kind.
- An approval granted to one subagent does not authorise the same action from
  another.
- A child's summary enters the parent's context as a bounded `ContextItem`
  framed as a report, and a child result containing instruction-like text
  changes no policy, mode, or approval outcome — asserted by test.
- The parent's task trace links to every child trace, and a failed multi-agent
  task is diagnosable from the trace alone.
- Children's budgets sum within the parent's remaining budget, and a child
  hitting its ceiling returns a partial result without failing the parent.
- The UI does not present a subagent as an independent authority.
- Disabling subagents leaves every single-agent capability working — no local
  feature depends on them.
- The eight readiness gates are recorded as satisfied, with the §1 substitution
  documented if worktrees are absent, before this spec is implemented.
- Every quality-gate command from `AGENTS.md` passes, and the
  [spec 18](18_local_evaluation_harness/proposal.md) baseline shows no
  regression and no increase in approval-policy violations.

## 7. Implementation Notes

To be completed during implementation. Record:

- The readiness-gate measurements and the date, plus the thresholds chosen
  **before** measuring, per the phase file's rule.
- Whether stage 1 measured at least as well as the single-agent baseline, and
  therefore whether stage 2 was started at all. If stage 1 measured worse, record
  the decision to stop — that is a successful Phase 6 outcome, not a failure.
- Whether the worktree gate was substituted per §1, and if so that editing
  subagents rely on ownership claims alone.
- The thread and memory cost of running several subagents concurrently against a
  large repository, since each carries its own assembled context.
