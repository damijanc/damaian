# Context: Context Inspector

Background for [`tasks.md`](tasks.md). Read this before Task 1, with
[`proposal.md`](proposal.md) §2 beside it. Everything below was checked against
the tree on 2026-10-26, after #26, #55 and #66 landed. Line numbers drift:
re-read the function before editing near it.

## 1. What exists, and what does not

The data exists. #26 built a sanitized `ContextManifest` on every assembly and
records it as `context_assembled`. Nothing reads it back, and the shell has never
seen it.

The controls do not exist. `explicit_paths` is a per-request argument; the
composer's "pinned" chips are `localStorage` state sent as that argument
(`app.js:1859–1903`, `:11966`), so they are neither session-scoped nor the
`Pinned` category. There is no session restriction of any kind.

The pre-send point does not exist. Assembly and sending are still adjacent
statements in `ask_with_session_with_options`; the only pause machinery around
them is for actions, plans, questions, checks, fallbacks and round budgets.

## 2. The pipeline as it is today

### Chat turn

`ChatOrchestrator::ask_with_session_with_options` (`chat.rs:943`) does, in order:

1. read the index and resolve the session (`:963–1005`);
2. create the task, set `PreparingContext`, carry a plan and notes
   (`:1007–1045`);
3. compact the conversation if the #55 window is exceeded (`:1052–1079`);
4. append the user message and take the turn checkpoint (`:1081–1091`);
5. dispatch `before_context` and collect `hook_context` (`:1099–1155`);
6. **assemble**: `build_context(ContextRequest { … })` at `:1157`;
7. record `context_assembled` from `context.manifest` (`:1168–1178`);
8. **render**: `build_model_prompt(&context, …)` at `:1179`;
9. build `[system, user]` messages and call `run_agentic_turn` (`:1190–1222`).

There is no pause between 8 and 9, and a paused turn resumes straight into the
loop with the saved `messages`.

### One-shot edit

`EditOrchestrator::propose_edit` (`edit.rs:412`) creates its **own** session,
dispatches `before_context`, assembles through the same manager
(`edit.rs:509`), and renders through `build_edit_prompt` (`edit.rs:1086`). The
origin chat session's id is available at `:424` (`origin_session_id`), which is
where a session restriction and session pins have to be read if the edit flow is
to honour them.

### What the manifest already is

`ContextManifest` (`context_manager.rs:495`) is:

```
repository_id, task_id, assembled_at_ms, limit, total_tokens,
categories, included, excluded
```

- `EffectiveLimit` (`:191`) has `tokens`, `source` (`Observed`/`Declared`),
  `safety_margin_bps` and `window`.
- `CategoryUsage` (`:230`) has `category`, `allocated`, `used`, `items`,
  `truncated`, `estimated`.
- `IncludedRef` (`:460`) has `category`, `kind`, `path`, `range`, `tokens`,
  `root_path`, `provenance`.
- `ExclusionRecord` (`:444`) has `reason`, `path`, `range`, `category`, `detail`.

It holds no content, by construction and by test
(`no_manifest_field_contains_item_content`, `tests/context_assembly.rs:1741`).

### What the manifest does not have

- **No conversation-summary row.** `ContextCategory::ConversationSummary` exists
  (`context_manager.rs:47`) and is allocated a share, but no item is ever
  created for it. #55 sends the summary inside the `Recent conversation:` block
  (`chat.rs:8703–8717`), outside the plan.
- **No symbol provenance counts.** The symbols block produces one `IncludedRef`
  whose `provenance` is the *weakest* provenance among the lines shown
  (`context_manager.rs:1505`, `render_block`). "12 symbols (4 heuristic)"
  is not derivable from that.
- **No reason for a user removal, a path outside the session restriction, or a
  deleted pin.** `ExclusionReason` (`:423`) has six variants and none fits.

## 3. Corrections to the proposal

Numbered so a task can cite one. Each was checked against the code on
2026-10-26.

