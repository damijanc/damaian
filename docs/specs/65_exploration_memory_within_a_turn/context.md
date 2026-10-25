# Context: Exploration Memory

Background for [`tasks.md`](tasks.md). Read it before Task 1. This file is the
code map the plan was written against on 2026-10-24, plus the decisions the
proposal leaves open and the places where concurrent work touches the same
code. Where a correction here contradicts [`proposal.md`](proposal.md), the
correction wins, and the task that acts on it updates the proposal in the same
change (the Corrections table in `tasks.md` records the ones found while
planning).

`chat.rs` line numbers drift faster than any other file in this repository
(see `docs/specs/README.md` → "Parallel work"). Re-check a line before editing
near it. Everything below was checked on 2026-10-24 against
`spec65-plan-65b021`.

**Re-checked on 2026-10-25 against main `e7e5258`**, where #26 is Done. The
seed is still exactly two messages (`system` + `user`), `bounded_messages` is
unchanged, and `build_model_prompt` now takes a `ContextPlan` and a scanner.
Line numbers below have moved by roughly 500–1,300; resolve by function name,
not by the number. The notes block stays a system message, not a `Pinned`
context item: `context_budget.pinned` exists, but notes are turn state and must
survive `bounded_messages` without a repository read.

## 1. Spec citations that moved

| Proposal cited (2026-10-18/19) | Now |
|---|---|
| `bounded_messages` at `chat.rs:4802` | `chat.rs:6161` |
| `bounded_messages` call in the turn loop at `chat.rs:2331` | `chat.rs:2911` |
| `build_model_prompt` at `chat.rs:4748` | `chat.rs:6101`; the `take(8)` is `:6111`, the 2,000-character cut `:6121` |
| `PendingChatTurn` at `chat.rs:4396` | `chat.rs:5696` |
| `action_effect` at `chat.rs:4960` | `chat.rs:6322` |
| `agent_max_turn_messages` restrict-only at `config.rs:1602` | `config.rs:1604–1612`; the field is `:352` |
| `agent_max_turn_messages` partition at `config.rs:2812` | `config.rs:2843` |
| Effective policy label at `effective_policy.rs:161` | `:168` ("Messages sent per model request") |
| `mode_permits` at `mode.rs:233` | Unchanged (`:233`) |
| `PendingChatTurn`'s `round` / `exploration_rounds` | `chat.rs:5703` / `:5709`; the struct is `:5696` |
| `#61` "Tasks 1–5 on main since 2026-10-19" | Done 2026-10-23, Tasks 1–7; `ABSOLUTE_TURN_ROUND_CAP = 64` at `chat.rs:5970` |
| `#59`'s seen-call set "not yet built" | Built 2026-10-23: `seen_read_calls` at `chat.rs:2760`, `read_call_key` at `chat.rs:5116` |

## 2. The turn loop, and where the notes state lives

`ChatOrchestrator::run_agentic_turn` is `chat.rs:2505`. It takes
`mut messages: Vec<ModelMessage>`, `mut action_rounds: u32` and
`mut exploration_rounds: u32`, and returns `ChatTurnResult` (`chat.rs:462`).

- **Mode and tools are captured once at the top** (`chat.rs:2525`): `mode`,
  `self.config.profile_capabilities()`, and the `native_tools` closure
  (`chat.rs:2550`) that asks `permits` about a placeholder `ToolAction` per
  definition. The definition list is built at `chat.rs:2596–2690`; add
  `turn_notes` beside the read tools there, behind the same
  `key > 0 && permits(...)` gate.
- **The request** is built once per iteration at `chat.rs:2903`; its `messages`
  field is `bounded_messages(&messages, self.config.agent_max_turn_messages)`
  (`:2911`). `force_final` removes `tools` (`:2901`) but still bounds the
  messages, which is why the notes must be inside `bounded_messages` rather
  than added by the caller.
- **Audit** `model_request_prepared` is written at `chat.rs:2925`, with
  `toolRound`/`toolRoundLimit` and `explorationRounds`/`explorationLimit`
  (`:2941–2942`). `sink.phase` is called at `:2946` with the same two counts.
- **Classification** happens after the model call returns and before dispatch:
  the canonical keys are computed at `chat.rs:3348`, `repeats_a_seen_call` at
  `:3352`, `round_is_exploration` at `:3363`. Its four conditions are: calls
  non-empty, no undecodable call, every call `ReadOnly`, not a repeat, room in
  the allowance. A `Notes` call must be treated as exploration, so this becomes
  `ReadOnly | Notes` — one predicate, not a second list.
