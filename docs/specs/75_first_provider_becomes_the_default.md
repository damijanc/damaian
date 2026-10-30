# Feature Spec: The First Saved Provider Becomes the Default

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 75 of 75
Plan: none. A defect found in review, like #73 and #74: an exception to the
rule in [`README.md`](README.md).
Depends on: nothing unbuilt. Cross-references:
[`74_resumed_turns_keep_the_chat_model.md`](74_resumed_turns_keep_the_chat_model.md)
(the same chat-selection rule, applied to resumes), and
[`57_worker_sessions/proposal.md`](57_worker_sessions/proposal.md) (a worker
takes the configured provider and model).

## 1. Motivation

On 2026-10-30 we reviewed what a clean installation writes to `user.conf`
when the user follows the Quick start: **+ Connect**, paste the key, **Save
Provider**. The file holds only `model_provider.<id>.*` lines. No Settings
control has written `model_provider` since multi-provider support arrived in
July. The model picker keeps its choice per project in browser storage and
sends it with each request.

So the saved config kept the built-in default, `openai` with
`OPENAI_API_KEY`, and three things read that default:

- **A new project.** With no stored choice of its own, its picker started on
  OpenAI, so the first prompt failed with `OPENAI_API_KEY is required`.
- **A worker session.** It takes the configured provider (#57), so it failed
  to spawn with the same error.
- **The commit-message draft.** It built its adapter from the saved config and
  silently produced no draft.

## 2. Requirements

1. Saving a provider in Settings writes `model_provider=<id>` when the config
   being edited names none. A config that already names a default keeps it.
2. Renaming the default provider's id moves `model_provider` to the new id.
3. Removing the default provider makes the next configured provider the
   default and drops `model_name`, which named one of the removed provider's
   models. When none is left, both lines go and the built-in default applies.
4. The commit-message draft applies the chat's provider, model and effort,
   like a turn.
5. Worker sessions keep taking the configured provider and model (#57). With
   requirement 1 that is the provider the user set up.

## 3. Non-goals

- Changing a config that already names `model_provider`, or rewriting a
  default the user chose.
- Carrying the chat's selection into a worker. That would change #57's
  handover and its record, which restarts read.

## 4. Design

`saveProviderConfig` (`app.js`) adds `model_provider` after upserting the
entry when `configValue(content, "model_provider")` is empty, and repoints it
on a rename. `removeProviderConfigFromSettings` passes the edited text through
`withoutDefaultProvider`, which uses the new `providerIdsInConfig` and
`removeConfigValue` helpers.

`handle_commit_message` (`lib.rs`) builds its engine through
`config_for_repo_with_model_options`, and the client sends
`chatModelFormFields()` with the request.

## 5. Acceptance Criteria

- Saving DeepSeek into an empty config writes `model_provider=deepseek`.
  Saving OpenRouter next leaves it at `deepseek`.
- Removing DeepSeek then sets `model_provider=openrouter` and removes
  `model_name`. Removing OpenRouter as well leaves neither line.
- Removing a provider that is not the default leaves `model_provider` as it
  was.

## 6. Implementation Notes

There is no JavaScript test harness. On 2026-10-30 the helpers were extracted
from `app.js` in Node and run against each criterion above, and all five held.
The commit-draft change is the same helper swap #74 made for the resume
routes.

## 7. Verification

Local checks: see the change's hand-back. The workspace clippy and test suite
are left to CI.
