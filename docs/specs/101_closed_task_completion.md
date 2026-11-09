# Feature Spec: A Worker Completes Only a Closed Task

Status: Done (started 2026-11-01; done 2026-11-01)
Order: 101 of 106
Plan: none. This is a failure-driven specification from the 2026-10-31
attempt to implement the reserved spec 86 with one Damaian worker.
Depends on: [#21](21_task_plan_progress_and_budget/proposal.md) (durable plans),
[#23](23_verification_loop/proposal.md) (verification outcomes),
[#66](66_plan_left_open_at_turn_end/proposal.md) (the existing one-reminder
behaviour), and [#99](99_worker_task_outcomes.md) (worker outcomes), all built.
Everything else named below is a cross-reference, not a prerequisite.

## 1. Motivation

A model answer is not proof that its task is finished. Today
`StopReason::Answered` becomes `TaskStatus::Complete` after #66's one reminder
even when the carried plan still has open implementation or validation steps.
#99 then presents that durable status as **Task complete**.

The spec 86 attempt demonstrated the consequence: Damaian produced a
specification commit and a partial `chat.rs` commit, but never landed all of the
planned code, tests, documentation and checks. The execution record had no
engine-owned condition that could distinguish that partial result from a
finished task.

## 2. Requirements

1. `TaskStatus::Complete` is written only when an engine-owned completion gate
   says the task is closed. A model cannot assert or tool-call its way around
   the gate.
2. A planned task is closed only when every non-verification plan step is
   either completed or skipped — a blocked step is outstanding, not closed —
   when no verification is due or running, no proposal or question is pending,
   and every required check known to the task has a current terminal
   disposition. Required checks are the approved plan's structured required
   checks plus the completion floor: the detected checks of the owning and
   enclosing roots of every file the task touched. A documentation-only task
   has no floor.
3. A skipped or declined check counts only as an explicit user waiver. It is
   never represented as passed, and the completion report names the waiver. An
   explicit decline of the exact deferred command a required check would run is
   the same waiver (spec 105 review fix R8); a policy refusal, a timeout or a
   model that never runs the command is not.
4. When the model answers after #66's reminder while work remains, the task
   becomes a new durable `incomplete` status rather than `complete`. The open
   plan and the reasons from the completion gate remain attached.
5. `incomplete` is terminal for the current run but resumable. An ordinary
   session and a worker both offer **Continue task**; continuing carries the
   same approved plan — including when every step is already terminal or a step
   is blocked — and the source task's unmet completion reasons into the next
   run. The model cannot replace that carried plan while it still has open or
   blocked steps or a required check; only a user-reviewed plan revision may
   drop or replace them.
6. A worker progress record and #99's outcome renderer distinguish
   **Complete** from **Incomplete**. A result returned through #100 carries the
   truthful status; #100 does not translate `incomplete` into success.
7. Recovery recomputes the gate from durable state. A crash between the final
   model answer and the status write cannot turn an open task into complete.
8. Existing approval boundaries are unchanged. The completion gate may stop or
   continue work, but it cannot approve a patch, command, check or question.

## 3. Non-goals

- Making the model finish an arbitrarily difficult task. This spec makes the
  outcome truthful and resumable; #103 supplies longer bounded execution.
- Deciding whether a failing check was caused by the task. That is #102.
- Defining required checks from prose. #105 makes approved checks structured.
- Treating every open finding in the session as a blocker. Findings from an
  earlier task or a pre-existing failure may remain; the gate uses the current
  task's verification disposition.

## 4. Design

Add a `CompletionGate` report computed from the task, active plan, verification
state, pending links and required-check evidence. It contains a boolean
`eligible` plus stable reason codes such as `open_plan_steps`,
`blocked_plan_steps`, `verification_due`, `pending_decision`,
`required_check_missing` and `required_check_failed`.

`Blocked` is terminal for the step itself — its outcome is settled, and a plan
revision may not delete it — but it is not finished work. The gate therefore
reports a blocked step as `blocked_plan_steps` rather than folding it into
`open_plan_steps`. `open_plan_steps` reads `TaskPlan::open_steps`, which is also
spec 66's reminder set; that reminder deliberately stays quiet about a blocked
step because the model has already been told, while completion must not treat
it as done.

The required set is the plan's structured required checks plus the **completion
floor**: the detected checks of the owning and enclosing roots of every file the
task touched, selected with spec 23 §5.2's root rule. The two sets are merged by
`CheckId`, so a plan check identical to a floor check is one obligation, and a
plan's own checks never replace the floor: a trivial declared check cannot
escape the root checks the task's changes selected. A documentation-only task
selects no root and has no floor. The floor only reads the persisted repository
map; it never stores a command proposal or runs anything, so every check still
follows command policy and its ordinary approval route. A check the user
declines is the named waiver below; an auto-runnable floor check simply runs.
Since spec 105 review fix R8, declining the exact command a deferred required
check runs is also the named waiver: the decline is durable plan evidence and
the gate matches it by the same `CheckId` identity as the check's exit.

The final `StopReason::Answered` settlement calls the gate. Eligible work keeps
today's `complete`; ineligible work writes `incomplete` and stores the reason
codes in the task-status event. The final response remains in the transcript —
it may be useful — but is not treated as the execution verdict.

`TaskStatus::Incomplete` is terminal but not successful. It is included in
session listing, progress reconstruction, worker return, task reports and
recovery classification. Continuing it follows the same durable-plan carry
path as a budget continuation, without granting more approvals. The source
plan is carried whenever the task has one, not only while a step is open:
an `incomplete` settlement can rest on an unmet required check after every
step is terminal, or on blocked steps whose outcome is settled but whose work
is not finished. Carrying the plan is what keeps those completion obligations
visible to the continued task's gate; the continuation prompt names the carried
plan rather than claiming that no step is open.

Because that carried plan is also the record of what the continuation still
owes, the model may not replace it while it has open or blocked steps or a
plan-declared required check (review fix R9, 2026-11-06). A `propose_plan` on
such a continuation is refused with a tool result that tells the model to work
through the carried steps; dropping or replacing one of those obligations is
the user's decision, taken through the ordinary plan-review revision path. A
carried plan with nothing outstanding — every step completed or skipped and no
required check — has no work to lose and may still be replaced; that
replacement then goes through the ordinary plan review before its first
mutating action. The refusal is specific to the engine's **Continue task**
operation: a fresh task's first `propose_plan` and spec 83's typed-turn carry
keep their existing behaviour.

The required check is part of this guard for the R2 reason: continuing carries
the plan precisely so an unmet check stays visible. A replacement that dropped
the check while every step was already terminal would otherwise let the
continued run answer `complete` with the check still unmet.

## 5. Acceptance

- A model that answers twice with an open plan receives one reminder and ends
  `incomplete`, with the plan unchanged.
- Completing the remaining steps and current required checks changes the next
  run to `complete`.
- A declined required check — on its check card or by declining the exact
  deferred command — produces a named waiver, never pass evidence. Declining an
  unrelated command or the same command in another directory is not a waiver,
  and a later approved run decides instead of an earlier decline.
- A due verification, pending patch, pending question or open required step
  prevents `complete` in ordinary and worker sessions.
- Reloading an incomplete worker shows **Incomplete — Continue task** and
  continuing it carries the same plan.
- No historical task already stored as complete is rewritten during migration;
  the stricter rule applies when new status events are settled.

## 6. Implementation Plan

**Started:** 2026-11-01 — **Done:** 2026-11-01

| Task | State | Durable result |
|---|---|---|
| 1 · Completion model and engine gate | Done 2026-11-01 | Added terminal-but-unsuccessful `incomplete`, typed engine-owned reason/waiver reports, compatible durable storage, and a repository-bound gate over the active plan, pending decision, explicit continuation chain, and current verification evidence. The full chain is required for completion evidence because a continuation and its verification turn may be sibling descendants of the same plan task; protected paths retain one-way ancestry. Passed locally: `cargo fmt --all`, scoped workspace-engine clippy, the `closed_task_completion`, `session_rewind`, `crash_recovery`, `plan`, and `verification_loop` tests, strict spec-status checking, and typos. |
| 2 · Settlement and recovery | Done 2026-11-01 | Recomputed and persisted the gate at both new-settlement paths. `run_agentic_turn` settles an answered turn through one engine operation: eligible work writes `complete`, otherwise `incomplete`, while #66's reminder transcript and pending/budget statuses are unchanged. A verification appends `Finished` before the same settlement; a successful outcome becomes `complete` only when the whole task is closed, and intermediate continuation reopens only an `incomplete` whose sole blocker is `open_plan_steps`. A command verification still runs after an `incomplete` answer settlement, so `verification_due` is not stranded. A non-side-effecting `completion_settlement` marker stores a typed JSON reference (repository root plus `gate`/`verification_failed`) and is written before the gate is recomputed, so a crash during report computation is still inside the recognizable answer-to-status window; recovery parses it, validates root/session/task, recomputes currentness, never reads a stored eligible boolean, keeps a crashed failed verification `failed` with its report, and fails safe to `interrupted` when validation fails or the raw status is unknown to this build. A dangling side effect on the same task outranks the marker, so recovery cannot overwrite `unknown_external_outcome`. Recovery closes the original marker through crate-private `finish_recorded_action`; tests prove one start and one finish, with no duplicate marker, and that normal settlement order is `action_started` < `task_status_updated` with the report < `action_finished`. `model_response_completed` records the persisted verdict. Regression tests cover two answers with an open plan (`incomplete`, unchanged plan, both transcript answers, `incomplete` audit), a closed plan (`complete`), successful intermediate verification with no transient `complete`, failed verification staying `failed` with `required_check_failed`, crash markers recomputed to `incomplete`/`complete`/`failed`, an unusable root staying interrupted, an unknown future status not being settled, a dangling side effect outranking a completion marker, and settlement event order. The browser-skip batch test now expects `incomplete` because a policy-skipped check is not a pass and the declined check is only a named waiver. Passed locally: `cargo fmt --all`; `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`; nextest `completion_settlement`, `closed_task_completion`, `crash_recovery`, `session_rewind`, `verification_loop` (157 passed, 4 skipped) and workspace-engine lib tests (679 passed, 5 skipped); `npm run specs:check -- --strict`; `scripts/quality-gate.sh typos`. Workspace clippy and tests are left to CI. Task 3 discovery: read the durable report through `read_completion_report`; use `TaskStatus::is_successful` and the report's reason codes and waivers instead of re-deriving completion, and do not assume only `complete` can carry a verification report because `verify_after_command_turn` now accepts an `incomplete` settlement. |
| 3 · Resume, worker outcome and thin UIs | Done 2026-11-01 | Added `workspace-engine::task_control`: one serializable `TaskControl` per ordinary session or worker with outcome, dot, group, headline, detail, completion reasons/waivers, primary/board/composer actions and the engine-computed stale-progress sentence. `SessionStore::continue_incomplete_task` writes the new task and its `plan_resumed` edge in one locked batch after re-reading the newest task, its `incomplete` status and the exact completion report the prompt was built from; `ChatOrchestrator::continue_incomplete_task` builds the prompt from the durable reasons plus an optional note and runs that task through the ordinary path, granting no approval. Every continuation action carries its exact source `taskId` and the engine's `wireCommand`, so the shell, socket, `TurnJob` and worker host never resolve "the latest task" at execution time; a stale second click is refused instead of continuing a later incomplete task. `ChatTurnResult` now carries the engine-owned `taskControl` (with its durable reasons/waivers, so a live result matches a reload) and a `turnPresentation` with an exhaustive serialized `TurnIndicatorState` (including `pending` for paused and still-running tasks), a typed stop notice/action, and the engine-computed `paused` reload-grouping fact; the same presentation is serialized per task for reloaded sessions, and only `TaskStatus::Complete` yields `complete`. Worker facts carry the typed `WorkerProcessState` and durable/reported `TaskStatus`; the engine decides `needs_decision` itself, so the shell transports facts rather than a pre-decided flag. `app.js` groups paused reloads from `turnPresentation.paused` and compares no task statuses. `/api/continue-task-stream`, the worker `continue-task` command, `/api/session.taskControl`, enriched `workerReturns.control`, `/api/worker-progress.control` and board-row `control` all transport the engine model. `app.js` lost `WORKER_TASK_OUTCOMES`, `WORKER_END_LABELS`, `WORKER_COMPOSER_MODES`, `WORKER_WIRE_COMMANDS`, `workerViewOutcome`, `workerComposerMode`, `chatCompletionState` and the budget/stop status tables; it renders returned labels/actions, including **Task incomplete — Continue task**, and keeps the literal `incomplete` in worker progress and returns. A failed streamed continuation now returns a false result and leaves the ordinary **Continue task** card in place rather than treating a swallowed error as success. Specs 99 and 100 record the amendment. Two stale desktop tests were aligned with Task 2: an answered command task with a due verification is now expected to be `incomplete` with its verification task `complete`, and the verification fixtures create their session under the real repository id so the repository-bound gate can settle them. Regression tests cover the exact worker target, stale-target refusal, report equality under the lock, live result reasons/waivers, engine stop notices, typed indicator and paused facts (including `waiting_for_approval`), decision derivation from typed/reported worker facts, and source-level thin-UI assertions that reject the raw paused-status comparison. Passed locally: `cargo fmt --all`; `node --check crates/desktop-shell/static/app.js`; `npm run lint:web` (one pre-existing descending-specificity warning in `style.css`); scoped clippy for `workspace-engine` and `desktop-shell`; `workspace-engine` `task_control` unit and integration tests (17 passed), `closed_task_completion` and `completion_settlement`; the thin-UI source test and four desktop worker tests (all passed). Workspace-wide clippy and tests remain CI-only. |
| 4 · Integration, documentation and close-out | Done 2026-11-01 | Audited all eight requirements and six acceptance criteria against the engine, shell, worker and tests. Closed four integration gaps: the deterministic harness counted the non-side-effecting `completion_settlement` marker as a tool round and a tool call, so its trace now excludes it from both and a unit test pins that; `plan_left_open_at_end` asserts the truthful `incomplete` verdict; the plan-review hand-off clears its resolved pending link before settling through the same gate instead of writing `complete` directly; and recovery closes a settlement marker whose status landed before the crash instead of leaving a permanent crash window. Added evidence for a continuation that closes the carried plan (`complete`) and for a verification-only continuation prompt that does not claim a plan. Updated `USER_GUIDE.md`, `TROUBLESHOOTING.md` and `SECURITY.md` for complete versus incomplete, **Continue task**, crash settlement, worker outcomes and unchanged approvals; reconciled #103/#105/#106 dependencies and `README.md`; and removed #101's completed item from `CHANGELOG.md` `Unreleased`. Deterministic evaluation: all 46 scenarios pass. Local checks passed: `scripts/quality-gate.sh fmt node lint typos deny`; scoped clippy for `workspace-engine`, `desktop-shell` and `eval-harness`; nextest 898 passed / 9 skipped across `workspace-engine` lib plus `closed_task_completion`, `completion_settlement`, `task_control`, `crash_recovery`, `verification_loop`, `session_rewind` and `plan_turn`; 67 passed (1 leaky) / 209 skipped desktop-shell worker/continue/incomplete tests; the `eval-harness` trace unit test; `npm run specs:check -- --strict`; and `git diff --check`. Workspace clippy and tests remain CI-only. |

### Task 1: completion model and engine gate

Task 1 establishes the reusable application boundary before any settlement or
UI wiring:

- `TaskStatus::Incomplete` is terminal for one run, is never successful, and
  parses/replays without rewriting historical `complete` events.
- `CompletionReasonCode` and `CompletionGateReport` are typed engine data. The
  report carries `eligible`, stable reason codes, user-visible explanations,
  and explicit declined-check waivers; adapters must not reconstruct their
  meaning.
- The session log stores the report beside the status event and can read the
  newest report for a task. Old events have no report and keep their recorded
  status unchanged.
- A `workspace-engine` service builds the report from durable task state: the
  active plan, folded verification events, pending approval/question link, and
  the latest known disposition of each selected check. Non-verification plan
  steps must be terminal. Verification due or started-but-unfinished blocks
  completion. Passed current checks and checks explicitly declined by the user
  are terminal; failed, stale, missing, or policy-skipped checks are not.
- Repository/session/task validation and check-currentness decisions remain in
  the engine. A future CLI and the desktop receive the same report rather than
  reimplementing the gate.

Task 1 does not yet change `StopReason::Answered`, recovery, worker outcomes or
the desktop. Its scoped tests pin the pure reason matrix, durable round-trip,
historical compatibility, explicit-waiver semantics, and repository-bound
service behavior. It stops after updating this table and recording its actual
checks and discoveries.

### Task 2: settlement and recovery

Task 2 makes the gate the only engine path from newly settled work to
`TaskStatus::Complete`:

- Add one engine-owned settlement operation that recomputes a
  `CompletionGateReport` from durable state and writes the report beside the
  chosen status. An answered turn may become `complete` only when the report is
  eligible; otherwise it becomes `incomplete`. A failed verification remains
  `failed`, but carries the ineligible report rather than bypassing the gate.
- `run_agentic_turn` keeps #66's transcript behavior: the answer that triggered
  the reminder and the answer after it remain useful assistant messages. After
  the final plan evidence is settled, `StopReason::Answered` calls the shared
  settlement operation. Pending approvals/questions and budget stops retain
  their existing statuses and do not pass through completion settlement.
- Verification appends its terminal `Finished` event before invoking the same
  settlement operation. A successful verification outcome becomes `complete`
  only when the whole task is closed; otherwise it is `incomplete`. The
  existing automatic continuation after a successful intermediate
  verification may reopen only an `incomplete` task whose remaining blocker is
  its open non-verification plan work. Failed, stale, missing, pending or
  policy-skipped evidence must not enter that continuation.
- Record a durable, non-side-effecting completion-settlement marker before the
  status write. It identifies the repository through an engine-validated root,
  and is finished only after the report and status are durable. Recovery of a
  dangling marker calls the same engine settlement operation and recomputes
  currentness from the checkout; it never trusts a stored `eligible` boolean.
  If the root/session/task can no longer be validated, recovery fails safe into
  the ordinary interrupted path rather than writing `complete`.
- Audit fields and returned task state use the persisted verdict. No adapter,
  shell or JavaScript decides whether the report is eligible or translates an
  ineligible report into success.

Scoped tests must cover two answers with an open plan becoming `incomplete`, a
closed plan becoming `complete`, successful intermediate verification
continuing without a transient false success, failed verification remaining
failed with reasons, and a crash-window marker being recomputed into the same
truthful verdict. Keep historical `complete` events unchanged. Stop after Task
2; do not add Continue-task controls, worker outcome mapping or UI behavior,
which belong to Task 3.

### Task 3: resume, worker outcome and thin UIs

Task 3 makes an `incomplete` verdict understandable and directly resumable in
ordinary and worker sessions without making either desktop layer interpret
task state:

- Add an engine-owned, serializable task control model. It supplies the
  outcome's visual kind, headline and detail, the completion reasons and
  waivers to show, and the permitted primary action with its command, label and
  input requirements. The engine has separate ordinary-session and worker
  views of the same task verdict; worker lifecycle facts such as alive,
  running, paused, waiting for a decision and last progress time are typed
  inputs, not JavaScript conditions. The stale-progress sentence remains a
  presentation fact computed by the engine from those inputs. A live
  `ChatTurnResult` carries the same control (including the durable completion
  reasons and waivers) plus a `turnPresentation` for the indicator and any
  stop row/action; a reloaded task carries the same presentation, so the UI
  never maps `taskStatus` or `outcome` to a stop affordance.
- Move the status/action decisions introduced by #99 out of `app.js` and the
  desktop shell. In particular, browser maps and conditionals must not decide
  that a status means complete, incomplete, failed, needs a decision, follow-up
  or continuation. Worker-board rows, the open worker strip and worker-return
  cards consume engine-produced labels and actions. Amend #99 and #100 where
  their older designs assign those decisions to the browser.
- Provide an engine-authorized **Continue task** operation for the newest
  `incomplete` task. The rendered control carries the exact source task id, and
  the execution boundary re-reads that id under the session-log lock: it must
  still be the newest task, still be `incomplete`, and still carry the exact
  completion report the prompt was built from. It refuses an unknown, stale,
  re-settled, non-incomplete or wrong-repository target, creates a new run
  through the ordinary turn path, and uses an engine-built prompt containing
  the durable completion reasons.
  It must work without requiring the user to type the word "continue"; an
  optional user note may be appended as data. The new run carries the source
  task's open plan through the existing durable plan-resume edge. It grants no
  patch, command, check, question or fallback approval.
- Expose the same continuation command through the worker host. A reloaded
  idle incomplete worker offers **Continue task**; a completed worker offers
  **Send follow-up**; a decision-pending worker offers only its existing
  decision control. Starting the continuation records a normal new worker task
  and carries the same plan. The shell and browser transport the engine action
  rather than selecting the worker command from lifecycle/status strings.
- Preserve the literal `incomplete` status in worker progress and in #100's
  durable `WorkerReturn`. A returned incomplete result is labelled as
  incomplete and is never presented as successful; continuing a parent return
  remains #100's separate **Continue with result** action.
- Keep all action authorization at the execution boundary. A stale browser
  control, a second click, or a manually forged request is refused by the
  engine after re-reading the newest durable task and its report. UI source
  tests should prove that it renders returned labels/actions and contains no
  task-status outcome table or completion decision logic.

Scoped tests must cover ordinary continuation and refusal cases, plan and
reason carry, worker continuation after reload, complete versus incomplete
worker actions, decision-pending suppression, stale progress presentation,
worker progress and return preservation, and source-level thin-UI assertions.
Stop after Task 3; final documentation, dependency reconciliation,
deterministic evaluation and spec close-out belong to Task 4.

### Global implementation constraints

- The completion gate is application logic in `workspace-engine`. The desktop
  shell, CLI, worker host and JavaScript may transport or render its typed
  result but may not select facts, infer success, translate reason codes, or
  decide whether a task may complete.
- A question answer is not an approval or a check waiver. Only an explicit
  user decline — the check card's `DeclinedByUser` or the exact deferred
  command's `CommandDeclined` (spec 105 R8) — can produce a waiver.
- Do not rewrite historical `complete` tasks or infer reports for them during
  schema migration. The stricter verdict begins with new settlements.
- Keep #102 and #105 boundaries intact: this spec consumes current verification
  dispositions but does not assign failure provenance or derive required
  checks from prose.

## 7. Acceptance and evidence

**Closed 2026-11-01.** All eight requirements and all six acceptance criteria
above are implemented. Nothing in this section is deferred, and no acceptance
criterion depends on #102, #103, #104, #105 or #106.

### Requirements

| Requirement | Evidence |
|---|---|
| 1 · `complete` only through the engine gate | `completion.rs::CompletionGateService::settle` is the only path from a newly settled planned task to `TaskStatus::Complete`; answered turns, successful verification and the plan-review hand-off all settle through it. `completion_settlement` tests cover the answered and verification paths; `plan_turn` proves the hand-off records an eligible report. Synthetic commit/branch tasks carry no plan and keep their own action outcome. |
| 2 · every plan step, verification, pending decision and selected check is closed, and no step is blocked | `CompletionGateReport::evaluate` reads the active plan, folded verification state, newest pending decision and current file hashes. `closed_task_completion` pins the full reason matrix, including due, started-but-unfinished, missing, failed, stale and policy-skipped checks. `a_blocked_plan_step_blocks_the_gate_with_its_own_reason` and `the_service_settles_a_blocked_step_incomplete_and_a_skipped_step_complete` pin a blocked step as outstanding (`blocked_plan_steps`) while a skipped step stays closed. The required set merges the plan's required checks with the R15 floor: `CompletionGateService::report` reads the persisted repository map and `SessionStore::task_touched_paths`, and `verification_loop` tests `a_changed_source_without_a_plan_check_requires_the_discovered_floor_check`, `a_plan_check_identical_to_a_floor_check_is_one_obligation`, `a_documentation_only_change_has_no_floor_and_completes`, `declining_a_floor_check_is_a_named_waiver_and_the_task_completes`, `an_auto_runnable_floor_check_runs_without_a_waiver`, `a_member_change_puts_the_workspace_root_check_in_the_floor`, `two_checks_needing_approval_produce_one_batch_and_one_resume` and `a_failed_verification_is_not_continued` pin it. |
| 3 · declined checks are named waivers, never passes | `CompletionReasonCode` has no waiver variant; `CompletionWaiver` is produced for `SkipReason::DeclinedByUser` on a `CheckRun` and, since spec 105 review fix R8, for `Evidence::CommandDeclined` matching the check's `CheckId`. The check still never enters `passed`. A policy-skipped check is `required_check_skipped` and blocks; a policy refusal, timeout or a model that never ran the command records no decline. |
| 4 · an answer with work left is `incomplete` | `run_agentic_turn` settles `StopReason::Answered` through the gate; `two_answers_with_an_open_plan_settle_incomplete_and_keep_the_plan` proves the single #66 reminder, unchanged plan and two transcript answers. `plan_left_open_at_end` is the deterministic scenario for the same outcome. |
| 5 · terminal but resumable, in an ordinary session and a worker | `TaskStatus::Incomplete` is terminal and not successful. `TaskControl::for_session` and `TaskControl::for_worker` offer **Continue task** with the exact `taskId`; `SessionStore::continue_incomplete_task` writes the new task and `plan_resumed` edge in one locked batch and compares the whole report. `ChatOrchestrator::continue_incomplete_task` builds the prompt from the durable reasons and the source plan. Engine, route and worker tests cover carry, refusal, stale click, wrong repository and the next run settling `complete`. `continuing_a_task_with_only_a_missing_required_check_keeps_the_check` and `continuing_a_task_stopped_by_a_blocked_step_carries_the_blocked_plan` pin that a plan is carried even when every step is terminal or a step is blocked, and `a_continuation_prompt_names_the_carried_plan_even_when_no_step_is_open` pins the prompt. `a_continued_task_cannot_replace_its_carried_open_plan`, `a_continued_task_cannot_replace_its_carried_blocked_plan` and `a_worker_continued_task_cannot_replace_its_carried_blocked_plan` pin that a continuation's model cannot replace the carried plan while it has open or blocked steps or a required check; `a_continued_task_without_open_work_can_propose_a_plan_that_is_reviewed` keeps the replacement path when nothing in the plan is outstanding, and `a_fresh_tasks_first_propose_plan_is_unchanged` pins the fresh-task boundary. |
| 6 · worker progress and #100 preserve the truthful outcome | The worker progress status carries `TaskStatus::as_str()` and `/api/worker-progress` returns the engine `control`; the board row and result card draw its headline/detail. `returned_task_control` labels an incomplete return **Task incomplete** and never success, while #100's **Continue with result** remains a separate action. Source tests prove `app.js` has no task-status outcome table. |
| 7 · recovery recomputes the gate | `recovery::settle_crashed_completions` validates the marker's root, session and task, recomputes the report from the checkout and writes the status through the same service; it never reads a stored `eligible` boolean. Tests cover a crash window settling `incomplete`, `complete` and `failed`, an unusable root staying `interrupted`, an unknown future status being left alone, a dangling side effect outranking the marker, and a marker whose status already landed being closed without a rewrite. |
| 8 · approval boundaries unchanged | Continuing grants no patch, command, check, question or fallback approval. The prompt says so, the route carries only the target id and optional note, and the worker host refuses the command while a turn is running or waiting. A question answer remains neither an approval nor a waiver. |

### Acceptance criteria

| Criterion | Evidence |
|---|---|
| Two answers with an open plan get one reminder and end `incomplete`, plan unchanged | `completion_settlement::two_answers_with_an_open_plan_settle_incomplete_and_keep_the_plan`; deterministic `plan_left_open_at_end`. |
| Completing the remaining steps and checks changes the next run to `complete` | `task_control::completing_the_carried_plan_on_the_next_run_settles_complete`; `completion_settlement::a_closed_plan_settles_complete_with_an_eligible_report`. |
| A declined required check is a named waiver, never pass evidence | `closed_task_completion::a_declined_check_is_a_named_waiver_and_never_a_pass`; the full reason-matrix test. The deferred-command form is pinned by `deferred_check_waiver::declining_the_deferred_required_check_is_a_named_waiver_and_completes`, `a_declined_check_waiver_names_the_check_and_is_never_a_pass`, `declining_an_unrelated_command_is_not_a_waiver`, `declining_the_same_command_in_another_directory_is_not_a_waiver`, the later-run guards and `declining_a_model_run_floor_check_is_a_named_waiver_and_completes`. |
| Due verification, pending patch/question, blocked step or open required step prevents `complete` | `closed_task_completion::every_blocking_fact_produces_its_stable_reason_code`; `closed_task_completion::a_blocked_plan_step_blocks_the_gate_with_its_own_reason` and `the_service_settles_a_blocked_step_incomplete_and_a_skipped_step_complete`; the shell route test where an answered command task with a due verification settles `incomplete`; the worker progress/outcome tests. |
| Reloading an incomplete worker shows **Incomplete — Continue task** and carries the same plan | `worker_supervisor::worker_outcomes_use_engine_control_and_preserve_incomplete`; `worker_turns::continuing_an_incomplete_task_through_the_route_carries_plan_and_reasons`; `worker_turns::a_worker_continues_only_the_exact_task_its_control_named`; engine `task_control` carry test. |
| Historical `complete` events are not rewritten | `closed_task_completion::a_historical_status_event_has_no_report_and_stays_valid`; reports are additive siblings and the stricter rule applies only to new settlements. |

### Behaviour and documentation

- **Complete versus incomplete.** Only an eligible report yields `Task complete`.
  An ineligible answer is shown as **Task incomplete** with the engine's reasons
  and waivers; the answer stays in the transcript but is not the execution
  verdict.
- **Continue task.** The desktop ordinary session and the worker host offer the
  engine-provided action, including after reload. It requires no typed
  "continue", accepts an optional note as data, and re-validates the newest
  task and report under the session-log lock.
- **Recovery.** The settlement marker is non-side-effecting and is written
  before the report is recomputed. A crash in that window is settled from
  current durable state; an unusable repository or unknown status fails safe to
  the ordinary interrupted path.
- **Worker outcomes.** Worker progress, the board and #100's parent result
  preserve the literal status. Complete offers **Send follow-up**, incomplete
  offers **Continue task**, and a decision-pending worker offers only its review
  action.
- **Approvals unchanged.** The gate may stop or continue work; it cannot approve
  anything. Every patch, command, check card, question and provider fallback
  still takes its own explicit decision.

### Verification record

Run locally on 2026-11-01:

- `scripts/quality-gate.sh fmt node lint typos deny`: all five steps passed.
- `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`,
  `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`, and
  `cargo clippy -p eval-harness --all-targets --locked -- -D warnings`: passed.
- `cargo nextest run -p workspace-engine --lib --test closed_task_completion
  --test completion_settlement --test task_control --test crash_recovery
  --test verification_loop --test session_rewind --test plan_turn`: 898 passed,
  9 skipped.
- `cargo nextest run -p desktop-shell -E
  'test(/worker|continue|incomplete|task_control/)'`: 67 passed (1 leaky), 209
  skipped.
- `cargo nextest run -p eval-harness --lib`: 1 passed.
- `cargo run -p eval-harness -- run --tier deterministic`: all 46 scenarios
  passed, including `plan_left_open_at_end` and `plan_answer_without_complete_step`.
  With the settlement marker excluded from tool calls as well as rounds, the
  final metrics record a tool and model error rate of 0.161 and 3.174 model
  calls and tool rounds per task.
- `npm run specs:check -- --strict` and `git diff --check`: passed.

Workspace-wide clippy and the full nextest suite were not run locally, per
`AGENTS.md`; they remain for CI. The deterministic tier is not a live-provider
run, so it proves the engine paths, not model behaviour against a real provider.
The live acceptance for this spec is therefore the truthful state machine above,
not a claim that a worker can finish an arbitrarily large task; #103 and #106
own that longer-horizon evidence.

### Review fix 2026-11-04: a blocked step is outstanding (R1)

The review of specs 101–105 found that the gate used
`StepStatus::is_terminal`, which counts `Blocked`. A plan whose last step failed
its command therefore had no open steps; an answer after that settled
`complete`. The gate now reports `blocked_plan_steps` for a blocked
non-verification step, and `open_plan_steps` reads `TaskPlan::open_steps`, so it
stays the same set as spec 66's reminder. `StepStatus::is_terminal` is
unchanged, so plan-revision protection still keeps a blocked step.

The regression test was written first:
`a_blocked_plan_step_blocks_the_gate_with_its_own_reason` failed with
`CompletionGateReport { task_id: "task_1", eligible: true, reasons: [], waivers: [] }`,
and `the_service_settles_a_blocked_step_incomplete_and_a_skipped_step_complete`
failed with `left: Complete, right: Incomplete`. Both pass now, and
`a_skipped_plan_step_is_closed` pins the boundary.

Continuation is unchanged here: `SessionStore::continue_incomplete_task`
(`session.rs:3204`) still writes `plan_resumed` only when some step is
non-terminal, so a plan whose only outstanding step is blocked does not resume
yet. R2 owns carrying the source plan and reasons for every `incomplete`
settlement.

Checks run locally on 2026-11-04:

- `scripts/quality-gate.sh fmt node lint typos deny`: all five passed after
  `npm ci` (the first `lint` run exited 127 because `node_modules` was absent).
- `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`:
  passed.
- `cargo nextest run -p workspace-engine --test closed_task_completion`: 21
  passed.
- `cargo nextest run -p workspace-engine --locked --no-fail-fast`: 2486 passed,
  30 skipped. An earlier run of the same suite failed once in
  `symbol_index::a_server_that_never_answers_costs_one_timeout_per_file_and_never_delays_a_reader`
  (`assertion left == right failed: every opened file is closed again`, left
  `["src/a.rs", "src/b.rs", "src/c.rs"]`, right `["src/a.rs", "src/b.rs"]`); the
  test passed when rerun alone and in the second full run. No loopback-port test
  failed.

Workspace clippy and the other crates' suites are left to CI.

### Review fix 2026-11-04: a continuation always carries the plan (R2)

The review found that `SessionStore::continue_incomplete_task` wrote
`plan_resumed` only when the source plan still had a non-terminal step. An
`incomplete` settlement can rest on an unmet required check after every step is
terminal, or on a blocked step (terminal but unfinished). In both cases Continue
carried no plan, no required check and no verification chain, so the next model
answer settled `complete` without running anything.

The continuation transaction now writes `plan_resumed` whenever the source task
has a plan. The continued task's gate therefore still sees the plan's unmet
required checks and its blocked steps. `continuation_prompt` now receives the
source plan: it names the carried plan, says explicitly when it includes a
blocked step, and says plainly when the task carries no plan instead of
claiming that no step is open. Continue still grants no approval, and the
automatic continuation in `chat.rs` that reopens only an `incomplete` whose sole
blocker is `open_plan_steps` is unchanged.

The regression tests were written first:
`continuing_a_task_with_only_a_missing_required_check_keeps_the_check` failed
with `assertion left == right failed: answering without the check must not close
the continued task`, left `Complete`, right `Incomplete`; and
`continuing_a_task_stopped_by_a_blocked_step_carries_the_blocked_plan` failed
because the prompt said `No plan step is open, so none is carried.` Both pass
now, along with
`a_continuation_prompt_names_the_carried_plan_even_when_no_step_is_open`.

Checks run locally on 2026-11-04:

- `scripts/quality-gate.sh fmt node lint typos deny`: all five passed after
  `cargo fmt --all` and `npm ci` (the first run failed `fmt` on the new test
  formatting, and `lint` exited 127 because `node_modules` was absent).
- `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`,
  `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`, and
  `cargo clippy -p damaian-cli --all-targets --locked -- -D warnings`: passed.
- `cargo nextest run -p workspace-engine --test task_control`: 10 passed; the
  `task_control` library tests: 12 passed.
- `cargo nextest run -p workspace-engine --locked --no-fail-fast`:
  2488 passed, 30 skipped on the second run. The first run passed 2487 with one
  leaky test and failed the known timing-sensitive
  `shell_path::a_shell_that_does_not_answer_is_killed_at_the_timeout`, which
  panicked at `shell_path.rs:160` with
  `called Result::unwrap() on an Err value: Os { code: 2, kind: NotFound, message: "No such file or directory" }`;
  it passed when rerun alone. No loopback-port test failed.

Workspace clippy and the other crates' suites are left to CI.

### Review fix 2026-11-06: the discovered-check floor (R15)

Spec 105 Task 5 made only plan-declared structured checks completion
obligations. The review of specs 101–105 found the gap: a plan with no `check`
could change source files, run nothing, and settle `complete`. Review fix R15
(decision D1) restores an engine-owned floor. The gate's required set is now:

- the approved plan's `required: true` checks, in plan order, and
- the detected checks of the owning and enclosing roots of every file the task
  touched — applied patch paths and command-changed paths from its continuation
  chain — selected with spec 23 §5.2's enclosing-root rule.

The two sets are merged by `CheckId`: the same identity is one obligation, and
the plan's checks are added to the floor rather than replacing it. A
documentation-only task selects no root and gets no floor. The floor only reads
the persisted repository map through `RepositoryMapStore::load_stored_map`; a
gate report stores no proposal and runs nothing, so command policy and the
ordinary approval route are untouched. Declining a floor check that needs
approval is the existing `DeclinedByUser` waiver; an auto-runnable floor check
simply runs. Review fix R16 (2026-11-06) split selection: per-change
verification uses the owning root only, while this floor keeps the owning and
enclosing roots. The gate evaluates the floor only when no primary plan step is
open or blocked, and a completion verification — or an answered turn blocked
only by a missing or stale required check — runs it through the ordinary check
policy before settlement. The gate itself still runs nothing.

The floor also feeds R14's task-start baseline: `BaselineCoordinator::prepare`
merges the plan's required checks with the floor for the first patch's touched
paths, so a root the first patch touches is baselined on the untouched tree. A
root first touched by a later patch has no task-start baseline and is
`uncertain`, never silently `pre_existing` (spec 102's R15 note).

Regression tests were written first and recorded failing:

- `verification_loop::a_changed_source_without_a_plan_check_requires_the_discovered_floor_check`
  failed `left: Complete, right: Incomplete`.
- `verification_loop::declining_a_floor_check_is_a_named_waiver_and_the_task_completes`
  failed with `CompletionGateReport { eligible: true, reasons: [], waivers: [] }`,
  `left: 0, right: 1`.
- `verification_loop::two_checks_needing_approval_produce_one_batch_and_one_resume`
  failed the restored waiver assertion with `eligible: true` and no waiver.
- `verification_loop::a_failed_verification_is_not_continued` failed the restored
  `RequiredCheckFailed` assertion; the report held only `OpenPlanSteps`.
- `verification_loop::a_member_change_puts_the_workspace_root_check_in_the_floor`
  failed `left: Complete, right: Incomplete`.
- `plan_turn::a_trivial_plan_check_does_not_replace_the_discovered_floor` failed
  because the report named only the plan's `ls`, not the root's `cargo test`.
- `plan_turn::floor_checks_are_baselined_at_task_start_and_a_later_root_is_uncertain`
  failed with `the first patch's floor check has a task-start baseline: {}`.
- `plan_turn::a_floor_check_for_a_root_first_touched_later_is_uncertain_not_pre_existing`
  failed with the later root recorded as `Failed { .. }` from a post-patch
  baseline.
- `completion_settlement::a_crashed_failed_verification_settles_failed_with_its_report`
  failed `assertion failed: !report.eligible` when the failed discovered check
  was not a plan check.

All pass now. `a_plan_check_identical_to_a_floor_check_is_one_obligation`, the
documentation-only case and the auto-run case are guards: they passed before the
change and keep a duplicate-merge or over-broad floor from regressing.

The two reviewed assertions spec 105 Task 5 had re-pointed are restored where D1
makes them true again. `two_checks_needing_approval_produce_one_batch_and_one_resume`
again asserts the declined discovered check is a named waiver; the browser check
is not a root-detected command, so it remains a verification-report fact rather
than a floor reason. `a_failed_verification_is_not_continued` again requires
`RequiredCheckFailed` alongside the open plan step. The crash-settlement test now
uses the floor instead of the plan check Task 5 added to it.

### Review fix 2026-11-06: the floor is a completion obligation (R16)

Review fix R16 (decision D2 piece 2) implements the split the R15 note left
open. Per-change verification selects only the owning root; the floor above is
unchanged and still uses the owning and enclosing roots. Because the floor is a
task-completion obligation, the gate adds its checks only when no primary plan
step is open or blocked — while plan work remains the task is ineligible anyway.
A verification whose plan has no open or blocked primary step is the completion
run and selects the floor through the ordinary approval and command policy.
When an answered turn is at completion, the engine asks which floor checks are
missing or stale and are not plan-declared; if any, it records a completion
`verification_due` carrying those exact identities and runs only those before
the answer becomes final, regardless of any other completion reason. A
plan-declared check never suppresses the floor and is never auto-run; it stays
the model's responsibility. The floor check may be satisfied by a current
`CheckRun` from the loop or by a current `CommandExit` of the identical command
and directory anywhere in the plan (the same matcher and R4 currentness rule the
deferred path uses); a current exit also supersedes a stale earlier `CheckRun`.
The gate itself still executes nothing.

This supersedes three R15 expectations. `verification_loop::a_failed_verification_is_not_continued`
now sees only `OpenPlanSteps` in its mid-plan gate report (the task status is
still `Failed`, set by the verification settlement), because the failed check is
not a floor obligation until completion. `plan_turn::a_trivial_plan_check_does_not_replace_the_discovered_floor`
closes its plan before asking the completion question, so the floor is still
required beside the plan's `ls`. `verification_loop::a_member_check_is_kept_when_the_root_does_not_run_it`
became `a_member_check_is_kept_in_the_completion_floor_when_the_root_does_not_run_it`
and asserts the completion set, because a per-change run now has no enclosing
root to compare against.

The regression tests written first for R16 are
`verification_loop::a_member_change_selects_only_its_owning_root`,
`a_member_change_runs_only_the_members_check_per_change` and
`a_member_change_baselines_its_own_and_enclosing_root_at_task_start` (all failed
on the old enclosing-root selection), plus
`plan_turn::a_member_change_runs_only_the_members_check_per_change_and_the_root_check_at_completion`,
which drives the member check per change, the workspace-root floor check at
completion, the `new_regression` classification from the task-start baseline,
the repair turn and the passing rerun. `a_root_owned_change_still_runs_the_root_check_per_change`
and the documentation-only case are guards.

### R16 follow-up 2026-11-06: the floor is always satisfiable

The first R16 shape had one defect: a plan that declared any required check
suppressed the answer-time floor run, so an enclosing-root floor check could be
`RequiredCheckMissing` forever — the model had no way to satisfy it and
**Continue** could not either. The trigger now asks
`CompletionGateService::floor_checks_needing_verification` for the floor checks
that are missing or stale and are **not** plan-declared, at completion, and
records those exact identities on the `verification_due`. The run is limited to
them, so a plan-declared check is neither suppressed nor auto-run. The
`approved_plan_required_check` evaluation scenario keeps its meaning: a planned
check the model never ran still settles `incomplete`.

Floor evidence also accepts a current `CommandExit` of the identical identity
anywhere in the plan. The deferred path and this floor lookup share one matcher
(`matching_command_evidence`; named `matching_exit_code` before review fix R8
added declines to it) and R4's `deferred_evidence_is_current`; there is no
second currentness rule. A current exit supersedes a stale earlier `CheckRun`,
and review fix R8's declined command is the same named-waiver disposition.

Regression tests written first and recorded failing in `plan_turn`:
`a_plan_declared_member_check_does_not_hide_the_missing_workspace_root_floor`
failed `left: Incomplete, right: Complete` with only
`Required check cargo test (repository root) has no recorded result.`;
`a_missing_floor_check_runs_even_when_a_plan_check_is_also_missing` failed
`the floor check still runs`; and
`a_model_run_floor_command_satisfies_the_floor_without_an_automatic_run` failed
because the loop started an extra completion run for the already-satisfied
floor. All pass now.

### Review fix 2026-11-06: a declined deferred command is a named waiver (R8)

Spec 105's deferred required check runs as an ordinary `run_command` at its plan
step, so it produces no `CheckRun`. This spec's rule that only
`SkipReason::DeclinedByUser` produces a waiver therefore left such a check with
no waiver: declining the command changed nothing the gate read, and the task
stayed `required_check_missing` for good.

The explicit decline is now durable plan evidence, and this gate reads it with
the same `CheckId` matcher as the check's exit. `CompletionWaiver` is still not a
pass and is still named with the check's label. The latest matching command
evidence decides, so a later approved run of the same check supersedes the
decline. A policy refusal, a timeout and a model that never requests the command
are unchanged: none is a waiver. The owning design and its tests live in spec
105 (`Evidence::CommandDeclined`); this spec consumes the disposition.

### Review fix 2026-11-06: a continuation cannot replace its carried plan (R9)

The review of specs 101–105 suspected that `carried_plan_untouched` let a
continuation's `propose_plan` replace the plan it had just carried. The
regression tests confirmed it. A source task settled `incomplete` with open or
blocked steps; **Continue task** carried the plan; in the continued run the
model proposed a different, smaller plan, called `complete_step` on it, and
answered. Because the replacement was the turn's own `plan_created`, the gate
read the new plan, found no open or blocked step, and settled `complete`. The
original work was gone without the user ever seeing the replacement, in both an
ordinary session and a worker session.

The tool schema requires `minItems: 2`, so a one-step replacement is rejected
before dispatch; the reproduction uses a two-step replacement and two
`complete_step` calls, which is the smallest valid shape.

A continuation now refuses `propose_plan` while its carried plan still has an
open (`Pending`/`InProgress`) or `Blocked` step, or a plan-declared required
check. The tool result tells the model to work through the carried steps and
that only the user can change which of them survive, through plan review. The
required check is included for R2's reason: continuing carries the plan so an
unmet check stays visible, and a replacement that dropped it while every step
was completed would let the continued run answer `complete` with the check
unmet. A carried plan with nothing outstanding — every step completed or
skipped and no required check — may still be replaced; that replacement then
takes the ordinary plan-review pause before its first mutating action. The
refusal is specific to the engine's Continue operation
(`ChatTurnOptions::plan_already_carried`): a fresh task's first `propose_plan`
and spec 83's typed-turn carry are unchanged.

Refusal was chosen over routing the replacement into plan review because the
existing review shows the plan currently in force and resumes by asking the
model again; it has no representation for a proposed replacement, so reviewing
the old plan while approving a new one would misdescribe what the user is
approving. The existing second-plan refusal already tells a model with a plan
to work through its steps, and `apply_plan_revision` remains the user-owned way
to drop steps, so extending that refusal is the smallest change that preserves
the review boundary.

Regression tests were written first and recorded failing in `task_control`:

- `a_continued_task_cannot_replace_its_carried_open_plan` failed with
  `assertion left == right failed: the model's smaller plan must not replace
  the carried one`, left `["step_1", "step_2"]`, right `["read", "write"]`.
- `a_continued_task_cannot_replace_its_carried_blocked_plan` failed with
  `assertion left == right failed: the blocked step is still outstanding`,
  left `Complete`, right `Incomplete`.
- `a_worker_continued_task_cannot_replace_its_carried_blocked_plan` failed with
  `assertion left == right failed`, left `Complete`, right `Incomplete`.

All three pass now. `a_continued_task_without_open_work_can_propose_a_plan_that_is_reviewed`
is the guard that the replacement path survives when nothing in the plan is
outstanding: its source task is incomplete on a discovered floor check, which
follows the continuation chain and cannot be hidden by the replacement, and the
new plan still pauses for review before its patch. `a_fresh_tasks_first_propose_plan_is_unchanged`
pins the fresh-task boundary. Both passed before the change and keep it from
becoming a blanket refusal.

Checks run locally on 2026-11-06:

- `scripts/quality-gate.sh fmt node lint typos deny`: all five passed after
  `npm ci` (the first `lint` run exited 127 because `node_modules` was absent).
- `cargo clippy -p workspace-engine -p desktop-shell -p damaian-cli -p
  eval-harness --all-targets --locked -- -D warnings`: passed.
- `cargo nextest run -p workspace-engine`: 2583 passed, 30 skipped, one failure —
  `shell_path::a_shell_that_does_not_answer_is_killed_at_the_timeout` panicked
  at `shell_path.rs:160` with `called Result::unwrap() on an Err value: Os {
  code: 2, kind: NotFound, message: "No such file or directory" }` (the fake
  shell did not write its pid file inside the 500 ms timeout). That is the
  known local-machine timing failure recorded in R0 and R2; the file is
  unchanged by this fix, and the test is not related to plan handling. An
  earlier full run also failed
  `verification_loop::an_explicit_continue_runs_one_more_repair_after_the_limit`
  at `verification_loop.rs:1690` with
  `Io("process 44420 has no readable identity to record")`, which passed alone
  and did not recur. In the first run before the required-check extension,
  `repository_map::a_resumed_proposal_is_re_classified_at_its_own_directory`
  failed, then passed alone, in three runs of its file, and in every later
  full run. No test in `task_control` or `plan_carry_over` failed.
- `cargo nextest run -p desktop-shell`: 285 passed, 18 skipped.
  `cargo nextest run -p damaian-cli`: 17 passed.
  `cargo nextest run -p eval-harness`: 123 passed, 2 skipped.
- `cargo run -p eval-harness -- run --tier deterministic`: all 63 scenarios
  passed.

Workspace clippy and the other crates' suites beyond those four are left to CI.