| # | Proposal said | Code says |
|---|---|---|
| C1 | `build_context` is at `context_manager.rs:66` | `:1057`. The manager grew: categories, budgets, ranges, the manifest and two block renderers. |
| C2 | `ContextPlan` is `{repository_id, task_id, token_estimate, items, files}` and carries no exclusion/provenance | It also carries `manifest: ContextManifest` (`:576`). The proposal's statement was true before #26; it is false now. |
| C3 | `ContextItem` is at `context_manager.rs:22-29` and has no line range | `:528`, and it has `category`, `range`, `provenance` and `root_path`. |
| C4 | Per-session state is at `session.rs:261-291` | `allow_browser_diagnostics_for_session` is `:1063` and its reader `:1079`; `set_session_mode` is `:1100` and `session_mode` `:1129`. The pattern is unchanged. |
| C5 | `explicit_paths` is a `build_context` parameter at `context_manager.rs:73` | It is a field of `ContextRequest` (`:664`) with nine sibling fields. Adding pins and a restriction there is the seam #26 left on purpose. |
| C6 | The view is rendered "from `ContextManifest`, which already carries every field requirement 1 asks for" | Two fields are missing: the conversation summary (not an item at all) and symbol provenance counts. §5.2 of the proposal now adds both to the manifest. |
| C7 | `ExclusionReason` covers the acceptance list | It does not cover `UserRemoved`, a path outside a session restriction, or a deleted pinned file. The first two are this spec's own controls; the third is required by its acceptance criteria. |
| C8 | The pre-send/mid-turn state can live in a `PreparedContext` alone | A pause must also carry the request messages, the conversation block and the summary, because a removal has to re-render the user message before sending. `PreparedContext` is the plan half; `PendingContextReview` is the persistence half (§4). |
| C9 | The pre-send view is one thing | It is the same manifest on two paths: a live `context` event when the pause is off, and a paused review card when it is on. Building only one would leave either the default or the requirement unmet. |
| C10 | "Restrict the session to selected paths" | The sentence is broader than the safe reading. This spec restricts **file-bearing collection** (explicit, mention, keyword/semantic hits, project rules, pins) and leaves `Instructions` and `Required` alone. See §5. |
| C11 | Every path in the view is clickable, including a restricted one | `/api/open-vscode-file` (`lib.rs:977`) opens any path by name and line. The existing `/api/context-file` route validates a path *for context*, which is the wrong gate for a restricted path the user wants to read. Use the open route. |
| C12 | `PendingChatTurn` is a command-approval payload | It is the one persisted shape for every pause kind (`chat.rs:8125`); with this spec there are eight, including `ContextReview`, and `paused_for` (`:8055`) is the exhaustive tuple match that keeps each resume in its lane. A new field there is a compile-time decision. |

## 4. The resolved §7 open question: does separating assembly from sending restructure `chat.rs`?

**Yes, and the shape is a pause, not a pipeline rewrite.** The answer the
implementation takes:

1. **Assembly stays where it is.** `build_context` already returns a `ContextPlan`
   containing everything needed to render the prompt. There is no reason to move
   it or to introduce a new orchestrator stage.
2. **`PreparedContext` is a view of the plan, not a parallel pipeline.** It is
   `{ manifest, items }`; its `files()` and `sections()` are derived from
   `items`, and a `ContextPlan` converts to it losslessly. Keeping a second,
   divergently-updated prompt path is the failure mode to avoid.
3. **The pause is a new `PausedFor::ContextReview`.** `ask_with_session_with_options`
   assembles, records, emits the manifest and renders exactly as today. Only
   then, if `config.pause_for_context_review` is true, it saves a
   `PendingChatTurn` and returns a `ChatTurnResult` with `context_review: Some(…)`.
   With the setting off, it calls `run_agentic_turn` at the same point it always
   did. The default path gains one `if`, one progress event and one audit record;
   it does not gain a state transition.
4. **`build_model_prompt` stays intact.** The pause needs to re-render the user
   message after a removal, so the function is split *without changing its
   output*: a helper builds the conversation block, and a second helper joins
   `plan.sections()` with that block. `build_model_prompt` remains the
   `prior_messages + summary + window + scanner` entry point every existing
   caller uses. The paused resume uses the second helper with the saved block.
   This is the smallest split that lets a removal change the request without
   re-reading the conversation.
