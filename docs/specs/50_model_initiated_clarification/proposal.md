# Feature Spec: Model-Initiated Clarification

Status: Done (2026-10-24; Tasks 1–8, started 2026-10-09). Split into a folder and
planned on 2026-10-07. Design
unchanged from the original flat spec. Corrections where it no longer matches
the code, and the decisions it leaves open, are in [`context.md`](context.md),
not inlined here, the way specs 20, 22, 24 and 31 kept theirs. The most
important ones:
- `PendingApprovalRef` is not the only untyped path: the paused-turn file
  store is keyed by an id string, and both resume entry points load whatever
  file that id names, so a typed `PendingDecision` alone does not close
  requirement 2;
- recovery fails any waiting task whose link it does not recognise, so a
  question-waiting task is failed on restart until recovery learns the new
  link;
- no "headless" flag exists, so availability becomes a turn option that
  defaults to *no user present*;
- nothing in CI or the quality gate runs doctests, so the "compile-fail test"
  is a `const` assertion that breaks the build, not a doctest.

Read `context.md` before starting any task in [`tasks.md`](tasks.md).
Order: 50 of 53
Plan: `docs/PLAN/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 8 (Should). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#20](../20_working_modes/proposal.md) (modes) — built;
[#21](../21_task_plan_progress_and_budget/proposal.md) (the plan gate and turn
budget) — built. Everything else named below is a cross-reference, not a
prerequisite.
Related implementation specs:
[`03_structured_tool_calling.md`](../03_structured_tool_calling.md) (owns the tool
surface this adds to),
[`10_persistent_command_approval.md`](../10_persistent_command_approval.md) and
[`34_repository_config_trust_boundary.md`](../34_repository_config_trust_boundary.md)
(own the approval path this must never become),
[`17_durable_task_state_and_crash_recovery/proposal.md`](../17_durable_task_state_and_crash_recovery/proposal.md)
(owns the waiting state and reattach machinery this reuses),
[`20_working_modes/proposal.md`](../20_working_modes/proposal.md) (the capability boundary this sits
inside), [`21_task_plan_progress_and_budget/proposal.md`](../21_task_plan_progress_and_budget/proposal.md)
(owns the plan review gate and the budget this counts against), and
[`29_memory_creation_and_consent.md`](../29_memory_creation_and_consent.md) (the
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
is a new turn that re-reads everything. [Spec 21](../21_task_plan_progress_and_budget/proposal.md)
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
  tool-call surface from [spec 03](../03_structured_tool_calling.md). The current
  set is `read_file`, `search_codebase`, `run_command`, `propose_patch`,
  `read_git_diff`, `read_git_status`, `inspect_web_page`, `run_web_scenario`,
  `propose_plan` and `complete_step`. None of them asks anything.
- **Waiting for a human already works.** `SessionStore::await_approval` sets
  `TaskStatus::WaitingForApproval` and records a `PendingApprovalRef { kind,
  proposal_id }` on the status event; `pending_approval_for` replays the log to
  find it, and a later status event without one clears it. This survives
  restart — [spec 17](../17_durable_task_state_and_crash_recovery/proposal.md)'s
  pending-approval reattach — and it is the machinery to reuse.
- **`PendingApprovalRef.kind` is a `String`.** Nothing in the type system stops
  a new kind of pending thing from being stored there, which is precisely the
  hazard §5.2 addresses.
- **The plan review gate is the nearest existing thing.**
  [Spec 21](../21_task_plan_progress_and_budget/proposal.md) pauses before the
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
- **Replacing the plan review gate.** [Spec 21](../21_task_plan_progress_and_budget/proposal.md)'s
  gate is engine-driven and fires whether or not the model wants it. This tool
  does not weaken it, substitute for it, or let the model skip it by asking a
  question instead.
- **Free-form conversation mid-turn.** One question, one answer, one tool
  result. A dialogue is a turn.
- **Asking the user to choose between patches.** Diff review already does that
  ([spec 04](../04_hunk_level_patch_apply.md)), and routing it through a question
  would bypass the review surface.
- **Remembering answers across turns or sessions.** That is memory
  ([spec 28](../28_memory_model_and_storage.md) onward) and has its own consent
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
discipline — the pattern [spec 29](../29_memory_creation_and_consent.md) uses to
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
[spec 17](../17_durable_task_state_and_crash_recovery/proposal.md)'s kill matrix,
already reattached on restart, and already understood by
[spec 45](../45_crash_recovery_prompt.md)'s card. A new state would add three
matrix cells and a recovery classification to distinguish two things that behave
identically — the task is stopped, waiting for a human, with no side effect in
flight. What is waiting is named by the `PendingDecision`, which is where the
distinction belongs. The recovery card names it accordingly: "A question was
waiting for your answer", not "An approval was pending".

### 5.3 Budget and pacing

At most **two** questions per turn, and each consumes a tool round from the
same budget every other tool call draws on ([spec 21](../21_task_plan_progress_and_budget/proposal.md),
and the continuation budget in [spec 47](../47_agent_working_capability/proposal.md)). A
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

[Spec 20](../20_working_modes/proposal.md)'s mode gate is the mechanism: the tool is
read-only and available in every mode, and the headless case is an execution
context that narrows the offered set, never a mode that widens one.

### 5.5 Untrusted text

The question is written by a model, and in a session carrying repository content
or fetched pages ([spec 51](../51_external_reference_retrieval.md)) it may be
repeating text from either. It is therefore rendered as untrusted content on the
same terms as any other model output: no markup interpretation that could
produce a control, no link that navigates anywhere on its own, and the
`SecretScanner` applied before it is displayed or persisted — a question that
quotes a line of a config file must not be the path by which a key reaches the
transcript unredacted.

The dialog is visually a *question*, distinct from the approval surface
([spec 41](../41_ui_density_and_action_hierarchy/proposal.md)'s button scale), and
it never uses the approval card's shape or its primary-button treatment. A user
who learns to click through questions must not have learned to click through
approvals.

### 5.6 Persistence

Two events, following [spec 17](../17_durable_task_state_and_crash_recovery/proposal.md)'s
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
  [`docs/UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md).
