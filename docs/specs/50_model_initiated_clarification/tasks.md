# Model-Initiated Clarification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
decisions it left open in [`context.md`](context.md)
**Started:** not yet (planned 2026-10-07)

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Question types, bounds, typed link and session events | Not started | No `chat.rs`. Carries the build-breaking no-conversion assertion |
| 2 · `agent_max_questions_per_turn` | Not started | No `chat.rs`. Shares four files with #24's Task 6 |
| 3 · Offer `ask_user` where a user is present | Not started | **Edits `chat.rs`.** Changes the headless system prompt: run the deterministic eval tier |
| 4 · Pause, answer, resume; no resume crosses kinds | Not started | **Edits `chat.rs`.** The requirement 2 task |
| 5 · Recovery of an open or answered question | Not started | `chat.rs` only if `PausedTurns` needs a method (`context.md` §3). Check the pairing rule first if it does |
| 6 · Shell route, question card, transcript, style guide | Not started | |
| 7 · Eval-harness scripted answers | Not started | |
| 8 · Docs, acceptance criteria, close the spec | Not started | Runs the full seven-command gate |

**Goal:** the model can ask the user one bounded question mid-turn and get the
answer back as a tool result, without ending the turn. An answer can never
stand in for an approval, by construction at three points: the types, the
paused-turn store and the shell routes. Where no user is present, the tool is
not offered.

**Architecture:**
- **Types and store (Task 1).** A new module, `clarification.rs`, holds
  `AskUserInput` and its bounds, `PendingQuestionRef` and `PendingDecision`.
  `SessionStore` gains a separate status-event key (`pendingQuestion`) and two
  question events (`context.md` §5–§6).
- **Cap (Task 2).** The per-turn cap is a config Preference.
- **Offering (Task 3).** `chat.rs` offers the tool only when
  `ChatTurnOptions.user_present` is true (`context.md` §4).
- **Pause and resume (Task 4).** `chat.rs` pauses like a command, and resumes
  with the answer as a tool result (`context.md` §7). Every resume entry point
  matches exhaustively on what its paused turn is waiting for (`context.md` §2).
- **Recovery (Task 5).** Recovery reattaches a question instead of failing the
  task (`context.md` §3).
- **Shell and UI (Task 6).** The shell gets one answer route, and the UI gets
  a question card that is not an approval card.

**Tech Stack:** Rust 2024, `serde`/`serde_json` (already dependencies), vanilla
JS in `app.js`. No new dependencies; in particular no `trybuild`
(`context.md` §5).

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Several of its
  decisions override the proposal. Do not re-derive any of these from
  `proposal.md` alone:
  - §2: the paused-turn store, not only `PendingApprovalRef`, is where
    requirement 2 is enforced;
  - §3: recovery must learn the question link, or it fails the task;
  - §4: `user_present` defaults to false;
  - §5: the no-conversion test is a `const` assertion, because nothing runs
    doctests;
  - §6: questions get their own status-event key and two events;
  - §7: the answer is a tool result;
  - §8: the bounds, and the cap as a Preference with a hard ceiling.
- **#50 edits `chat.rs` in Tasks 3 and 4.** Task 5 edits it too, but only if
  `PausedTurns` needs a method. Of the specs that edit `chat.rs`, #50 may pair
  with **#25 only**, and with no other `chat.rs` spec (#23, #32, #56, #57, or
  #24's Task 7).
  - Before starting a `chat.rs` task, read the progress table of every spec in
    flight.
  - If another spec's `chat.rs` task is in progress, wait, or agree which
    track rebases.
  - Re-read every `chat.rs` line number in `context.md` §1 before editing near
    it.
- **Every cargo command runs with `CARGO_TARGET_DIR="$PWD/target"` from the
  worktree root.** Parallel worktrees must not share one target directory.
  Shell state does not persist between commands, so put the variable on every
  invocation, for example:

  ```bash
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  ```
- **No question ever reaches an approval.**
  - No `From`/`Into` between `PendingQuestionRef` and `PendingApprovalRef`.
  - No constructor of one that takes the other.
  - No `kind: "question"` string anywhere.
  - No resume entry point that accepts a paused turn of another kind.

  Every task that touches a resume or approval path tests this at that path.
