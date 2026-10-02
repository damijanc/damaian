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
**Corrected 2026-10-01 by §10.2:** it keeps both the diagnostics and the test
failures, because a compiler warning and failing tests appear in the same run.
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

**Amended 2026-10-01 by §10.3:** "when the drafts are empty" became "when no
draft is an `Error`". A failed run whose parser found only warnings also gets
the generic finding.

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

## 10. What real `cargo test` output looks like (Task 4, 2026-10-01)

Captured with cargo 1.98.0 and cargo-nextest 0.9.144. The workspace had the
same shape as §9's (`crates/demo`), plus an integration test
(`crates/demo/tests/integration.rs`) and one failing doctest. The crash
capture came from a separate one-package crate. The Task 4 fixtures are these
captures, with two kinds of edit. The workspace path was replaced with
`/repo`, and the doctest's per-user temporary directory with `/tmp/`. cargo's
`Finished` line and rustdoc's `all doctests ran in …` line were dropped
because they report timings. The `test result:` lines were kept. Their
`finished in 0.00s` is a timing too, but the parser never reads it.

| Capture | Command | Exit | stdout | stderr |
|---|---|---|---|---|
| Compile error | `cargo test` | 101 | empty | rustc diagnostics |
| Failures | `cargo test --no-fail-fast` | 101 | three `failures:` blocks (lib, integration, doctest) | one warning, cargo's per-target lines |
| Crash | `cargo test` (the test calls `abort()`) | 101 | `running 1 test` only | one warning, `Caused by: … (signal: 6, SIGABRT …)` |
| Pass | `cargo test` | 0 | `test result: ok` | progress lines only |
| nextest | `cargo nextest run` | 100 | empty | everything (see §10.5) |

### 10.1 libtest's stdout

- **Each test binary prints `failures:` twice.** The first one introduces the
  `---- name stdout ----` sections. The second is followed by the failing
  names, indented four spaces and **sorted by name**. Then comes
  `test result: FAILED`. With `--no-fail-fast` there is one such block per
  binary, doctests included. The default fail-fast run stops after the first
  failing binary.
- **Sections come in the order tests finished**, which changes from run to run.
  Only the name list has a stable order, so findings follow the list.
- **A section does not always end in a blank line.** A `#[should_panic]` test
  that did not panic prints one `note:` line and then runs straight into the
  next `---- … ----` header. **Decision:** a section ends at the next section
  header or the next `failures:` line. It does not end at a blank line, and
  captured test output can contain blank lines anyway.
- **What each kind of failure prints**, inside its section:
  - **A panic:** any captured stdout, then
    `thread 'name' (9907617) panicked at crates/demo/src/lib.rs:27:9:`, then
    the message on the next line. The bracketed thread id is new in this
    toolchain. Earlier toolchains print the same line without it, so the
    pattern accepts both.
  - **`assert_eq!`:** the same, with the message
    `` assertion `left == right` failed: two plus two `` followed by the
    `left:` and `right:` lines.
  - **`unwrap()` in library code:** `#[track_caller]` puts the location in
    the library function the test called (`lib.rs:11:15`), not in the test.
    That is where it panicked, so it is kept.
  - **`#[should_panic]` that did not panic:**
    `note: test did not panic as expected at crates/demo/src/lib.rs:42:8`,
    which points at the test function's name. There is no `panicked at` line.
  - **`#[should_panic(expected = …)]` with the wrong message:** a normal
    `panicked at` line with the actual message (`underflow`), then
    `note: panic did not contain expected string` and the two strings.
  - **A test returning `Err`:** `Error: "config missing"`, with no location.
  - **A doctest:** the name is `crates/demo/src/lib.rs - add (line 3)`. The
    section says `Test executable failed (exit status: 101).`, then
    `stderr:`, then a panic at an **absolute temporary path**
    (`/tmp/rustdoctestVr99Wj/doctest_bundle_2024.rs:6:1` after scrubbing).
- **`note: run with RUST_BACKTRACE=1 …`** appears once per process, under
  whichever test panicked first. The parser ignores it.
- **No colour on stdout.** `CARGO_TERM_COLOR=always` colours only cargo's
  stderr, which `parse_rust_diagnostics` already strips. With output piped,
  libtest printed no escapes even with `-- --color always`.
- **`-q` keeps the sections.** **`-- --nocapture` moves panics to stderr**,
  so sections are left only for the `#[should_panic]` notes. Matching a
  stderr panic back to a test by its thread name is out of scope: those
  failures get no range.

**Decisions for each failure:**