- **The per-call loop** begins at `chat.rs:3740` and creates the action marker,
  checks permission (`action_permission`, `chat.rs:5076`), dispatches, and
  finishes the marker. A dispatched read's key is captured at `chat.rs:3682`
  and inserted into `seen_read_calls` after the marker closes. The write flag
  is captured per call and clears the set after the round (`chat.rs:4601`).
- **Charging** is at `chat.rs:4605–4612`: `exploration_rounds += 1` or
  `action_rounds += 1`.
- **Every pause saves a `PendingChatTurn`** at one of seven construction
  sites: `chat.rs:3496`, `:3765`, `:4141`, `:4253`, `:4376` (in-loop terminal
  arms) and `:4760` / `:4881` (the question and round-budget helpers). Six
  resume calls pass the saved counts back into `run_agentic_turn`
  (`:1430`, `:1555`, `:1707`, `:1847`, `:1949`, `:1983`); the initial call is
  `:1055`.

**Decision:** `turn_notes: String`, a `carried: bool` and the call list are
**locals of `run_agentic_turn`**, beside `messages`. They are not stored in
`messages` (proposal §5.2): a replace would rewrite history and the persisted
array would stop matching what was dispatched. Every `PendingChatTurn` save
site copies them in; every resume site passes them back out, exactly as
`exploration_rounds` is threaded today.

## 3. `bounded_messages` in full

`chat.rs:6161`. Today:

1. `if messages.len() <= max_messages { return messages.to_vec(); }`
2. `head = messages[..2]` (system prompt, user seed).
3. `tail = messages[2..]`.
4. `budget = max_messages.saturating_sub(3)` — head (2) plus one notice.
5. `drop_count = tail.len().saturating_sub(budget)`, then moved forward to the
   next round start (an `assistant` message) or back to the previous one, so a
   tool result never loses its call (`:6179–6188`).
6. `results` counts the non-assistant messages in the dropped prefix
   (`:6189`).
7. The return is `head + [notice] + tail[drop_count..]`, with the notice text
   as a literal at `:6195`.

The tests that pin it: `a_long_turn_is_clamped_and_says_what_it_cut`
(`chat.rs:7975`), `a_clamp_never_splits_a_batched_round` (`:8024`) and
`a_turn_inside_the_window_is_left_alone` (`:8055`).

**Decision:** the signature becomes
`bounded_messages(messages, max_messages, window: &NotesWindow<'_>)`, where
`NotesWindow` carries the notes text, the carried flag, the call list and the
per-call cap. It returns the same `Vec<ModelMessage>` with up to three extra
system messages inserted:

- the notes block immediately after `head`;
- the notice where the notice is today (so the cut stays next to it);
- the pre-cut warning appended after the newest round.

`budget` **stays** `max_messages.saturating_sub(3)`. The two new blocks are
added beside the bounded tail, so the round budget is unchanged and the request
may exceed `max_messages` by their size. That is deliberate (proposal §5.2,
Correction C9): shrinking the round budget would evict more history in a
repository that lowers the window.

`NotesWindow::none()` (key `0`) must return exactly today's vector, including
today's notice literal. The test is a **pinned reference copy**: a test-local
`bounded_messages_before_65` containing today's algorithm verbatim, compared
against the real function for a matrix of lengths and `max_messages` values
down to the floor of 3. That is the strongest available form of "byte-identical
to before", because the pre-change code cannot be compiled beside the new one.

## 4. The notes tool: every exhaustive `match` it enters

`ToolAction` is at `chat.rs:6227`. Adding `TurnNotes { text: String, replace: bool }`
forces a decision in each of these, all of which have no wildcard or an
explicit fallback that must be reconsidered:

| Site | Line | Decision |
|---|---|---|
| `action_effect` | `chat.rs:6322` | New `ActionEffect::Notes` variant |
| `ActionEffect` enum and doc | `chat.rs:6309` | Doc says the effect is also the budget class (spec 59 §5.1) |
| `action_is_batchable_read_only` | `chat.rs:6348` | `ReadOnly \| Notes` |
| Round classification | `chat.rs:3363` | `ReadOnly \| Notes` (requirement 8) |
| `before_tool_effect` | `chat.rs:6370` | Falls into `_ => Refuse` for `RequestApproval`, which is right: an answer is not an approval |
| `tool_action_marker` / `_unscoped` | `chat.rs:6504` / `:6541` | `("turn_notes", chars.to_string(), false)` — not side-effecting |
| `tool_event_payload` | `chat.rs:6414` | `("turn_notes", Vec::new())` |
| `action_awaits_plan_review` | `chat.rs:6636` | Falls through `tool_action_marker(other).2`, so `false` |
| `tool_action_label` | `chat.rs:6705` | "Writing notes" / "Updating notes" |
| `tool_action_from_call` | `chat.rs:6924` | Decode `text` (non-empty) and `replace` (default false) |
| `profile_permits` | `mode.rs:155` | `None` (no capability key), beside `AskUser` |
| `mode_permits` | `mode.rs:233` | `Permission::Allowed` in every mode |
| `read_call_key` → general call key | `chat.rs:5116` | `Some(vec!["turn_notes", replace, text-hash-or-text])` so an identical append is a repeat |

`mode_permits` and `profile_permits` are separate exhaustive matches; placing
the tool in one and not the other does not compile.

The tool definition goes beside `read_file_tool_definition`
(`chat.rs:6824`). Decoding returns an error (not `None`) for an empty `text`,
so the model learns what was wrong rather than seeing "undecodable": that is
the `ask_user` precedent (`chat.rs:6967`).

**Dispatch:** the `turn_notes` arm mutates `turn_notes` and `carried` in place.
It redacts `text` through `self.scanner.redact(...)` before measuring, storing
or sending (requirement 9), then:

- append: refuse when `notes.len() + 2 + text.len() > limit`, with a result
  naming `notes.len()` and `limit` and suggesting `replace`;
- replace: replace the whole record, set `carried = false`, and refuse when the
  new text alone is over `limit` (proposal §5.7: carried notes over a lowered
  bound are kept whole, and the next write must shorten them);
- on success, append the `turn_notes` session event and the
  `turn_notes_recorded` audit event, and return
  `notes: {total} / {limit} characters`.

The batch path matters: an all-read batch is dispatched concurrently
(`run_read_only_batch`). A round containing a `Notes` call must still batch
with reads, so the batch's "read-only" predicate becomes the same
`ReadOnly | Notes` predicate, and the notes mutation happens on the loop thread
after the batch returns — never inside the batch worker. Keep the mutation at
the single dispatch arm.

## 5. #59's repeat detection, exactly as built

- Identity: `ChatOrchestrator::read_call_key(&self, repository_root, action) ->
  Option<Vec<String>>` (`chat.rs:5116`). `Some` for the eight read tools,
  `None` for everything else. The vector is a tagged identity, not a display
  string: `["read_file", checkout, path, "range", "10", "20"]`.
- Checkout: `read_call_checkout` (`chat.rs:5254`) resolves `worktree` through
  spec 60's resolver; the session checkout is a marker, and a refused
  `worktree` is kept as given.
- Paths: resolved through `PathPolicy` relative to that checkout; a refused
  path is kept as given.
- Set: `seen_read_calls: HashSet<Vec<String>>` (`chat.rs:2760`), a local of
  `run_agentic_turn`. `None` is never a repeat (err toward exploration).
- Per round: `read_call_keys` is computed once before dispatch
  (`chat.rs:3348`); a duplicate within the round is a repeat too.
- On dispatch: the key enters the set only when `permission.is_allowed()`
  (a denied call never dispatched); a read that failed on its own still
  counts (`chat.rs:3682–3689`).
- On write: a round that dispatched an allowed `WritesRepository` call clears
  the set after the round (`chat.rs:4601`).
- On resume: the set starts empty (`chat.rs:2756–2760`).

**Decision (requirement 8):** the plan persists a call list and rebuilds the
set from it on resume, taking entries since the last `writes: true` entry.
That changes #59's accepted "one uncaught repeat after a resume" into "none",
which is strictly narrower and needs a test that a repeat made before a
*Continue* is still caught after it.

**Decision:** generalize `read_call_key` into `call_key`, which also covers
`TurnNotes`, and add `render_call_line` over the same resolved pieces. Do not
duplicate the checkout/path resolution in the renderer: a second copy is how
the notice and the repeat rule would start disagreeing. Non-read calls
(commands, patches, plans) have no canonical row; render them from
`tool_action_marker`'s `(name, reference)` with the reference redacted and
truncated. The notice lists every dispatched call in the dropped rounds,
oldest first.

## 6. Persistence, pauses and crash recovery

### `PendingChatTurn`

