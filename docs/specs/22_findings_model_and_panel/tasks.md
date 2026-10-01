# Findings Model and Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
decisions it left open in [`context.md`](context.md)
**Started:** 2026-09-25 — **Done:** —

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `Finding`, `FindingDraft`, and the redacting, bounding constructor | Done 2026-09-30 | `finding.rs` landed as sketched (rustfmt only), registered in `lib.rs`. Before the implementation the tests failed to compile (`E0432`). After it, `test(finding::tests)` passed 16/16. Each mutation failed its own test: (1) bound-then-redact failed `a_secret_straddling_…`; (2) no summary redaction failed `new_redacts_…`; (3) `<=`→`<` failed `a_summary_at_exactly_the_bound_…`; (4) a plain byte slice failed `details_are_bounded_on_a_char_boundary` by panicking. Privacy: a struct literal gives `E0451`, and a field read gives `E0616`. Probe these one at a time, because rustc reports `E0616` and stops before `E0451`, so a combined probe shows only one of them. Scoped fmt, clippy `-p workspace-engine` and typos are clean. |
| 2 · Parser trait, dispatch with fall-through, generic parser | Done 2026-09-30 | Landed as sketched (rustfmt only): `FindingSource::Command`, `FindingParser`, `default_parsers()` (still empty), `findings_from_execution`, the generic fallback, and `GENERIC_DETAIL_LINES`. `bound_summary` now calls `first_non_empty_line`. Before the implementation the dispatch tests failed to compile (`E0405`/`E0425`). After it, `test(finding::tests) + test(finding::dispatch_tests)` passed 32/32. Each mutation failed its named test: (1) generic only when no parser matched failed only `a_matching_parser_that_extracts_nothing_falls_through_…`, while `a_failure_no_parser_matches_…` still passed; (2) `(Exited, None)`→`Passed` failed `a_signal_kill_is_a_failure`; (3) `Cancelled`→`Failed` failed `a_cancelled_execution_gets_no_generic_finding`; (4) head-not-tail `last_lines` failed `the_generic_details_keep_the_last_lines_…`; (5) dropping the verdict check failed `a_passing_execution_with_no_parser_yields_nothing`, plus the cancelled test and `a_passing_execution_whose_parser_finds_nothing_…`. Parser layout follows `context.md` §8.4: one file per parser under `src/finding/`. Scoped fmt, clippy `-p workspace-engine` and typos are clean. |
| 3 · Rust diagnostics parser (`cargo build`/`check`/`clippy`) | Done 2026-10-01 | Planned on 2026-09-30 against captured cargo 1.98.0 output (`context.md` §9). The first session was lost after Step 1, and its fixtures were checked byte-identical before work resumed. `finding/rust_diagnostics.rs` landed as sketched, with only rustfmt changes. It is declared from `finding.rs`, and `RustDiagnosticsParser` is registered in `default_parsers()`. Before the implementation the tests failed to compile (`E0425` on `parse_rust_diagnostics`, `cargo_subcommand` and `RustDiagnosticsParser`). After it, `test(finding::)` passed 49/49 (16 + 16 + 17), and the Task 2 dispatch tests were unaffected. Mutations: (1) no `take_while` failed only `a_child_notes_location_is_not_borrowed_…`, as predicted. (2) The plan's two variants differ. The last `-->` *before a child header* fails nothing, because rustc prints exactly one `-->` per primary region (a secondary span in another file uses `:::`), so first and last are the same line. The last `-->` *in the whole block* failed `the_location_is_the_primary_one_…` with `13:4`, and also the no-primary-location test. (3) `is_cargo_summary` always `false` failed `cargo_summary_lines_are_not_findings`, plus `every_clippy_occurrence_…`, `default_parsers_turn_…` and `a_diagnostic_without_a_location_…`, whose counts include the summary lines. (4) No `CLIPPY_URL` branch failed `every_clippy_occurrence_gets_its_code_from_the_help_url`. (5) No `is_absolute` failed `a_location_outside_the_workspace_has_no_range`. (6) No ANSI strip failed `ansi_colour_codes_are_ignored`. (7) Ending a block only at a blank line failed nothing, as expected. The header check stays as defence against a runner that strips blank lines. Scoped fmt, clippy `-p workspace-engine`, `test(finding::)` and typos are clean. typos needed no fixture exclusion. |
| 4 · Rust test parser (`cargo test`), delegating compile errors to Task 3 | Not started | |
| 5 · Biome parser | Not started | |
| 6 · Browser findings from spec 12's `WebDiagnosticDetails` | Not started | Re-scoped 2026-09-29 by spec 12's close-out: no `entries` field (`context.md` §7.1). |
| 7 · Recording, persistence, staleness, `Evidence::Findings` | Not started | |
| 8 · Dismissal and the scoped repair request | Not started | |
| 9 · Shell API | Not started | |
| 10 · Findings panel | Not started | |
| 11 · Docs, acceptance criteria, close the spec | Not started | |

**Goal:** One structured, redacted, addressable `Finding` type shared by every
check source, with parsers that degrade to one honest generic finding rather
than losing a failure, persisted in the session log, and surfaced in a panel
from which a user can navigate to, dismiss, or ask the agent to fix a
selected subset.

**Architecture:** A new `workspace-engine` module, `finding.rs`, owns the type.
Its only constructor redacts and bounds (Task 1). Parsers produce plain
`FindingDraft`s and never construct a `Finding` themselves. One dispatcher
turns drafts into findings, so redaction happens at exactly one point
(`context.md` §3). Recording attaches the task, origin and file hash, then
appends to the spec 17 session log. A replay reader derives status, including
`Stale`, from that log (Task 7). The shell and panel are thin views over the
reader (Tasks 9–10).

**Tech Stack:** Rust 2024, `serde`, and `regex` (all already dependencies of
`workspace-engine`, so nothing new is added). The panel is vanilla JS in
`app.js`.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Four of its
  decisions change the proposal's sketches: §1 adds a `file_hash` field, §2
  makes the fields private, §3 changes the parser trait to return drafts, and
  §4 fixes the order of redaction and bounding. Do not re-derive these from
  `proposal.md` alone.
- **One construction point.** Only `Finding::new` creates a `Finding` (apart
  from deserialising the session log, per `context.md` §2). If a task needs a
  finding built another way, it adds a builder to `finding.rs`. It does not
  add a second constructor.
- **Never invent a location** (§5.3). `range` is `None` unless the tool
  printed a `path:line`. `file_hash` is `None` unless the file was read
  (`context.md` §1).
- **Never lose a failure** (§5.3). An execution that failed and produced zero
  parsed drafts falls through to the generic parser. That covers a non-zero
  exit, a timeout, and a signal (`context.md` §7). A cancellation is not a
  failure and never gets a generic finding (`context.md` §8.2). The
  fall-through is the rule the proposal calls "most likely to be omitted",
  so every parser task tests it against its own parser.