- **One `Test` draft of severity `Error` per listed name, with no code.**
  libtest has no other level and no codes.
- **The summary is `<name>: <message>`, or `<name> failed` when no message was
  found.** The message is taken from the first of these that exists:
  1. the `#[should_panic]` note (`test did not panic as expected at …` or
     `panic did not contain expected string`), which explains the failure
     better than the panic's own text;
  2. the line after `panicked at`;
  3. an `Error: …` line.
- **The range** is the first `panicked at` or `did not panic … at` location,
  passed through §9's absolute-or-`..` rule (Task 4 moves that rule into a
  shared `workspace_range`). When that gives nothing, a doctest name gives
  `path` and `line`, with no column. That location is the doc block, as
  rustdoc printed it. A returned `Err` has no range.
- **The details are the section**, header included, with trailing blank lines
  trimmed. `Finding::new` applies the bound.
- **A listed name with no section** (cut off by tail truncation, §6, or by
  `--nocapture`) still becomes a finding: `<name> failed`, with no range and
  no details.
- **Each list reads only the sections printed since the previous list.**
  Names repeat across binaries (two crates can both have `tests::adds`), and a
  later binary's name must not take an earlier binary's location.

### 10.2 cargo's stderr under `cargo test`, and why §5's "either" was wrong

- A compile error looks exactly like §9's, with `(lib test)` in the
  `could not compile` line. stdout is empty.
- **cargo adds three header-shaped lines** that are summaries, not
  diagnostics: `` error: test failed, to rerun pass `--lib` ``,
  `` error: doctest failed, to rerun pass `--doc` ``, and
  `error: 3 targets failed:`, which is followed by the indented target list.
  Read as diagnostics, they would become `Compiler` errors that say nothing.
  Task 4 adds them to `is_cargo_summary`.
- **Warnings and failing tests appear in the same run.** In the failures
  capture, stderr has an `unused_variables` warning and stdout has eight
  failures. §5's "uses its drafts when it returns any" would report the
  warning and drop all eight failures. **Decision:** `RustTestParser::parse`
  returns both, with the diagnostics first.

### 10.3 A warning must not stand in for a failure

The crash capture has a warning on stderr, and then the test binary died
from `SIGABRT` before it printed any list. The parser returns one `Warning`
draft. Under §8.2's rule ("generic fallback when the drafts are empty"), that
draft suppressed the generic finding, and a failed `cargo test` reported
nothing but a warning. The same hole exists for any parser that reports
warnings: a `cargo build` killed after printing a warning, or a Biome run
that fails because of its warnings (Task 5).

**Decision:** `findings_from_execution` falls through to the generic finding
when a failed run produced **no draft of severity `Error`**, not only when it
produced no drafts. The warnings are kept, and the generic finding comes after
them. Passing and cancelled runs are unchanged. The crash's generic summary is
`cargo test: Compiling crash v0.1.0 (/repo)`, which is cargo's first stderr
line. That is weak, but it is honest, and the details' tail carries the
`Caused by: … SIGABRT` lines. Improving it would mean parsing `Caused by:`,
which no task requires.

### 10.4 Paths

Test locations follow §9: relative to the workspace root, even for an
integration test (`crates/demo/tests/integration.rs:8:5`). A one-package
crate prints `src/lib.rs`. Task 7 still resolves every relative path against
the repository root.

### 10.5 `cargo nextest run` is out of scope

nextest writes everything to stderr, in a different shape. Each failing test
is a `FAIL [ 0.013s] (1/8) demo tests::name` line. Under it come indented
`stdout ───` and `stderr ───` blocks, each holding a whole one-test libtest
run with a 4-space indent. At the end there is a `Summary` line, the `FAIL`
lines repeated, and `error: test run failed`, with exit code 100. By default
the first failure cancels the rest, so a run reports only part of the suite.

**Decision:** Task 4 does not parse nextest output, and `RustTestParser` does
not match `cargo nextest`. No shipped parser matches it, so a failed nextest
run gets one generic finding, compile errors included. The reasons:

- `detect_project_commands` proposes `cargo test` (`command_policy.rs:216-222`),
  so Damaian never proposes nextest itself.
- This repository's gate runs nextest from a developer's terminal or CI, not
  through Damaian's validation.
- nextest's shape needs its own parser. Bending this one to read indented
  copies on another stream would make both harder to falsify.

A later nextest parser could read the `FAIL` lines for names and the indented
`panicked at` lines for locations. It would also need the dispatcher's
generic fallback, which still applies, because of the fail-fast cancellation.

