# Task Plan, Progress, and Budget — Context

Background for [`proposal.md`](proposal.md): why this work exists, what the code
looks like today, and the corrections found while planning it. The proposal is
the decision; this is the ground it stands on.

## 1. Motivation

Damaian has no representation of work larger than a turn.

[Spec 08](../08_stop_and_progress.md) made a turn stoppable and gave it distinct
UI states, and that was the right scope for the problem it solved. But a real
task — "add retry handling to the upload client and cover it with tests" — is a
sequence of steps whose intermediate state is currently invisible and
unrecoverable. The user sees a spinner and a stream of tool calls. If it goes
wrong at step four, there is nothing that says steps one through three happened,
and nothing to resume.

Two consequences follow, and the second is the more serious.

**Work is unbounded in the dimension that costs money.**
`agent_max_tool_rounds` bounds the loop by round count, which is a poor proxy:
one round that resends a 100k-token context costs more than twenty rounds that
read three small files. [Spec 19](../19_token_and_cost_accounting/proposal.md)
makes spend visible; without a ceiling, visible is all it is.

**Completion is asserted, not demonstrated.** The model says it is done, and
Damaian relays that. There is no distinction between "the test passed" and "the
model believes the test would pass". That distinction is the whole value of the
completion report, and it cannot exist without steps that carry evidence.

## 2. Current state

Verified against the tree on 2026-09-11, after specs 16, 17, 18 and 19 landed.
Line numbers are a reading aid and drift; the symbol names are the durable part.

- **No plan or progress model exists.** [Spec 08](../08_stop_and_progress.md)
  delivered turn-level cancellation and UI states. There is no multi-step
  representation, no step status, and nothing to recover after restart beyond
  `TaskStatus`.
- **`TaskStatus` has thirteen variants** (`session.rs:32-46`), not the seven the
  proposal was written against — see §3.2.
- **A budget-stop shape already exists**, but it does not have the shape the
  proposal assumes. `agent_max_tool_rounds` is enforced through `force_final`
  (`chat.rs:947`) and produces `tool_budget_exhausted_response`
  (`chat.rs:2346`) and `TaskStatus::ToolBudgetExhausted` (`chat.rs:1570`) — see
  §3.3.
- **Related bounds**: `agent_tool_retry_limit` and
  `agent_web_debug_max_tool_rounds`, both in the "Free" block of
  `apply_overlay_scoped` (`config.rs:740-762`) and so settable by a repository.
- **Sessions are an append-only event log** with replay-based readers
  (`read_task_statuses`, `session.rs:423`), carrying a monotonic `seq` since
  spec 17 and a cached `last_seq` so an append does not rescan the log.
- **Token accounting exists and is validated.** `read_task_usage`
  (`session.rs:681`) sums `task_usage_recorded` events per task, marking a total
  `Estimated` when any contributing run was. Spec 19's live run against DeepSeek
  on 2026-09-10 confirmed the measured path end to end.
- **Action markers exist and are paired.** `start_action` / `finish_action`
  (`session.rs:747`, `:818`) bracket every tool dispatch with a `markerId`, and
  `dangling_actions` (`session.rs:844`) reports what a crash interrupted.
- **Evidence sources exist, unstructured.** `CommandExecution` carries
  `exit_code: Option<i32>`, `stdout`, `stderr` (`command_runner.rs:11-22`), with
  `exit_code` coming straight from `output.status.code()` — `None` for a
  signalled command. `ProposedFilePatch` carries `base_hash` and the patch
  engine computes an `applied_hash` (`patch_engine.rs:93`).

## 3. Corrections found while planning

Eight statements in the proposal describe behaviour the code does not have, or
rest on an assumption the code contradicts. Each is accounted for in
[`tasks.md`](tasks.md); none should be "fixed" back to the proposal's original
wording.

### 3.1 A task is one turn, so "per task" means "per turn"

`create_task` is called in exactly two production places — `chat.rs:487` and
`edit.rs:225` — both once per user prompt, and the turn ends by writing a
terminal status. There is no object in the system that spans turns except the
session.

This is load-bearing three times over:

- **The plan lives inside one turn**, bounded by `agent_max_tool_rounds` rounds
  (default 8). "Step" means a step of the agent loop, not a step of a
  multi-turn project. The proposal's motivating example — "add retry handling
  and cover it with tests" — is a plausible single turn at that round budget,
  but only just.
