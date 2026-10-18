# Continue the Stopped Turn Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) · background in §2 of the proposal
**Status:** In progress. Planned 2026-10-18; Task 1 done 2026-10-18, Task 2 done 2026-10-18, Task 3 done 2026-10-18.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `round_budget` pause and the `PendingChatTurn` field | Done 2026-10-18 | Landed: `RoundBudgetPause` (private) and the `round_budget: Option<RoundBudgetPause>` field on `PendingChatTurn` (`#[serde(default)]`), `AgentRoundBudgetProposal` (public) and `round_budget_proposal` on `ChatTurnResult`, `pause_for_round_budget` (mirrors `pause_for_provider_fallback`: saves the turn, `note_pending_approvals`, `await_approval` with `kind: "round_budget"`, returns the proposal; no model call and no `model_call` marker), the `round_budget_pause_response` helper ("Used N of M tool rounds."), and the loop hook before the forced call at `force_final && (no token ceiling || round >= ABSOLUTE_TOOL_ROUND_CAP)`. Serialization: `round_budget_proposal_json` and `roundBudgetProposal` in `chat_result_json`. `PendingApprovalRef`'s doc comment now names `"round_budget"`. An audit record `round_budget_paused` (not in the plan) accompanies the pause like the other kinds. **Tests:** 4 new in `tests/round_budget.rs`, written first and confirmed failing to compile on the missing field, now passing: `a_turn_that_hits_the_limit_wants_a_tool_and_pauses` (8 read-only rounds, `WaitingForApproval`, kind `round_budget`, file on disk, proposal `rounds_used==8/total==8/granted==0`, exactly 8 model calls), `no_round_budget_pause_when_the_model_answers_at_the_forced_call`, `no_round_budget_pause_with_a_token_ceiling_set` (requirement 8, `turn_continued` still fires), `a_paused_round_budget_persists_messages_round_and_turn_options`. **Deviations:** §5.1 and the acceptance criterion "no model call between limit and choice" (plus Step 5 and Step 8) are unambiguous that the pause precedes the forced call, so the forced call is *never* made on the no-ceiling path; Step 1's "ninth scripted response" wording is therefore imprecise — Test 1 scripts 8 read-only rounds and the ninth response is never consumed, and Test 2 drives a model that answers within its rounds (the forced call is deferred to *Answer now* in Task 2). Two existing tests asserted the old "stop dead" behaviour and had to change: `foundation.rs::tool_round_exhaustion_is_terminal_when_model_still_requests_tools` → `tool_round_exhaustion_pauses_when_model_still_requests_tools`, and `token_ceiling.rs::without_a_ceiling_the_round_cap_still_stops_the_turn` → `without_a_ceiling_the_round_cap_pauses_the_turn` — both outside the named files but forced by the loop change. **Falsified:** dropping the token-ceiling guard fails `no_round_budget_pause_with_a_token_ceiling_set`; moving the pause after the forced call fails `a_turn_that_hits_the_limit_wants_a_tool_and_pauses` (turn ends `ToolBudgetExhausted`, a `model_request_prepared` is emitted for the forced call). Both reverted. **Checks:** `fmt --check`, `node`, `lint`, `typos`, `deny` clean; `clippy -D warnings` clean on `workspace-engine` and `desktop-shell`; `nextest` 1293 passed / 24 skipped (`workspace-engine`) and 158 passed / 17 skipped (`desktop-shell`). **For Task 2:** `resume_round_budget` must seed `round_budget_extension` from `rounds_granted` (C3) and refuse the other kinds; `at_hard_cap` is hardcoded `false` here — Task 3 computes it from `ABSOLUTE_TURN_ROUND_CAP`. §2 and §5.2 line numbers in `proposal.md` were re-checked against the post-merge tree and updated. |
| 2 · `resume_round_budget` with Continue and Answer now | Done 2026-10-18 | Landed: `RoundBudgetDecision` (public, module scope) and `resume_round_budget`, which `take`s the pending turn, refuses a non-`round_budget` kind (`InvalidInput`, put back), and dispatches: *Continue* appends "You have N more tool rounds. Continue the task.", records `round_budget_decision` and `turn_budget_extended` (actor=user, round, `roundsGranted` delta, `roundsTotal` running total), seeds `round_budget_extension` from `rounds_granted`, and re-enters `run_agentic_turn` at `pending.round`; *Answer now* records the decision and re-enters with the forced final call. Two new `ChatTurnOptions` fields carry the seed and the answer-now bypass — `round_budget_extension: u32` (seeds the local) and `round_budget_answer_now: bool` (skips the §5.1 pause so the forced call runs); both `#[serde(default)]`. `resume_after_command_decision_with_options` gains an explicit `round_budget` guard mirroring the plan/fallback ones; `resume_after_fallback_decision` and `resume_after_plan_decision` already refuse via their generic `else`. `RoundBudgetDecision` re-exported from `lib.rs`. **Tests:** 4 new in `tests/round_budget.rs`, written first and confirmed failing to compile on the missing entry point, now passing: `continue_resumes_the_same_task_with_more_rounds` (same task id; 9th request replays the prompt and the `a.rs` tool results; `turn_budget_extended` with `roundsGranted:8`/`roundsTotal:8`), `answer_now_makes_exactly_one_forced_final_call` (exactly 9 calls, `ToolBudgetExhausted`), `resume_round_budget_refuses_a_foreign_pause`, `command_and_fallback_resume_refuse_a_round_budget_pause`. **Deviations:** the seed and the answer-now bypass travel through `ChatTurnOptions` (two new fields) rather than a new `run_agentic_turn` parameter, because the turn options already survive a pause and a restart and the method already takes them; the Interfaces section records the fields. The *Continue* delta is `default_tool_round_limit()` (matching the token-ceiling continuation at the `turn_continued` site) rather than special-casing web debug, since the base limit is recomputed from `web_debug_mode` inside the loop and the seed carries only the running total. `turn_budget_extended` is written here (Task 2 Step 3) even though the Interfaces section tags the record as Task 3's; Task 3 asserts it. **Falsified:** zeroing the seed fails `continue_resumes_...` (turn re-pauses, 8 requests not 9); dropping the answer-now bypass fails `answer_now_makes_...` (8 requests, re-pauses). Both reverted. **Checks:** fmt/typos/deny/node/lint clean; clippy `-D warnings` clean on `workspace-engine`; nextest 1297 passed / 24 skipped (`workspace-engine`). **For Task 3:** `at_hard_cap` is still hardcoded `false`; `turn_budget_extended` is already recorded by `resume_round_budget`'s *Continue* branch (round, `roundsGranted`, `roundsTotal`) — Task 3 adds `ABSOLUTE_TURN_ROUND_CAP`, computes `at_hard_cap`, and asserts the audit. |
| 3 · `ABSOLUTE_TURN_ROUND_CAP` and `turn_budget_extended` | Done 2026-10-18 | Landed: `ABSOLUTE_TURN_ROUND_CAP` (`const`, value 64, `chat.rs:4546`, beside `ABSOLUTE_TOOL_ROUND_CAP`), `at_hard_cap` computed in `pause_for_round_budget` (`round >= ABSOLUTE_TURN_ROUND_CAP`), the two `max_rounds` computations now `.min(ABSOLUTE_TURN_ROUND_CAP)` instead of `.min(ABSOLUTE_TOOL_ROUND_CAP)` (closing the gap where a second *Continue* granted nothing and re-paused at once), and `resume_round_budget`'s *Continue* branch refuses at the hard cap (`InvalidInput`, pending turn saved back for *Answer now*). `turn_budget_extended` was already written by Task 2; Task 3 asserts it. **Tests:** 2 new in `tests/round_budget.rs`, written first and confirmed failing, now passing: `each_continuation_is_recorded` (two consecutive *Continue*s each gain a segment — `rounds_granted` 0→8→16, `rounds_used` 24; two `turn_budget_extended` records with `actor`, `round` 8/16, `roundsGranted` 8 delta, `roundsTotal` 8/16), `a_repeatedly_continued_turn_still_ends` (a model that never answers reaches the cap after 7 continuations — 64 = 8 + 7×8 — with `rounds_used` 64, `rounds_granted` 56, `at_hard_cap` true, and *Continue* refused while the pending turn stays on disk). The third listed test, `turn_continued_still_fires_with_a_token_ceiling`, is #47's existing `token_ceiling.rs::a_turn_continues_past_its_round_cap_under_the_ceiling`, unchanged and still passing. **Deviations:** `at_hard_cap` is computed from `round` (the round that hit the limit) rather than `rounds_granted`, because `rounds_granted` tops out at 56 while the cap is measured in total rounds (64). **Falsified:** pointing the hard cap at `ABSOLUTE_TOOL_ROUND_CAP` fails `a_repeatedly_continued_turn_still_ends` (1 continuation, not 7); dropping `roundsTotal` from the audit fails `each_continuation_is_recorded` (`Null` vs `"8"`). Both reverted. **Checks:** `fmt --check`, `node`, `lint`, `typos`, `deny` clean; clippy `-D warnings` clean on `workspace-engine`; nextest 1299 passed / 24 skipped (`workspace-engine`). **For Task 4:** `at_hard_cap` travels through `AgentRoundBudgetProposal` and is already serialized as `atHardCap` (`desktop-shell` `round_budget_proposal_json`), so the recovery card can offer only *Answer now* at the cap with no shell change. |
| 4 · Crash recovery and worker sessions | Not started | |
| 5 · UI row and the *Ask again* path | Not started | |
| 6 · The `continue_keeps_state` eval scenario | Not started | |
| 7 · Docs and close-out | Not started | |

