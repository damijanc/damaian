# Feature Spec: Findings Model and Panel

Status: Done 2026-10-04. One redacted, addressable `Finding` type, four
parsers with a generic fallback, findings persisted in the session log, and a
panel to open, dismiss or ask for a fix of a selected subset; §7 records what
was built and what was left out. Split into a folder and planned on
2026-09-25. Design unchanged from the original flat spec; corrections where it no longer matches
the code — most importantly, §5.1's `Finding` has no field for the hash its own
staleness rule compares against, and its public fields contradict §5.6's
"no code path can create an unredacted finding" — are in
[`context.md`](context.md), not inlined here, the way spec 20 kept its own.
Read `context.md` before starting any task in [`tasks.md`](tasks.md).
Order: 22 of 23
Plan: `docs/PLAN/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 6 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#20](../20_working_modes/proposal.md) (the mode that gates tools) —
built; [#21](../21_task_plan_progress_and_budget/proposal.md) (plan state) —
built. Everything else named below is a cross-reference, not a prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.1 (chat
interface), section 7.10 (secret detection), section 11 (error handling).
Related implementation specs:
[`05_clickable_file_references.md`](../05_clickable_file_references.md) (navigation),
[`12_web_app_troubleshooting.md`](../12_web_app_troubleshooting/proposal.md) (the browser
diagnostic source, extended additively in §5.4), and
[`23_verification_loop.md`](../23_verification_loop.md), which consumes this model.

Implementation order note: the roadmap has WP3 (verification loop) requiring
"convert failures into structured findings (WP6)" while listing WP6 as depending
on WP3. The `Finding` type has to exist before the loop that produces them, so
this spec is numbered ahead of [`23_verification_loop.md`](../23_verification_loop.md).
The panel may follow the loop; the type may not.

## 1. Motivation

A failing check reaches the model as truncated, redacted prose.

`ValidationOrchestrator::run_proposal` returns a `CommandRunRecord` whose
`CommandExecution` holds `stdout`, `stderr`, and `exit_code`
(`crates/workspace-engine/src/command_runner.rs:11-22`). When `cargo test` fails,
what the agent gets is the tail of a text blob. There is no addressable failure,
no file and line, and no way for a user to say "fix this one" — because there is
no *this one* to point at. The same is true of a lint error, a compiler error, and
a browser console error, each arriving in a different shape and none of them
addressable.

This is why the work package is Must-tier despite producing no user-visible
capability on its own. Four later phases consume the model: Phase 3's LSP
diagnostics, Phase 4's hook findings, Phase 5's pull-request review findings,
Phase 6's subagent results. Defining it late means defining it four times, and
four incompatible definitions is the normal outcome.

It is also the prerequisite for [spec 23](../23_verification_loop.md). A repair loop
needs to know what to repair, and "the test output contained the word failed" is
not a repair target.

## 2. Current State

- **Check results are unstructured.** `CommandExecution` carries `exit_code:
  Option<i32>`, `stdout`, `stderr` (`command_runner.rs:11-22`). Failures reach the
  model through the existing truncation and redaction path as text.
- **Browser diagnostics are prose plus artifacts.** `WebDiagnosticReport` is
  `{ text: String, artifacts: Vec<WebDiagnosticArtifact>, is_error: bool }`
  (`crates/workspace-engine/src/web_diagnostics.rs:83`), with artifacts extracted
  from the text by `extract_artifacts_from_text`. A console error is a substring
  of `text`, and `is_error` is one boolean for a whole report that may contain
  several distinct problems.
- **`WebDiagnosticKind`** (`web_diagnostics.rs:8`) distinguishes the kind of
  diagnostic *call*, not the kind of problem found.
- **Nothing is addressable.** No type in the workspace represents "one problem, at
  this file and line, from this source, with this severity".
- **Clickable file references exist.**
  [Spec 05](../05_clickable_file_references.md) delivered in-text file references
  that open the file in the app or configured editor. This is the navigation
  mechanism to reuse.
- **Secret redaction is centralised.** `SecretScanner`, applied on the way into
  the audit log (`crates/workspace-engine/src/audit.rs:50`) and to command output.
- **Validation discovery exists.**
  `ValidationOrchestrator::propose_detected_validations`
  (`crates/workspace-engine/src/validation.rs:167`) finds project checks via
  `CommandPolicy::detect_project_commands`.

## 3. Requirements

1. One structured `Finding` type is shared by every source: compiler output, test
   failures, linter findings, security findings, browser console and network
   errors, code-review findings, and — additively in Phase 3 — LSP diagnostics.
2. Each finding carries source, severity, summary, details, file and range where
   applicable, task and tool references, status, and an optional fix action.
3. Findings support grouping, filtering, navigation to the referenced code,
   dismissal, and asking the agent to fix a selected subset.
4. Findings are redacted through `SecretScanner` before display or persistence.
5. Navigation reuses the clickable file references from
   [spec 05](../05_clickable_file_references.md).

## 4. Non-goals

- Writing a parser for every tool in existence. §5.3 defines a parser interface
  and ships parsers for the checks this repository actually uses, plus a
  generic fallback that never claims more structure than it found.
- Inventing file and line information. A finding whose source gave no location
  has none, and is displayed without one rather than attributed to a guess.
- Deduplicating findings across sources. A compiler error and an LSP diagnostic
  for the same line are two findings from two sources; merging them is a Phase 3
  question once LSP exists.
- Fixing findings. This spec makes a finding addressable and lets the user select
  a subset to fix; the repair loop is [spec 23](../23_verification_loop.md).
- Severity normalisation across tools into a single scale with comparable
  meaning. Severity is recorded as the source reported it, mapped onto a small
  fixed set, and §5.2 is explicit that cross-source comparison is not implied.
- Persisting findings beyond the session that produced them, or a
  cross-session findings history.
- Suppression rules, baselines, or "accepted finding" tracking.

## 5. Design

### 5.1 The type

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSource {
    Compiler,
    Test,
    Lint,
    Security,
    BrowserConsole,
    BrowserNetwork,
    CodeReview,
    /// Phase 3 WP3. Declared now so adding it is not a schema change.
    LanguageServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity { Error, Warning, Info }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus { Open, Dismissed, Fixed, Stale }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRange {
    pub path: String,
    pub start_line: u32,
    pub start_column: Option<u32>,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    pub source: FindingSource,
    pub severity: Severity,
    /// One line. Already redacted.
    pub summary: String,
    /// Bounded excerpt, already redacted. Never the whole tool output.
    pub details: Option<String>,
    pub range: Option<SourceRange>,
    pub task_id: Option<String>,
    /// The command execution, diagnostic call, or tool call this came from.
    pub origin_ref: Option<String>,
    pub status: FindingStatus,
    /// Machine-readable code where the source has one: `E0308`, `no-unused-vars`.
    pub code: Option<String>,
    pub created_at_ms: u128,
}
```

