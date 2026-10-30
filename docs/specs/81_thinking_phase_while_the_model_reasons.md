# Feature Spec: A Thinking Phase While the Model Reasons

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 81 of 81
Plan: none. Usability feedback from use, like #73–#80: an exception to the
rule in [`README.md`](README.md). #79, the context budget work, also landed
the same day.
Depends on: [#03](03_structured_tool_calling.md) (thinking-mode reasoning is
read from the response and replayed), built. Related: #59 and #65, whose counts
the same indicator line already shows.

## 1. Motivation

While a turn waits on the model, the indicator in the reply says "Waiting for
model" and an elapsed clock. With DeepSeek's thinking mode
(`deepseek-v4-flash`), a call often takes 30–60 s, and most of that time the
provider is streaming reasoning, not answer text. Nothing on screen changes
except the clock, and the user reported that it "feels stuck".

The engine already reads that reasoning from the stream
(`extract_reasoning_content` in `model.rs`), but only once the response is
whole, and only so it can be replayed on the next request (#03). The UI
never learns that the provider is producing anything.

## 2. Requirements

1. While a model call streams reasoning and no answer text yet, the turn
   indicator says **Thinking**, followed by how much reasoning has arrived:
   "Thinking · 1.2k chars".
2. The count grows while the reasoning does. The label is updated at most about
   twice a second.
3. The reasoning text itself is not shown, anywhere. Only its size leaves the
   engine.
4. Once answer text streams, the indicator says "Streaming" as before; a
   reasoning count that arrives after that does not replace it.
5. A model that does not reason never shows the phase. The indicator is then
   exactly what it was.
6. A worker session shows the same phase in its live view, from its progress
   file.
7. The reasoning stored on the assistant message and replayed to the provider
   is unchanged.

## 3. Non-goals

- Showing the reasoning text. A later, opt-in disclosure may; it would need its
  own decision about redaction and storage, because reasoning is model output
  that has not crossed the secret scanner.
- Recording the reasoning count in the session log. It is a live signal only.
- Reasoning on calls other than an agentic turn's model round: compaction's
  summary, the commit-message draft and patch generation keep calling
  `stream_response`, which reports nothing.

## 4. Design

- **Adapter.** `ModelAdapter` gains `stream_response_with_reasoning`, which
  takes an extra `on_reasoning: FnMut(usize)` called with the cumulative count
  of reasoning characters each time a fragment arrives. Its default ignores the
  callback and calls `stream_response`, so every existing adapter and test
  double is unchanged. `OpenAICompatibleAdapter` implements it:
  `send_with_retries` runs `line_reasoning` on each complete SSE line, which
  reads the same three sources in the same order as `extract_reasoning_content`
  (`reasoning_content`, `reasoning`, `reasoning_details`) through the shared
  `chunk_reasoning`. The count restarts with each retried attempt.
  `stream_response` is the new method with a no-op callback.
  `MockModelAdapter` reports a scripted response's reasoning once, whole,
  before its first token.
- **Engine.** `PhaseKind::Thinking` (`"thinking"`) and
  `TurnPhase::reasoning_chars` (zero on every other phase). The agentic turn's
  model round calls `stream_response_with_reasoning` and turns each count into
  a `Thinking` phase with an empty label and the round's usual counts. The
  first count is reported at once, later ones at most every
  `THINKING_PROGRESS_INTERVAL` (500 ms). Without that, a worker's progress
  file would gain one line per chunk.
- **Shell.** `phase_json` adds `reasoningChars`. The SSE `phase` event and the
  worker's progress file both use it, so neither needed further changes.
- **UI.** `TURN_PHASE_LABELS.thinking` is "Thinking". `startTurnIndicator`
  treats `thinking` like `model`: it does not end the round's streaming, and is
  ignored once "Streaming" is shown. It appends `· N chars`, formatted like the
  notes size (`formatCharCount`, "1.2k" from 1,200). The worker view folds
  phase events into the same indicator, so it needed no change.

## 5. Acceptance Criteria

- `streamed_reasoning_is_counted_as_it_arrives` (`model.rs`): a stream with
  reasoning under `reasoning_content`, then under `reasoning` with its
  duplicate `reasoning_details`, reports `[6, 12]`, not doubled; the run's
  content and stored `reasoning_content` are as before.
- `reasoning_is_reported_as_a_thinking_phase_with_its_size`
  (`tests/foundation.rs`): a turn whose model reasons 1,234 characters emits a
  `Thinking` phase after the `Model` phase, with `reasoning_chars` 1,234, round
  1 and an empty label.
- `no_reasoning_means_no_thinking_phase`: a model that does not reason emits
  none.
- `a_thinking_phase_carries_the_reasoning_count_and_no_text` (shell): the
  phase JSON is `"phase":"thinking"` with `"reasoningChars":1234` and an empty
  label.
- The existing reasoning replay tests (`chat_replays_reasoning_content_*`)
  still pass.

## 6. Implementation Notes

`chunk_reasoning` and `reasoning_details_text` were nested inside
`extract_reasoning_content`; they moved to module level so the streaming count
and the stored reasoning read the provider the same way. The stored value is
still assembled from the whole response, not from the counted fragments.

The turn's refusal wait and the reasoning count both report progress during
the same call, so the round shares `on_progress` between them through a
`RefCell`.

## 7. Verification

Scoped checks passed locally: the fast checks
(`scripts/quality-gate.sh fmt node lint typos deny`) and clippy for
`workspace-engine` and `desktop-shell`. The two crates' suites ran 2,358
tests. 2,351 passed and seven `desktop-shell` route tests failed from a
worktree under `.claude/`. The same seven, with this change applied, passed
from a checkout outside `.claude/`, so the failures come from the checkout's
location, not from this change. The workspace clippy and test suite are left
to CI.
