# Feature Spec: Resumed Turns Keep the Chat's Model

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 74 of 74
Plan: none. A defect reported in use, like #73: an exception to the rule in
[`README.md`](README.md).
Depends on: nothing unbuilt. Cross-references:
[`73_provider_key_reference_wins.md`](73_provider_key_reference_wins.md) (found
while fixing it),
[`44_composer_control_density/`](44_composer_control_density/proposal.md)
(the composer's model picker),
[`56_provider_fallback_consent/proposal.md`](56_provider_fallback_consent/proposal.md)
(the one resume that deliberately switches provider).

## 1. Motivation

On 2026-10-30 the user removed the `model_provider`, `model_name`,
`model_base_url` and `model_api_key_env` lines from `user.conf`. The model
picker still said DeepSeek, and the first answer of each turn came from
DeepSeek. When the turn continued, the reply ended with
`Request failed: OPENAI_API_KEY is required`.

Only the first request of a turn (`/api/ask-stream`, `/api/propose-edit`)
sent the picker's `model_provider`, `model` and `reasoning_level`. Every route
that resumes a paused turn built its engine from the saved config alone. With
no provider line that config named the built-in default, `openai`. With a
provider line, a resume could run on a different provider, model or key from
the turn's first request. This is how the stale key in #73 reached some
requests but not others.

## 2. Requirements

1. Every request that continues a paused turn applies the chat's provider,
   model and effort, the same way the first request does:
   - command approval, both streaming and the non-streaming `run-command` and
     `reject-command` fallbacks;
   - fetch decision;
   - plan decision;
   - answering a question;
   - *Continue* or *Answer now* at the round budget;
   - context review;
   - verification;
   - checks.
2. Manual compaction (`/api/compact`) makes a model call, so it applies the
   chat's selection too.
3. Accepting a provider fallback offer keeps its own target provider (#56).
   That route does not take the chat's fields.
4. A request without the fields behaves as before, using the saved config. This
   covers older clients and tests.

## 3. Non-goals

- Recording the provider in the paused turn itself. If the user changes the
  picker while a turn is paused, the resume uses the new choice. That is what
  "the picker sets the next request" already means.
- Worker sessions, which run with the config and key they were spawned with
  (#57).

## 4. Design

The server change is in `desktop-shell/src/lib.rs`. Each resume handler now
builds its engine through `config_for_repo_with_model_options(repo, form)`,
the helper the chat route already used, instead of
`config_and_report_for_repo(repo)`. `run_compact_request` takes the form for
the same reason.

The client change is in `app.js`. `withChatModel(data)` merges
`chatModelFormFields()` into the request, and each `streamResume*Request`
helper, `streamVerifyRequest` and the compact call use it. Fields the caller
passes win.

## 5. Acceptance Criteria

- `a_resume_uses_the_chats_selected_provider`: with no provider in the saved
  config, a fetch-decision resume whose form names `deepseek` /
  `deepseek-v4-pro` builds its adapter for that provider and model. Against
  the old route the test fails with `openai` / `gpt-4.1`.
- The existing round-budget, fetch, compaction and worker resume tests pass
  unchanged.

## 6. Implementation Notes

The fallback route stays on `config_for_repo_with_provider` with the offer's
target, and its decline makes no model call. The two non-streaming command
routes now build their engine with the report, as the streaming ones already
did.

## 7. Verification

Local checks: see the change's hand-back. The workspace clippy and test suite
are left to CI.
