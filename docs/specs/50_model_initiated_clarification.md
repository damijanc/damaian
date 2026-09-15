# Feature Spec: Model-Initiated Clarification

Status: Not started
Order: 50 of 53
Plan: `docs/PLAN/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 8 (Should). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#20](20_working_modes.md) (modes) — **not built**;
[#21](21_task_plan_progress_and_budget/proposal.md) (the plan gate and turn
budget) — built. Everything else named below is a cross-reference, not a
prerequisite.
Related implementation specs:
[`03_structured_tool_calling.md`](03_structured_tool_calling.md) (owns the tool
surface this adds to),
[`10_persistent_command_approval.md`](10_persistent_command_approval.md) and
[`34_repository_config_trust_boundary.md`](34_repository_config_trust_boundary.md)
(own the approval path this must never become),
[`17_durable_task_state_and_crash_recovery/proposal.md`](17_durable_task_state_and_crash_recovery/proposal.md)
(owns the waiting state and reattach machinery this reuses),
[`20_working_modes.md`](20_working_modes.md) (the capability boundary this sits
inside), [`21_task_plan_progress_and_budget/proposal.md`](21_task_plan_progress_and_budget/proposal.md)
(owns the plan review gate and the budget this counts against), and
[`29_memory_creation_and_consent.md`](29_memory_creation_and_consent.md) (the
type-separation pattern §5.2 follows).

## 1. Motivation

The only way the engine can currently pause and ask the user anything is to ask
for **permission**. There is no way to ask a **question**.

So when a task is ambiguous — two plausible interpretations of "clean this up",
a package that could live in either of two crates, a migration that could be
destructive or additive — the model has exactly two options. It can guess, and
spend the rest of the turn building the wrong thing; or it can stop and write a
paragraph asking, which ends the turn and discards the context it just spent
several rounds assembling.

Both are expensive in a way that is easy to measure and hard to notice. A wrong
guess is discovered at review time, after the tokens are spent, and the recovery
is a new turn that re-reads everything. [Spec 21](21_task_plan_progress_and_budget/proposal.md)
made this sharper rather than softer: a turn now has an enforced token ceiling,
so rounds spent on a misread instruction are rounds the correct work no longer
has.

A ten-second answer to "did you mean the CLI or the shell?" is the cheapest
token in the system. Every mainstream agent tool has converged on some version
of this, and the reason is not ergonomics — it is that a question asked at round
three costs one round, and the same question answered wrongly at round three
costs the turn.

## 2. Current State

- **Tools are the only way the model reaches the outside**, through the native
  tool-call surface from [spec 03](03_structured_tool_calling.md). The current
  set is `read_file`, `search_codebase`, `run_command`, `propose_patch`,
  `read_git_diff`, `read_git_status`, `inspect_web_page`, `run_web_scenario`,
  `propose_plan` and `complete_step`. None of them asks anything.
- **Waiting for a human already works.** `SessionStore::await_approval` sets
  `TaskStatus::WaitingForApproval` and records a `PendingApprovalRef { kind,
  proposal_id }` on the status event; `pending_approval_for` replays the log to
  find it, and a later status event without one clears it. This survives
  restart — [spec 17](17_durable_task_state_and_crash_recovery/proposal.md)'s
  pending-approval reattach — and it is the machinery to reuse.
- **`PendingApprovalRef.kind` is a `String`.** Nothing in the type system stops
  a new kind of pending thing from being stored there, which is precisely the
  hazard §5.2 addresses.
- **The plan review gate is the nearest existing thing.**
  [Spec 21](21_task_plan_progress_and_budget/proposal.md) pauses before the
  first mutating step for the user to review a plan. That is a fixed,
  engine-driven checkpoint, not a question the model chose to ask.
- **`complete_step` takes no arguments by design** (spec 21): the model asks to
  move on and the engine decides from what it observed. That is the design
  temperament this spec has to match — a model-facing tool that requests
  something rather than asserting it.

## 3. Requirements

1. A tool lets the model ask the user one bounded question mid-turn, with a
   small set of suggested answers, and receive the answer as a tool result
   without ending the turn.
2. **An answer to a question can never satisfy an approval.** No patch, no
   command, no network call, no configuration change may proceed because of
   anything returned by this tool.
3. The question and the answer are recorded in the session log and are visible
   in the transcript as what they are — a question the assistant asked and an
   answer the user gave.
4. A turn interrupted while waiting for an answer resumes with the question
   still pending, and never re-asks a question already answered.
5. Use is bounded per turn and each use costs a tool round, so the tool cannot
   become an interrogation.
6. Where no user can answer — headless or scheduled execution — the tool is
   unavailable, and the model is told so in terms that let it proceed under a
   stated assumption or stop.
7. The question is model-authored text and is treated as untrusted content
   everywhere it is displayed or stored.

## 4. Non-goals

- **Any widening of what the agent may do.** The tool returns a string. It
  gains the agent no capability, and requirement 2 is the constraint the design
  is built around rather than a caveat on it.
- **Replacing the plan review gate.** [Spec 21](21_task_plan_progress_and_budget/proposal.md)'s
  gate is engine-driven and fires whether or not the model wants it. This tool
  does not weaken it, substitute for it, or let the model skip it by asking a
  question instead.
- **Free-form conversation mid-turn.** One question, one answer, one tool
  result. A dialogue is a turn.
- **Asking the user to choose between patches.** Diff review already does that
  ([spec 04](04_hunk_level_patch_apply.md)), and routing it through a question
  would bypass the review surface.
- **Remembering answers across turns or sessions.** That is memory
  ([spec 28](28_memory_model_and_storage.md) onward) and has its own consent
  rules.
- **A timeout that answers for the user.** A question with no answer waits. An
  engine that picked a default after thirty seconds would be guessing with extra
  steps, and the user would never know it had asked.

## 5. Design

### 5.1 The tool

```rust
/// Arguments the model supplies.
pub struct AskUserInput {
    /// One question. Bounded length; rendered as untrusted text.
    pub question: String,
    /// 2–4 suggested answers. Short labels, not paragraphs.
    pub options: Vec<String>,
    /// Whether a free-text answer is accepted alongside the options.
    pub allow_free_text: bool,
}
```

The tool result is the user's answer as a string, plus which option it
corresponded to where one did. Bounds — question length, option count, option
length — are enforced at the schema boundary and a violation is a tool error,
not a truncation, so a model cannot smuggle a wall of text into a dialog by
exceeding a limit.

Options are required. A question with no options is an essay prompt, and the
value of this tool is that answering it takes one click.

### 5.2 A question is not an approval, and the types make that true

This is the load-bearing rule, and it is enforced by construction rather than by
discipline — the pattern [spec 29](29_memory_creation_and_consent.md) uses to
make unconfirmed persistence unrepresentable.

`PendingApprovalRef.kind` is a `String` today, so the cheap implementation is to
store a question as another kind and reuse the same reattach path. That is
exactly what must not happen: one `String` away from a question, and any code
that matches on "is something pending" treats "the user answered *Yes, the CLI*"
as "the user approved running this command".

Instead, the pending mechanism is generalised over a **typed** decision:

```rust
pub enum PendingDecision {
    Approval(PendingApprovalRef),
    Question(PendingQuestionRef),
}

