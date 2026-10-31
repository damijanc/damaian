# Feature Spec: Change Requests Reach the Agent Loop

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 72 of 72
Plan: none. Evidence-driven, like #64, #69 and #70 — an exception to the rule
in [`README.md`](README.md).
Depends on: [#03](03_structured_tool_calling.md) (the agentic turn proposes
patches through `propose_patch`) and
[#47](47_agent_working_capability/proposal.md) (`edit_file`, ranged reads and
search) — both built. Everything else named below is a cross-reference.
Related implementation specs:
[`20_working_modes/proposal.md`](20_working_modes/proposal.md) (the mode check
the one-shot flow gained in §7),
[`57_worker_sessions/proposal.md`](57_worker_sessions/proposal.md) (the
planning-request exception to the same heuristic).

## 1. Motivation

On 2026-10-30 the user gave Damaian its first implementation task: add `{a,b}`
alternatives to `glob_match`, with unit tests, a tool-description update and a
spec note, then run the tests and clippy on a new branch. The session log shows
one model call answering in the `DAMAIAN_EDIT_V1` whole-file envelope, then an
applied patch, and nothing else: no file read, no tool call, no command, no
branch. The model said it could not edit `chat.rs` because the file "was not
included in the supplied repository context". The patch broke an existing test
it never ran, changed `.gitignore` matching as a side effect, and its spec note
described changes it had not made.

> **Note, 2026-10-30.** That brace request was built later the same day, as
> [#47](47_agent_working_capability/proposal.md) §7.4's amendment: a new
> `glob_match_with_alternatives` used by `search_content`'s `path_glob`, with
> `glob_match` and the ignore rules left literal. This spec is about the flow
> that garbled the request, not about that feature.

The turn never reached the agent loop. `looksLikeEditRequest` in `app.js`
sends any prompt with an edit verb (`add`, `implement`, `fix`, …) and a code
word (`test`, `code`, `docs`, …) to `/api/propose-edit`, the one-shot patch
flow that predates native tool calling. Nearly every implementation request
matches, so tools, plans, checks and the round budget never applied to the
work they exist for.

## 2. Current State

- `sendChatPrompt` (`app.js`) calls `proposePatchFromChat` when
  `!options.agentic && looksLikeEditRequest(prompt)`, whatever the provider.
- The agentic turn (`ChatOrchestrator`, `chat.rs`) offers `propose_patch` and
  `edit_file` to a provider with `supports_native_tools`, so it can already
  answer a change request with a reviewable patch.
- A provider without native tools gets the text-envelope fallback, whose
  system prompt teaches only `DAMAIAN_COMMAND_V1`; it cannot propose a patch.

## 3. Requirements

1. With a provider whose native tool-calling is on, every chat request runs as
   an agentic turn. No request goes to the one-shot patch flow.
2. With a provider whose native tool-calling is off, routing is unchanged: a
   change request still goes to the one-shot flow, with the existing planning
   and findings-repair exceptions.
3. The engine, the one-shot flow, the CLI's `propose-edit` and every approval
   are unchanged.

## 4. Non-goals

- Removing the one-shot flow. It is still the only way a provider without
  tools produces a patch, and the CLI uses it.
- Changing what the agentic turn does with a change request. Whether it reads,
  edits, tests and finishes well is what the next live run measures.

## 5. Design

`changeRequestsUseOneShotPatch()` (`app.js`) is true only when the chat's
selected provider has `supportsNativeTools === false` in the provider catalogue
the UI already loads from the effective policy, which defaults it to true. The
send path takes the one-shot flow only when that and `looksLikeEditRequest`
both hold. Documentation: `docs/USER_GUIDE.md` (how a change request runs) and
`docs/TROUBLESHOOTING.md` (a change request answered with one patch and no
tools; the worker handoff note now names the condition).

## 6. Acceptance Criteria

- With DeepSeek (native tools on), "Add a test for the glob matcher in
  ignore.rs" sends
  `/api/ask-stream`, not `/api/propose-edit`.
- With a provider whose native tool-calling is off, the same prompt sends
  `/api/propose-edit`.
- A question ("how do I add a test?") goes to the chat turn with either
  provider, as before.

## 7. Implementation Notes

The change is one predicate in `app.js` and its use at the routing branch.
There is no JavaScript test harness for `app.js`, so the criteria were walked
on 2026-10-30 in the UI served by `serves_the_ui_for_manual_inspection`, with
`fetch` intercepted so no model was called. With DeepSeek selected, "Add a test
for the glob matcher in ignore.rs" (which `looksLikeEditRequest` matches) went
to `/api/ask-stream` with native tools on and with the flag unset (its
default), and to `/api/propose-edit` with it set to false; "how do I add a
test?" went to `/api/ask-stream` with it false.

Found while walking it: `looksLikeEditRequest` matches `test` but not `tests`
(its code words are whole words), so "Add tests for …" never took the one-shot
flow even before this change. Left as it is: with native tools the rule no
longer decides anything.

Local checks: `node --check`, web lint, typos, fmt and deny. No Rust changed.
The workspace clippy and test suite are left to CI.