`FindingStatus::Stale` exists because a finding references a file at a moment.
Once that file changes, the finding may no longer apply, and displaying it as
open invites the agent to fix something that is already gone. Staleness is
computed by comparing the file's current hash against the hash recorded when the
finding was created — the same mechanism `patch_engine.rs:291` uses, reused
rather than reinvented.

`LanguageServer` is declared now, unused until Phase 3. A variant added later is
a serialised-enum change across persisted findings; a variant declared now costs
nothing.

### 5.2 Severity is recorded, not normalised

`Severity` has three values, and the mapping from each source is explicit and
documented per parser. What it does **not** claim is that a `cargo clippy`
warning and a browser console warning are equally important — they are both
`Warning` because each source called them that.

This is stated as a design position because the alternative is worse in a
specific way: a scoring scheme that ranked findings across sources would need a
judgement per tool per rule, would be wrong often, and would quietly decide what
the agent repairs first. Grouping is by source (§5.5), so the user compares like
with like.

### 5.3 Parsers

```rust
pub trait FindingParser {
    /// Source this parser produces.
    fn source(&self) -> FindingSource;
    /// Whether this parser recognises the output of the given command.
    fn matches(&self, command: &str) -> bool;
    /// Parse redacted output into findings. Returns empty when nothing parsed.
    fn parse(&self, execution: &CommandExecution) -> Vec<Finding>;
}
```

Shipped parsers cover the checks this repository runs, which are also the ones
`CommandPolicy::detect_project_commands` discovers:

| Parser | Recognises | Extracts |
|---|---|---|
| Rust diagnostics | `cargo build`, `cargo check`, `cargo clippy` | `error[E0308]: msg` plus the `--> path:line:col` line, severity from `error`/`warning`, code from the bracket |
| Rust test | `cargo test` | Failing test names from the `failures:` block; `panicked at path:line` where present |
| Biome | `biome check`, `npm run lint:web` | Path, line, column, rule name, severity |
| Generic | anything else that exited non-zero | **One** finding: severity `Error`, summary from the first non-empty stderr line, details a bounded tail, no range |