- **The ceiling resets every turn.** Five turns under a 100k ceiling can spend
  500k. §4 of the proposal rules out per-session budgets deliberately, so this
  is a choice rather than an oversight — but it is a much weaker guarantee than
  "a per-task token ceiling" sounds, and the user guide has to say so in plain
  words rather than let the reader assume otherwise.
- **Resumption crosses a task boundary.** §5.4 says the stop is recoverable:
  "the user can raise the ceiling and resume". A resumed turn is a *new* task
  with a new id, so `read_task_plan(session_id, task_id)` will not find the
  plan that was persisted, and the new task's usage starts at zero. Carrying a
  plan across that boundary is a mechanism the proposal does not specify and
  the plan below adds.

### 3.2 `TaskStatus` has thirteen variants, and three places must learn the new one

The proposal says seven, "extended to twelve by spec 17". Spec 17 landed and
the count is thirteen: `Created`, `PreparingContext`, `WaitingForModel`,
`RunningTool`, `WaitingForApproval`, `ApplyingPatch`, `Validating`, `Complete`,
`Failed`, `Cancelled`, `ToolBudgetExhausted`, `Interrupted`,
`UnknownExternalOutcome`.

Adding `TokenBudgetExhausted` is not one edit. `TaskStatus::all()`
(`session.rs:53`) exists precisely so a new variant makes the state tests fail
until it has been given a terminality and a side-effect answer, so the compiler
plus those tests will catch `as_str`, `all`, `is_terminal` and
`may_have_side_effect_in_flight`. What neither catches is
`update_task_status` (`session.rs:301-310`), which decides whether to stamp
`completed_at_ms` from a **hand-written `matches!` listing the four terminal
statuses**. A new terminal status omitted there gets no completion timestamp
and nothing fails. That one needs a test of its own.

### 3.3 The `tool_budget_exhausted` pattern cannot be followed "exactly"

§5.4 says enforcement "follows the existing `tool_budget_exhausted` pattern
exactly". It cannot, and following it would defeat the purpose.

`force_final` (`chat.rs:947`) does not stop the loop. It makes **one more model
call** with `tools` dropped, and flags exhaustion only if the model still asks
for a tool anyway (`chat.rs:1121-1127`). For a round ceiling that is correct
behaviour: you spend your last round getting an answer out of the evidence
already gathered, which is what the user wanted the rounds for.

For a money ceiling it is backwards. Crossing the ceiling would trigger one
further call, and because context grows monotonically across rounds that call is
the most expensive one of the turn. A ceiling whose enforcement action is "spend
more than the ceiling" is not a ceiling.

The token check therefore stops **before** the call, not by forcing a final one.
What it shares with the round pattern is the plumbing — a boolean carried out of
the loop, a distinct `TaskStatus`, a distinct audit status string — not the
control flow.

### 3.4 The session log records no failure outcome, so evidence cannot be read from it today

Requirement 6 wants a step's status to be a function of observed evidence. The
observation the engine already makes is the `action_finished` outcome. It is
useless for this purpose as written: **every tool arm converges on a single
`finish_action(action_marker, "ok")`** (`chat.rs:1507`), regardless of what the
tool reported. A command that exited non-zero, an MCP call that returned
`is_error`, a browser diagnostic that failed — all three record `"ok"`.

This is not a new discovery so much as a confirmed one: spec 18's live run found
`tool_and_model_error_rate` reporting 0.000, and the root cause was exactly this
— the metric's failure value and its expected value were the same number,
because no failure outcome exists to count.

The fix is local. In the command arm the exit code is in hand at the call site
(`record.execution`, `chat.rs:1262`), one statement before the marker is
finished. But it is a **prerequisite**, not a detail: without it there is no
evidence to build requirement 6 on, and a plan step would be marked complete
from the same "ok" a failed command produces.

### 3.5 Evidence must carry the fact, not a pointer into a store that expires

`Evidence::CommandExit { r#ref: String, exit_code: Option<i32> }` reads as though
`ref` is the evidence and `exit_code` a convenience. It is the other way round.

A `CommandExecution` id reaches the audit log (`commandId`, `validation.rs:217`)
and a validation artifact directory. It never reaches the session log. The audit
log is a separate store on its own retention clock (`audit_retention_days`), so
a plan read back after that window would carry references resolving to nothing —
and a step's recorded status would then rest on a dangling pointer.

