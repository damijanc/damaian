# Context: Model-Initiated Clarification

Background for [`tasks.md`](tasks.md). This file records where
[`proposal.md`](proposal.md) (the flat spec, unchanged in substance) no longer
matches the code, checked on 2026-10-07. It also records the decisions the
proposal leaves open. Read it before Task 1. Do not "fix" any of these back to
the proposal's wording without re-reading the code they cite.

The proposal names no source files. §1 is the map. §2–§5 are the four places
where the proposal's design, built as written, would leave requirement 2 or 4
open. §6 onwards records the decisions the proposal leaves to the implementer.

Line numbers drift: `chat.rs` alone is about 6,800 lines and other specs edit
it. Re-read a line before editing near it.

## 1. Where the code is

### Engine (`crates/workspace-engine/src/`)

**The turn loop, `chat.rs`**
- Entry: `ask_with_session_with_options` (`:669`). It reads the mode, builds
  `[system(system_prompt(mode)), user(build_model_prompt(..))]` (`:762–767`),
  then calls `run_agentic_turn` (`:769`).
- `run_agentic_turn` (`:1482`) captures the mode (`:1504`) and the profile
  capabilities (`:1507`) once per turn.
- **Tool list**: `native_tools` is built at `:1508–1637`, gated on
  `config.supports_native_tools()`. Every definition passes through the
  `offered` closure (`:1513`), which calls `mode::permits` with a placeholder
  `ToolAction`. Push order: `propose_patch` `:1550`, `propose_plan` and
  `complete_step` `:1556–1558`, the reads `:1560–1595`, web diagnostics
  `:1598–1609`, MCP `:1615–1635`.
- **Tool definitions**: one function each, `run_command_tool_definition`
  (`:3993`) through `run_web_scenario_tool_definition` (`:4095`).
- **Parsing**: `ToolAction` (`:3734–3777`, `pub(crate)`) with
  `tool_action_from_call` (`:4115`), a `match` on the tool name whose last arm
  becomes an MCP call (`:4316`). Exhaustive helpers over `ToolAction`:
  `action_effect` (`:3801`), `action_is_batchable_read_only` (`:3822`),
  `tool_action_marker` (`:3834`), `action_awaits_plan_review` (`:3886`) and
  `tool_action_label` (`:3952`). A new variant fails to compile until each one
  decides.
- **Per-call loop**: the outer `loop` runs `:1686–2963`, and the per-call
  `for` runs `:2144–2932`. `action_permission` (`:3150`) is checked at
  `:2162`. Spec 21's plan-review gate is `:2176–2234`. Then comes the
  sequential `match tool_action` (`:2310–2829`): Command `:2311`, ProposePlan
  `:2452`, CompleteStep `:2505`, and so on.
- **Budgets**: `ABSOLUTE_TOOL_ROUND_CAP = 16` (`:3578`) and
  `tool_round_limit` (`:467`). Spec 21's token ceiling is checked before each
  model call (`:1725–1756`), and `force_final` drops the tools at the round
  limit (`:1758–1766`).
- **Pausing**: every pause sets `terminal = Some((.., TurnProposals{..},
  StopReason::Answered))` and breaks. The command pause (`:2318–2365`) saves a
  `PendingChatTurn` with `matched_tool_call` and calls
  `finish_action(marker, "awaiting_approval")`.
- **After the loop**: `TurnProposals` (`:358`) becomes a `PendingApprovalRef`
  with kind `"command"`, `"patch"` or `"plan"` (`:2970–2994`), and the task
  goes to `await_approval` (`:2996`).
- **Resume**: `resume_after_command_decision_with_options` (`:805`) and
  `resume_after_plan_decision` (`:1114`). The plan one appends the decision
  as a message and recurses with `round + 1`. That is the shape to copy, with
  one difference: the answer goes back as a **tool** result (§7).
- **Paused-turn store**: `PendingChatTurn` (`:3423–3465`, every later field
  `#[serde(default)]`), `PendingCommandStore` (`:3526`, files at
  `<data_dir>/chat/pending/<id>.json`, `take` loads *and deletes*), and
  `PausedTurns` (`:3496`), the read-only view recovery uses.
  `has_pending_chat_command` is at `:1415`.
