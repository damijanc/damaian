# Feature Spec: Exploration Memory Within a Turn

Status: Not started. Specified on 2026-10-18 from a review of
[#59](59_exploration_round_allowance/proposal.md)–[#64](64_read_only_command_boundary.md),
asking whether they are enough for Damaian to explore a large repository
before changing it. They give a turn the rounds to explore, but nothing keeps
what the agent found in early rounds in front of it once the turn's request
window drops those rounds.
Order: 65 of 65
Plan: none. Evidence-driven, like [#47](47_agent_working_capability/proposal.md)
— an exception to the rule in [`README.md`](README.md).
Depends on: [#47](47_agent_working_capability/proposal.md) (the read tools,
`action_effect`, and `bounded_messages` with its restrict-only window) — built;
[#20](20_working_modes/proposal.md) (the mode matrix a new tool must enter) —
built; [#34](34_repository_config_trust_boundary.md) (scope classes for the new
key) — built. Requirement 7 also depends on
[#59](59_exploration_round_allowance/proposal.md) (the exploration allowance
and its repeat rule) — not started; everything else in this spec can be built
without it. Everything else named below is a cross-reference, not a
prerequisite.
Related implementation specs:
[`55_conversation_compaction.md`](55_conversation_compaction.md) (keeps the
objective **across** turns; this spec is the in-turn counterpart),
[`61_continue_the_stopped_turn/`](61_continue_the_stopped_turn/proposal.md) (a paused
turn keeps its state, which now includes its notes),
[`49_prompt_cache_accounting_and_reuse`](49_prompt_cache_accounting_and_reuse/proposal.md)
(prefix stability; §5.4 here changes where the in-turn cut lands),
[`18_local_evaluation_harness`](18_local_evaluation_harness/proposal.md)
(where the long-turn scenario lands).

## 1. Motivation

Exploring a real repository before changing it is one of the most important
things Damaian does. Exploration builds up: the file read in round 2 explains
the call site found in round 12, and the answer depends on both.

Inside a turn, every model request is cut down to `agent_max_turn_messages`
(default 24) messages by `bounded_messages`. The cut keeps the system and user
seed, drops the oldest **whole rounds**, and tells the model: "Earlier rounds of
this turn were elided to bound the request … The session log still holds them;
repeat a read if you need it." After the seed and the notice, 21 messages are
left. A round is one assistant message plus one result per call, so at two or
three calls per round the model sees roughly the **last seven rounds**.

Today the eight-round budget mostly hides this, because few turns get far
enough for the cut to matter. #59 changes that. With its defaults a turn can
run 24 read-only rounds plus 8 action rounds. At the end of such a turn about
three quarters of what the agent read, and everything it said to itself about
it, is not in the request that writes the answer. The model then has three
options, all bad:

- **Answer from the last seven rounds.** The forced final call (#59 §4, #61
  *Answer now*) sees only those, so a report on a large repository describes
  whatever the agent happened to look at last.
- **Re-read.** That is what the notice tells it to do. Under #59 §5.3 an exact
  repeat read is charged to the action budget. That is right for a model stuck
  in a loop, but here the engine caused the repeat and the action budget pays
  for it.
- **Guess.** It knows it read something relevant but not what it said.

The window exists for good reasons: it bounds per-call size and cost
(#47 requirement 6, `OBSERVATIONS.md` entry 6). Raising it only moves the point
where the agent forgets, and makes every call after that point more expensive.
What is missing is a small, bounded record of **what was learned** that
survives the cut.

## 2. Current State

- `bounded_messages` ([chat.rs:4318](../../crates/workspace-engine/src/chat.rs:4318))
  keeps `messages[..2]` (system, user), adds one system notice, and keeps the
  newest rounds that fit `max_messages - 3`. It cuts at round starts so a tool
  result never loses its call. If the newest round alone is larger than the
  budget, it is kept whole and the request goes over the bound. It is applied
  to every request in the turn loop (`chat.rs:2083`).
- The notice says how many messages and results were left out, not **which**
  calls they were. The model cannot tell which reads it already made.
- The cut moves forward one round at a time. Once the window is full, each new
  round drops the oldest one, so everything after the seed is at a new position
  in every request.
- `agent_max_turn_messages` is restrict-only at repository scope and a
  **Capability** in the key partition (`config.rs:1578`, `config.rs:2777`),
  labelled "Messages sent per model request" (`effective_policy.rs:160`).
- `PendingChatTurn` (`chat.rs:3995`) persists the full `messages` array, so a
  paused turn resumes with every round. The cut only affects what is sent.
- The agent has no way to write anything down that is not a tool call or
  assistant text, and both of those are dropped with their round.
- `action_effect` (`chat.rs:4465`) classifies every tool as `ReadOnly`,
  `WritesRepository` or `ShapesTurn`. `ShapesTurn` tools ("writes only the
  session log") cannot be batched with reads (`action_is_batchable_read_only`).
  `mode_permits` (`mode.rs:231`) allows the six read tools in every mode.
- #55 (not started) summarises a conversation across turns and explicitly
  leaves the in-turn array alone (#61 §4 says the same).
- The eval harness's deterministic tier scripts every model response, so it
  can check which calls a run made and how many rounds it used, but not what a
  request contained (`crates/eval-harness/src/scenario.rs`).

## 3. Requirements

1. The agent has a **notes** tool for the current turn. With it, the model can
   add to or replace a short plain-text record of what it has learned.
2. The turn's notes are sent in **every** later request of that turn, in a fixed
   place right after the seed. `bounded_messages` never drops them, and they do
   not count toward `agent_max_turn_messages`.
3. Notes are bounded by `agent_max_turn_notes_chars` (default 8,000). An append
   that would go over the bound is refused, and the tool result says how full
   the notes are and that they can be rewritten shorter with replace. The value
   is clamped to `ABSOLUTE_TURN_NOTES_CHARS` (32,000). `0` turns the tool off:
   it is not offered and nothing is pinned. The key is **restrict-only** at
   repository scope and a **Capability**.
4. When rounds are elided, the notice lists **which calls** were left out, one
   short line each (tool and canonical arguments, as #59 §5.3 defines them),
   and stays within a fixed size. This tells the model what it has already
   looked at, though not what it found.
5. The cut moves in **steps**, not one round at a time. When a request would go
   over the window, rounds are dropped until it is at most half full, and the
   cut does not move again until the window fills up again. The
   newest-round-kept-whole rule and the round-start rule stay as they are.
6. Notes are part of the turn's durable state. They are written to the session
   log when they change, persisted in `PendingChatTurn`, and restored when a
   turn resumes after an approval, a #61 pause or a crash.
7. *(Needs #59.)* For the round budget, a notes call counts the same as a read:
   a round of reads and notes is an exploration round. An append identical to
   an earlier append in the same turn is a repeat. A read the model makes again
   because it was elided is **still** a repeat; requirement 4 is what tells the
   model it already made it.
8. Notes go through the secret scanner before they are stored or sent. They are
   model-written text that may quote what the agent read.
9. The user can see a turn's notes: in the conversation, collapsed under the
   turn that wrote them, and in session export.
10. Every notes change is audited with its size and the running total, not its
    text (the text is in the session log). The audit records of the forced-final
    and exhausted paths include the notes size. The eval harness reports how
    many notes calls a run made and how many reads repeated an elided call.

## 4. Non-goals

- **Summarising elided rounds with a model call.** It would cost a billed call
  per cut that the user did not ask for at that moment (the same objection #55
  §5 deals with using an off switch). The quality of the summary would be
  outside both the user's and the agent's control. The agent is the one that
  knows what mattered in a read, so the agent writes it down.
- **Raising `agent_max_turn_messages` or changing it to a token measure.** That
  moves the point of forgetting and makes each later call bigger. A token-based
  bound may be worth doing for size reasons, but it does not solve this.
- **Keeping notes across turns.** When the turn ends, its notes are in the
  session log and nothing more happens. Carrying findings into later turns is
  #55's job: its summary should read a finished turn's notes, but that is #55's
  decision to make.
- **Persistent project memory.** That is #28–#30. Turn notes are working memory
  for one task, not facts about the project.
- **Delegated exploration** (a sub-agent that explores and returns only a
  conclusion). That would also keep the main request small, but #38 is deferred
  and its premise is unmeasured. This spec should be measured first (§6).
- **Telling the model how to explore.** Beyond describing the tool and the cut,
  the system prompt does not prescribe a method.

## 5. Design

### 5.1 The tool

`turn_notes`, a native tool with two arguments:

- `text` (string, required): what to record;
- `replace` (boolean, default `false`): when true, `text` becomes the whole
  record; when false, it is appended as a new paragraph.

The result is a short line, for example `notes: 2,140 / 8,000 characters`, or
the refusal from requirement 3. The notes are not echoed back in the result,
because they already appear in the pinned block on the next request.

The tool description is a few sentences: notes stay visible for the rest of
the turn when earlier rounds are dropped, they are the place to record
findings worth keeping (paths, what a component does, open questions), and
they should be short. That text is the only change to model-facing prompts,
together with the notice in §5.3.

`ToolAction::TurnNotes { text, replace }` gets its own `ActionEffect` variant,
`Notes`. It is not `ReadOnly`, because it changes turn state. It is not
`ShapesTurn` either, because those cannot be batched with reads, and recording
a finding in the same round as the read that found it is exactly what should
happen. `action_is_batchable_read_only` becomes `ReadOnly | Notes`, and the doc
comment on `action_effect` says the variant also decides the budget class
(requirement 7). `tool_action_marker` marks it not side-effecting: it writes
only the session log, as the `ShapesTurn` tools do.

`mode_permits` allows it in every mode. It is offered wherever the read tools
are offered and `agent_max_turn_notes_chars > 0`. A worker session (#57) gets
it as any session does.

### 5.2 Where the notes go

The turn loop keeps `turn_notes: String` next to `messages`. Notes are **not**
kept as a message in `messages`. If they were, a replace would have to rewrite
history, and the persisted array would no longer match what was dispatched.

`bounded_messages` gains the notes as a parameter. When they are not empty,
the request is:

```
messages[0]  system   (unchanged)
messages[1]  user     (unchanged)
             system   "Notes you recorded earlier in this turn:" + notes
             system   elision notice (§5.3), only when rounds were cut
             …        newest rounds
```

The notes come before the elision notice so that the notice stays next to the
cut. Neither block counts toward `max_messages`, so a repository that lowers
the window cannot push the notes out. The size of the notes is bounded by
requirement 3 instead.

Before the first notes call, the request is exactly what it is today.
`agent_max_turn_notes_chars = 0` keeps it that way for the whole turn.

### 5.3 Saying which calls were cut

The elision notice becomes:

```
Earlier rounds of this turn were elided to bound the request: 31 messages
(26 call results) omitted. Calls already made, oldest first:
  read_file docs/specs/README.md 1–400
  list_directory docs/specs
  search_content "Status:" docs/specs/**/*.md
  …and 9 more
The session log still holds every result. Your turn notes are above.
```

Each line uses #59 §5.3's canonical form, with paths repository-relative and
resolved through `PathPolicy`. The list is capped at 40 lines. Past the cap it
says "…and N more". The canonical form is computed once per dispatched call
and kept in the loop with the seen-call set #59 adds, so building the notice
does not re-decode calls. If this spec is built first, it adds that per-turn
list itself, and #59 reuses it.

### 5.4 Cutting in steps

Today a full window drops one round per new round. Requirement 5 changes this
to hysteresis. When `tail.len()` goes over `budget`, `drop_count` is chosen so
that the kept tail is at most `budget / 2` (still moved forward to a round
start). The cut position is kept in the loop and only moves forward when the
tail goes over `budget` again.

There are two effects. The model gets about `budget / 2` rounds of extra
history before the next cut, instead of losing one round on every call. And the
request prefix (seed, notes, notice, then the oldest kept rounds) stays the
same between cuts. Notes still change when the model writes to them, and an
append changes only the end of the notes block.

Whether that turns into provider cache hits depends on the provider and on
#49's reuse slice, which is not built. This spec does not claim a cache saving
and does not depend on one. It removes a cause of cache misses that exists by
construction. The cut position and whether a cut happened are recorded on
`model_request_prepared` (`elidedThrough`, `cutMoved`), so #49's measurements
can see the effect.

`bounded_messages` stays a pure function. The cut position comes in as an
argument and goes out with the result.

### 5.5 Persistence

- **Session log:** a `turn_notes` event with the task id, `replace`, and the
  redacted notes text after the change. The latest event for a task is that
  task's notes. Older readers skip unknown events.
- **`PendingChatTurn`:** a `#[serde(default)] turn_notes: String` and a
  `#[serde(default)] elided_through: usize`. A turn written before this spec
  resumes with no notes and no cut position, which is today's behaviour.
- **Crash recovery** (#17): a task resumed from the log rebuilds its notes from
  its latest `turn_notes` event.
- **#61's pause:** a paused turn is a `PendingChatTurn`, so its notes come with
  it. Continuing does not reset them.

### 5.6 The setting

`agent_max_turn_notes_chars: usize` in `Config`. Default 8,000 (roughly 2,000
tokens, under 2% of the 141k-token request in #59 `context.md` §1). Ceiling:
`ABSOLUTE_TURN_NOTES_CHARS = 32_000`, a constant with the same kind of doc
comment as `ABSOLUTE_TOOL_ROUND_CAP`. Restrict-only, lower wins, in the same
loop as `agent_max_turn_messages` (`config.rs:1570`), `=> Capability` in the key
partition. It touches all eight places a key touches (spec 21 `context.md`
§3.8), including permission profiles (`profile.rs`) and the effective policy
label ("Turn notes size").

Restrict-only for the same reason as #59 §5.4: a larger value is sent on every
later call, so raising it spends the user's money.

### 5.7 What the user sees

- A turn that wrote notes shows a collapsed **Notes** disclosure below the
  turn's answer, using the disclosure pattern from #41 and #42. It shows the
  final text after redaction. A turn that wrote no notes shows nothing.
- Session export includes each turn's final notes.
- Progress adds `· notes 2.1k` to the round label while a turn runs and has
  notes, next to #59's `· N reads`.

### 5.8 Eval and tests

**Engine tests** (in `chat.rs` tests, using a scripted adapter that records
every request it receives):

- notes written in round 2 of a 20-round scripted turn appear, unchanged, in
  the request of round 20 and in the forced final request;
- with `agent_max_turn_notes_chars = 0` every request is byte-identical to one
  built before this spec;
- an append over the bound is refused, the notes are unchanged, and a replace
  that fits is accepted;
- the elision notice lists every elided call, oldest first, capped with
  "…and N more";
- the cut moves only when the window fills, and every request after it starts
  with the same prefix until it moves again;
- notes survive an approval pause, a #61 pause, and a crash resume;
- a secret placed in a note is redacted in the log, the request and the
  export.

**Deterministic scenario `long_exploration_memory`:** a 20-round scripted turn
over the `rust-workspace` fixture. Round 2 batches a read with a notes call,
and the turn answers. It asserts the turn completes, the run records one notes
call, and no read repeats an elided call. The script decides the calls, so like
`batched_reads` these assertions are `deterministic_only`: they pin the
dispatch and accounting paths, not model behaviour.

**Live acceptance (§6):** the real check, because only a real model decides
whether to take notes and whether they are good enough to answer from.

### 5.9 Documentation

On Done: `docs/USER_GUIDE.md` (what turn notes are, where they appear, the
key), `docs/TROUBLESHOOTING.md` (why an answer may say it relied on notes,
what the elision notice means, how to read `turn_notes` events in a session
log). `SECURITY.md` only if it by then lists per-key budget classes.

## 6. Acceptance Criteria

- A notes call in round 2 of a long turn is present in every later request of
  that turn, including the forced final call, and none of those requests drops
  it at any `agent_max_turn_messages` value down to the floor.
- With the key at `0`, the tool is not offered and every request is identical to
  one built before this spec, across the existing deterministic scenarios.
- A value above 32,000 is clamped. A repository config that raises the key is
  ignored and audited as a restrict-only weakening attempt; one that lowers it
  applies.
- An over-bound append is refused with the current size, and the turn goes on.
- The elision notice names the elided calls in canonical form within its cap.
- Between two cuts, consecutive requests share their whole prefix up to the
  newest round, and the request audit records where the cut is.
- Notes are restored after an approval pause, a #61 pause and a crash.
- A secret in a note never reaches the request, the log or the export
  unredacted.
- The user sees a turn's notes collapsed under that turn, and in the export.
- With #59 built: a round of reads and notes is an exploration round, and an
  identical append is a repeat.
- **Live:** against `deepseek-v4-flash` on this repository, with #59 built, run
  two prompts: #59's motivating prompt, and "Where is the command approval
  decision made, what feeds it, and which specs changed it?". Record, for each:
  rounds used, notes calls made, reads that repeated an elided call, and
  whether the answer cites files that were read before the last cut. Run each
  once with the key at its default and once at `0`. This spec is accepted if
  the default run repeats fewer elided reads and cites more pre-cut files. If
  the model does not take notes unprompted, record that. The next step is then
  the tool description and system prompt, not a bigger window.

## 7. Implementation Notes

None yet.