**Goal:** When a turn reaches its round limit with the model still wanting a
tool, pause it with its full in-turn state instead of stopping it dead, and let
the user *Continue* (more rounds, same task) or *Answer now* (the forced final
call). Replace the *Continue debugging* button, which today starts a fresh turn
that sees only the last eight messages and forces web debug mode.

**Architecture:** One new pause kind on the machinery spec 56 already built: a
`round_budget` pending approval persisted as a `PendingChatTurn`, reattached by
`recovery.rs`, answered by a new `resume_round_budget` entry point, and rendered
by the desktop shell like the provider-fallback offer. `ABSOLUTE_TURN_ROUND_CAP`
bounds the turn across continuations, and the old `ToolBudgetExhausted` stop
becomes an *Ask again* path that carries the request and plan into a new turn.

**Tech Stack:** Rust 2024 (workspace edition). No new dependencies are expected:
the pause, the audit record and the resume reuse the pending-turn store, the
session log and the approval machinery already in `workspace-engine`. If a task
finds it needs a new crate, it must stop and justify the crate in the Progress
table first.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`proposal.md`](proposal.md) §2 and §5 and the corrections below before
  starting.** The corrections were found while planning, on 2026-10-18, by
  reading the code. Where one contradicts the proposal's prose, the correction
  wins, and the task that acts on it updates the proposal in the same change.