- **Redaction precedent**: model-authored plan titles and details are redacted
  at `:2474–2478`, and model output at `:1982`. `scanner` is a field of the
  orchestrator (`:379`).
- **Turn options**: `ChatTurnOptions { continue_debugging }` (`:366`), serde
  default, stored on `PendingChatTurn.turn_options`.
- **Test modules**: `mode_tool_list_tests` (`:5200`, helper
  `offered_tool_names`), `system_prompt_tests` (`:5514`), and
  `mode_refusal_tests` (`:5612`, helpers `turn`, `resume`, `call`,
  `calls_then_answer`, `pending_chat_turns`).

**Modes, `mode.rs`**
- `permits` (`:139`) checks `mode_permits` (`:228`) and then
  `profile_permits` (`:157`). Both are exhaustive over `ToolAction`.
- The matrix test is `the_permission_matrix_matches_the_spec_table` (`:365`).

**Session log, `session.rs`**
- `TaskStatus` (`:40`).
- `PendingApprovalRef { kind: String, proposal_id: String }` (`:219`) has no
  serde derive. `await_approval` (`:754`) writes it by hand into the
  `task_status_updated` payload as `"pendingApproval"`.
  `pending_approval_for` (`:770`) replays `active_events`, and a later status
  event without the key clears it.
- `read_messages` (`:541`) filters `message_appended`. `parsed_events`
  (`:2062`) ignores rewinds, while `active_events` (`:2089`) honours them.

**Recovery, `recovery.rs`**
- `reattach_pending_approvals` (`:290`) dispatches on `kind` (`:309`, `:318`,
  `:325`). Any other kind, and a missing link, **fails the task** (`:328`,
  `:375`).
- `ReattachedApproval` (`:253`), `headline` (`:421`), `action_subject`
  (`:445`), and `state_phrase` (`:469`), which reads "waiting for your
  approval".

**Other engine files**
- `secret_scanner.rs`: `SecretScanner::redact(&str) -> Redaction` (`:42`).
- `config.rs`: `agent_max_tool_rounds` is a **Preference**, applied with
  `preference(..)` (`:1331`). `agent_max_task_tokens` is restrict-only
  (`:1176`).
- `model.rs`: `MockModelAdapter::new_sequence_with_tool_calls` (`:402`) and
  `ToolCall { id, name, arguments_json }` (`:157`).

**Tests that pin the machinery this reuses**
- `tests/crash_recovery.rs`: the kill matrix from `:753`.
  `expected(status, shape)` is at `:834`, `cells == 42` at `:937`, and
  `a_pending_approval_survives_restart_with_its_proposal` at `:295`.
- `tests/recovery_prompt.rs`:
  `a_plan_review_interrupted_by_a_crash_is_re_presented` (`:376`).
- `tests/plan_turn.rs`: the scripted plan-gate tests (`scripted` `:104`,
  `decide` `:114`).

### Shell (`crates/desktop-shell/src/`)

- **Turns stream over SSE.** `handle_ask_stream` (`lib.rs:1328`) and
  `stream_turn` (`:3260`) feed `relay_turn_events` (`:3192`). The `done` event
  body is `chat_result_json` (`:3286`), which carries `commandProposal`,
  `patchProposal` and `planProposal`.
- **Resume routes**:
  - `/api/resume-command-stream` (`:820`, `run_resume_command_request`
    `:1378`)
  - `/api/resume-plan-stream` (`:821`, `run_resume_plan_request` `:1450`)
  - `/api/run-command` (`:1055`) and `/api/reject-command` (`:1131`). Both
    resume a chat turn whenever `has_pending_chat_command(id)` is true (§2).
- **Turn options**: built as a struct literal at `lib.rs:1576`. It is the
  only place outside the engine that constructs `ChatTurnOptions`.
