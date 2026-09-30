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
| 3 · Rust diagnostics parser (`cargo build`/`check`/`clippy`) | Not started | |
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

**Files:** `finding.rs` or a submodule, per Task 2's choice.

Recognises `cargo build`, `cargo check`, and `cargo clippy`. Extracts
`error[E0308]: msg` / `warning: msg` / `error: msg` headers plus the following
`--> path:line:col`. Severity comes from the header word, and `code` comes
from the bracket, or the `#[warn(clippy::…)]` note for clippy. The details are
the diagnostic block. Skip the `warning: N warnings emitted` and
`error: could not compile` summary lines, which are not problems of their own.
A header with no `-->` gets `range: None`. Fixtures are real captured rustc
and clippy output, committed as test strings. Also expose the parser as a
function that Task 4 can call over `cargo test` output.

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