The durable anchor already exists: spec 17's `markerId`, written to the session
log on both `action_started` and `action_finished`, already paired by
`dangling_actions`, and already read by the eval harness
(`eval-harness/src/trace.rs`). Evidence keys on the marker and carries the exit
code **inline**. The reference is a breadcrumb for a human reading the log; the
value is the evidence. Anyone later tempted to normalise the value away in
favour of the reference should read this paragraph first.

### 3.6 Two design elements reference specs that do not exist yet

- **`Evidence::Findings { refs: Vec<String>, failing: usize }`** references
  finding ids from [spec 22](../22_findings_model_and_panel.md), which is Not
  started. A variant holding ids from an id space with no producer is a field
  that can only ever be empty. It is **deferred** — added when spec 22 lands,
  with the enum marked `#[non_exhaustive]` so adding it later is not a breaking
  change for the shell.
- **The spec-23 acceptance criterion** — "the end-to-end fixture from
  [spec 23](../23_verification_loop.md) exercises a plan through to a completion
  report" — is unsatisfiable: spec 23 is Not started. It is **restated** against
  [spec 18](../18_local_evaluation_harness/proposal.md)'s harness, which is Done,
  runs in CI deterministically, and has run against a live provider. That is a
  stronger gate than waiting for a spec that has not been written.

The §5.3 status table's other spec-22 reference ("the failure becomes a
finding") stays as written — it describes what happens *after* spec 22, and the
`blocked` status it assigns is available now.

### 3.7 `PhaseKind` is not `TaskPhase`

`PhaseKind` (`chat.rs:43`) has four variants — `Context`, `Model`, `Tool`,
`Finalizing` — and answers "which stage of the agent loop is executing right
now". It drives the spinner so a user can tell a slow provider from a running
tool from a hang.

Requirement 3's phase — understanding, planning, editing, validating, reviewing,
complete — answers "what is this work about". They are orthogonal axes: a
`Model` phase occurs during every one of the six. Extending `PhaseKind` with
`Editing` would produce a type whose variants are not mutually exclusive and a
spinner that lies. Two types, and `TaskPhase` derived from step state as §5.1
requires.

### 3.8 The config surface is eight places, and the scope class is not the obvious one

Adding one key means eight edits: the `Config` field, the `ConfigOverlay` field,
the destructuring in `apply_overlay_scoped`, the apply block, the key parser
(`config.rs:1494`), `config_show`'s line list (`config.rs:1233`), the overlay
serializer (`config.rs:1723`), and `Config::default` (`config.rs:1303`). The
destructuring is exhaustive with every binding used — spec 19 added that
deliberately after a silently-dropped field — so the compiler catches most of
these. It does not catch `config_show` or the serializer.

The scope class is the real decision. The three existing `agent_*` bounds sit in
the block commented "Free: preferences and budgets, with no capability behind
them" (`config.rs:740`), applied without the `scoped()` filter, so a cloned
repository can set them.

`agent_max_task_tokens` does not belong there. A repository that *lowers* a
ceiling is a nuisance; a repository that *raises* one the user deliberately set
low spends the user's money, which is a capability, and spec 34 exists to stop
exactly that. The shape already in the file is `restrict_only_flag`
(`config.rs:654`): either scope may tighten, only a trusted scope may loosen.
The ceiling is restrict-only — a repository may lower it or set one where none
existed, never raise one and never remove one.

That `agent_max_tool_rounds` is Free today is arguably the same hole one step
removed. It is left alone here: rounds are not money, changing its class would
break configurations that rely on it, and widening this work package to
re-litigate spec 34's classifications is how a work package stops shipping.

### 3.9 Absent usage means zero for a ceiling, and "not recorded" for a report

`read_task_usage` deliberately **omits** a task with no usage events rather than
returning a zero, because "not recorded" and "used nothing" are different facts
and spec 19's display surfaces must not conflate them.

The ceiling inherits the lookup but not the rule. For enforcement, absent means
nothing has been spent yet, which is arithmetically zero and must not be an
error or a refusal to enforce. This asymmetry is easy to get wrong in the
direction that matters: treating absent as "cannot evaluate" would make the
ceiling silently inoperative for the first call of every turn, which is the only
call some turns make.