- **Session payload**: `GET /api/session` (`:555`) returns `session_json`,
  `messages` (`message_json` `:3681`, role only) and `tasks`. **It has no
  pending-card field**, so a reopened session never shows a waiting card
  today. Only the crash-recovery sweep brings one back.
- **Recovery**: `recovery.rs` `run_sweep` (`:345`) and `describe_approval`
  (`:505`). Its tests: `a_reattached_plan_review_is_offered_for_review`
  (`:892`) and `an_answered_approval_is_not_reattached_again` (`:1065`).

### UI (`crates/desktop-shell/static/`)

- `app.js`:
  - `appendProposals` (`:5785`) dispatches to `createCommandApprovalPreview`
    (`:6349`), `createPatchPreview` (`:6012`) and `createPlanReview`
    (`:5804`).
  - Streams: `streamResumePlanRequest` (`:6668`), `streamRequest` (`:6672`)
    and `processSseEvent` (`:6712`).
  - Transcript: `renderMessages` (`:3611`) has no switch on message kind;
    every message goes through `appendChatMessage(role, content)` (`:3347`).
  - Recovery: `createReattachedApprovalCard` (`:3994`), whose headlines and
    notes are keyed by kind at `:4018–4034`.
- `style.css`: the button scale (`:193–273`), `.command-approval` (`:2727`)
  and `.recovery-card` (`:2863`).