- A reloaded session renders the question and answer in the transcript in place.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

The execution order, the per-task deviations, the mutations that falsify every
load-bearing test, and the scoped check results are in [`tasks.md`](tasks.md).
Where each layer landed:

- **Types and store (Task 1).** `crates/workspace-engine/src/clarification.rs`
  holds the bounds (question 300 characters, 2–4 options, option 60, answer
  2,000), `AskUserInput`, `validate`/`validate_answer`, `PendingQuestionRef`,
  `PendingDecision` and `QuestionState`. `session.rs` gained `await_answer`
  (which writes `pendingQuestion`, never `pendingApproval`),
  `pending_decision_for`, `record_question_asked`/`_answered`, `question_state`
  and `questions_asked_in_task`, and `read_messages` renders the two events as
  the `question` and `answer` roles. §5.2's compile-fail test is two `const`
  assertions at the bottom of `clarification.rs`: adding a `From` conversion for
  `PendingQuestionRef` or `PendingDecision` fails the build, on every build,
  because nothing here runs doctests (`context.md` §5).
- **Cap (Task 2).** `agent_max_questions_per_turn` is a config Preference,
  default 2, with a hard ceiling of 4 enforced at parse rather than clamped. A
  profile may not carry it, the same as `agent_max_tool_rounds`. The count is
  the task's `question_asked` events, so a resumed turn counts what was already
  asked instead of starting from zero.
- **Offering (Task 3).** `ChatTurnOptions.user_present` defaults to `false`
  through `#[serde(default)]`. The desktop shell sets it to `true`; the CLI, the
  eval harness and every other driver leave it false and are offered no
  `ask_user` at all. The tool is pushed after `complete_step` when a user is
  present and the cap is above zero, and is allowed in all four modes. A call
  that arrives unoffered is refused in the per-call loop as a tool result saying
  no user can answer, and the turn continues. The system prompt is
  byte-identical when a user is present and gains a short headless paragraph
  when there is none.
- **Pause and resume (Task 4).** The question pause copies the command pause:
  it saves a `PendingChatTurn` with the matched tool call and both round
  counters, finishes the marker `awaiting_answer`, and ends the turn with
  `question_proposal`. The task goes to `await_answer` with audit status
  `question_pending`, never `await_approval`. `PausedFor` now names six kinds
  (`Action`, `PlanReview`, `Question`, `ProviderFallback`, `RoundBudget`, and
  `Conflicting` for a file with two markers), and every resume calls one shared
  `refuse_unless_paused_for`; a question id cannot resume a command, a plan, a
  fallback or a round budget, and their ids cannot be answered.
  `resume_after_answer` validates the answer, records it, replays the paused
  tool call on its round's assistant message, appends the fixed
  `The user answered: "<answer>" (option <n>: "<label>")` tool result, and
  recurses with `round + 1` and the unchanged exploration count. An
  already-answered question uses the recorded answer and is never re-asked.
  Later calls in the paused round are not dispatched, exactly as a command pause
  leaves them; the resumed turn asks again rather than replaying them.
  `has_pending_chat_command` is false for a question.
