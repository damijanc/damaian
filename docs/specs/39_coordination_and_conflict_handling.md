# Feature Spec: Coordination and Conflict Handling

Status: Not started
Order: 39 of 40
Roadmap: `docs/ROADMAP/06_phase_6_advanced_autonomy.md`, Phase 6, Work
Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.6 (tool
and action orchestrator), section 7.7 (diff and patch engine). Related
implementation specs:
[`04_hunk_level_patch_apply.md`](04_hunk_level_patch_apply.md),
[`08_stop_and_progress.md`](08_stop_and_progress.md),
[`16_session_checkpoints_and_rewind.md`](16_session_checkpoints_and_rewind.md),
[`17_durable_task_state_and_crash_recovery.md`](17_durable_task_state_and_crash_recovery.md),
[`21_task_plan_progress_and_budget.md`](21_task_plan_progress_and_budget.md),
[`22_findings_model_and_panel.md`](22_findings_model_and_panel.md),
[`23_verification_loop.md`](23_verification_loop.md) (the checks §5.5 reruns),
[`38_subagent_model.md`](38_subagent_model.md) (declares the agents this
coordinates; its §5.4 staging gates when this spec's write path is needed),
[`40_autonomy_evaluations.md`](40_autonomy_evaluations.md).

## 1. Motivation

[Spec 38](38_subagent_model.md) can run several agents. This spec is what stops
them from ruining each other's work.

The dangerous case is specific and easy to reach: two Implementation subagents
both decide a shared helper needs changing. Each reads it, each composes a patch
against what it read, and both patches apply cleanly in sequence — the second
overwriting the first, or applying to a file whose content has moved out from
under its hunks. Nothing errors. The result is a repository state neither agent
produced and no one reviewed.

`patch_engine.rs` already has the mechanism that prevents the second half of
that: `apply_patch` compares the current file hash against `base_hash` and
refuses to overwrite work changed after the preview (`patch_engine.rs:291`),
which `AGENTS.md` records as a product guarantee. What it cannot do is prevent
the *first* half — two agents deciding to own the same file — because that is a
scheduling question, not a patch question. So this spec's core is claiming
before writing rather than reconciling afterwards.

The second theme is that concurrency makes "did it work?" harder. Two patches
that each pass their own checks can fail together: one renames a function, the
other adds a caller for the old name, and both were individually green. Rerunning
checks after combining is the only way to know, and skipping it is the most likely
way a multi-agent run silently produces something broken.

## 2. Current State

Nothing in this phase exists. What it builds on:

- **Hash-based conflict refusal already exists**, twice. Apply compares against
  `base_hash` (`patch_engine.rs:291`); rollback compares against `applied_hash`
  so partial-hunk applies stay reversible (`patch_engine.rs:600-611`).
- **Patch application is already selective** — `approved_paths` and
  `hunk_selection` (`patch_engine.rs:376-384`) from
  [spec 04](04_hunk_level_patch_apply.md).
- **`CancelToken` is flat and clone-shared** (`cancel.rs`).
  [Spec 38](38_subagent_model.md) §5.7 adds `LinkedCancelToken` — own flag plus
  ancestor handle — which this spec's propagation requirement depends on.
- **There is no task dependency, concurrency, or ownership concept.** `Task`
  (`session.rs:46-56`) gains `parent_task_id` in
  [spec 38](38_subagent_model.md) §5.9 and nothing else.
- **The verification loop is per-task.**
  [Spec 23](23_verification_loop.md) runs checks after a task's edits and records
  per-run file hashes, marking a run `stale_after_partial_acceptance` when the
  accepted set changes (§5.6 there) — the same staleness idea this spec needs for
  combined work.
- **Plans and evidence exist** ([spec 21](21_task_plan_progress_and_budget.md)),
  with `Evidence` deliberately having no model-asserted variant.
- **Checkpoints are per repository path**
  ([spec 16](16_session_checkpoints_and_rewind.md) §5.1), keyed on
  `repository_id` = `sha256(canonical path)` (`indexer.rs:382-385`).
- **The session event log is append-only** with `seq` ordering
  ([spec 17](17_durable_task_state_and_crash_recovery.md) §5.2).

## 3. Requirements

1. Add task dependencies; bounded concurrency; agent status and cancellation;
   structured result handoff; file ownership claims; conflict detection; and
   parent review before integration.
2. **Never let two agents silently write the same file.** Ownership claims are
   checked before a write, not reconciled after.
3. Integration of proposed patches is serialized: applied one at a time, with
   hashes revalidated between them, reusing the `base_hash` check.
4. Hashes are revalidated and checks rerun after combining work.
5. A failed child does not automatically fail unrelated completed children.
6. Cancellation propagates predictably, built on `CancelToken`.
7. Agents are not created recursively beyond a configurable depth.

## 4. Non-goals

- Distributed or cross-machine coordination. Agents are threads in one process
  ([spec 38](38_subagent_model.md) §5.2).
- Automatic conflict resolution or three-way merging between agents' patches. A
  conflict is reported to the parent and the user; nothing merges it.
- A general work-queue or scheduler abstraction. §5.2's concurrency control is a
  bounded pool, not a framework.
- Speculative or retried delegation — re-running a failed child automatically.
  §5.6 reports failure to the parent, which decides.
- Alternative implementations of the same task — Phase 6 WP4, Could-tier, and
  the phase's first cut.
- Inter-agent messaging as a channel agents use to negotiate. Results flow to the
  parent; agents do not talk to each other. §5.7 explains why.
- Long-running pause and resume — Phase 6 WP3, Should-tier.

## 5. Design

### 5.1 Ownership claims, taken at spawn

Requirement 2. An Implementation subagent's `path_scope`
([spec 38](38_subagent_model.md) §5.1) becomes an **exclusive claim**, registered
when the agent is spawned:

```rust
/// Exclusive write claims, keyed by repository. Checked at spawn and again
/// before every write.
pub struct OwnershipRegistry { /* private */ }

impl OwnershipRegistry {
    /// Fails if any pattern overlaps a live claim held by another agent.
    pub fn claim(&self, agent_task_id: &str, patterns: &[String]) -> Result<Claim>;
    /// Fails if the path is not covered by this agent's claim.
    pub fn assert_writable(&self, agent_task_id: &str, path: &str) -> Result<()>;
}
```

Two check points, deliberately:

- **At spawn**, overlapping scopes are refused ([spec 38](38_subagent_model.md)
  §5.5). This is where the phase's "never both agents in one tree with
  overlapping scope" rule is enforced, and refusing here means no wasted work.
- **Before every write**, `assert_writable` runs inside the patch-application
  path. A claim checked only at spawn would be bypassed by an agent that later
  proposes a patch outside its declared scope — which a model will do, because a
  declared scope is a prompt constraint until something enforces it.

Overlap is computed on glob patterns, and **overlap is decided conservatively**:
if two patterns cannot be proven disjoint, they are treated as overlapping. A
false overlap costs a refused spawn; a missed overlap costs a lost write.

Read access is not claimed. Any agent may read anything its capability allows —
concurrent reads conflict with nothing, and claiming reads would serialise
exploration, which is the case with the most to gain from concurrency.

Claims are released when the agent's task reaches a terminal state, and are
recorded in the session event log so a crash mid-run does not leave a phantom
claim blocking the next session.

### 5.2 Bounded concurrency and dependencies

Requirement 1. A parent declares its children with optional dependencies:

```rust
pub struct AgentGroup {
    pub max_concurrent: u32,
    pub members: Vec<(SubagentDeclaration, Vec<String>)>, // decl, depends_on names
}
```

Dependencies are a topological order, not a solver: a child runs when its named
dependencies have reached a terminal state. A cycle is refused at declaration.

`max_concurrent` defaults low — a small number, configurable — because each agent
carries its own assembled context ([spec 26](26_context_assembly.md)) and its own
model calls, so concurrency multiplies both memory and spend. The default exists
to make the cost visible rather than to maximise throughput, and
[spec 40](40_autonomy_evaluations.md) measures whether raising it helps.

Concurrency is also bounded by the parent's remaining budget
([spec 38](38_subagent_model.md) §5.10): if the parent cannot afford three
children, it does not start three.

### 5.3 Depth

Requirement 7. `agent_max_depth` in `Config`, defaulting to **1** — the parent
may spawn children; children may not spawn grandchildren by default.

Depth 1 as the default rather than 2 or 3, because each level multiplies cost and
makes traces harder to follow, and the phase's own value case (parallel
exploration, review) needs exactly one level. Deeper nesting is available by
configuration for anyone who measures a benefit.