5. **What else it touches.** `ChatTurnResult` (one new field), the `done`
   serialisation in the shell (one new key), `PendingChatTurn` (one new field and
   one new literal in all seven existing constructors — the compiler finds them),
   `PausedFor`/`paused_for` (one variant and one tuple position),
   `TurnProgress`/`TurnEvent` (one boxed manifest variant), and the tests that
   assert the pause matrix. It does **not** touch `run_agentic_turn`'s loop,
   tool dispatch, the round budgets or the model adapter.
6. **Why not always pause?** The product decision in §5.1 is explicit: a
   confirmation on every message is abandoned within a day. The default path is
   a live view, not a wait. The setting is user config, forbidden to repository
   and profile scope (§6).

The one thing the pause cannot do is let a pin affect the turn already
assembled: the plan was built before the review. The panel says "applies to
later turns" and the current turn's manifest stays truthful.

## 5. Decisions this spec takes

- **Pins are session events, not `localStorage`.** They are read once per turn,
  before assembly, and enter the `Pinned` category. The existing composer chips
  stay exactly as they are (per-request `explicit_paths`) and are relabelled to
  distinguish them.
- **Pins are `(path, range?)`.** A whole-file pin is `range: None`; a ranged pin
  reads through `ReadWindow::ContextRange`, which #26 added for assembly ranges.
  The pin store keeps insertion order and deduplicates an identical pair.
- **A pin is collected through the same `add_file` path as every other file**, so
  the collection-time `PathPolicy` check applies and a restricted pin becomes a
  `RestrictedPath` exclusion with category `Pinned`. A read that fails is
  recorded as `Missing`. The pin list itself is never mutated by a failing read.
- **The restriction is an intersection, and it is read at assembly.** It filters
  explicit paths, mentions, keyword and semantic hits, project rules and pins.
  `Instructions` (the `AGENTS.md` hierarchy) and `Required` are not filtered
  (C10). The filter runs after normalisation, so `./src/x` and `src/x` cannot
  disagree.
- **Removal is only available on a paused review.** `remove_context_item` loads
  the pending turn, refuses anything that is not `PausedFor::ContextReview`, and
  then refuses a `Required` item. It removes the item, appends a `UserRemoved`
  exclusion, recomputes category use, `total_tokens` and `files`, saves the
  pending turn again, appends a new `context_assembled` event, and returns the
  amended manifest. `Instructions` removal is therefore a per-turn action with no
  effect on #11's precedence.
- **The summary is removable, but it is not an item.** `context_remove_summary`
  sets a flag on the pending review; the resumed request passes `summary: None`
  and the conversation block states that the user removed a summary covering N
  messages. The manifest records `UserRemoved` in the `ConversationSummary`
  category.
- **The manifest crosses the wire, items never do.** The paused result and the
  `context` event carry `ContextManifest`; `PreparedContext.items` stays in the
  pending file on disk. This is the structural half of the "no content in the
  panel" criterion.
- **The manifest is extended, not forked.** `ConversationSummaryUsage` and
  `SymbolProvenanceCounts` join `ContextManifest`; `ExclusionReason` gains
  `UserRemoved`, `PathNotSelected` and `Missing`. Existing serialisation keeps
  every old field name.

## 6. Config: the one new key, and its scope

`pause_for_context_review` is a user preference, default `false`, and it is
**forbidden** to repository and profile config. It decides whether every turn
waits for a human, which is neither a restriction nor a repository's business.
The overlay classification is exhaustive (`classify_overlay_fields!`,
`config.rs:3009`), the scoped application uses `scoped(..., forbidden, ...)`
(`:1205`), and `split_profile_keys` (`profile.rs:333`) must list it as
forbidden too or the two statements about profile scope disagree.

`rule_label` (`effective_policy.rs:130`) gets a human name so Settings' policy
view does not show a bare key.