- **Severity is recorded, not normalised** (§5.2). Each parser documents its
  mapping in a doc comment. Nothing ranks findings across sources.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  then revert, and record the mutation in the progress row. This repository has
  a history of tests that passed without testing anything (spec 47 §7.2,
  spec 21's error rate).
- **Scope per-task checks; run the full seven-command gate from `AGENTS.md`
  once, in Task 11.** `cargo nextest run --workspace` takes about 5 minutes
  locally, and `cargo clippy --workspace --all-targets` takes up to 18
  minutes cold.
- **`chat.rs` is contended.** See `docs/specs/README.md` "What to build next"
  → Parallel work. Tasks 1–6 and 8 do not touch it. Task 7 may, where a check
  runs inside a turn. Check that section before running Task 7 alongside
  another spec.
- **Never `git commit` unasked.** Each task ends by showing the change and the
  scoped check results, then asking. When asked, write one subject line with no
  body.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/finding.rs` | New: the types, `FindingDraft`, `Finding::new` (Task 1); `FindingParser`, dispatch, and the generic parser (Task 2) |
| `crates/workspace-engine/src/finding/` or inline modules | Rust diagnostics, Rust test, and Biome parsers (Tasks 3–5). Task 2 decides between a submodule directory and inline `mod`s, and records the choice |
| `crates/workspace-engine/src/lib.rs` | `pub mod finding;` (Task 1) |
| `crates/workspace-engine/src/web_diagnostics.rs` | Findings from a report's `WebDiagnosticDetails`, which spec 12 already defines (Task 6) |
| `crates/workspace-engine/src/session.rs` | `finding_recorded` and `finding_status_changed` events, and `read_findings` (Task 7) |
| `crates/workspace-engine/src/plan.rs` | `Evidence::Findings` (Task 7, closing spec 21's deferral) |
| `crates/workspace-engine/src/validation.rs` (and `chat.rs` if needed) | The recording call sites (Task 7) |
| `crates/desktop-shell/src/lib.rs` | Findings endpoints (Task 9) |
| `crates/desktop-shell/static/app.js`, `styles.css` | The panel (Task 10) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | §5.8 (Task 11) |

## Interface reference

- `SecretScanner::redact(&self, &str) -> Redaction { text, findings }`
  (`secret_scanner.rs:42`). Placeholders look like
  `[REDACTED_AWS_ACCESS_KEY_<10 hex>]`.
  `SecretScanner::default()` has no custom patterns and is what unit tests
  use.
- `hash::create_id(prefix) -> String`, `hash::now_millis() -> u128`,
  `hash::sha256(bytes) -> "sha256:<hex>"`, `hash::file_hash(path)`
  (`hash.rs:104-138`).
- `CommandExecution { id, command, exit_code, termination, stdout, stderr, .. }`
  (`command_runner.rs:16-32`). The output is already tail-truncated and
  redacted (`context.md` §6).
- `CommandRunRecord { proposal_id, execution, stdout_ref, .. }`
  (`validation.rs:30`). This is where Task 7 gets `origin_ref`.
- `WebDiagnosticReport { text, artifacts, is_error }`
  (`web_diagnostics.rs:83`), which Task 6 extends.
- `SessionStore::create_plan` / `append_plan` (`session.rs:1112`, and the
  `serde_json::to_value` at `:1162`). This is the serde-into-session-log
  pattern Task 7 follows.

---

## Task 1: `Finding`, `FindingDraft`, and the redacting, bounding constructor

**Requirements:** 2, 4 (and the type half of 1). **Files:** create
`crates/workspace-engine/src/finding.rs`, modify
`crates/workspace-engine/src/lib.rs`.

This task stands alone. It adds the types and their one constructor, and
nothing calls them yet. It adds no parsers, persistence, filesystem access, or
changes to any other module. Because the module is `pub` and the items are
`pub`, `clippy -D warnings` raises no dead-code warnings, so this task needs no
`#[allow(dead_code)]`. Spec 20 needed several, because its items were
`pub(crate)`.

**Why `pub` and not `pub(crate)`:** `desktop-shell` is a separate crate and
reads findings in Task 9. Spec 20 Task 8 had to widen `pub(crate)` items
after the fact, so this task starts at `pub`.

**Interfaces:**
- Consumes: `SecretScanner` (`secret_scanner.rs`), and `create_id` /
  `now_millis` (`hash.rs`). Neither is modified.
- Produces, and every later task relies on these names:
  - `pub enum FindingSource { Compiler, Test, Lint, Security, BrowserConsole, BrowserNetwork, CodeReview, LanguageServer }`
    with `pub fn as_str(self) -> &'static str`, which returns the serde form
    (`"browser_console"`).
  - `pub enum Severity { Error, Warning, Info }`, which is `Ord`, with `Error`
    sorting first.
  - `pub enum FindingStatus { Open, Dismissed, Fixed, Stale }`
  - `pub struct SourceRange { pub path, pub start_line, pub start_column, pub end_line, pub end_column }`,
    as in proposal §5.1.
  - `pub struct FindingDraft { pub source, pub severity, pub summary: String, pub details: Option<String>, pub range: Option<SourceRange>, pub code: Option<String> }`
  - `pub struct Finding`, with private fields and these methods:
    - `Finding::new(draft: FindingDraft, scanner: &SecretScanner) -> Finding`
    - builders `with_task_id(self, impl Into<String>) -> Self`,
      `with_origin_ref(self, impl Into<String>) -> Self`, and
      `with_file_hash(self, impl Into<String>) -> Self`
    - getters `id`, `source`, `severity`, `summary`, `details`, `range`,
      `task_id`, `origin_ref`, `status`, `code`, `file_hash`, and
      `created_at_ms`
    - `set_status(&mut self, FindingStatus)`
  - `pub const MAX_SUMMARY_CHARS: usize = 240`,
    `pub const MAX_DETAILS_BYTES: usize = 4096`, and
    `pub const DETAILS_TRUNCATION_MARKER: &str = "\n… (truncated)"`.
  - The JSON shape: camelCase keys (`taskId`, `originRef`, `fileHash`,
    `createdAtMs`, `startLine`, …) and snake_case enum values. Task 7
    persists exactly this shape, and Task 9 serves it.

- [x] **Step 1: Register the module**

  In `crates/workspace-engine/src/lib.rs`, add `pub mod finding;` in
  alphabetical order, between `pub mod file_access;` and `pub mod git_service;`.

- [x] **Step 2: Write the failing tests**

  Create `crates/workspace-engine/src/finding.rs` containing only the test
  module, so that it fails to compile:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::secret_scanner::SecretScanner;

      const FAKE_AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

      fn draft(summary: &str, details: Option<&str>) -> FindingDraft {
          FindingDraft {
              source: FindingSource::Test,
              severity: Severity::Error,
              summary: summary.to_string(),
              details: details.map(str::to_string),
              range: None,
              code: None,
          }
      }

      #[test]
      fn a_new_finding_is_open_with_a_finding_id_and_no_references() {
          let finding = Finding::new(draft("it broke", None), &SecretScanner::default());
          assert!(finding.id().starts_with("finding_"), "{}", finding.id());
          assert_eq!(finding.status(), FindingStatus::Open);
          assert_eq!(finding.source(), FindingSource::Test);
          assert_eq!(finding.severity(), Severity::Error);
          assert_eq!(finding.task_id(), None);
          assert_eq!(finding.origin_ref(), None);
          assert_eq!(finding.file_hash(), None);
          assert!(finding.created_at_ms() > 0);
      }

      #[test]
      fn two_findings_from_the_same_draft_have_different_ids() {
          let scanner = SecretScanner::default();
          let a = Finding::new(draft("same", None), &scanner);
          let b = Finding::new(draft("same", None), &scanner);
          assert_ne!(a.id(), b.id());
      }

      /// Requirement 4, at construction (proposal §5.6).
      #[test]
      fn new_redacts_a_secret_in_summary_and_details() {
          let finding = Finding::new(
              draft(
                  &format!("login failed with key {FAKE_AWS_KEY}"),
                  Some(&format!("request\nAuthorization key: {FAKE_AWS_KEY}\nend")),
              ),
              &SecretScanner::default(),
          );
          assert!(!finding.summary().contains(FAKE_AWS_KEY), "{}", finding.summary());
          assert!(finding.summary().contains("[REDACTED_"), "{}", finding.summary());
          let details = finding.details().expect("details kept");
          assert!(!details.contains(FAKE_AWS_KEY), "{details}");
          assert!(details.contains("[REDACTED_"), "{details}");
      }

      /// `context.md` §4: redact, *then* bound. The key starts six bytes
      /// before the bound. Bounding first would keep `AKIAIO`, too short for
      /// the AWS rule to match, and leave it in plain text. The filler is
      /// spaces so no other scanner rule can swallow the key and hide the
      /// ordering bug.
      #[test]
      fn a_secret_straddling_the_details_bound_is_redacted_not_cut() {
          let details = format!("{}{FAKE_AWS_KEY}", " ".repeat(MAX_DETAILS_BYTES - 6));
          let finding =
              Finding::new(draft("s", Some(&details)), &SecretScanner::default());
          let kept = finding.details().expect("details kept");
          assert!(!kept.contains("AKIA"), "a fragment of the key survived: {:?}",
              &kept[kept.len().saturating_sub(40)..]);
          assert!(kept.ends_with(DETAILS_TRUNCATION_MARKER));
      }

      #[test]
      fn summary_is_the_first_non_empty_line_trimmed() {
          let finding = Finding::new(
              draft("\n   \n  error: first real line  \nsecond line", None),
              &SecretScanner::default(),
          );
          assert_eq!(finding.summary(), "error: first real line");
      }

      #[test]
      fn a_long_summary_is_cut_to_the_bound_with_an_ellipsis() {
          let long = "é".repeat(MAX_SUMMARY_CHARS + 50);
          let finding = Finding::new(draft(&long, None), &SecretScanner::default());
          assert_eq!(finding.summary().chars().count(), MAX_SUMMARY_CHARS);
          assert!(finding.summary().ends_with('…'));
      }

      #[test]
      fn a_summary_at_exactly_the_bound_is_not_cut() {
          let exact = "a".repeat(MAX_SUMMARY_CHARS);
          let finding = Finding::new(draft(&exact, None), &SecretScanner::default());
          assert_eq!(finding.summary(), exact);
      }

      /// A finding is never blank, even when the source printed nothing.
      #[test]
      fn an_empty_summary_becomes_a_message_naming_the_source() {
          let mut blank = draft("  \n\t\n", None);
          blank.source = FindingSource::BrowserConsole;
          let finding = Finding::new(blank, &SecretScanner::default());
          assert_eq!(finding.summary(), "browser_console finding with no message");
      }

      #[test]
      fn details_are_bounded_on_a_char_boundary() {
          // 3-byte chars, so MAX_DETAILS_BYTES is unlikely to land on a boundary.
          let details = "€".repeat(MAX_DETAILS_BYTES);
          let finding =
              Finding::new(draft("s", Some(&details)), &SecretScanner::default());
          let kept = finding.details().expect("details kept");
          assert!(kept.len() <= MAX_DETAILS_BYTES + DETAILS_TRUNCATION_MARKER.len());
          let body = kept.strip_suffix(DETAILS_TRUNCATION_MARKER).expect("marked as cut");
          assert!(body.chars().all(|c| c == '€'), "the cut split a character");
      }

      #[test]
      fn details_within_the_bound_are_kept_verbatim() {
          let details = "line one\n  line two\n";
          let finding =
              Finding::new(draft("s", Some(details)), &SecretScanner::default());
          assert_eq!(finding.details(), Some(details));
      }

      #[test]
      fn blank_details_become_none() {
          let finding =
              Finding::new(draft("s", Some("  \n  ")), &SecretScanner::default());
          assert_eq!(finding.details(), None);
      }

      #[test]
      fn builders_attach_task_origin_and_file_hash() {
          let finding = Finding::new(draft("s", None), &SecretScanner::default())
              .with_task_id("task_1")
              .with_origin_ref("cmd_1")
              .with_file_hash("sha256:abc");
          assert_eq!(finding.task_id(), Some("task_1"));
          assert_eq!(finding.origin_ref(), Some("cmd_1"));
          assert_eq!(finding.file_hash(), Some("sha256:abc"));
      }

      #[test]
      fn set_status_changes_only_the_status() {
          let mut finding = Finding::new(draft("s", None), &SecretScanner::default());
          let before = finding.clone();
          finding.set_status(FindingStatus::Dismissed);
          assert_eq!(finding.status(), FindingStatus::Dismissed);
          assert_eq!(finding.id(), before.id());
          assert_eq!(finding.summary(), before.summary());
      }

      /// The persisted and served shape (Tasks 7 and 9 depend on it).
      #[test]
      fn json_uses_camel_case_keys_and_snake_case_enum_values() {
          let finding = Finding::new(
              FindingDraft {
                  source: FindingSource::BrowserConsole,
                  severity: Severity::Warning,
                  summary: "s".to_string(),
                  details: None,
                  range: Some(SourceRange {
                      path: "src/app.js".to_string(),
                      start_line: 3,
                      start_column: Some(7),
                      end_line: None,
                      end_column: None,
                  }),
                  code: Some("no-unused-vars".to_string()),
              },
              &SecretScanner::default(),
          )
          .with_task_id("task_1")
          .with_origin_ref("cmd_1")
          .with_file_hash("sha256:abc");

          let value = serde_json::to_value(&finding).unwrap();
          assert_eq!(value["source"], "browser_console");
          assert_eq!(value["severity"], "warning");
          assert_eq!(value["status"], "open");
          assert_eq!(value["taskId"], "task_1");
          assert_eq!(value["originRef"], "cmd_1");
          assert_eq!(value["fileHash"], "sha256:abc");
          assert_eq!(value["code"], "no-unused-vars");
          assert_eq!(value["range"]["path"], "src/app.js");
          assert_eq!(value["range"]["startLine"], 3);
          assert_eq!(value["range"]["startColumn"], 7);
          assert!(value["createdAtMs"].is_u64());

          let back: Finding = serde_json::from_value(value).unwrap();
          assert_eq!(back, finding);
      }

      #[test]
      fn every_source_serialises_as_its_as_str() {
          use FindingSource::*;
          for source in [
              Compiler, Test, Lint, Security, BrowserConsole, BrowserNetwork,
              CodeReview, LanguageServer,
          ] {
              assert_eq!(
                  serde_json::to_value(source).unwrap(),
                  serde_json::Value::String(source.as_str().to_string()),
              );
          }
      }

      #[test]
      fn severity_sorts_error_before_warning_before_info() {
          let mut severities = vec![Severity::Info, Severity::Error, Severity::Warning];
          severities.sort();
          assert_eq!(severities, [Severity::Error, Severity::Warning, Severity::Info]);
      }
  }
  ```

- [x] **Step 3: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::tests)'`
  Expected: a compile failure, because `FindingDraft`, `Finding`, and the rest
  are not defined.

