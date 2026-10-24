# Exploration Round Allowance — Background

Background for [`proposal.md`](proposal.md): the session that motivated it, what
the round loop does today, what the change costs, and the designs rejected on
the way.

## 1. The session that ran out

On 2026-10-17 a Damaian session against this repository, on
`deepseek-v4-flash` with default configuration, was asked:

> check what specs are left to implement, also check worktrees to see specs
> that are in progress, this is a reporting only task, no code changes

It stopped after about 50 seconds with *Tool budget exhausted*. Its rounds, from
the session log:

| Round | Calls | Read-only? |
|---|---|---|
| 1 | `read_file docs/specs/README.md` (truncated at line 400 of 468), `list_directory docs/specs` | yes |
| 2 | `read_file docs/specs/README.md` lines 400–468 | yes |
| 3 | `search_content` for `Status:` lines | yes |
| 4 | `run_command head -n 12` over four spec files (approved) | no |
| 5 | `run_command git worktree list --porcelain` (approved) | no |
| 6 | `run_command` loop over `.claude/worktrees/*` for status and ahead/behind (approved) | no |
| 7 | `read_file` two `tasks.md` files (both truncated at 400 lines) | yes |
| 8 | `read_file` two more `tasks.md` files, `run_command git diff` in two worktrees (approved) | no |
| 9 | forced final call; the model still asked for a tool | — |

Five of eight rounds were read-only. Under this spec rounds 1, 2, 3 and 7 are
exploration and round 8 is an action round because of its command, so the turn
would have reached its ninth call having used four action rounds.

Input tokens grew from 67k on the first call to 141k on the last, and the
last call's prompt cache hit fell to 256 tokens. That growth is the cost this
spec has to bound (§3).

The prompt could have been narrower. That is not the point: a user asking an
agent to look around before answering is ordinary, and eight rounds is not
enough to look around a repository of this size.

## 2. Why the budget became tight