`chat.rs:5696`. Relevant fields today: `round` (`:5703`), `exploration_rounds`
(`:5709`, `#[serde(default)]`), `messages` (`:5712`), `round_calls_fed_back`,
`mcp_call`, `web_diagnostic_call`, `plan_review`, `provider_fallback`,
`round_budget` (`RoundBudgetPause`), and the question pause. Notes add:

```rust
#[serde(default)] turn_notes: String,
#[serde(default)] turn_notes_carried: bool,
#[serde(default)] calls: Vec<RecordedCall>,
```

`RecordedCall` (serialisable) is `{ round: u32, message_index: usize,
line: String, key: Option<Vec<String>>, writes: bool }`. `message_index` is the
index of the round's assistant message in `messages`, which is append-only
within a turn, so the index is stable and lets `bounded_messages` decide which
entries precede the cut without re-deriving round boundaries.

### Resume routes

- `resume_after_command_decision` / `_with_options` (`chat.rs:1074`, `:1092`)
- `resume_after_plan_decision` (`:1462`)
- `resume_after_answer` (`:1579`)
- `resume_after_fallback_decision` (`:1728`)
- `resume_round_budget` (`:1873`) — *Continue* and *Answer now*
- `tool_budget_ask_again_prompt` (`:2004`) — *Ask again* builds a **new** turn,
  which carries notes through the normal new-turn path (§5.5), not through
  `PendingChatTurn`

All but *Ask again* resume the same task; each must pass the saved notes, the
carried flag and the call list into `run_agentic_turn`, and rebuild
`seen_read_calls` from `calls` since the last `writes: true`.

### Crash recovery

`recovery::reattach_pending_approvals`
(`crates/workspace-engine/src/recovery.rs:339`) re-presents a pause after a
restart. A pending turn written before this spec loads with the
`#[serde(default)]` values, which is today's behaviour. When a task is resumed
from the log rather than from a pending file, notes are rebuilt from its latest
`turn_notes` event and the call list from the tool calls in `messages` using
`tool_action_from_call` (`chat.rs:6924`). Note that a task resumed *from the
log* does not have `messages` unless it was stored; the durable source is the
`turn_notes` event plus the task's stored pending file, so the plan's crash task
must state which source it uses and test both (pending file present / only the
log available).

### Session log

Appends go through `SessionStore::append_session_event`
(`session.rs:2267`) via typed helpers (`create_plan`/`revise_plan` at `:1418`,
findings at `:1697`, usage, questions). Each event is one JSON object per line
with `eventId`, `seq`, `timestampMs`, `eventType`, `payload`. A `turn_notes`
event is:

```json
{"taskId":"task_…","replace":true,"carried":false,"text":"…","chars":812,"totalChars":812}
```

The text is already redacted. `SessionStore::read_turn_notes(session_id) ->
BTreeMap<String, TurnNotes>` replays `active_events(content)`
(`session.rs:2447`) and keeps the latest event per task, which makes rewinds
correct without a second rule.

### Rewind

`SessionStore::rewind_conversation` (`session.rs:2252`) appends a
`conversation_rewound` marker with `throughEventSeq`. `active_events`
(`session.rs:2447`) filters everything after the newest marker, and
`read_tasks` (`session.rs:643`), `read_messages` (`:570`) and
`read_messages_with_seq` (`:586`) all read through it. `read_turn_notes` must
do the same. A new turn after a rewind therefore sees the turn before the
rewound one as "previous", with no extra code.

## 7. Config key plumbing

Spec 21 `context.md` §3.8 lists eight places; `agent_max_exploration_rounds`
(spec 59) is the most recent complete example and can be copied end to end.
For `agent_max_turn_notes_chars: usize`:

| Place | Where the neighbouring key is |
|---|---|
| `Config` field | `config.rs:352` (`agent_max_turn_messages`) |
| `Config::default` | `config.rs:2639` |
| Exhaustive destructure in `apply_overlay_scoped` | `config.rs:1123` |
| Restrict-only apply block | `config.rs:1604–1612` (the tuple loop with `agent_max_turn_messages`) |
| `ConfigOverlay` field | `config.rs:2705` |
| Key partition | `config.rs:2843` (`=> Capability`) |
| Overlay key parser (`ConfigOverlay::set`) | `config.rs:3067` |
| `Config::to_policy_text` (config-show) | `config.rs:2529` |
| `ConfigOverlay::to_policy_text` (overlay serializer) | `config.rs:3597` |
| Profile carried key | `profile.rs:362` / `:545` / `:676` (`split_profile_keys` at `:322`, `equals_base_limit` at `:663`) |
| Effective policy label | `effective_policy.rs:168`, `rule_label` at `:129` |

