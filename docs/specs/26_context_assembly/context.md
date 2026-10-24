# Context: Context Assembly

Background for [`tasks.md`](tasks.md). Read this before Task 1, with
[`proposal.md`](proposal.md) §2 beside it. The proposal's design holds; its file
and line references were written before specs 25, 32, 49, 50, 59 and 62 landed,
and three of its statements about the current code were wrong. The corrections
themselves are the table in [`tasks.md`](tasks.md); this file is the map a task
worker needs, the places other specs already depend on, and the tests that
already exist.

Everything below was checked against the tree on 2026-10-24. Line numbers drift:
re-read the function before editing near it.

## 1. The two paths into the model

There are two, and both go through `ContextManager`.

### Chat turn

`ChatOrchestrator::ask_with_session_with_options` (`chat.rs:862`) does, in
order:

1. read the repository index, and check the session belongs to it (`:879-891`);
2. dispatch the `session_start` hook when there is no session yet (`:898-921`);
3. create the task, set `PreparingContext`, carry any plan from the previous
   turn (`:924-947`), append the user message (`:948-951`);
4. dispatch `before_context` (`:978-1028`) and collect its `hook_context` and
   requested-command findings;
5. **assemble**: `build_context_with_hooks(...)` at `chat.rs:1030-1039`, with
   `self.config.context_token_budget()` as the one budget;
6. **render**: `build_model_prompt(prompt, &context.items, &prior_messages, None)`
   at `chat.rs:1040`;
7. build the message array — `[system(system_prompt_for_session(..)), user(model_prompt)]`
   (`:1045-1053`) — and hand `context.files` to `run_agentic_turn`
   (`:1055-1066`).

Within the turn, later rounds append real `ModelMessage`s (assistant, tool
results) and are bounded by `bounded_messages` (`chat.rs:6161`); they do **not**
re-enter context assembly. `context.files` becomes `ChatTurnResult.context_files`
(`chat.rs:466`), carried through every pause and resume (`:1434`, `:1559`,
`:1711`, `:1851`) and read by the eval harness.

### One-shot edit

`EditOrchestrator::propose_edit` (`edit.rs:412`) dispatches `before_context`
(`:484-508`), assembles through the same `build_context_with_hooks`
(`edit.rs:509-518`), and renders through `build_edit_prompt` (`:1086`) into a
`ModelRequest` at `:519-533`. It records `contextFiles` and `tokenEstimate` on
`edit_model_request_prepared` (`:535-545`).

### What assembly is not

- **Not persisted.** `ContextPlan` is built and consumed inside the call. It is
  never written anywhere; the only trace is the audit fields named below. That
  is the gap spec 27 closes.
- **Not the within-turn context.** Tool results reach the model as messages.
  Spec 65's exploration memory and spec 66 work on that array, not on this
  pipeline, so they do not collide with this spec's *data*, only with
  `build_model_prompt` (see §6).
- **Not the command result.** `build_model_prompt`'s `command_context` parameter
  (`chat.rs:6105`) appends a `--- command_result ---` section. Its only production
  call passes `None` (`chat.rs:1040`); one unit test (`chat.rs:10466`) exercises
  it. A resumed command result is appended as a `ModelMessage` instead, in
  `resume_after_command_decision_with_options` (`chat.rs:1092`-ff), so the
  parameter is legacy.

## 2. What the model actually receives

`build_model_prompt` (`chat.rs:6101-6150`) produces **one string**, in this
order:

1. `Recent conversation:` — the last 8 `prior_messages`, each truncated to 2,000
   characters (`:6108-6125`);
2. `User request:` — the raw prompt (`:6126-6127`);
3. `Repository context:` — every `ContextItem` as
   `\n--- <kind>: <path> ---\n<content>` (`:6128-6141`), `kind` and `path`
   omitted where absent;
4. optionally `--- command_result ---`.

Two things follow that the proposal now owns:

- The user prompt is sent **twice**: once as the `user_prompt` item
  (`context_manager.rs:139-147`) and once as section 2. It is charged to the
  token budget twice as well.