- **The existing approval machinery behaves exactly as before.**
  `pending_approval_for`, `await_approval`, the command and plan resumes, and
  the recovery of command, patch and plan links all stay unchanged.
  - `tests/crash_recovery.rs`, `tests/recovery_prompt.rs` and
    `tests/plan_turn.rs` pass unchanged after every task.
  - New assertions go in new tests, never in edits to theirs.
- **Spec 34's tests are the floor.** `tests/repository_config_trust.rs` passes
  unchanged after every task, and no task edits it. Spec 31's
  `tests/permission_profiles.rs` is extended in Task 2 only.
- **New engine tests go in `crates/workspace-engine/tests/clarification.rs`**,
  which Task 1 creates. The exceptions are tests that need `chat.rs`-private
  items: those go in a new `#[cfg(test)] mod clarification_tests` in
  `chat.rs`, beside `mode_refusal_tests`. Fixtures that build an engine set
  `enable_index_watcher: false`. A test that needs a command to run sets
  `shell` to `/usr/bin/true`.
- **Model-authored text is untrusted everywhere** (requirement 7).
  - The question and its options are redacted with `SecretScanner` before
    they are stored or sent anywhere.
  - In the UI they are set with `textContent` only: no markdown, no
    `innerHTML`, no links.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record the mutation in the progress row. This repository has a
  history of tests that passed without testing anything (spec 47 §7.2,
  spec 21's error rate).
- **Scope per-task checks. Run the full seven-command gate from `AGENTS.md`
  once, in Task 8.** `cargo nextest run --workspace` takes about 5 minutes
  locally, and `cargo clippy --workspace --all-targets` up to 18 minutes cold.
- **Never `git commit` unasked, never push, never switch branches.** Each task
  ends by showing the change and the scoped check results, then asking. When
  asked, write one subject line with no body and no trailer.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/clarification.rs` | New (Task 1): `AskUserInput`, bounds, `validate`, `PendingQuestionRef`, `PendingDecision`, `QuestionState`, the no-conversion `const` assertion |
| `crates/workspace-engine/src/session.rs` | `await_answer`, `pending_decision_for`, `record_question_asked`/`_answered`, `question_state`, `questions_asked_in_task`, `read_messages` roles (Task 1) |
| `crates/workspace-engine/src/lib.rs` | Module registration and re-exports (Task 1) |
| `crates/workspace-engine/src/config.rs`, `profile.rs`, `effective_policy.rs` | `agent_max_questions_per_turn` (Task 2) |
| `crates/workspace-engine/src/chat.rs` | **Tasks 3 and 4 (Task 5 only if needed).** `ToolAction::AskUser`, the definition, parsing, `user_present`, `system_prompt(mode, user_present)` (Task 3). Dispatch, pause, `PendingChatTurn.question`, `PausedFor`, `resume_after_answer`, `has_pending_chat_command` (Task 4) |
| `crates/workspace-engine/src/mode.rs` | `AskUser` rows in `mode_permits`, `profile_permits` and the matrix test (Task 3) |
| `crates/workspace-engine/src/recovery.rs` | `pending_decision_for`, `ReattachedApproval::Question`, wording (Task 5) |
| `crates/workspace-engine/tests/clarification.rs` | New (Task 1). Every later engine task adds to it |
| `crates/workspace-engine/tests/permission_profiles.rs` | The new Preference key (Task 2) |
| `crates/workspace-engine/tests/crash_recovery.rs` | New question tests in the matrix's shape. No existing cell changes (Task 5) |
| `crates/desktop-shell/src/lib.rs` | `user_present` in the turn-options literal (`false` in Task 3, `true` in Task 6). `questionProposal` in `chat_result_json`, `/api/answer-question-stream`, message roles in the session payload (Task 6) |
| `crates/desktop-shell/src/recovery.rs` | Describe a reattached question (Task 5) |
| `crates/desktop-shell/static/app.js`, `style.css`, `docs/UI_STYLE_GUIDE.md`, `docs/ui-style-guide.html` | Question card, transcript roles, recovery card text, style-guide entry and specimen (Task 6) |
| `crates/eval-harness/src/{scenario,runner}.rs`, `evals/` | `answers`, one deterministic scenario (Task 7) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `AGENTS.md`, `CHANGELOG.md`, `docs/specs/README.md` | Task 8 (`context.md` §10) |

---

## Task 1: Question types, bounds, typed link and session events

**Requirements:** 2 (the type half), 3 and 4 (the store half), 7 (the bounds),
and proposal §5.1, §5.2 and §5.6. Acceptance criterion "No approval call site
accepts a question … asserted by the absence of any conversion and by a
compile-fail test". **Files:** create `clarification.rs` and
`tests/clarification.rs`; modify `session.rs` and `lib.rs`. **Does not touch
`chat.rs`.**

This task changes no behaviour. Nothing writes the new events except tests.

**Interfaces produced** (later tasks rely on these names):
- `pub struct AskUserInput { question, options, allow_free_text }`, with
  `Deserialize`. `pub fn validate(&self) -> Result<(), String>` returns the
  bound that failed, in words the model can act on. The bounds are `pub
  const`s with the values in `context.md` §8.
- `pub fn validate_answer(input: &AskUserInput, answer: &str, selected_option: Option<usize>) -> Result<(), String>`.
  - A `selected_option` must be in range, and `answer` must equal that option's
    label.
  - A free-text answer requires `allow_free_text`, must be non-empty after
    trimming, and must be within the answer bound.
- `pub struct PendingQuestionRef { pub question_id: String }`. Its only
  constructor is `PendingQuestionRef::new()`, which mints `question_<uuid>`.
  Replay rebuilds one from a `questionId` read out of the log.
- `pub enum PendingDecision { Approval(PendingApprovalRef), Question(PendingQuestionRef) }`.
- `pub struct QuestionState { question_id, task_id, question, options, allow_free_text, answer: Option<(String, Option<usize>)> }`.
- `SessionStore`:
  - `await_answer(&Task, &PendingQuestionRef) -> Result<Task>`. It writes
    `WaitingForApproval` with `"pendingQuestion"` and never `"pendingApproval"`.
  - `pending_decision_for(session_id, task_id) -> Result<Option<PendingDecision>>`.
    The newest status event wins, and an event with neither key clears it.
  - `record_question_asked(session_id, task_id, &PendingQuestionRef, &AskUserInput)`.
    The input must already be redacted; Task 4 does that before calling.
  - `record_question_answered(session_id, question_id, answer, selected_option)`.
    It refuses when the question is unknown or already answered.
  - `question_state(session_id, question_id) -> Result<Option<QuestionState>>`.
  - `questions_asked_in_task(session_id, task_id) -> Result<usize>`.
- `read_messages` and `read_messages_with_seq` yield `question_asked` as
  `role: "question"` and `question_answered` as `role: "answer"`, in log
  order (`context.md` §6).

- [ ] **Step 1: Write the failing tests** in `tests/clarification.rs`:
  - `the_bounds_are_enforced_as_errors_not_truncation`. One case per bound,
    each asserting the error names the bound. An input exactly at a bound
    passes.
  - `options_are_required`. Zero or one option is an error.
  - `an_answer_must_match_its_option_or_be_permitted_free_text`.
  - `a_question_link_is_not_an_approval_link`:
    - after `await_answer`, `pending_approval_for` is `None`;
    - `pending_decision_for` is `Question`;
    - the raw status-event line contains `pendingQuestion` and not
      `pendingApproval`.
  - `an_approval_link_still_reads_as_before`. `await_approval` followed by
    `pending_approval_for` is unchanged, and `pending_decision_for` returns
    `Approval`.
  - `the_newest_link_wins_across_kinds`. Approval, then question, then a plain
    status: each read is correct, and the last read is `None`.
  - `an_answered_question_cannot_be_answered_again`.
  - `a_question_rewound_past_is_gone`. It uses spec 16's
    `conversation_rewound`, so `question_state` is `None` and
    `read_messages` drops both roles.
  - `questions_appear_in_the_transcript_in_order`.
  - `a_session_with_no_questions_reads_exactly_as_before`. Compare
    `read_messages` against a fixture written without any question event.
  - `questions_are_counted_per_task`.
- [ ] **Step 2: Confirm they fail to compile** on the missing names:
  `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test clarification`.
- [ ] **Step 3: Implement.**
  - Write the event payloads by hand like `await_approval`, or with
    `serde_json::json!`.
  - Read them by parsed `eventType` from `active_events`, never with
    `line.contains` (spec 17 §5.2).
  - Add the `const` assertion from `context.md` §5 at the bottom of
    `clarification.rs`, with a comment citing proposal §5.2.
- [ ] **Step 4: Confirm the tests pass.**
- [ ] **Step 5: Mutation-test and record each one:**
  1. Add `impl From<PendingQuestionRef> for PendingApprovalRef`. The **build
     fails** on the `const` assertion. This is the compile-fail test.
  2. Make `await_answer` write `pendingApproval` with `kind: "question"`.
     `a_question_link_is_not_an_approval_link` fails.
  3. Drop the already-answered check.
     `an_answered_question_cannot_be_answered_again` fails.
  4. Read the question events with `parsed_events` instead of
     `active_events`. The rewind test fails.
  5. Truncate an over-long question instead of erroring. The bounds test
     fails.
- [ ] **Step 6: Confirm the floor holds:**
  `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test crash_recovery --test recovery_prompt --test session_rewind --test repository_config_trust`.
- [ ] **Step 7: Scoped checks.** `cargo fmt --all -- --check`; scoped
  `clippy -D warnings` on `workspace-engine` with `CARGO_TARGET_DIR`;
  `typos`.
- [ ] **Step 8: Update this task's row, then show the change and ask before
  committing.** Suggested subject: `Add typed question links and question
  events to the session log`.

## Task 2: `agent_max_questions_per_turn`

**Requirements:** proposal §5.3 ("two is a starting value, configurable").
**Files:** `config.rs`, `profile.rs`, `effective_policy.rs`,
`tests/permission_profiles.rs`, `tests/clarification.rs`. **Does not touch
`chat.rs`.** #24's Task 6 edits the same files (`context.md` §11).

- `Config.agent_max_questions_per_turn: u32` (default 2) and the matching
  `ConfigOverlay` field.
  - A value above `MAX_QUESTIONS_PER_TURN_CEILING = 4` is rejected at
    validation with a message naming the ceiling. It is never clamped.
  - Handled in `set`, both `to_policy_text`s, `apply_overlay_scoped` (as a
    `preference`, beside `agent_max_tool_rounds`) and
    `classify_overlay_fields!` as Preference.
- `profile.rs`: decide whether a profile may carry it. Follow
  `agent_max_tool_rounds`' existing treatment, and record which.
  `effective_policy.rs`: `rule_label`.
- Tests:
  - the default is 2;
  - 5 is rejected at user scope and at repository scope;
  - 0 is accepted;
  - repository scope can set it (Preference);
  - the key is added to `the_preference_keys_are_exactly_spec_34s_free_keys`,
    with a `PreferenceCase`;
  - `repository_config_trust.rs` is untouched and passes.
- Mutations to record:
  - classify the key as Capability (spec 31's partition tests fail);
  - clamp instead of rejecting (the ceiling test fails).

## Task 3: Offer `ask_user` where a user is present

**Requirements:** 6, and acceptance criterion "the tool is absent from the
tool list in a headless execution context — asserted against the emitted tool
definitions". **Files:** `chat.rs`, `mode.rs`, `desktop-shell/src/lib.rs` (one
literal), `tests/clarification.rs`. **Edits `chat.rs`: read Global
Constraints first.**

- `ChatTurnOptions.user_present: bool`, `#[serde(default)]` (`context.md` §4).
  Set it to `false` in the shell literal (`lib.rs:1576`) for now; Task 6 flips
  it. So no production path offers the tool until the pause exists.
- `ToolAction::AskUser(AskUserInput)`.
  - `tool_action_from_call` parses `"ask_user"` with `serde_json`. A parse or
    `validate` failure becomes a tool error naming the bound, never `None`.
  - Every exhaustive helper decides about the new variant:
    - `action_effect`: `ShapesTurn`;
    - batchable read-only: no;
    - marker: not side-effecting;
    - `action_awaits_plan_review`: **false**;
    - `tool_action_label`.
  - `mode_permits` and `profile_permits` allow it in every mode. The matrix
    test gains its row.
- `ask_user_tool_definition()`.
  - The JSON schema restates the bounds (`context.md` §8).
  - The description says what the tool is for and what it is not: an answer
    authorises nothing, and patches are never chosen through it.
  - It is pushed after `complete_step`, only when `user_present` and
    `agent_max_questions_per_turn > 0`, and through `offered` like every other
    tool.
- Layer 3: if `AskUser` arrives while the tool was not offered, the per-call
  loop answers with a tool error saying no user can answer, and that the
  model should proceed under a stated assumption. Until Task 4, the dispatch
  arm returns the same error whatever the context, replacing the
  `unreachable!` path for this variant.
- `system_prompt(mode, user_present)`. With `true` it is byte-identical to
  today's prompt for every mode; with `false` it appends `context.md` §4's
  paragraph. Update every caller.
- Tests:
  - offered when `user_present` is true;
  - absent when it is false, and absent when the cap is 0. Both are asserted
    against `adapter.requests[0].tools`, via `offered_tool_names`;
  - offered in all four modes;
  - a model that emits `ask_user` unoffered gets the refusal as a tool result,
    and the turn continues;
  - `system_prompt(mode, true)` equals the pre-change prompt for all four
    modes (pin the bytes);
  - spec 49's prompt-cache guards pass unmodified.
- Run `CARGO_TARGET_DIR="$PWD/target" cargo run -p eval-harness -- run --tier deterministic`.
  Only the headless paragraph's tokens should move (`context.md` §9). Record
  the before and after numbers. Regenerate `evals/baseline.json` only
  deliberately, and say so.
- Mutations to record:
  - offer the tool regardless of `user_present` (the headless test fails);
  - make `action_awaits_plan_review` true for `AskUser` (record what fails,
    or add the test that catches it).

## Task 4: Pause, answer, resume; no resume crosses kinds

**Requirements:** 1, 2 (the runtime half), 4 (the loop half), 5, 7. Acceptance
criteria:
- "the model can ask a question mid-turn and receive the answer as a tool
  result, with the turn continuing";
- "a command, a patch and a configuration change each still require their own
  approval after a question has been answered";
- "a third question in one turn is refused";
- "a question containing a secret-shaped string is redacted".

**Files:** `chat.rs`, `tests/clarification.rs`. **Edits `chat.rs`: read
Global Constraints first.**

- **Dispatch arm for `AskUser`:**
  1. If `questions_asked_in_task >= agent_max_questions_per_turn`, return a
     tool error saying so; the model proceeds.
  2. Otherwise, redact the question and options with `self.scanner`.
  3. Mint a `PendingQuestionRef` and call `record_question_asked`.
  4. Save a `PendingChatTurn`, keyed by the question id, with
     `matched_tool_call` and `question: Some(PendingQuestion { question_id })`.
  5. Finish the marker as `awaiting_answer`.
  6. Set `terminal` with `TurnProposals.question` and break.
- Copy the command pause's handling of the round's other calls
  (`context.md` §7). Do not invent a second rule.
- After the loop, a question proposal goes to `await_answer`, never
  `await_approval`. The audit status is `question_pending`.
  `ChatTurnResult.question_proposal: Option<AgentQuestionProposal>` carries
  `{ question_id, question, options, allow_free_text }`, all redacted.
- **`PausedFor` and the three resumes** (`context.md` §2):
  - `resume_after_command_decision_with_options` and
    `resume_after_plan_decision` each match `paused_for(&pending)`
    exhaustively, refuse `Question`, and put the file back.
  - The new `resume_after_answer(question_id, answer, selected_option, ..)`
    refuses `Action` and `PlanReview`.
  - `has_pending_chat_command` returns false for a question turn.
- **`resume_after_answer`:**
  - Validate the answer with `validate_answer`. On failure, put the file back.
  - Call `record_question_answered`. If it was already answered, use the
    recorded answer and ignore the new one; never re-ask (requirement 4).
  - Push the tool result in `context.md` §7's fixed format, set
    `PreparingContext`, and recurse with `round + 1`.
- Tests, using `MockModelAdapter::new_sequence_with_tool_calls`:
  - `a_question_is_answered_and_the_turn_continues`. The next request's last
    message is the tool result carrying the answer, and the turn ends
    `Complete`.
  - `a_free_text_answer_is_refused_when_not_allowed`, and the paused turn is
    still resumable.
  - `a_third_question_in_one_turn_is_refused_with_a_tool_error`. With the cap
    at 2, the third call's tool result names the cap. Also: a resumed turn
    counts the questions already asked.
  - `an_answer_never_authorises_a_command`. After an answer, a scripted
    `run_command` that needs approval pauses for approval as usual.
  - `an_answer_never_authorises_a_patch`. After an answer, `propose_patch`
    ends awaiting review, and the patch is not applied.
  - `a_question_id_cannot_resume_a_command_or_a_plan`:
    - `resume_after_command_decision_with_options(question_id, approved: true, ..)`
      errors;
    - `resume_after_plan_decision(question_id, ..)` errors;
    - in both cases nothing ran and the question is still answerable.
  - `a_command_or_plan_id_cannot_be_answered`.
  - `a_question_id_is_not_a_pending_chat_command`.
  - `a_configuration_change_still_needs_its_own_approval`. A model can change
    configuration only through a command or a patch to a config file, so this
    asserts both against `.damaian/config.conf`. Record that reading in this
    row.
  - `a_secret_in_a_question_is_redacted_before_the_log_and_the_result`.
  - `asking_a_question_does_not_satisfy_the_plan_gate`. In Plan mode, after an
    answer, a mutation still raises spec 21's plan review.
- Mutations to record:
  1. Drop the `Question` refusal in the command resume. The cross-kind test
     fails, because the shell branch would run.
  2. Make `has_pending_chat_command` true for questions. Its test fails.
  3. Use `await_approval` for the question. Task 1's link test fails, or add
     one here that does.
  4. Skip redaction. The secret test fails.
  5. Count questions in memory rather than from the log. The resumed-turn
     count fails.

## Task 5: Recovery of an open or answered question

**Requirements:** 4 (the restart half), and acceptance criterion "a crash while
a question is open resumes with the question pending and unanswered; a crash
after it is answered resumes without re-asking". **Files:** `recovery.rs`
(engine), `desktop-shell/src/recovery.rs`, `tests/crash_recovery.rs`,
`tests/recovery_prompt.rs`. `chat.rs` is edited only if `PausedTurns` needs a
method (`context.md` §3). Check the pairing rule first if it does.

- `reattach_pending_approvals` reads `pending_decision_for`. Its approval arms
  are unchanged. A `Question` arm builds `ReattachedApproval::Question` from
  `question_state`, including the answer if one is recorded.
- A question whose state is missing fails the task with a reason, as a missing
  proposal does today.
- `state_phrase`, `headline` and `action_subject`: "A question was waiting
  for your answer".
- The shell's `describe_approval` emits `kind: "question"` with the redacted
  text, the options and `answered`. This is a display payload only; nothing
  routes it to a command or patch endpoint.
- Tests in the kill matrix's shape (new tests; `expected()` and the 42 cells
  are unchanged):
  - `a_question_open_at_a_crash_is_reattached_unanswered`;
  - `a_question_answered_before_a_crash_is_not_asked_again`. The reattached
    record carries the answer, and resuming uses it;
  - `a_question_with_no_recorded_state_fails_the_task_with_a_reason`;
  - shell: `a_reattached_question_is_offered_for_an_answer_not_an_approval`.