Helpers: `restrict_only_limit` (`config.rs:4550`, `usize`), `restrict_only_u32`
(`:4572`), `parse_read_lines` (`:4517`, the parser
`agent_max_turn_messages` uses) and `parse_exploration_round_count` (`:4501`,
the accept-0-and-clamp precedent). Requirement 3 needs both properties —
accept `0`, clamp above the ceiling — so this key needs its own parser like
`parse_exploration_round_count`, or `parse_read_lines` widened (it already
accepts a positive count and refuses 0; check before reusing).

The ceiling constant: `ABSOLUTE_TURN_NOTES_CHARS = 32_000`. Spec 59 put
`ABSOLUTE_EXPLORATION_ROUND_CAP` in `chat.rs` (`:5963`) and left the parser
with a literal `32` naming it in a comment. This key is used by both the
parser and the bound check, so Task 1 defines
`pub const ABSOLUTE_TURN_NOTES_CHARS: usize = 32_000;` in `config.rs` beside
`parse_read_lines` and uses it in both places. Do not define a second value.

Tests for the key live in `tests/foundation.rs`,
`tests/repository_config_trust.rs` and `tests/permission_profiles.rs`, exactly
where spec 59 put its four.

## 8. UI, export and progress

- **Disclosure:** `createDisclosure(label, panel)` (`app.js:4349`) is the one
  shared disclosure implementation (spec 42 promoted it). The closest model is
  `appendContextDisclosure` (`app.js:7436`), which folds a turn's read files
  under the turn. Notes need the same shape but durable, so the text must come
  from the shell, not from a live-turn-only payload.
- **Live turn:** `chat_result_json` (`desktop-shell/src/lib.rs:4141`) builds
  the turn response. Add `turnNotes` there from a new `ChatTurnResult` field.
- **Reload:** `handle_session` (`desktop-shell/src/lib.rs:543`) joins task
  statuses, usage, plans, failure kinds and web diagnostics by `taskId` in
  `task_states_json` (`:4702`). Add each task's notes the same way, read
  through `read_turn_notes`.
- **Progress:** `TurnPhase` (`chat.rs:168`) and `TurnSink::phase`
  (`chat.rs:262`) carry `exploration_round`/`max_exploration_rounds`; the
  shell's `phase_json` (`desktop-shell/src/lib.rs:3904`) sends them and
  `workers.rs:1101` mirrors them into the progress file. The label is built in
  `app.js` at `:4481` (` · N reads`). Notes add one count field along this
  whole path, or the label has nothing to read. Adding a seventh argument to
  `sink.phase` touches its three call sites (`chat.rs:965`, `:2946`, `:3618`).
- **Export:** `SessionStore::export_session` (`session.rs:2094`) renders
  Markdown and JSON; plans come from `read_session_plans` (`:1611`). Notes
  should be read once per task and rendered under the task, redacted through
  the same `redact` closure the other fields use (the export's redaction count
  must include them). The shell route is `/api/session-export`
  (`desktop-shell/src/lib.rs:307`, handler `:498`); the UI calls it from
  `exportSessionDownload` (`app.js:8538`).

## 9. Tests and fixtures a worker will use

- `crates/workspace-engine/tests/exploration_rounds.rs` — #59's suite: budget
  classification, repeat detection, resume. The notes-vs-budget tests belong
  beside them, because that file already owns a scripted long turn.
- `crates/workspace-engine/src/chat.rs` inline tests — `bounded_messages`,
  the tool contract, the notice and warning text.
- `crates/workspace-engine/tests/prompt_cache.rs:67` — the precedent for
  comparing whole `ModelRequest`s (`ModelRequest` derives `PartialEq`), which
  the key=0 byte-identical test extends.
- `crates/workspace-engine/tests/foundation.rs`,
  `repository_config_trust.rs`, `permission_profiles.rs` — the config key.
- `crates/workspace-engine/tests/round_budget.rs`, `token_ceiling.rs`,
  `crash_recovery.rs` — every pause and resume path the notes must survive.
- `crates/eval-harness/scenarios/*.toml` and `src/{scenario,runner,trace,record}.rs`
  — the deterministic scenario. `MockModelAdapter` (`model.rs:360`) records
  requests; `Scenario` (`scenario.rs:164`) has `turns` (model responses, not
  user turns), `approval_decision`, `round_budget_decision`, `answers`,
  `config_overlay`, `crash_mid_action`.