- [x] **Step 4: Write the implementation**

  Put this above the test module in `finding.rs`:

  ```rust
  //! One structured finding type shared by every check source (spec 22).
  //!
  //! Fields are private so that [`Finding::new`] is the only constructor,
  //! and therefore the only place redaction and bounding happen
  //! (`docs/specs/22_findings_model_and_panel/context.md` §2).

  use crate::hash::{create_id, now_millis};
  use crate::secret_scanner::SecretScanner;
  use serde::{Deserialize, Serialize};

  pub const MAX_SUMMARY_CHARS: usize = 240;
  pub const MAX_DETAILS_BYTES: usize = 4096;
  pub const DETAILS_TRUNCATION_MARKER: &str = "\n… (truncated)";

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
      /// Phase 3. Declared now so adding it is not a schema change.
      LanguageServer,
  }

  impl FindingSource {
      /// The serialised form, for text that names a source.
      pub fn as_str(self) -> &'static str {
          match self {
              Self::Compiler => "compiler",
              Self::Test => "test",
              Self::Lint => "lint",
              Self::Security => "security",
              Self::BrowserConsole => "browser_console",
              Self::BrowserNetwork => "browser_network",
              Self::CodeReview => "code_review",
              Self::LanguageServer => "language_server",
          }
      }
  }

  /// As the source reported it. Two sources' `Warning`s are not claimed to be
  /// equally important (proposal §5.2).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum Severity {
      Error,
      Warning,
      Info,
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum FindingStatus {
      Open,
      Dismissed,
      Fixed,
      Stale,
  }

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct SourceRange {
      /// Repository-relative.
      pub path: String,
      pub start_line: u32,
      pub start_column: Option<u32>,
      pub end_line: Option<u32>,
      pub end_column: Option<u32>,
  }

  /// What a parser extracted, before redaction and bounding. Plain data:
  /// parsers build these, and only [`Finding::new`] turns one into a finding
  /// (`context.md` §3).
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct FindingDraft {
      pub source: FindingSource,
      pub severity: Severity,
      pub summary: String,
      pub details: Option<String>,
      pub range: Option<SourceRange>,
      pub code: Option<String>,
  }

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct Finding {
      id: String,
      source: FindingSource,
      severity: Severity,
      /// One line, redacted, at most `MAX_SUMMARY_CHARS`.
      summary: String,
      /// Redacted, at most `MAX_DETAILS_BYTES` plus the marker. Never the
      /// whole tool output, which stays where `origin_ref` points.
      details: Option<String>,
      range: Option<SourceRange>,
      task_id: Option<String>,
      /// The command execution, diagnostic call, or tool call this came from.
      origin_ref: Option<String>,
      status: FindingStatus,
      /// Machine-readable code where the source has one: `E0308`, `no-unused-vars`.
      code: Option<String>,
      /// Hash of `range.path` when recorded. `None` means staleness cannot be
      /// judged, which is not the same as stale (`context.md` §1).
      file_hash: Option<String>,
      created_at_ms: u128,
  }

  impl Finding {
      /// The only constructor. Redacts, then bounds: the other order can cut
      /// a secret into a fragment the scanner no longer recognises
      /// (`context.md` §4).
      pub fn new(draft: FindingDraft, scanner: &SecretScanner) -> Self {
          let summary = bound_summary(&scanner.redact(&draft.summary).text, draft.source);
          let details = draft
              .details
              .as_deref()
              .and_then(|details| bound_details(&scanner.redact(details).text));
          Self {
              id: create_id("finding"),
              source: draft.source,
              severity: draft.severity,
              summary,
              details,
              range: draft.range,
              task_id: None,
              origin_ref: None,
              status: FindingStatus::Open,
              code: draft.code,
              file_hash: None,
              created_at_ms: now_millis(),
          }
      }

      pub fn with_task_id(mut self, task_id: impl Into<String>) -> Self {
          self.task_id = Some(task_id.into());
          self
      }

      pub fn with_origin_ref(mut self, origin_ref: impl Into<String>) -> Self {
          self.origin_ref = Some(origin_ref.into());
          self
      }

      pub fn with_file_hash(mut self, file_hash: impl Into<String>) -> Self {
          self.file_hash = Some(file_hash.into());
          self
      }

      pub fn set_status(&mut self, status: FindingStatus) {
          self.status = status;
      }

      pub fn id(&self) -> &str {
          &self.id
      }

      pub fn source(&self) -> FindingSource {
          self.source
      }

      pub fn severity(&self) -> Severity {
          self.severity
      }

      pub fn summary(&self) -> &str {
          &self.summary
      }

      pub fn details(&self) -> Option<&str> {
          self.details.as_deref()
      }

      pub fn range(&self) -> Option<&SourceRange> {
          self.range.as_ref()
      }

      pub fn task_id(&self) -> Option<&str> {
          self.task_id.as_deref()
      }

      pub fn origin_ref(&self) -> Option<&str> {
          self.origin_ref.as_deref()
      }

      pub fn status(&self) -> FindingStatus {
          self.status
      }

      pub fn code(&self) -> Option<&str> {
          self.code.as_deref()
      }

      pub fn file_hash(&self) -> Option<&str> {
          self.file_hash.as_deref()
      }

      pub fn created_at_ms(&self) -> u128 {
          self.created_at_ms
      }
  }

  fn bound_summary(text: &str, source: FindingSource) -> String {
      let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty()) else {
          return format!("{} finding with no message", source.as_str());
      };
      if line.chars().count() <= MAX_SUMMARY_CHARS {
          return line.to_string();
      }
      let mut cut: String = line.chars().take(MAX_SUMMARY_CHARS - 1).collect();
      cut.push('…');
      cut
  }

  /// Keeps the head: parsed details put the useful lines first, and the
  /// generic parser picks its own tail before building the draft.
  fn bound_details(text: &str) -> Option<String> {
      if text.trim().is_empty() {
          return None;
      }
      if text.len() <= MAX_DETAILS_BYTES {
          return Some(text.to_string());
      }
      let mut end = MAX_DETAILS_BYTES;
      while !text.is_char_boundary(end) {
          end -= 1;
      }
      Some(format!("{}{DETAILS_TRUNCATION_MARKER}", &text[..end]))
  }
  ```

  If a test and this sketch disagree, the test wins, because it comes from
  `proposal.md` §5.1/§5.6 and `context.md` §§1–4. Record any deviation in the
  progress row.

- [x] **Step 5: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::tests)'`
  Expected: all 16 pass. Count them. `test(finding)` would also match any
  other test whose name contains "finding", such as `secret_scanner`'s
  `SecretFinding` tests, so use the module path, as spec 20 Task 3 learned.

- [x] **Step 6: Mutation-test the load-bearing guarantees**

  Apply each change on its own, confirm the named test fails, then revert it:

  1. Swap the order in `new` so it bounds `draft.details` first and redacts
     the result. `a_secret_straddling_the_details_bound_is_redacted_not_cut`
     must fail. This is the ordering decision from `context.md` §4, and the
     test's only reason to exist.
  2. Drop `scanner.redact` from the summary path.
     `new_redacts_a_secret_in_summary_and_details` must fail.
  3. Change `<=` to `<` in `bound_summary`.
     `a_summary_at_exactly_the_bound_is_not_cut` must fail.
  4. Replace the `is_char_boundary` loop with a plain `&text[..MAX_DETAILS_BYTES]`.
     `details_are_bounded_on_a_char_boundary` must fail. It fails by
     panicking, and that counts.

  Record all four results in the progress row.

- [x] **Step 7: Confirm the privacy guarantee is real**

  Temporarily add, in a scratch `#[test]` outside `finding.rs` (for example in
  `lib.rs`), a `finding::Finding { .. }` struct literal that names every field.
  Confirm it fails with `E0451` (private field). Then add a direct read of
  `finding.summary` on an existing value, and confirm it fails with `E0616`.
  Remove both. Do not leave
  a `compile_fail` doctest behind as the guard. `cargo nextest` does not run
  doctests, so the quality gate would never run it. Field privacy is the
  guard, and it is enforced every time the crate compiles.