The generic fallback is the honest case and does most of the work in the wild. It
produces one finding for the whole failure rather than pretending to have found
individual problems, so a check Damaian does not understand is still addressable
as a unit and is never misrepresented as parsed.

A parser must not invent a location. If the output has no `path:line`, `range` is
`None`. Requirement 3's navigation is simply unavailable for that finding, which
is correct — the alternative is a click that opens the wrong line.

Ordering: the first parser whose `matches` returns true wins; the generic parser
matches last. A parser that returns zero findings for output that exited non-zero
falls through to the generic parser, so a parse failure degrades to a usable
finding instead of losing the failure entirely. That fall-through is the rule most
likely to be omitted, and without it a regex that stops matching after a tool
upgrade silently swallows every failure from that tool.

### 5.4 Browser findings need the runner to say more

> **Superseded in part, 2026-09-29.** Spec 12 is Done, and it built this
> structure as `WebDiagnosticReport.details: Option<WebDiagnosticDetails>`
> (page errors, console entries with source locations, failed requests), not
> as the `entries` field below. Findings come from `details`, and no
> `WebDiagnosticEntry` is added. See [`context.md`](context.md) §7.1 and Task 6.
> The option analysis below is kept as the reasoning; its shape is not built.

`WebDiagnosticReport` is `{ text, artifacts, is_error }`
(`web_diagnostics.rs:83`). Requirement 1 wants console and network errors as
individual findings, and a single boolean over a prose blob cannot supply them.

Two options, and this spec takes the second:

1. Parse `report.text` for console and network lines. Cheap, and wrong for the
   same reason the generic parser is a fallback rather than a solution: the text
   is written for a human and its shape is not a contract.
2. **Extend the report with structured entries.**
   [Spec 12](../12_web_app_troubleshooting/proposal.md) is `In progress`, so its runner
   contract is still being settled — this is the moment to add structure to it
   rather than parse around it afterwards.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDiagnosticEntry {
    pub kind: WebEntryKind,       // ConsoleError, ConsoleWarning, NetworkError, PageError
    pub message: String,
    pub url: Option<String>,
    pub status: Option<u16>,
    /// Source location when the browser reported one.
    pub source: Option<SourceRange>,
}
```

`WebDiagnosticReport` gains `entries: Vec<WebDiagnosticEntry>`, additive, with
`is_error` and `text` unchanged so nothing existing breaks. Findings come from
`entries`; a report with no entries and `is_error: true` produces one generic
finding from `text`, mirroring §5.3's fallback.

A browser source location maps to a `SourceRange` only when it resolves to a
repository path. A `SourceRange` pointing at a bundled URL or a `node_modules`
path is dropped rather than recorded, since clicking it would go nowhere useful.

### 5.5 Panel, navigation, and scoped repair

The findings panel groups by source, then by file, and filters by severity and
status. Default view is `Open` findings of severity `Error`, because a panel that
opens showing forty warnings is a panel users close.

Navigation reuses [spec 05](../05_clickable_file_references.md)'s mechanism: a
finding with a `range` renders as a clickable reference that opens the file at the
line in the app or the configured editor. No new navigation path is added.

Requirement 3's "ask the agent to fix a selected subset" produces a scoped repair
request carrying the selected finding IDs and their current ranges. Two rules:

- A finding that has gone `Stale` is excluded from the selection with a note, not
  silently included. Repairing against a range that no longer exists is how an
  agent edits the wrong lines.
- The repair request carries the findings, not the raw tool output. This is the
  point of the whole model: the agent is asked to fix three specific things
  rather than handed a log and asked to interpret it.

Dismissal sets `Dismissed` and is per finding, session-scoped. Dismissal is not
suppression: the same problem found by a later check is a new finding, because
suppression rules are a non-goal and a dismissed-forever finding is how real
problems get buried.

### 5.6 Redaction

Requirement 4 is satisfied at construction, not at display: `Finding::new`
redacts `summary` and `details` through `SecretScanner` before the value is
stored, so no code path can create an unredacted finding and no display path has
to remember to redact.

This mirrors `AuditLog::record` (`audit.rs:50`), which redacts every field value
on the way in. The reason to put it at construction rather than at the panel is
that findings also travel to the model, to the completion report, and — in later
phases — into pull-request comments, and each of those would otherwise need its
own redaction call.

`details` is a bounded excerpt, never full tool output. Full output already has a
home: `CommandStore::save_execution` writes `stdout.log` and `stderr.log`
(`validation.rs:63-90`), and `origin_ref` points at it. The roadmap's data rule —
store a reference and a bounded excerpt — is followed rather than duplicating
output into every finding.

### 5.7 Persistence

Findings are appended to the session log per
[spec 17](../17_durable_task_state_and_crash_recovery/proposal.md) §5.2:

```json
{"seq":260,"eventType":"finding_recorded","taskId":"task_…","finding":{…}}
{"seq":277,"eventType":"finding_status_changed","findingId":"finding_…",
 "status":"fixed"}