- **Follow spec 56's shape, do not invent a second one.** The `provider_fallback`
  offer is the template: a `PendingChatTurn` field, a `PendingApprovalRef` kind,
  a `ChatTurnResult` field, a resume entry point, a `PausedTurns` reader, a
  `ReattachedApproval` arm, a shell route and a UI card. If a task finds itself
  building pause/resume plumbing that is not the fallback's, it has left the
  spec. Stop and say so.
- **Every resume entry point refuses another kind's pause.** `resume_round_budget`
  must refuse a command, plan, patch or fallback pending turn, and the existing
  `resume_after_command_decision` / `resume_after_plan_decision` /
  `resume_after_fallback_decision` must refuse a `round_budget` one (C5). This is
  the same rule specs 23 and 50 record for their own kinds.
- **A resumed turn is the same turn, never a new one** (requirement 2). The
  pending turn's `messages`, `round`, `task` and plan are replayed through
  `run_agentic_turn`; nothing re-reads the user request from
  `build_model_prompt`'s eight-message window.
- **No fixed prompt and no web debug mode** (requirement 3). No path sends the
  "Continue debugging…" string, and `continue_debugging` must not force
  `web_debug_mode` anywhere (C4).
- **The token ceiling keeps #47's continuation** (requirement 8). The round-budget
  pause must not fire when `agent_max_task_tokens` is set and
  `round < ABSOLUTE_TOOL_ROUND_CAP`; that path already records `turn_continued`
  (`chat.rs:2400`). Do not regress it, and its tests must still pass.
- **`chat.rs` is shared.** Specs 59, 60 and 64 are in flight on other worktrees
  and edit `chat.rs` around the round loop, the tool definitions and
  `command_policy.rs`. This spec pairs with none of them
  (`docs/specs/README.md`, "Parallel work"). Keep the edits to the round-budget
  sites and do not reflow unrelated code.
- **Tests:** fixture repositories set `enable_index_watcher: false`. Every test
  uses its own data directory. Model-dependent paths use `MockModelAdapter` or
  `DAMAIAN_MOCK_MODEL_RESPONSE`. Anything that spawns a real worker process,
  binds a socket outside a temp dir, or `SIGKILL`s something is `#[ignore]`d,
  with the exact manual command in its doc comment.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record that in the Progress row.
- **A private Cargo target directory per worktree.** Prefix every `cargo`
  command with `CARGO_TARGET_DIR="$PWD/target"` from the worktree root.
- **Scoped checks per task; the full gate runs on CI only** (AGENTS.md, since
  2026-10-15). Each task runs `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p <crate> --all-targets --locked -- -D warnings` for every crate
  it touches, and the test files it names. The workspace-wide clippy and
  nextest runs are left to CI.
- **UI verification** runs the shell on a port other than 4765, with its own
  `DAMAIAN_DATA_DIR`. Static assets are `include_str!`-embedded, so rebuild and
  restart before checking. Stop the instance by its PID.
- **Never `git commit` or `git push` unasked.** Each task ends by showing the
  change and the scoped check results. One-line commit subject, no body.

## Corrections to the proposal found while planning

Each one names the task that owns it.