- Style guides: `docs/UI_STYLE_GUIDE.md` §3 Buttons (`:93`), §4 Action
  hierarchy (`:140`) and §5 Approval surfaces (`:195`, "Nothing is
  pre-selected toward approval", `:209`). The specimens are in
  `docs/ui-style-guide.html`: Command approval (`:403`) and Crash recovery
  prompt (`:476`).

### Headless drivers

- **CLI**: `damaian-cli/src/main.rs` `ask` (`:385–441`) calls `ask` once,
  prints the response and ignores every proposal. There is no resume.
- **Eval harness**: `eval-harness/src/runner.rs` `drive` (`:249`) calls `ask`.
  It resumes only a command proposal, and only when the scenario has an
  `approval_decision` (`:286–304`).

## 2. The paused-turn store is the real untyped path, not only `PendingApprovalRef`

The proposal's §5.2 types the *link* on the status event. But a paused turn's
state lives in `PendingCommandStore`, keyed by a bare id string, and the two
resume entry points load whatever file that id names:

- `resume_after_command_decision_with_options` (`chat.rs:814`) takes the file
  and refuses it only if `plan_review.is_some()` (`:820`). A paused
  *question* turn would pass that check and fall through to the shell-command
  branch, which runs `last_content` as a command.
- `/api/run-command` and `/api/reject-command` route to that function
  whenever `has_pending_chat_command(id)` is true. So `POST /api/run-command`
  with a question's id would be "approve" for a turn that has nothing approved.

That is requirement 2 failing at runtime, through no conversion the compiler
can see.

**Decision (Task 4):** a paused turn says what it is paused for, and every
resume entry point checks it before doing anything else.

- `PendingChatTurn` gains `question: Option<PendingQuestion>`, `#[serde(default)]`,
  like `plan_review`.
- A private `fn paused_for(&PendingChatTurn) -> PausedFor { Action, PlanReview, Question }`
  is matched exhaustively by each of the three resumes. Each resume refuses the
  other two kinds and puts the file back, as `:820` does today. A fourth paused
  kind then fails to compile until every resume decides about it.
- `has_pending_chat_command` returns `false` for a question turn. A question id
  then reaches `/api/run-command` as a standalone proposal, and
  `run_proposal` fails because no command proposal has that id.
- Question ids are `question_<uuid>`, never a proposal-id shape. That is
  belt-and-braces, not the guard.

Acceptance criterion "a command, a patch and a configuration change each still
require their own approval after a question has been answered" is tested at
these entry points (Task 4), not only through the scripted model.

## 3. Recovery fails a task whose link it does not recognise

`reattach_pending_approvals` (`recovery.rs:290`) treats a waiting task with no
`pendingApproval` link, or with an unknown kind, as unrecoverable and **fails
it** (`:328`, `:375`). The proposal's §5.2 keeps `WaitingForApproval` and moves
the "what is waiting" fact to a typed `PendingDecision`. Built as written, every
open question would be failed on the next restart. That is requirement 4
inverted.

**Decision (Task 5):**
- Recovery reads `SessionStore::pending_decision_for` (Task 1), not
  `pending_approval_for`.
- A `Question` decision reattaches as a new `ReattachedApproval::Question {
  task_id, question_id, question, options, allow_free_text, answered: Option<..> }`.
  It carries no proposal id, and the desktop handles it only with the answer
  route (Task 6), never with `/api/run-command`.
- `state_phrase` and `headline` gain the wording §5.2 asks for: "A question
  was waiting for your answer".
- The `TaskStatus` set is unchanged, so the kill matrix's 42 cells stay 42
  (`crash_recovery.rs:937`). The question cells are new tests in the matrix's
  shape, not new cells in `expected()`.

The enum's name, `ReattachedApproval`, is a misnomer once it holds a question.
Rename it only if the rename is small. A misleading name on a display record is
cheaper than churn across `recovery.rs` and the shell. Record which choice was
made in the Task 5 row.

## 4. No headless flag exists; availability defaults to "no user"

Requirement 6 and §5.4 assume an execution context that knows whether a user is
present. None exists. The CLI's `ask` and the eval harness's `drive` both call
`ask` with default options, and both ignore or auto-decide what a turn hands
back.

**Decision (Task 3):** `ChatTurnOptions` gains `user_present: bool`,
`#[serde(default)]`, so it defaults to **false**.
- Only the desktop shell sets it to `true` (`lib.rs:1576`). The CLI, the eval
  harness, and anything added later get no `ask_user` until they opt in. A new
  driver that forgets the flag fails safe: the model proceeds under an
  assumption, and nothing waits forever for an answer.
- It travels on `PendingChatTurn.turn_options`, so a resumed turn keeps its
  context. A turn paused before this spec resumes with `false`, which is
  harmless.
- With `user_present == false`, `ask_user` is **not pushed** onto
  `native_tools`. If a model emits the call anyway, Layer 3 refuses it in the
  per-call loop with a tool error saying no user can answer.
- Spec 20's mode gate stays as it is. `ask_user` is read-only and allowed in
  every mode (`mode_permits` and `profile_permits` both allow it). The context
  narrows the offered set and never widens it, which is what §5.4 asks for.

**System prompt (Task 3).** `system_prompt(mode)` becomes
`system_prompt(mode, user_present)`:
- `user_present == true`: byte-identical to today's prompt. The tool's own
  description carries its usage guidance. Spec 49's two prompt-cache guards
  must pass unmodified.
- `user_present == false`: one appended paragraph. It says no user can answer
  mid-task, and that the model should proceed under an explicitly stated
  assumption, saying what it assumed and why.

This changes the prompt every headless driver sends, including the eval harness
(§9).

## 5. Nothing runs doctests, so the compile-fail test must break the build

The proposal asks for "a compile-fail test" that a `PendingQuestionRef` cannot
be passed where a `PendingApprovalRef` is required. The repository has no
`trybuild` and no `compile_fail` doctest. The quality gate runs
`cargo nextest run`, which does not run doctests, and CI
(`.github/workflows/quality.yml:111`) runs only nextest. A `compile_fail`
doctest would never be executed by anything.

Passing one distinct struct where another is required already fails to
compile, so that half needs no test. The real hazard is someone adding a
`From`/`Into` conversion, or a constructor, to make the reuse convenient.

**Decision (Task 1):** a `const` assertion in `session.rs` (or the new module)
that fails **the build** if `PendingQuestionRef: Into<PendingApprovalRef>`
ever holds. It uses the inherent-const-over-trait-const probe:

```rust
struct Probe<T>(core::marker::PhantomData<T>);
trait Fallback { const CONVERTS: bool = false; }
impl<T> Fallback for Probe<T> {}
impl<T: Into<PendingApprovalRef>> Probe<T> { const CONVERTS: bool = true; }
const _: () = assert!(!Probe::<PendingQuestionRef>::CONVERTS,
    "a question must never convert into an approval (spec 50 §5.2)");
```

Every build and every gate run checks it. It is falsified in Task 1 by adding a
`From` impl and confirming the build fails. A `compile_fail` doctest may be
added beside it as documentation, but it is not counted as the test.

## 6. Two persistence shapes, both typed

**Status-event link (Task 1).** `await_approval` writes `"pendingApproval"`. A
question writes a **different key**, `"pendingQuestion": {"questionId": ..}`,
through a new `SessionStore::await_answer(&Task, &PendingQuestionRef)`.
- `pending_approval_for` keeps its exact behaviour. It reads only
  `pendingApproval`, so it can never return a question.
- A new `pending_decision_for` returns `Option<PendingDecision>`, and the
  newest status event wins as today. An event carrying both keys is
  impossible by construction, because each writer takes one type.
- `PendingApprovalRef` gets no `kind` value for questions, and no code writes
  `kind: "question"`. Task 1 has a test asserting that the serialized
  question link contains no `pendingApproval` key.

**Question events (Task 1).** `question_asked` and `question_answered`, as
§5.6. Both are written through `append_session_event` and read with
`active_events`, so a rewind past the question removes it. Two methods,
`record_question_asked` and `record_question_answered`, and one reader,
`question_state(session_id, question_id) -> Option<QuestionState>`.
`QuestionState` holds the question, the options, `allow_free_text`, and the
answer if there is one.
- A second answer to an answered question is refused (`InvalidInput`). This
  makes "never re-asks" hold at the store as well as in the loop.

**Transcript (stored in Task 1, rendered in Task 6).** `read_messages` gains the two
events as messages with roles `question` and `answer`. Requirement 3 says the
transcript shows them as what they are, and `renderMessages` keys everything on
role, so a role is the cheapest honest shape.
- `build_model_prompt` (`chat.rs:3612`) writes each prior message's role
  literally (`output.push_str(&message.role)`). So a later turn's model sees
  `question: …` and `answer: …` lines in its recent history. That is honest,
  and it needs no `chat.rs` edit. The answer is the user's own words, so it is
  no more trusted than any user message already in that history.
- Sessions without questions produce exactly today's messages, so no existing
  `read_messages` assertion moves. Task 1 pins this.
- Requirement 3 is about the transcript. Proposal §4 rules out remembering
  answers across turns, and that refers to memory (spec 28), not to the
  conversation history every turn already carries.

## 7. The answer goes back as a tool result

The plan-review resume (`chat.rs:1114`) feeds its decision back as a **user**
message. Requirement 1 says the answer is a **tool result**. The resume
therefore follows the command resume's shape:
- `matched_tool_call` is saved on the paused turn;
- the round's `assistant_with_tool_calls` message is re-joined;
- `ModelMessage::tool(call.id, result)` is pushed, and the turn recurses with
  `round + 1`.

That is also how §5.3's "each question consumes a tool round" holds without
new counting.

The result text is fixed by the engine, not by the user's words alone:

```text
The user answered: "<answer>" (option <n>: "<label>")
```

`selectedOption` is omitted for a free-text answer. The answer is the user's
text, untrusted to the model in the same way a user prompt is, and redacted the
same way the turn's prompt is (check which path that is in Task 4, and record
it).

