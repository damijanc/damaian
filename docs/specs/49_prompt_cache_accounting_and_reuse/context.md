# Prompt Cache Accounting and Reuse — Context

Background for [`proposal.md`](proposal.md): why this work exists, what the code
looks like today, and what planning against the code found that the original
flat spec did not know. The proposal is the decision; this is the ground it
stands on.

## 1. Motivation

**This is not a future problem. The cost figures Damaian reports today are
already wrong for any configured provider that caches automatically and reports
the split.**

`Config::estimated_cost` (`config.rs:909`) applies one rate to every input
token:

```rust
let input_rate = provider.price_per_million_input_tokens?;
let output_rate = provider.price_per_million_output_tokens?;
Some(
    (usage.input_tokens as f64 / 1_000_000.0) * input_rate
        + (usage.output_tokens as f64 / 1_000_000.0) * output_rate,
)
```

A cache hit is commonly billed at a fraction of an uncached input token — around
a tenth — and the providers that do it report the split in the usage object. So
the figure [#19](../19_token_and_cost_accounting/proposal.md) worked to make
honest is measured on one axis and assumed on another, and the overstatement
grows with the conversation, because the cached prefix is the part that grows.

The second half is the opportunity. An agentic turn re-sends a growing prefix on
every round of the tool loop, and that prefix is the largest, most repetitive
part of the request. Nothing in the pipeline is designed to keep it stable, so
any hit today is an accident.

## 2. Current state, verified against the code

Everything the flat spec's §2 claimed was checked and holds:

- **`TokenUsage` has three fields** — `input_tokens`, `output_tokens`, `source`
  (`model.rs:198`) — and no notion of a cached subset. `UsageSource` is
  `Measured` / `Estimated` (`model.rs:172`).
- **`extract_usage(raw) -> Option<(u64, u64, Option<f64>)>`** (`model.rs:1407`)
  pulls prompt tokens, completion tokens and cost. Nothing parses a cache field.
- **`ModelProviderConfig` carries two rates**, both `Option<f64>`, and
  `estimated_cost` returns `None` unless both are set.
- **No built-in price table**, deliberately (#19 §4). This spec adds none.
- **The usage probe is the precedent for capability detection**: observed once,
  recorded per provider, never inferred from the provider's name.
- **`TaskUsage`** (`session.rs:283`) aggregates per task and is read through the
  single `SessionStore::read_task_usage` (`session.rs:812`) that the shell, the
  turn result and the eval harness all share. #19's rule is that no second count
  may exist, and this spec inherits it.

## 3. What planning found

### 3.1 Requirement 5 is already satisfied, and its test is a guard rather than a fix

`system_prompt()` (`chat.rs:2846`) returns a static string: no timestamp, no
session id, no run id, no round counter. It is `messages[0]`
(`chat.rs:698`), so §5.3's section 1 is already exactly where the design wants
it and already stable.

The test the flat spec asks for — two requests assembled a measurable interval
apart, identical prefixes — therefore passes the day it is written. That is not
a reason to skip it. It is the cheapest possible guard on a property that is
easy to destroy accidentally and silent when destroyed, and writing it now costs
one test.

### 3.2 The prefix is one string, and its internal order is the reverse of §5.3's

`build_model_prompt` (`chat.rs:2851`) concatenates, in this order:

1. `Recent conversation:` — the last eight messages, each truncated to 2,000
   characters
2. `User request:` — the current prompt
3. `Repository context:` — the assembled items

The stable part is last and the most volatile part is first, so the cacheable
prefix ends at the end of the system prompt. Everything the feature exists to
cache sits behind two volatile sections.

**This was already recorded by [#55](../55_conversation_compaction.md) §2**,
which found it while surveying the same function and explicitly assigned
resolving it to this spec or [#26](../26_context_assembly.md). Two specs
arriving at it independently is worth noting: it is a real property of the code,
not an artifact of one reading.

§5.3 of the flat spec assumes the sections are separately orderable. They are
concatenated into a single `ModelMessage::user`, so "reorder the sections" is an
edit to one string builder — small in itself, and not the hard part.

### 3.3 The eight-message window is the hard part, and no spec owns it yet

```rust
let recent_messages = prior_messages.iter().rev().take(8).collect::<Vec<_>>()
    .into_iter().rev().collect::<Vec<_>>();
```

This is a **sliding window**. Past the eighth message, every new turn drops the
oldest one, so the conversation section changes *at its first byte* on every
turn. Reordering the sections does not fix that: it moves the instability
later in the prefix, which helps only if the sections ahead of it are large
enough to be worth caching on their own.

#55 knows this window as silent context loss and proposes replacing it with
compaction. Nobody has written down that it is also what makes prefix reuse
impossible past eight messages. Both specs want the same code changed, for
reasons that do not conflict, and neither can be done well without deciding what
replaces the window.

**Consequence for this spec:** requirement 4 is not the "obligation, not a
redesign" the flat spec calls it. Ordering is one edit; a stable conversation
section is a design decision that belongs with #55 or #26.

### 3.4 Measuring first is cheap and currently nobody knows the number

§7 asks for the hit rate over a representative multi-round task, and no such
measurement exists. The accounting half produces it. Building the reuse half
first would mean changing assembly with no before-number to compare against —
the mistake [#47](../47_agent_working_capability/proposal.md) §7.2 was written
to avoid repeating.

## 4. What this does not touch

- **Response caching.** Answering a question with a previous answer is a
  different feature with a different risk, and this spec does not open it.
- **A built-in cache price table.** #19 §4's reasoning is unchanged: rates move,
  and a stale table reports confident wrong numbers.
- **The estimate for providers that report no usage at all.** They stay exactly
  as #19 left them.
- **Context assembly's packing rules.** #26 owns them. This spec asks assembly
  for determinism and asserts it; it does not redesign it.
- **The eight-message window itself.** Named here as the blocker it is, and left
  to #55 or #26 to replace.