- [x] **Step 8: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  cargo nextest run -p workspace-engine -E 'test(finding::tests)'
  typos docs/specs/22_findings_model_and_panel crates/workspace-engine/src/finding.rs
  ```

- [x] **Step 9: Update this file's Task 1 row, then show the change and the
  check results and ask before committing**

---

## Task 2: Parser trait, dispatch with fall-through, generic parser

**Requirements:** 1, 3 (addressability), and the acceptance criteria "exactly
one generic finding, never zero", "falls through … asserted by test", and "no
parser produces a `range` for output that contained no location".
**Files:** modify `crates/workspace-engine/src/finding.rs`. Planned in full on
2026-09-30. **Read `context.md` §8 first.** It records four decisions this task
depends on, and two of them change the outline this section replaced:

- a new `FindingSource::Command` (§8.1);
- a cancelled run gets no generic finding (§8.2);
- the generic finding's exact text (§8.3);
- where Tasks 3–5 put their parsers (§8.4).

This task touches only `finding.rs`. It ships no real parser, so
`default_parsers()` is empty until Task 3. Everything is exercised through stub
parsers, which pin the dispatch rules independently of any one tool's output
format.

**Interfaces:**
- Consumes: Task 1's `Finding::new`, `FindingDraft`, `FindingSource` and
  `Severity`. Also `CommandExecution` and `CommandTermination`
  (`command_runner.rs:16-43`), read-only.
- Produces, and Tasks 3–7 rely on these names:
  - `FindingSource::Command`, serialised as `"command"`: the generic
    fallback's source and nothing else's.
  - The parser trait:
    `pub trait FindingParser { fn source(&self) -> FindingSource; fn matches(&self, command: &str) -> bool; fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft>; }`.
    `source()` is the parser's *primary* source. Task 4's `cargo test` parser
    also emits `Compiler` drafts (`context.md` §5).
  - `pub fn default_parsers() -> Vec<Box<dyn FindingParser>>`. Tasks 3–5
    register their parsers here, in the order they should be tried.
  - `pub fn findings_from_execution(execution: &CommandExecution, parsers: &[Box<dyn FindingParser>], scanner: &SecretScanner) -> Vec<Finding>`.
    This is the only place outside the tests that calls `Finding::new` on
    parser output. Task 7 calls it and then attaches `task_id`, `origin_ref`
    and `file_hash`.
  - `pub const GENERIC_DETAIL_LINES: usize = 40`.

- [x] **Step 1: Add `FindingSource::Command` and extend Task 1's test**

  In `finding.rs`, add the variant after `LanguageServer`:

  ```rust
      /// One command's failure taken whole: nothing was parsed out of it.
      /// Only the generic fallback produces this (`context.md` §8.1).
      Command,
  ```

  Add the `as_str` arm `Self::Command => "command",`. Add `Command` to the
  array in the existing
  `tests::every_source_serialises_as_its_as_str`. Run
  `cargo nextest run -p workspace-engine -E 'test(finding::tests)'`.
  Expected: all 16 still pass.

- [x] **Step 2: Write the failing tests**

  Append a second test module to `finding.rs`, after `mod tests`, so the
  dispatch tests can be filtered on their own:

  ```rust
  #[cfg(test)]
  mod dispatch_tests {
      use super::*;
      use crate::command_policy::CommandRisk;
      use crate::command_runner::{CommandExecution, CommandTermination};
      use crate::secret_scanner::SecretScanner;

      fn execution(
          command: &str,
          termination: CommandTermination,
          exit_code: Option<i32>,
          stdout: &str,
          stderr: &str,
      ) -> CommandExecution {
          CommandExecution {
              id: "cmd_test".to_string(),
              command: command.to_string(),
              working_directory: "/repo".to_string(),
              risk: CommandRisk::Low,
              approved_by: None,
              started_at_ms: 1,
              completed_at_ms: 2,
              exit_code,
              termination,
              stdout: stdout.to_string(),
              stderr: stderr.to_string(),
          }
      }

      fn failed(command: &str, stdout: &str, stderr: &str) -> CommandExecution {
          execution(command, CommandTermination::Exited, Some(1), stdout, stderr)
      }

      fn passed(command: &str) -> CommandExecution {
          execution(command, CommandTermination::Exited, Some(0), "ok\n", "")
      }

      fn draft(source: FindingSource, severity: Severity, summary: &str) -> FindingDraft {
          FindingDraft {
              source,
              severity,
              summary: summary.to_string(),
              details: None,
              range: None,
              code: None,
          }
      }

      /// Matches commands starting with `prefix` and returns fixed drafts.
      struct Stub {
          prefix: &'static str,
          drafts: Vec<FindingDraft>,
      }

      impl FindingParser for Stub {
          fn source(&self) -> FindingSource {
              FindingSource::Test
          }
          fn matches(&self, command: &str) -> bool {
              command.starts_with(self.prefix)
          }
          fn parse(&self, _execution: &CommandExecution) -> Vec<FindingDraft> {
              self.drafts.clone()
          }
      }

      /// Never matches, and fails the test if asked to parse.
      struct NeverMatches;

      impl FindingParser for NeverMatches {
          fn source(&self) -> FindingSource {
              FindingSource::Lint
          }
          fn matches(&self, _command: &str) -> bool {
              false
          }
          fn parse(&self, _execution: &CommandExecution) -> Vec<FindingDraft> {
              panic!("a parser that does not match was asked to parse")
          }
      }

      fn stub(prefix: &'static str, drafts: Vec<FindingDraft>) -> Box<dyn FindingParser> {
          Box::new(Stub { prefix, drafts })
      }

      fn run(execution: &CommandExecution, parsers: &[Box<dyn FindingParser>]) -> Vec<Finding> {
          findings_from_execution(execution, parsers, &SecretScanner::default())
      }

      #[test]
      fn a_failure_no_parser_matches_yields_exactly_one_generic_finding() {
          let findings = run(&failed("pytest", "", "\n  first line  \nsecond line\n"), &[]);
          assert_eq!(findings.len(), 1, "{findings:?}");
          let finding = &findings[0];
          assert_eq!(finding.source(), FindingSource::Command);
          assert_eq!(finding.severity(), Severity::Error);
          assert_eq!(finding.summary(), "pytest: first line");
          assert_eq!(finding.range(), None, "the generic parser invented a location");
          assert_eq!(finding.code(), None);
      }

      /// The rule proposal §5.3 calls "most likely to be omitted".
      #[test]
      fn a_matching_parser_that_extracts_nothing_falls_through_to_generic() {
          let parsers = [stub("cargo test", vec![])];
          let findings = run(&failed("cargo test", "", "something new\n"), &parsers);
          assert_eq!(findings.len(), 1, "{findings:?}");
          assert_eq!(findings[0].source(), FindingSource::Command);
          assert_eq!(findings[0].summary(), "cargo test: something new");
      }

      #[test]
      fn parsed_drafts_replace_the_generic_finding() {
          let parsers = [stub(
              "cargo test",
              vec![
                  draft(FindingSource::Test, Severity::Error, "a failed"),
                  draft(FindingSource::Compiler, Severity::Error, "b failed"),
              ],
          )];
          let findings = run(&failed("cargo test", "", "boom\n"), &parsers);
          let summaries: Vec<_> = findings.iter().map(Finding::summary).collect();
          assert_eq!(summaries, ["a failed", "b failed"]);
          assert!(findings.iter().all(|f| f.source() != FindingSource::Command));
      }

      #[test]
      fn the_first_matching_parser_wins() {
          let parsers = [
              stub("cargo", vec![draft(FindingSource::Compiler, Severity::Error, "first")]),
              stub("cargo test", vec![draft(FindingSource::Test, Severity::Error, "second")]),
          ];
          let findings = run(&failed("cargo test", "", "x\n"), &parsers);
          assert_eq!(findings.len(), 1);
          assert_eq!(findings[0].summary(), "first");
      }

      #[test]
      fn a_parser_that_does_not_match_is_never_asked() {
          let parsers: [Box<dyn FindingParser>; 2] = [
              Box::new(NeverMatches),
              stub("npm", vec![draft(FindingSource::Lint, Severity::Error, "lint")]),
          ];
          let findings = run(&failed("npm run lint", "", "x\n"), &parsers);
          assert_eq!(findings.len(), 1);
          assert_eq!(findings[0].summary(), "lint");
      }

      #[test]
      fn a_passing_execution_with_no_parser_yields_nothing() {
          assert!(run(&passed("pytest"), &[]).is_empty());
      }

      #[test]
      fn a_passing_execution_keeps_its_parsers_warnings() {
          let parsers = [stub(
              "cargo clippy",
              vec![draft(FindingSource::Compiler, Severity::Warning, "unused import")],
          )];
          let findings = run(&passed("cargo clippy"), &parsers);
          assert_eq!(findings.len(), 1);
          assert_eq!(findings[0].severity(), Severity::Warning);
      }

      #[test]
      fn a_passing_execution_whose_parser_finds_nothing_yields_nothing() {
          let parsers = [stub("cargo test", vec![])];
          assert!(run(&passed("cargo test"), &parsers).is_empty());
      }

      #[test]
      fn a_timeout_is_a_failure_even_without_an_exit_code() {
          let timed_out =
              execution("cargo test", CommandTermination::TimedOut, None, "running 3 tests\n", "");
          let findings = run(&timed_out, &[]);
          assert_eq!(findings.len(), 1, "{findings:?}");
          assert_eq!(findings[0].summary(), "cargo test timed out");
          assert!(findings[0].details().unwrap().contains("running 3 tests"));
      }

      /// `Exited` with no code is a signal. It is not a zero, and it must never
      /// be read as one (the same rule as `Evidence::CommandExit`).
      #[test]
      fn a_signal_kill_is_a_failure() {
          let killed = execution("make check", CommandTermination::Exited, None, "", "");
          let findings = run(&killed, &[]);
          assert_eq!(findings.len(), 1, "{findings:?}");
          assert_eq!(findings[0].summary(), "make check was killed by a signal");
      }

      /// `context.md` §8.2: a check the user stopped has no verdict to repair.
      #[test]
      fn a_cancelled_execution_gets_no_generic_finding() {
          let cancelled =
              execution("cargo test", CommandTermination::Cancelled, None, "", "Compiling x\n");
          assert!(run(&cancelled, &[]).is_empty());
          assert!(run(&cancelled, &[stub("cargo test", vec![])]).is_empty());
      }

      #[test]
      fn a_cancelled_execution_keeps_what_its_parser_found() {
          let cancelled = execution("cargo test", CommandTermination::Cancelled, None, "", "");
          let parsers = [stub(
              "cargo test",
              vec![draft(FindingSource::Compiler, Severity::Error, "E0308")],
          )];
          let findings = run(&cancelled, &parsers);
          assert_eq!(findings.len(), 1);
          assert_eq!(findings[0].summary(), "E0308");
      }

      #[test]
      fn the_generic_summary_falls_back_to_stdout_then_to_the_exit_code() {
          let from_stdout = run(&failed("pytest", "\nFAILED test_x\n", "  \n"), &[]);
          assert_eq!(from_stdout[0].summary(), "pytest: FAILED test_x");

          let silent = execution("pytest", CommandTermination::Exited, Some(2), "", "");
          let from_code = run(&silent, &[]);
          assert_eq!(from_code[0].summary(), "pytest exited with code 2");
          assert_eq!(from_code[0].details(), None);
      }

      #[test]
      fn the_generic_details_keep_the_last_lines_of_each_stream() {
          let stderr: String = (0..100).map(|n| format!("err {n:03}\n")).collect();
          let findings = run(&failed("pytest", "out a\nout b\n", &stderr), &[]);
          let details = findings[0].details().expect("details");
          assert!(details.starts_with("stderr:\n"), "{details}");
          assert!(details.contains("err 060"), "the tail was not kept");
          assert!(details.contains("err 099"));
          assert!(!details.contains("err 059"), "more than {GENERIC_DETAIL_LINES} lines kept");
          assert!(details.contains("\n\nstdout:\nout a\nout b"), "{details}");
      }

      #[test]
      fn an_empty_stream_is_left_out_of_the_generic_details() {
          let findings = run(&failed("pytest", "only stdout\n", "   \n"), &[]);
          assert_eq!(findings[0].details(), Some("stdout:\nonly stdout"));
      }

      /// Acceptance criterion: no unredacted secret in "a generic fallback".
      /// The runner already redacts command output (`context.md` §6). This
      /// test proves the dispatcher goes through `Finding::new` anyway.
      #[test]
      fn a_secret_in_failed_output_is_redacted_in_the_generic_finding() {
          let key = "AKIAIOSFODNN7EXAMPLE";
          let findings = run(&failed("deploy", "", &format!("bad key {key}\n")), &[]);
          assert!(!findings[0].summary().contains(key), "{}", findings[0].summary());
          assert!(!findings[0].details().unwrap().contains(key));
      }
  }
  ```

- [x] **Step 3: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::dispatch_tests)'`
  Expected: a compile failure, because `FindingParser` and
  `findings_from_execution` are not defined.