- Mutations to record:
  - drop the `Question` arm (the open-question test fails, because the task is
    failed);
  - ignore a recorded answer (the answered test fails).
- Record whether `ReattachedApproval` was renamed (`context.md` §3).

## Task 6: Shell route, question card, transcript, style guide

**Requirements:** 3 and 7, and acceptance criteria "the question surface is
visually distinct from the approval surface, per `docs/UI_STYLE_GUIDE.md`" and
"a reloaded session renders the question and answer in the transcript in
place". **Files:** `desktop-shell/src/lib.rs`, `app.js`, `style.css`,
`index.html` if needed, `docs/UI_STYLE_GUIDE.md`, `docs/ui-style-guide.html`.

- Shell:
  - `user_present: true` in the turn-options literal;
  - `questionProposal` in `chat_result_json`;
  - `POST /api/answer-question-stream` (`repo`, `question_id`, `answer`,
    `selected_option`), which streams `resume_after_answer` exactly like
    `/api/resume-plan-stream`;
  - `message_json` already carries the role, so the `question` and `answer`
    roles reach the UI without a new field.
- UI:
  - `appendProposals` gains `createQuestionCard`. `renderMessages` renders a
    `question` message as that card (answered or not, depending on whether an
    `answer` message follows it) and an `answer` message as a user-side reply.
  - That is how a plain reopen shows an unanswered question (`context.md`
    §10).
  - The recovery card's question branch points to the same card.
