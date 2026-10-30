# Feature Spec: The Model's Context Window and the Per-Turn Budget

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 79 of 80 (built alongside #80, which took the next number while this was in flight)
Plan: none. A defect reported in use, like #73–#78: an exception to the rule
in [`README.md`](README.md).
Depends on: [#26](26_context_assembly/proposal.md) (context assembly, the
declared limit) and [#71](71_provider_editor_shows_token_defaults.md) (the
provider editor's defaults), both built.

## 1. Motivation

When #71 was specified, the user said DeepSeek has a 1M-token window and the
provider editor showed only "default". They then set
`model_provider.deepseek.context_token_budget=1000000`.

That key is not the window. It is #26's per-turn packing budget: how much
repository content assembly puts into each request before the model reads
anything. #26 §5.7 and correction C6 say so, and the built-in for a 1M-token
model is 64k on purpose. Assembly packs to the budget, so with 1M available,
every turn on 2026-10-30 assembled about 832,000 tokens:

- whole files as search results, 581k in all, including `desktop-shell/src/lib.rs`
  at 185k;
- file ranges, 249k in all, including all of `chat.rs` at 212k.

The header meter read "83% of budget". It was arithmetically right and gave no
hint that the budget was the problem.

The configuration had nowhere to declare the window, and the editor's
"Context budget" read like one.

## 2. Requirements

1. A provider entry may declare `context_window`: how many input tokens the
   model accepts. Built in, `deepseek-v4-flash` and `deepseek-v4-pro` have
   1,000,000. Other models have no known window.
2. `context_token_budget` keeps its meaning and its defaults. It is clamped to
   the window when one is known, because more than the model accepts cannot be
   sent. Declaring a window never raises the budget.
3. The manifest records the window as `limit.window`. A chat turn also records
   what it sends beside the assembled context, both estimated at `len / 4`:
   - `conversationTokens`, the conversation messages; the summary is already a
     category of its own;
   - `systemPromptTokens`, the system prompt.

   Tool definitions are not counted.
4. The header meter measures the whole request against the window: "N% used".
   With no known window it falls back to "N% of budget".
5. The context panel opens with an overview modelled on Claude's:
   - the request against the window, with a stacked bar;
   - a row per part (each category in use, Conversation, System prompt) with
     its tokens and share;
   - Free space;
   - a line naming the per-turn budget and saying that tool definitions are not
     counted.
6. The provider editor shows **Context per turn** (`context_token_budget`) and
   **Model context window** (`context_window`). Each has its built-in default
   as a placeholder, from `/api/provider-token-defaults`, which gains
   `contextWindow`. The help text says not to put the window into the
   per-turn field.

## 3. Non-goals

- Measuring a provider's window (#26's `observed` limit). The window here is
  declared or built in, and no margin is subtracted from it.
- Rewriting a user's config. This repository's single user had their
  `context_token_budget=1000000` moved to `context_window` by hand.
- Counting tool definitions, and changing the conversation window's share of
  the budget (#55). Both are unchanged.

## 4. Design

- **`config.rs`.** `ModelProviderConfig` and its overlay gain
  `context_window`. It goes through the exhaustive `present_fields`, both
  branches of `upsert_model_provider`, the parse arm (`parse_token_count`, so
  `0` is refused) and both policy-text writers.
  - `builtin_context_window` is exported and returns the built-in window.
  - `Config::context_window()` returns the provider's value, falling back to
    the built-in.
  - `Config::context_token_budget()` takes the minimum of the budget and the
    window.
- **`context_manager.rs`.** `ContextRequest` gains `window`. `build_context`
  records it on the declared `EffectiveLimit`. `ContextManifest` gains
  `conversation_tokens` and `system_prompt_tokens` (`serde(default)`).
- **`chat.rs`.** The three turn sites that assemble context pass
  `config.context_window()`: the first turn, the repair turn and #77's
  continuation. Once the messages are built, they call `note_turn_overhead`
  before the manifest is shown. The first turn now shows its manifest after
  building messages instead of before, still before any request. `edit.rs`
  passes the window too.
- **`app.js`.**
  - `contextUsage` sums the parts.
  - `contextMeterText` measures against the window or the budget.
  - `contextOverview` draws the panel's overview.
  - The editor reads and writes `context_window`. `style.css` gives the
    overview's parts their colours.

## 5. Acceptance Criteria

- DeepSeek V4 with only `context_window=1000000` resolves a 1M window and the
  64,000 budget. Without the line, the built-in window applies. A 500,000
  budget with a 128,000 window resolves 128,000. With no window known, the
  budget stands alone.
- `context_window` round-trips through policy text, and `0` is refused.
- A DeepSeek V4 turn's manifest has `limit.window` 1,000,000, `limit.tokens`
  64,000, `systemPromptTokens` equal to the system message length / 4, and
  `conversationTokens` 0 on a first turn.
- `/api/provider-token-defaults` returns `contextWindow` 1,000,000 for
  `deepseek-v4-flash` and `null` for `deepseek-chat`.

## 6. Implementation Notes

The tests:
- `the_context_window_is_separate_from_the_per_turn_budget` and
  `the_context_window_round_trips_through_policy_text` in
  `workspace-engine/tests/foundation.rs`;
- `a_turns_manifest_carries_the_window_and_what_the_turn_adds` in
  `tests/context_inspector.rs`;
- the extended `provider_token_defaults_answers_per_model` in `desktop-shell`.

There is no JavaScript test harness. The meter and the overview read only
fields these tests pin.

## 7. Verification

The `workspace-engine`, `eval-harness` and `desktop-shell` suites passed
locally (2,458 tests). The fast checks and scoped clippy are in the change's
hand-back. The workspace clippy and test suite are left to CI.