- Fixtures: fresh `git init` directories under the temp dir;
  `enable_index_watcher: false`; `DAMAIAN_DATA_DIR` for anything run outside a
  test; `MockModelAdapter` rather than HTTP mocks.

## 10. Collisions with concurrent work

### #26 context assembly — in progress right now, in another session

[`26_context_assembly/`](../26_context_assembly/proposal.md) rewrites context
assembly: `ContextItem` (`context_manager.rs:22`) gains a category, a line
range, provenance and a root; `build_context` (`context_manager.rs:81`) gains
per-category budgets, de-duplication, exclusion records and a manifest; the
`Required` category is never dropped. Its plan is not yet in a `tasks.md`, so
the exact `chat.rs` lines it will touch cannot be named from the specification
alone; what can be named is the code that must change for it to land:

- **`build_model_prompt` (`chat.rs:6101`)** — it renders `&[ContextItem]` into
  the user seed, and every field it reads changes shape. It is also where a
  "Required" item would be surfaced.
- **`ask_with_session_with_options`'s assembly block (`chat.rs:1030–1058`)** —
  `build_context_with_hooks` call, `build_model_prompt` call, and the
  construction of `messages[0..2]`. If #26 adds a `Required` context item or
  changes the seed's shape, this is the block it changes.
- **`ContextManager` itself** (`context_manager.rs`), which is not `chat.rs`
  and therefore does not collide with this spec's edits.

Requirement 6 (carry notes into the next turn) and §5.2 (place the notes block
next to the seed) touch that first two regions:

- **Task 7 (carry across turns)** edits the new-turn path — where `messages` is
  seeded and `carry_plan_from_the_previous_turn` runs (`chat.rs:949`) — and
  therefore overlaps both `build_model_prompt`'s call site and the seed
  construction. **It was re-checked after #26 landed on 2026-10-25** (main
  `e7e5258`): the seed is still two messages, so the split point did not move.
- **Task 4 (`bounded_messages` pinning)** inserts the notes block immediately
  after `messages[..2]`, so it depends on the seed being *two* messages. #26
  may change what the seed contains but should not change the count; if it
  does, Task 4 re-reads this section and adjusts the split point rather than
  hard-coding 2.
- Tasks 1–3 and 5–6 are ordered before Task 7 for this reason: they can land
  while #26 is in flight, provided they stay inside `bounded_messages`, the
  tool contract, config, and the call list — not the seed.
- If #26 adds a `ContextCategory::Pinned` item for notes (it names one), the
  notes block could be delivered as context instead of as a system message.
  **That is not this spec's design**: notes are turn state, not assembled
  repository context, and they must survive `bounded_messages` and reach the
  next turn without a repository read. Do not move them into `ContextItem`
  while implementing #26 or #65.

### #66 plan left open at turn end — also edits `chat.rs`

[`66_plan_left_open_at_turn_end/`](../66_plan_left_open_at_turn_end/proposal.md)
edits the **turn end**: each `StopReason::Answered` break in
`run_agentic_turn` (the no-tool-call response and the empty-decode path), the
`step_evidence` buffer, `PendingChatTurn` (adding persisted step evidence), and
the audit records at turn end. Collisions with #65:

- **`PendingChatTurn` gains a field in both specs.** #66 adds step evidence,
  #65 adds notes and the call list; both must edit the same struct, the same
  six save sites and the same five resume sites. Land them sequentially, not in
  parallel.
- **The `Answered` exits** are where #66 adds its reminder. #65 does nothing at
  the exits except write the final `turn_notes` event (the write is on every
  change, so the last write is already the final state). Keep #65's per-change
  write away from the exit arms.
- **`bounded_messages`**: #66 mentions it when explaining the plan reminder
  being cut, but does not change it. No collision there.

The README's rule is binding: **one `chat.rs` implementation track at a time**
(`docs/specs/README.md` → "Parallel work"). #65 pairs with nothing that edits
`chat.rs`; #26, #66 and #65 are three `chat.rs` tracks and must run one after
another. A spec that does not edit `chat.rs` (for example a docs-only change)
may run beside #65.

### Ordering consequence

The plan orders the work so the `chat.rs`-light tasks land first. #26 landed on
2026-10-25 while Task 1–6 were in progress; §3 and §4 were re-read before Task
4, and Task 7 was re-checked against the post-#26 seed. The re-check found the
seed still two messages and `bounded_messages` unchanged.
