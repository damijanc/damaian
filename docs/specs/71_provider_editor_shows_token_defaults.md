# Feature Spec: The Provider Editor Shows Its Default Limits

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI). The user: when they open their provider's
configuration, it "just shows it is default, without actually displaying the
number."
Order: 71 of 71
Plan: none. Usability feedback, like #67 and #68 — an exception to the rule in
[`README.md`](README.md).
Depends on: nothing unbuilt. The per-model defaults it shows were introduced
with the output and context limits documented in the user guide's *Output and
Context Limits*.

## 1. Motivation

**Max output tokens** and **Context budget** in Settings › Providers ›
*Provider details* showed the placeholder `Automatic` when left blank. What
"automatic" means differs per model — 65,536 output and 64,000 context for
`deepseek-v4-flash`, no output ceiling and 16,000 context for most others — and
only the engine knew it, so a user could not tell what they would be raising
or lowering. The user guide listed the defaults, but nowhere near the field.

## 2. Current State

- `Config::max_output_tokens` and `Config::context_token_budget` (`config.rs`)
  resolve the configured value, then `builtin_model_output_tokens` /
  `builtin_model_context_budget`, then the built-in provider's value, then
  (context only) `DEFAULT_CONTEXT_TOKEN_BUDGET`.
- The editor (`app.js`, `renderProviderConfigForm`) carries a blank field
  through untouched and shows the static placeholder `Automatic`.

## 3. Requirements

1. A blank field's placeholder shows the number it resolves to for the first
   model in the provider's **Models** list, the one a new chat uses.
2. An output ceiling Damaian does not send reads as such, not as a number.
3. When the listed models have different defaults, a note under the fields
   names each model's pair.
4. The numbers come from the engine's own resolution, so the editor cannot show
   a value the engine would not use. The UI holds no copy of the table.
5. Editing the provider id or the models list updates them.

## 4. Non-goals

- Changing any default. The context budgets stay deliberately below the
  models' windows (user guide, *Output and Context Limits*).
- Showing the window a provider reports. Damaian does not measure one.

## 5. Design

`builtin_token_defaults(provider, model) -> (Option<u32>, u32)` in `config.rs`
is the blank-field resolution, and `Config::max_output_tokens` and
`Config::context_token_budget` now fall back through it, so the two share one
code path. `GET /api/provider-token-defaults?provider=<id>&models=<a,b>`
(`desktop-shell`) answers `{"models":[{"model","maxOutputTokens","contextTokenBudget"}]}`,
with `null` for no output ceiling; no models asks for the provider's own
default, and a blank provider id gets an unknown provider's. The editor's
`showProviderTokenDefaults` calls it when the form is drawn or cleared and on
`change` of the id or models fields, and drops an answer that arrives after the
form moved to another provider.

## 6. Acceptance Criteria

- DeepSeek with `deepseek-v4-flash` first shows `Default: 65,536` and
  `Default: 64,000`.
- OpenAI shows `Default: none sent, the provider decides` and
  `Default: 16,000`.
- Listing `deepseek-v4-flash` and `deepseek-chat` shows the per-model note;
  listing only V4 models hides it.
- A configured value still fills the field and wins, as before.

## 7. Implementation Notes

Tests: `builtin_token_defaults_match_what_a_blank_field_resolves_to`
(`workspace-engine/tests/foundation.rs`) checks the function against both
resolvers for five provider/model pairs; `provider_token_defaults_answers_per_model`
(`desktop-shell`) drives the endpoint, including the unnamed provider.

Walked on 2026-10-30 in the UI served by `serves_the_ui_for_manual_inspection`:
the three §6 cases render as written, and the longest placeholder fits the
field at the default window width.

Local checks: scoped clippy and tests for `workspace-engine` and
`desktop-shell`, and the fast checks (fmt, node, web lint, typos, deny). The
workspace clippy and test suite are left to CI.