Exceeding the depth is refused at spawn, reported as a `Finding`
([spec 22](22_findings_model_and_panel.md)) so it surfaces in the panel rather
than only in a log, and audited. It does not fail the requesting agent — a model
attempting to over-delegate should be told no and continue.

### 5.4 Serialized integration

Requirement 3. When several children have produced patches, the parent
integrates them **one at a time**:

```text
for each patch, in a deterministic order:
    revalidate every target file's hash against the patch's base_hash
    if a hash has moved  → conflict; do not apply; report and continue
    else                 → apply through the normal path, with approval
    record the new hashes
```

The revalidation between applies is the existing `base_hash` comparison
(`patch_engine.rs:291`) — reused rather than reimplemented, which is what makes
this correct: patch two was composed against a file that patch one may have just
changed, and that is exactly the situation the existing check was built for.

Order is deterministic — by child completion `seq` — so two runs over the same
inputs integrate in the same sequence and a conflict is reproducible.

**Parent review before integration** (requirement 1) is the normal patch preview
and approval path, not a new one. Each child's patch is previewed and approved
individually, with the requesting agent named
([spec 38](38_subagent_model.md) §5.8). A conflicting patch is presented as a
conflict with what it collided with, and the user's options are to take it
against the current state (which requires the child to re-propose against the new
base) or to drop it.

