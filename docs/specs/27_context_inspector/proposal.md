# Feature Spec: Context Inspector

Status: Done 2026-10-26 (planned and built on 2026-10-26; Tasks 1–10; the
manual browser walk is recorded in §7, and **CI is outstanding** — the
seven-check gate runs after the user pushes). Split into a folder and planned
from [#26](../26_context_assembly/proposal.md)'s shipped manifest.
[`context.md`](context.md) records the pre-implementation code, the corrections
planning found, and the resolved §7 open question (whether separating assembly
from sending restructures the turn path); [`tasks.md`](tasks.md) carries the
Global Constraints, the task breakdown and the progress record.
Order: 27 of 27
Plan: `docs/PLAN/03_phase_3_code_understanding.md`, Phase 3, Work
Package 5 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#26](../26_context_assembly/proposal.md) (the context manifest it renders) —
built (Done 2026-10-25). Everything else named below is a cross-reference, not a
prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.1 (chat
interface), section 7.3 (path and secret policy), section 7.9 (context
assembly). Related implementation specs:
[`05_clickable_file_references.md`](../05_clickable_file_references.md)
(navigation to referenced content),
[`11_agents_md_support.md`](../11_agents_md_support.md),
[`20_working_modes/proposal.md`](../20_working_modes/proposal.md) (session-scoped state pattern),
[`26_context_assembly/proposal.md`](../26_context_assembly/proposal.md) (produces the manifest this
renders — this spec is unusable without it).

## 1. Motivation

"The user can see what context the agent uses" is one of the product's central
claims, and it is currently false. There is no view of assembled context, before
or after a call.

[Spec 26](../26_context_assembly/proposal.md) produces the data — a manifest of what was
included, what was excluded, and why. This work package makes it visible and
gives the user control over the optional parts. Shipping the assembly pipeline
without the inspector ships the mechanism without the accountability: better
context selection that the user still cannot see.

It also matters for what comes next. Phase 3b delivers persistent memory through
this same view, and memory the user cannot inspect is precisely the
hidden-instruction failure the product direction rejects. An inspector built
only for files would have to be rebuilt for memory; built for categories, it
takes memory as one more category.

The control half is where the value compounds. A user who can see that 40% of
the window went to a large generated file can pin the two files that matter and
get a better answer for less money — a judgement they can make instantly and the
retrieval heuristics cannot make at all.

## 2. Current State

Checked against the tree on 2026-10-26, after #26, #55 and #66 landed. `context.md`
§2 has the full file:line map and every correction; this section is the summary
the design depends on.

- **No context view exists**, pre-send or post-send. The only live trace is a
  transient `contextFiles` disclosure under an assistant message, and it does not
  survive a reload.
- **`ContextPlan` is assembled and sent in one call path.**
  `ContextManager::build_context` (`crates/workspace-engine/src/context_manager.rs:1057`)
  returns a `ContextPlan` (`context_manager.rs:576`) carrying `items`, `files` and
  now a `manifest: ContextManifest` (`context_manager.rs:450`). `ChatOrchestrator`
  calls it at `chat.rs:1157`, records `context_assembled`, renders the prompt at
  `chat.rs:1179`, and enters `run_agentic_turn` at `chat.rs:1222`. There is no
  pause or other intervention point between assembly and the first model call.
- **The manifest already carries what requirement 1 asks for, with two gaps.**
  It has the limit and its source, per-category allocation/use/count, every
  included ref by path/range/tokens/provenance/root, and every exclusion with a
  reason (`context_manager.rs:380-470`). It has **no conversation-summary row**:
  #55 sends the summary inside the conversation block and leaves
  `ContextCategory::ConversationSummary` unused. It also has **no symbol
  provenance counts** — only the weakest provenance of the one symbols item — so
  "12 symbols (4 heuristic)" cannot be rendered from it.
- **`ContextItem` and `LineRange` exist.** `ContextItem` has `category`, `kind`,
  `path`, `range`, `tokens`, `redaction_status`, `provenance` and `root_path`
  (`context_manager.rs:528`); `LineRange` is `file_access.rs:16`. No collection
  source produces a range yet (#26 §7), so every included item is whole-file
  today; the inspector must still render a range the day one appears.
- **`ExclusionReason` is incomplete for this spec.** It has `RestrictedPath`,
  `DeduplicatedIntoRange`, `CategoryBudgetExhausted`, `Truncated`, `TooLarge`
  and `Binary` (`context_manager.rs:420-440`). It has no `UserRemoved` (this
  spec's removal), no reason for "not in the session's selected paths" (the path
  restriction), and no reason for "a pinned file was deleted".
- **Per-session state has an established pattern.**
  `SessionStore::allow_browser_diagnostics_for_session` /
  `browser_diagnostics_allowed_for_session` (`session.rs:1063`/`:1079`) append one
  event and replay the log. [Spec 20](../20_working_modes/proposal.md) §5.4
  follows it for session mode (`session.rs:1100`/`:1129`); pins and path
  restrictions follow it here.
- **Clickable file references exist** ([spec 05](../05_clickable_file_references.md)).
  `/api/open-vscode-file` opens a path and optional line; the inspector's path
  buttons reuse it.
- **Nothing is pinnable.** `explicit_paths` is a `ContextRequest` field
  (`context_manager.rs:664`) supplied per call. The composer's existing "pinned"
  chips are `localStorage` state sent as `explicit_paths` on each request
  (`app.js:1859-1903`, `:11966`): a per-request attachment, not durable session
  state and not the `Pinned` category.
- **The turn path already has typed pauses.** `PendingChatTurn` (`chat.rs:8125`)
  persists a turn and `PausedFor` (`chat.rs:8002`) makes every resume refuse a
  pause it does not own. A pre-send review is a new kind of that mechanism, not a
  new mechanism.
- **The summary is sent outside the plan.** `build_model_prompt`
  (`chat.rs:8670`) renders the #55 summary inside `Recent conversation:` and
  already redacts it. Nothing records its size in the manifest.

## 3. Requirements

1. A pre-send and post-send view shows: instructions included; file paths and
   line ranges; search and symbol results; the conversation summary;
   approximate tokens by category; and what was excluded or truncated and why.
2. The user can pin a file or range; remove optional context; restrict the
   session to selected paths; clear pinned context; and open referenced
   content.
3. **Required safety instructions and the user's own messages cannot be
   removed.** The inspector controls optional context only.

## 4. Non-goals

- Changing what assembly selects. The inspector shows and constrains; the
  selection logic is [spec 26](../26_context_assembly/proposal.md).
- Editing context content. A user may include or exclude an item, and pin a
  range — not rewrite what a file says on the way to the model.
- Showing raw provider payloads or message JSON. The view is categories, paths,
  ranges, and token counts.
- A permanent per-repository context configuration. Pins and restrictions are
  session-scoped; repository-wide rules are `AGENTS.md`
  ([spec 11](../11_agents_md_support.md)) and `restricted_patterns`.
- Retroactively changing a sent turn. Post-send is a record.
- Memory inspection — Phase 3b, which adds a category to this view.
- Diffing manifests between turns.

## 5. Design

### 5.1 A pre-send gate

Requirement 1's pre-send view needs a point that does not currently exist:
`build_context` returns a plan and the caller sends it in the same path. Assembly
and sending must be separated.

```rust
/// Assembled and inspectable, not yet sent.
pub struct PreparedContext {
    pub manifest: ContextManifest,
    pub items: Vec<ContextItem>,
}
```

The turn becomes: assemble → `PreparedContext` → (optional inspection) → send.

Inspection is **opt-in and non-blocking by default**. A turn does not wait for
the user to approve its context; that would add a confirmation step to every
message and would be abandoned within a day. The default is that the view is
available — a control showing the token total, openable during or after the turn
— and the turn proceeds. A user who wants to inspect before every send can
enable a `pause_for_context_review` setting, off by default.

This is the design decision most likely to be got wrong in the other direction.
A pre-send view that blocks is technically closer to the requirement's wording
and worse as a product; the requirement is that the user *can* see before
sending, not that they must.

**How the two halves coexist.** Both are the same `ContextManifest`:

- **Without the pause**, the orchestrator emits the manifest as
  `TurnProgress::Context` immediately after assembly and before the first model
  call. The shell relays it as an SSE `context` event, so the header control and
  panel show what this turn is about to send while the turn runs. It is the same
  view the pause would show; the user simply cannot change it in flight.
- **With the pause**, the turn saves a `PendingChatTurn` with a new
  `PausedFor::ContextReview` and a `PendingContextReview` holding the prepared
  plan, the already-built request messages, the conversation block and the
  summary. `resume_after_context_decision` is the only route that may continue
  it: it refuses every other pause kind, exactly as the existing resumes do. A
  declined review fails the turn with no model call; an approved one enters the
  same `run_agentic_turn` a non-paused turn would have entered.

The pause is a new kind in the existing typed-pause machinery, not a new
mechanism. That is deliberate: `pending_commands` is a single directory keyed by
an id, and each resume already refuses the kinds it does not own (`chat.rs:8002`,
`paused_for`). A pre-send review that could be resumed by a command-decision
route would be an approval bypass wearing a review's clothes.

### 5.2 What the view shows

Rendered from `ContextManifest` ([spec 26](../26_context_assembly/proposal.md) §5.5), which
carries the limit, the categories, the included refs and the exclusion records.
Two fields #26 left for this spec are added to it:

- **the conversation summary**, as its own `ConversationSummaryUsage` row (token
  count, how many messages it covers, whether it is the facts-only fallback).
  #55 sends the summary inside `Recent conversation:` rather than as a
  `ContextItem`, so it is not an `IncludedRef`; the row is what makes its cost
  visible and what lets the paused review remove it for a turn.
- **symbol provenance counts**, as `total`, `language_server` and `heuristic` on
  the symbols block, because the one `IncludedRef` can only carry the weakest
  provenance among the lines shown.

The rendering:

```text
Context for this turn                    18,240 / 32,768 tokens  (declared limit)

  Required            412   user message, safety instructions        [locked]
  Instructions      1,180   AGENTS.md, crates/workspace-engine/AGENTS.md
  Pinned            2,940   src/upload.rs                            [unpin]
  Plan state          260   4 steps
  Repository map      880   3 roots
  Symbols           1,640   12 symbols  (4 heuristic)
  Search results    3,100   9 results
  File ranges       7,420   6 ranges in 4 files
  Findings            408   2 findings
  Summary               0   none

Excluded (11)
  .env                          restricted path
  src/generated/schema.rs       too large (1.2 MB)
  src/upload.rs:88-96           merged into src/upload.rs:80-140
  tests/legacy.rs               search results budget exhausted
```

Three things this rendering makes visible that nothing does today:

- **The excluded list**, which is the half users actually need. "Why didn't it
  read `upload.rs`?" is answerable.
- **The limit's source** — declared, observed, or default
  ([spec 26](../26_context_assembly/proposal.md) §5.7) — so conservative context is
  attributable rather than mysterious.
- **Heuristic counts**, carrying [spec 25](../25_symbol_and_relationship_index/proposal.md)'s
  provenance through to the surface. "12 symbols (4 heuristic)" tells the user
  how much of the structural information is a guess.

Every path is a clickable reference ([spec 05](../05_clickable_file_references.md)),
including in the excluded list — a restricted file should still be openable by
the user, who has every right to read their own file.

The view is built from the manifest, and the manifest holds **no content** by
construction ([spec 26](../26_context_assembly/proposal.md) §5.5). The prepared
`ContextItem`s stay in the engine and in the paused turn; the shell is never
sent them. A restricted path therefore appears as a path with a reason, never as
text, and the acceptance criterion "no content from a restricted or
secret-bearing file is shown" is true because no content crosses the boundary —
not because a renderer remembered to skip a field.

### 5.3 Post-send

The post-send view is the same rendering of the manifest that was actually used.
Requirement 1 distinguishes pre-send from post-send, and the distinction is real:
if the user removed an item, or a `Required` item forced a re-plan, the sent
manifest differs from the first assembled one.

The manifest is recorded per turn (already the case, via
`AuditLog::record`), so the post-send view is available for any past turn in the
session, not only the most recent. A user debugging a bad answer three turns ago
can see what that turn was given.

The reader takes the **newest `context_assembled` event per task**. That is what
makes "what was actually sent" true after a removal: the pause records the first
assembly, and a removal appends a second `context_assembled` with the amended
manifest before the resumed turn sends anything. An unchanged turn has exactly
one event. `/api/session` joins the newest manifest onto each task, and each
assistant message offers the panel for its own task.

Post-send is a record and is read-only.

### 5.4 Controls, and their persistence

| Control | Effect | Scope |
|---|---|---|
| Pin file or range | Enters the `Pinned` category on every subsequent turn | Session |
| Unpin / clear pins | Removes them | Session |
| Remove optional item | Excluded from this turn, reason `UserRemoved` | This turn |
| Restrict session to paths | Only these paths are eligible for context | Session |
| Open referenced content | Opens in app or configured editor | — |

Pins and restrictions persist as appended session events, following the
`browser_diagnostics_allowed_for_session` pattern (`session.rs:1063`) and
[spec 20](../20_working_modes/proposal.md) §5.4:

```json
{"seq":88,"eventType":"context_pin_added","sessionId":"session_...",
 "path":"src/upload.rs","range":{"start":40,"end":120}}
{"seq":89,"eventType":"context_pin_removed","sessionId":"session_...",
 "path":"src/upload.rs","range":{"start":40,"end":120}}
{"seq":90,"eventType":"context_pins_cleared","sessionId":"session_..."}
{"seq":91,"eventType":"context_restriction_set","sessionId":"session_...",
 "paths":["src/upload.rs","tests/upload.rs"]}
{"seq":92,"eventType":"context_restriction_cleared","sessionId":"session_..."}
```

Replayed newest-wins, read by parsed `eventType` rather than substring matching,
per [spec 17](../17_durable_task_state_and_crash_recovery/proposal.md) §5.2. A pin is
identified by `(path, range)` so removing one does not remove a second range of
the same file. Pins therefore survive a restart, which matters because a user
who pinned three files for a long task should not lose that to a crash.

**A restriction narrows; it never widens.** `path_policy.rs` and
`restricted_patterns` still apply on top, so a user cannot restrict *to* a
restricted path and thereby include it. The restriction is an intersection with
what policy already permits — stated explicitly because "restrict the session to
selected paths" could be read as an override, and it is not.

A path restriction filters the **file-bearing collection sources** — explicit
paths, prompt mentions, keyword and semantic hits, project rules, and pins. It
does not filter `Instructions`: the `AGENTS.md` hierarchy is the repository's
policy and `Required` content is the user's own message, and silently ignoring a
repository's rules because the user narrowed *content* would be a surprising and
unsafe reading of "restrict to these paths". Both are still listed in the panel,
and an instruction may still be removed for a turn. This is recorded again in
`context.md` §4 because the sentence "only these paths are eligible for context"
could be read more broadly.

A pinned file that later becomes restricted, or is deleted, is reported in the
excluded list with the reason rather than silently dropped from the pin list:
`RestrictedPath` for policy, `Missing` for a file that no longer reads. The pin
stays in the session state either way, so restoring the file restores the pin
without the user re-pinning it.

### 5.5 What cannot be removed

Requirement 3 is enforced by the type, not by the UI.
[Spec 26](../26_context_assembly/proposal.md) §5.1 defines `ContextCategory::Required` as the
first variant, holding safety instructions and the user's own message. The
inspector renders `Required` items as locked, and the removal path rejects a
`Required` item — so a UI bug, or a future caller, cannot remove one.

This is worth enforcing below the UI because the failure is silent and severe: a
turn that sent no user message, or dropped a safety instruction, would look
normal and behave badly. The acceptance criterion asserts the rejection at the
API, not the absence of a button.

`Instructions` (the `AGENTS.md` hierarchy) is *not* `Required` and may be
removed for a turn. That is deliberate: repository instructions are the user's
own content, and a user debugging whether an `AGENTS.md` rule is causing a
behaviour needs to be able to take it out. Precedence stays
[spec 11](../11_agents_md_support.md)'s; removal is a per-turn user action, not a
precedence change.

### 5.6 Surface

A context control in the conversation header showing the current turn's token
total, opening a panel with the §5.2 rendering. It shows the last assembled
manifest between turns, so it is useful outside an active turn. Each assistant
message whose task has a recorded manifest gets a *Context* link that opens the
same panel for that turn, which is how a past turn is inspected.

Category rows expand to their items. An item row offers remove (only while a
review is paused, and only if not `Required`) and pin. The excluded section is
collapsed by default and shows its count, since it is long and mostly
uninteresting until it is exactly what you need.

The panel's controls are separate from the composer's existing attachment chips:
those are sent as `explicit_paths` for the request being typed and live in
`localStorage`; these are session events and change every subsequent turn.
Labelling them differently ("Attached for this request" versus "Pinned for this
session") is part of the work, not a detail — two controls that look identical
and behave differently is worse than one.

Removal needs a paused review because that is the only point at which a turn has
been assembled and not sent. On a completed turn the panel is read-only, which is
the same rule as §5.3 and is enforced by the pause kind rather than by a disabled
button: `remove_context_item` refuses a proposal that is not a context review.

### 5.7 Documentation

`docs/USER_GUIDE.md`: how to read the panel, what each category is, what pinning
and restricting do and how long they last, why some items are locked, and how to
enable pause-before-send. `docs/TROUBLESHOOTING.md`: using the excluded list to
diagnose why a file was not consulted, what each exclusion reason means, and how
to inspect a past turn's context.

## 6. Acceptance Criteria

- Before sending, the user can see every file path and line range that will be
  sent, the token count per category, and the effective limit with its source.
- The excluded list shows every omitted item with its reason, including
  restricted paths, oversized files, deduplicated ranges, and budget
  exhaustion.
- Symbol counts distinguish heuristic from language-server provenance.
- Every path in the view, including in the excluded list, is a clickable
  reference that opens the file.
- Removing an optional item measurably reduces what is sent, and the item
  appears in the manifest's excluded list as `UserRemoved`.
- A `Required` item cannot be removed: the removal path rejects it — asserted at
  the API, not by the absence of a UI control.
- An `Instructions` item can be removed for a turn without altering
  [spec 11](../11_agents_md_support.md) precedence.
- Pinning a file or range includes it on subsequent turns, and pins survive a
  restart.
- Clearing pins removes them from subsequent turns.
- A session restriction narrows eligible context and cannot include a path that
  `path_policy.rs` or `restricted_patterns` excludes — asserted by test.
- A pinned file that becomes restricted or is deleted appears in the excluded
  list with the reason, rather than disappearing.
- Post-send, the view reflects what was actually sent, not what was first
  planned, and is read-only.
- A past turn's context is inspectable, not only the most recent.
- Inspection does not block a turn unless `pause_for_context_review` is enabled,
  which is off by default.
- No content from a restricted or secret-bearing file is shown in the panel —
  the view renders paths, ranges, and counts from the manifest, never content.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

Implemented on 2026-10-26 in ten tasks. [`context.md`](context.md) is the
planning record; this section is what shipped and what was measured.

### Separating assembly from sending (§5.1)

**Yes, it required restructuring the turn path in `chat.rs` — as one new pause
kind, not a new pipeline.** The answer in full is `context.md` §4. In summary:

- `ContextPlan` already carried everything the prompt needs, so assembly stayed
  where it was. `PreparedContext { manifest, items }` is a lossless view of it;
  `files` and `sections` are derived, and `into_plan` reconstructs the plan.
- `build_model_prompt` was split without changing its output:
  `build_conversation` builds the summary and recent-message block, and
  `build_model_prompt_with_conversation` joins it with `plan.sections()`. The
  original function remains the entry point for every existing caller, and
  `prompt_cache`'s byte-identical determinism guard still passes.
- `PausedFor::ContextReview` was added to the existing exhaustive pause
  machinery. Every other resume refuses it and it refuses every other kind; the
  cross-resume tests cover both directions. `resume_after_context_decision`
  sends the amended plan through the same `run_agentic_turn` a non-paused turn
  uses.
- What else the work touched: `ChatTurnResult` (one field), `PendingChatTurn`
  (one field and nine literals), `TurnProgress`/`TurnEvent` (one boxed
  manifest), the shell's result and session JSON, five routes, the panel and
  review card, and recovery's reattachment table. It did not touch
  `run_agentic_turn`'s loop, tool dispatch, round budgets or the model adapter.
- **A declined review makes no model call.** The shell's decline path uses a
  no-op adapter, so saying "do not send this" does not require a provider key.

### The manual walk (2026-10-26)

The web UI was served against a throwaway `DAMAIAN_DATA_DIR` and the inspection
fixture repository on **port 4899**, with
`DAMAIAN_MOCK_MODEL_RESPONSE="Mock answer from the walk."`. A plain browser has
no Tauri bootstrap, so the devtools console set
`apiToken = 'damaian-ui-inspection-token'` and then ran
`setRepository('<repo>', false)`.

What was walked, and what was seen:

1. **A paused first turn.** `pause_for_context_review=true`; a prompt
   mentioning `retry.rs` paused with `contextreview_…`, no model call, and a
   card reading *Review the context before sending · 74 / 16.0K tokens*. The
   panel showed `Required 4`, `Repository map 63`, `File ranges 7`, every other
   category at `0`, and `Excluded (0)`. The limit read `declared`.
2. **`Required` is locked at the API.** A direct `POST /api/context-remove` at
   the Required index returned `Required context cannot be removed`; the item
   stayed in the manifest and the turn stayed resumable.
3. **Removing an optional item.** Removing the `File ranges` item took the total
   from **74 to 67** tokens and put `retry.rs · removed by you` under
   `Excluded (1)`.
4. **Send and post-send.** *Send* completed the task with the amended manifest.
   A reload showed a **Context · 67 tokens** link on the assistant message; it
   opened the read-only panel with the same total and the `user_removed`
   exclusion. This is the post-send view, and it is the amended manifest rather
   than the first assembly.
5. **Pins and the restriction.** A ranged pin (`retry.rs:1-3`) and a restriction
   to `retry.rs` were applied, shown in the panel, and cleared; `/api/session`
   reported both between the two requests. A prompt after the pin showed the
   file in the **Pinned** category.
6. **Past turns.** Both turns' manifests were present in `/api/session`, each on
   its own task, which is what the per-message link reads.
7. **Not walked in the browser:** the Tauri app's own window (its platform
   chrome and folder picker are outside this view), and opening a path in Visual
   Studio Code (the button is present and calls `/api/open-vscode-file`; the
   environment has no editor to open). The path button was exercised through the
   route in the handler tests instead.

### The two product questions the spec asked

- **Do users pin or remove more often?** **Not measured.** One manual walk is
  not usage data, and no live session corpus was read. The controls are
  instrumented by their audit events (`context_item_removed`,
  `context_summary_removed`, and the `context_pin_added` family), so a later
  session can answer this from the log rather than from a guess. Nothing in the
  design depends on the answer yet.
- **Was `pause_for_context_review` used after the first day?** **Not measured.**
  It remains off by default, which is the design decision §5.1 records; no
  evidence has arrived to reopen it, and none is claimed.

### What the manifest gained, and what that corrected

- `conversationSummary` (tokens, messages, span, facts-only) was missing because
  #55 sends the summary in the conversation block rather than as an item.
- `symbolProvenance` (`total`/`languageServer`/`heuristic`) was missing because a
  single `IncludedRef` can only carry the weakest provenance.
- `ExclusionReason` gained `user_removed`, `path_not_selected` and `missing`, the
  three facts this spec's controls can produce.
- **The first production line-range producer is this spec's pins.** #26's §7
  said no collection source yields a `LineRange`; a pinned range does, and #26's
  note and test name were corrected on the same day. Automatic collection is
  still whole-file.
- The session restriction filters file-bearing collection only; `AGENTS.md` and
  `Required` are exempt (`context.md` C10). That was a planning decision, not an
  oversight, and the user guide states it.

### Known gaps

- **No live model was called with a real provider.** The walk used
  `DAMAIAN_MOCK_MODEL_RESPONSE`; the engine's pause, removal, send and
  persistence paths are covered by tests, but a live provider adds no different
  code path to this view.
- **`context_summary_removed` is tested at the API and the manifest level**
  (`remove_summary` and the resumed `without_summary` block), not through a
  whole turn with an active #55 summary. Building a live summary in a fixture
  costs a model call and the renderer path is shared with the item removal that
  is tested end to end. This is a coverage gap, not an unbuilt control.
- **Composer chips are still per-request attachments.** They are deliberately
  not session pins; the panel labels the two differently. Unifying them is a
  later product decision, not a missing piece of this spec.
- **CI is outstanding.** The seven-check gate runs after the user pushes; scoped
  local checks are recorded in [`tasks.md`](tasks.md)'s Task 10 row.