- Card rules (style guide §3–§5):
  - The question and options are set with `textContent`.
  - The options are all secondary `.btn-sm` buttons. There is **no
    `.btn-primary`**, because no answer is the safe one.
  - Free text is a single input with its own Send.
  - Nothing is focused or pre-selected.
  - A separate `.question-card` class: never `.command-approval`, and never
    its header, risk badge or footer.
  - Once answered, the card shows the answer and disables itself.
- `docs/UI_STYLE_GUIDE.md` §5 gains a "Question surface" entry: why it is not
  an approval surface, and what it must never borrow from one. Add a specimen
  beside "Command approval" in `docs/ui-style-guide.html`.
- Tests (shell):
  - `the_chat_result_carries_a_question_put_up_for_an_answer`;
  - `a_reopened_session_carries_the_question_and_answer_messages_in_order`;
  - `the_answer_route_refuses_a_command_proposal_id`;
  - `run_command_with_a_question_id_runs_nothing`.
- Verify in the browser (memory: `include_str!`-embedded assets, so rebuild and
  restart). Use a separate port and `DAMAIAN_DATA_DIR`, never 4765, and kill
  only your own PID. Use `DAMAIAN_MOCK_MODEL_RESPONSE` or a scripted model for
  the question. Check:
  - ask, answer by option, answer by free text;
  - reload mid-question and confirm the card is back and answerable;
  - the card sits next to a command card and does not look like one.