The existing context-budget keys are untouched; the restriction itself is
session state, not config. A repository cannot restrict the session either — the
control exists only in the inspector.

## 7. What other specs rely on here

- **#28 memory** puts `PreparedContext` in its §5.6 and expects the send path to
  re-check entry state immediately before the call. This spec's pause is that
  seam: a memory entry present at assembly but deleted while the review is open
  must be re-checked on resume. Task 5 records the hook point even though there
  is no memory source yet.
- **#30 memory retrieval** expects memory to be visible and removable in this
  view. The `ConversationSummary` row is the precedent: a category can be visible
  and removable without being a file item.
- **#32 hooks** records its inspector criterion as met only as a bounded
  `hook_context` item "until #27". With this spec, `HookContext` is a first-class
  category row with its own items and exclusions; #32's §7.1 can be closed.
- **#54 image input** and **#51 external retrieval** name this view as where
  their content is seen. Neither category exists yet; the view renders whatever
  categories the manifest has, so adding one does not need a second panel.
- **#55 compaction** already records the summary in `contextView`; this spec adds
  its cost to the manifest and a removal control, not a second summary.
- **#49 prompt cache** owns `build_model_prompt`'s section boundaries. This spec
  keeps them byte-identical and adds no breakpoint.

## 8. Tests and helpers that already exist

| Where | What it pins |
|---|---|
| `tests/context_assembly.rs` | The manifest shape, every exclusion reason reachable today, no content in the manifest, category budgeting, range merging, `context_assembled` |
| `tests/foundation.rs:55` `test_config` | `data_dir` inside the fixture and `enable_index_watcher: false` |
| `tests/foundation.rs` `MockModelAdapter` | `requests` records every round, which is how "nothing was sent" and "the removed item is absent" are asserted |
| `tests/prompt_cache.rs` | Two assemblies of an unchanged repository produce byte-identical requests — the determinism guard this spec must not break |
| `tests/permission_profiles.rs` | `every_overlay_field_is_classified_exactly_once`, `the_keys_a_profile_may_carry_agree_with_the_profile_scope_merge`, `weakening_cases` |
| `tests/session.rs` / `tests/round_budget.rs` | Pending-turn serialisation and the old-file load rule |
| `desktop-shell/src/lib.rs` tests | `post_form_for_test`, `get_for_test`, `json_of`, the SSE turn harness |

New engine tests belong in `crates/workspace-engine/tests/context_inspector.rs`.
Shell route tests belong beside the existing handle tests in
`crates/desktop-shell/src/lib.rs`.

## 9. Parallel work and shared files

This is a **solo `chat.rs` track**. It edits `chat.rs` in Tasks 5–6 (the turn
path, `PendingChatTurn`, `build_model_prompt`'s split, the pause and resume). No
other spec may run while those tasks are in flight. The README says #49's reuse
slice, #55, #65 and #66 are the `chat.rs` cluster; #55, #65 and #66 are Done, and
#49's reuse slice is not in the ready set, so #27 has the file to itself once it
starts. It also edits `context_manager.rs`, which #26 finished and nothing else
owns, plus `session.rs` (events), `config.rs`/`profile.rs`/
`effective_policy.rs` (one key) and the shell (`lib.rs`, `app.js`, `index.html`,
`style.css`). #35 and #36 also touch the shell; that is a rebase, not a blocker,
because they do not touch the context routes.

## 10. The manual browser walk

The web UI in a plain browser has no Tauri bootstrap, so `apiToken` is empty and
`setRepository` is not called. The recipe that works, and that the close-out
task records in `tasks.md`:

1. serve the shell against a throwaway `DAMAIAN_DATA_DIR` and a fixture
   repository, on a port other than 4765;
2. open the page in the browser;
3. in the devtools console, read the token the shell printed and run
   `apiToken = '<token>'`, then `setRepository('<absolute repo path>', false)`;
4. start a turn, open the header's context control, expand a category, pin a
   file, set a restriction, and (with the setting on) pause a turn, remove an
   item and send it.

A browser walk cannot exercise the desktop app's native dialogs; those are not
part of this spec's surface.