**Calls after `ask_user` in the same round.** Copy whatever the command pause
does with later calls in its round (`round_calls_fed_back`). Do not invent a
second rule. Record the behaviour in the Task 4 row.

## 8. Bounds, and the per-turn cap is a Preference

**Schema bounds (Task 1).** These are constants in the new module, enforced by
`AskUserInput::validate`:

| Bound | Value |
|---|---|
| Question length | 300 characters |
| Options | 2–4 |
| Option label length | 60 characters |
| Free-text answer length | 2,000 characters |

A violation is a tool error naming the bound, never a truncation (§5.1). The
three model-facing values are restated in the tool's JSON schema
(`maxLength`, `minItems`/`maxItems`), so a provider that honours schemas never
sends a violation. The engine still validates, because providers do not all
honour schemas.

**Per-turn cap (Task 2).** A new config key, `agent_max_questions_per_turn`.
- Default `2`. A hard ceiling of `4` is enforced by validation, not clamping.
- `0` means the tool is **not offered**. It does not mean "offered and always
  refused".
- It is classified as a **Preference**, beside `agent_max_tool_rounds`.
  Asking widens no capability and spends no money while the user is deciding.
  The hard ceiling stops a repository config from turning the tool into an
  interrogation. Spec 31's partition tests
  (`the_preference_keys_are_exactly_spec_34s_free_keys`,
  `every_preference_key_applies_from_repository_scope`) gain the key, and
  `split_profile_keys` decides whether a profile may carry it.