- [x] **Step 4: Write the implementation**

  Add `use crate::command_runner::{CommandExecution, CommandTermination};` to
  the imports. Then put this after the `Finding` impl and before
  `bound_summary`:

  ```rust
  /// Lines kept from each stream for a generic finding's details.
  pub const GENERIC_DETAIL_LINES: usize = 40;

  /// Turns one tool's output into drafts. Returns drafts, not findings, so that
  /// redaction happens in `findings_from_execution` and nowhere else
  /// (`context.md` §3). Severity mapping is documented on each parser (§5.2).
  pub trait FindingParser {
      /// The source this parser mostly produces. A parser may emit drafts of
      /// another source; `cargo test` emits `Compiler` drafts for a compile
      /// error (`context.md` §5).
      fn source(&self) -> FindingSource;
      /// Whether this parser recognises the output of `command`.
      fn matches(&self, command: &str) -> bool;
      /// Parse already-redacted output. Empty when nothing parsed, and must
      /// never invent a location (§5.3).
      fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft>;
  }

  /// The shipped parsers, in the order they are tried. Tasks 3–5 register
  /// theirs here. The generic fallback is not in this list: it is applied by
  /// `findings_from_execution` itself, so no caller can forget it.
  pub fn default_parsers() -> Vec<Box<dyn FindingParser>> {
      Vec::new()
  }

  /// Whether a run reached a verdict, and which. `context.md` §8.2 has the
  /// table this encodes.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  enum Verdict {
      Passed,
      Failed,
      /// Cancelled by the user before it finished.
      Unfinished,
  }

  fn verdict(execution: &CommandExecution) -> Verdict {
      match (execution.termination, execution.exit_code) {
          (CommandTermination::Exited, Some(0)) => Verdict::Passed,
          (CommandTermination::Exited, _) | (CommandTermination::TimedOut, _) => Verdict::Failed,
          (CommandTermination::Cancelled, _) => Verdict::Unfinished,
      }
  }

  /// The first parser that matches wins. A failed run whose parser found
  /// nothing falls through to one generic finding, so a regex that stops
  /// matching after a tool upgrade cannot swallow a failure (§5.3).
  pub fn findings_from_execution(
      execution: &CommandExecution,
      parsers: &[Box<dyn FindingParser>],
      scanner: &SecretScanner,
  ) -> Vec<Finding> {
      let mut drafts = parsers
          .iter()
          .find(|parser| parser.matches(&execution.command))
          .map(|parser| parser.parse(execution))
          .unwrap_or_default();
      if drafts.is_empty() && verdict(execution) == Verdict::Failed {
          drafts.push(generic_draft(execution));
      }
      drafts
          .into_iter()
          .map(|draft| Finding::new(draft, scanner))
          .collect()
  }

  /// One finding for the whole failure, never claiming structure it did not
  /// find: no range, no code (`context.md` §8.3).
  fn generic_draft(execution: &CommandExecution) -> FindingDraft {
      let command = execution.command.trim();
      let summary = match (execution.termination, execution.exit_code) {
          (CommandTermination::TimedOut, _) => format!("{command} timed out"),
          (_, None) => format!("{command} was killed by a signal"),
          (_, Some(code)) => first_non_empty_line(&execution.stderr)
              .or_else(|| first_non_empty_line(&execution.stdout))
              .map(|line| format!("{command}: {line}"))
              .unwrap_or_else(|| format!("{command} exited with code {code}")),
      };
      FindingDraft {
          source: FindingSource::Command,
          severity: Severity::Error,
          summary,
          details: generic_details(execution),
          range: None,
          code: None,
      }
  }

  fn generic_details(execution: &CommandExecution) -> Option<String> {
      let sections: Vec<String> = [("stderr", &execution.stderr), ("stdout", &execution.stdout)]
          .into_iter()
          .filter(|(_, text)| !text.trim().is_empty())
          .map(|(label, text)| format!("{label}:\n{}", last_lines(text, GENERIC_DETAIL_LINES)))
          .collect();
      (!sections.is_empty()).then(|| sections.join("\n\n"))
  }

  fn last_lines(text: &str, count: usize) -> String {
      let lines: Vec<&str> = text.trim_end().lines().collect();
      lines[lines.len().saturating_sub(count)..].join("\n")
  }

  fn first_non_empty_line(text: &str) -> Option<&str> {
      text.lines().map(str::trim).find(|line| !line.is_empty())
  }
  ```

  `bound_summary` already has an identical first-line search. Make it call
  `first_non_empty_line` rather than keeping two copies. Task 1's
  `summary_is_the_first_non_empty_line_trimmed` guards that refactor.

  If a test and this sketch disagree, the test wins, because the tests come
  from proposal §5.3 and `context.md` §8. Record any deviation in the progress
  row.

- [x] **Step 5: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::tests) + test(finding::dispatch_tests)'`
  Expected: 32 pass (16 + 16). Count them.

- [x] **Step 6: Mutation-test the dispatch rules**

  Apply each change on its own, confirm the named test fails, then revert it:

  1. Call `generic_draft` only when *no* parser matched, instead of when the
     drafts are empty.
     `a_matching_parser_that_extracts_nothing_falls_through_to_generic` must
     fail, while `a_failure_no_parser_matches_…` still passes. This is the
     fall-through, and the reason Task 2 exists.
  2. Map `(Exited, None)` to `Verdict::Passed`.
     `a_signal_kill_is_a_failure` must fail.
  3. Map `Cancelled` to `Verdict::Failed`.
     `a_cancelled_execution_gets_no_generic_finding` must fail.
  4. Make `last_lines` keep the first `count` lines instead of the last.
     `the_generic_details_keep_the_last_lines_of_each_stream` must fail.
  5. Drop the `verdict(execution) == Verdict::Failed` condition.
     `a_passing_execution_with_no_parser_yields_nothing` must fail.

  Record all five results in the progress row.

- [x] **Step 7: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  cargo nextest run -p workspace-engine -E 'test(finding::tests) + test(finding::dispatch_tests)'
  typos docs/specs/22_findings_model_and_panel crates/workspace-engine/src/finding.rs
  ```

- [x] **Step 8: Update this file's Task 2 row, then show the change and the
  check results and ask before committing**

## Task 3: Rust diagnostics parser

**Requirements:** 1 (compiler and lint sources), 2, and the acceptance
criteria "a … lint error … normalise[s] into `Finding` with correct source,
severity, and … file and range" and "no parser produces a `range` for output
that contained no location". **Files:** create
`crates/workspace-engine/src/finding/rust_diagnostics.rs` and three fixture
files under `crates/workspace-engine/src/finding/fixtures/`; modify
`crates/workspace-engine/src/finding.rs` (module declaration and registration
only). Planned in full on 2026-09-30.

**Read `context.md` §9 first.** It records what real cargo 1.98.0 output looks
like, and each rule below comes from it. Two of those rules correct the old
outline:

- the code does **not** come from the `#[warn(clippy::…)]` note, which only
  appears on a lint's first occurrence;
- the location is the first `-->` before any child `note:`/`help:`, not just
  any `-->` in the block.

**Interfaces:**
- Consumes: `FindingParser`, `FindingDraft`, `FindingSource`, `Severity`,
  `SourceRange`, `default_parsers` and `findings_from_execution` (Tasks 1–2,
  `finding.rs`). Also `CommandExecution` (`command_runner.rs`).
- Produces, for Task 4:
  - `pub(super) fn parse_rust_diagnostics(output: &str) -> Vec<FindingDraft>`,
    which parses rustc and clippy diagnostics out of any text. Task 4 runs it
    over `cargo test`'s stderr (`context.md` §5).
  - `pub(super) fn cargo_subcommand(command: &str) -> Option<&str>`, which
    returns the subcommand of the first `cargo` invocation in a shell command.
    Task 4 matches `test` with it.
  - `pub(super) struct RustDiagnosticsParser`, registered first in
    `default_parsers()`.

- [x] **Step 1: Commit the fixtures as files**

  Create `crates/workspace-engine/src/finding/fixtures/` and write these three
  files **byte for byte**. They are cargo 1.98.0 captures from a workspace
  with one member at `crates/demo`. The absolute path in the `Compiling` /
  `Checking` line was replaced with `/repo`, and the timing-dependent
  `Finished` line was removed (`context.md` §9). Each file ends with a single
  newline.

  `rust_build_errors.txt`. This is `cargo build` with two type errors and two
  unused variables. It exits 101.

  ````text
   Compiling demo v0.1.0 (/repo/crates/demo)