[#47](../47_agent_working_capability/proposal.md) gave the agent `read_file`
ranges, `list_directory` and `search_content` as tools, so navigating no longer
needs an approved shell command. Its live A/B (§7.2) measured the trade:
approval stops fell from 17 to 4, and tool calls rose 90%, with `read_file`
going from 26 to 70 calls. The model now reads in more, smaller steps — which is
what the tools were for — and every one of those steps costs a round that was
sized when reads were rarer and more expensive.

#47 §7.2 also said its premise ("the tool surface is why sessions run out of
rounds") needed a long-task measurement it did not have. The `long_discovery`
scenario in [`proposal.md`](proposal.md) §5.7 is that measurement's
deterministic half.

## 3. What this costs

The round cap is the only runaway bound when `agent_max_task_tokens` is unset,
which is the default: spec 21 §5.4 picked no default ceiling on purpose and
named `agent_max_tool_rounds` as "the safety net". Today a turn makes at most
`8 + 1` model calls with defaults, or `16 + 1` at the cap.

With this spec it makes at most `24 + 8 + 1` with defaults and `32 + 16 + 1` at
both caps. Each call's request is bounded by `agent_max_turn_messages` (24) via
`bounded_messages`, and each read's result by `max_read_lines` and its siblings,
so per-call size does not grow without limit — but the number of calls can
grow by roughly 3.7× with defaults. That is a real increase in worst-case spend
for a user who has set no token ceiling, and it is why:

- the allowance is a separate, clamped number rather than an exemption;
- repeats are charged to the action budget, so the common runaway — a model
  re-reading the same file — stops where it stops today;
- the key is restrict-only, so no cloned repository can raise it;
- `0` restores today's behaviour exactly, for a user who prefers the old bound.

## 4. Alternatives rejected

- **Exempt read-only rounds entirely.** The first idea. Rejected because with no
  token ceiling a model looping on reads would have no bound at all, and each
  round resends a context that has grown all turn.
- **Raise `agent_max_tool_rounds`.** It also raises how many commands, edits and
  approvals a turn may make, which is the part that should stay tight. It is
  also Free at repository scope, so a raised default is one a clone can raise
  further up to the cap.
- **Charge reads a fraction of a round.** Equivalent in effect to a separate
  allowance, but the user would have to reason about fractional rounds to know
  when a turn will stop. Two whole counters are easier to show and explain.
- **Rely on the token ceiling.** The ceiling is the right bound for spend, and
  with one set spec 47's continuation already lets a turn run past the round
  cap. But it has no default, and inventing one is the decision spec 21 §5.4
  declined to make. This spec has to be safe without it.
- **Count auto-allowed read-only commands as exploration.** Rejected in
  [`proposal.md`](proposal.md) §4: it would tie the budget to the command
  allow-list and policy, which #47 requirement 3 keeps apart from the tools.

## 5. Found while specifying, out of scope

Three things in the motivating session cost rounds or interruptions and are not
the budget's fault. They belong in `docs/PLAN/OBSERVATIONS.md` or their own
specs, not here:

- Reads stop at 400 lines (`max_read_lines`), and two of the eight rounds went
  on continuing a truncated read. A larger default, or returning the next range
  hint the model can batch, would save rounds without touching the budget.
- `git worktree list --porcelain` needed approval because it "cannot run in
  sandbox mode", and was marked side-effecting, although it only reads.
  [#60](../60_worktree_aware_read_tools/proposal.md) gives the agent a tool for
  it and for reading inside worktrees, which was most of the session's later
  approvals.
- The forced final call (tools dropped) still produced a tool request that
  `model_output_requests_tool` caught. Whether the model was told clearly
  enough that tools were gone is unexamined.
  [#61](../61_continue_the_stopped_turn/proposal.md) moves the pause before that call,
  so it is made only when the user asks for an answer.

## 6. Measured: rounds used by real delegated tasks (2026-10-24)

§5.4 of the proposal chose 24 without a measurement of real turns. This is the
first one. It comes from another agent, not from Damaian: eleven opencode
sessions on 2026-10-18, each a single task prompt delegated from Claude Code to
`deepseek-v4-pro` against this repository (spec planning and spec tasks), each
run to completion with no round limit. The counts were read from opencode's
local database (`~/.local/share/opencode/opencode.db`): a round is a
`step-start` part followed by at least one `tool` part, the same thing
Damaian counts as a round.

| Task | Rounds | Calls | Read tools | Read-only shell | Edit | Build/test | Other shell | Exploration† | Action† |
|---|---|---|---|---|---|---|---|---|---|
| Spec 59 planning | 16 | 39 | 8 | 8 | 0 | 0 | 0 | 16 | 0 |
| Spec 59 planning (retry) | 31 | 62 | 24 | 1 | 5 | 1 | 0 | 25 | 6 |
| Spec 62 planning | 49 | 70 | 26 | 10 | 10 | 1 | 2 | 36 | 13 |
| Spec 61 planning | 50 | 74 | 24 | 13 | 11 | 1 | 1 | 37 | 13 |
| Spec 61 Task 3 | 64 | 70 | 24 | 13 | 15 | 10 | 2 | 37 | 27 |
| Spec 64 requirement 5 | 69 | 74 | 20 | 8 | 31 | 10 | 0 | 28 | 41 |
| Spec 63 Task 4 | 70 | 77 | 29 | 8 | 21 | 12 | 0 | 37 | 33 |
| Spec 60 Task 4 (two prompts) | 89 | 95 | 26 | 9 | 36 | 13 | 5 | 35 | 54 |
| Spec 61 Task 2 | 99 | 103 | 41 | 22 | 24 | 12 | 0 | 63 | 36 |
| Spec 60 Task 5 | 103 | 119 | 37 | 15 | 28 | 17 | 6 | 52 | 51 |
| Spec 61 Task 1 | 150 | 176 | 50 | 22 | 49 | 25 | 4 | 72 | 78 |

† Exploration is read-tool rounds plus rounds whose shell commands only read
(`git status`/`log`/`diff`/`show`, `grep`, `ls`, `cat`, `sed -n`, …). Damaian
serves most of those with read tools, but §4 of the proposal does not count a
read-only *command* as exploration, so this column is an upper bound for
Damaian. Action is everything else: any round with an edit, a build or test
command, or another command.

What it shows:

- **Planning tasks fit the design as specified.** 0–13 action rounds and 16–37
  exploration rounds: at most one or two *Continue* pauses with today's
  defaults, and the allowance of 24 covers about half of them.
- **Implementation tasks do not.** Action rounds were 27–78 (median 41), so
  with `agent_max_tool_rounds = 8` a typical task pauses five times. Against
  `ABSOLUTE_TURN_ROUND_CAP` (64): with the allowance at its default of 24, four
  of the seven would hit the cap before finishing; at the ceiling of 32, three;
  and Spec 61 Task 1 would hit it even if every read were free.
  Exploration rounds were 28–72 (median 37), above the default of 24 and, for
  three of seven, above the ceiling of 32.
- **Batching does not close the gap.** Implementation tasks averaged 1.04–1.17
  calls per round, so the model, not the loop, decides that a round holds one
  call. Planning tasks batched more (1.4–2.4).
- **Build/test rounds dominate action rounds less than expected:** 10–25 per
  task, with Spec 61 Task 1's 20 `cargo nextest` runs the outlier. Edits are the
  larger share (15–49).

Limits on what this proves: opencode's prompts, tools and read windows differ
from Damaian's (its `read` returns up to 2,000 lines where Damaian's stops at
400, which would raise Damaian's count); the tasks were sized for an
unattended agent; and every command ran without an approval stop. It is
evidence of scale, not a target.
