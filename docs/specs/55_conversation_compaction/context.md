# Conversation Compaction — Context

Pre-implementation state, checked against the tree at `82858d0` on 2026-10-26,
with every claim below verified in the file and line it names. The proposal is
the decision; this file records what the code actually did before the work and
where the proposal's own text is now stale.

## 1. Why this is being built

`build_model_prompt` sent the model the **last eight messages, each cut to
2,000 characters**:

```rust
// crates/workspace-engine/src/chat.rs:8399-8407
let recent_messages = prior_messages
    .iter()
    .filter(|message| message.kind.as_deref() != Some("plan_reminder"))
    .rev()
    .take(8)
    ...
// :8420
output.push_str(&truncate_for_prompt(&redacted, 2_000));
```

Neither number is configurable, and nothing told the user or the model that
anything had been dropped. A constraint in the first message was gone at the
ninth, with no summary and no marker. That is the defect: not unbounded growth —
which the provider would have reported as an error — but silent loss that gets
harder to notice the longer a session runs.

## 2. Pre-implementation state

- **History is one prose block inside the first user message.** A turn starts as
  `[system, user(model_prompt)]` (`chat.rs:1152-1160`), and `model_prompt`
  contains the repository context, the recent conversation, and the user
  request in that order (`chat.rs:8382-8433`). Within a turn the array grows
  into real role-tagged messages.
