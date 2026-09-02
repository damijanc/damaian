# Feature Spec: Findings Model and Panel

Status: Not started
Order: 22 of 23
Roadmap: `docs/ROADMAP/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 6 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.1 (chat
interface), section 7.10 (secret detection), section 11 (error handling).
Related implementation specs:
[`05_clickable_file_references.md`](05_clickable_file_references.md) (navigation),
[`12_web_app_troubleshooting.md`](12_web_app_troubleshooting.md) (the browser
diagnostic source, extended additively in §5.4), and
[`23_verification_loop.md`](23_verification_loop.md), which consumes this model.

Implementation order note: the roadmap has WP3 (verification loop) requiring
"convert failures into structured findings (WP6)" while listing WP6 as depending
on WP3. The `Finding` type has to exist before the loop that produces them, so
this spec is numbered ahead of [`23_verification_loop.md`](23_verification_loop.md).
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

It is also the prerequisite for [spec 23](23_verification_loop.md). A repair loop
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
  [Spec 05](05_clickable_file_references.md) delivered in-text file references
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
   [spec 05](05_clickable_file_references.md).

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
  a subset to fix; the repair loop is [spec 23](23_verification_loop.md).
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

`WebDiagnosticReport` is `{ text, artifacts, is_error }`
(`web_diagnostics.rs:83`). Requirement 1 wants console and network errors as
individual findings, and a single boolean over a prose blob cannot supply them.

Two options, and this spec takes the second:

1. Parse `report.text` for console and network lines. Cheap, and wrong for the
   same reason the generic parser is a fallback rather than a solution: the text
   is written for a human and its shape is not a contract.
2. **Extend the report with structured entries.**
   [Spec 12](12_web_app_troubleshooting.md) is `In progress`, so its runner
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

Navigation reuses [spec 05](05_clickable_file_references.md)'s mechanism: a
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
[spec 17](17_durable_task_state_and_crash_recovery.md) §5.2:

```json
{"seq":260,"eventType":"finding_recorded","taskId":"task_…","finding":{…}}
{"seq":277,"eventType":"finding_status_changed","findingId":"finding_…",
 "status":"fixed"}
```

`SessionStore::read_findings(session_id)` replays them, newest status per ID
winning. Findings therefore survive restart, which
[spec 21](21_task_plan_progress_and_budget.md) needs — a step blocked by a finding
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
  existing [spec 05](05_clickable_file_references.md) mechanism.
- Selecting findings and asking for a fix produces a scoped repair request
  carrying finding IDs, excluding stale findings with a note.
- A finding becomes `Stale` when its file's hash no longer matches the hash
  recorded at creation.
- No finding displays, persists, or transmits an unredacted secret — asserted with
  a seeded fake secret in command output, browser output, and a generic fallback.
- `details` is bounded, and full output remains reachable through `origin_ref`.
- Findings survive a restart with their statuses intact.
- Dismissing a finding does not suppress the same problem found by a later check.
- The five quality-gate commands from `AGENTS.md` pass.

## 7. Implementation Notes

To be completed during implementation. Record:

- Which parsers were shipped, and the share of real failures during testing that
  fell through to the generic parser. A high share is not a failure of this spec,
  but it tells the next person where to add a parser.
- Whether `WebDiagnosticReport.entries` was added in coordination with
  [spec 12](12_web_app_troubleshooting.md), or whether that spec had already
  closed and the text-parsing fallback was used instead.
