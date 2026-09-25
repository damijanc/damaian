# Context: Findings Model and Panel

Background for [`tasks.md`](tasks.md). Corrections to
[`proposal.md`](proposal.md) (the flat spec, unchanged in substance) where its
"Current State" or design sections no longer match the code, checked on
2026-09-25. Read this before Task 1. None of these should be "fixed" back to
the proposal's original wording without re-reading the code it cites.

## 1. Staleness has nothing to compare against

§5.1 says staleness "is computed by comparing the file's current hash against
the hash recorded when the finding was created". The `Finding` struct in the
same section records no hash. As written, `Stale` could never be computed.

**Decision:** `Finding` gains `file_hash: Option<String>`, the
`crate::hash::sha256` form (`"sha256:<hex>"`) of the file at `range.path` when
the finding was recorded.

- `None` when the finding has no range, or the file could not be read at
  creation time.
- A finding with `file_hash: None` is **never** marked `Stale`: there is
  nothing to compare, and guessing "stale" would drop a finding from a repair
  selection on no evidence. This is the proposal's own "never invent a
  location" rule (§5.3), applied to time instead of place.
- A finding whose file no longer exists is `Stale`.

Parsers never compute the hash. They see only a `CommandExecution`, not the
repository. The call site that turns an execution into findings (Task 7) hashes
the file, the same way it attaches `task_id` and `origin_ref`. So `Finding::new`
stays pure and needs no filesystem.

§5.1 cites `patch_engine.rs:291` for the mechanism being reused. The comparison
is actually at `patch_engine.rs:328` (`current_hash != file.base_hash`), and
both sides use `hash::sha256`. What is reused is the hash function and the
"compare against the recorded one" shape, not a function to call:
`patch_engine` has no standalone stale check.

## 2. Public fields contradict "no code path can create an unredacted finding"

§5.6 puts redaction at construction so that "no code path can create an
unredacted finding". §5.1's struct has every field `pub`. A struct literal
`Finding { summary: raw, .. }` compiles anywhere in the crate and bypasses
`Finding::new` completely. The guarantee holds only if the compiler enforces
it.

**Decision:** every `Finding` field is private.

- Construction goes through `Finding::new(draft, &scanner)` and three
  `with_*` builders (`task_id`, `origin_ref`, `file_hash`), none of which take
  free text.
- Reads go through getters.
- `status` is the only mutable field, through `set_status`.
- `Deserialize` is the one other way to get a `Finding`. It reads back values
  this code wrote to the session log from already-redacted findings (Task 7),
  so it does not reopen the hole. If a later spec deserialises findings from
  somewhere else, such as hook output (spec 32), that spec must go through
  `Finding::new` instead.

`SourceRange` and the three enums stay plain `pub` data. They carry no free
text that §5.6 covers.

## 3. The parser trait cannot call `Finding::new`

§5.3's `FindingParser::parse(&self, execution) -> Vec<Finding>` has no scanner
to pass to `Finding::new`. Threading a scanner into every parser would mean
every parser has to remember to redact, which is what §5.6 exists to prevent.

**Decision:** parsers return `Vec<FindingDraft>`. `FindingDraft` is plain `pub`
data: source, severity, summary, details, range, and code. Only the dispatcher
(Task 2) calls `Finding::new`. That keeps one construction point per path, and
one redaction point in total. Task 1 defines `FindingDraft`. Task 2 changes the
trait signature accordingly.

## 4. Redact first, then bound

§5.6 says `details` is redacted and bounded, but not in which order. The order
matters. Bounding first can cut a secret so that what remains no longer
matches its pattern. The AWS key rule looks for the `AKIA` prefix (`secret_scanner.rs:114`),
so a cut that drops that prefix leaves the rest of the key in plain text.

**Decision:** `Finding::new` redacts the whole draft text first, then bounds
the redacted text. A bound that cuts through a placeholder leaves a harmless
fragment. A bound that cuts through a secret can leak most of it. Task 1 pins
this with a test in which the secret sits across the bound.

The bounds are decided here because the proposal gives none:

- `summary`: the first non-empty line, trimmed, at most **240 characters**
  (ending in `…` when cut). An empty draft summary becomes
  `"<source> finding with no message"`, so a finding is never blank.
- `details`: at most **4096 bytes**, cut on a char boundary, keeping the
  **head** and appending `\n… (truncated)`. Parsed findings put the most
  useful lines first. The generic parser (Task 2) chooses its own tail
  *before* building the draft, so "a bounded tail" (§5.3) still holds for it.

## 5. `detect_project_commands` does not discover what §5.3's table says

§5.3 says the shipped parsers cover "the ones
`CommandPolicy::detect_project_commands` discovers". For Rust it discovers only
`cargo test` (`command_policy.rs:216-222`), not `cargo build`, `check` or
`clippy`. For npm it discovers `npm run lint`, not this repository's
`npm run lint:web` (`package.json`).

That has a consequence the table misses. The most common failure of
`cargo test` is a compile error, which prints rustc diagnostics
(`error[E0308]` plus `--> path:line:col`) and no `failures:` block. A test
parser that only reads `failures:` returns nothing for that output. By §5.3's
fall-through rule it then drops to a single generic finding, even though a
structural parser for exactly that output ships beside it.

**Decision:** the Rust test parser (Task 4) runs the Rust diagnostics parser
(Task 3) over the same output first, and uses its drafts when it returns any.
The parser table's "Recognises" column stays as written. `matches` still
recognises the commands the table lists, and `npm run lint:web` is added to the
Biome row because this repository's own check needs it.

## 6. "Full output" is already bounded

§5.6 says full output lives in `stdout.log` and `stderr.log`
(`CommandStore::save_execution`, `validation.rs:64-90`). Those files hold
`CommandExecution.stdout` and `stderr`, which the runner has already
**tail-truncated** to `max_command_output_bytes` (default 1 MiB,
`config.rs:1582`) and redacted (`command_runner.rs`, `truncate_output` then
`redact`).

So "full output remains reachable through `origin_ref`" means "the stored
output". That is the most any part of Damaian keeps, and it is enough for the
acceptance criterion. Two consequences for the parsers:

- The **head** of a very long output may be missing, so parsers must not assume
  the first line is a header. `cargo test`'s `failures:` block is at the end
  and survives.
- Command output reaching a parser is **already redacted**. Redacting again in
  `Finding::new` is still required: browser entries (Task 6) do not pass
  through the command runner.

## 7. Things the proposal says that were checked and hold

- Spec 12 is still `In progress`, so §5.4's plan to extend
  `WebDiagnosticReport` rather than parse its text still applies. The struct is
  at `web_diagnostics.rs:83`, exactly as cited.
- Spec 05 is Done, and its navigation is what the panel reuses (Task 10).
- `CommandExecution` is at `command_runner.rs:16-32`, not `11-22`. It has since
  gained `termination: CommandTermination`. The generic parser (Task 2) should
  use it: a timed-out command is a failure with no exit code, and "exited
  non-zero" must not be read as `exit_code != Some(0)` alone.
- `serde` with `derive` and `serde_json` are already dependencies of
  `workspace-engine`, and `u128` fields already round-trip through them
  (`PlanStep.started_at_ms`, `plan.rs`).
- Ids come from `hash::create_id(prefix)`. Findings use the prefix `finding`,
  which gives the `finding_…` form that §5.7's example shows.
- Spec 21 deferred `Evidence::Findings` until this spec exists
  (`21_task_plan_progress_and_budget/context.md` §3.6). `Evidence` is already
  `#[non_exhaustive]` for this. Task 7 adds the variant, because that is when
  finding ids first exist in the session log.
