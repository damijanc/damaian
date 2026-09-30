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

- **Superseded 2026-09-29 by spec 12's close-out; see §7.1 below.** This note
  used to say that spec 12 was `In progress`, so that §5.4's plan to extend
  `WebDiagnosticReport` with `entries` still applied.

### 7.1 The browser structure now exists (spec 12, 2026-09-29)

Spec 12's close-out built what §5.4 planned to add, under a different shape.
Its [`context.md`](../12_web_app_troubleshooting/context.md) §3.5 records the
decision. `WebDiagnosticReport` (`web_diagnostics.rs`) now carries
`details: Option<WebDiagnosticDetails>`, parsed once from the browser
companion's JSON in `WebDiagnosticReport::from_text`, and redacted by
`WebDiagnosticReport::redacted`. The typed values Task 6 needs are:

- `details.page_errors: Vec<String>`: uncaught page exceptions, with no
  location.
- `details.console: Vec<WebConsoleEntry>` with `level`, `text` and
  `location: Option<WebSourceLocation { url, line, column }>`.
  `WebConsoleEntry::is_problem()` selects errors, warnings and asserts.
- `details.failed_requests: Vec<WebFailedRequest>` with `url`, `method`,
  `resource_type`, `status` and `failure`.
- `WebDiagnosticReport::tool_failed()`: the runner itself failed (MCP
  `is_error`, or the companion's `"error": true`). That is not the page being
  broken.

So **Task 6 converts these into `Finding`s and must not add
`WebDiagnosticEntry`, `WebEntryKind` or `WebDiagnosticReport.entries`**. Those
would be a second browser structure next to `details`. Map the page errors,
the `is_problem()` console entries and the failed requests. A console
`location` becomes a `SourceRange` only when it resolves inside the
repository. Note that `location.url` is the *served* URL (for example
`http://localhost:5001/static/js/main.js`), not a file path, so the resolution
rule has to map a URL path to a repository file, or give up to `range: None`.
§5.4's "no entries and `is_error: true` yields one generic finding" becomes
"`details` is `None`, or `tool_failed()` is true with nothing to convert". A
runner that is not the companion always has `details: None`.

That answers proposal §7's open question. `entries` was never added. Spec 12
supplied the structure itself, as `WebDiagnosticDetails`, and the runner
already emits it for the companion.
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

## 8. Decisions made while planning Task 2 (2026-09-30)

### 8.1 A generic finding needs a source of its own

§5.3's table gives the generic parser a severity, a summary and details, but no
source. None of the eight `FindingSource` variants fits a check Damaian does
not understand. Calling a failed `pytest` `Test`, or a failed `make` `Compiler`,
would be a guess from the command name, which is exactly what §5.2 rules out.
§5.8 also promises that `TROUBLESHOOTING.md` will explain "how to tell a
generic-fallback finding from a parsed one", and the type as built gives no way
to tell.

**Decision:** Task 2 adds `FindingSource::Command`, serialised as `"command"`:
one command's failure taken as a whole, with nothing parsed out of it. Every
generic finding has this source, and only generic findings have it. This
includes a `cargo test` failure whose parser matched but extracted nothing:
that finding really is unparsed, and the source says so. Adding the variant now
costs nothing, because no finding has been persisted yet (Task 7). The panel
(Task 10) groups by source, so the generic findings sit together, and each one
names its command in its summary (§8.3).

### 8.2 A cancelled check has no verdict, so it gets no generic finding

The Task 2 outline and Global Constraints listed "a cancellation" among the
failures that fall through to the generic parser. They should not have. A
cancelled check was stopped by the user before it reached a verdict. A generic
`Error` saying "`cargo test` was cancelled" would appear in the panel's default
Open + Error view and could be selected for "Fix selected". That would ask the
agent to repair something the user did on purpose.

**Decision:** the dispatcher classifies each execution one way:

| Execution | Verdict | Parser drafts | Generic fallback |
|---|---|---|---|
| `Exited`, `exit_code == Some(0)` | passed | kept (warnings) | never |
| `Exited`, any other code, or `None` (killed by a signal) | failed | kept | when the drafts are empty |
| `TimedOut` | failed | kept | when the drafts are empty |
| `Cancelled` | none | kept | never |

A cancelled run still keeps what its parser found. A compile error printed
before the user pressed Stop is a real problem, and it has a real location. So
"never lose a failure" still holds: a cancelled run had no failure to lose.
Spec 23 reads `termination` directly when it needs to know that a run was cut
short.

### 8.3 The generic finding's text

- **Summary.** When the process timed out, it is `"<command> timed out"`. When it
  was killed by a signal, it is `"<command> was killed by a signal"`. Both
  times, the output goes in `details`, because how it ended is the headline.
  Otherwise the summary is `"<command>: <first non-empty stderr line>"`, then
  stdout's first line when stderr is empty, then
  `"<command> exited with code N"` when both are empty. The command is in the
  summary because every generic finding shares one source, so the summary is
  the only place a reader can see which check failed.
- **Details.** The last 40 lines of stderr, then the last 40 of stdout. Each is
  labelled, and an empty stream is left out. The tail is kept here, and
  `Finding::new`'s bound keeps the head of that (§4). Together
  that gives "a bounded tail" (§5.3), starting with stderr, where most tools
  report failure.
- **No range and no code.** Always. An exit code is not a diagnostic code, and
  the panel reads the exit code from the summary.

### 8.4 Where the parsers live

Task 2 leaves the trait, the dispatcher and the generic fallback in
`finding.rs`. Tasks 3–5 each add one file under
`crates/workspace-engine/src/finding/` (`rust_diagnostics.rs`, `rust_test.rs`,
`biome.rs`), declared from `finding.rs`, and register the parser in
`default_parsers()`. That keeps `finding.rs` a single-screen type module and
puts each parser's fixtures next to the parser they test.

## 9. What real rustc and clippy output looks like (Task 3, 2026-09-30)

Captured with cargo 1.98.0 from a throwaway two-level workspace (a root
`Cargo.toml` with one member under `crates/demo`). The Task 3 fixtures are
these captures, with the absolute `Compiling` path replaced by `/repo`. What
they showed, and what each finding means for the parser:

- **Diagnostics are blocks separated by one blank line.** A block starts with
  a column-0 header, `error[E0308]: msg`, `error: msg` or `warning: msg`. It
  runs until the next blank line. Blocks contain no blank lines, even when they
  have child notes.
- **Child `note:` and `help:` lines sit at column 0 inside the block, and can
  carry their own `-->`.** E0061's block carries `note: function defined here`
  followed by `--> crates/demo/src/lib.rs:13:4`, which points at the function's
  definition, not at the error. **Decision:** the location is the first `-->`
  after the header and before any child header. A parser that takes "a `-->`
  in the block" would send the user to the wrong line.
- **The `-->` indent varies** with the width of the line numbers (` -->` and
  `  -->`), so match it with leading whitespace. Snippet lines can also start
  at column 0 (`13 | fn takes…`), so "starts with a space" does not mark a
  continuation line.
- **Paths are relative to the workspace root, even when cargo runs in a
  member's directory.** `cd crates/demo && cargo check` still prints
  `crates/demo/src/lib.rs`. So the parser emits the path as printed.
  **Decision:** an absolute path, or one with a `..` component, gets
  `range: None`. Examples are registry sources, the standard library under
  `/rustc/<hash>/`, and a path dependency outside the workspace. Clicking
  those would open something that is not the user's code. Task 7 still checks
  every relative path against the repository root, because a workspace root
  nested below the repository root would make these paths relative to the
  wrong directory.
- **Codes come from different places depending on the kind of diagnostic.**
  - A compiler error has `[E0308]` in its header.
  - A clippy lint has
    `= help: for further information visit …/index.html#needless_return` on
    **every** occurrence.
  - The `= note: #[warn(clippy::needless_return)]` line appears only on a
    lint's **first** occurrence, so it cannot be the source of the code (the
    old outline said to use it).
  - A rustc lint has only the first-occurrence note: `#[warn(unused_variables)]`,
    or under `-D warnings`, `` `-D unused-variables` implied by `-D warnings` ``
    with dashes. Later occurrences carry nothing.

  **Decision:** take the bracket first, then the clippy URL (giving
  `clippy::<name>`), then the attribute note, then the `-D` note (with dashes
  turned into underscores). A later rustc-lint occurrence gets `code: None`.
  That is honest, not a bug. Inventing the code from an earlier block's note
  would be the cross-block guesswork §5.3 rules out.
- **Source.** A diagnostic whose code starts `clippy::` is `Lint`. Everything
  else is `Compiler`, including rustc's own lints like `unused_variables`,
  because the compiler is what reported them.
- **`-D warnings` turns every warning header into `error:`.** Severity follows
  the header word as printed (§5.2: recorded, not normalised).
- **Cargo's own summary lines use the same header shape and must be skipped.**
  They are `` warning: `demo` (lib) generated 2 warnings ``,
  `error: could not compile …`, and, from rustc run directly,
  `error: aborting due to …` and `warning: N warnings emitted`. The trailing
  `Some errors have detailed explanations` and `For more information…` lines
  are not headers, so a header match ignores them.
- **`CARGO_TERM_COLOR=always` wraps headers and `-->` in ANSI escapes**
  (`\x1b[1m\x1b[91merror[E0308]\x1b[0m\x1b[1m: …`). The runner pipes output, so
  cargo normally prints no colour. But a user's environment can force colour,
  so the parser strips `\x1b[…m` sequences before matching.
- **Diagnostics go to stderr.** The parser reads only `stderr`.
- **A warnings-only build exits 0.** `findings_from_execution` keeps a passing
  run's parsed drafts (Task 2), so a clean-exit `cargo clippy` still yields its
  warnings.