- `node --check` on `app.js`, and `npm run lint:web`.

## Task 7: Eval-harness scripted answers

**Requirements:** proposal §7's measurement (`context.md` §9). **Files:**
`crates/eval-harness/src/scenario.rs`, `runner.rs`, `evals/`.

- A scenario field, `answers: Vec<String>`, with serde default empty.
  - Non-empty makes the run `user_present: true`.
  - The runner answers each question in order, by option label when one
    matches and otherwise as free text.
  - Asking more questions than are scripted fails the scenario with that
    reason.
- One deterministic scenario: a scripted model asks one question, gets the
  scripted answer, and completes. The scenario asserts the answer reached the
  model.
- Every existing scenario's result is unchanged, apart from Task 3's recorded
  prompt delta.
- The live measurement for proposal §7 is a manual run against a real
  provider. It is `#[ignore]`d, with a doc comment giving the command. If it
  is not run, Task 8 says so in §7 rather than implying a number.

## Task 8: Docs, acceptance criteria, close the spec

**Requirements:** proposal §5.7 and §6, plus AGENTS.md "When a spec becomes
Done". This is a fresh session that reads the whole spec folder and the final
code.

- `docs/USER_GUIDE.md`:
  - what a question looks like;
  - that answering one never authorises anything;
  - that a question can be left unanswered (the task waits; nothing times
    out);
  - add `ask_user` to "Agent tools for reading and navigating" (`:72`);
  - that a question appears only in the desktop app.
- `docs/TROUBLESHOOTING.md`:
  - telling a waiting question from a waiting approval in the session log
    (`pendingQuestion` against `pendingApproval`);
  - where the two question events are.
- `AGENTS.md`: the one sentence under "Security boundaries" from `context.md`
  §10.
- `proposal.md` §7: what landed, where each layer lives, the two-per-turn
  question, and the harness numbers or the plain statement that the live run
  was not done.
- A §7 table: each §6 acceptance criterion, the test that covers it, and a
  verdict.
- Status records, all in one change:
  - `Status: Done (date)` in `proposal.md`;
  - this file's header and table;
  - the `docs/specs/README.md` row and "What to build next";
  - remove the `Unreleased` line in `CHANGELOG.md`;
  - `grep -rn "50_model_initiated_clarification" docs/specs` for every
    `Depends on:` line. #51 cites §5.4.
- Run `npm run specs:check` and read its output.
- Run the full seven-command gate from `AGENTS.md`, each with
  `CARGO_TARGET_DIR="$PWD/target"` where it is cargo, and record the results
  in this row.