- **The section order is now stable-to-volatile.** `prompt section ordering` was
  still described as a defect in the proposal, but [#26](../26_context_assembly/proposal.md)
  landed on 2026-10-25 and reordered it: `RepositoryContext` first,
  `RecentConversation` second, `UserRequest` last (`build_model_prompt`, and
  `PlanSectionName`/`PromptSectionName` in `context_manager.rs`). **No ordering
  work is in scope for #55**; #49's reuse slice consumes this order.
- **The context budget exists.** `Config::context_token_budget()`
  (`config.rs:2033-2042`) resolves per provider and model, defaulting to
  `DEFAULT_CONTEXT_TOKEN_BUDGET = 16_000` (`config.rs:16`). #26's per-category
  `ContextBudget` exists with shares in basis points, and
  `ContextCategory::ConversationSummary` is reserved and always allocated zero
  items (`context_manager.rs:52,68,84,100`; `config.rs:1741`). Nothing populates
  it.
- **Repository context does not accumulate.** `ContextManager::build_context`
  rebuilds a `ContextPlan` per turn against the budget.
- **Messages are stored faithfully and unredacted.** `SessionStore::append_message`
  → `append_message_with_kind` (`session.rs:557`, `:572`) writes `content`
  verbatim. `read_messages` (`session.rs:654`) returns every active message;
  `read_messages_with_seq` (`:670`) returns them with their event `seq`.
- **The log is append-only with a `seq`.** `append_session_event`
  (`session.rs:2549`) stamps `next_seq`; `active_events` (~`:2720`) applies the
  newest `conversation_rewound`, and `rewind_conversation` (`:2535`) appends a
  marker rather than rewriting.
- **Plan state persists.** `read_session_plans` (`session.rs:1762`) replays
  `plan_created`/`plan_revised`/`plan_resumed` and `plan_step_updated`; each
  step carries status and evidence (`plan.rs:109-123`).
- **Patch decisions persist.** `patch_file_decided` events and
  `read_all_patch_file_decisions` (`session.rs:1880`) record which files an
  applied or rejected patch touched (`PatchFileDecision`, `session.rs:71-79`).
  `plan_approved` records plan approval (`session.rs:2033`, `:2109`).
- **Command decisions do not persist in the session log.** Approve/reject is
  recorded in the audit log (`validation.rs:415`, `:531`), which carries no
  session id and has no read API. The session log therefore cannot today answer
  "which commands were approved in this conversation" on its own.
- **Validation facts persist.** `read_verification_events` (`session.rs:1931`)
  replays per-check outcomes (`VerificationEvent::Check`, `verification.rs:164`)
  and `read_findings` (`session.rs:2005`) holds structured findings.
- **In-flight actions are discoverable.** `dangling_actions`
  (`session.rs:2197`) returns a `DanglingAction` with the start `seq`
  (`session.rs:352-368`), which is the boundary a compaction must not cross.
- **Pending decisions are discoverable.** `pending_approval_for`
  (`session.rs:986`) and `pending_decision_for` (`:1044`) read the waiting
  state and its `PendingApprovalRef`.
- **Model calls are accounted per task.** `record_task_usage` /
  `record_task_usage_for_task_id` (`session.rs:1269`, `:1292`) write
  `task_usage_recorded`; `read_task_usage` (`:1351`) sums by task. The per-turn
  ceiling reads the current task's total (`token_ceiling_spend`, `chat.rs:3986`).
- **No summarisation exists**, and no second model call is made outside the turn
  loop. `ModelAdapter` has exactly one method pair
  (`stream_response`/`estimate_tokens`, `model.rs:345-356`).
- **The transformer is content-aware.** `TokenEstimate` (`context_manager.rs:320-371`)
  exposes `Prose.tokens(text)` in integer arithmetic; a conversation message is
  prose.
- **`ChatMessage` already carries a `kind`** (`session.rs:246-259`), which
  `build_model_prompt` uses to skip spec 66's `plan_reminder`.
- **The session view is assembled in the shell.** `handle_session`
  (`desktop-shell/src/lib.rs:546`) reads display messages with
  `read_messages_with_seq` and serialises them through `message_json`
  (`lib.rs:5009`); `renderMessages` (`app.js:4679`) draws them.
- **Settings › General already has a checkbox pattern that writes one key**:
  `worker-sessions-input` reads the effective value from `/api/workers-enabled`
  and writes through the config editor (`app.js:5559-5592`,
  `index.html:435-444`). `POST /api/config-set` exists
  (`lib.rs:1017`) and is limited to the desktop user config path.
- **The eval harness drives one `ask` per scenario.** `drive`
  (`eval-harness/src/runner.rs:383+`) calls
  `ask_with_session_with_options` once; `[[turn]]` entries are model responses
  within that turn, not user turns. There is no seeding of a prior
  conversation, so a crossing-turn compaction scenario needs a harness feature.

## 3. Corrections to the proposal

The proposal was written on 2026-10-25 before #26 landed. These statements are
now wrong or incomplete and the implementation follows the code:

1. **"The ordering in that string is the opposite of cache-friendly"** (§2,
   §5.1's last note) — already fixed by #26's stable-to-volatile order. #55 does
   not reorder anything.
2. **"Spec 26 is `Not started`, so the budget it will own does not exist yet"**
   (§5.1) — #26 is Done. Its category budget exists, but it **created no
   conversation-window category**; `ConversationSummary` is reserved and has no
   producer. The proposal's promise that "the window becomes one of its
   categories" therefore cannot be kept without moving conversation messages
   into `ContextManager`, which is not where the request's conversation block is
   assembled. #55 keeps the window as a share of `context_token_budget` and adds
   its own `conversation_window_share` key (see D2).
3. **"Approval decisions | Session log"** (§5.2) — true for patches and plan
   approval, false for commands. #55 records a `conversation_approval_recorded`
   session event at the two command decision points and reads all three sources;
   without it the digest would silently omit every command approval.
4. **"Spec 30's injection-resistance pattern"** (§5.6) — #30 is `Not started`,
   so there is no implemented pattern to reuse. Its design rule is followed
   directly: the summary is delimited, labelled as a record, never merged into
   the system prompt, and never placed where repository instructions live.
5. **"The eight-message window"** is the only thing replaced. The 2,000-character
   cut inside a kept message stays, as the proposal says.
6. **`prior_messages` is read before the new user message is appended**
   (`chat.rs:970` vs `:1035`), which is the natural place to decide what is
   compactable: the current request can never be inside the summarised span.
7. **The eval harness cannot build a long conversation through `[[turn]]`
   entries.** The implementation adds a `seed_conversation` scenario field and a
   request-level assertion; see D9.

## 4. Decisions the implementation makes

**D1 — Compaction is a turn-boundary operation, never mid-turn.** It runs in
`ask_with_session_with_options` after the session and its prior messages are
read and before the request is built, and in the public
`ChatOrchestrator::compact_conversation` used by the manual control. It is
never called from `run_agentic_turn` or between tool rounds.

**D2 — One window number, two uses.** `conversation_window_share` (default
`0.25`, stored as basis points) is both the recency window's size as a share of
`context_token_budget` and the automatic threshold: a projected conversation
above the window triggers compaction, and the result fits the window. The
proposal's separate "threshold" and "window" are the same quantity; splitting
them would let the threshold trigger a compaction that cannot change anything.

**D3 — Auto-compaction is a config key, Forbidden to repository and profile
config.** `conversation_auto_compact` defaults to `true`. The window share is
restrict-only lower-wins: a repository may send less of the user's conversation,
never more.

**D4 — The facts half is read from the log; only narrative is model-authored.**
Files come from patch decisions, plan steps from `read_session_plans`,
approvals from patch decisions, `plan_approved` and the new command-decision
event, validation from verification events and findings, and the objective from
the first user message *in the covered span*, quoted and bounded. The
summarisation prompt is told not to write those fields; the parser only accepts
the four narrative fields.

**D5 — The summarisation call is accounted to a synthetic task id.**
`record_task_usage_for_task_id(session_id, "compaction_<summary id>", run_id, …,
Some("conversation_compaction"))`. It appears in the session's totals and in no
turn's ceiling, which is exactly §5.5's rule without teaching the ceiling to
subtract a special case.

**D6 — Failure is not fatal.** Any error from the model call, or a response
whose narrative cannot be parsed, still produces a **facts-only** summary and
appends the event. Only a failure to read the facts or to append the event
aborts compaction, and that aborts only the compaction: the turn continues and
an audit event `conversation_compaction_failed` names the reason. Never is a
turn failed because a summary could not be produced.

**D7 — Guard rails are hard stops, not warnings.** `through_seq` is clamped to
below the earliest start `seq` of any dangling action and below the first
message belonging to a task with a pending approval or question. A compaction
that would cover no message is a no-op. The current request is never covered
because compaction reads messages before the new one is appended.

**D8 — The summary is rendered as delimited data inside the conversation block.**
`build_model_prompt` gains one optional summary argument and writes it under
`Recent conversation:` before the window, between explicit
`--- begin prior conversation summary (recorded data) ---` /
`--- end prior conversation summary ---` markers, with every model-authored
line quoted. It never enters the system prompt.

**D9 — The eval harness gains a `seed_conversation` field and a request
assertion.** Scenarios can seed prior user/assistant messages, and a
`compacted_request_contains` / `compacted_request_excludes` assertion reads the
first request the mock adapter recorded after a `conversation_compacted` event.
The deterministic tier therefore measures what the model was actually sent, not
what a script says it was.

**D10 — The UI shows the boundary from the session, not from a second store.**
`/api/session` gains a `compactions` array (summary, `throughSeq`, message
count) and `conversation` boundary metadata. `renderMessages` inserts an
expandable marker before the first message after the covered span; the original
messages remain rendered below it. Manual compaction is a button on the marker
and in the session row menu.

## 5. File map

| File | Role |
|---|---|
| `crates/workspace-engine/src/compaction.rs` | New: summary types, rendering, narrative prompt/parser, fact extraction, window selection |
| `crates/workspace-engine/src/session.rs` | `conversation_compacted` append/read, model-view reader, command-decision event, boundary read |
| `crates/workspace-engine/src/config.rs` | Two new keys, overlay classification, policy text |
| `crates/workspace-engine/src/profile.rs`, `effective_policy.rs` | Key classification and labels |
| `crates/workspace-engine/src/chat.rs` | Turn-boundary trigger, summarisation call, `build_model_prompt` window + summary |
| `crates/workspace-engine/src/lib.rs` | Module and re-exports |
| `crates/desktop-shell/src/lib.rs` | `/api/session` compaction data, `POST /api/compact` |
| `crates/desktop-shell/static/index.html`, `app.js` | Boundary marker, manual control, Settings checkbox |
| `crates/eval-harness/*` | Seeded conversation, request assertions, two scenarios |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | User-visible behaviour |

## 6. What later tasks need

- The `PendingChatTurn` literals and the turn loop are untouched, except for
  the new call before `build_context`; #35's rebase should be mechanical.
- `build_model_prompt` is the only shared function changed, and its signature
  change is confined to the function, its two call sites in `chat.rs`, and two
  tests that call it directly.
- The summary format is versioned; a reader ignores unknown versions rather
  than failing, and a format change takes effect at the next compaction.
- No stored message is rewritten or deleted. `read_messages` is unchanged, so
  spec 16's rewind and spec 53's export see the same transcript.