- **Redaction.** The question and its options are redacted before they are
  logged or shown, then revalidated because redaction can lengthen them past a
  bound. The user's answer is redacted on the way in because it becomes an
  engine-authored tool result that leaves the protected boundary. `context.md`
  §7 asked which path redacts the turn's own prompt: **none does** — a user's
  prompt is stored and sent as typed, as it was before this spec — so the answer
  is stricter than the prompt, not equal to it.
- **Recovery (Task 5).** `recovery.rs` reads `pending_decision_for`; the approval
  arms are unchanged and the new `ReattachedApproval::Question` is built from
  `question_state`, carrying the question, options and recorded answer. An open
  question comes back unanswered, an answered one keeps its answer, and a link
  whose question is absent fails the task with the reason, as a missing proposal
  does. The engine-authored headline is "A question was waiting for your
  answer". The enum kept its name (`context.md` §3 allowed either; renaming a
  display record was not worth the churn).
- **Shell and UI (Task 6).** `POST /api/answer-question-stream` resumes through
  `resume_after_answer` and refuses any non-question pause before it builds a
  model adapter or reads a key. `chat_result_json` carries `questionProposal`
  with no `proposalId` or `requiresApproval`, so no approval route has anything
  to act on. `/api/session` joins each `question` message to `question_state` so
  a reopened card still carries its options and `allowFreeText`; without that a
  reloaded card could not be answered when free text is off. `app.js` renders
  one `.question-card` for a live proposal, a transcript question and a
  reattached question: `textContent` only, secondary `.btn-sm` options, no
  `.btn-primary`, nothing focused or pre-selected, and an answered card shows
  the answer and disables itself. `docs/UI_STYLE_GUIDE.md` §5 and
  `docs/ui-style-guide.html` gained the "Question surface" entry and specimen.
- **Evaluation (Task 7).** A scenario may declare `answers`; a non-empty list
  makes the run interactive (`user_present: true`) and the runner answers each
  question in order by option label, otherwise as free text, failing the run if
  the model asks for more answers than are scripted. `answered_question` asserts
  the engine's exact tool result reaches the next request and the task completes.
  `awaiting_answer` counts as a clean human pause, not a tool error. The harness
  guards moved from 22 to 23 scenarios.

**The live usage measurement was not run.** Proposal §7 asks how often a real
model asks when the tool is available; the number is **unmeasured**, and this
section deliberately records no figure rather than implying one. The
deterministic tier cannot supply it: its model asks only because a script tells
it to. Task 7 added the manual measurement as
`crates/eval-harness/tests/harness.rs::live_tier_measures_ask_user_use`, an
`#[ignore]`d test that gives every scenario two in-memory answers, prints the
`ask_user` calls that ended `awaiting_answer` per scenario, and asserts no rate.
The command that would measure it:

```text
DAMAIAN_EVAL_PROVIDER=deepseek DAMAIAN_EVAL_MODEL=deepseek-v4-flash \
  cargo test -p eval-harness --locked -- --ignored --nocapture \
  live_tier_measures_ask_user_use
```

Two things that run would also need to be read with care: it spends money and
calls a real provider, and its answers are generic free text, so a real
question that offers fixed options refuses them and reports no figure for that
scenario. A run that never asks, and one that always asks, are both outcomes for
a reader to judge — the test does not. For the same reason, whether two
questions per turn was the right bound, and whether any scenario's outcome
measurably improved, are **not settled by evidence here**. Two is a design
decision backed by the refusal test, not a measured optimum, and the
deterministic scenario proves the mechanics rather than a quality gain; the
live A/B that would answer either question remains to be run.

### Acceptance criteria, and how each is covered

Each criterion from §6, the test that covers it, and an honest verdict.

