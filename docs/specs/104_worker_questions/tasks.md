# Spec 104 tasks

**Started:** 2026-11-03 — **Done:** 2026-11-03

Implementation plan for [`proposal.md`](proposal.md). The design and the
planning corrections live there and in [`context.md`](context.md).

## Global constraints

- `workspace-engine` owns the offering rule, the prompt wording, the typed
  pending kind, the answer resume path and the task-control outcome. The shell
  and worker host transport those facts and must not recreate the rules.
- A question is never an approval. No type conversion, no shared route and no
  shared id space may connect them; `approve`/`reject` and `answer_question`
  refuse each other's ids before any turn is resumed.
- The question store, bounds, per-turn cap, redaction and crash recovery from
  spec 50 are reused unchanged.
- The worker progress projection carries only the question id and already
  redacted display text. Options and answers stay in the session log.
- No new configuration key. Repository configuration gains no way to enable,
  widen or redirect a worker question.
- All worker ownership, lifetime, crash-recovery and parent-deletion rules stay
  those of spec 57; this spec adds one decision kind, not a second lifetime.
- Each task is a fresh agent session in the normal case. Scoped checks run per
  task; the workspace-wide clippy and test suite remain for CI under `AGENTS.md`.

| Task | State | Durable result |
|---|---|---|
| 1 · Split the offering capability | Done 2026-11-03 | `ChatTurnOptions` replaces `user_present` with `question_route_available` and `synchronous_user_present`. The old JSON key is a serde alias on the synchronous field, and `question_route()` treats a synchronous user as having the route, so pending turns written by spec 50 still load. `interactive()` and `worker()` constructors make the two intended combinations explicit. `ask_user` is offered when the effective route exists and `agent_max_questions_per_turn > 0`; `fetch_url` stays tied to the synchronous user. Ordinary shell turns set both; worker turns set only the route; the CLI, eval harness and other drivers set neither. Engine-started repair/continuation turns derive the worker route from the session's worker record. The old `user_present` call sites in the engine, desktop shell, eval harness and focused tests were updated. |
| 2 · Prompt split | Done 2026-11-03 | `system_prompt_with_capabilities` has three states. Both capabilities keep the pre-spec-50 interactive prompt byte-for-byte. The route-only worker prompt keeps the stated-assumption rule for low-impact ambiguity and adds the instruction to ask when the answer would materially change scope, public behaviour, destructive actions or acceptance criteria. Neither capability keeps a headless paragraph that now says material ambiguity must stop as incomplete rather than guess. The handoff paragraph still composes on top unchanged. |
| 3 · Worker question route | Done 2026-11-03 | `TurnDriver::take_turn` starts worker messages with `ChatTurnOptions::worker()`. `pending_decision` maps `result.question_proposal` to `DecisionKind::Question`; the host settles `WaitingForApproval` on the question id. The `question` progress kind carries only `questionId` and the redacted question text, and the status line carries `questionId`. `ControlCommand::AnswerQuestion` parses `questionId`, `answer` and an optional numeric or string `selectedOption`, dispatches to `resume_after_answer`, and is refused on any other pending kind; `approve`/`reject` refuse a question. The snapshot reports `questionId` rather than `proposalId` for a question, and notes queued before the answer are carried into the resumed turn. `POST /api/worker-control` forwards `answer_question` with the socket's field names. |
| 4 · Restart reattachment | Done 2026-11-03 | `restore_pending_decision` replaces the plan-only restore: a plan review reattaches exactly as spec 103 built it, and an unanswered question reattaches from `question_state` by writing the question and waiting status and settling the same id, with no model call and no configuration re-reading. A refused answer (an invalid option, a wrong answer shape, a raced answer) re-reads the same durable state and keeps the question waiting with the refusal in the status, rather than reporting idle and dropping the card. An answered question is deliberately not reattached because spec 50 forbids asking it again; missing, corrupt or foreign-task state restores nothing. A worker whose process ended while the question was open keeps spec 57's interrupted/restart path, so the restart is what reattaches it. |
| 5 · Outcome, board and UI | Done 2026-11-03 | `WorkerTaskFacts.pending_question` is derived by the shell from the status line's `questionId`. `TaskControl` renders a waiting question as `NeedsAnswer`, headline **Needs your answer**, *Needs you* group, no composer and the existing review action, distinct from **Needs your decision**. The board's `doing` therefore reads **Needs your answer**. `app.js` keeps no status map: it draws the existing transcript question card, routes a worker card's answer through `/api/worker-control` via an `onAnswer` callback, treats the `question` progress line as a marker only, and includes `.question-card` when *Review* scrolls to the waiting card. |
| 6 · Tests, documentation and close-out | Done 2026-11-03 | Engine tests add the route-only offer, the headless no-offer, a worker asking and resuming after the process is idle, a worker answer never satisfying a command approval, the worker prompt split and the **Needs your answer** control. Desktop tests add `answer_question` parsing (numeric and string option), both endpoints refusing the other kind, live settle/answer through the control command, restart reattachment without a model call, the board/progress projection, and the route's field names. `USER_GUIDE.md`, `TROUBLESHOOTING.md`, this spec's README row and the `Unreleased` wording were updated. Scoped checks: `cargo fmt --all`; `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`; `cargo nextest run -p workspace-engine --lib --test task_outcomes` (745 passed, 5 skipped); `cargo nextest run -p workspace-engine --test clarification` (46 passed); `cargo nextest run -p desktop-shell` (281 passed, 18 skipped), with the worker subset re-run after the refused-answer restore (22 passed); `node --check`, `npm run lint:web`, `typos`, `cargo deny check`, `npm run specs:check -- --strict` and `git diff --check`. Workspace-wide clippy and tests were left to CI. No manual browser walk was performed; the card is the unchanged spec 50 component and only its answer route changed. |

## Task 1: capability split

Implemented in `crates/workspace-engine/src/chat.rs`, with call-site updates in
the desktop shell and eval harness. The backwards-compatible alias and
`question_route()` are the load-bearing part: an older pending turn must not
become headless halfway through.

## Task 2: prompts

Implemented beside the offering rule so the tool list and the instruction cannot
disagree. The interactive prompt is unchanged; only the worker-only and headless
states gained text.

## Task 3: worker host route

Implemented in `crates/desktop-shell/src/worker.rs` and `workers.rs`. The
control command, the pending kind and the progress projection are one change:
splitting them would leave a state the other half cannot represent.

## Task 4: restart

Implemented as a generalisation of `restore_pending_plan_review`. It reads only
durable state and never constructs authorisation from current configuration.

## Task 5: presentation

Implemented in `task_control.rs`, `workers.rs` and `app.js`. The engine decides
the label and action; the browser only transports the command.

## Task 6: verification and documents

Focused tests and scoped checks are recorded in the table above. The full
seven-check gate is not claimed until CI is green, per `AGENTS.md`.