- The count is the task's `question_asked` events (Task 1 store), read at the
  call, not an in-memory counter. A resumed turn counts what was already asked.

## 9. Requirement 6's prompt change moves the eval harness

The harness drives turns with default options, so after Task 3 it is headless.
That has two consequences:
- `ask_user` is never offered in any existing scenario.
- Every Code-mode scenario's system prompt gains §4's paragraph.

So the deterministic tier's token figures will move by the paragraph's size,
and nothing else should. Task 3 runs the tier. If `evals/baseline.json` must
be regenerated, that is done deliberately and recorded with the before/after
numbers. It is never a side effect.

Proposal §7 asks how often the model used the tool across spec 18's
scenarios. With the harness headless, the answer would trivially be "never".
**Decision (Task 7):** a scenario may declare `answers: [..]`. That makes the
run interactive (`user_present: true`), and the runner answers each question
in order, failing the scenario if the model asks more questions than were
scripted. One deterministic scenario pins the mechanics. The usage measurement
itself needs the live tier against a real provider. That is a manual,
`#[ignore]`d run, and if it was not done, §7 says so plainly.

## 10. What the proposal asks for that has nowhere to go

- **"`AGENTS.md` gains a line under the tool surface" (§5.7).** `AGENTS.md`
  has no tool-surface section, and neither has `docs/DEVELOPMENT.md`. The
  user-facing tool list is `docs/USER_GUIDE.md` "Agent tools for reading and
  navigating" (`:72`). Task 8 adds `ask_user` there. In `AGENTS.md` it adds
  one sentence under "Security boundaries": an answer to a question never
  satisfies an approval, and a paused turn is resumed only by the route for
  what it is paused for. That is the invariant an agent editing this
  repository could break.
- **"Headless or CI execution (Phase 5 WP8)" and "the task report (Phase 5
  WP7)" (§5.4).** Neither exists. The assumption the model states lands in its
  response text, and that is all this spec can promise. The Phase 5 report
  picks it up when it exists.
- **"A reloaded session renders the question and answer in place."** Today a
  reloaded session shows *no* waiting card of any kind (§1, Shell). Questions
  get this through their transcript roles (§6). The answerable card is
  rendered from an unanswered `question` message, not from the recovery
  sweep, so it works on an ordinary reopen as well as after a crash. Commands
  and plans are unchanged; extending them is out of scope.

## 11. Parallel work

`#50` edits `chat.rs` in **Tasks 3 and 4**, and Task 5 if `PausedTurns` needs a
method (§3; avoid it if the question events suffice). Of the specs that edit
`chat.rs`, it may run beside **#25 only** (#25 names no `chat.rs` change).
It must not run its `chat.rs` tasks beside #23, #32, #56 or #57, or beside
#24's Task 7.

Outside `chat.rs`:
- Task 2 edits `config.rs`, `profile.rs`, `effective_policy.rs` and
  `tests/permission_profiles.rs`. #24's Task 6 edits the same four. At
  planning time this checkout also held #24's uncommitted Task 4–5 changes to
  them. Rebase on whatever is there, and never revert another track's edits.
- Task 6 edits `desktop-shell/src/lib.rs` and `app.js`. #24's Task 8 and #57
  edit both.
