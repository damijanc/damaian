# Feature Spec: Exploration Memory

Status: Not started. Specified on 2026-10-18 from a review of
[#59](59_exploration_round_allowance/proposal.md)–[#64](64_read_only_command_boundary.md),
asking whether they are enough for Damaian to investigate a large repository
before changing it. They give a turn the rounds to explore, but nothing keeps
what the agent found in early rounds in front of it once the turn's request
window drops those rounds, and nothing carries it into the next turn. Revised
2026-10-19: the stepped cut moved to #49 (§4), and the pre-cut notice
(requirement 5) and carrying notes into the next turn (requirement 6) were
added, because investigating is the prerequisite for everything else Damaian
does (§1). The file keeps its original name so existing links still work.
Order: 65 of 66
Plan: none. Evidence-driven, like [#47](47_agent_working_capability/proposal.md)
— an exception to the rule in [`README.md`](README.md).
Depends on: [#47](47_agent_working_capability/proposal.md) (the read tools,
`action_effect`, and `bounded_messages` with its restrict-only window) — built;
[#20](20_working_modes/proposal.md) (the mode matrix a new tool must enter) —
built; [#34](34_repository_config_trust_boundary.md) (scope classes for the new
key) — built. Requirement 8 also depends on
[#59](59_exploration_round_allowance/proposal.md) (the exploration allowance
and its repeat rule) — built 2026-10-23; everything else in this spec can be
built without it. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`55_conversation_compaction.md`](55_conversation_compaction.md) (summarises a
whole conversation across turns; this spec carries only the agent's own notes,
and #55 should read them),
[`61_continue_the_stopped_turn/`](61_continue_the_stopped_turn/proposal.md) (a paused
turn keeps its state, which now includes its notes; its continuations are what
make turns long enough for the cut to matter, §1),
[`49_prompt_cache_accounting_and_reuse`](49_prompt_cache_accounting_and_reuse/proposal.md)
(prefix stability; the stepped cut this spec first proposed now lives in its
§5.9),
[`18_local_evaluation_harness`](18_local_evaluation_harness/proposal.md)
(where the long-turn scenario lands).

## 1. Motivation

Damaian cannot investigate a large project well enough to understand it. That
is the prerequisite for real work: an agent that cannot learn a codebase cannot
fix it, design in it or implement in it, however good its edit tools are. Every
other capability builds on what the agent found out first.

The evidence is the session behind #59 (its `context.md` §1): a reporting-only
request against this repository ran out of rounds, and its continuations lost
the request. #59, #60 and #61 remove the round limit, the approvals and the
lost continuation. What is left is **memory**, and it fails in two places.

**Inside a turn.** Exploration builds up: the file read in round 2 explains the
call site found in round 12, and the answer depends on both. But every model
request is cut down to `agent_max_turn_messages` (default 24) messages by
`bounded_messages`. The cut keeps the system and user seed, drops the oldest
**whole rounds**, and tells the model: "Earlier rounds of this turn were elided
to bound the request … The session log still holds them; repeat a read if you
need it." After the seed and the notice, 21 messages are left. A round is one
assistant message plus one result per call, so at two or three calls per round
the model sees roughly the **last seven rounds**.

An eight-round turn mostly hid this, because few turns got far enough for the
cut to matter. Two specs change that. #61 (Tasks 1–5 on main since
2026-10-19) lets the user continue a turn that ran out of rounds: each
*Continue* grants another `agent_max_tool_rounds` segment of the **same** turn,
up to `ABSOLUTE_TURN_ROUND_CAP` (64) rounds, and replays the saved `messages`
through `bounded_messages` — so a continued turn is exactly a long turn. #59
then adds up to 24 read-only rounds to a turn's budget. At the end of a
32-round turn about three quarters of what the agent read, and everything it
said to itself about it, is not in the request that writes the answer; at 64
rounds, about nine tenths. The model then has three options, all bad:

- **Answer from the last seven rounds.** The forced final call — made only
  when the user picks #61's *Answer now* — sees only those, so a report on a
  large repository describes whatever the agent happened to look at last.
- **Re-read.** That is what the notice tells it to do. Under #59 §5.3 an exact
  repeat read is charged to the action budget. That is right for a model stuck
  in a loop, but here the engine caused the repeat and the action budget pays
  for it.
- **Guess.** It knows it read something relevant but not what it said.

**Between turns.** Real work is several turns: "find out how approvals are
decided", then "add a cause for X", then "now test it". A new turn sees the
earlier ones only through `build_model_prompt`, which gives it the last eight
session messages, each cut to 2,000 characters
([chat.rs:4762](../../crates/workspace-engine/src/chat.rs)). However well
the investigating turn went, the turn that acts on it starts from a
2,000-character fragment of its answer and has to investigate again. #55 would
replace that window with a summary, but it is not started and waits on #26.

The request window exists for good reasons: it bounds per-call size and cost
(#47 requirement 6, `OBSERVATIONS.md` entry 6). Raising it only moves the point
where the agent forgets, and makes every call after that point more expensive.
What is missing is a small, bounded record of **what was learned**, written by
the agent that learned it, that survives the cut and reaches the next turn.

## 2. Current State

- `bounded_messages` ([chat.rs:4802](../../crates/workspace-engine/src/chat.rs))
  keeps `messages[..2]` (system, user), adds one system notice, and keeps the
  newest rounds that fit `max_messages - 3`. It cuts at round starts so a tool
  result never loses its call. If the newest round alone is larger than the
  budget, it is kept whole and the request goes over the bound. It is applied
  to every request in the turn loop (`chat.rs:2331`), including the requests
  of a turn #61 continued.
- The notice says how many messages and results were left out, not **which**
  calls they were. The model cannot tell which reads it already made.
- Nothing warns the model before a round leaves the window. The first it hears
  of a cut is the notice, after the results are gone.
- `build_model_prompt` (`chat.rs:4748`) seeds a new turn with the last eight
  session messages, each truncated to 2,000 characters. Nothing else from an
  earlier turn reaches it.
- `agent_max_turn_messages` is restrict-only at repository scope and a
  **Capability** in the key partition (`config.rs:1602`, `config.rs:2812`),
  labelled "Messages sent per model request" (`effective_policy.rs:161`).
- `PendingChatTurn` (`chat.rs:4396`) persists the full `messages` array, so a
  paused turn resumes with every round. The cut only affects what is sent.
  Since #61 it also carries `round_budget`, and a round-budget pause is one of
  the ways a turn is persisted and resumed — on every *Continue*, not only
  after an approval or a crash.
- #59's seen-call set (its §5.3, not yet built) lives in the loop and is not
  persisted, so a resumed turn starts with it empty. With #61 that is every
  *Continue*.
- The agent has no way to write anything down that is not a tool call or
  assistant text, and both of those are dropped with their round.
- `action_effect` (`chat.rs:4960`) classifies every tool as `ReadOnly`,
  `WritesRepository` or `ShapesTurn`. `ShapesTurn` tools ("writes only the
  session log") cannot be batched with reads (`action_is_batchable_read_only`).
  `mode_permits` (`mode.rs:233`) allows the eight read tools in every mode:
  #47's six plus #60's `list_worktrees` and `read_git_log`.
- #55 (not started) summarises a conversation across turns and explicitly
  leaves the in-turn array alone (#61 §4 says the same).
- The eval harness's deterministic tier scripts every model response, so it
  can check which calls a run made and how many rounds it used, but not what a
  request contained (`crates/eval-harness/src/scenario.rs`).

## 3. Requirements

1. The agent has a **notes** tool. With it, the model can add to or replace a
   short plain-text record of what it has learned.
2. The current notes are sent in **every** later request of the turn, in a
   fixed place right after the seed. `bounded_messages` never drops them, and
   they do not count toward `agent_max_turn_messages`.
3. Notes are bounded by `agent_max_turn_notes_chars` (default 8,000). An append
   that would go over the bound is refused, and the tool result says how full
   the notes are and that they can be rewritten shorter with replace. The value
   is clamped to `ABSOLUTE_TURN_NOTES_CHARS` (32,000). The key is
   **restrict-only** at repository scope and a **Capability**.
   **`0` restores today's requests exactly**: the tool is not offered, nothing
   is pinned or carried, and requirements 4 and 5 do not apply, so the elision
   notice is today's. This mirrors #59 requirement 6.
4. When notes are on and rounds are elided, the notice lists **which calls**
   were left out, one short line each (tool and canonical arguments, as #59
   §5.3 defines them, including the checkout a call read), and stays within a
   fixed size. This tells the model what it has already looked at, though not
   what it found.
5. **Before a round leaves the window, the model is told.** When notes are on
   and the window is full, every request names the oldest round still in view
   — its calls in the same canonical form — as the one the next request drops,
   and asks the model to record in its notes anything from it that it still
   needs. The model gets one round to act before the results are gone.
6. **Notes carry into the next turn.** A new turn in the same session starts
   with the previous turn's final notes as its own, pinned from its first
   request and marked as carried from an earlier turn. The model extends or
   replaces them as in any turn, so they stay one bounded record across the
   session rather than growing per turn.
7. Notes are part of the turn's durable state. They are written to the session
   log when they change, persisted in `PendingChatTurn` with the list of
   dispatched calls, and restored when a turn resumes after an approval, a #61
   pause (every *Continue* and *Answer now*) or a crash.
8. *(Needs #59.)* For the round budget, a notes call counts the same as a read:
   a round of reads and notes is an exploration round. An append identical to
   an earlier append in the same turn is a repeat. A read the model makes again
   because it was elided is **still** a repeat; requirements 4 and 5 are what
   tell the model it already made it. Because this spec persists the
   dispatched-call list (requirement 7), #59's seen-call set is rebuilt on
   resume from the calls in it since the last round that wrote (#59 §5.3
   clears the set there), instead of starting empty, so a *Continue* does not
   reset repeat detection.
9. Notes go through the secret scanner before they are stored or sent. They are
   model-written text that may quote what the agent read.
10. The user can see a turn's notes: in the conversation, collapsed under the
    turn that wrote them, with carried text marked as such, and in session
    export.
11. Every notes change is audited with its size and the running total, not its
    text (the text is in the session log). A carry is audited with its size.
    The audit records of the forced-final and exhausted paths include the notes
    size. The eval harness reports how many notes calls a run made and how many
    reads repeated an elided call.

## 4. Non-goals

- **Summarising elided rounds or earlier turns with a model call.** It would
  cost a billed call per cut that the user did not ask for at that moment (the
  same objection #55 §5 deals with using an off switch). The quality of the
  summary would be outside both the user's and the agent's control. The agent
  is the one that knows what mattered in a read, so the agent writes it down.
  Summarising a whole conversation stays #55's job; this spec carries only the
  agent's own bounded notes, and #55's summary should read them.
- **Cutting the window in steps.** The first draft dropped rounds until the
  window was half full and then held the cut still, so the request prefix
  stayed stable for caching. That gives the model less history than today —
  about five rounds on average instead of seven — which is the opposite of
  what this spec is for. It is a cache optimisation, and it is recorded in
  #49 §5.9 to be measured there.
- **Raising `agent_max_turn_messages` or changing it to a token measure.** That
  moves the point of forgetting and makes each later call bigger. A token-based
  bound may be worth doing for size reasons, but it does not solve this.
- **Persistent project memory.** That is #28–#30. Notes are working memory for
  one session's task, not facts about the project, and they end with the
  session.
- **Delegated exploration** (a sub-agent that explores and returns only a
  conclusion). That would also keep the main request small, but #38 is deferred
  and its premise is unmeasured. This spec should be measured first (§6).
- **Telling the model how to explore.** Beyond describing the tool, the cut
  and what is about to leave the window, the system prompt does not prescribe
  a method.

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
the turn when earlier rounds are dropped and carry into the next turn of the
session, they are the place to record findings worth keeping (paths, what a
component does, how pieces connect, open questions), and they should be short.
That text, the notice in §5.3 and the warning in §5.4 are the only changes to
model-facing prompts.

`ToolAction::TurnNotes { text, replace }` gets its own `ActionEffect` variant,
`Notes`. It is not `ReadOnly`, because it changes turn state. It is not
`ShapesTurn` either, because those cannot be batched with reads, and recording
a finding in the same round as the read that found it is exactly what should
happen. `action_is_batchable_read_only` becomes `ReadOnly | Notes`, and the doc
comment on `action_effect` says the variant also decides the budget class
(requirement 8). `tool_action_marker` marks it not side-effecting: it writes
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
             system   notes block (header below) + notes
             system   elision notice (§5.3), only when rounds were cut
             …        newest rounds
             system   pre-cut warning (§5.4), only when the window is full
```

The notes block's header is "Notes you recorded earlier in this turn:", or,
while the notes still hold text carried from an earlier turn (§5.5), "Notes
carried from earlier turns in this session, and added to in this one. Files
may have changed since they were written:". The notes come before the elision
notice so that the notice stays next to the cut. The warning goes last, next
to the newest round, where the model reads it before deciding its next calls.
None of the three blocks counts toward `max_messages`, so a repository that
lowers the window cannot push the notes out. The size of the notes is bounded
by requirement 3 instead, and the notice and the warning by their own caps.

`agent_max_turn_notes_chars = 0` keeps every request exactly today's for the
whole turn (requirement 3).

### 5.3 Saying which calls were cut

The elision notice becomes:

```
Earlier rounds of this turn were elided to bound the request: 31 messages
(26 call results) omitted. Calls already made, oldest first:
  read_file docs/specs/README.md 1–400
  list_directory docs/specs
  search_content "Status:" docs/specs/**/*.md
  read_file docs/specs/60_worktree_aware_read_tools/tasks.md (worktree .claude/worktrees/spec60)
  …and 9 more
The session log still holds every result. Your notes are above.
```

Each line uses #59 §5.3's canonical form: paths relative to the checkout the
call read, resolved through `PathPolicy`, and a call into a registered worktree
names that worktree, so a read of the same path in two checkouts prints as two
different lines. The list is capped at 40 lines. Past the cap it says "…and N
more".

The canonical form is computed once per dispatched call and appended to a
per-turn **call list**, so building the notice and the warning does not
re-decode calls. The list also marks each round that wrote (#59 §5.3 clears its
seen-call set there). It is kept whatever the key's value — it is not sent
unless requirement 4 or 5 applies — because #59 needs it: its seen-call set is
the list's entries since the last write, and persisting the list (§5.6) is
what lets a resumed turn rebuild that set. Whichever of #59 and this spec is
built first adds the list; the other reuses it.

### 5.4 Warning before the cut

Once the tail fills the window, each new round pushes the oldest one out, so
the oldest round in view is the one the next request will not contain. When
notes are on and `tail.len() >= budget`, the request ends with:

```
The window is full. With your next request, round 7 leaves it:
  read_file crates/workspace-engine/src/command_policy.rs 1–400
  search_content "ApprovalCause" crates/**/*.rs
Record in turn_notes anything from these results you still need.
Notes: 2,140 / 8,000 characters.
```

The round is named by its position in the turn and listed with the same
canonical lines as §5.3, capped at 10 calls. A newest round larger than the
one it replaces can push out more than one; the warning names the oldest, and
the elision notice still lists every call that went. The warning is a
**system** message after the newest round, not part of the tool results, so
it never changes what a call returned.

This puts the model's attention where the information is about to be lost,
rather than relying on it to remember an instruction from the tool
description dozens of rounds earlier. It costs one short message per request
while the window is full, and nothing before that. It does not force a notes
call: a round whose results no longer matter needs no note, and the model is
the one that knows.

### 5.5 Carrying notes into the next turn

When a turn starts in a session whose previous turn has notes, the new turn's
`turn_notes` starts as that turn's final notes, and a `turn_notes` event with
`carried: true` and the source task id is written before the first request.
"Previous turn" is the turn the new one follows in the conversation as it
stands, read from the session log, so a rewind to before a turn carries the
notes of the turn before it, not of the turn rewound away. The previous turn's
final notes include anything it carried itself, so findings flow along the
session as one record. The model keeps it within the bound by replacing
rather than appending, as within a turn.

The carried text keeps the header from §5.2 until the model replaces the whole
record; an append keeps it, because the carried part is still there. Every
kind of new turn carries: a normal prompt, #61's *Ask again* (so a turn that
ran out keeps its findings) and a turn started by a worker (#57). A new
session starts with no notes.

Carrying adds the notes to every request of the next turn from its first one,
so a session that investigates and then acts pays up to the bound — about
2,000 tokens at the default — on each of those calls. That is the point: it
is far less than investigating again. A user who does not want it sets the
key to `0`.

Notes are text the model wrote from what it read, so they carry the same trust
as the assistant's own answers, which already cross turns through
`build_model_prompt`: they are redacted (requirement 9), shown to the user
(§5.8), and they cannot widen anything — the mode, profile and policy of the
new turn are its own.

### 5.6 Persistence

- **Session log:** a `turn_notes` event with the task id, `replace`,
  `carried` and the redacted notes text after the change. The latest event for
  a task is that task's notes. Older readers skip unknown events.
- **`PendingChatTurn`:** beside #61's `round_budget`, a `#[serde(default)]
  turn_notes: String` and a `#[serde(default)]` call list (§5.3). A turn
  written before this spec resumes with no notes and an empty call list, which
  is today's behaviour.
- **Crash recovery** (#17): a task resumed from the log rebuilds its notes from
  its latest `turn_notes` event. Its call list is rebuilt from the tool calls
  in its saved `messages`.
- **#61's pause:** a round-budget pause is a `PendingChatTurn`, so the notes
  and the call list come with it. *Continue* resumes the same turn and resets
  neither. *Answer now*'s forced final call carries the notes block. *Ask
  again* starts a new turn, which carries the stopped turn's notes (§5.5).

### 5.7 The setting

`agent_max_turn_notes_chars: usize` in `Config`. Default 8,000 (roughly 2,000
tokens, under 2% of the 141k-token request in #59 `context.md` §1). Ceiling:
`ABSOLUTE_TURN_NOTES_CHARS = 32_000`, a constant with the same kind of doc
comment as `ABSOLUTE_TOOL_ROUND_CAP`. Restrict-only, lower wins, in the same
loop as `agent_max_turn_messages` (`config.rs:1602`), `=> Capability` in the key
partition. It touches all eight places a key touches (spec 21 `context.md`
§3.8), including permission profiles (`profile.rs`) and the effective policy
label ("Notes size").

Restrict-only for the same reason as #59 §5.4: a larger value is sent on every
later call, now across turns too, so raising it spends the user's money. A
lowered key truncates nothing already written: carried notes longer than the
new bound are carried whole, and the next append is refused until the model
replaces them with something shorter.

### 5.8 What the user sees

- A turn that has notes shows a collapsed **Notes** disclosure below the turn's
  answer, using the disclosure pattern from #41 and #42. It shows the final
  text after redaction, and says "carried from the previous turn" when the
  turn started with carried notes. A turn with no notes shows nothing.
- Session export includes each turn's final notes and whether they were
  carried.
- Progress adds `· notes 2.1k` to the round label while a turn runs and has
  notes, next to #59's `· N reads`.

### 5.9 Eval and tests

**Engine tests** (in `chat.rs` tests, using a scripted adapter that records
every request it receives):

- notes written in round 2 of a 20-round scripted turn — run past the
  eight-round segment through #61's *Continue* — appear, unchanged, in the
  request of round 20 and in the forced final request *Answer now* makes;
- with `agent_max_turn_notes_chars = 0` every request of a turn long enough to
  be cut, and of the turn after it, is byte-identical to one built before this
  spec: no tool definition, no notes block, no warning, today's notice text;
- an append over the bound is refused, the notes are unchanged, and a replace
  that fits is accepted;
- the elision notice lists every elided call, oldest first, capped with
  "…and N more", and a read of the same path in the main checkout and in a
  registered worktree prints as two lines;
- once the window is full, every request ends with a warning naming the
  oldest round in view, and the next request's elision notice lists that
  round's calls; before it is full, no request has a warning;
- a second turn in the session starts with the first turn's final notes
  pinned under the carried header from its first request; a third turn after
  a second that replaced them starts with the replacement; after a rewind to
  before the second turn, a new turn carries the first turn's notes; a new
  session starts with none;
- notes and call list survive an approval pause, a #61 pause (*Continue* and
  *Answer now*) and a crash resume; *Ask again*'s turn starts with the stopped
  turn's notes;
- a secret placed in a note is redacted in the log, the request, the carried
  copy and the export.

**Deterministic scenario `long_exploration_memory`:** a 20-round scripted turn
over the `rust-workspace` fixture, then a second turn in the same session.
Round 2 batches a read with a notes call, and the first turn answers. It
asserts the turn completes, the run records one notes call, no read repeats an
elided call, and the second turn's first request carries the notes. The script
decides the calls, so like `batched_reads` these assertions are
`deterministic_only`: they pin the dispatch and accounting paths, not model
behaviour.

**Live acceptance (§6):** the real check, because only a real model decides
whether to take notes and whether they are good enough to work from.

### 5.10 Documentation

On Done: `docs/USER_GUIDE.md` (what notes are, that they carry into the next
turn of a session, where they appear, the key), `docs/TROUBLESHOOTING.md` (why
an answer may say it relied on notes, why a later turn knows something it did
not re-read, what the elision notice and the warning mean, how to read
`turn_notes` events in a session log). `SECURITY.md` only if it by then lists
per-key budget classes.

## 6. Acceptance Criteria

- A notes call in round 2 of a long turn is present in every later request of
  that turn, including the forced final call, and none of those requests drops
  it at any `agent_max_turn_messages` value down to the floor.
- With the key at `0`, the tool is not offered and every request — including
  its elision notice, and the requests of the next turn — is identical to one
  built before this spec, across the existing deterministic scenarios.
- A value above 32,000 is clamped. A repository config that raises the key is
  ignored and audited as a restrict-only weakening attempt; one that lowers it
  applies.
- An over-bound append is refused with the current size, and the turn goes on.
- With notes on, the elision notice names the elided calls in #59 §5.3's
  canonical form, including the checkout each read, within its cap.
- With notes on and the window full, every request warns which round leaves
  next and lists its calls.
- The next turn in a session starts with the previous turn's final notes,
  marked as carried; a rewind carries from the turn before the rewound one; a
  new session starts empty.
- Notes and the call list are restored after an approval pause, a #61
  *Continue* and a crash, *Answer now*'s forced call carries the notes, and
  *Ask again*'s turn starts with them.
- A secret in a note never reaches the request, the log, a carried copy or the
  export unredacted.
- The user sees a turn's notes collapsed under that turn, with carried text
  marked, and in the export.
- With #59 built: a round of reads and notes is an exploration round, an
  identical append is a repeat, and a repeat made before a *Continue* is still
  caught after it.
- **Live, within a turn:** against `deepseek-v4-flash` on this repository,
  with #59 built, run two prompts: #59's motivating prompt, and "Where is the
  command approval decision made, what feeds it, and which specs changed it?".
  Run each once with the key at its default and once at `0`, continuing
  through #61 until the model answers or the turn reaches its hard cap. Record,
  for each run: rounds used, *Continue*s taken, notes calls made, reads that
  repeated an elided call, and every claim in the answer that depends on a
  result elided before the answering request — checked against the session
  log. The default run is **accepted only if its answer uses findings from
  before the last cut correctly**: it names files, functions or facts that
  were read only in elided rounds, and they are right. Fewer repeated reads
  than the `0` run is expected too, but it is not enough on its own. If the
  model does not take notes, or its answer still comes only from the last
  rounds in view, the spec is **not** accepted: the tool description and the
  warning are revised and the run repeated, not the window enlarged.
- **Live, across turns:** in the same session as the second prompt's default
  run, ask "Add a new approval cause for commands that write outside the
  repository: where does it go and what has to change?". Accepted if the
  answer uses the carried notes — it names the places the first turn found
  without reading all of them again — and the run records how many reads it
  repeated from the first turn.

## 7. Implementation Notes

None yet.