| §6 criterion | Covered by | Verdict |
|---|---|---|
| The model asks mid-turn and receives the answer as a tool result, with the turn continuing, end to end against a scripted model | `workspace-engine/tests/clarification.rs::a_question_is_answered_and_the_turn_continues`; `eval-harness/tests/harness.rs::a_scripted_answer_reaches_the_model_and_the_turn_completes` with `scenarios/answered_question.toml` | **Met.** The engine test drives the pause, the answer and the resumed request through `MockModelAdapter`; the harness test asserts the exact tool result in the next request and a `complete` task over two model calls. |
| No approval call site accepts a question, asserted by the absence of any conversion and by a compile-fail test | The two `const` assertions in `clarification.rs`; `a_question_link_is_not_an_approval_link`; `a_question_id_cannot_resume_a_command_a_plan_a_fallback_or_a_round_budget`; `a_command_or_plan_id_cannot_be_answered`; shell `the_answer_route_refuses_a_command_proposal_id` and `run_command_with_a_question_id_runs_nothing` | **Met.** The compile-fail half is a build-time `const` assertion, not a doctest (`context.md` §5); Task 1 falsified it by adding a `From` impl and confirming `E0080`. The runtime half is asserted at every resume entry point and at both shell routes, because `context.md` §2 found the paused-turn store to be the real untyped path. |
| A command, a patch and a configuration change each still require their own approval after a question has been answered | `an_answer_never_authorises_a_command`, `an_answer_never_authorises_a_patch`, `a_configuration_change_still_needs_its_own_approval` | **Met.** The configuration criterion is asserted both ways a model can change configuration — a command writing `.damaian/config.conf` and a patch proposing one — because the product has no separate configuration approval route. |
| A crash while a question is open resumes with it pending and unanswered; a crash after it is answered resumes without re-asking | `crash_recovery.rs::a_question_open_at_a_crash_is_reattached_unanswered`; `a_question_answered_before_a_crash_is_not_asked_again`; `clarification.rs::an_answered_question_resumes_with_the_recorded_answer_and_is_not_asked_again`; shell `a_reattached_question_is_offered_for_an_answer_not_an_approval` | **Met,** with the "resumes" half split across two files. Recovery reattaches both states in the kill matrix's shape; the store refuses a second answer; the engine test models the same crash window and resumes with the recorded answer. |
| A third question in one turn is refused with a tool error the model can read | `a_third_question_in_one_turn_is_refused_with_a_tool_error` | **Met.** With the cap at 2, the third call's result names the cap ("already asked 2 of 2 questions"), the task completes, only two questions are recorded, and no pause is left. The test also proves the resumed turn counts the questions already asked. |
| The tool is absent from the tool list in a headless context, asserted against the emitted definitions | `ask_user_is_not_offered_without_a_user` and `a_cap_of_zero_means_ask_user_is_not_offered` (both read `adapter.requests[0].tools`); `harness.rs::a_scenario_without_scripted_answers_runs_headless` | **Met.** The assertions are on the emitted `tools` array, not on behaviour after a call; the harness test additionally proves an emitted call is refused and nothing waits. |
| A question containing a secret-shaped string is redacted before display and before the session log | `a_secret_in_a_question_is_redacted_before_the_log_and_the_result`; Task 4 mutation 4 | **Met** for the question, its options and the model-visible result: the payload, the log and every request are asserted free of a GitHub-token-shaped secret. The answer's own redaction path is implemented but not separately covered by a secret test. The user's own prompt is not redacted and is not claimed to be. |
| The question surface is visually distinct from the approval surface, per the style guide | No automated test. Task 6's manual browser check on port 4898 with a separate data directory; `docs/UI_STYLE_GUIDE.md` §5; `docs/ui-style-guide.html`; `style.css` `.question-card` | **Met by manual check only.** In the running app: 0 `.btn-primary`, no `.command-approval` class, `document.activeElement` was `BODY`, and a command pause and a question pause in one conversation shared no chrome. There is no automated test of CSS/JS visual distinctness; the shell tests stop at the payload. |
| A reloaded session renders the question and answer in the transcript in place | Shell `a_reopened_session_carries_the_question_and_answer_messages_in_order`; engine `questions_appear_in_the_transcript_in_order`; Task 6's manual reload check | **Met,** with the rendered card verified manually. The payload and order are automated; the card itself was checked by reloading a seeded paused question in the browser, which came back open and answerable. |
| Every quality-gate command from `AGENTS.md` passes | The CI **Quality** workflow on the commit | **Left to CI, not claimed here.** Per `AGENTS.md`'s policy since 2026-10-15 the seven-command gate does not run locally; the scoped checks that did run are recorded in [`tasks.md`](tasks.md)'s Task 8 row. |

The docs this spec owed are in place: `docs/USER_GUIDE.md` describes the card,
that answering authorises nothing, that a question may be left unanswered, that
`ask_user` exists and that it appears only in the desktop app;
`docs/TROUBLESHOOTING.md` tells a waiting question from a waiting approval in
the session log and names the two question events; `AGENTS.md` states the
invariant under Security boundaries (`context.md` §10 found no tool-surface
section to add it to); and the `CHANGELOG.md` `Unreleased` entry is gone.