pub struct PendingQuestionRef {
    pub question_id: String,
}
```

`PendingQuestionRef` carries no proposal id, because there is no proposal.
Nothing can convert one into a `PendingApprovalRef`; there is no `From`
implementation and no constructor that takes one. Every approval call site keeps
taking `&PendingApprovalRef` and therefore cannot be reached with a question —
the compiler enforces requirement 2 rather than a reviewer.

The waiting state is the existing `TaskStatus::WaitingForApproval`. Reusing it is
deliberate: it is already terminal-adjacent, already in
[spec 17](17_durable_task_state_and_crash_recovery/proposal.md)'s kill matrix,
already reattached on restart, and already understood by
[spec 45](45_crash_recovery_prompt.md)'s card. A new state would add three
matrix cells and a recovery classification to distinguish two things that behave
identically — the task is stopped, waiting for a human, with no side effect in
flight. What is waiting is named by the `PendingDecision`, which is where the
distinction belongs. The recovery card names it accordingly: "A question was
waiting for your answer", not "An approval was pending".

### 5.3 Budget and pacing

At most **two** questions per turn, and each consumes a tool round from the
same budget every other tool call draws on ([spec 21](21_task_plan_progress_and_budget/proposal.md),
and the continuation budget in [spec 47](47_agent_working_capability.md)). A
third call in a turn is refused with a tool error saying so, which the model can
read and work around by proceeding.

Two is a starting value, configurable, and §7 records what it should have been.
The reason for a small number rather than none: a turn that asks four questions
has not understood the task, and the useful failure there is the model
proceeding under a stated assumption — which is visible and reviewable — rather
than the user answering four dialogs and still not knowing what was decided.

Waiting time is not billed, because no call is in flight while the question is
open. A question that goes unanswered for a long time is a task sitting in
`WaitingForApproval`, which is a state the product already handles.

### 5.4 Where there is no user

Requirement 6. Availability is a property of the execution context, not a
runtime check inside the tool:

- Interactive session: available.
- Headless or CI execution (Phase 5 WP8), and any scheduled or unattended run:
  **not offered in the tool list at all.**

Omitting it beats offering it and failing, because a tool the model can see is a
tool it will plan around, and a plan whose third step is "ask the user" is a plan
that fails at step three in CI. Where the model would have asked, the system
prompt for those contexts instructs it to proceed under an explicitly stated
assumption and record it — which lands in the task report (Phase 5 WP7) as a
line saying what was assumed and why.

[Spec 20](20_working_modes.md)'s mode gate is the mechanism: the tool is
read-only and available in every mode, and the headless case is an execution
context that narrows the offered set, never a mode that widens one.

### 5.5 Untrusted text

The question is written by a model, and in a session carrying repository content
or fetched pages ([spec 51](51_external_reference_retrieval.md)) it may be
repeating text from either. It is therefore rendered as untrusted content on the
same terms as any other model output: no markup interpretation that could
produce a control, no link that navigates anywhere on its own, and the
`SecretScanner` applied before it is displayed or persisted — a question that
quotes a line of a config file must not be the path by which a key reaches the
transcript unredacted.

The dialog is visually a *question*, distinct from the approval surface
([spec 41](41_ui_density_and_action_hierarchy/proposal.md)'s button scale), and
it never uses the approval card's shape or its primary-button treatment. A user
who learns to click through questions must not have learned to click through
approvals.

### 5.6 Persistence

Two events, following [spec 17](17_durable_task_state_and_crash_recovery/proposal.md)'s
append-only rule:

```json
{"seq":301,"eventType":"question_asked","taskId":"task_…",
 "questionId":"question_…","question":"…","options":["…","…"]}
{"seq":318,"eventType":"question_answered","questionId":"question_…",
 "answer":"…","selectedOption":0}
