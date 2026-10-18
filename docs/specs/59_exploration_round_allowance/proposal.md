# Feature Spec: Exploration Round Allowance

Status: Planned, not started. Specified on 2026-10-17 from a session that ran out of tool
rounds on a reporting-only request; the evidence is in
[`context.md`](context.md) §1. Planned on 2026-10-18; see [`tasks.md`](tasks.md).
Order: 59 of 59
Plan: none. Like [#47](../47_agent_working_capability/proposal.md), this spec is
evidence-driven rather than a roadmap graduation — an exception to the rule in
[`README.md`](../README.md).
Also in this spec: [`context.md`](context.md) (the session that motivated it,
the current round loop, the cost bound, and the alternatives rejected).
Depends on: [#21](../21_task_plan_progress_and_budget/proposal.md) (the round
budget and token ceiling this changes) — built;
[#34](../34_repository_config_trust_boundary.md) (scope classes for the new
key) — built; [#47](../47_agent_working_capability/proposal.md) (the read-only
tool surface and `action_effect`, and requirement 6's continuation) — built.
Everything else named below is a cross-reference, not a prerequisite.
Related implementation specs:
[`08_stop_and_progress.md`](../08_stop_and_progress.md) (the progress channel
that shows round counts),
[`18_local_evaluation_harness`](../18_local_evaluation_harness/proposal.md)
(where the long-task scenario lands),
[`57_worker_sessions`](../57_worker_sessions/proposal.md) (whose progress file
carries the round counts).

## 1. Motivation

A turn has eight tool rounds by default (`agent_max_tool_rounds`), and every
round counts the same whether the model read one file or ran a command. A
discovery request — "which specs are left, and which worktrees are working on
them" — is mostly reading, and it runs out of rounds before it can answer.

That happened on 2026-10-17 ([`context.md`](context.md) §1). Five of the eight
rounds only read, listed or searched the repository. Had they not counted, the
turn would have used four of its eight rounds and kept going.

Spec 47 made this worse on purpose. It moved navigation out of the shell and
into first-class read tools, so the model stopped asking for approval to look
around, but it now spends more rounds doing it: its live A/B measured `read_file`
calls going from 26 to 70 and approval stops falling from 17 to 4
([#47 §7.2](../47_agent_working_capability/proposal.md)). A budget that was
sized when navigation went through approved commands now charges reading at the
price of acting.

A user asking Damaian to look around a repository before answering is not
doing anything unusual. The round budget should bound what the agent *does*,
and bound what it *reads* separately and more generously.

## 2. Current State

- `agent_max_tool_rounds` (default 8) is clamped to
  `ABSOLUTE_TOOL_ROUND_CAP` (16) by `default_tool_round_limit`
  ([chat.rs:530](../../../crates/workspace-engine/src/chat.rs:530)). Web debug
  mode uses `agent_web_debug_max_tool_rounds` (12), and *Continue debugging*
  uses the cap itself (`tool_round_limit`, `chat.rs:542`).
- The turn loop counts every model call that returns tool calls as one round.
  At `round >= max_rounds` it sets `force_final`, makes one more call with
  tools dropped, and stops with `StopReason::ToolBudget` if the model still asks
  for a tool (`chat.rs:2072`, `chat.rs:2340`).
- With `agent_max_task_tokens` set, spec 47 requirement 6 continues past the
  cap in further segments, bounded by the token ceiling and by
  `round < ABSOLUTE_TOOL_ROUND_CAP` (`chat.rs:2350`).
- `action_effect` ([chat.rs:4465](../../../crates/workspace-engine/src/chat.rs:4465))
  already classifies every tool. `ReadFile`, `ListDirectory`, `SearchContent`,
  `SearchCodebase`, `ReadGitStatus` and `ReadGitDiff` — all now carrying spec
  60's `worktree` argument, and to be joined by that spec's `list_worktrees`
  and `read_git_log` when its Task 4 lands — are
  `ActionEffect::ReadOnly`; commands, browser diagnostics and MCP calls are
  `WritesRepository`; patches, edits, plans, step completion and `ask_user`
  are `ShapesTurn`. Requirement 8's within-round batching is derived from the
  same function, and so is this spec's classification (§5.1), which is why the
  allowance counts spec 60's read tools automatically.
- `agent_max_tool_rounds` is a **Preference** and Free at repository scope
  (`config.rs`, the key partition; spec 34 §5.1). Spec 21
  [`context.md`](../21_task_plan_progress_and_budget/context.md) §3.8 calls
  that "arguably the same hole one step removed" and leaves it alone.
- The stop reads "This turn reached the configured tool round limit (8) …
  Continue debugging with a narrower next request or approve a larger
  diagnostic budget if available" (`tool_budget_exhausted_response`,
  `chat.rs:5443`), and the button under it says *Continue debugging*
  (`markMessageToolBudgetExhausted`, `app.js:6967`), whatever the turn was
  doing.
- Progress shows `round N/M` (`app.js:4172`) and the Workers board draws a bar
  from `round` and `maxRounds` (`app.js:6047`), both from `TurnPhase`
  (`chat.rs:168`).

## 3. Requirements

1. A tool round in which **every** call is `ActionEffect::ReadOnly` is an
   *exploration round*. It is charged to a separate per-turn allowance,
   `agent_max_exploration_rounds`, and not to `agent_max_tool_rounds`, while
   that allowance lasts.
2. Any other round — one containing a command, an edit, a patch, a plan, a step
   completion, a question, a browser diagnostic or an MCP call, alone or
   beside reads — is an *action round* and is charged to
   `agent_max_tool_rounds` exactly as today.
3. An exploration round that finds the allowance spent is charged as an action
   round. Exhausting the allowance never stops a turn by itself; only the action
   budget, the token ceiling or the hard caps do.
4. A read-only call that exactly repeats an earlier call in the same turn — same
   tool and same canonical arguments — makes its round an action round. A model
   re-reading the same thing in a loop draws down the budget that stops turns.
5. The total is hard-bounded. `agent_max_exploration_rounds` is clamped to a
   constant ceiling, as `agent_max_tool_rounds` is to
   `ABSOLUTE_TOOL_ROUND_CAP`, so no configuration makes a turn unbounded when no
   token ceiling is set.
6. `agent_max_exploration_rounds = 0` reproduces today's behaviour exactly:
   every round is charged to `agent_max_tool_rounds`.
7. The new key is **restrict-only** at repository scope (lower value wins) and a
   **Capability** in the key partition. A cloned repository may lower it or set
   it to 0, never raise it.
8. The user can see both counts while a turn runs, and a stopped turn says
   which budget stopped it and what each had used. The stop message and its
   button do not assume the turn was debugging.
9. The exploration allowance and its use are recorded: the turn's audit
   records name both counts, and the eval harness reports exploration rounds
   beside tool rounds.

## 4. Non-goals

- Exempting reads from every bound. The round cap is the only runaway bound when
  no token ceiling is set (spec 21 §5.4), and each round resends a context that
  has grown all turn. Requirement 5 exists because of this.
- Counting commands as exploration, even low-risk read-only ones such as
  `git status` that `is_low_risk_read_only` would auto-allow. A command's cost
  and output are not bounded the way a read tool's are, and #47 requirement 3
  keeps the shell and the read tools on separate trust terms. A read the agent
  needs often should become a tool, as `ReadGitStatus` did.
- Changing `agent_max_tool_rounds`, its default, its Free scope class, or
  `ABSOLUTE_TOOL_ROUND_CAP`. Spec 21 §3.8 leaves the scope class question open
  and this spec does not settle it.
- Reducing how many rounds a task needs: larger read windows, fewer truncation
  re-reads, or batching more reads per round. Those are worth doing and are
  recorded in [`context.md`](context.md) §5, but they change the tools, not the
  budget.
- Giving the forced final call a different shape. It still drops tools and
  answers from what was gathered.

## 5. Design

### 5.1 Classifying a round

After a model call returns its tool calls and before any of them dispatches,
the loop classifies the round:

- **Exploration** when every decoded call satisfies
  `matches!(action_effect(action), ActionEffect::ReadOnly)`, none is a repeat
  (§5.3), and the exploration allowance has room.
- **Action** otherwise.

Classification uses `action_effect` and nothing else, so it cannot drift from
the batching rule or from what a new tool declares. Adding a `ToolAction`
variant already forces a decision in that `match`; that decision is now also
the budget decision, and the doc comment on `action_effect` says so.

A call that fails to decode does not make its round exploration. An undecodable
call is not known to be a read.

A read the path policy refuses is still a read-only call and does not change
the round's class. It costs nothing to dispatch, and the allowance bounds a
model that keeps asking.

### 5.2 Two counters, one loop

The loop's single `round` counter becomes two: `action_rounds` and
`exploration_rounds`. A third, `model_calls`, keeps the role `round` has for
anything that is genuinely per call — spec 57's pause-and-steer boundary and
the round markers in the audit log. (`TurnPhase`'s `round` becomes the action
count; see §5.5.)

- `force_final` becomes `action_rounds >= max_action_rounds`, where
  `max_action_rounds` is today's `max_rounds` (including spec 47's
  `round_budget_extension`).
- Spec 47's continuation condition `round < ABSOLUTE_TOOL_ROUND_CAP` becomes
  `action_rounds < ABSOLUTE_TOOL_ROUND_CAP`. Continuation extends the action
  budget only; the exploration allowance is per turn and never extended.
- *Continue debugging* (`options.continue_debugging`) keeps raising the action
  limit to the cap. It does not change the exploration allowance, which is
  already fresh for the new turn.
- Web debug mode keeps its action limit, `agent_web_debug_max_tool_rounds`, and
  has the same exploration allowance as any other turn.

### 5.3 What counts as a repeat

A repeat is a read-only call whose tool and canonical arguments equal those of
a call already dispatched in this turn. Canonical means: the path resolved
through `PathPolicy` to a repository-relative path; the line range as given (a
read of lines 400–468 after 1–400 is not a repeat — that is how a truncated
read continues); and for search, the pattern, glob and match cap as given. Two
calls in the same round that are identical make it an action round as well.

This is deliberately narrow. It catches a model stuck re-reading the same
thing, and nothing that reads something new. A legitimate re-read — for
example after `bounded_messages` elided the first result — is charged to the
action budget, not refused, so it costs a round and never stops the work by
itself.

The set of seen calls lives for one turn, in the loop, and is not persisted. A
resumed turn (`PendingChatTurn`) starts with an empty set; the worst case is
one uncaught repeat after a resume.

### 5.4 The setting

`agent_max_exploration_rounds: u32` in `Config`.

- **Default: 24.** Three times the action budget, so a discovery turn can read
  roughly what the motivating session needed several times over. The number is
  a starting point and §6's long-task scenario is what checks it. The
  measurement that would settle it — rounds used by real discovery turns — does
  not exist yet ([`context.md`](context.md) §3).
- **Ceiling: `ABSOLUTE_EXPLORATION_ROUND_CAP = 32`**, a constant beside
  `ABSOLUTE_TOOL_ROUND_CAP`, with the same doc comment: raising it past the
  constant requires explicit work, not configuration.
- **0 is accepted** and means "no exploration allowance" (requirement 6).
- **Parsing differs from `agent_max_tool_rounds`.** `parse_round_count`
  (`config.rs`) refuses 0 and anything over 16, but this key must accept 0
  (requirement 6) and clamp a value above 32 to the ceiling (requirement 5).
  It gets its own parser: accept `0..=32`, clamping anything larger to
  `ABSOLUTE_EXPLORATION_ROUND_CAP`, and let the round loop's
  `default_exploration_round_limit` clamp again at runtime.
- **Scope: restrict-only, lower wins**, in a `u32` variant of the
  `restrict_only_limit` helper used for `agent_max_turn_messages` and
  `max_read_lines` (`config.rs:1576` — that loop is `usize`-typed, so the
  `u32` key needs its own apply block), and `=> Capability` in the key
  partition. Raising it spends the user's money, which is exactly what spec 21
  §3.8 made the token ceiling restrict-only for.
- The eight places a key touches (spec 21 `context.md` §3.8) all get it:
  the field, the overlay field, the destructuring, the apply block, the key
  parser, `config_show`, the overlay serializer and `Default`. Permission
  profiles (`profile.rs`) and the effective policy label
  (`effective_policy.rs`, "Read-only rounds per turn") classify it as the
  other `agent_*` Capability keys are.

### 5.5 What the user sees

- **While running.** `TurnPhase` gains `exploration_round` and
  `max_exploration_rounds`. `round` and `max_rounds` keep their names and now
  mean action rounds, so the existing `round N/M` label and the Workers board
  bar stay correct without change. The progress label adds the reads when there
  are any: `round 3/8 · 12 reads`. Spec 57's progress file carries the two new
  fields additively; older readers ignore them.
- **When stopped.** `tool_budget_exhausted_response` names both counts and does
  not mention debugging, for example: "This turn used its 8 action rounds (and
  24 read-only rounds) and the model still wanted another tool. Nothing was
  lost: continue to give it more action rounds, or narrow the request." The
  wording is settled when the task is planned; the constraints are that it
  names both numbers and assumes nothing about the kind of task.
- **The button** under a stopped turn becomes *Continue* (`app.js:6978`). It
  does what *Continue debugging* does today. The internal option name
  `continue_debugging` may stay; it is not user-visible.

### 5.6 Recording

- The round loop's existing audit records gain `explorationRounds` and
  `explorationLimit`: `model_request_prepared` (whose round fields are
  `toolRound`/`toolRoundLimit`, `chat.rs:2108`) and `turn_continued` (whose are
  `round`/`roundLimit`, `chat.rs:2358`). There is no separate forced-final or
  exhausted audit event — exhaustion is visible through
  `model_request_prepared` at `toolRound == toolRoundLimit` plus the stop
  reason.
- No new session-log event. Which rounds were exploration is derivable from the
  log already: each round opens with a `model_call` marker and its tool calls
  are recorded as action markers by name. `trace::tool_rounds` in the eval
  harness gains a sibling, `exploration_rounds`, derived the same way, and the
  run record carries it.

### 5.7 Eval scenarios

Two deterministic scenarios, so the behaviour is pinned in CI:

- **`long_discovery`**: a scripted turn of fourteen read-only rounds (reads,
  listings, searches over a fixture repository, no repeats) followed by an
  answer. With the defaults it completes; with `agent_max_exploration_rounds =
  0` it stops with `ToolBudgetExhausted`. This is also the long-task scenario
  #47 §7.2 said was missing.
- **`read_loop`**: a scripted turn that reads the same file with the same range
  every round. It stops with `ToolBudgetExhausted` after one exploration round
  and the action budget — nine rounds with the defaults, not thirty-three.

A live-tier run of the motivating prompt against `deepseek-v4-flash` is the
acceptance check that the defaults are sensible (§6).

### 5.8 Documentation

On Done: `docs/USER_GUIDE.md` (the budget section that explains *Tool budget
exhausted*, and the config reference), `docs/TROUBLESHOOTING.md` (the
`tool_budget_exhausted` entry, which today says "default 8, plus one closing
model call"). `SECURITY.md` lists no individual budget keys today, so it
needs no change unless that has changed by then.

## 6. Acceptance Criteria

- A turn whose rounds are all read-only and non-repeating completes past eight
  rounds with the defaults (`long_discovery`).
- A turn that mixes reads and commands is stopped by the action budget at the
  same point it is today, counting only its action rounds.
- A round containing one command and three reads is charged as one action round
  and no exploration round.
- A repeated identical read is charged as an action round (`read_loop` stops
  within `1 + agent_max_tool_rounds` rounds plus the forced final call).
- Once the allowance is spent, further read-only rounds are charged as action
  rounds, and the turn is not stopped at that moment.
- `agent_max_exploration_rounds = 0` yields the same rounds, stop reason and
  audit sequence as before this spec on the existing deterministic scenarios.
- A value above 32 is clamped to 32.
- A repository config that sets a higher value than the user's is ignored and
  audited as a restrict-only weakening attempt; a lower one applies.
- With a token ceiling set, spec 47's continuation still extends only the
  action budget, and its tests pass unchanged apart from counting action
  rounds.
- The progress label shows reads when there are any; a stopped turn names both
  counts and offers *Continue*; nothing on screen says "debugging" for a turn
  that was not one.
- The eval harness reports `explorationRounds` for every run.
- The motivating prompt, re-run live against `deepseek-v4-flash` on this
  repository, completes with an answer rather than *Tool budget exhausted*. If
  it does not, record how many exploration rounds it used before stopping; that
  is the evidence for revisiting the default.

## 7. Implementation Notes

None yet.
