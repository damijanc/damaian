# Feature Spec: A Provider's Own Key Reference Wins

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 73 of 73
Plan: none. A defect reported in use, like #7: an exception to the rule in
[`README.md`](README.md).
Depends on: nothing unbuilt. Cross-references:
[`18_local_evaluation_harness/proposal.md`](18_local_evaluation_harness/proposal.md)
§7 (the `config-set` provider-switch warning this changes) and
[`34_repository_config_trust_boundary.md`](34_repository_config_trust_boundary.md)
(`model_api_key_env` and `model_provider.*` stay forbidden in repository
config).

## 1. Motivation

On 2026-10-30 the user revoked a DeepSeek key and saved the new one on the
DeepSeek provider page. That stored it in the Keychain as `DEEPSEEK_API_KEY`
and wrote `model_provider.deepseek.api_key_env=keychain:DEEPSEEK_API_KEY`.
Settings reported the key `Saved`, but every chat turn sent the revoked key.

The same `user.conf` also held `model_api_key_env=keychain:model-api-key`, a
global line from July whose Keychain item still held the old key. Two code
paths resolved the reference differently:

- **Loading the config** applied the explicit `model_api_key_env` after
  `model_provider`, so the global line overrode the provider's own key.
- **Switching provider per request**, which `/api/model-key-status` and
  some routes do, re-ran `apply_model_provider_defaults` on the loaded config.
  That prefers a provider's `keychain:` reference, so it named the new key.

The status check therefore looked at a different key from the one that turns
used.

## 2. Requirements

1. Within one config scope, the key reference a loaded config resolves is the
   one switching to its active provider resolves, whatever order the lines are
   in.
2. The resolution rule itself is unchanged
   (`Config::apply_model_provider_defaults`). A provider's own reference wins
   when it is a `keychain:` reference or the provider is user-defined. A
   built-in provider's environment-variable default does not displace a
   keychain reference.
3. A user-defined provider with no `api_key_env` keeps the global reference
   rather than resolving to an empty variable name.
4. A higher-precedence scope's explicit `model_api_key_env` (admin over
   user) still overrides a provider chosen in a lower scope.
5. `config-set … model_provider` warns about an explicit `model_api_key_env`
   only when it actually overrides the provider's key.

## 3. Non-goals

- Merging the two Keychain flows (the Model API Key panel and a provider's
  own key) into one. The panel already shows the account the active provider
  resolves to, so saving there updates the item turns use.
- Re-reading the key for a running worker session. Workers receive the key
  when they are spawned or restarted (#57), so one started before a key change
  keeps the old key until it is restarted. `TROUBLESHOOTING.md` says so.

## 4. Design

`ConfigOverlay` application (`config.rs`) now applies `model_api_key_env`
before `model_provider` in each scope. When a scope names both,
`apply_model_provider_defaults` sees the global reference and makes the same
decision a later per-request switch would. `model_name` and `model_base_url`
are still applied after the provider and keep winning, as #18 §7 documents.

`apply_model_provider_defaults` skips a provider whose `api_key_env` is empty.

`print_provider_switch_warnings` (`damaian-cli`) loads the effective config
and stays silent for a line whose value is not the resolved one.

The Settings override warning (`keyOverrideWarning`, `app.js`) now names the
active provider's key reference as a possible cause, alongside an admin
override.

## 5. Acceptance Criteria

- A `user.conf` with `model_provider=deepseek`,
  `model_api_key_env=keychain:model-api-key` and
  `model_provider.deepseek.api_key_env=keychain:DEEPSEEK_API_KEY` resolves
  `keychain:DEEPSEEK_API_KEY` in either line order, and switching to `deepseek`
  resolves the same reference.
- `model_api_key_env=keychain:model-api-key` with `model_provider=deepseek`
  and no provider override still resolves `keychain:model-api-key`
  (`provider_defaults_preserve_keychain_references`).
- A user-defined provider without `api_key_env` keeps the global reference.
- `config-set user model_provider deepseek` with the first criterion's
  provider line prints no `model_api_key_env` warning. The existing switch
  to OpenRouter still warns about the keychain reference it keeps.

## 6. Implementation Notes

The tests are `a_providers_own_keychain_key_wins_over_the_global_reference`
and `a_custom_provider_without_a_key_reference_keeps_the_global_one` in
`workspace-engine/tests/foundation.rs`, and
`a_global_key_reference_the_provider_key_outranks_is_not_a_warning` in
`damaian-cli/tests/provider_switch_warning.rs`. The first engine test fails
against the old order; the second guards the new order against an empty
provider reference.

## 7. Verification

Local checks: see the change's hand-back. The workspace clippy and test suite
are left to CI.