Nothing is auto-merged. Two agents' overlapping edits to one file, having been
prevented at spawn by §5.1, should not occur — and if they do, the conflict is
reported rather than resolved.

### 5.5 Rerunning checks after combining

Requirement 4, and the requirement most likely to be dropped because each child
already ran its own checks and everything looked green.

After the last patch is integrated, the verification loop
([spec 23](23_verification_loop.md)) runs again over the **combined** state:

- The check set is the union of the checks the children's changed paths select,
  per [spec 23](23_verification_loop.md) §5.2's file-category targeting.
- Every child's check runs whose recorded file hashes no longer match — the same
  `stale_after_partial_acceptance` mechanism (§5.6 there), applied to
  integration rather than partial acceptance. A child's passing `cargo test` is
  stale the moment a sibling's patch lands.
- **A combination that fails a check is reported as failed.** The children
  individually succeeded and the combination did not, and the completion report
  says exactly that rather than averaging the two into a success.

This is where the "two individually passing patches fail together" case is
caught, and the acceptance criterion constructs that case deliberately: one child
renames a symbol, another adds a reference to the old name.

Repair after a failed combination goes to the **parent**, not back to the
children. The parent has the whole picture; a child that only ever saw its own
scope cannot fix an integration failure caused by a sibling, and asking it to
would produce a patch composed in ignorance of the other half.

### 5.6 Failure isolation