- The sections are in volatile-to-stable order, the reverse of what a prompt
  cache wants ([`49_prompt_cache_accounting_and_reuse/context.md`](../49_prompt_cache_accounting_and_reuse/context.md)
  §3.2; [`55_conversation_compaction.md`](../55_conversation_compaction.md) §2;
  `docs/PLAN/OBSERVATIONS.md` entry 5, which assigns the decision to #26 or
  #49).

`build_edit_prompt` (`edit.rs:1086-1105`) duplicates the item header format with
its own copy. Any change to the `--- kind: path ---` shape has to change both,
which is why Task 1 collapses them into one renderer.

### The user's prompt is not redacted on any path

Checked on 2026-10-24, because §2 above shows the request is written twice and
only one copy goes through `SecretScanner`. **It is not redacted anywhere before
it reaches the model.** The evidence, one path at a time:

| Path | What happens |
|---|---|
| Desktop shell → chat | `/api/chat` parses the form and passes `prompt` straight to `ask_with_session_with_options` (`desktop-shell/src/lib.rs:2270`). `required_form` (`:3660-3665`) only checks it is non-empty |
| Desktop shell → edit | `/api/propose-edit` does the same into `propose_edit` (`lib.rs:1370`, `:1398-1404`) |
| CLI | `damaian` passes its argument through to the same two entry points (`damaian-cli/src/main.rs:516`, `:532`) |
| Eval harness | Builds the prompt from the scenario file (`eval-harness/src/runner.rs:368`) |
| Engine, chat | `chat.rs` contains no `scanner.redact` of the incoming `prompt`; every `redact` call in the file is for model output, tool results, questions, command text or an audit field (e.g. `:1612`, `:3165`, `:4350`, `:6455`) |
| Engine, renderer | `build_model_prompt` writes the raw `prompt` under `User request:` (`chat.rs:6126-6127`) |
| Engine, edit | `build_edit_prompt` writes the raw `prompt` under `User edit request:` (`edit.rs:1088-1089`) |
| Transport | `model_request_json` (`model.rs:1394-1418`) escapes JSON and nothing else |

So a secret pasted into the prompt is sent to the provider in cleartext, while
the duplicate `user_prompt` item is correctly redacted by `add_text`
(`context_manager.rs:422`). The raw text is also what `task.user_prompt` and the
session log hold — deliberately, since spec 16 requires a faithful transcript
that a rewind can restore — but the transcript is local and the request is not.

**The same hole is in the `Recent conversation:` block.** It writes each stored
message's `content` raw (`chat.rs:6117-6124`), and stored messages are verbatim
by spec 16, so a secret typed in an earlier turn is replayed to the provider on
every later turn of that session. Nothing redacts it on the way out either.

Both holes are fixed at the renderer in Task 1 (Correction C17): the request is
rendered from the already-redacted `Required` item, and the conversation block is
passed through the scanner before it is written. Redacting clean text is a no-op,
so a prompt with no secret renders byte-identically.

## 3. The current code, by file

### `crates/workspace-engine/src/context_manager.rs` (566 lines)

| Item | Line | Note |
|---|---|---|
| `AGENT_INSTRUCTIONS_FILE`, `PROJECT_RULES` | `:12`, `:13-23` | Root-level project rules; a package's own manifest is not included (OBSERVATIONS 32) |
| `ContextItem` | `:25-32` | `kind: String`, no range, no category, no provenance |
| `ContextPlan` | `:34-41` | `{ repository_id, task_id, token_estimate, items, files }`, `Eq` |
| `ContextManager` | `:43-51` | Holds file access, scanner, data dir, semantic flag, `RepositoryMapSource`, `SymbolSource`. No `PathPolicy`, no `AuditLog` |
| `RepositoryMapSource` | `:54-61` | The map store, the engine's `CommandPolicy`, `repository_map_max_tokens` |
| `SymbolSource` | `:64-71` | The symbol store, **its own `PathPolicy`**, `symbol_context_max_tokens` |
| `new` | `:74-90` | Six arguments |
| `build_context` | `:96-116` | Delegates to `build_context_with_hooks` with `&[]` |
| `build_context_with_hooks` | `:124-312` | The whole pipeline, in source order: user prompt (`:139-147`), path candidates (`:149-182`), explicit and mentioned files (`:184-207`), instructions (`:209-222`), project rules (`:224-237`), repository map (`:241-252`), symbols (`:256-269`), hook context (`:276-286`), search results (`:288-303`) |
| `load_map` | `:318-328` | Returns `None` when both ceilings are 0 or the store cannot load; failure leaves the item out rather than failing the turn |
| `repository_map_text` | `:331-334` | `render_for_model(max_tokens)` |
| `symbols_text` | `:342-362` | Reads the stored index as it is; never waits on a language server |
| `add_file` | `:364-407` | Path dedup at `:378`; `ReadWindow::Whole` at `:391`; `files.push` only when `add_text` succeeded (`:404-406`) |
| `add_text` | `:410-440` | Empty check `:419`; **redact `:422`**; estimate `:423`; budget test `:424-426`; push `:428-438` |
| `agent_instruction_paths` | `:442-470` | Root `AGENTS.md` first, then each ancestor directory, deduped. The `..` check runs before the backslash normalisation (OBSERVATIONS 30, still open, harmless because reads are policy-checked) |
| `prompt_file_mentions` | `:472-519` | Exact path, or a basename that matches exactly one indexed file |
| `prompt_path_candidates` | `:521-566` | Tokenises the prompt, rejects absolute/URL/`..`/directory-shaped candidates |

Note the source of the search results: `keyword_search` and `semantic_search`
are **concatenated** into one list at `:164-172`, and only `result.path` is used
— the snippet is dropped, and which list a path came from is lost before any
item exists. Keeping them apart is Task 2's requirement, because §5.3 drops
semantic hits first.

### `crates/workspace-engine/src/file_access.rs`

| Item | Line | Note |
|---|---|---|
| `LineRange` | `:16-19` | `usize`, 1-based inclusive; already re-exported (`lib.rs:103`) |
| `ReadWindow` | `:30-39` | `Whole` (no caps), `Default` (first `max_read_lines`), `Range` (clamped to `max_read_lines`) |
| `FileRead` | `:41-58` | Includes `line_range`, `total_lines`, `truncated_by`, `hash`, `redaction_status` |
| `read_file` / `read_file_in` | `:88`, `:114` | Resolve → `assert_not_restricted` (`:125-129`) → byte/line window → **redact** (`:219`) → audit |
| line cap | `:184-189` | `Range` is capped to `max_read_lines` (default 400, `config.rs:2585`); `Whole` opts out |
| path policy | `:149-153` (`path_policy.rs`) | `is_restricted(path, is_directory)`, `assert_not_restricted` |

### `crates/workspace-engine/src/config.rs`

| Item | Line | Note |
|---|---|---|
| `Config.context_token_budget()` | `:1925-1934` | Per-provider/model, then built-in, then `DEFAULT_CONTEXT_TOKEN_BUDGET` |
| the doc that calls it a packing budget | `:1920-1924` | "how many tokens of repository context to pack" |
| built-in budgets | `:3843-3848` | DeepSeek V4 → 64k, against a 1M window, on purpose |
| `ConfigOverlay` | `:2650`-area | The struct a new `context_budget.*` field joins |
| `classify_overlay_fields!` | `:2787-2795`, list `:2797`-ff | Exhaustive destructure: a new field does not compile until classified |
| `apply_overlay_scoped` | around `:1600-1650` | The restrict-only numeric loop (`restrict_only_limit`) new budget keys follow |
| `to_policy_text` (overlay) | `:3374`-ff | Exhaustive; a field skipped here is erased from the user's file on any save |
| `parse_price` | `:3866` | The existing decimal parser |

### Other files a task touches

| File | Item | Line |
|---|---|---|
| `chat.rs` | `build_model_prompt` | `:6101-6150` |
| `chat.rs` | `model_request_prepared` (records `contextFiles`, `tokenEstimate`) | `:2927-2944` |
| `chat.rs` | per-round `len/4` estimate over the message array | `:2921-2926` |
| `chat.rs` | `model_call` marker estimate (survives a crash) | `:2963-2969` |
| `chat.rs` | `system_prompt_for_session` | `:5980` |
| `edit.rs` | `build_edit_prompt` / `edit_system_prompt` | `:1086-1105` / `:1080` |
| `workspace_engine.rs` | `ContextManager::new` call, with the engine's `PathPolicy` in scope | `:102-117` |
| `session.rs` | `read_task_plan` | `:1554` |
| `session.rs` | `record_finding` / `read_findings` | `:1697` / `:1738` |
| `audit.rs` | `record(event_type, fields)` — redacts every value | `:42` |
| `symbol_index.rs` | `Provenance` | `:64` |
| `symbol_index.rs` | `find_for_prompt` (sorts by provenance, then quality) | `:490-536` |
| `symbol_index.rs` | `render_for_model(matches, max_tokens)` | `:545` |
| `repository_map.rs` | `render_for_model(max_tokens)`, `root_for_path` | `:425`, `:525` |
| `indexer.rs` | `RepositoryIndex`, `keyword_search`, `semantic_search` — **no path policy** | `:59`, `:68`, `:99` |

## 4. What other specs already changed or rely on here

- **#24 repository map.** Adds one whole-block `repository_map` item between the
  project rules and the retrieved files (`context_manager.rs:241-252`), with its
  own `repository_map_max_tokens` ceiling (800) that a repository may lower but
  not raise. `build_context_places_one_map_item_before_retrieved_files`
  (`tests/repository_map.rs:1832`) asserts the position by `kind` string. #26
  keeps the item, gives it the `RepositoryMap` category, and must keep it before
  retrieved files.
- **#25 symbols.** Adds one whole-block `symbols` item after the map, with
  `symbol_context_max_tokens` (1,600), already sorted by provenance then match
  quality, and already filtered for restricted files. `tests/symbol_index.rs:3193`
  and `:3245` assert its position. The item's provenance is inside the rendered text
  (`heuristic — may be inaccurate`), so a per-item `provenance` can only be the
  lowest one present.
- **#32 hooks.** `build_context_with_hooks` exists only for hooks: each
  `before_context`/`session_start` `context` entry becomes a `hook_context` item
  after symbols and before retrieved files (`context_manager.rs:276-286`). Its
  test (`tests/hooks.rs:2977-3024`) asserts the item's `kind`, that the content is
  redacted, and that an oversized entry is dropped rather than displacing the
  prompt. **#32's Task 10 requires `hook_context` to survive as a category**;
  #26 gives it its own share, which makes its priority explicit instead of
  implicit in its position, so it moves ahead of the map. Only the item's
  presence is asserted today, not its position. It must never become `Required`.
- **#49 prompt cache.** Its accounting slice shipped; its **reuse slice is
  blocked on #26 or #55** and its `proposal.md` says "#26 decides the ordering".
  It also owns `emit_cache_breakpoints` (`chat.rs:2918`) and the two
  prefix-stability tests in `tests/prompt_cache.rs`, which compare whole
  `ModelRequest`s for equality across two assemblies of an unchanged repository.
  **That test is this spec's determinism guard as well.** Do not emit breakpoints
  here.
- **#55 compaction.** Assumes (a) one user message holds the prompt, (b) the
  recent-message window is the last eight, and (c) when #26 lands the window
  "becomes one of its categories". This spec keeps (a) and (b) as they are and
  reserves the category (c) without filling it.
- **#27 inspector.** Needs the manifest, the categories and the ranges; the
  `Pinned` category is its to fill. This spec leaves the seam by construction:
  `ContextRequest` is a struct, so #27 adds a `pinned` field without changing what
  either caller passes for everything else. Nothing populates it in #26.
- **#59 exploration allowance.** Touches the round loop and `read_file`'s tool
  arguments, not assembly. It does read `context.files` indirectly through the
  turn result, and its tests assert round counts, not prompts, so assembly
  changes do not reach it.
- **#50 clarification.** A paused turn is resumed from the `messages` stored in
  the pending-turn file (`PendingTurn.messages`, read at the top of every resume
  path, e.g. `chat.rs:1478` and `:1111`); it never re-assembles context. A new
  prompt shape therefore cannot change what a resumed turn sends, because the
  built messages travel with the pause.
- **#65 exploration memory.** Edits `build_model_prompt` and the turn loop. It
  must not run beside this spec (§6).
- **#19 token accounting.** Records measured input tokens per call
  (`task_usage_recorded`). That is half of §5.8's calibration and the reason the
  estimate may improve without a tokenizer. Spec 19 explicitly refuses
  "attributing cost to individual context components", so the calibration
  compares totals, not per-category measured values.
- **#18 eval harness.** Reads `context_files` from the turn result and asserts
  `context_contains`, `context_excludes` and `context_ranks_within`. It never
  reads `ContextItem`.

## 5. Tests and helpers that already exist

| Where | What it pins |
|---|---|
| `tests/foundation.rs:4285` `attaches_unique_file_mentions_to_chat_context` | A uniquely named prompt mention enters context |
| `tests/foundation.rs:4319-4446` `chat_includes_root_agents_md_as_agent_instructions`, `propose_edit_includes_agents_md_in_edit_prompt` | `--- agent_instruction: AGENTS.md ---` appears, and the hierarchy renders root → `crates/` → `crates/workspace-engine/` in that order |
| `tests/foundation.rs:1819` `chat_reports_context_assembly_before_it_calls_the_model` | `PhaseKind::Context` precedes `PhaseKind::Model` |
| `tests/foundation.rs:5597` `context_token_budget_follows_the_selected_model`, `:5621` `configured_context_token_budget_overrides_the_model_default` | The budget resolution, through the audit fields |
| `tests/repository_map.rs:1832` | One map item, after the last `project_rule`, before `retrieved_file` |
| `tests/symbol_index.rs:3193`, `:3245` | The symbols item's position and ceiling |
| `tests/hooks.rs:2977`, `:3416` | `hook_context` from an edit-flow and from a chat turn; redaction; dropping |
| `tests/prompt_cache.rs` | Two assemblies of an unchanged repository produce byte-identical `ModelRequest`s (this is the determinism guard) |
| `tests/agent_tools.rs:137-237` | `ReadWindow::Default` and `ReadWindow::Range` behaviour through the `read_file` tool |
| `tests/permission_profiles.rs` | `every_overlay_field_is_classified_exactly_once`, `weakening_cases`, `every_capability_key_resists_weakening_from_repository_scope` — every config task reruns these |
| `tests/repository_config_trust.rs` | The trust boundary; never edited, always run after a config change |

Helpers worth reusing: `test_config(&repo)` in `foundation.rs:55` (sets
`data_dir` inside the fixture and `enable_index_watcher: false`), `temp_dir` /
`write_fixture` in the same file, `MockModelAdapter` with `.requests` for
asserting what a turn actually sent, and `collect_turn_progress`.

New pipeline tests belong in a new `crates/workspace-engine/tests/context_assembly.rs`.
The existing files above are edited only where a `kind` string or an item
position moved — never to weaken an assertion.

## 6. Parallel work and shared files

- **`build_model_prompt` is the contended function.** #26, #55, #49's reuse
  slice and #65 all edit it; #65 and #66 also edit the turn loop. None of them
  may run beside this spec. `docs/specs/README.md` says #26, #55 and #49's reuse
  slice never run together; #65 should be added to that list.
- **`context_manager.rs`** is edited only by this spec now that #24 and #25 are
  done. #32 touched it last (Task 6, `build_context_with_hooks`).
- **`config.rs`, `profile.rs`, `effective_policy.rs`,
  `tests/permission_profiles.rs`** are edited by the budget task. That is a
  rebase with nothing in flight; rerun the partition tests.
- **`file_access.rs`** gains one `ReadWindow` variant. `chat.rs`'s tool call
  (`:5333-5334`), `damaian-cli/src/main.rs:169` and `tests/agent_tools.rs` match
  on `ReadWindow`, so all of them must still compile and pass.

## 7. The deterministic eval tier, and what it exercises here

`cargo run -p eval-harness -- run --tier deterministic` runs 23 scenarios against
fixture repositories with `MockModelAdapter`, so no network and no key. Its
config leaves `context_token_budget: None` (`eval-harness/src/runner.rs:62`,
`:263`), which resolves to the built-in 64k for the DeepSeek fixtures.

Scenarios that touch this pipeline, and why they are the regression baseline:

| Scenario | Asserts |
|---|---|
| `agents_md_scoping` | `context_contains = ["src/AGENTS.md"]` — the nested instruction must survive |
| `restricted_path` | `context_excludes = ["secrets/api_token.txt"]` and the token is absent everywhere |
| `seeded_secret` | A readable file with a seeded key reaches context and is redacted everywhere |
| `file_references`, `exact_symbol`, `conceptual_feature` | Which files/items reach context, and their rank |
| `worktree_survey`, `long_discovery`, `batched_reads` | Tool traffic, not prompts, but they run a turn through assembly |
| `planned_task` | Plan state exists in the turn |

`run.context_files` comes from `ContextPlan.files`, so `files` must keep meaning
"unique paths included, in inclusion order" after the rewrite, or every
`context_*` assertion moves at once.

## 8. Things the proposal says that were checked and hold

- `SecretScanner::redact` is the only path by which item content becomes text,
  and `add_text` calls it before measuring (`context_manager.rs:422`). Preserve
  that order: measuring unredacted content would size the budget by the secret's
  original length, and storing it would put the secret in the prompt.
  Item content is the only thing that goes through it today; the user's prompt
  and the prior-conversation block do not (§2, "The user's prompt is not
  redacted on any path"), which is Correction C17.
- `AuditLog::record` redacts every field value (`audit.rs:49-51`), so a manifest
  that holds no content is a second-order safety net rather than the only one.
- `PathPolicy` is `Clone` and cheap to hold; `workspace_engine.rs:88` already
  builds the one instance the engine keeps.
- `SymbolSource`'s policy is the engine's own; a collection-time check in the
  symbol source is the same instance, not a second rule.
- The map and symbol stores already filter restricted files
  (`repository_map.rs:1046`, `symbol_index.rs:295`), so their items need the
  collection-time check only for the paths they embed in text.
- `ContextItem`, `ContextPlan` and the new manifest types can all keep `Eq` if
  fractions are basis points rather than floats — which is why §5.2 stores them
  that way.
