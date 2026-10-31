# Feature Spec: Continue Repair at the Limit

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 96 of 96
Plan: none. A defect reported in use, like #73–#95: an exception to the rule
in [`README.md`](README.md).
Depends on: [#23](23_verification_loop/proposal.md) (verification loop),
[#57](57_worker_sessions/proposal.md) (worker sessions),
[#77](77_continue_the_task_after_verification.md) (continuation) and
[#80](80_worker_verifies_after_its_patch.md) (worker verification), all built.

## 1. Motivation

`agent_tool_retry_limit` defaulted to 2. A real repair sequence often needs
more attempts than that, and the limit ended the task as `failed` with no way
forward except typing a generic message. For a worker session the problem was
worse: no user is present to type anything, so it stopped idle with the failing
evidence already recorded.

An interrupted local draft tried to fix this by letting only worker sessions
run a repair turn past the limit automatically. That is not acceptable: it
silently bypassed a bound for one session kind, and it could keep repairing
without a user decision. This spec replaces it with the same explicit action
for both kinds.

## 2. Requirements

1. The default `agent_tool_retry_limit` becomes **16**. A repository may still
   set a lower or higher value; the key remains a Preference.
2. Reaching the limit still records `repair_limit_exhausted`, task status
   `failed`, and the report that says what still fails. No session kind starts
   another repair on its own.
3. Where a verification report is surfaced, a `repair_limit_exhausted` report
   offers **Continue repair**. Activating it is an explicit user decision.
4. One activation starts **exactly one** repair attempt. It does not raise the
   limit and does not schedule further attempts. A second activation while that
   repair is pending is refused without spending a model call. After the next
   verification hits the limit again, the action is offered again.
5. The repair attempt carries the failing verification's plan, including the
   failed check's `CommandExit` and findings, so the request is the same
   findings-based repair request #23 defines.
6. All approval rules are unchanged: a repair patch is still a reviewable
   proposal, and a command or check still pauses on its own card.
7. The action works for ordinary sessions and worker sessions. A worker runs
   the continuation in its own process through its control socket; the shell
   does not run a worker's turn.
8. The ability to offer the action survives a reload: it is derived from the
   durable session log and task list, not from an in-memory turn. It is offered
   only while the limit's verification is still the latest task and that task
   is still `failed`.
9. The report and status stay honest: `failed` is not presented as complete,
   and no attempt is counted as started until the user asks for it.

## 3. Non-goals

- Raising the limit automatically when a repair fails. The configured limit is
  still the bound, and the action is the only way past it.
- Continuing an ordinary `failed` or `no_repair_applied` verification. The
  action is specifically the repair limit's stop.
- Running a worker's turn in the shell.
- Changing check selection, command policy, patch approval, or secret
  scanning.
- Persisting an in-memory button state. A reload redraws the offer from the
  log and the same eligibility rule.

## 4. Design

- **`config.rs`:** `Config::default().agent_tool_retry_limit` is 16, and the
  accepted range becomes 1–32 so the default is representable and can still be
  raised. The key also bounds the browser diagnostic's repeated-failure note
  (#23 context §5); that shared meaning is unchanged.
- **`chat.rs`:**
  - `finish_verification` always records `RepairLimitExhausted` at the limit.
    The worker-only bypass is removed.
  - `ChatOrchestrator::continue_repair` accepts the explicit decision. It
    requires the last finished outcome to be `repair_limit_exhausted`, the
    limit's verification to be the latest task, and that task to be `Failed`.
    It then creates a new task, resumes the failed plan onto it, records
    `verification_repair_continued` in the audit log, and calls the ordinary
    `run_repair_turn` once.
  - The no-patch stop and the per-attempt verification count are unchanged.
  - `verification_changed_files` takes an optional `Due`, because the explicit
    continuation begins after a `Finished` event, not from a due patch.
- **`worker.rs`:** `ControlCommand::ContinueRepair` parses `continue-repair`,
  `TurnJob::ContinueRepair` calls `continue_repair` in the worker's process,
  and the command table refuses an argument it does not name.
- **`desktop-shell/src/lib.rs`:**
  - `POST /api/continue-repair-stream` runs the continuation with the chat's
    model selection, as the other resume routes do.
  - `/api/session` gains `verification: {taskId, outcome, canContinueRepair}`,
    built from the verification events and the latest task. This is what
    redraws the action after a reload.
- **`app.js`:**
  - A live `repair_limit_exhausted` report card gains **Continue repair** with
    a note that it starts one attempt and every approval still applies.
  - A reloaded session draws the same offer after the verification turn's last
    assistant message, from the durable `verification` field. An ordinary
    session runs `/api/continue-repair-stream`; a worker sends
    `continue-repair` through `/api/worker-control`.
- **`damaian-cli`:** `damaian continue-repair <repo> <session>` calls the same
  engine method with `local_user` as the approving actor and prints the same
  result the `verify` command prints.

## 5. Acceptance Criteria

All engine coverage is in `workspace-engine/tests/verification_loop.rs`:
- `the_default_repair_limit_is_sixteen` (in `config.rs`).
- `an_explicit_continue_runs_one_more_repair_after_the_limit`: the limit
  starts no model call; a fresh orchestrator accepts the action; it runs one
  repair turn, proposes (not applies) a patch, carries the failed exit and
  findings into the new task's plan, and the applied patch makes the next
  verification pass with attempts `[0, 1]`.
- `a_worker_at_the_limit_waits_for_an_explicit_continue`: a worker's limit also
  starts no repair call, and the explicit action then starts one.
- `repeated_limit_exhaustion_needs_one_explicit_decision_each_time`: a second
  activation while the repair is pending is refused with no model call, and
  after the next limit is reached the action starts one more attempt.
- `an_explicit_repair_turn_still_waits_for_command_approval`: the command
  pauses on its card, the task is `waiting_for_approval`, and the file is not
  written.
- `desktop-shell`: `session_payload_offers_continue_repair_only_at_the_latest_limit`
  and `a_worker_accepts_continue_repair_and_the_engine_gates_it` cover the
  durable offer and the worker command. The worker command table test accepts
  `continue-repair` and refuses an argument, and
  `worker_control_takes_form_fields` proves `/api/worker-control` forwards it.

## 6. Verification

Local checks, 2026-10-31:
- `scripts/quality-gate.sh fmt node lint typos deny`;
- `cargo clippy -p workspace-engine -p desktop-shell -p damaian-cli --all-targets --locked -- -D warnings`;
- `cargo nextest run -p workspace-engine --test verification_loop --locked` (69/69);
- `cargo nextest run -p workspace-engine --lib --locked` (643/643);
- `cargo nextest run -p workspace-engine --test foundation --test permission_profiles --test repository_config_trust --locked` (321/321);
- `cargo nextest run -p desktop-shell --lib --locked` (236/236);
- `cargo nextest run -p damaian-cli --locked` (7/7);
- `cargo run -p eval-harness --locked -- run --tier deterministic` (all
  46 scenarios passed);
- `npm run specs:check` (records agree).

The workspace clippy and test suite are left to CI. No commit or push was made.