```

`SessionStore::read_findings(session_id)` replays them, newest status per ID
winning. Findings therefore survive restart, which
[spec 21](../21_task_plan_progress_and_budget/proposal.md) needs — a step blocked by a finding
must still be blocked by it after a crash.

### 5.8 Documentation

`docs/USER_GUIDE.md`: what the panel shows, how to filter, how to ask for a fix,
what stale means and why a stale finding is not repaired. `docs/TROUBLESHOOTING.md`:
which checks are parsed structurally versus generically, how to tell a
generic-fallback finding from a parsed one, and where full output lives.

## 6. Acceptance Criteria

- A failing test, a lint error, and a browser console error all normalise into
  `Finding` with correct source, severity, and — where the tool reported one —
  file and range.
- A check with no parser exits non-zero and produces exactly one generic finding
  carrying a usable summary, never zero findings.
- A parser that recognises a command but extracts nothing falls through to the
  generic parser rather than losing the failure — asserted by test.
- No parser produces a `range` for output that contained no location.
- A browser diagnostic with structured entries produces one finding per entry; one
  with `is_error: true` and no entries produces a single generic finding.
- A browser source location outside the repository is dropped rather than recorded
  as a range.
- Clicking a finding with a range opens the referenced file and line through the
  existing [spec 05](../05_clickable_file_references.md) mechanism.
- Selecting findings and asking for a fix produces a scoped repair request
  carrying finding IDs, excluding stale findings with a note.
- A finding becomes `Stale` when its file's hash no longer matches the hash
  recorded at creation.
- No finding displays, persists, or transmits an unredacted secret — asserted with
  a seeded fake secret in command output, browser output, and a generic fallback.
- `details` is bounded, and full output remains reachable through `origin_ref`.
- Findings survive a restart with their statuses intact.
- Dismissing a finding does not suppress the same problem found by a later check.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

Built in twelve tasks between 2026-09-25 and 2026-10-04.
[`tasks.md`](tasks.md)'s Progress table has the per-task detail, including the
mutation runs. [`context.md`](context.md) has every correction to §§1–6. This
section is the summary.

The two questions this section was asked to answer:

- **Which parsers shipped, and how many real failures fell through.** Four
  shipped: Rust diagnostics (`cargo build`/`check`/`clippy`), Rust test
  (`cargo test`, which runs the diagnostics parser over the same output
  first), Biome (`biome …` and `npm`/`pnpm`/`yarn run lint`/`lint:*`), and
  browser findings from spec 12's `WebDiagnosticDetails`. They sit beside the
  generic fallback, whose source is `Command` (`context.md` §8.1). The share
  rests on **the failures captured while building**, not on the eval tier.
  Task 12's deterministic run recorded eight findings, all generic, all from
  `failed_validation_retry`'s eight `ls no-such-directory` rounds. `ls` has no
  parser and none was intended, so the tier says nothing about how often a
  parsed tool falls through (§7.3). The captured sample is small. Of eight real failing runs from tools in scope, **two fell through
  (25%)**:
  - **parsed:** a `cargo build` with errors, `cargo clippy -D warnings`, a
    `cargo test` that failed to compile, a `cargo test --no-fail-fast` with
    eight failures across lib, integration and doctest, a failing Biome run
    (`npm run lint:web` on seeded problems), and spec 12's companion report;
  - **generic:** a `cargo test` whose binary died of `SIGABRT` after a warning
    (§10.3), and `cargo nextest run` (§10.5).

  Task 8's `ls no-such-directory` is a third generic finding, but `ls` has no
  parser and none was intended. The next parser worth adding is **nextest**,
  because this repository's own gate uses it. `context.md` §10.5 sketches what
  it would read.
- **`WebDiagnosticReport.entries`** was never added. Spec 12 closed first, on
  2026-09-29, and its close-out built the structure as
  `WebDiagnosticReport.details: Option<WebDiagnosticDetails>`. Findings come
  from `details`. The text-parsing fallback §5.4 rejected was not used either
  (`context.md` §7.1, §12).

### 7.1 Where each part lives

- **The type:** `crates/workspace-engine/src/finding.rs`. Every field is
  private. `Finding::new(draft, &scanner)` is the only constructor, and it
  redacts, then bounds: 240 characters of summary, 4096 bytes of details.
  Builders attach the task, the origin and the file hash, and
  `without_range` drops a range. None of them takes free text (`context.md`
  §§1–4).
- **Parsers** return `FindingDraft`s, never `Finding`s. One dispatcher,
  `findings_from_execution`, turns drafts into findings. It falls through to
  the generic finding when a failed run has no `Error` draft (§10.3), and
  never for a cancelled run (§8.2). Each parser has its own file under
  `src/finding/`, with captured fixtures.
- **Browser:** `finding/browser.rs`, `findings_from_web_record`. A served URL
  maps to a range only when exactly one repository file matches (§12.3).
- **Persistence:** `SessionStore::record_finding`, `set_finding_status` and
  `read_findings` (`session.rs`), in two events, `finding_recorded` and
  `finding_status_changed`. `Stale` is derived on read and never stored.
- **Recording:** `chat.rs`, at the sandbox auto-run, the approval resume and
  `run_and_record_web_diagnostic`. A range survives only if it names a file in
  the repository, and the file is hashed at that moment. The sandbox path
  attaches `Evidence::Findings` beside its `CommandExit`.
- **Repair:** `finding/repair.rs`, `RepairRequest`. Only `Open` findings are
  kept. Stale, dismissed, fixed and unknown ids are excluded, each with its
  reason (§15).
- **Shell:** `GET /api/findings`, `POST /api/finding-status` and
  `POST /api/findings-repair` (§16). **Panel:** `#findings-panel` in the web
  UI, with "Fix selected" sent as an agentic turn that bypasses the
  edit-request heuristic (§17).