error[E0308]: mismatched types
 --> crates/demo/src/lib.rs:8:18
  |
8 |     let x: u32 = "a";
  |            ---   ^^^ expected `u32`, found `&str`
  |            |
  |            expected due to this

error[E0061]: this function takes 1 argument but 2 arguments were supplied
  --> crates/demo/src/lib.rs:9:5
   |
 9 |     takes(1, 2);
   |     ^^^^^    - unexpected argument #2 of type `{integer}`
   |
note: function defined here
  --> crates/demo/src/lib.rs:13:4
   |
13 | fn takes(a: u32) -> u32 { a }
   |    ^^^^^
help: remove the extra argument
   |
 9 -     takes(1, 2);
 9 +     takes(1);
   |

warning: unused variable: `unused`
 --> crates/demo/src/lib.rs:2:9
  |
2 |     let unused = 5;
  |         ^^^^^^ help: if this is intentional, prefix it with an underscore: `_unused`
  |
  = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: unused variable: `other`
 --> crates/demo/src/lib.rs:3:9
  |
3 |     let other = 6;
  |         ^^^^^ help: if this is intentional, prefix it with an underscore: `_other`

Some errors have detailed explanations: E0061, E0308.
For more information about an error, try `rustc --explain E0061`.
warning: `demo` (lib) generated 2 warnings
error: could not compile `demo` (lib) due to 2 previous errors; 2 warnings emitted
  ````

  `rust_clippy_warnings.txt`. This is `cargo clippy` with one rustc lint and
  three clippy lints, two of them the same lint. It exits 0.

  ````text
    Checking demo v0.1.0 (/repo/crates/demo)
warning: unused variable: `unused`
 --> crates/demo/src/lib.rs:2:9
  |
2 |     let unused = 5;
  |         ^^^^^^ help: if this is intentional, prefix it with an underscore: `_unused`
  |
  = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: unneeded `return` statement
 --> crates/demo/src/lib.rs:3:5
  |
3 |     return 1;
  |     ^^^^^^^^
  |
  = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.98.0/index.html#needless_return
  = note: `#[warn(clippy::needless_return)]` on by default
help: remove `return`
  |
3 -     return 1;
3 +     1
  |

warning: unneeded `return` statement
 --> crates/demo/src/lib.rs:7:5
  |
7 |     return v.len();
  |     ^^^^^^^^^^^^^^
  |
  = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.98.0/index.html#needless_return
help: remove `return`
  |
7 -     return v.len();
7 +     v.len()
  |

warning: writing `&Vec` instead of `&[_]` involves a new object where a slice will do
 --> crates/demo/src/lib.rs:6:15
  |