```

Requirement 4 follows from the replay: a question with an answer event is
answered and is never re-asked; a question without one is still pending and
reattaches. This is the same shape `pending_approval_for` already uses, and it
gives spec 17's guarantee for free — an answered question is not a repeatable
action, and an unanswered one has no outcome to be unknown.

`read_messages` renders both events into the transcript so a reloaded session
shows the exchange in place, which is what requirement 3 asks for.

### 5.7 Documentation

`docs/USER_GUIDE.md`: what a question looks like, that answering one never
authorises anything, and that a question can be left unanswered. `AGENTS.md`
gains a line under the tool surface, since an agent working in this repository
now has one more tool available to it.

## 6. Acceptance Criteria

- The model can ask a question mid-turn and receive the answer as a tool result,
  with the turn continuing — asserted end to end against a scripted model.
- No approval call site accepts a question: `PendingQuestionRef` cannot be
  passed where `PendingApprovalRef` is required, asserted by the absence of any
  conversion and by a compile-fail test.
- A command, a patch and a configuration change each still require their own
  approval after a question has been answered — asserted by attempting each with
  only a question answered.
- A crash while a question is open resumes with the question pending and
  unanswered; a crash after it is answered resumes without re-asking —
  both asserted in the kill matrix's existing shape.
- A third question in one turn is refused with a tool error the model can read.
- The tool is absent from the tool list in a headless execution context —
  asserted against the emitted tool definitions, not against its behaviour when
  called.
- A question containing a secret-shaped string is redacted before display and
  before it reaches the session log.
- The question surface is visually distinct from the approval surface, per
  [`docs/UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md).
- A reloaded session renders the question and answer in the transcript in place.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation. Record:

- How often the model used the tool across the [spec 18](18_local_evaluation_harness/proposal.md)
  scenarios, and whether the questions were worth asking. A model that never
  asks and a model that always asks are both failures, and the number tells
  which one was built.
- Whether two questions per turn was the right bound.
- Whether any scenario's outcome measurably improved, since the justification for
  this tool is a quality claim and the harness is how a quality claim is settled.