Requirement 5. Each child's task status is independent
([spec 17](17_durable_task_state_and_crash_recovery.md)'s twelve states). A child
that fails, is cancelled, or exhausts its budget:

- Releases its ownership claim.
- Returns a partial `SubagentResult` with its status and whatever evidence it
  produced.
- Does **not** fail its siblings, and does not discard completed siblings' work.

The parent decides. A group's outcome is reported per child, and the parent's own
status reflects whether it can still complete its plan
([spec 21](21_task_plan_progress_and_budget.md)) — which may be yes with three of
four children's work, or no.

One case needs stating: a child that failed **after** its patch was integrated
leaves that patch in place. Rolling it back automatically would discard reviewed,
applied, approved work because of a later unrelated failure. The failure is
reported, and rewind ([spec 16](16_session_checkpoints_and_rewind.md)) is
available if the user wants it — as a user decision, not an automatic one.

### 5.7 Cancellation, and why agents do not message each other

Requirement 6, built on [spec 38](38_subagent_model.md) §5.7's
`LinkedCancelToken`: a child's `is_cancelled` is own-flag-or-ancestor, so
cancelling the parent reaches every descendant, and cancelling one child leaves
its siblings running.

Cancellation stays **cooperative**, as `cancel.rs` documents — checked at points
where stopping is safe. Cancelling a group therefore does not abort an
in-flight patch application or command; it stops before the next one, which is
what keeps [spec 17](17_durable_task_state_and_crash_recovery.md)'s
unknown-outcome surface from growing with concurrency.

Requirement 1's "structured result handoff" is parent-mediated and one-way:
children return `SubagentResult` to the parent, and the parent may include a
child's summary in another child's input task. There is deliberately **no
agent-to-agent channel**. Two reasons: a message from one agent to another is
model output being consumed as input with no user in the loop, which is a
prompt-injection surface with no reader; and a mesh of messages destroys the
trace property the readiness gate requires — a failed task diagnosable from its
trace alone is achievable with a tree and not with a graph.

Where a child's output does enter another child's input, it enters as a bounded
context item framed as a report, exactly as it does for the parent
([spec 38](38_subagent_model.md) §5.6).

### 5.8 Status and audit

The agent hierarchy view (the phase's UX section) shows per agent: kind, name,
status, current assignment, granted tools, path scope, elapsed time, tokens and
cost consumed, pending approvals, and produced patches and findings — with cancel
controls per agent and for the group.

Audited through `AuditLog::record` (`audit.rs:42`): every spawn, claim, claim
release, refusal (overlap, depth), integration attempt, conflict, combined-check
result, cancellation, and handoff, each carrying the agent task id and parent
task id.

### 5.9 Documentation

`docs/USER_GUIDE.md`: what running several agents does, why two agents cannot own
the same files, why checks run again after combining, and what happens when one
agent fails. `docs/TROUBLESHOOTING.md`: reading the hierarchy, why a spawn was
refused, why a patch was reported as conflicting, how to interpret a
combined-check failure, and where claims appear in the session log.

## 6. Acceptance Criteria

- Two agents claiming the same file produce a detected conflict at spawn, not a
  lost write.
- Two patterns that cannot be proven disjoint are treated as overlapping.
- An agent proposing a patch outside its claimed scope is refused at write time,
  not only at spawn.
- Read access is not claimed, and two agents can read the same file
  concurrently.
- A claim is released when its agent reaches a terminal state, and a crash
  mid-run leaves no phantom claim blocking a later session.
- Patches integrate one at a time in deterministic completion order, with
  `base_hash` revalidated between applies, and a patch whose target moved is
  reported as a conflict rather than applied.
- Nothing is auto-merged, and each child's patch is previewed and approved
  individually with the requesting agent named.
- Checks rerun over the combined state, and a combination that breaks a check is
  reported as **failed** — asserted with a constructed case where one child
  renames a symbol and another references the old name, and each child's own
  checks passed.
- A child's check whose recorded hashes no longer match after integration is
  marked stale rather than counted as passing.
- Integration repair goes to the parent, not back to the children.
- A failed child releases its claim, returns a partial result, and does not fail
  its siblings or discard their completed work.
- A child that fails after its patch was integrated leaves that patch in place,
  with the failure reported and rewind offered rather than performed.
- Cancelling the parent cancels every descendant; cancelling one child leaves
  siblings running.
- Cancellation does not abort an in-flight patch application or command.
- Recursive spawning stops at `agent_max_depth`, defaulting to 1, reports the
  refusal as a `Finding`, and does not fail the requesting agent.
- A dependency cycle is refused at declaration.
- Concurrency respects `max_concurrent` and the parent's remaining budget.
- There is no agent-to-agent message channel, and a child's output entering
  another child's input does so as a bounded context item framed as a report —
  asserted by test that instruction-like text in a child result changes no
  policy outcome.
- Every spawn, claim, refusal, conflict, integration, and cancellation is
  audited with agent and parent task ids.
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no regression and no
  increase in approval-policy violations.

## 7. Implementation Notes

To be completed during implementation. Record:

- How often the conservative overlap rule (§5.1) refused a spawn that would in
  fact have been safe. A high rate means path scopes are being declared too
  broadly by the model, which is a prompt problem rather than a reason to relax
  the rule.
- How often the combined-check rerun (§5.5) caught a failure the children's own
  checks missed. If it never fires, that is worth knowing before someone
  proposes removing it as redundant — and if it fires often, multi-agent editing
  is riskier than it looks and [spec 40](40_autonomy_evaluations.md) should say
  so.
- The `max_concurrent` default and the measured memory cost per concurrent
  agent on a large repository.
- Whether `agent_max_depth` above 1 was ever measured to help.