### 7.2 Acceptance criteria (§6), each with its evidence

Every test named below was checked to exist with `cargo nextest list` on
2026-10-04.

| Criterion | Evidence | Verdict |
|---|---|---|
| A failing test, a lint error and a browser console error normalise with source, severity and, where reported, range | `finding::rust_test::tests::a_panic_takes_its_location_and_message`, `a_failure_in_an_integration_test_binary_is_found_too`; `finding::biome::tests::a_lint_error_keeps_its_rule_location_and_message`; `finding::browser::tests::a_console_error_at_a_served_url_maps_to_the_one_repository_file`. End to end: `finding_recording::a_browser_console_error_is_recorded_with_its_mapped_range_and_record` and `an_approved_command_records_findings_with_checked_and_hashed_ranges` | Met |
| A check with no parser yields exactly one generic finding, never zero | `finding::dispatch_tests::a_failure_no_parser_matches_yields_exactly_one_generic_finding`, `a_timeout_is_a_failure_even_without_an_exit_code`, `a_signal_kill_is_a_failure`; end to end, `finding_recording::a_failed_sandbox_command_is_recorded_with_its_task_and_reachable_output` | Met |
| A parser that recognises a command but extracts nothing falls through | `finding::dispatch_tests::a_matching_parser_that_extracts_nothing_falls_through_to_generic`, `a_failure_whose_parser_found_only_warnings_also_gets_the_generic_finding`, and each parser's own: `rust_diagnostics::tests::a_failed_build_this_parser_cannot_read_falls_through_to_generic`, `rust_test::tests::a_failed_cargo_test_this_parser_cannot_read_falls_through_to_generic`, `a_crashed_test_binary_with_a_warning_still_gets_the_generic_finding`, `biome::tests::a_failed_lint_script_that_is_not_biome_falls_through_to_generic` | Met |
| No range for output with no location | the generic finding's `range` in `a_failure_no_parser_matches_…`; `rust_diagnostics::tests::a_diagnostic_without_a_location_has_no_range`; `biome::tests::a_format_diagnostic_has_no_range`; `browser::tests::a_page_error_is_a_console_error_without_a_location`; `rust_test::tests::a_failure_whose_section_was_cut_off_keeps_its_name_and_no_range` | Met |
| Browser: one finding per entry; a failed tool with nothing to convert gives one generic finding | Restated against `WebDiagnosticDetails`, because `entries` was never built (`context.md` §7.1): `browser::tests::an_inspection_yields_one_finding_per_problem_in_report_order`, `a_runner_that_failed_yields_one_generic_finding`, `a_non_companion_runner_failure_yields_one_generic_finding_from_its_text` | Met, as restated |
| A browser location outside the repository is dropped | `browser::tests::a_console_location_on_another_origin_has_no_range`, `an_ambiguous_served_path_has_no_range`, `a_bundled_url_with_no_repository_file_has_no_range`; recording's file check in `finding_recording::an_approved_command_…` (`src/gone.rs` loses its range) | Met |
| Clicking a ranged finding opens the file through spec 05 | Task 11's browser walk-through, item (5): the location is spec 05's `button.file-reference` with `data-path`, `data-line` and `data-col`, wired by the existing `wireFileReferences`. It was **not clicked**, because that launches VS Code. There is no JS test suite | Met by reuse, verified by attribute |
| Fix selected produces a scoped request with ids, excluding stale with a note | `finding::repair::tests::*` (9 tests), `finding_recording::a_finding_made_stale_by_an_edit_is_excluded_when_the_request_is_built`, the shell's `post_findings_repair_returns_the_request_and_its_prompt`, and Task 11's walk-through, item (8) | Met |
| Stale when the file hash no longer matches | `session::tests::a_changed_file_makes_its_finding_stale`, `a_deleted_file_makes_its_finding_stale`, `staleness_is_derived_on_read_so_a_reverted_file_reopens_its_finding`; end to end in `finding_recording::an_approved_command_…` | Met |
| No finding displays, persists or transmits an unredacted secret: command output, browser output, generic fallback | **Construction:** `finding::tests::new_redacts_a_secret_in_summary_and_details`, `a_secret_straddling_the_details_bound_is_redacted_not_cut`. **Generic fallback:** `dispatch_tests::a_secret_in_failed_output_is_redacted_in_the_generic_finding`. **Browser:** `browser::tests::a_secret_in_browser_output_is_redacted`. **Persistence, added by Task 12:** `finding_recording::a_secret_in_a_failed_command_is_not_in_the_recorded_finding` reads the stored `finding_recorded` line for a command whose secret reaches the generic summary. Spec 12's `chat::…::a_web_diagnostic_is_recorded_redacted_and_streamed` now asserts that its page error was recorded as a finding, so its whole-log check covers persisted browser findings. **Transmission:** the repair prompt renders from these findings, and nothing else | Met |
| `details` bounded; full output reachable through `origin_ref` | `finding::tests::details_are_bounded_on_a_char_boundary`, `details_within_the_bound_are_kept_verbatim`; `finding_recording::a_failed_sandbox_command_is_recorded_with_its_task_and_reachable_output` reads `stderr.log` under `origin_ref` | Met |
| Findings survive a restart with their statuses intact | `session::tests::findings_and_their_statuses_survive_a_new_store_over_the_same_data_dir` | Met |
| Dismissing does not suppress a later check's finding | `finding_recording::dismissing_a_finding_does_not_suppress_the_same_problem_from_a_later_check` | Met |
| Every quality-gate command passes | [`tasks.md`](tasks.md), Task 12's row | See that row |

### 7.3 Known gaps, named rather than implied

- **A command run outside a conversation records no findings.** The
  standalone `/api/run-command` has no session or task (`context.md` §13.4).
- **The approval-resume path attaches no plan evidence.** It records findings
  but attaches neither `CommandExit` nor `Findings`. The missing `CommandExit`
  predates this spec (§14).
- **`cargo nextest run` gets one generic finding**, compile errors included
  (§10.5).
- **Percent-encoded served paths never map to a file** (§12.3).
- **The eval harness has no scenario whose failing check has a parser.** Its
  one failing check is `ls`, so it cannot measure the generic share for parsed
  tools. That is why §7's share rests on captured failures.
- **Shell routes that hold a `WorkspaceEngine` must be functions,** not
  inline arms of `handle_connection`. Inline, their locals share one frame,
  and a debug build overflowed the default thread stack (Task 10, deviation
  1). Nothing in the shell enforces this. A comment above the arms says why.
