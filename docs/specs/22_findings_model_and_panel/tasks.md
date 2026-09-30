# Findings Model and Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
decisions it left open in [`context.md`](context.md)
**Started:** 2026-09-25 — **Done:** —

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `Finding`, `FindingDraft`, and the redacting, bounding constructor | Done 2026-09-30 | `finding.rs` landed as sketched (rustfmt only), registered in `lib.rs`. Before the implementation the tests failed to compile (`E0432`). After it, `test(finding::tests)` passed 16/16. Each mutation failed its own test: (1) bound-then-redact failed `a_secret_straddling_…`; (2) no summary redaction failed `new_redacts_…`; (3) `<=`→`<` failed `a_summary_at_exactly_the_bound_…`; (4) a plain byte slice failed `details_are_bounded_on_a_char_boundary` by panicking. Privacy: a struct literal gives `E0451`, and a field read gives `E0616`. Probe these one at a time, because rustc reports `E0616` and stops before `E0451`, so a combined probe shows only one of them. Scoped fmt, clippy `-p workspace-engine` and typos are clean. |
| 2 · Parser trait, dispatch with fall-through, generic parser | Not started | |
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
  exit, a timeout, a cancellation, and a signal (`context.md` §7). The
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
**Files:** `finding.rs`.

`trait FindingParser { fn source(&self) -> FindingSource; fn matches(&self, command: &str) -> bool; fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft>; }`.
The return type is drafts (`context.md` §3). The dispatcher is
`findings_from_execution(execution, parsers, scanner) -> Vec<Finding>`. The
first matching parser wins. When the execution failed and the chosen parser
returned zero drafts, it falls through to the generic parser. The execution
counts as failed on `termination != Exited`, or on `Exited` with
`exit_code != Some(0)` (`context.md` §7). A successful execution yields zero
findings unless a parser found warnings. The generic parser produces **one**
`Error` draft. Its summary is the first non-empty stderr line, or stdout's
when stderr is empty, or `"<command> exited with code N"` / `"timed out"` /
`"was cancelled"` when both are empty. Its details are the last ~40 lines, and
it never has a range. Tests use a stub parser that `matches` and returns
nothing, to pin the fall-through independently of any real parser.

*(Expand into full TDD steps before starting this task.)*

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