6 | pub fn two(v: &Vec<u32>) -> usize {
  |               ^^^^^^^^^
  |
  = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.98.0/index.html#ptr_arg
  = note: `#[warn(clippy::ptr_arg)]` on by default
help: change this to
  |
6 - pub fn two(v: &Vec<u32>) -> usize {
6 + pub fn two(v: &[u32]) -> usize {
  |

warning: `demo` (lib) generated 4 warnings (run `cargo clippy --fix --lib -p demo -- ` to apply 3 suggestions)
  ````

  `rust_clippy_deny.txt`. This is the same source, run as
  `cargo clippy -- -D warnings`. It exits 101.

  ````text
    Checking demo v0.1.0 (/repo/crates/demo)
error: unused variable: `unused`
 --> crates/demo/src/lib.rs:2:9
  |
2 |     let unused = 5;
  |         ^^^^^^ help: if this is intentional, prefix it with an underscore: `_unused`
  |
  = note: `-D unused-variables` implied by `-D warnings`
  = help: to override `-D warnings` add `#[allow(unused_variables)]`

error: unneeded `return` statement
 --> crates/demo/src/lib.rs:3:5
  |
3 |     return 1;
  |     ^^^^^^^^
  |
  = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.98.0/index.html#needless_return
  = note: `-D clippy::needless-return` implied by `-D warnings`
  = help: to override `-D warnings` add `#[allow(clippy::needless_return)]`
help: remove `return`
  |
3 -     return 1;
3 +     1
  |

error: unneeded `return` statement
 --> crates/demo/src/lib.rs:7:5
  |
7 |     return v.len();
  |     ^^^^^^^^^^^^^^
  |
  = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.98.0/index.html#needless_return
help: remove `return`
  |
7 -     return v.len();
7 +     v.len()
  |

error: writing `&Vec` instead of `&[_]` involves a new object where a slice will do
 --> crates/demo/src/lib.rs:6:15
  |
6 | pub fn two(v: &Vec<u32>) -> usize {
  |               ^^^^^^^^^
  |
  = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.98.0/index.html#ptr_arg
  = note: `-D clippy::ptr-arg` implied by `-D warnings`
  = help: to override `-D warnings` add `#[allow(clippy::ptr_arg)]`
help: change this to
  |
6 - pub fn two(v: &Vec<u32>) -> usize {
6 + pub fn two(v: &[u32]) -> usize {
  |

error: could not compile `demo` (lib) due to 4 previous errors
  ````

  Each file is the text between its fences, verbatim. The fixture lines sit at
  column 0 here, exactly as cargo printed them.

- [x] **Step 2: Declare the module and write the failing tests**

  In `finding.rs`, add `mod rust_diagnostics;` directly below the `use`
  lines. Then create `crates/workspace-engine/src/finding/rust_diagnostics.rs`
  containing only the test module:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::command_policy::CommandRisk;
      use crate::command_runner::{CommandExecution, CommandTermination};
      use crate::finding::{default_parsers, findings_from_execution};
      use crate::secret_scanner::SecretScanner;

      const BUILD_ERRORS: &str = include_str!("fixtures/rust_build_errors.txt");
      const CLIPPY_WARNINGS: &str = include_str!("fixtures/rust_clippy_warnings.txt");
      const CLIPPY_DENY: &str = include_str!("fixtures/rust_clippy_deny.txt");

      fn range(path: &str, line: u32, column: u32) -> Option<SourceRange> {
          Some(SourceRange {
              path: path.to_string(),
              start_line: line,
              start_column: Some(column),
              end_line: None,
              end_column: None,
          })
      }

      fn execution(command: &str, exit_code: i32, stderr: &str) -> CommandExecution {
          CommandExecution {
              id: "cmd_test".to_string(),
              command: command.to_string(),
              working_directory: "/repo".to_string(),
              risk: CommandRisk::Low,
              approved_by: None,
              started_at_ms: 1,
              completed_at_ms: 2,
              exit_code: Some(exit_code),
              termination: CommandTermination::Exited,
              stdout: String::new(),
              stderr: stderr.to_string(),
          }
      }

      #[test]
      fn a_type_error_keeps_its_code_message_and_location() {
          let drafts = parse_rust_diagnostics(BUILD_ERRORS);
          let e0308 = &drafts[0];
          assert_eq!(e0308.source, FindingSource::Compiler);
          assert_eq!(e0308.severity, Severity::Error);
          assert_eq!(e0308.code.as_deref(), Some("E0308"));
          assert_eq!(e0308.summary, "mismatched types");
          assert_eq!(e0308.range, range("crates/demo/src/lib.rs", 8, 18));
      }

      /// `context.md` §9: E0061's block also carries
      /// `note: function defined here --> …:13:4`, which is the definition,
      /// not the error.
      #[test]
      fn the_location_is_the_primary_one_not_a_child_notes() {
          let drafts = parse_rust_diagnostics(BUILD_ERRORS);
          let e0061 = &drafts[1];
          assert_eq!(e0061.code.as_deref(), Some("E0061"));
          assert_eq!(e0061.range, range("crates/demo/src/lib.rs", 9, 5));
      }

      /// The second rustc-lint occurrence carries no note, so it has no code.
      /// That is honest, not missing (`context.md` §9).
      #[test]
      fn compiler_warnings_keep_their_severity_and_only_a_code_they_printed() {
          let drafts = parse_rust_diagnostics(BUILD_ERRORS);
          let (unused, other) = (&drafts[2], &drafts[3]);
          assert_eq!(unused.severity, Severity::Warning);
          assert_eq!(unused.source, FindingSource::Compiler);
          assert_eq!(unused.summary, "unused variable: `unused`");
          assert_eq!(unused.code.as_deref(), Some("unused_variables"));
          assert_eq!(unused.range, range("crates/demo/src/lib.rs", 2, 9));
          assert_eq!(other.summary, "unused variable: `other`");
          assert_eq!(other.code, None);
      }

      #[test]
      fn cargo_summary_lines_are_not_findings() {
          for (fixture, expected) in [(BUILD_ERRORS, 4), (CLIPPY_WARNINGS, 4), (CLIPPY_DENY, 4)] {
              let drafts = parse_rust_diagnostics(fixture);
              let summaries: Vec<_> = drafts.iter().map(|d| d.summary.as_str()).collect();
              assert_eq!(drafts.len(), expected, "{summaries:?}");
              assert!(
                  summaries.iter().all(|s| !s.starts_with("could not compile")
                      && !s.contains(" generated ")),
                  "{summaries:?}"
              );
          }
      }

      /// The `#[warn(clippy::…)]` note is on the first occurrence only. The
      /// help URL is on every one (`context.md` §9).
      #[test]
      fn every_clippy_occurrence_gets_its_code_from_the_help_url() {
          let drafts = parse_rust_diagnostics(CLIPPY_WARNINGS);
          let codes: Vec<_> = drafts.iter().map(|d| d.code.as_deref()).collect();
          assert_eq!(
              codes,
              [
                  Some("unused_variables"),
                  Some("clippy::needless_return"),
                  Some("clippy::needless_return"),
                  Some("clippy::ptr_arg"),
              ]
          );
          let sources: Vec<_> = drafts.iter().map(|d| d.source).collect();
          assert_eq!(
              sources,
              [
                  FindingSource::Compiler,
                  FindingSource::Lint,
                  FindingSource::Lint,
                  FindingSource::Lint,
              ]
          );
          assert!(drafts.iter().all(|d| d.severity == Severity::Warning));
          assert_eq!(drafts[1].range, range("crates/demo/src/lib.rs", 3, 5));
          assert_eq!(drafts[2].range, range("crates/demo/src/lib.rs", 7, 5));
      }

      /// Severity is recorded as printed (§5.2): `-D warnings` prints `error`.
      #[test]
      fn deny_warnings_records_lints_as_errors_and_reads_the_dash_flag() {
          let drafts = parse_rust_diagnostics(CLIPPY_DENY);
          assert!(drafts.iter().all(|d| d.severity == Severity::Error));
          assert_eq!(drafts[0].code.as_deref(), Some("unused_variables"));
          assert_eq!(drafts[0].source, FindingSource::Compiler);
          assert_eq!(drafts[3].code.as_deref(), Some("clippy::ptr_arg"));
      }

      #[test]
      fn details_are_the_diagnostic_block_and_nothing_after_it() {
          let drafts = parse_rust_diagnostics(BUILD_ERRORS);
          let details = drafts[0].details.as_deref().expect("details");
          assert!(details.starts_with("error[E0308]: mismatched types\n"), "{details}");
          assert!(details.contains("expected `u32`, found `&str`"));
          assert!(!details.contains("E0061"), "the next block leaked in: {details}");
          let e0061 = drafts[1].details.as_deref().expect("details");
          assert!(e0061.contains("help: remove the extra argument"), "child lost: {e0061}");
      }

      #[test]
      fn a_diagnostic_without_a_location_has_no_range() {
          let output = "error: linker `cc` not found\n  |\n  = note: No such file or directory (os error 2)\n\nerror: could not compile `demo` (bin \"demo\") due to 1 previous error\n";
          let drafts = parse_rust_diagnostics(output);
          assert_eq!(drafts.len(), 1, "{drafts:?}");
          assert_eq!(drafts[0].summary, "linker `cc` not found");
          assert_eq!(drafts[0].range, None);
      }

      /// An error with no location of its own must not borrow its child
      /// note's, which points at something else.
      #[test]
      fn a_child_notes_location_is_not_borrowed_when_the_error_has_none() {
          let output = "error: cannot find macro `foo` in this scope\n  |\nnote: a macro with a similar name exists\n --> crates/demo/src/lib.rs:1:1\n  |\n";
          let drafts = parse_rust_diagnostics(output);
          assert_eq!(drafts.len(), 1, "{drafts:?}");
          assert_eq!(drafts[0].range, None);
      }

      #[test]
      fn a_location_outside_the_workspace_has_no_range() {
          for path in [
              "/Users/me/.cargo/registry/src/index.crates.io-1/serde-1.0.0/src/lib.rs",
              "/rustc/0123abcd/library/core/src/option.rs",
              "../sibling/src/lib.rs",
          ] {
              let output = format!("error[E0599]: no method\n --> {path}:1:1\n  |\n");
              let drafts = parse_rust_diagnostics(&output);
              assert_eq!(drafts.len(), 1);
              assert_eq!(drafts[0].range, None, "{path} kept as a range");
          }
      }

      /// Captured with `CARGO_TERM_COLOR=always` (`context.md` §9).
      #[test]
      fn ansi_colour_codes_are_ignored() {
          let output = "\x1b[1m\x1b[91merror[E0308]\x1b[0m\x1b[1m: mismatched types\x1b[0m\n \x1b[1m\x1b[94m--> \x1b[0mcrates/demo/src/lib.rs:2:18\n  \x1b[1m\x1b[94m|\x1b[0m\n";
          let drafts = parse_rust_diagnostics(output);
          assert_eq!(drafts.len(), 1, "{drafts:?}");
          assert_eq!(drafts[0].code.as_deref(), Some("E0308"));
          assert_eq!(drafts[0].summary, "mismatched types");
          assert_eq!(drafts[0].range, range("crates/demo/src/lib.rs", 2, 18));
      }

      #[test]
      fn clean_output_yields_nothing() {
          let output = "   Compiling demo v0.1.0 (/repo/crates/demo)\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.33s\n";
          assert!(parse_rust_diagnostics(output).is_empty());
      }

      #[test]
      fn matches_build_check_and_clippy_in_their_common_forms() {
          let parser = RustDiagnosticsParser;
          for command in [
              "cargo build",
              "cargo b --release",
              "cargo check --workspace",
              "cargo c",
              "cargo clippy --workspace --all-targets --locked -- -D warnings",
              "cargo +nightly check",
              "RUSTFLAGS=-Dwarnings cargo check",
              "cd crates/demo && cargo check",
              "/Users/me/.cargo/bin/cargo build",
          ] {
              assert!(parser.matches(command), "{command}");
          }
      }

      #[test]
      fn does_not_match_other_commands() {
          let parser = RustDiagnosticsParser;
          for command in ["cargo test", "cargo run", "cargo fmt --check", "cargo", "npm run build", "make check"] {
              assert!(!parser.matches(command), "{command}");
          }
      }

      #[test]
      fn cargo_subcommand_skips_toolchains_and_flags() {
          assert_eq!(cargo_subcommand("cargo +nightly -q test --lib"), Some("test"));
          assert_eq!(cargo_subcommand("cd x && cargo nextest run"), Some("nextest"));
          assert_eq!(cargo_subcommand("npm test"), None);
          assert_eq!(cargo_subcommand("cargo"), None);
      }

      /// Registration, end to end through the Task 2 dispatcher.
      #[test]
      fn default_parsers_turn_a_failed_build_into_parsed_findings() {
          let findings = findings_from_execution(
              &execution("cargo build", 101, BUILD_ERRORS),
              &default_parsers(),
              &SecretScanner::default(),
          );
          assert_eq!(findings.len(), 4, "{findings:?}");
          assert!(findings.iter().all(|f| f.source() != FindingSource::Command));
          assert_eq!(findings[0].code(), Some("E0308"));
      }

      /// §5.3's fall-through, against this parser: a build that failed with
      /// output this parser cannot read still yields one finding.
      #[test]
      fn a_failed_build_this_parser_cannot_read_falls_through_to_generic() {
          let findings = findings_from_execution(
              &execution("cargo build", 101, "Killed: 9\n"),
              &default_parsers(),
              &SecretScanner::default(),
          );
          assert_eq!(findings.len(), 1, "{findings:?}");
          assert_eq!(findings[0].source(), FindingSource::Command);
          assert_eq!(findings[0].summary(), "cargo build: Killed: 9");
      }
  }
  ```

- [x] **Step 3: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::rust_diagnostics)'`
  Expected: a compile failure, because `parse_rust_diagnostics`,
  `RustDiagnosticsParser` and `cargo_subcommand` are not defined.

- [x] **Step 4: Write the implementation**

  Put this above the test module in `rust_diagnostics.rs`:

  ```rust
  //! `cargo build` / `check` / `clippy` diagnostics (spec 22 Task 3).
  //!
  //! Severity is the header word as printed: `error` is `Error` and `warning`
  //! is `Warning`. Under `-D warnings` a lint prints as `error` and is recorded
  //! as one (proposal §5.2). A `clippy::` code makes the finding `Lint`, and
  //! anything else is `Compiler`. Every rule here comes from real output
  //! recorded in `docs/specs/22_findings_model_and_panel/context.md` §9.

  use super::{FindingDraft, FindingParser, FindingSource, Severity, SourceRange};
  use crate::command_runner::CommandExecution;
  use regex::Regex;
  use std::path::{Component, Path};
  use std::sync::LazyLock;

  fn regex(pattern: &str) -> Regex {
      Regex::new(pattern).expect("a valid built-in pattern")
  }

  static HEADER: LazyLock<Regex> =
      LazyLock::new(|| regex(r"^(error|warning)(?:\[([A-Z]\d{4})\])?: (.+)$"));
  static LOCATION: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*--> (.+):(\d+):(\d+)$"));
  static CHILD_HEADER: LazyLock<Regex> = LazyLock::new(|| regex(r"^(note|help): "));
  static CLIPPY_URL: LazyLock<Regex> = LazyLock::new(|| regex(r"rust-clippy/\S*#([a-z0-9_]+)"));
  static LINT_ATTRIBUTE: LazyLock<Regex> =
      LazyLock::new(|| regex(r"#\[(?:warn|deny|forbid)\(([a-z0-9_:]+)\)\]"));
  static DENY_FLAG: LazyLock<Regex> = LazyLock::new(|| regex(r"`-D ([a-z0-9_:-]+)` implied by"));
  static GENERATED: LazyLock<Regex> =
      LazyLock::new(|| regex(r"^`[^`]+` \([^)]*\) generated \d+ warnings?"));
  static EMITTED: LazyLock<Regex> = LazyLock::new(|| regex(r"^\d+ warnings? emitted"));
  static ANSI: LazyLock<Regex> = LazyLock::new(|| regex(r"\x1b\[[0-9;]*m"));

  pub(super) struct RustDiagnosticsParser;

  impl FindingParser for RustDiagnosticsParser {
      fn source(&self) -> FindingSource {
          FindingSource::Compiler
      }

      fn matches(&self, command: &str) -> bool {
          matches!(
              cargo_subcommand(command),
              Some("build" | "b" | "check" | "c" | "clippy")
          )
      }

      fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft> {
          parse_rust_diagnostics(&execution.stderr)
      }
  }

  /// The subcommand of the first `cargo` in a shell command line, skipping a
  /// `+toolchain` and flags. Environment assignments and `cd … &&` before it
  /// are just tokens that are passed over.
  pub(super) fn cargo_subcommand(command: &str) -> Option<&str> {
      let mut tokens = command.split_whitespace();
      tokens.find(|token| *token == "cargo" || token.ends_with("/cargo"))?;
      tokens.find(|token| !token.starts_with('+') && !token.starts_with('-'))
  }

  /// rustc and clippy diagnostics in any text. A block runs from a column-0
  /// `error`/`warning` header to the next blank line or header (`context.md` §9).
  pub(super) fn parse_rust_diagnostics(output: &str) -> Vec<FindingDraft> {
      let clean = ANSI.replace_all(output, "");
      let lines: Vec<&str> = clean.lines().collect();
      let mut drafts = Vec::new();
      let mut index = 0;
      while index < lines.len() {
          let Some(header) = HEADER.captures(lines[index]) else {
              index += 1;
              continue;
          };
          let end = lines[index + 1..]
              .iter()
              .position(|line| line.trim().is_empty() || HEADER.is_match(line))
              .map_or(lines.len(), |offset| index + 1 + offset);
          let block = &lines[index..end];
          index = end;

          let message = header[3].trim();
          if is_cargo_summary(message) {
              continue;
          }
          let code = header
              .get(2)
              .map(|code| code.as_str().to_string())
              .or_else(|| lint_code(block));
          let source = if code.as_deref().is_some_and(|code| code.starts_with("clippy::")) {
              FindingSource::Lint
          } else {
              FindingSource::Compiler
          };
          drafts.push(FindingDraft {
              source,
              severity: if &header[1] == "error" {
                  Severity::Error
              } else {
                  Severity::Warning
              },
              summary: message.to_string(),
              details: Some(block.join("\n")),
              range: primary_location(block),
              code,
          });
      }
      drafts
  }

  fn is_cargo_summary(message: &str) -> bool {
      message.starts_with("could not compile ")
          || message.starts_with("aborting due to ")
          || message.starts_with("build failed")
          || GENERATED.is_match(message)
          || EMITTED.is_match(message)
  }

  /// The first `-->` after the header and before any child `note:`/`help:`,
  /// whose own `-->` points somewhere else (`context.md` §9).
  fn primary_location(block: &[&str]) -> Option<SourceRange> {
      let captures = block[1..]
          .iter()
          .take_while(|line| !CHILD_HEADER.is_match(line))
          .find_map(|line| LOCATION.captures(line))?;
      let path = &captures[1];
      let outside = Path::new(path).is_absolute()
          || Path::new(path).components().any(|part| part == Component::ParentDir);
      if outside {
          return None;
      }
      Some(SourceRange {
          path: path.to_string(),
          start_line: captures[2].parse().ok()?,
          start_column: captures[3].parse().ok(),
          end_line: None,
          end_column: None,
      })
  }

  /// Bracket codes are read from the header. This handles lint codes, in the
  /// order `context.md` §9 sets, reading only `= note:`/`= help:` annotations
  /// so a snippet of the user's own `#[warn(…)]` source is never read as one.
  fn lint_code(block: &[&str]) -> Option<String> {
      let annotations = || {
          block
              .iter()
              .map(|line| line.trim_start())
              .filter(|line| line.starts_with("= "))
      };
      annotations()
          .find_map(|line| CLIPPY_URL.captures(line))
          .map(|captures| format!("clippy::{}", &captures[1]))
          .or_else(|| {
              annotations()
                  .find_map(|line| LINT_ATTRIBUTE.captures(line))
                  .map(|captures| captures[1].to_string())
          })
          .or_else(|| {
              annotations()
                  .find_map(|line| DENY_FLAG.captures(line))
                  .map(|captures| captures[1].replace('-', "_"))
          })
  }
  ```

  In `finding.rs`, register the parser:

  ```rust
  pub fn default_parsers() -> Vec<Box<dyn FindingParser>> {
      vec![Box::new(rust_diagnostics::RustDiagnosticsParser)]
  }
  ```

  If a test and this sketch disagree, the test wins, because the tests come
  from the captured fixtures. Record any deviation in the progress row. If the
  lifetimes in `primary_location`'s `find_map` or `lint_code`'s closure do not
  compile as written, restructure them into a plain loop. Do not weaken a
  test to fit.

- [x] **Step 5: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::)'`
  Expected: 49 pass: 16 in `tests`, 16 in `dispatch_tests`, and 17 in
  `rust_diagnostics::tests`. Count them. The Task 2 dispatch tests pass
  explicit parser lists and never call `default_parsers()`, so registering a
  parser must not change their result. If one fails, that is a real signal.

- [x] **Step 6: Mutation-test the rules that came from real output**

  Apply each change on its own, confirm the named test fails, then revert it:

  1. Drop the `take_while` in `primary_location`, so that any `-->` in the
     block counts.
     `a_child_notes_location_is_not_borrowed_when_the_error_has_none` must
     fail.
  2. Take the **last** `-->` before a child header, or in the whole block,
     instead of the first.
     `the_location_is_the_primary_one_not_a_child_notes` must fail with
     `13:4`.
  3. Make `is_cargo_summary` always return `false`.
     `cargo_summary_lines_are_not_findings` must fail.
  4. Remove the `CLIPPY_URL` branch from `lint_code`.
     `every_clippy_occurrence_gets_its_code_from_the_help_url` must fail on
     the second `needless_return`.
  5. Remove the `is_absolute` check.
     `a_location_outside_the_workspace_has_no_range` must fail.
  6. Skip the `ANSI.replace_all`, parsing `output` directly.
     `ansi_colour_codes_are_ignored` must fail.
  7. End a block only at a blank line, not at the next header.
     This should fail nothing in the fixtures, because cargo always separates
     blocks with a blank line. Record whether it does. If nothing fails, keep
     the header check anyway, as defence against a runner that strips blank
     lines, and say so in the row.

  Record all seven results in the progress row. Mutation 1 does not fail any
  fixture test. E0061's primary `-->` comes before its child note, so "any
  `-->`" and "first before a child" pick the same line. Only the synthetic
  no-primary-location test catches it.

- [x] **Step 7: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  cargo nextest run -p workspace-engine -E 'test(finding::)'
  typos docs/specs/22_findings_model_and_panel crates/workspace-engine/src/finding.rs crates/workspace-engine/src/finding
  ```

  If `typos` flags a word inside a fixture, it is cargo's text, not ours. Add
  the fixture directory to `_typos.toml`'s excludes with a comment saying why,
  rather than editing captured output.

- [x] **Step 8: Update this file's Task 3 row, then show the change and the
  check results and ask before committing**

## Task 4: Rust test parser

Recognises `cargo test`. It first runs Task 3's diagnostics over the output
(`context.md` §5), because a compile failure has no `failures:` block. It then
reads failing test names from the `failures:` list at the end of stdout, which
survives tail truncation (`context.md` §6), and takes the location from
`panicked at path:line:col:` in the test's `---- name stdout ----` section
where present. Tests cover a compile failure, a panic with a location, an
assertion without a recognisable location (`range: None`), and a
fall-through. Also record which `cargo nextest` output shape, if any, is in
scope. This repository's gate uses nextest, but `detect_project_commands`
proposes `cargo test`.

## Task 5: Biome parser

Recognises `biome check`, `npm run lint:web`, and `npm run lint` when the
script is Biome. Extracts path, line, column, rule name (`lint/…` →
`code`), and severity. Fixture: real `npm run lint:web` output from this
repository with a lint error seeded into a scratch copy.

## Task 6: Browser findings

**Files:** `web_diagnostics.rs`. **Updated 2026-09-29 by spec 12's
close-out:** read `context.md` §7.1 first.

Spec 12 already types the browser evidence as
`WebDiagnosticReport.details: Option<WebDiagnosticDetails>`. **Do not add
`WebDiagnosticEntry`, `WebEntryKind` or `WebDiagnosticReport.entries`.**
Findings come from `details`:

- each of `page_errors`;
- each `console` entry for which `WebConsoleEntry::is_problem()` is true,
  whose `location` becomes a `SourceRange` when it resolves inside the
  repository;
- each of `failed_requests`.

A report with `details: None` and `tool_failed()` true yields one generic
finding. A location that does not resolve to a path inside the repository root
is dropped to `range: None`: a served URL no rule maps, a bundled URL,
`node_modules`, or an absolute path outside the root. The report is already
redacted where the engine records it, but `Finding::new` still redacts
(`context.md` §6). Proposal §7's question about `entries` is answered in
`context.md` §7.1: they were never added, because spec 12 supplied
`WebDiagnosticDetails` instead.

## Task 7: Recording, persistence, staleness, `Evidence::Findings`

**Files:** `session.rs`, `plan.rs`, `validation.rs`, and possibly `chat.rs`.

`SessionStore::record_finding(session_id, &Finding)` appends
`finding_recorded`, and `set_finding_status(session_id, finding_id, status)`
appends `finding_status_changed`. `read_findings(session_id, root) -> Vec<Finding>`
replays them, with the newest status winning, then computes `Stale` from
`file_hash` (`context.md` §1). A dismissed or fixed finding is not re-marked
stale. The recording call site sets `task_id`, `origin_ref` (the execution
id), and `file_hash`. Add `Evidence::Findings { refs, failing }` to close
spec 21 `context.md` §3.6, and update that spec's deferral note. Acceptance:
findings survive a restart with their statuses intact.

## Task 8: Dismissal and the scoped repair request

**Files:** `finding.rs`, `session.rs`.

`RepairRequest { findings: Vec<RepairItem>, excluded_stale: Vec<String> }`,
built from selected ids. Stale findings are excluded with a note. The request
carries the findings' summary, range, and code, never raw output (§5.5). It
renders to the model-facing prompt text. Test: dismissing a finding does not
suppress a new finding for the same problem from a later check.

## Task 9: Shell API

**Files:** `crates/desktop-shell/src/lib.rs`.

`GET /api/findings?sessionId=`, `POST /api/finding-status` (dismiss), and
`POST /api/findings-repair`, which returns the rendered request that the
panel sends as a chat message. Follow spec 20 Task 8's `serve_for_test`
pattern for the HTTP tests.

## Task 10: Findings panel

**Files:** `app.js`, `styles.css`, read against `docs/UI_STYLE_GUIDE.md`.

Group by source, then by file. Filter by severity and status. The default
view is Open plus Error. A finding with a range renders through spec 05's
existing clickable-reference mechanism, with no new navigation path. Each
finding has a dismiss control, and there is a "Fix selected" action. Stale
findings are shown but not selectable, with the reason stated. Verify in the
browser against the ignored `serves_the_ui_for_manual_inspection` harness on
port 4899, never on 4765.

## Task 11: Docs, acceptance criteria, close the spec

`USER_GUIDE.md` and `TROUBLESHOOTING.md` per §5.8. Walk every acceptance
criterion into `proposal.md` §7, including the seeded-secret criterion across
command output, browser output, and the generic fallback. Record the share of
real failures that fell through to the generic parser. Then work through the
AGENTS.md "When a spec becomes Done" checklist: `Depends on:` lines in #23,
#24, #26, #32, and #35; re-deriving "What to build next"; the CHANGELOG
`Unreleased` entry; and `npm run specs:check`. Finish with the full
seven-command gate.