## 11. What real Biome output looks like (Task 5, 2026-10-01)

Captured with Biome 2.5.7, which is this repository's pinned version. There
are three captures:

1. `npm run lint:web` on this repository as it is. It exits 0 with one
   `info` diagnostic, the `useTemplate` one in `scripts/check-spec-status.mjs`
   that spec 20's notes mention.
2. `npm run lint:web` on a scratch copy of this repository's `biome.json`,
   with seeded problems: an unused variable, a `debugger`, a duplicate CSS
   property, two unformatted files and a syntax error. It exits 1.
3. The same tree with 25 more seeded files, to see what Biome does above its
   default cap of 20 diagnostics.

The fixtures are captures 1 and 2. Capture 2's paths are relative to the
scratch root, so nothing needed scrubbing. What the captures showed:

- **Diagnostics go to stderr. The summary goes to stdout.** That summary is
  `Checked 5 files …`, `Found 6 errors.` and `Found 2 warnings.`. `npm run`
  adds only its `> name@version script` banner to stdout. On failure it adds
  nothing to stderr. The seeded stderr is byte-identical whether Biome runs
  through `npm run` or directly.
- **A header is `path:line:col category [FIXABLE] ━━━…`**, for example
  `crates/desktop-shell/static/app.js:6:3 lint/suspicious/noDebugger  FIXABLE  ━━━`.
  The category is the rule's full name, and becomes `code`. A `format`
  diagnostic has **no location**:
  `crates/desktop-shell/static/styles.css format ━━━`. Its body is a diff of
  the whole file, so `range` is `None` and the summary is Biome's own
  "Formatter would have printed the following content:". A `parse`
  diagnostic has a location: `scripts/parse.mjs:1:17 parse ━━━`.
- **Blocks contain lines that are only whitespace** (`"  "`). Unlike rustc's
  blocks (§9), Biome's can't end at a blank line. **Decision:** a block runs
  from its header to the next line that ends in a `━━━` rule. Trailing
  whitespace-only lines are trimmed from its details.
- **Severity is the marker on the block's first message line.** That is
  `  × ` for an error, `  ! ` for a warning, and `  i ` for info. Later `i`
  lines in the same block are hints, not more findings. Biome's
  "Found 6 errors / 2 warnings" agrees with these markers, so the mapping is
  Biome's own, not a judgement (§5.2). A block with no marker line is
  recorded as `Error` with the category as its summary. A header only appears
  for a problem, and dropping it would lose a failure.
- **The run ends with a `check ━━━` block,** "Some errors were emitted while
  running checks". It is a header with no path and no category, so the
  diagnostic header pattern does not match it. It still ends the previous
  block, which is why block ends use the broader "any `━━━` line" rule.
- **Every Biome finding is `Lint`.** That includes `format` and `parse`,
  because Biome reported them as a check, not a compiler.
- **Above 20 diagnostics, Biome stops printing them.** It reports
  `The number of diagnostics exceeds the limit allowed. Use --max-diagnostics
  to increase it.` and `Diagnostics not shown: 9.`, **on stdout**. The
  `Found 28 errors` count still includes the hidden ones. **Decision:** the
  parser adds one `Info` draft naming the hidden count. Otherwise nine real
  failures would vanish without trace, which is the loss §5.3's fall-through
  exists to prevent. It is `Info`, not `Error`, because Biome does not say what
  the hidden diagnostics were. The shown errors already make the run's
  failure addressable.
- **Paths are relative to the directory Biome ran in,** which is the
  repository root for `npm run`. An absolute path gets no range, through the
  same `workspace_range` that Tasks 3 and 4 use. Task 7 resolves the rest.
- **Forced colour (`--colors=force`) changes more than rustc's colour does
  (§9).** Besides CSI colour codes, it wraps the category in an OSC 8
  hyperlink (`\x1b]8;;https://biomejs.dev/…\x1b\\`). It also swaps the
  markers for `✖`, `⚠` and `ℹ`. The runner pipes output, so neither normally
  appears. The parser strips both escape forms and accepts both marker sets.
- **Matching.** The parser matches a `biome` invocation (`check`, `lint`,
  `ci` or `format`), and `npm`/`pnpm`/`yarn run lint` or `run lint:*`. It
  cannot read `package.json` to learn whether a lint script really runs Biome.
  Matching by name is safe because of the fall-through: an ESLint
  `npm run lint` failure yields no Biome drafts, so it gets one generic
  finding, which is exactly what it would get with no parser. No shipped
  parser is displaced.