| # | Proposal said | Code says | Resolution |
|---|---|---|---|
| C1 | §2's line numbers: `tool_budget_exhausted_response` at `chat.rs:2324,5238`; `turn_continued` at `2333`; new task at `774`; `web_debug_mode` at `1900`; `build_model_prompt` at `4184`; `carry_plan_from_the_previous_turn` at `1680`; `PendingChatTurn` at `3921`; reattach at `recovery.rs:315` | `tool_budget_exhausted_response` is *called* at `chat.rs:2364` and *defined* at `5244` (`5238` is inside `token_budget_exhausted_response`); `turn_continued` at `2342`; task creation at `780`; `web_debug_mode` seed at `1906`; `build_model_prompt` at `4190`; `carry_plan_from_the_previous_turn` at `1686`; `PendingChatTurn` at `3927`; `reattach_pending_approvals` at `recovery.rs:311` | §2 rewritten with the real lines. Every task re-checks its references before editing. **Task 1** re-checked on 2026-10-18 after the spec 60/63 merges and rewrote §2 again: called at `chat.rs:2421`/defined `5630`, `turn_continued` `2400`, task `807`, `web_debug_mode` seed `1942`, `build_model_prompt` `4445`, `carry_plan_from_the_previous_turn` `1713`, `PendingChatTurn` `4163`, `ABSOLUTE_TOOL_ROUND_CAP` `4372`, `reattach_pending_approvals` `recovery.rs:311` (unchanged) |
| C2 | §5.3's `ABSOLUTE_TURN_ROUND_CAP` bounds one turn across continuations | The only existing cap is `ABSOLUTE_TOOL_ROUND_CAP` (`chat.rs:4372`), value **16**, and the default segment is `agent_max_tool_rounds` (`config.rs:2570`), value **8** | `ABSOLUTE_TURN_ROUND_CAP` is a new, distinct constant (64). §2 and §5.3 updated; §5.3's "eight continuations" was 64 = 8 + 7×8. **Task 3** |
| C3 | §5.2 "adds … to the turn's `round_budget_extension`" | `round_budget_extension` is a *local* of `run_agentic_turn` (`chat.rs:1981`), not persisted | `RoundBudgetPause.rounds_granted` is the running total; the resumed turn seeds `round_budget_extension` from it. §5.2 updated. **Tasks 1, 2** |
| C4 | §5.7 "the `continue_debugging` turn option no longer forces web debug mode" | `continue_debugging` is read in two places — `tool_round_limit` (`chat.rs:568`) returns `ABSOLUTE_TOOL_ROUND_CAP` for it, and the `web_debug_mode` seed (`chat.rs:1942`) ORs it in — plus the field itself (`chat.rs:451`) | Both sites and the field are touched; the field is removed or repurposed. §5.7 updated. **Task 5** |
| C5 | §5.2 adds `resume_round_budget` | The existing resume entry points already refuse the wrong kind: `resume_after_command_decision_with_options` refuses `plan_review` and `provider_fallback` (`chat.rs:914`), and `resume_after_fallback_decision` refuses a non-fallback (`chat.rs:1350`) | `round_budget` joins those refusals, and `resume_round_budget` refuses the other kinds, mirroring specs 23 and 50. **Task 2** |
| C6 | §5.2 "the request the user typed is the first message of the saved array" | The saved array is `[system prompt, user request, …]` (`chat.rs:869`), and the user's text is inside `build_model_prompt`'s assembled prompt, not a standalone first message; the raw text also sits on `task.user_prompt` | §5.2 rewritten. The *Ask again* prompt uses `task.user_prompt`, not the message array. **Tasks 2, 5** |
| C7 | §5.8 the `continue_keeps_state` scenario is a scenario file | The runner's `drive` (`runner.rs:261`) drives one `ask()` and resumes command decisions via a `bool` `approval_decision`; there is no field or call for a round-budget decision, and `MockModelAdapter` repeats its last response when the sequence runs out | The scenario needs a new `RoundBudgetDecision` option, a `resume_round_budget` call in `drive`, and a full twelve-response script. §5.8 updated. **Task 6** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/chat.rs` | `RoundBudgetPause`, the `round_budget` field on `PendingChatTurn` and `AgentRoundBudgetProposal` on `ChatTurnResult` (Task 1); `resume_round_budget` + `RoundBudgetDecision` and the kind refusals (Task 2); `ABSOLUTE_TURN_ROUND_CAP`, the `turn_budget_extended` audit and the `rounds_granted` seed (Tasks 2, 3); `carry_plan_from_the_previous_turn` gains `ToolBudgetExhausted`, and the `continue_debugging` web-debug seed is dropped (Task 5) |
| `crates/workspace-engine/src/session.rs` | `PendingApprovalRef` doc comment names `round_budget`; no `TaskStatus` variant is added (the pause uses `WaitingForApproval`) |
| `crates/workspace-engine/src/recovery.rs` | `ReattachedApproval::RoundBudget` arm and a `round_budget` match in `reattach_pending_approvals` (Task 4) |
| `crates/desktop-shell/src/lib.rs` | `round_budget_proposal_json`, the `chat_result_json` field, and the `/api/resume-round-budget-stream` route (Tasks 1, 5) |
| `crates/desktop-shell/src/worker.rs` | Answers a `round_budget` pause in-process through `resume_round_budget` (Task 4) |
| `crates/desktop-shell/static/app.js` | The pause row (*Continue*/*Answer now*), the recovery card, and the *Ask again* path for stored `ToolBudgetExhausted` tasks (Task 5) |
| `crates/eval-harness/src/scenario.rs`, `assertions.rs`, `runner.rs` | The `RoundBudgetDecision` scenario option, the `resume_round_budget` call, and the continuation count (Task 6) |
| `crates/eval-harness/scenarios/continue_keeps_state.toml` | **New.** The scenario (Task 6) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | The pause, the two buttons, and a task waiting on `round_budget` (Task 7) |

## Interfaces

Names fixed here so that tasks run in separate sessions agree. A task that needs
to change one records the change in its Progress row and updates this section.

```rust
// chat.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RoundBudgetPause {
    rounds_used: u32,    // the round that hit the limit
    rounds_granted: u32, // running total of granted rounds across continuations
}

// PendingChatTurn gains, beside `provider_fallback`:
#[serde(default)]
round_budget: Option<RoundBudgetPause>,

// The proposal surfaced to the user, beside AgentFallbackProposal:
pub struct AgentRoundBudgetProposal {
    pub id: String,          // the proposal_id, == the pending-turn key
    pub rounds_used: u32,
    pub rounds_total: u32,   // max_rounds the turn had this segment
    pub rounds_granted: u32, // running total across continuations
    pub at_hard_cap: bool,   // ABSOLUTE_TURN_ROUND_CAP reached -> only Answer now
}

// ChatTurnResult gains:
pub round_budget_proposal: Option<AgentRoundBudgetProposal>,

// chat.rs — the constant (Task 3)
const ABSOLUTE_TURN_ROUND_CAP: u32 = 64;

// chat.rs — the resume decision and entry point (Task 2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundBudgetDecision { Continue, AnswerNow }

pub fn resume_round_budget(
    &self,
    proposal_id: &str,
    decision: RoundBudgetDecision,
    approved_by: &str,
    model_adapter: &mut dyn ModelAdapter,
    sink: &mut TurnSink<'_>,
) -> Result<ChatTurnResult>;

// ChatTurnOptions gains (Task 2), beside provider_fallback_used:
//   round_budget_extension: u32  — seeds run_agentic_turn's local from rounds_granted
//   round_budget_answer_now: bool — the Answer-now re-entry skips the §5.1 pause
// Both #[serde(default)]; callers leave them at their defaults.

// PausedTurns gains (Task 4), mirroring provider_fallback_target:
pub fn round_budget_pause(&self, proposal_id: &str) -> Option<RoundBudgetPause>;

// recovery.rs — ReattachedApproval gains (Task 4)
RoundBudget {
    task_id: String,
    proposal_id: String,
    rounds_used: u32,
    rounds_granted: u32,
},
```

The audit record mirrors `turn_continued` (`chat.rs:2400`). The record call is
written by Task 2's *Continue* branch; Task 3 asserts it and adds
`ABSOLUTE_TURN_ROUND_CAP`:

```text
turn_budget_extended  actor=user  sessionId  taskId  round  roundsGranted  roundsTotal
```

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6 ──> 7
```

Sequential. Tasks 1–5 all edit `chat.rs`, and 4 and 5 both touch the shell.
Task 1 must land before the pause can be resumed or recovered, and Task 5's
*Ask again* path needs Task 3's hard cap in place. None of these tasks may run
beside spec 59, 60 or 64, which also edit `chat.rs`.

---

## Task 1: The `round_budget` pause and the `PendingChatTurn` field

**Requirements:** 1. **Proposal:** §5.1. **Corrections:** C1, C3.
**Files:** `chat.rs`, `session.rs` (doc comment only), `desktop-shell/src/lib.rs`
(serialization only).

- [x] **Step 1: Write the failing tests** in `crates/workspace-engine/tests/` (a
      new `round_budget.rs`, or the existing chat test file, whichever the crate
      convention prefers), each through `MockModelAdapter` with
      `enable_index_watcher: false` and a fresh data directory:
  - `a_turn_that_hits_the_limit_wants_a_tool_and_pauses`: eight read-only rounds
    (the default `agent_max_tool_rounds`), then a ninth scripted response that
    requests a tool. The turn stops with the task `WaitingForApproval`, a
    `PendingApprovalRef { kind: "round_budget" }`, a pending-turn file on disk,
    and `round_budget_proposal: Some(..)` on the result.
  - `no_round_budget_pause_when_the_model_answers_at_the_forced_call`: the ninth
    response answers with text, no tool; the turn completes, no pause, no file.
  - `no_round_budget_pause_with_a_token_ceiling_set` (requirement 8): with
    `agent_max_task_tokens` set and `round < ABSOLUTE_TOOL_ROUND_CAP`, the turn
    records `turn_continued` and continues instead of pausing.
  - `a_paused_round_budget_persists_messages_round_and_turn_options`: read the
    file back (or via `PausedTurns`) and assert `messages`, `round`,
    `turn_options` and `round_budget` are all present.
- [x] **Step 2: Run them and confirm they fail** (the field and type do not
      exist, so they fail to compile).
- [x] **Step 3: Add `RoundBudgetPause` and the `round_budget` field** on
      `PendingChatTurn` (`#[serde(default)]`, beside `provider_fallback`), and
      `AgentRoundBudgetProposal` with the `round_budget_proposal` field on
      `ChatTurnResult`.
- [x] **Step 4: Add `pause_for_round_budget`** following `pause_for_provider_fallback`
      (`chat.rs:3516`): save the `PendingChatTurn` with `round_budget: Some(..)`,
      `note_pending_approvals`, `await_approval` with kind `"round_budget"`, and
      return a `ChatTurnResult` carrying the proposal (no model call, no
      `model_call` marker — nothing is in flight).
- [x] **Step 5: Hook the pause into the loop**, at the `force_final` block
      (`chat.rs:2098`): *before* the forced call is made, when
      `round >= max_rounds` and the turn is not eligible for #47's continuation
      (no token ceiling, or `round >= ABSOLUTE_TOOL_ROUND_CAP`), pause instead of
      making the forced call. The forced call itself stays for the *Answer now*
      path (Task 2).
- [x] **Step 6: Serialization.** Add `round_budget_proposal_json` and the
      `roundBudgetProposal` field to `chat_result_json` (`desktop-shell/src/lib.rs:3748`)
      so the shell carries it; rendering is Task 5.
- [x] **Step 7: Update `PendingApprovalRef`'s doc comment** (`session.rs:244`) to
      name `"round_budget"`.
- [x] **Step 8: Falsify.** Drop the token-ceiling guard and confirm the
      requirement-8 test fails. Move the pause after the forced call and confirm
      the "no call between limit and choice" acceptance is violated (a
      `model_request_prepared` appears). Revert both.
- [x] **Step 9: Update the proposal** for C1 and C3.
- [x] **Step 10: Scoped checks.** `cargo nextest run -p workspace-engine` (the
      result-type change touches call sites), `cargo nextest run -p desktop-shell`,
      `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`,
      and the fast checks. Then show the change and ask.

## Task 2: The resume entry point with Continue and Answer now

**Requirements:** 2, 3. **Proposal:** §5.2. **Corrections:** C3, C5, C6.
**Files:** `chat.rs`.

- [x] **Step 1: Write the failing tests** (continue in `round_budget.rs`):
  - `continue_resumes_the_same_task_with_more_rounds`: `resume_round_budget(..,
    Continue)` re-enters `run_agentic_turn` with the saved `messages` and `round`
    and the same task id; `rounds_granted` grows by `agent_max_tool_rounds` and
    `round_budget_extension` is seeded from it; the next model request is the
    ninth and its `messages` contain the earlier tool results and the user's
    original request (C6).
  - `answer_now_makes_exactly_one_forced_final_call`: `AnswerNow` re-enters with
    `force_final` set; if the model still asks for a tool the turn ends
    `ToolBudgetExhausted`, otherwise `Complete`; exactly one model call is made
    on resume.
  - `resume_round_budget_refuses_a_foreign_pause`: a command/plan/fallback
    pending turn is put back and an `InvalidInput` is returned.
  - `command_and_fallback_resume_refuse_a_round_budget_pause`: the existing
    `resume_after_command_decision` and `resume_after_fallback_decision` return
    `InvalidInput` for a `round_budget` pending turn (C5).
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Add `RoundBudgetDecision` and `resume_round_budget`**, taking the
      pending turn, refusing a non-`round_budget` kind, and dispatching:
      *Continue* appends the "You have N more tool rounds. Continue the task."
      user message, seeds `round_budget_extension` from `rounds_granted`, and
      re-enters; *Answer now* re-enters with `force_final` set. Record the
      decision and audit `turn_budget_extended` (the audit itself is Task 3's
      constant; add the record call here so Task 3 can assert it).
- [x] **Step 4: Extend the kind refusals** in `resume_after_command_decision_with_options`
      and `resume_after_fallback_decision` (C5), mirroring the existing
      `plan_review` / `provider_fallback` guards.
- [x] **Step 5: Falsify.** Reset `round_budget_extension` to zero on resume and
      confirm the "more rounds" test fails. Answer-now without `force_final`
      fails the "exactly one forced call" test. Revert.
- [x] **Step 6: Update the proposal** for C3, C5, C6.
- [x] **Step 7: Scoped checks.** `cargo nextest run -p workspace-engine`,
      scoped clippy, fast checks. Show and ask.

## Task 3: `ABSOLUTE_TURN_ROUND_CAP` and the `turn_budget_extended` audit

**Requirements:** 4, 5. **Proposal:** §5.3, §5.4. **Corrections:** C2.
**Files:** `chat.rs`.

- [x] **Step 1: Write the failing tests:**
  - `a_repeatedly_continued_turn_still_ends`: continue a turn until
    `rounds_granted >= ABSOLUTE_TURN_ROUND_CAP`; the pause's
    `at_hard_cap` is true and `AnswerNow` is the only decision offered (the
    engine refuses or does not offer `Continue`).
  - `each_continuation_is_recorded`: after two `Continue`s, the session log has
    two `turn_budget_extended` audit events with `actor=user`, the `round`, the
    `roundsGranted` delta and the running `roundsTotal`.
  - `turn_continued_still_fires_with_a_token_ceiling`: unchanged from #47, to
    guard requirement 8.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Add `ABSOLUTE_TURN_ROUND_CAP`** (64, distinct from
      `ABSOLUTE_TOOL_ROUND_CAP`), and compute `at_hard_cap` in
      `pause_for_round_budget`. The pause and the resume entry point consult it
      rather than the per-segment cap.
- [x] **Step 4: Write the `turn_budget_extended` audit record** with the fields
      above, in the same place `turn_continued` is recorded.
- [x] **Step 5: Falsify.** Point the hard cap at `ABSOLUTE_TOOL_ROUND_CAP` and
      confirm the "still ends" test fails at the wrong bound. Drop the
      `roundsTotal` field and confirm the audit test fails. Revert.
- [x] **Step 6: Update the proposal** for C2 (already in §5.3).
- [x] **Step 7: Scoped checks.** `cargo nextest run -p workspace-engine`, scoped
      clippy, fast checks. Show and ask.

## Task 4: Crash recovery and worker sessions

**Requirements:** 6. **Proposal:** §5.5. **Corrections:** none new (C1's line
check applies to `recovery.rs:311`).
**Files:** `chat.rs` (`PausedTurns` reader), `recovery.rs`,
`desktop-shell/src/worker.rs`, `desktop-shell/src/lib.rs` (worker route mapping).

- [ ] **Step 1: Write the failing tests:**
  - `a_paused_round_budget_reattaches_after_a_restart`: write the
    `WaitingForApproval` status with a `round_budget` link, then
    `reattach_pending_approvals` returns `ReattachedApproval::RoundBudget` with
    the saved `rounds_used`/`rounds_granted`.
  - `a_missing_round_budget_file_fails_the_task`: delete the pending file; the
    task is failed with a stated reason and `pending_approval_unavailable` is
    audited (mirroring the other kinds).
  - `an_unknown_round_budget_kind_is_not_reattached`: the `Some(PendingApprovalRef
    { kind, .. })` fallback arm still fails unknown kinds.
  - A worker test: `a_worker_answers_a_round_budget_pause_in_process` (scripted
    through the worker's approve path, no shell reads the pending file).
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Add `PausedTurns::round_budget_pause`** and the
      `ReattachedApproval::RoundBudget` arm in `reattach_pending_approvals`
      (`recovery.rs:335` block), failing with a stated reason when the file is
      gone.
- [ ] **Step 4: Worker.** In `desktop-shell/src/worker.rs`, map the worker's
      approval command to `resume_round_budget` (Continue or Answer now from the
      socket command), so the worker answers in its own process — the same
      in-process path its command approvals use.
- [ ] **Step 5: Falsify.** Read the pending turn through the shell's engine
      instead of `PausedTurns` and confirm the worker test fails. Skip the
      missing-file arm and confirm the crash test fails. Revert.
- [ ] **Step 6: Update the proposal** if any §5.5 detail changed.
- [ ] **Step 7: Scoped checks.** `cargo nextest run -p workspace-engine -E
      'test(crash_recovery)'`, the worker tests in `desktop-shell`, scoped clippy
      for both crates, fast checks. Show and ask.

## Task 5: The UI row and the *Ask again* path

**Requirements:** 3, 7. **Proposal:** §5.6, §5.7. **Corrections:** C4, C6.
**Files:** `desktop-shell/static/app.js`, `desktop-shell/src/lib.rs` (route),
`chat.rs` (`carry_plan_from_the_previous_turn`, the `continue_debugging` seed).

- [ ] **Step 1: The pause row.** In place of `markMessageToolBudgetExhausted`
      (`app.js:6967`), render "Used N of M tool rounds." with *Continue — 8
      more* and *Answer now* buttons, posting to the new
      `/api/resume-round-budget-stream` route. The row sits where the turn
      indicator is (#58); the buttons say nothing about debugging.
- [ ] **Step 2: The reattached card.** Add a `round_budget` recovery card
      (beside `provider_fallback` at `app.js:4764`/`4801`) that offers the same
      two decisions after a restart.
- [ ] **Step 3: The route.** Add `/api/resume-round-budget-stream` in
      `desktop-shell/src/lib.rs` (mirroring `/api/resume-fallback-stream`,
      `lib.rs:346`/`1571`), calling `resume_round_budget`.
- [ ] **Step 4: *Ask again* for old tasks.** For a stored `ToolBudgetExhausted`
      task, replace the button with *Ask again*, which starts a new turn whose
      prompt is built by the engine from `task.user_prompt` ("Continue this
      earlier request: …"), not by the UI.
- [ ] **Step 5: `carry_plan_from_the_previous_turn`** (`chat.rs:1686`) also
      carries the plan from a `ToolBudgetExhausted` task (add it to the
      `handed_over` condition at `chat.rs:1692`).
- [ ] **Step 6: Drop the forced web debug mode** (C4): remove the
      `turn_options.continue_debugging || …` term at `chat.rs:1906`, and remove
      or repurpose the `continue_debugging` branch in `tool_round_limit`
      (`chat.rs:541`) and the field itself. Grep for any remaining
      `continue_debugging` and the fixed prompt string and confirm none remains.
- [ ] **Step 7: Falsify.** Restore the fixed prompt and confirm a grep-based
      check (or the `app.js` scratch script) fails. Restore the web-debug term
      and confirm a test that a continued turn stays out of web debug mode
      fails. Revert.
- [ ] **Step 8: Update the proposal** for C4 and C6.
- [ ] **Step 9: Scoped checks.** `cargo nextest run -p workspace-engine -p
      desktop-shell`, scoped clippy for both, `node --check
      crates/desktop-shell/static/app.js`, `npm run lint:web`, fast checks. UI
      verification on a non-4765 port. Show and ask.

## Task 6: The `continue_keeps_state` eval scenario

**Requirements:** acceptance §6. **Proposal:** §5.4, §5.8. **Corrections:** C7.
**Files:** `crates/eval-harness/src/{scenario.rs,assertions.rs,runner.rs}`,
`crates/eval-harness/scenarios/continue_keeps_state.toml`.

- [ ] **Step 1: The scenario file**, per §5.8: twelve scripted responses (eight
      read-only rounds, a ninth that asks for a tool, then three more), and the
      assertions (same task id finishes, the ninth request carries the first
      turn's tool results and the original prompt, `turn_budget_extended` once,
      no second task). A second case answers *Answer now* and asserts one forced
      final call and the terminal outcome.
- [ ] **Step 2: The runner.** Add a `RoundBudgetDecision` scenario option and,
      in `drive` (`runner.rs:261`), detect the `round_budget_proposal` on the
      outcome and call `resume_round_budget` accordingly, superseding the
      original outcome as the command-approval path does.
- [ ] **Step 3: Record continuations** (§5.4): count `turn_budget_extended` in
      the run record so the metric is reported per run.
- [ ] **Step 4: Run it and confirm it fails** before the runner support lands,
      then make it pass.
- [ ] **Step 5: Falsify.** Undersize the script and confirm the scenario fails
      rather than silently ending early. Revert.
- [ ] **Step 6: Scoped checks.** `cargo nextest run -p eval-harness`,
      `cargo run -p eval-harness -- run --tier deterministic`, scoped clippy for
      `eval-harness`, fast checks. Show and ask.

## Task 7: Docs and close-out

- [ ] **Step 1: Docs** (§5.9): `docs/USER_GUIDE.md` (the budget section: what
      the pause is and what the two buttons do) and `docs/TROUBLESHOOTING.md`
      (a task waiting on `round_budget`, and what *Ask again* sends).
- [ ] **Step 2: Close the spec** per AGENTS.md "When a spec becomes Done":
      status in the proposal, README row and this file (tick the table); the
      `Depends on:` search (`grep -rn "61_continue_the_stopped_turn" docs`);
      re-derive What to build next; remove the item from `CHANGELOG.md`
      Unreleased.
- [ ] **Step 3: Checks.** Fast checks and scoped clippy/tests locally; the full
      gate on CI. Record the CI result here when it is green, the way spec 58
      did.
