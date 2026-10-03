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
| 4 · Rust test parser (`cargo test`), delegating compile errors to Task 3 | Done 2026-10-01 | Planned on 2026-10-01 against captured cargo 1.98.0 `cargo test` output (`context.md` §10). The captures changed two earlier decisions. The parser returns the diagnostics **and** the test failures, because a warning and failing tests appear in the same run (§10.2 corrects §5). The dispatcher falls through to the generic finding when a failed run has no `Error` draft: a test binary that crashed after a warning otherwise reported only the warning (§10.3 amends §8.2). `cargo nextest run` is out of scope, and a failed nextest run gets one generic finding (§10.5). Landed as sketched, with only rustfmt changes. The fixtures were extracted from this file's line ranges, so they are byte-identical to the plan. `finding/rust_test.rs` is new. `rust_diagnostics.rs` gained the three summary clauses and `workspace_range`, and its `regex` is now `pub(super)`. `finding.rs` registers `RustTestParser` second and falls through on "no `Error` draft". Before the implementation the tests failed to compile (`E0425`, `E0433`, `E0422`). After it, `test(finding::)` passed 71/71 (16 + 17 + 17 + 21), with Task 3's tests unchanged. Mutations: 1, 2, 3, 5, 6, 7, 8, 9 and 10 failed exactly the tests the plan names. Mutation 4 (end a section at a blank line) failed **9**, not 8. The extra one is `a_panic_line_without_a_thread_id_still_has_its_location`, because its inline stdout also has a blank line straight after the section header. Mutation 9 spares that test because its panic line has no thread id. So the plan's "the same eight as mutation 4" is one test short, not a defect. Each mutation took about 9 minutes when run without `--lib`, because every edit to the library relinks all 21 integration-test binaries. Use `cargo nextest run -p workspace-engine --lib -E 'test(finding::)'` for mutation loops. Scoped fmt, clippy `-p workspace-engine`, `test(finding::)` and typos are clean. typos needed no fixture exclusion. |
| 5 · Biome parser | Done 2026-10-02 | Planned in full on 2026-10-01 against captured Biome 2.5.7 output (`context.md` §11). Landed as sketched, with only rustfmt changes. The fixtures were extracted from this file's line ranges, so they are byte-identical to the plan, and `grep -c '^  $'` on the check stderr prints 28. `finding/biome.rs` is new. `finding.rs` declares `mod biome;` and registers `BiomeParser` third. Before the implementation the tests failed to compile (`E0425` on `parse_biome`/`BiomeParser`, plus `E0433`). After it, `test(finding::)` passed 89/89 (71 unchanged + 18). All eight mutations failed their named test. (1) Ending a block at a whitespace-only line failed 10 tests, `details_run_past_…` among them: the empty line after each header ends the block before its marker, so every summary falls back to the category. (2) A greedy path failed 5, including `a_lint_error_keeps_…`. (3) No OSC 8 alternative failed only `forced_colour_output_parses_the_same`. (4) Plain-only markers failed `colour_markers_map_like_plain_ones` and the forced-colour test. (5) Non-error→`Warning` failed `an_info_marker_is_info`, `a_passing_run_keeps_its_info` and the colour-marker test. (6) No `HIDDEN` check failed only `hidden_diagnostics_become_one_info_draft`. (7) Any `npm run` script failed only `does_not_match_other_commands`. (8) Skipping marker-less blocks failed only `a_block_without_a_marker_is_kept_…`. Scoped fmt, clippy `-p workspace-engine`, `test(finding::)` and typos are clean. typos needed no fixture exclusion. |
| 6 · Browser findings from spec 12's `WebDiagnosticDetails` | Done 2026-10-02 | Re-scoped on 2026-09-29 by spec 12's close-out, so there is no `entries` field (`context.md` §7.1). Planned in full on 2026-10-02 (`context.md` §12). Landed as sketched, with only rustfmt changes. `browser.rs` was extracted from this file's line ranges, so before formatting it was byte-identical to the plan. rustfmt rewrapped lines in `browser.rs` and sorted `mod browser;` below `mod biome;`, not above it as Step 2 says. It did not touch the `finding.rs` test array, contrary to Step 7's note. `finding/browser.rs` is new. `finding.rs` adds `FindingSource::BrowserScenario` (`"browser_scenario"`), broadens `Command`'s doc and re-exports `findings_from_web_record`. In `web_diagnostics.rs`, the three `model_item`s and `is_loopback_url` became `pub(crate)`, and nothing else changed. Steps 1 and 2 were applied together, so Step 1's separate test run was skipped. Before the implementation the tests failed to compile (`E0425` on `findings_from_web_record`/`served_path`, `E0432` on the re-export). After it, `test(finding::) + test(web_diagnostics::)` passed 121/121: 107 `finding::` (89 + 18 `browser::tests`), and the `web_diagnostics::` tests were unchanged. All six mutations failed only their named tests. (1) No `node_modules` exclusion failed only `a_console_error_at_a_served_url_maps_to_…`. (2) First candidate wins failed only `an_ambiguous_served_path_has_no_range`. (3) No `is_loopback_url` failed `a_console_location_on_another_origin_has_no_range` and `served_path_accepts_only_loopback_…`. (4) No `!explained` failed only `a_tool_failure_is_not_doubled_…`. (5) Skipping failed steps failed only `a_failed_scenario_step_is_a_browser_scenario_error`. (6) Every problem level as `Error` failed only `an_inspection_yields_one_finding_per_problem_…`. Scoped fmt, clippy `-p workspace-engine`, `test(finding::) + test(web_diagnostics::)` and typos are clean. |
| 7 · Persistence, derived status, `Evidence::Findings` | Done 2026-10-03 | Split on 2026-10-02 from the old "Recording, persistence, staleness" task (`context.md` §13.1) and planned in full. Landed as sketched, with only rustfmt changes. The tests and implementation were extracted from this file's line ranges. `session.rs` gains `record_finding`, `set_finding_status` and `read_findings` after `read_session_web_diagnostics`, and the free functions `replay_findings` and `finding_is_stale` before `parse_session_log`. The `let`-chains compiled as written. `plan.rs` gains `Evidence::Findings` and its two match arms. rustfmt turned the `status_from_evidence` arm into a block. Two comments in `plan.rs` were reworded beyond the plan because the new variant made them stale: the `#[non_exhaustive]` note, which said `Findings` "joins this enum when spec 22 exists", and the "Neither has a failure mode" comment, which now covers three variants. Step 1's two task-number comments were updated. Before the implementation, the session tests failed to compile (`E0599` on all three methods). After it, `test(session::tests) + binary(plan) + test(finding::)` passed 181/181. That includes the 15 new session tests (`session::tests` is now 23) and the 4 new plan tests (`binary(plan)` is now 51). All seven mutations failed only their named test. Mutations 1–6 ran with `--lib -E 'test(session::tests)'`, and 7 ran with `binary(plan)`. (1) Checking every status failed `a_dismissed_finding_is_not_re_marked_stale`. (2) Missing hash as stale failed `a_finding_without_a_recorded_hash_is_never_stale`. (3) `Err(_) => false` failed `a_deleted_file_makes_its_finding_stale`. (4) No `Stale` refusal failed `setting_stale_directly_is_refused_…`. (5) No unknown-id check failed `a_status_change_for_an_unknown_finding_…`. (6) `parsed_events(content).0` failed `a_rewind_takes_the_findings_…`. (7) `*failing > 0` failed `findings_evidence_alone_does_not_block_a_step`. No other crate needed a change. Scoped fmt, clippy `-p workspace-engine`, the scoped tests and typos are clean. |
| 8 · Record findings where checks run (`chat.rs`) | Not started | New on 2026-10-02 from that split; outline only. Its facts are in `context.md` §13.4. |
| 9 · Dismissal and the scoped repair request | Not started | |
| 10 · Shell API | Not started | |
| 11 · Findings panel | Not started | |
| 12 · Docs, acceptance criteria, close the spec | Not started | |

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
reader (Tasks 10–11).

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
- **Never lose a failure** (§5.3). An execution that failed and produced no
  parsed draft of severity `Error` falls through to the generic parser
  (`context.md` §10.3; warnings alone do not explain a failure). That covers a non-zero
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
  once, in Task 12.** `cargo nextest run --workspace` takes about 5 minutes
  locally, and `cargo clippy --workspace --all-targets` takes up to 18
  minutes cold.
- **`chat.rs` is contended.** See `docs/specs/README.md` "What to build next"
  → Parallel work. Tasks 1–7 and 9 do not touch it. Task 8 does, where a check
  runs inside a turn. Check that section before running Task 8 alongside
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
| `crates/workspace-engine/src/validation.rs` (and `chat.rs` if needed) | The recording call sites (Task 8) |
| `crates/desktop-shell/src/lib.rs` | Findings endpoints (Task 10) |
| `crates/desktop-shell/static/app.js`, `styles.css` | The panel (Task 11) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | §5.8 (Task 12) |

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
  (`validation.rs:30`). This is where Task 8 gets `origin_ref`.
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
reads findings in Task 10. Spec 20 Task 8 had to widen `pub(crate)` items
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
    persists exactly this shape, and Task 10 serves it.

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

      /// The persisted and served shape (Tasks 7 and 10 depend on it).
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
    parser output. Task 8 calls it and then attaches `task_id`, `origin_ref`
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

**Requirements:** 1 (test source), 2, 3, and the acceptance criteria "a
failing test … normalise[s] into `Finding` with correct source, severity,
and, where the tool reported one, file and range", "a check whose output no
parser recognises still produces exactly one generic finding", and "no parser
produces a `range` for output that contained no location". **Files:** create
`crates/workspace-engine/src/finding/rust_test.rs` and four fixture files
under `crates/workspace-engine/src/finding/fixtures/`. Modify
`crates/workspace-engine/src/finding/rust_diagnostics.rs` (three summary
lines, a shared `workspace_range`, and `regex` made `pub(super)`) and
`crates/workspace-engine/src/finding.rs` (module declaration, registration,
the fall-through rule, and one dispatch test). Planned in full on 2026-10-01.

**Read `context.md` §10 first.** It records what real cargo 1.98.0
`cargo test` output looks like, and each rule below comes from it. Three of
its findings change earlier decisions:

- the parser returns the diagnostics **and** the test failures, not one or the
  other (§10.2 corrects §5);
- the dispatcher falls through to the generic finding when a failed run has
  no `Error` draft, not only when it has no drafts (§10.3 amends §8.2);
- `cargo nextest run` is out of scope and reaches the generic fallback
  (§10.5).

**Interfaces:**
- Consumes: `parse_rust_diagnostics`, `cargo_subcommand`,
  `RustDiagnosticsParser` (Task 3, `finding/rust_diagnostics.rs`);
  `FindingParser`, `FindingDraft`, `FindingSource`, `Severity`, `SourceRange`,
  `default_parsers` and `findings_from_execution` (Tasks 1–2, `finding.rs`);
  `CommandExecution` (`command_runner.rs`).
- Produces:
  - `pub(super) struct RustTestParser`, registered second in
    `default_parsers()`. It matches `cargo test` and `cargo t`.
  - `pub(super) fn parse_test_failures(stdout: &str) -> Vec<FindingDraft>`.
  - `pub(super) fn workspace_range(path, line, column) -> Option<SourceRange>`
    in `rust_diagnostics.rs`, which holds §9's absolute-or-`..` rule. Task 5
    may reuse it.
  - For Task 5: a failed run whose parser returns only warnings now also gets
    a generic finding (§10.3). A Biome run that fails because of its warnings
    therefore shows both.

- [x] **Step 1: Commit the fixtures as files**

  Write these four files into `crates/workspace-engine/src/finding/fixtures/`
  **byte for byte**. They are cargo 1.98.0 captures, scrubbed as `context.md`
  §10 describes: the workspace path is `/repo`, the doctest's temporary
  directory is `/tmp/`, and the `Finished` and `all doctests ran in` lines
  are dropped. Each file ends with a single newline. The two `_stdout` files
  **begin with one empty line**, as libtest prints it. The fixture lines sit
  at column 0 here, exactly as captured.

  `rust_test_compile_error_stderr.txt` is the stderr of `cargo test` with a
  type error in a test. stdout was empty. It exits 101.

  ````text
   Compiling demo v0.1.0 (/repo/crates/demo)
error[E0308]: mismatched types
  --> crates/demo/src/lib.rs:11:22
   |
11 |         let x: u32 = "three";
   |                ---   ^^^^^^^ expected `u32`, found `&str`
   |                |
   |                expected due to this

For more information about this error, try `rustc --explain E0308`.
error: could not compile `demo` (lib test) due to 1 previous error
  ````

  `rust_test_failures_stdout.txt` is the stdout of
  `cargo test --no-fail-fast`. It has seven unit tests (one passes), an
  integration test binary, and a doctest. It exits 101.

  ````text

running 7 tests
test tests::adds ... ok
test tests::returns_err ... FAILED
test tests::should_have_panicked - should panic ... FAILED
test tests::panicked_with_the_wrong_message - should panic ... FAILED
test tests::assert_eq_fails ... FAILED
test tests::panics_with_message ... FAILED
test tests::unwrap_in_library_code ... FAILED

failures:

---- tests::returns_err stdout ----
Error: "config missing"

---- tests::should_have_panicked stdout ----
note: test did not panic as expected at crates/demo/src/lib.rs:42:8
---- tests::panicked_with_the_wrong_message stdout ----

thread 'tests::panicked_with_the_wrong_message' (9907643) panicked at crates/demo/src/lib.rs:49:9:
underflow
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
note: panic did not contain expected string
      panic message: "underflow"
 expected substring: "overflow"
---- tests::assert_eq_fails stdout ----

thread 'tests::assert_eq_fails' (9907642) panicked at crates/demo/src/lib.rs:32:9:
assertion `left == right` failed: two plus two
  left: 4
 right: 5

---- tests::panics_with_message stdout ----
preparing the widget

thread 'tests::panics_with_message' (9907644) panicked at crates/demo/src/lib.rs:27:9:
the widget was not ready

---- tests::unwrap_in_library_code stdout ----

thread 'tests::unwrap_in_library_code' (9907647) panicked at crates/demo/src/lib.rs:11:15:
called `Result::unwrap()` on an `Err` value: ParseIntError { kind: InvalidDigit }


failures:
    tests::assert_eq_fails
    tests::panicked_with_the_wrong_message
    tests::panics_with_message
    tests::returns_err
    tests::should_have_panicked
    tests::unwrap_in_library_code

test result: FAILED. 1 passed; 6 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 2 tests
test integration_passes ... ok
test integration_fails ... FAILED

failures:

---- integration_fails stdout ----

thread 'integration_fails' (9907650) panicked at crates/demo/tests/integration.rs:8:5:
sum too small
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    integration_fails

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 1 test
test crates/demo/src/lib.rs - add (line 3) ... FAILED

failures:

---- crates/demo/src/lib.rs - add (line 3) stdout ----
Test executable failed (exit status: 101).

stderr:

thread 'main' (9907728) panicked at /tmp/rustdoctestVr99Wj/doctest_bundle_2024.rs:6:1:
assertion `left == right` failed
  left: 2
 right: 3
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace



failures:
    crates/demo/src/lib.rs - add (line 3)

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
  ````

  `rust_test_failures_stderr.txt` is the same run's stderr. It is a second
  run, so it has no `Compiling` line.

  ````text
warning: unused variable: `unused`
  --> crates/demo/src/lib.rs:20:13
   |
20 |         let unused = 1;
   |             ^^^^^^ help: if this is intentional, prefix it with an underscore: `_unused`
   |
   = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `demo` (lib test) generated 1 warning (run `cargo fix --lib -p demo --tests` to apply 1 suggestion)
     Running unittests src/lib.rs (target/debug/deps/demo-379d40ceacbd0d01)
error: test failed, to rerun pass `--lib`
     Running tests/integration.rs (target/debug/deps/integration-e39223e19cdb234a)
error: test failed, to rerun pass `--test integration`
   Doc-tests demo
error: doctest failed, to rerun pass `--doc`
error: 3 targets failed:
    `--lib`
    `--test integration`
    `--doc`
  ````

  `rust_test_crash_stderr.txt` is the stderr of `cargo test` in a one-package
  crate whose only test calls `std::process::abort()`. Its stdout was
  `"\nrunning 1 test\n"`, which the tests inline. It exits 101.

  ````text
   Compiling crash v0.1.0 (/repo)
warning: unused variable: `unused`
 --> src/lib.rs:5:13
  |
5 |         let unused = 1;
  |             ^^^^^^ help: if this is intentional, prefix it with an underscore: `_unused`
  |
  = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `crash` (lib test) generated 1 warning (run `cargo fix --lib -p crash --tests` to apply 1 suggestion)
     Running unittests src/lib.rs (target/debug/deps/crash-46156d6990275a20)
error: test failed, to rerun pass `--lib`

Caused by:
  process didn't exit successfully: `/repo/target/debug/deps/crash-46156d6990275a20` (signal: 6, SIGABRT: process abort signal)
  ````

- [x] **Step 2: Declare the module and write the failing tests**

  In `finding.rs`, add `mod rust_test;` directly below
  `mod rust_diagnostics;`. In `finding.rs`'s `dispatch_tests`, add this test
  directly above `a_passing_execution_with_no_parser_yields_nothing`:

  ```rust
  /// `context.md` §10: a crashed `cargo test` after a compiler warning.
  #[test]
  fn a_failure_whose_parser_found_only_warnings_also_gets_the_generic_finding() {
      let parsers = [stub(
          "cargo test",
          vec![draft(FindingSource::Compiler, Severity::Warning, "unused")],
      )];
      let findings = run(&failed("cargo test", "", "SIGABRT\n"), &parsers);
      let sources: Vec<_> = findings.iter().map(Finding::source).collect();
      assert_eq!(sources, [FindingSource::Compiler, FindingSource::Command]);
      assert_eq!(findings[1].summary(), "cargo test: SIGABRT");
  }
  ```

  Then create `crates/workspace-engine/src/finding/rust_test.rs` containing
  only the test module:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::command_policy::CommandRisk;
      use crate::command_runner::{CommandExecution, CommandTermination};
      use crate::finding::{default_parsers, findings_from_execution};
      use crate::secret_scanner::SecretScanner;

      const COMPILE_ERROR_STDERR: &str = include_str!("fixtures/rust_test_compile_error_stderr.txt");
      const FAILURES_STDOUT: &str = include_str!("fixtures/rust_test_failures_stdout.txt");
      const FAILURES_STDERR: &str = include_str!("fixtures/rust_test_failures_stderr.txt");
      const CRASH_STDERR: &str = include_str!("fixtures/rust_test_crash_stderr.txt");
      /// The crash capture's whole stdout: the binary died before reporting.
      const CRASH_STDOUT: &str = "\nrunning 1 test\n";

      fn range(path: &str, line: u32, column: Option<u32>) -> Option<SourceRange> {
          Some(SourceRange {
              path: path.to_string(),
              start_line: line,
              start_column: column,
              end_line: None,
              end_column: None,
          })
      }

      fn execution(command: &str, exit_code: i32, stdout: &str, stderr: &str) -> CommandExecution {
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
              stdout: stdout.to_string(),
              stderr: stderr.to_string(),
          }
      }

      fn failure<'a>(drafts: &'a [FindingDraft], name: &str) -> &'a FindingDraft {
          drafts
              .iter()
              .find(|draft| draft.summary.starts_with(&format!("{name}: ")))
              .unwrap_or_else(|| panic!("no finding for {name}: {drafts:#?}"))
      }

      /// `context.md` §5: a compile error has no `failures:` block, so the
      /// diagnostics parser is what reads it.
      #[test]
      fn a_compile_error_under_cargo_test_is_a_compiler_finding_with_its_location() {
          let drafts = RustTestParser.parse(&execution("cargo test", 101, "", COMPILE_ERROR_STDERR));
          assert_eq!(drafts.len(), 1, "{drafts:#?}");
          assert_eq!(drafts[0].source, FindingSource::Compiler);
          assert_eq!(drafts[0].severity, Severity::Error);
          assert_eq!(drafts[0].code.as_deref(), Some("E0308"));
          assert_eq!(
              drafts[0].range,
              range("crates/demo/src/lib.rs", 11, Some(22))
          );
      }

      /// libtest sorts the `failures:` list by name, so the order is stable
      /// even though the sections above it come in thread-finishing order.
      #[test]
      fn each_failed_test_is_one_test_error_in_failures_list_order() {
          let drafts = parse_test_failures(FAILURES_STDOUT);
          let names: Vec<_> = drafts
              .iter()
              .map(|draft| draft.summary.split(": ").next().unwrap_or_default())
              .collect();
          assert_eq!(
              names,
              [
                  "tests::assert_eq_fails",
                  "tests::panicked_with_the_wrong_message",
                  "tests::panics_with_message",
                  "tests::returns_err",
                  "tests::should_have_panicked",
                  "tests::unwrap_in_library_code",
                  "integration_fails",
                  "crates/demo/src/lib.rs - add (line 3)",
              ]
          );
          assert!(drafts.iter().all(|d| d.source == FindingSource::Test));
          assert!(drafts.iter().all(|d| d.severity == Severity::Error));
          assert!(drafts.iter().all(|d| d.code.is_none()));
      }

      /// The test printed a line before panicking, so the message is the line
      /// after `panicked at`, not the section's first line.
      #[test]
      fn a_panic_takes_its_location_and_message() {
          let drafts = parse_test_failures(FAILURES_STDOUT);
          let panic = failure(&drafts, "tests::panics_with_message");
          assert_eq!(
              panic.summary,
              "tests::panics_with_message: the widget was not ready"
          );
          assert_eq!(panic.range, range("crates/demo/src/lib.rs", 27, Some(9)));
      }

      #[test]
      fn an_assert_eq_failure_keeps_its_values_in_the_details() {
          let drafts = parse_test_failures(FAILURES_STDOUT);
          let assert = failure(&drafts, "tests::assert_eq_fails");
          assert_eq!(
              assert.summary,
              "tests::assert_eq_fails: assertion `left == right` failed: two plus two"
          );
          assert_eq!(assert.range, range("crates/demo/src/lib.rs", 32, Some(9)));
          let details = assert.details.as_deref().expect("details");
          assert!(
              details.starts_with("---- tests::assert_eq_fails stdout ----"),
              "{details}"
          );
          assert!(details.contains("\n  left: 4\n right: 5"), "{details}");
      }

      /// Its section runs straight into the next one with no blank line
      /// (`context.md` §10).
      #[test]
      fn a_should_panic_that_did_not_panic_points_at_the_test() {
          let drafts = parse_test_failures(FAILURES_STDOUT);
          let missed = failure(&drafts, "tests::should_have_panicked");
          assert_eq!(
              missed.summary,
              "tests::should_have_panicked: test did not panic as expected at crates/demo/src/lib.rs:42:8"
          );
          assert_eq!(missed.range, range("crates/demo/src/lib.rs", 42, Some(8)));
          let details = missed.details.as_deref().expect("details");
          assert!(
              !details.contains("underflow"),
              "the next section leaked in: {details}"
          );
      }

      /// The panic message, `underflow`, is not why the test failed.
      #[test]
      fn a_wrong_panic_message_is_summarised_by_the_note_not_the_panic() {
          let drafts = parse_test_failures(FAILURES_STDOUT);
          let wrong = failure(&drafts, "tests::panicked_with_the_wrong_message");
          assert_eq!(
              wrong.summary,
              "tests::panicked_with_the_wrong_message: panic did not contain expected string"
          );
          assert_eq!(wrong.range, range("crates/demo/src/lib.rs", 49, Some(9)));
      }

      /// A test returning `Err` prints no location. It must not get one.
      #[test]
      fn a_test_returning_err_has_no_range() {
          let drafts = parse_test_failures(FAILURES_STDOUT);
          let err = failure(&drafts, "tests::returns_err");
          assert_eq!(err.summary, "tests::returns_err: Error: \"config missing\"");
          assert_eq!(err.range, None);
      }

      /// `#[track_caller]` puts the location in the library function the test
      /// called. That is where it panicked, so that is the range.
      #[test]
      fn a_panic_in_library_code_points_where_it_panicked() {
          let drafts = parse_test_failures(FAILURES_STDOUT);
          let unwrap = failure(&drafts, "tests::unwrap_in_library_code");
          assert_eq!(unwrap.range, range("crates/demo/src/lib.rs", 11, Some(15)));
      }

      #[test]
      fn a_failure_in_an_integration_test_binary_is_found_too() {
          let drafts = parse_test_failures(FAILURES_STDOUT);
          let integration = failure(&drafts, "integration_fails");
          assert_eq!(integration.summary, "integration_fails: sum too small");
          assert_eq!(
              integration.range,
              range("crates/demo/tests/integration.rs", 8, Some(5))
          );
      }

      /// A doctest panics in a temporary file, which is not the user's code.
      /// Its name carries the path and line of the doc block instead.
      #[test]
      fn a_doctest_points_at_its_doc_block_not_the_temporary_file() {
          let drafts = parse_test_failures(FAILURES_STDOUT);
          let doctest = failure(&drafts, "crates/demo/src/lib.rs - add (line 3)");
          assert_eq!(
              doctest.summary,
              "crates/demo/src/lib.rs - add (line 3): assertion `left == right` failed"
          );
          assert_eq!(doctest.range, range("crates/demo/src/lib.rs", 3, None));
      }

      /// `context.md` §6: tail truncation can remove a section and keep the
      /// list. The name is still a finding. The location is not guessed.
      #[test]
      fn a_failure_whose_section_was_cut_off_keeps_its_name_and_no_range() {
          let stdout = "failures:\n    tests::lost\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
          let drafts = parse_test_failures(stdout);
          assert_eq!(drafts.len(), 1, "{drafts:#?}");
          assert_eq!(drafts[0].summary, "tests::lost failed");
          assert_eq!(drafts[0].range, None);
          assert_eq!(drafts[0].details, None);
      }

      /// Each binary's list reads only that binary's sections. The second
      /// binary's `tests::same` printed no section, so it must not borrow the
      /// first binary's location.
      #[test]
      fn a_name_repeated_in_a_later_binary_does_not_borrow_an_earlier_section() {
          let stdout = "\
  failures:

  ---- tests::same stdout ----

  thread 'tests::same' (1) panicked at crates/a/src/lib.rs:4:9:
  first


  failures:
      tests::same

  test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


  running 1 test
  test tests::same ... FAILED

  failures:
      tests::same

  test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
  ";
          let drafts = parse_test_failures(stdout);
          assert_eq!(drafts.len(), 2, "{drafts:#?}");
          assert_eq!(drafts[0].range, range("crates/a/src/lib.rs", 4, Some(9)));
          assert_eq!(drafts[1].range, None);
          assert_eq!(drafts[1].summary, "tests::same failed");
      }

      /// cargo 1.98.0 prints a thread id in brackets. Earlier toolchains print
      /// the same line without it.
      #[test]
      fn a_panic_line_without_a_thread_id_still_has_its_location() {
          let stdout = "failures:\n\n---- tests::old stdout ----\n\nthread 'tests::old' panicked at src/lib.rs:5:9:\nboom\n\n\nfailures:\n    tests::old\n";
          let drafts = parse_test_failures(stdout);
          assert_eq!(drafts.len(), 1, "{drafts:#?}");
          assert_eq!(drafts[0].summary, "tests::old: boom");
          assert_eq!(drafts[0].range, range("src/lib.rs", 5, Some(9)));
      }

      #[test]
      fn a_passing_run_has_no_failures() {
          let stdout = "\nrunning 1 test\ntest tests::adds ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
          assert!(parse_test_failures(stdout).is_empty());
      }

      /// `context.md` §10: cargo's per-target lines on `cargo test`'s stderr use
      /// the diagnostic header shape, and are summaries, not diagnostics.
      #[test]
      fn cargo_test_target_lines_on_stderr_are_not_findings() {
          let drafts = parse_rust_diagnostics(FAILURES_STDERR);
          let summaries: Vec<_> = drafts.iter().map(|d| d.summary.as_str()).collect();
          assert_eq!(summaries, ["unused variable: `unused`"]);
      }

      /// Registration, end to end. The compiler warning on stderr must not
      /// stand in for the test failures on stdout (`context.md` §10 corrects
      /// §5's "uses its drafts when it returns any").
      #[test]
      fn default_parsers_turn_a_failed_cargo_test_into_compiler_and_test_findings() {
          let findings = findings_from_execution(
              &execution(
                  "cargo test --no-fail-fast",
                  101,
                  FAILURES_STDOUT,
                  FAILURES_STDERR,
              ),
              &default_parsers(),
              &SecretScanner::default(),
          );
          let sources: Vec<_> = findings.iter().map(|f| f.source()).collect();
          let mut expected = vec![FindingSource::Compiler];
          expected.extend([FindingSource::Test; 8]);
          assert_eq!(sources, expected, "{findings:#?}");
          assert_eq!(findings[0].severity(), Severity::Warning);
      }

      /// The crash capture: a warning, then a test binary killed by SIGABRT
      /// before it printed a `failures:` list. The warning alone must not
      /// swallow the failure (`context.md` §10).
      #[test]
      fn a_crashed_test_binary_with_a_warning_still_gets_the_generic_finding() {
          let findings = findings_from_execution(
              &execution("cargo test", 101, CRASH_STDOUT, CRASH_STDERR),
              &default_parsers(),
              &SecretScanner::default(),
          );
          assert_eq!(findings.len(), 2, "{findings:#?}");
          assert_eq!(findings[0].code(), Some("unused_variables"));
          assert_eq!(findings[0].severity(), Severity::Warning);
          assert_eq!(findings[1].source(), FindingSource::Command);
          assert!(findings[1].details().unwrap().contains("SIGABRT"));
      }

      /// §5.3's fall-through, against this parser.
      #[test]
      fn a_failed_cargo_test_this_parser_cannot_read_falls_through_to_generic() {
          let findings = findings_from_execution(
              &execution("cargo test", 101, "\nrunning 3 tests\n", "Killed: 9\n"),
              &default_parsers(),
              &SecretScanner::default(),
          );
          assert_eq!(findings.len(), 1, "{findings:#?}");
          assert_eq!(findings[0].source(), FindingSource::Command);
          assert_eq!(findings[0].summary(), "cargo test: Killed: 9");
      }

      #[test]
      fn a_passing_cargo_test_yields_nothing() {
          let stdout = "\nrunning 1 test\ntest tests::adds ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
          let stderr = "   Compiling demo v0.1.0 (/repo/crates/demo)\n     Running unittests src/lib.rs (target/debug/deps/demo-379d40ceacbd0d01)\n   Doc-tests demo\n";
          let findings = findings_from_execution(
              &execution("cargo test", 0, stdout, stderr),
              &default_parsers(),
              &SecretScanner::default(),
          );
          assert!(findings.is_empty(), "{findings:#?}");
      }

      #[test]
      fn matches_cargo_test_in_its_common_forms() {
          for command in [
              "cargo test",
              "cargo t",
              "cargo test --workspace --no-fail-fast",
              "cargo +nightly test -p demo",
              "RUST_BACKTRACE=1 cargo test",
              "cd crates/demo && cargo test",
          ] {
              assert!(RustTestParser.matches(command), "{command}");
          }
      }

      /// `cargo nextest run` is out of scope (`context.md` §10): no parser
      /// matches it, so a failure reaches the generic fallback.
      #[test]
      fn does_not_match_nextest_or_other_commands() {
          for command in [
              "cargo nextest run",
              "cargo build",
              "cargo bench",
              "npm test",
              "cargo",
          ] {
              assert!(!RustTestParser.matches(command), "{command}");
          }
          let findings = findings_from_execution(
              &execution(
                  "cargo nextest run",
                  100,
                  "",
                  "        FAIL [   0.013s] (1/8) demo tests::panics_with_message\nerror: test run failed\n",
              ),
              &default_parsers(),
              &SecretScanner::default(),
          );
          assert_eq!(findings.len(), 1, "{findings:#?}");
          assert_eq!(findings[0].source(), FindingSource::Command);
      }
  }
  ```

- [x] **Step 3: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::)'`
  Expected: a compile failure. `RustTestParser`, `parse_test_failures`,
  `FindingDraft`, `SourceRange`, `FindingSource`, `Severity` and
  `parse_rust_diagnostics` are not in scope in `rust_test.rs`, which gives
  `E0425`, `E0433` and `E0422`. The new dispatch test compiles, but the crate
  does not, so its failure is shown by mutation 2 in Step 6 instead.

- [x] **Step 4: Write the implementation**

  In `rust_diagnostics.rs`, make three changes.

  1. Make the regex helper visible to the sibling module:

     ```rust
     pub(super) fn regex(pattern: &str) -> Regex {
     ```

  2. Teach `is_cargo_summary` cargo's `cargo test` lines (`context.md`
     §10.2). Add this static below `EMITTED`:

     ```rust
     static TARGETS_FAILED: LazyLock<Regex> = LazyLock::new(|| regex(r"^\d+ targets? failed"));
     ```

     Then add three clauses after `build failed`:

     ```rust
             || message.starts_with("build failed")
             || message.starts_with("test failed, to rerun pass ")
             || message.starts_with("doctest failed, to rerun pass ")
             || TARGETS_FAILED.is_match(message)
     ```

  3. Move the path rule out of `primary_location` into a shared function, so
     the test parser applies the same rule to `panicked at` locations. The
     tail of `primary_location` becomes one call:

     ```rust
     fn primary_location(block: &[&str]) -> Option<SourceRange> {
         let captures = block[1..]
             .iter()
             .take_while(|line| !CHILD_HEADER.is_match(line))
             .find_map(|line| LOCATION.captures(line))?;
         workspace_range(&captures[1], &captures[2], Some(&captures[3]))
     }

     /// A printed `path:line[:column]` as a range, or `None` when the path is
     /// absolute or climbs out with `..`: registry sources, the standard library,
     /// a doctest's temporary file (`context.md` §9, §10).
     pub(super) fn workspace_range(path: &str, line: &str, column: Option<&str>) -> Option<SourceRange> {
         let outside = Path::new(path).is_absolute()
             || Path::new(path)
                 .components()
                 .any(|part| part == Component::ParentDir);
         if outside {
             return None;
         }
         Some(SourceRange {
             path: path.to_string(),
             start_line: line.parse().ok()?,
             start_column: column.and_then(|column| column.parse().ok()),
             end_line: None,
             end_column: None,
         })
     }
     ```

  Put this above the test module in `rust_test.rs`:

  ```rust
  //! `cargo test` failures (spec 22 Task 4).
  //!
  //! Compiler diagnostics on stderr come first, through
  //! `parse_rust_diagnostics` and its own severity mapping (`context.md` §5).
  //! Then each name in a `failures:` list on stdout is one `Test` finding of
  //! severity `Error`: libtest reports no other level, and no code. Every rule
  //! here comes from real output recorded in
  //! `docs/specs/22_findings_model_and_panel/context.md` §10.

  use super::rust_diagnostics::{cargo_subcommand, parse_rust_diagnostics, regex, workspace_range};
  use super::{FindingDraft, FindingParser, FindingSource, Severity, SourceRange};
  use crate::command_runner::CommandExecution;
  use regex::Regex;
  use std::collections::HashMap;
  use std::sync::LazyLock;

  static SECTION: LazyLock<Regex> = LazyLock::new(|| regex(r"^---- (.+) stdout ----$"));
  static PANICKED: LazyLock<Regex> =
      LazyLock::new(|| regex(r"^thread '[^']*'(?: \(\d+\))? panicked at (.+):(\d+):(\d+):$"));
  static DID_NOT_PANIC: LazyLock<Regex> =
      LazyLock::new(|| regex(r"^note: test did not panic as expected at (.+):(\d+):(\d+)$"));
  static DOCTEST_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\S+) - .+ \(line (\d+)\)$"));

  pub(super) struct RustTestParser;

  impl FindingParser for RustTestParser {
      fn source(&self) -> FindingSource {
          FindingSource::Test
      }

      fn matches(&self, command: &str) -> bool {
          matches!(cargo_subcommand(command), Some("test" | "t"))
      }

      /// Both, not either: a build that printed a warning can still fail its
      /// tests (`context.md` §10).
      fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft> {
          let mut drafts = parse_rust_diagnostics(&execution.stderr);
          drafts.extend(parse_test_failures(&execution.stdout));
          drafts
      }
  }

  /// One draft per name in each `failures:` list. A list reads only the
  /// `---- name stdout ----` sections printed since the previous list, because
  /// `--no-fail-fast` prints one block per test binary and names can repeat.
  pub(super) fn parse_test_failures(stdout: &str) -> Vec<FindingDraft> {
      let lines: Vec<&str> = stdout.lines().collect();
      let mut sections: HashMap<&str, &[&str]> = HashMap::new();
      let mut drafts = Vec::new();
      let mut index = 0;
      while index < lines.len() {
          if let Some(header) = SECTION.captures(lines[index]) {
              let end = lines[index + 1..]
                  .iter()
                  .position(|line| SECTION.is_match(line) || *line == "failures:")
                  .map_or(lines.len(), |offset| index + 1 + offset);
              if let Some(name) = header.get(1) {
                  sections.insert(name.as_str(), &lines[index..end]);
              }
              index = end;
          } else if lines[index] == "failures:" {
              let names: Vec<&str> = lines[index + 1..]
                  .iter()
                  .take_while(|line| line.starts_with("    ") && !line.trim().is_empty())
                  .map(|line| line.trim())
                  .collect();
              index += 1 + names.len();
              if !names.is_empty() {
                  for name in names {
                      drafts.push(test_draft(name, sections.get(name).copied()));
                  }
                  sections.clear();
              }
          } else {
              index += 1;
          }
      }
      drafts
  }

  fn test_draft(name: &str, section: Option<&[&str]>) -> FindingDraft {
      let body = section.map_or(&[][..], |lines| &lines[1..]);
      FindingDraft {
          source: FindingSource::Test,
          severity: Severity::Error,
          summary: match failure_message(body) {
              Some(message) => format!("{name}: {message}"),
              None => format!("{name} failed"),
          },
          details: section.map(|lines| lines.join("\n").trim_end().to_string()),
          range: failure_location(body).or_else(|| doctest_location(name)),
          code: None,
      }
  }

  /// libtest's `#[should_panic]` notes say why the test failed better than the
  /// panic message does. Then the panic message, which is the line after
  /// `panicked at`, then a returned `Err`.
  fn failure_message<'a>(body: &[&'a str]) -> Option<&'a str> {
      let should_panic_note = body.iter().find_map(|line| {
          line.strip_prefix("note: ").filter(|note| {
              note.starts_with("test did not panic as expected")
                  || note.starts_with("panic did not contain expected string")
          })
      });
      let panic_message = || {
          let at = body.iter().position(|line| PANICKED.is_match(line))?;
          body.get(at + 1)
              .copied()
              .filter(|line| !line.trim().is_empty())
      };
      should_panic_note
          .or_else(panic_message)
          .or_else(|| {
              body.iter()
                  .copied()
                  .find(|line| line.starts_with("Error: "))
          })
          .map(str::trim)
  }

  /// The first `panicked at` or `did not panic … at`, when it is inside the
  /// workspace. A doctest panics in a temporary file, which is not.
  fn failure_location(body: &[&str]) -> Option<SourceRange> {
      let captures = body.iter().find_map(|line| {
          PANICKED
              .captures(line)
              .or_else(|| DID_NOT_PANIC.captures(line))
      })?;
      workspace_range(&captures[1], &captures[2], Some(&captures[3]))
  }

  /// A doctest's name is `path - item (line N)`: the doc block's location as
  /// rustdoc printed it.
  fn doctest_location(name: &str) -> Option<SourceRange> {
      let captures = DOCTEST_NAME.captures(name)?;
      workspace_range(&captures[1], &captures[2], None)
  }
  ```

  In `finding.rs`, register the parser and change the fall-through rule
  (`context.md` §10.3):

  ```rust
  pub fn default_parsers() -> Vec<Box<dyn FindingParser>> {
      vec![
          Box::new(rust_diagnostics::RustDiagnosticsParser),
          Box::new(rust_test::RustTestParser),
      ]
  }
  ```

  ```rust
  /// The first parser that matches wins. A failed run whose parser found no
  /// error falls through to one generic finding, so a regex that stops
  /// matching after a tool upgrade cannot swallow a failure (§5.3). Warnings
  /// alone do not explain a failure (`context.md` §10).
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
      let explained = drafts.iter().any(|draft| draft.severity == Severity::Error);
      if !explained && verdict(execution) == Verdict::Failed {
          drafts.push(generic_draft(execution));
      }
      drafts
          .into_iter()
          .map(|draft| Finding::new(draft, scanner))
          .collect()
  }
  ```

  This sketch was compiled and run on 2026-10-01 in a scratch crate. That
  crate held a copy of `finding.rs`, `finding/`, `hash.rs` and
  `secret_scanner.rs`, with stubs for `CommandExecution` and `CommandRisk`.
  It passed 71/71, and `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` were clean. If a test and this sketch disagree, the test
  wins, because the tests come from the captured fixtures. Record any
  deviation in the progress row. Do not weaken a test to fit.

- [x] **Step 5: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::)'`
  Expected: 71 pass. That is 16 in `tests`, 17 in `dispatch_tests` (16 plus
  the new one), 17 in `rust_diagnostics::tests`, and 21 in
  `rust_test::tests`. Count them. Task 3's tests must pass unchanged. The
  `workspace_range` move is a refactor, and Task 3's
  `a_location_outside_the_workspace_has_no_range` guards it. Nothing in the
  build fixtures contains the three new summary lines, so their counts do not
  change.

- [x] **Step 6: Mutation-test the rules that came from real output**

  Apply each change on its own, confirm the named tests fail, then revert it.
  **Revert by undoing the edit, not by copying a saved file back.** A copy
  that keeps the old modification time can leave cargo running the mutated
  build. That happened while this plan was being verified.

  1. In `RustTestParser::parse`, return the diagnostics drafts early when
     there are any (§5's old wording).
     Only `default_parsers_turn_a_failed_cargo_test_into_compiler_and_test_findings`
     must fail.
  2. Change the dispatcher back to `drafts.is_empty()`.
     `a_failure_whose_parser_found_only_warnings_also_gets_the_generic_finding`
     and `a_crashed_test_binary_with_a_warning_still_gets_the_generic_finding`
     must fail.
  3. Remove the three new `is_cargo_summary` clauses.
     `cargo_test_target_lines_on_stderr_are_not_findings`, the crash test and
     the `default_parsers_turn_a_failed_cargo_test…` test must fail. In the
     crash test, the bogus `Compiler` error suppresses the generic finding.
  4. End a section at the first blank line as well.
     Eight tests must fail, including `a_panic_takes_its_location_and_message`.
     The captured `println!` line is followed by a blank line, so the section
     loses its `panicked at` line.
  5. Remove `sections.clear()`.
     Only `a_name_repeated_in_a_later_binary_does_not_borrow_an_earlier_section`
     must fail.
  6. Put the panic message before the `#[should_panic]` note in
     `failure_message`.
     Only `a_wrong_panic_message_is_summarised_by_the_note_not_the_panic`
     must fail.
  7. Drop the `doctest_location` fallback.
     Only `a_doctest_points_at_its_doc_block_not_the_temporary_file` must
     fail.
  8. Build the panic range without `workspace_range`, so the absolute
     temporary path is kept.
     Only `a_doctest_points_at_its_doc_block_not_the_temporary_file` must
     fail.
  9. Drop the optional `(?: \(\d+\))?` thread-id group from `PANICKED`.
     The same eight tests as mutation 4 must fail.
  10. Make the thread-id group required.
      Only `a_panic_line_without_a_thread_id_still_has_its_location` must
      fail.

  Each result above was observed in the scratch crate. Record what actually
  happens in the progress row. A different set of failures is a signal: work
  out why before moving on.

- [x] **Step 7: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  cargo nextest run -p workspace-engine -E 'test(finding::)'
  typos docs/specs/22_findings_model_and_panel crates/workspace-engine/src/finding.rs crates/workspace-engine/src/finding
  ```

  If `typos` flags a word inside a fixture, it is cargo's or libtest's text,
  not ours. Add the fixture directory to `_typos.toml`'s excludes with a
  comment saying why, rather than editing captured output.

- [x] **Step 8: Update this file's Task 4 row, then show the change and the
  check results and ask before committing**

## Task 5: Biome parser

**Requirements:** 1 (lint source), 2, and the acceptance criteria "a … lint
error … normalise[s] into `Finding` with correct source, severity, and …
file and range" and "no parser produces a `range` for output that contained
no location". **Files:** create
`crates/workspace-engine/src/finding/biome.rs` and three fixture files under
`crates/workspace-engine/src/finding/fixtures/`. Modify `finding.rs` only to
declare the module and register the parser. Planned in full on 2026-10-01.

**Read `context.md` §11 first.** It records what real Biome 2.5.7 output looks
like, and each rule below comes from it. Three of those rules differ from how
the Rust parsers (§9, §10) work:

- a block ends at the next `━━━` line, not at a blank line, because Biome
  blocks contain whitespace-only lines;
- severity comes from a marker character, not a header word;
- a hidden-diagnostics count is read from **stdout**.

**Interfaces:**
- Consumes: `FindingParser`, `FindingDraft`, `FindingSource`, `Severity`,
  `default_parsers` and `findings_from_execution` (`finding.rs`). Also
  `regex` and `workspace_range(path, line, column: Option<&str>)`, which are
  `pub(super)` in `finding/rust_diagnostics.rs` (Tasks 3–4), and
  `CommandExecution`.
- Produces:
  - `pub(super) struct BiomeParser`, registered **third** in
    `default_parsers()`, after `RustTestParser`. No command matches both
    the Rust parsers and this one, so the order does not matter for
    correctness. Appending simply leaves the existing order untouched.
  - `pub(super) fn parse_biome(stderr: &str, stdout: &str) -> Vec<FindingDraft>`.
  No later task depends on this module's other items.

- [x] **Step 1: Commit the fixtures as files**

  Write these three files under `crates/workspace-engine/src/finding/fixtures/`
  **byte for byte**. Each file is the text between its fences, verbatim,
  followed by one newline. The stdout capture starts with an empty line, and
  both stderr captures end with one. Those empty lines are Biome's and npm's,
  and they are part of the fixtures. Several lines in `biome_check_stderr.txt` and
  `biome_info_stderr.txt` consist of exactly two spaces. Those are Biome's,
  and the parser's block rule depends on them (`context.md` §11), so do not
  let an editor strip trailing whitespace. After writing, check with
  `grep -c '^  $' crates/workspace-engine/src/finding/fixtures/biome_check_stderr.txt`.
  It must print **28**.

  `biome_check_stderr.txt` is `npm run lint:web` on the seeded scratch tree,
  stderr. It exits 1.

  ````text
crates/desktop-shell/static/app.js:2:9 lint/correctness/noUnusedVariables  FIXABLE  ━━━━━━━━━━━━━━━━

  ! This variable unused is unused.
  
    1 │ function load(value) {
  > 2 │   const unused = 1;
      │         ^^^^^^
    3 │   if (value == null) {
    4 │     return 0;
  
  i Unused variables are often the result of typos, incomplete refactors, or other sources of bugs.
  
  i Unsafe fix: If this is intentional, prepend unused with an underscore.
  
     1  1 │   function load(value) {
     2    │ - ··const·unused·=·1;
        2 │ + ··const·_unused·=·1;
     3  3 │     if (value == null) {
     4  4 │       return 0;
  

scripts/parse.mjs:1:7 lint/correctness/noUnusedVariables  FIXABLE  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  ! This variable broken is unused.
  
  > 1 │ const broken = (;
      │       ^^^^^^
    2 │ 
  
  i Unused variables are often the result of typos, incomplete refactors, or other sources of bugs.
  
  i Unsafe fix: If this is intentional, prepend broken with an underscore.
  
    1   │ - const·broken·=·(;
      1 │ + const·_broken·=·(;
    2 2 │   
  

crates/desktop-shell/static/app.js:6:3 lint/suspicious/noDebugger  FIXABLE  ━━━━━━━━━━━━━━━━━━━━━━━━

  × This is an unexpected use of the debugger statement.
  
    4 │     return 0;
    5 │   }
  > 6 │   debugger;
      │   ^^^^^^^^^
    7 │   return value;
    8 │ }
  
  i Unsafe fix: Remove debugger statement
  
     4 4 │       return 0;
     5 5 │     }
     6   │ - ··debugger;
     7 6 │     return value;
     8 7 │   }
  

crates/desktop-shell/static/styles.css:1:18 lint/suspicious/noDuplicateProperties ━━━━━━━━━━━━━━━━━━

  × Duplicate properties can lead to unexpected behavior and may override previous declarations unintentionally.
  
  > 1 │ .a { color: red; color: blue; }
      │                  ^^^^^
    2 │ 
  
  i color is already defined here.
  
  > 1 │ .a { color: red; color: blue; }
      │      ^^^^^
    2 │ 
  
  i Remove or rename the duplicate property to ensure consistent styling.
  

crates/desktop-shell/static/styles.css format ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  × Formatter would have printed the following content:
  
    1   │ - .a·{·color:·red;·color:·blue;·}
      1 │ + .a·{
      2 │ + ··color:·red;
      3 │ + ··color:·blue;
      4 │ + }
    2 5 │   
  

scripts/format.mjs format ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  × Formatter would have printed the following content:
  
    1   │ - export·const·x·=·{a:1,
    2   │ - ·b:2}
      1 │ + export·const·x·=·{·a:·1,·b:·2·};
    3 2 │   
  

scripts/parse.mjs:1:17 parse ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  × expected `)` but instead found `;`
  
  > 1 │ const broken = (;
      │                 ^
    2 │ 
  
  i Remove ;
  

scripts/parse.mjs format ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  × Code formatting aborted due to parsing errors. To format code with errors, enable the 'formatter.formatWithErrors' option.
  

check ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  × Some errors were emitted while running checks.
  

  ````

  `biome_check_stdout.txt` is the same run's stdout, including `npm run`'s
  banner.

  ````text

> fix@0.0.0 lint:web
> biome check

Checked 5 files in 59ms. No fixes applied.
Found 6 errors.
Found 2 warnings.
  ````

  `biome_info_stderr.txt` is `npm run lint:web` on this repository at the
  time of planning, stderr. It exits 0, with one `info`.

  ````text
scripts/check-spec-status.mjs:136:7 lint/style/useTemplate  FIXABLE  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  i Template literals are preferred over string concatenation.
  
    134 │ console.error(
    135 │   problems.length === 1
  > 136 │     ? "Spec status check " + label + ": 1 dependency line disagrees with the spec it names.\n"
        │       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    137 │     : `Spec status check ${label}: ${problems.length} dependency lines disagree with the specs they name.\n`,
    138 │ );
  
  i Unsafe fix: Use a template literal.
  
    134 134 │   console.error(
    135 135 │     problems.length === 1
    136     │ - ····?·"Spec·status·check·"·+·label·+·":·1·dependency·line·disagrees·with·the·spec·it·names.\n"
        136 │ + ····?·`Spec·status·check·${label}:·1·dependency·line·disagrees·with·the·spec·it·names.\n`
    137 137 │       : `Spec status check ${label}: ${problems.length} dependency lines disagree with the specs they name.\n`,
    138 138 │   );
  

  ````

- [x] **Step 2: Declare the module and write the failing tests**

  In `finding.rs`, add `mod biome;` above `mod rust_diagnostics;`. Then create
  `crates/workspace-engine/src/finding/biome.rs` containing only the test
  module:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::command_policy::CommandRisk;
      use crate::command_runner::{CommandExecution, CommandTermination};
      use crate::finding::{SourceRange, default_parsers, findings_from_execution};
      use crate::secret_scanner::SecretScanner;

      const CHECK_STDERR: &str = include_str!("fixtures/biome_check_stderr.txt");
      const CHECK_STDOUT: &str = include_str!("fixtures/biome_check_stdout.txt");
      const INFO_STDERR: &str = include_str!("fixtures/biome_info_stderr.txt");

      fn range(path: &str, line: u32, column: u32) -> Option<SourceRange> {
          Some(SourceRange {
              path: path.to_string(),
              start_line: line,
              start_column: Some(column),
              end_line: None,
              end_column: None,
          })
      }

      fn execution(command: &str, exit_code: i32, stdout: &str, stderr: &str) -> CommandExecution {
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
              stdout: stdout.to_string(),
              stderr: stderr.to_string(),
          }
      }

      fn check() -> Vec<FindingDraft> {
          parse_biome(CHECK_STDERR, CHECK_STDOUT)
      }

      #[test]
      fn a_lint_error_keeps_its_rule_location_and_message() {
          let drafts = check();
          let debugger = &drafts[2];
          assert_eq!(debugger.source, FindingSource::Lint);
          assert_eq!(debugger.severity, Severity::Error);
          assert_eq!(debugger.code.as_deref(), Some("lint/suspicious/noDebugger"));
          assert_eq!(debugger.summary, "This is an unexpected use of the debugger statement.");
          assert_eq!(debugger.range, range("crates/desktop-shell/static/app.js", 6, 3));
      }

      #[test]
      fn a_warning_marker_is_a_warning() {
          let unused = &check()[0];
          assert_eq!(unused.severity, Severity::Warning);
          assert_eq!(unused.code.as_deref(), Some("lint/correctness/noUnusedVariables"));
          assert_eq!(unused.summary, "This variable unused is unused.");
          assert_eq!(unused.range, range("crates/desktop-shell/static/app.js", 2, 9));
      }

      /// This repository's own output at planning time (`context.md` §11).
      #[test]
      fn an_info_marker_is_info() {
          let drafts = parse_biome(INFO_STDERR, "");
          assert_eq!(drafts.len(), 1, "{drafts:?}");
          assert_eq!(drafts[0].severity, Severity::Info);
          assert_eq!(drafts[0].code.as_deref(), Some("lint/style/useTemplate"));
          assert_eq!(drafts[0].range, range("scripts/check-spec-status.mjs", 136, 7));
      }

      /// Biome's own stdout says "Found 6 errors. Found 2 warnings." The
      /// marker mapping must agree with it (`context.md` §11).
      #[test]
      fn one_draft_per_diagnostic_agreeing_with_biomes_own_count() {
          let drafts = check();
          let errors = drafts.iter().filter(|d| d.severity == Severity::Error).count();
          let warnings = drafts.iter().filter(|d| d.severity == Severity::Warning).count();
          assert_eq!((drafts.len(), errors, warnings), (8, 6, 2), "{drafts:#?}");
          assert!(CHECK_STDOUT.contains("Found 6 errors.") && CHECK_STDOUT.contains("Found 2 warnings."));
      }

      #[test]
      fn a_format_diagnostic_has_no_range() {
          let drafts = check();
          let styles = &drafts[4];
          assert_eq!(styles.code.as_deref(), Some("format"));
          assert_eq!(styles.severity, Severity::Error);
          assert_eq!(styles.summary, "Formatter would have printed the following content:");
          assert_eq!(styles.range, None, "a location was invented for a whole-file diff");
      }

      #[test]
      fn a_parse_error_keeps_its_location() {
          let parse = &check()[6];
          assert_eq!(parse.code.as_deref(), Some("parse"));
          assert_eq!(parse.summary, "expected `)` but instead found `;`");
          assert_eq!(parse.range, range("scripts/parse.mjs", 1, 17));
      }

      #[test]
      fn the_closing_check_block_is_not_a_finding() {
          assert!(
              check().iter().all(|d| !d.summary.contains("Some errors were emitted")),
              "the run summary became a finding"
          );
      }

      /// Biome blocks contain whitespace-only lines, so a block must run to
      /// the next `━━━` header (`context.md` §11).
      #[test]
      fn details_run_past_whitespace_only_lines_to_the_next_header() {
          let drafts = check();
          let details = drafts[0].details.as_deref().expect("details");
          assert!(details.starts_with("crates/desktop-shell/static/app.js:2:9 "), "{details}");
          assert!(details.contains("const·_unused·=·1;"), "the fix was cut off: {details}");
          assert!(!details.contains("scripts/parse.mjs"), "the next block leaked in: {details}");
          assert!(!details.ends_with(' ') && !details.ends_with('\n'), "{details:?}");
      }

      #[test]
      fn hidden_diagnostics_become_one_info_draft() {
          let stdout = "The number of diagnostics exceeds the limit allowed. Use --max-diagnostics to increase it.\nDiagnostics not shown: 9.\nChecked 27 files in 4ms. No fixes applied.\nFound 28 errors.\n";
          let drafts = parse_biome(INFO_STDERR, stdout);
          assert_eq!(drafts.len(), 2, "{drafts:?}");
          let hidden = &drafts[1];
          assert_eq!(hidden.severity, Severity::Info);
          assert!(hidden.summary.contains('9'), "{}", hidden.summary);
          assert_eq!(hidden.range, None);
      }

      /// Captured with `--colors=force` (`context.md` §11): CSI colours, an
      /// OSC 8 hyperlink around the category, and `⚠` for the warning marker.
      #[test]
      fn forced_colour_output_parses_the_same() {
          let stderr = "\x1b[0mcrates/desktop-shell/static/app.js\x1b[0m\x1b[0m:\x1b[0m\x1b[0m2\x1b[0m\x1b[0m:\x1b[0m\x1b[0m9\x1b[0m\x1b[0m \x1b[0m\x1b[0m\x1b]8;;https://biomejs.dev/linter/rules/no-unused-variables\x1b\\lint/correctness/noUnusedVariables\x1b]8;;\x1b\\\x1b[0m\x1b[0m \x1b[0m\x1b[0m\x1b[30m\x1b[47m FIXABLE \x1b[0m\x1b[0m \x1b[0m\x1b[0m━━━━━━━━━━━━━━━━\x1b[0m\x1b[0m\n\n\x1b[0m\x1b[0m  \x1b[0m\x1b[0m\x1b[1m\x1b[33m⚠\x1b[0m\x1b[0m \x1b[0m\x1b[0m\x1b[33mThis variable \x1b[0m\x1b[0m\x1b[1m\x1b[33munused\x1b[0m\x1b[0m\x1b[33m is unused.\x1b[0m\x1b[0m\n";
          let drafts = parse_biome(stderr, "");
          assert_eq!(drafts.len(), 1, "{drafts:?}");
          assert_eq!(drafts[0].severity, Severity::Warning);
          assert_eq!(drafts[0].summary, "This variable unused is unused.");
          assert_eq!(drafts[0].code.as_deref(), Some("lint/correctness/noUnusedVariables"));
          assert_eq!(drafts[0].range, range("crates/desktop-shell/static/app.js", 2, 9));
      }

      #[test]
      fn colour_markers_map_like_plain_ones() {
          for (marker, severity) in [("✖", Severity::Error), ("⚠", Severity::Warning), ("ℹ", Severity::Info)] {
              let stderr = format!("a.js:1:1 lint/x/y ━━━━━━\n\n  {marker} message\n");
              assert_eq!(parse_biome(&stderr, "")[0].severity, severity, "{marker}");
          }
      }

      #[test]
      fn an_absolute_path_has_no_range() {
          let stderr = "/tmp/elsewhere/app.js:1:1 lint/suspicious/noDebugger ━━━━━━\n\n  × debugger\n";
          let drafts = parse_biome(stderr, "");
          assert_eq!(drafts.len(), 1);
          assert_eq!(drafts[0].range, None);
      }

      #[test]
      fn a_block_without_a_marker_is_kept_as_an_error_named_by_its_category() {
          let stderr = "a.js:3:1 lint/x/y ━━━━━━\n\n    3 │ code\n";
          let drafts = parse_biome(stderr, "");
          assert_eq!(drafts.len(), 1, "a printed diagnostic was dropped");
          assert_eq!(drafts[0].severity, Severity::Error);
          assert_eq!(drafts[0].summary, "lint/x/y");
      }

      #[test]
      fn matches_biome_invocations_and_npm_lint_scripts() {
          let parser = BiomeParser;
          for command in [
              "biome check",
              "npx biome check --write",
              "./node_modules/.bin/biome lint scripts",
              "biome ci",
              "biome format .",
              "npm run lint:web",
              "npm run lint",
              "pnpm run lint:js",
              "yarn run lint",
          ] {
              assert!(parser.matches(command), "{command}");
          }
      }

      #[test]
      fn does_not_match_other_commands() {
          let parser = BiomeParser;
          for command in [
              "npm test",
              "npm run build",
              "npm run lint-staged",
              "biome --version",
              "cargo clippy",
              "eslint .",
          ] {
              assert!(!parser.matches(command), "{command}");
          }
      }

      /// Registration, end to end through the dispatcher.
      #[test]
      fn default_parsers_turn_a_failed_lint_run_into_parsed_findings() {
          let findings = findings_from_execution(
              &execution("npm run lint:web", 1, CHECK_STDOUT, CHECK_STDERR),
              &default_parsers(),
              &SecretScanner::default(),
          );
          assert_eq!(findings.len(), 8, "{findings:?}");
          assert!(findings.iter().all(|f| f.source() == FindingSource::Lint));
      }

      /// `context.md` §11: `npm run lint` may not be Biome at all. Output this
      /// parser cannot read must fall through, exactly as with no parser.
      #[test]
      fn a_failed_lint_script_that_is_not_biome_falls_through_to_generic() {
          let findings = findings_from_execution(
              &execution("npm run lint", 1, "", "sh: eslint: command not found\n"),
              &default_parsers(),
              &SecretScanner::default(),
          );
          assert_eq!(findings.len(), 1, "{findings:?}");
          assert_eq!(findings[0].source(), FindingSource::Command);
      }

      #[test]
      fn a_passing_run_keeps_its_info() {
          let findings = findings_from_execution(
              &execution("npm run lint:web", 0, "", INFO_STDERR),
              &default_parsers(),
              &SecretScanner::default(),
          );
          assert_eq!(findings.len(), 1, "{findings:?}");
          assert_eq!(findings[0].severity(), Severity::Info);
      }
  }
  ```

- [x] **Step 3: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::biome)'`
  Expected: a compile failure, because `parse_biome` and `BiomeParser` are not
  defined.

- [x] **Step 4: Write the implementation**

  Put this above the test module in `biome.rs`:

  ```rust
  //! Biome diagnostics: `biome check` / `lint` / `ci` / `format`, and
  //! `npm run lint*` (spec 22 Task 5).
  //!
  //! Severity is Biome's marker on a diagnostic's first message line. `×`
  //! (`✖` under forced colour) is `Error`, `!` (`⚠`) is `Warning`, and `i`
  //! (`ℹ`) is `Info`. That mapping agrees with Biome's own "Found N errors"
  //! count (proposal §5.2). Every Biome finding is `Lint`, `format` and `parse`
  //! included. The code is the full category, such as
  //! `lint/suspicious/noDebugger`. Every rule here comes from real output
  //! recorded in `docs/specs/22_findings_model_and_panel/context.md` §11.

  use super::rust_diagnostics::{regex, workspace_range};
  use super::{FindingDraft, FindingParser, FindingSource, Severity};
  use crate::command_runner::CommandExecution;
  use regex::Regex;
  use std::sync::LazyLock;

  /// Any line ending in a `━━━` rule. A Biome block runs from its header to
  /// the next of these, because blocks contain whitespace-only lines.
  static ANY_HEADER: LazyLock<Regex> = LazyLock::new(|| regex(r"^\S.*━{3,}\s*$"));
  /// `path[:line:col] category [FIXABLE] ━━━`. The path is lazy so it cannot
  /// swallow the `:line:col`.
  static DIAGNOSTIC: LazyLock<Regex> = LazyLock::new(|| {
      regex(r"^(\S+?)(?::(\d+):(\d+))? ([A-Za-z][A-Za-z0-9/_-]*)(?:\s+FIXABLE)?\s+━{3,}\s*$")
  });
  static MARKER: LazyLock<Regex> = LazyLock::new(|| regex(r"^  ([×✖!⚠iℹ]) (.+)$"));
  static HIDDEN: LazyLock<Regex> = LazyLock::new(|| regex(r"Diagnostics not shown: (\d+)\."));
  /// CSI colour codes and OSC 8 hyperlinks, both present under `--colors=force`.
  static ESCAPES: LazyLock<Regex> =
      LazyLock::new(|| regex(r"\x1b\[[0-9;]*m|\x1b\]8;;[^\x1b]*\x1b\\"));

  pub(super) struct BiomeParser;

  impl FindingParser for BiomeParser {
      fn source(&self) -> FindingSource {
          FindingSource::Lint
      }

      /// A `biome` subcommand, or an `npm`/`pnpm`/`yarn run lint[:…]` script.
      /// A lint script may not be Biome. If it is not, `parse` finds nothing
      /// and the run falls through to its generic finding (`context.md` §11).
      fn matches(&self, command: &str) -> bool {
          let tokens: Vec<&str> = command.split_whitespace().collect();
          let biome = tokens.windows(2).any(|pair| {
              (pair[0] == "biome" || pair[0].ends_with("/biome"))
                  && matches!(pair[1], "check" | "lint" | "ci" | "format")
          });
          let lint_script = tokens.windows(3).any(|words| {
              matches!(words[0], "npm" | "pnpm" | "yarn")
                  && words[1] == "run"
                  && (words[2] == "lint" || words[2].starts_with("lint:"))
          });
          biome || lint_script
      }

      fn parse(&self, execution: &CommandExecution) -> Vec<FindingDraft> {
          parse_biome(&execution.stderr, &execution.stdout)
      }
  }

  /// Diagnostics from stderr, plus one `Info` draft when stdout says Biome
  /// hid some above its display cap (`context.md` §11).
  pub(super) fn parse_biome(stderr: &str, stdout: &str) -> Vec<FindingDraft> {
      let clean = ESCAPES.replace_all(stderr, "");
      let lines: Vec<&str> = clean.lines().collect();
      let mut drafts = Vec::new();
      for (index, line) in lines.iter().enumerate() {
          let Some(header) = DIAGNOSTIC.captures(line) else {
              continue;
          };
          let end = lines[index + 1..]
              .iter()
              .position(|line| ANY_HEADER.is_match(line))
              .map_or(lines.len(), |offset| index + 1 + offset);
          let block = &lines[index..end];
          let kept = block
              .iter()
              .rposition(|line| !line.trim().is_empty())
              .map_or(1, |last| last + 1);
          let block = &block[..kept];

          let category = &header[4];
          let (severity, summary) = block[1..]
              .iter()
              .find_map(|line| MARKER.captures(line))
              .map(|marker| (severity_of(&marker[1]), marker[2].trim().to_string()))
              .unwrap_or_else(|| (Severity::Error, category.to_string()));
          let range = header.get(2).and_then(|line| {
              workspace_range(&header[1], line.as_str(), header.get(3).map(|c| c.as_str()))
          });
          drafts.push(FindingDraft {
              source: FindingSource::Lint,
              severity,
              summary,
              details: Some(block.join("\n")),
              range,
              code: Some(category.to_string()),
          });
      }

      let stdout = ESCAPES.replace_all(stdout, "");
      if let Some(hidden) = HIDDEN.captures(&stdout) {
          drafts.push(FindingDraft {
              source: FindingSource::Lint,
              severity: Severity::Info,
              summary: format!(
                  "Biome did not show {} more diagnostics; rerun with --max-diagnostics to see them",
                  &hidden[1]
              ),
              details: None,
              range: None,
              code: None,
          });
      }
      drafts
  }

  fn severity_of(marker: &str) -> Severity {
      match marker {
          "×" | "✖" => Severity::Error,
          "!" | "⚠" => Severity::Warning,
          _ => Severity::Info,
      }
  }
  ```

  In `finding.rs`, register the parser last:

  ```rust
  pub fn default_parsers() -> Vec<Box<dyn FindingParser>> {
      vec![
          Box::new(rust_diagnostics::RustDiagnosticsParser),
          Box::new(rust_test::RustTestParser),
          Box::new(biome::BiomeParser),
      ]
  }
  ```

  If a test and this sketch disagree, the test wins, because the tests come
  from the captured fixtures. Record any deviation in the progress row.

- [x] **Step 5: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::)'`
  Expected: 89 pass. That is 71 from Tasks 1–4, all unchanged, plus 18 in
  `biome::tests`. Count them.

- [x] **Step 6: Mutation-test the rules that came from real output**

  Apply each change on its own, confirm the named test fails, then revert it:

  1. End a block at the first whitespace-only line, as the Rust parsers do.
     `details_run_past_whitespace_only_lines_to_the_next_header` must fail.
  2. Make the path greedy (`(\S+)` instead of `(\S+?)`).
     `a_lint_error_keeps_its_rule_location_and_message` must fail. The path
     swallows `:6:3`, and the range becomes `None`.
  3. Drop the OSC 8 alternative from `ESCAPES`.
     `forced_colour_output_parses_the_same` must fail, because the header
     no longer matches.
  4. Map only `×`, `!` and `i` in `severity_of`.
     `colour_markers_map_like_plain_ones` must fail on `✖` or `⚠`.
  5. Map every non-error marker to `Warning`.
     `an_info_marker_is_info` must fail.
  6. Remove the `HIDDEN` stdout check.
     `hidden_diagnostics_become_one_info_draft` must fail.
  7. Match any `npm run` script.
     `does_not_match_other_commands` must fail on `npm run build`.
  8. Drop the `unwrap_or_else` fallback and skip a block with no marker.
     `a_block_without_a_marker_is_kept_…` must fail.

  Record all eight results in the progress row.

- [x] **Step 7: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  cargo nextest run -p workspace-engine -E 'test(finding::)'
  typos docs/specs/22_findings_model_and_panel crates/workspace-engine/src/finding.rs crates/workspace-engine/src/finding
  ```

  If `typos` flags a word inside a fixture, it is Biome's text, not ours.
  Follow Task 3's rule: exclude the fixture directory in `_typos.toml`, with a
  comment, rather than editing captured output.

- [x] **Step 8: Update this file's Task 5 row, then show the change and the
  check results and ask before committing**

## Task 6: Browser findings from spec 12's `WebDiagnosticRecord`

**Requirements:** 1 (browser console and network sources), 2, 4, and these
acceptance criteria:

- "a browser console error … normalise[s] into `Finding`";
- "a browser diagnostic with structured entries produces one finding per
  entry; one with `is_error: true` and no entries produces a single generic
  finding";
- "a browser source location outside the repository is dropped";
- the seeded secret in "browser output".

**Files:** create `crates/workspace-engine/src/finding/browser.rs`. Modify
`finding.rs` to declare the module, re-export the function and add
`FindingSource::BrowserScenario`. Modify `web_diagnostics.rs` for
**visibility only**: four private functions become `pub(crate)`. Planned in
full on 2026-10-02.

**Read `context.md` §7.1 and §12 first.** §12 settles five things the outline
left open:

- failed scenario steps are findings, under a new `BrowserScenario` source;
- the source/severity/text table;
- how a served URL maps to a file: loopback only, exactly one match;
- the tool-failure rule;
- where the code lives.

This task does **not** add `WebDiagnosticEntry`, `WebEntryKind` or
`WebDiagnosticReport.entries` (§7.1). It does not record findings anywhere;
that is Task 8.

**Interfaces:**
- Consumes:
  - `Finding::new`, `FindingDraft`, `FindingSource`, `Severity` and
    `SourceRange`, plus the private `first_non_empty_line` (`finding.rs`).
  - `WebDiagnosticRecord`, `WebDiagnosticReport::from_text` /
    `tool_failed`, and `WebConsoleEntry::is_problem` (`web_diagnostics.rs`).
- Produces:
  - `FindingSource::BrowserScenario`, serialised as `"browser_scenario"`.
  - `pub fn findings_from_web_record(record: &WebDiagnosticRecord, repository_files: &[&str], scanner: &SecretScanner) -> Vec<Finding>`,
    re-exported as `crate::finding::findings_from_web_record`.
    `repository_files` holds repository-relative paths. Task 8 passes
    `RepositoryIndex.files`' paths, and attaches `record.task_id` and
    `record.id` (`context.md` §12.5).
  - The `Command` variant's doc comment is broadened, per §12.4. The variant
    itself is not renamed.
  - `pub(crate)` on `WebConsoleEntry::model_item`,
    `WebFailedRequest::model_item`, `WebScenarioStep::model_item` and
    `is_loopback_url`. Bodies are unchanged.

- [x] **Step 1: Add the source variant, broaden `Command`'s doc, widen
  visibility**

  In `finding.rs`, add after `Command`:

  ```rust
      /// A failed scenario step, such as a click that timed out
      /// (`context.md` §12.1).
      BrowserScenario,
  ```

  Add the `as_str` arm `Self::BrowserScenario => "browser_scenario",`. Add
  `BrowserScenario` to the array in `tests::every_source_serialises_as_its_as_str`.
  Replace `Command`'s doc comment with:

  ```rust
      /// A check's failure taken whole, nothing parsed out of it: a command
      /// (`context.md` §8.1) or a browser diagnostic whose runner failed
      /// (§12.4). Only the generic fallbacks produce this.
  ```

  In `web_diagnostics.rs`, change `fn model_item` to `pub(crate) fn model_item`
  in the three `impl` blocks (`WebConsoleEntry`, `WebFailedRequest`,
  `WebScenarioStep`), and `fn is_loopback_url` to `pub(crate) fn is_loopback_url`.
  Change nothing else in that file. Run
  `cargo nextest run -p workspace-engine -E 'test(finding::) + test(web_diagnostics::)'`.
  Every test still passes.

- [x] **Step 2: Declare the module and write the failing tests**

  In `finding.rs`, add `mod browser;` above `mod biome;`, and
  `pub use browser::findings_from_web_record;` below the `mod` lines. Create
  `crates/workspace-engine/src/finding/browser.rs` containing only the test
  module:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::web_diagnostics::WebDiagnosticReport;

      /// spec 12's `COMPANION_REPORT` (`web_diagnostics.rs` tests), which is
      /// trimmed from a real companion `inspect_page` result. One console
      /// warning from another origin is added (`context.md` §12).
      const INSPECT_REPORT: &str = r#"{
        "success": false, "diagnostic_ok": false, "run_id": "20260925-1",
        "url": "http://localhost:5001/", "final_url": "http://localhost:5001/",
        "title": "Snake Game", "status": 200,
        "page_errors": ["ReferenceError: Cannot access 'game' before initialization"],
        "console": [
          {"type": "error", "text": "Failed to load resource: 404",
           "location": {"url": "http://localhost:5001/js/app.js", "lineNumber": 41, "columnNumber": 7}},
          {"type": "log", "text": "booting"},
          {"type": "warning", "text": "Deprecated API used",
           "location": {"url": "https://cdn.example.com/js/app.js", "lineNumber": 0, "columnNumber": 0}}
        ],
        "failed_requests": [
          {"kind": "response", "url": "http://localhost:5001/api/me", "method": "GET",
           "resource_type": "fetch", "status": 404, "status_text": "Not Found"},
          {"kind": "requestfailed", "url": "http://localhost:5001/ws", "method": "GET",
           "resource_type": "websocket", "failure": "net::ERR_CONNECTION_REFUSED"}
        ],
        "dom_summary": {"forms": 1, "buttons": ["Log in", "Register"], "fields": [],
          "status_text": "", "visible_text_excerpt": "Snake Log in Register Score: 0"},
        "artifacts": [],
        "text_report": "ignored by Damaian"
      }"#;

      /// The repository's file list (`context.md` §12.3). The `node_modules`
      /// copy would make `js/app.js` ambiguous if it were not excluded.
      const FILES: &[&str] = &[
          "static/js/app.js",
          "static/index.html",
          "server.py",
          "node_modules/lib/js/app.js",
      ];

      fn record(tool: &str, text: &str, is_error: bool) -> WebDiagnosticRecord {
          WebDiagnosticRecord {
              id: "webdiagrec_1".to_string(),
              task_id: "task_1".to_string(),
              tool: tool.to_string(),
              url: "http://localhost:5001/".to_string(),
              recorded_at_ms: 1,
              report: WebDiagnosticReport::from_text(text, is_error),
          }
      }

      fn findings_in(text: &str, is_error: bool, files: &[&str]) -> Vec<Finding> {
          findings_from_web_record(
              &record("inspect_web_page", text, is_error),
              files,
              &SecretScanner::default(),
          )
      }

      fn findings(text: &str, is_error: bool) -> Vec<Finding> {
          findings_in(text, is_error, FILES)
      }

      fn console_error_at(url: &str, line: Option<u32>) -> String {
          let line = line.map_or(String::new(), |line| format!(r#", "lineNumber": {}"#, line - 1));
          format!(
              r#"{{"final_url": "http://localhost:3000/", "console": [
                {{"type": "error", "text": "boom", "location": {{"url": "{url}"{line}}}}}]}}"#
          )
      }

      #[test]
      fn an_inspection_yields_one_finding_per_problem_in_report_order() {
          let found = findings(INSPECT_REPORT, false);
          let shape: Vec<_> = found.iter().map(|f| (f.source(), f.severity())).collect();
          assert_eq!(
              shape,
              [
                  (FindingSource::BrowserConsole, Severity::Error),
                  (FindingSource::BrowserConsole, Severity::Error),
                  (FindingSource::BrowserConsole, Severity::Warning),
                  (FindingSource::BrowserNetwork, Severity::Error),
                  (FindingSource::BrowserNetwork, Severity::Error),
              ],
              "{found:#?}"
          );
          assert!(found.iter().all(|f| f.task_id().is_none() && f.origin_ref().is_none()));
      }

      #[test]
      fn a_page_error_is_a_console_error_without_a_location() {
          let page_error = &findings(INSPECT_REPORT, false)[0];
          assert_eq!(
              page_error.summary(),
              "ReferenceError: Cannot access 'game' before initialization"
          );
          assert_eq!(page_error.range(), None);
      }

      /// The served `/js/app.js` is `static/js/app.js`. The `node_modules`
      /// copy is excluded, so there is exactly one match. Spec 12 stored the
      /// line and column 1-based (41/7 → 42/8).
      #[test]
      fn a_console_error_at_a_served_url_maps_to_the_one_repository_file() {
          let console_error = &findings(INSPECT_REPORT, false)[1];
          assert_eq!(console_error.summary(), "Failed to load resource: 404");
          assert_eq!(
              console_error.range(),
              Some(&SourceRange {
                  path: "static/js/app.js".to_string(),
                  start_line: 42,
                  start_column: Some(8),
                  end_line: None,
                  end_column: None,
              })
          );
          assert_eq!(
              console_error.details(),
              Some("console error: Failed to load resource: 404 (http://localhost:5001/js/app.js:42:8)")
          );
      }

      #[test]
      fn a_console_location_on_another_origin_has_no_range() {
          let warning = &findings(INSPECT_REPORT, false)[2];
          assert_eq!(warning.summary(), "Deprecated API used");
          assert_eq!(warning.range(), None, "a CDN script was mapped to a repository file");
      }

      #[test]
      fn a_log_line_is_not_a_finding() {
          assert!(findings(INSPECT_REPORT, false).iter().all(|f| f.summary() != "booting"));
      }

      #[test]
      fn failed_requests_carry_their_status_or_network_error_as_code() {
          let found = findings(INSPECT_REPORT, false);
          assert_eq!(
              found[3].summary(),
              "failed request: GET http://localhost:5001/api/me → 404 Not Found"
          );
          assert_eq!(found[3].code(), Some("404"));
          assert_eq!(found[4].code(), Some("net::ERR_CONNECTION_REFUSED"));
          assert_eq!(found[3].range(), None);
      }

      /// `context.md` §12.1: the model is told about failed steps, so the
      /// panel must show them.
      #[test]
      fn a_failed_scenario_step_is_a_browser_scenario_error() {
          let scenario = r##"{"final_url": "http://localhost:5001/", "results": [
            {"step": 1, "action": "goto", "success": true},
            {"step": 2, "action": "click", "selector": "#start", "success": false,
             "error": "Timeout 5000ms exceeded"}]}"##;
          let found = findings_from_web_record(
              &record("run_web_scenario", scenario, false),
              FILES,
              &SecretScanner::default(),
          );
          assert_eq!(found.len(), 1, "{found:?}");
          assert_eq!(found[0].source(), FindingSource::BrowserScenario);
          assert_eq!(found[0].severity(), Severity::Error);
          assert_eq!(found[0].summary(), "step 2 click failed: Timeout 5000ms exceeded");
      }

      /// spec 12's own companion-error shape (`web_diagnostics.rs` tests).
      #[test]
      fn a_runner_that_failed_yields_one_generic_finding() {
          let failed = r#"{"error": true, "success": false, "message": "Timeout 30000ms exceeded",
              "final_url": "http://localhost:5001/"}"#;
          let found = findings(failed, false);
          assert_eq!(found.len(), 1, "{found:?}");
          assert_eq!(found[0].source(), FindingSource::Command);
          assert_eq!(found[0].severity(), Severity::Error);
          assert_eq!(
              found[0].summary(),
              "inspect_web_page http://localhost:5001/ failed: Timeout 30000ms exceeded"
          );
          assert_eq!(found[0].range(), None);
      }

      #[test]
      fn a_non_companion_runner_failure_yields_one_generic_finding_from_its_text() {
          let found = findings("\nBrowser could not start\nstack…\n", true);
          assert_eq!(found.len(), 1, "{found:?}");
          assert_eq!(found[0].source(), FindingSource::Command);
          assert_eq!(
              found[0].summary(),
              "inspect_web_page http://localhost:5001/ failed: Browser could not start"
          );
      }

      /// §12.4: prose from another runner is not parsed.
      #[test]
      fn a_non_companion_runner_that_succeeded_yields_nothing() {
          assert!(findings("The page loaded and looked fine.", false).is_empty());
      }

      #[test]
      fn a_clean_inspection_yields_nothing() {
          let clean = r#"{"final_url": "http://localhost:5001/", "page_errors": [],
              "console": [{"type": "log", "text": "ready"}], "failed_requests": []}"#;
          assert!(findings(clean, false).is_empty());
      }

      /// §10.3's rule: the generic finding is added only when nothing else
      /// explains the failure.
      #[test]
      fn a_tool_failure_is_not_doubled_when_the_report_already_explains_it() {
          let found = findings(INSPECT_REPORT, true);
          assert_eq!(found.len(), 5, "{found:?}");
          assert!(found.iter().all(|f| f.source() != FindingSource::Command));
      }

      #[test]
      fn an_ambiguous_served_path_has_no_range() {
          let found = findings_in(
              &console_error_at("http://localhost:3000/app.js", Some(3)),
              false,
              &["dist/app.js", "src/app.js"],
          );
          assert_eq!(found.len(), 1);
          assert_eq!(found[0].range(), None, "one of two candidates was guessed");
      }

      #[test]
      fn an_exact_path_match_counts() {
          let found = findings_in(
              &console_error_at("http://127.0.0.1:3000/app.js", Some(3)),
              false,
              &["app.js", "README.md"],
          );
          assert_eq!(found[0].range().map(|r| (r.path.as_str(), r.start_line)), Some(("app.js", 3)));
      }

      #[test]
      fn a_bundled_url_with_no_repository_file_has_no_range() {
          let found = findings(&console_error_at("http://localhost:5001/assets/index-3f9a.js", Some(1)), false);
          assert_eq!(found[0].range(), None);
      }

      #[test]
      fn a_location_without_a_line_has_no_range() {
          let found = findings(&console_error_at("http://localhost:5001/js/app.js", None), false);
          assert_eq!(found.len(), 1);
          assert_eq!(found[0].range(), None);
      }

      #[test]
      fn served_path_accepts_only_loopback_http_file_paths() {
          assert_eq!(served_path("http://localhost:5001/js/app.js?v=3#top"), Some("js/app.js"));
          assert_eq!(served_path("http://127.0.0.1/app.js"), Some("app.js"));
          assert_eq!(served_path("http://[::1]:8080/a/b.js"), Some("a/b.js"));
          for url in [
              "https://cdn.example.com/js/app.js",
              "file:///repo/static/js/app.js",
              "webpack:///./src/app.js",
              "http://localhost:5001/",
              "http://localhost:5001",
              "http://localhost:5001/static/",
              "http://localhost:5001/../etc/passwd",
          ] {
              assert_eq!(served_path(url), None, "{url}");
          }
      }

      /// Acceptance criterion: no unredacted secret from "browser output".
      /// The record is built **without** `redacted()`, so only `Finding::new`
      /// stands between the key and the finding.
      #[test]
      fn a_secret_in_browser_output_is_redacted() {
          let key = "AKIAIOSFODNN7EXAMPLE";
          let report = format!(
              r#"{{"final_url": "http://localhost:5001/", "page_errors": ["bad key {key}"],
                  "console": [{{"type": "error", "text": "token {key}"}}],
                  "failed_requests": [{{"url": "http://localhost:5001/api?key={key}", "method": "GET", "status": 401}}]}}"#
          );
          let found = findings(&report, false);
          assert_eq!(found.len(), 3, "{found:?}");
          for finding in &found {
              assert!(!finding.summary().contains(key), "{}", finding.summary());
              assert!(!finding.details().unwrap_or_default().contains(key));
          }
      }
  }
  ```

- [x] **Step 3: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::browser)'`
  Expected: a compile failure, because `findings_from_web_record` and
  `served_path` are not defined.

- [x] **Step 4: Write the implementation**

  Put this above the test module in `browser.rs`:

  ```rust
  //! Browser findings from a recorded diagnostic (spec 22 Task 6).
  //!
  //! Not a `FindingParser`: a browser diagnostic is spec 12's typed report,
  //! not command output to match. Sources and severities are the browser's
  //! own (proposal §5.2; the table is `context.md` §12.2):
  //! - a page error is `BrowserConsole`/`Error`;
  //! - a console `error` or `assert` is `BrowserConsole`/`Error`, and a
  //!   `warning` is `BrowserConsole`/`Warning`;
  //! - a failed request is `BrowserNetwork`/`Error`;
  //! - a failed scenario step is `BrowserScenario`/`Error`.
  //!
  //! A runner that failed, with nothing else to explain it, is one `Command`
  //! finding (§12.4).

  use super::{Finding, FindingDraft, FindingSource, Severity, SourceRange, first_non_empty_line};
  use crate::secret_scanner::SecretScanner;
  use crate::web_diagnostics::{WebDiagnosticRecord, WebSourceLocation, is_loopback_url};
  use std::path::{Component, Path};

  /// Findings for one diagnostic run. `repository_files` holds
  /// repository-relative paths, against which a served URL is matched
  /// (§12.3). Task 8 attaches `task_id` and `origin_ref`.
  pub fn findings_from_web_record(
      record: &WebDiagnosticRecord,
      repository_files: &[&str],
      scanner: &SecretScanner,
  ) -> Vec<Finding> {
      let report = &record.report;
      let mut drafts = Vec::new();
      if let Some(details) = &report.details {
          for error in &details.page_errors {
              drafts.push(draft(
                  FindingSource::BrowserConsole,
                  Severity::Error,
                  error.clone(),
                  Some(error.clone()),
              ));
          }
          for entry in details.console.iter().filter(|entry| entry.is_problem()) {
              let level = entry.level.to_ascii_lowercase();
              let severity = if matches!(level.as_str(), "error" | "assert") {
                  Severity::Error
              } else {
                  Severity::Warning
              };
              let mut console = draft(
                  FindingSource::BrowserConsole,
                  severity,
                  entry.text.clone(),
                  Some(entry.model_item()),
              );
              console.range = entry
                  .location
                  .as_ref()
                  .and_then(|location| repository_range(location, repository_files));
              drafts.push(console);
          }
          for request in &details.failed_requests {
              let mut network = draft(
                  FindingSource::BrowserNetwork,
                  Severity::Error,
                  request.model_item(),
                  None,
              );
              network.code = request.status.map(|status| status.to_string()).or_else(|| {
                  request
                      .failure
                      .clone()
                      .filter(|failure| failure.starts_with("net::"))
              });
              drafts.push(network);
          }
          for step in details.steps.iter().filter(|step| !step.success) {
              drafts.push(draft(
                  FindingSource::BrowserScenario,
                  Severity::Error,
                  step.model_item(),
                  None,
              ));
          }
      }

      let explained = drafts.iter().any(|draft| draft.severity == Severity::Error);
      if report.tool_failed() && !explained {
          let message = report
              .details
              .as_ref()
              .and_then(|details| details.tool_error.as_deref())
              .or_else(|| first_non_empty_line(&report.text))
              .unwrap_or("no message");
          drafts.push(draft(
              FindingSource::Command,
              Severity::Error,
              format!("{} {} failed: {message}", record.tool, record.url),
              Some(report.text.clone()),
          ));
      }

      drafts
          .into_iter()
          .map(|draft| Finding::new(draft, scanner))
          .collect()
  }

  fn draft(
      source: FindingSource,
      severity: Severity,
      summary: String,
      details: Option<String>,
  ) -> FindingDraft {
      FindingDraft {
          source,
          severity,
          summary,
          details,
          range: None,
          code: None,
      }
  }

  /// A range only when the served path matches exactly one repository file
  /// outside `node_modules`. Two candidates are not a choice to make (§12.3).
  fn repository_range(location: &WebSourceLocation, files: &[&str]) -> Option<SourceRange> {
      let line = location.line?;
      let path = served_path(&location.url)?;
      let suffix = format!("/{path}");
      let mut candidates = files.iter().copied().filter(|file| {
          (*file == path || file.ends_with(&suffix))
              && !Path::new(file)
                  .components()
                  .any(|part| part.as_os_str() == "node_modules")
      });
      let file = candidates.next()?;
      if candidates.next().is_some() {
          return None;
      }
      Some(SourceRange {
          path: file.to_string(),
          start_line: line,
          start_column: location.column,
          end_line: None,
          end_column: None,
      })
  }

  /// The path of a loopback `http(s)` URL, without its leading `/`, query or
  /// fragment. `None` for any other URL, a directory, or a `..` path.
  fn served_path(url: &str) -> Option<&str> {
      if !is_loopback_url(url) {
          return None;
      }
      let (_, rest) = url.trim().split_once("://")?;
      let (_, path) = rest.split_once('/')?;
      let path = path.split(['?', '#']).next().unwrap_or_default();
      let unusable = path.is_empty()
          || path.ends_with('/')
          || Path::new(path)
              .components()
              .any(|part| part == Component::ParentDir);
      (!unusable).then_some(path)
  }
  ```

  If a test and this sketch disagree, the test wins, because the tests come
  from spec 12's real-derived fixture and `context.md` §12. Record any
  deviation in the progress row.

- [x] **Step 5: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine -E 'test(finding::) + test(web_diagnostics::)'`
  Expected: every `finding::` test passes. That is 89 from Tasks 1–5, all
  unchanged except `every_source_serialises_as_its_as_str`, which now
  includes `BrowserScenario`, plus 18 in `browser::tests`. Every spec 12
  `web_diagnostics::` test is unchanged. Count both.

- [x] **Step 6: Mutation-test the decisions in §12**

  Apply each change on its own, confirm the named test fails, then revert it:

  1. Remove the `node_modules` exclusion.
     `a_console_error_at_a_served_url_maps_to_…` must fail: two candidates,
     so no range.
  2. Take the first candidate instead of refusing a second.
     `an_ambiguous_served_path_has_no_range` must fail.
  3. Drop the `is_loopback_url` check from `served_path`.
     `a_console_location_on_another_origin_has_no_range` and
     `served_path_accepts_only_loopback_…` must fail.
  4. Remove the `!explained` condition.
     `a_tool_failure_is_not_doubled_…` must fail.
  5. Skip failed steps.
     `a_failed_scenario_step_is_a_browser_scenario_error` must fail.
  6. Treat every problem console level as `Error`.
     `an_inspection_yields_one_finding_per_problem_…` must fail on the
     warning.
  There is no redaction mutation for this task. Fields are private, so the
  only way to get a `Finding` is `Finding::new`, which always redacts, and
  Task 1 mutation-tested that. `a_secret_in_browser_output_is_redacted`
  proves that the browser path goes through it, starting from an unredacted
  record.

  Record results 1–6 in the progress row.

- [x] **Step 7: Scoped checks**

  Run `cargo fmt --all` first. The sketch is not rustfmt-shaped, and when
  this plan was checked, rustfmt rewrapped lines in `browser.rs` and the
  `finding.rs` test array. Those are the only changes it made.

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  cargo nextest run -p workspace-engine -E 'test(finding::) + test(web_diagnostics::)'
  typos docs/specs/22_findings_model_and_panel crates/workspace-engine/src/finding.rs crates/workspace-engine/src/finding crates/workspace-engine/src/web_diagnostics.rs
  ```

- [x] **Step 8: Update this file's Task 6 row, then show the change and the
  check results and ask before committing**

## Task 7: Persistence, derived status, `Evidence::Findings`

**Requirements:** 3 (dismissal's storage), and the acceptance criteria
"findings survive a restart with their statuses intact" and "a finding
becomes `Stale` when its file's hash no longer matches the hash recorded at
creation". **Files:** modify `crates/workspace-engine/src/session.rs` (three
methods, two private helpers, and tests in its existing `mod tests`),
`crates/workspace-engine/src/plan.rs` (one variant, two match arms), and
`crates/workspace-engine/tests/plan.rs` (tests). It also updates two code
comments, in `finding.rs` and `finding/browser.rs`, whose task numbers the
2026-10-02 split shifted. Planned in full on 2026-10-02.

**Read `context.md` §1, §13.2 and §13.3 first.**
- §13.1 explains why this task no longer records anything: recording is the
  new Task 8.
- §13.2 fixes the log format. `Stale` is never stored, only `Open` is checked
  for staleness, an unknown id is refused, and reads use `active_events`.
- §13.3 says `Evidence::Findings` links a step to findings but never decides
  its status.

This task does **not** touch `chat.rs`. Nothing calls `record_finding` outside
the tests until Task 8. The methods are `pub` on a `pub` type, so
`clippy -D warnings` raises no dead-code warning.

**Interfaces:**
- Consumes: `Finding`, `FindingDraft`, `FindingSource`, `FindingStatus`,
  `Severity`, `SourceRange`, `Finding::with_file_hash` and `set_status`
  (`finding.rs`). Also `hash::file_hash`. From `session.rs`:
  `append_session_event`, `session_log_path`, `active_events`, and
  `SessionEvent { event_type, payload: serde_json::Value }`.
- Produces, and Tasks 8–11 rely on these names:
  - `SessionStore::record_finding(&self, session_id: &str, finding: &Finding) -> Result<()>`
  - `SessionStore::set_finding_status(&self, session_id: &str, finding_id: &str, status: FindingStatus) -> Result<()>`.
    `Err(InvalidInput)` for `Stale`, or for an id the session never recorded.
  - `SessionStore::read_findings(&self, session_id: &str, repository_root: &Path) -> Result<Vec<Finding>>`,
    in record order, with the newest status applied and `Stale` derived.
  - The events `finding_recorded` (payload: the `Finding`) and
    `finding_status_changed` (payload: `{"findingId", "status"}`).
  - `Evidence::Findings { refs: Vec<String>, failing: usize }`, serialised
    as `{"kind":"findings","refs":[…],"failing":N}`.

- [x] **Step 1: Update the two shifted comments**

  In `finding.rs`, the test doc comment
  `/// The persisted and served shape (Tasks 7 and 9 depend on it).` becomes
  `(Tasks 7 and 10 depend on it)`. In `finding/browser.rs`, the doc line
  `/// (§12.3). Task 7 attaches `task_id` and `origin_ref`.` becomes
  `Task 8 attaches`. Change nothing else in either file.

- [x] **Step 2: Write the failing session tests**

  Add these to the **end** of the existing `#[cfg(test)] mod tests` in
  `session.rs`, which already has `temp_data_dir`:

  ```rust
      use crate::finding::{
          Finding, FindingDraft, FindingSource, FindingStatus, Severity, SourceRange,
      };

      fn repo_with(name: &str, file: &str, content: &str) -> PathBuf {
          let root = temp_data_dir(&format!("findings-repo-{name}"));
          fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
          fs::write(root.join(file), content).unwrap();
          root
      }

      fn finding(summary: &str, range: Option<&str>) -> Finding {
          Finding::new(
              FindingDraft {
                  source: FindingSource::Compiler,
                  severity: Severity::Error,
                  summary: summary.to_string(),
                  details: None,
                  range: range.map(|path| SourceRange {
                      path: path.to_string(),
                      start_line: 1,
                      start_column: None,
                      end_line: None,
                      end_column: None,
                  }),
                  code: None,
              },
              &SecretScanner::default(),
          )
      }

      /// A finding on `file` with the file's current hash, as Task 8 records it.
      fn hashed_finding(root: &Path, file: &str) -> Finding {
          finding("mismatched types", Some(file))
              .with_file_hash(crate::hash::file_hash(root.join(file)).unwrap())
      }

      fn store_and_session(name: &str) -> (SessionStore, Session) {
          let store = SessionStore::new(temp_data_dir(&format!("findings-{name}")));
          let session = store.create_session("repo_1", "Findings").unwrap();
          (store, session)
      }

      #[test]
      fn a_session_without_findings_reads_empty() {
          let (store, session) = store_and_session("empty");
          assert!(store.read_findings(&session.id, Path::new("/nonexistent")).unwrap().is_empty());
      }

      #[test]
      fn a_recorded_finding_reads_back_unchanged() {
          let (store, session) = store_and_session("round-trip");
          let recorded = finding("it broke", None).with_task_id("task_1").with_origin_ref("cmd_1");
          store.record_finding(&session.id, &recorded).unwrap();
          let read = store.read_findings(&session.id, Path::new("/nonexistent")).unwrap();
          assert_eq!(read, vec![recorded]);
      }

      #[test]
      fn findings_read_back_in_record_order() {
          let (store, session) = store_and_session("order");
          for summary in ["first", "second", "third"] {
              store.record_finding(&session.id, &finding(summary, None)).unwrap();
          }
          let read = store.read_findings(&session.id, Path::new("/nonexistent")).unwrap();
          let summaries: Vec<_> = read.iter().map(Finding::summary).collect();
          assert_eq!(summaries, ["first", "second", "third"]);
      }

      #[test]
      fn the_newest_status_change_wins() {
          let (store, session) = store_and_session("newest");
          let recorded = finding("x", None);
          store.record_finding(&session.id, &recorded).unwrap();
          let status = |store: &SessionStore| {
              store.read_findings(&session.id, Path::new("/nonexistent")).unwrap()[0].status()
          };
          store.set_finding_status(&session.id, recorded.id(), FindingStatus::Dismissed).unwrap();
          assert_eq!(status(&store), FindingStatus::Dismissed);
          store.set_finding_status(&session.id, recorded.id(), FindingStatus::Fixed).unwrap();
          assert_eq!(status(&store), FindingStatus::Fixed);
          store.set_finding_status(&session.id, recorded.id(), FindingStatus::Open).unwrap();
          assert_eq!(status(&store), FindingStatus::Open);
      }

      /// Acceptance criterion: findings survive a restart with their statuses.
      #[test]
      fn findings_and_their_statuses_survive_a_new_store_over_the_same_data_dir() {
          let data_dir = temp_data_dir("findings-restart");
          let first = SessionStore::new(&data_dir);
          let session = first.create_session("repo_1", "Before restart").unwrap();
          let kept = finding("kept", None);
          let dismissed = finding("dismissed", None);
          first.record_finding(&session.id, &kept).unwrap();
          first.record_finding(&session.id, &dismissed).unwrap();
          first.set_finding_status(&session.id, dismissed.id(), FindingStatus::Dismissed).unwrap();
          drop(first);

          let second = SessionStore::new(&data_dir);
          let read = second.read_findings(&session.id, Path::new("/nonexistent")).unwrap();
          let statuses: Vec<_> = read.iter().map(|f| (f.summary(), f.status())).collect();
          assert_eq!(
              statuses,
              [("kept", FindingStatus::Open), ("dismissed", FindingStatus::Dismissed)]
          );
      }

      #[test]
      fn an_unchanged_file_keeps_its_finding_open() {
          let root = repo_with("unchanged", "src/lib.rs", "fn a() {}\n");
          let (store, session) = store_and_session("unchanged");
          store.record_finding(&session.id, &hashed_finding(&root, "src/lib.rs")).unwrap();
          assert_eq!(store.read_findings(&session.id, &root).unwrap()[0].status(), FindingStatus::Open);
      }

      /// Acceptance criterion, and `context.md` §1.
      #[test]
      fn a_changed_file_makes_its_finding_stale() {
          let root = repo_with("changed", "src/lib.rs", "fn a() {}\n");
          let (store, session) = store_and_session("changed");
          store.record_finding(&session.id, &hashed_finding(&root, "src/lib.rs")).unwrap();
          fs::write(root.join("src/lib.rs"), "fn a() { 1 }\n").unwrap();
          assert_eq!(store.read_findings(&session.id, &root).unwrap()[0].status(), FindingStatus::Stale);
      }

      #[test]
      fn a_deleted_file_makes_its_finding_stale() {
          let root = repo_with("deleted", "src/lib.rs", "fn a() {}\n");
          let (store, session) = store_and_session("deleted");
          store.record_finding(&session.id, &hashed_finding(&root, "src/lib.rs")).unwrap();
          fs::remove_file(root.join("src/lib.rs")).unwrap();
          assert_eq!(store.read_findings(&session.id, &root).unwrap()[0].status(), FindingStatus::Stale);
      }

      /// §13.2: derived, not stored. Reverting the file brings the finding back.
      #[test]
      fn staleness_is_derived_on_read_so_a_reverted_file_reopens_its_finding() {
          let root = repo_with("reverted", "src/lib.rs", "fn a() {}\n");
          let (store, session) = store_and_session("reverted");
          store.record_finding(&session.id, &hashed_finding(&root, "src/lib.rs")).unwrap();
          fs::write(root.join("src/lib.rs"), "changed\n").unwrap();
          assert_eq!(store.read_findings(&session.id, &root).unwrap()[0].status(), FindingStatus::Stale);
          fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
          assert_eq!(store.read_findings(&session.id, &root).unwrap()[0].status(), FindingStatus::Open);
      }

      /// `context.md` §1: no recorded hash means staleness cannot be judged.
      #[test]
      fn a_finding_without_a_recorded_hash_is_never_stale() {
          let root = repo_with("no-hash", "src/lib.rs", "fn a() {}\n");
          let (store, session) = store_and_session("no-hash");
          store.record_finding(&session.id, &finding("x", Some("src/lib.rs"))).unwrap();
          fs::write(root.join("src/lib.rs"), "changed\n").unwrap();
          assert_eq!(store.read_findings(&session.id, &root).unwrap()[0].status(), FindingStatus::Open);
      }

      /// §13.2: the user's decision is not overwritten by a hash.
      #[test]
      fn a_dismissed_finding_is_not_re_marked_stale() {
          let root = repo_with("dismissed", "src/lib.rs", "fn a() {}\n");
          let (store, session) = store_and_session("dismissed");
          let recorded = hashed_finding(&root, "src/lib.rs");
          store.record_finding(&session.id, &recorded).unwrap();
          store.set_finding_status(&session.id, recorded.id(), FindingStatus::Dismissed).unwrap();
          fs::write(root.join("src/lib.rs"), "changed\n").unwrap();
          assert_eq!(
              store.read_findings(&session.id, &root).unwrap()[0].status(),
              FindingStatus::Dismissed
          );
      }

      #[test]
      fn setting_stale_directly_is_refused_and_appends_nothing() {
          let (store, session) = store_and_session("set-stale");
          let recorded = finding("x", None);
          store.record_finding(&session.id, &recorded).unwrap();
          let before = store.latest_event_seq(&session.id).unwrap();
          assert!(store.set_finding_status(&session.id, recorded.id(), FindingStatus::Stale).is_err());
          assert_eq!(store.latest_event_seq(&session.id).unwrap(), before);
      }

      #[test]
      fn a_status_change_for_an_unknown_finding_is_refused_and_appends_nothing() {
          let (store, session) = store_and_session("unknown");
          let before = store.latest_event_seq(&session.id).unwrap();
          assert!(store
              .set_finding_status(&session.id, "finding_never_recorded", FindingStatus::Dismissed)
              .is_err());
          assert_eq!(store.latest_event_seq(&session.id).unwrap(), before);
      }

      /// §13.2: findings follow `active_events`, as plans and diagnostics do.
      #[test]
      fn a_rewind_takes_the_findings_recorded_after_its_point() {
          let (store, session) = store_and_session("rewind");
          store.record_finding(&session.id, &finding("before", None)).unwrap();
          let point = store.latest_event_seq(&session.id).unwrap();
          store.record_finding(&session.id, &finding("after", None)).unwrap();
          store.rewind_conversation(&session.id, point).unwrap();
          let read = store.read_findings(&session.id, Path::new("/nonexistent")).unwrap();
          let summaries: Vec<_> = read.iter().map(Finding::summary).collect();
          assert_eq!(summaries, ["before"]);
      }

      /// The §5.7 log shape, readable by hand.
      #[test]
      fn the_log_carries_both_event_kinds_in_their_documented_shape() {
          let (store, session) = store_and_session("shape");
          let recorded = finding("x", None);
          store.record_finding(&session.id, &recorded).unwrap();
          store.set_finding_status(&session.id, recorded.id(), FindingStatus::Dismissed).unwrap();
          let log = fs::read_to_string(store.session_log_path(&session.id)).unwrap();
          let lines: Vec<serde_json::Value> = log
              .lines()
              .map(|line| serde_json::from_str(line).unwrap())
              .collect();
          let recorded_event = lines
              .iter()
              .find(|event| event["eventType"] == "finding_recorded")
              .expect("finding_recorded");
          assert_eq!(recorded_event["payload"]["id"], recorded.id());
          assert_eq!(recorded_event["payload"]["source"], "compiler");
          let changed = lines
              .iter()
              .find(|event| event["eventType"] == "finding_status_changed")
              .expect("finding_status_changed");
          assert_eq!(changed["payload"]["findingId"], recorded.id());
          assert_eq!(changed["payload"]["status"], "dismissed");
      }
  ```

  If `fs`, `Path` or `Session` are not already in scope in that test module
  through `use super::*;`, import them. `session.rs` imports `std::fs` and
  `std::path::{Path, PathBuf}` at the top, and defines `Session`.

- [x] **Step 3: Write the failing plan tests**

  Append to `crates/workspace-engine/tests/plan.rs`, after
  `a_non_zero_exit_blocks_the_step`:

  ```rust
  fn findings(failing: usize) -> Evidence {
      Evidence::Findings {
          refs: vec!["finding_1".to_string(), "finding_2".to_string()],
          failing,
      }
  }

  /// spec 22 `context.md` §13.3: a step that set out to find problems is not
  /// blocked for finding them. The exit code decides.
  #[test]
  fn findings_evidence_alone_does_not_block_a_step() {
      assert_eq!(status_from_evidence(&[findings(2)]), StepStatus::Completed);
  }

  #[test]
  fn a_failing_exit_still_blocks_beside_its_findings() {
      assert_eq!(
          status_from_evidence(&[command_exit(Some(1)), findings(2)]),
          StepStatus::Blocked
      );
  }

  #[test]
  fn findings_evidence_serialises_with_its_kind_and_round_trips() {
      let evidence = findings(1);
      let value = serde_json::to_value(&evidence).unwrap();
      assert_eq!(value["kind"], "findings");
      assert_eq!(value["refs"][1], "finding_2");
      assert_eq!(value["failing"], 1);
      assert_eq!(serde_json::from_value::<Evidence>(value).unwrap(), evidence);
  }

  #[test]
  fn findings_as_the_newest_evidence_mean_validating() {
      let mut plan = TaskPlan::new("task_1", 0);
      let mut open = step("step_1", StepStatus::InProgress);
      open.evidence = vec![findings(0)];
      plan.steps.push(open);
      assert_eq!(plan.phase(false), TaskPhase::Validating);
  }
  ```

  `serde_json` is already a dev-dependency of the integration tests. If it is
  not, the compiler says so; use the crate's `serde_json` dependency as the
  other integration tests do.

- [x] **Step 4: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine -E 'test(session::tests) + binary(plan)'`
  Expected: compile failures. `record_finding`, `set_finding_status` and
  `read_findings` are not defined, and `Evidence::Findings` does not exist.

- [x] **Step 5: Write the implementation**

  In `plan.rs`, add the variant at the end of `Evidence`:

  ```rust
      /// The findings a check produced (spec 22): ids into the session log,
      /// and how many were `Error` at the time. Recorded beside the
      /// `CommandExit` it explains. It never decides the step's status, because
      /// the exit code does, and a step that set out to find problems must not be
      /// blocked for finding them (spec 22 `context.md` §13.3).
      Findings { refs: Vec<String>, failing: usize },
  ```

  In `status_from_evidence`, make the second arm
  `Evidence::PatchApplied { .. } | Evidence::FileRead { .. } | Evidence::Findings { .. } => false,`.
  In `TaskPlan::phase`, make the first arm
  `Some(Evidence::CommandExit { .. } | Evidence::Findings { .. }) => TaskPhase::Validating,`.
  The enum already has `#[serde(tag = "kind", rename_all = "camelCase")]`, so
  the variant serialises as `"findings"`. Its fields are single words, so no
  per-variant `rename_all` is needed.

  In `session.rs`, add after `read_session_web_diagnostics`:

  ```rust
      /// Appends one finding (spec 22 §5.7). A `Finding` is redacted by
      /// construction, so this store writes it as given.
      pub fn record_finding(&self, session_id: &str, finding: &crate::finding::Finding) -> Result<()> {
          let payload = serde_json::to_string(finding).map_err(|error| {
              crate::error::ClientError::Io(format!("finding serialization: {error}"))
          })?;
          self.append_session_event(session_id, "finding_recorded", &payload)
      }

      /// Records the user's decision about a finding. `Stale` is derived on
      /// read and never stored, and an id this session never recorded is
      /// refused before anything is appended (spec 22 `context.md` §13.2).
      pub fn set_finding_status(
          &self,
          session_id: &str,
          finding_id: &str,
          status: crate::finding::FindingStatus,
      ) -> Result<()> {
          if status == crate::finding::FindingStatus::Stale {
              return Err(crate::error::ClientError::InvalidInput(
                  "a finding becomes stale when its file changes; it cannot be set stale"
                      .to_string(),
              ));
          }
          let content = fs::read_to_string(self.session_log_path(session_id)).unwrap_or_default();
          if !replay_findings(&content).iter().any(|finding| finding.id() == finding_id) {
              return Err(crate::error::ClientError::InvalidInput(format!(
                  "no finding {finding_id} in session {session_id}"
              )));
          }
          let payload = serde_json::json!({ "findingId": finding_id, "status": status }).to_string();
          self.append_session_event(session_id, "finding_status_changed", &payload)
      }

      /// Every finding in the session, in record order, with the newest status
      /// applied and `Stale` derived against `repository_root` (spec 22
      /// `context.md` §1, §13.2). Only an `Open` finding is checked.
      pub fn read_findings(
          &self,
          session_id: &str,
          repository_root: &Path,
      ) -> Result<Vec<crate::finding::Finding>> {
          let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
              return Ok(Vec::new());
          };
          let mut findings = replay_findings(&content);
          for finding in &mut findings {
              if finding.status() == crate::finding::FindingStatus::Open
                  && finding_is_stale(finding, repository_root)
              {
                  finding.set_status(crate::finding::FindingStatus::Stale);
              }
          }
          Ok(findings)
      }
  ```

  Then add these free functions next to `active_events`:

  ```rust
  /// Findings as recorded, with their newest status change applied. Staleness
  /// is not derived here, so `set_finding_status` can use this to check that
  /// an id exists.
  fn replay_findings(content: &str) -> Vec<crate::finding::Finding> {
      let mut findings: Vec<crate::finding::Finding> = Vec::new();
      for event in active_events(content) {
          match event.event_type.as_str() {
              "finding_recorded" => {
                  if let Ok(finding) =
                      serde_json::from_value::<crate::finding::Finding>(event.payload)
                      && !findings.iter().any(|known| known.id() == finding.id())
                  {
                      findings.push(finding);
                  }
              }
              "finding_status_changed" => {
                  let id = event.payload.get("findingId").and_then(serde_json::Value::as_str);
                  let status = event.payload.get("status").cloned().and_then(|status| {
                      serde_json::from_value::<crate::finding::FindingStatus>(status).ok()
                  });
                  if let (Some(id), Some(status)) = (id, status)
                      && let Some(finding) = findings.iter_mut().find(|finding| finding.id() == id)
                  {
                      finding.set_status(status);
                  }
              }
              _ => {}
          }
      }
      findings
  }

  /// Stale only when a hash was recorded and the file now differs or is
  /// gone. No range or no hash means it cannot be judged (`context.md` §1).
  fn finding_is_stale(finding: &crate::finding::Finding, repository_root: &Path) -> bool {
      let (Some(range), Some(recorded)) = (finding.range(), finding.file_hash()) else {
          return false;
      };
      match crate::hash::file_hash(repository_root.join(&range.path)) {
          Ok(current) => current != recorded,
          Err(_) => true,
      }
  }
  ```

  If the `let`-chains do not compile as written, nest them as plain `if let`s.
  The crate is edition 2024, and `chat.rs` already uses `&& let Some(…)`. Do
  not weaken a test to fit.

- [x] **Step 6: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine -E 'test(session::tests) + binary(plan) + test(finding::)'`
  Expected: the 15 new session tests and 4 new plan tests pass. All
  existing `session::tests`, `tests/plan.rs` and `finding::` tests are
  unchanged. Count the new ones.

- [x] **Step 7: Mutation-test the §13 decisions**

  Apply each change on its own, confirm the named test fails, then revert it:

  1. Check every status for staleness, not only `Open`.
     `a_dismissed_finding_is_not_re_marked_stale` must fail.
  2. Treat a missing hash as stale.
     `a_finding_without_a_recorded_hash_is_never_stale` must fail.
  3. Treat a missing file as not stale (`Err(_) => false`).
     `a_deleted_file_makes_its_finding_stale` must fail.
  4. Remove the `Stale` refusal.
     `setting_stale_directly_is_refused_and_appends_nothing` must fail.
  5. Remove the unknown-id check.
     `a_status_change_for_an_unknown_finding_is_refused_…` must fail.
  6. Use `parsed_events(content).0` instead of `active_events(content)`.
     `a_rewind_takes_the_findings_recorded_after_its_point` must fail.
  7. Make `Evidence::Findings { failing, .. } => *failing > 0` in
     `status_from_evidence`.
     `findings_evidence_alone_does_not_block_a_step` must fail.

  Record all seven results in the progress row.

- [x] **Step 8: Scoped checks**

  Run `cargo fmt --all` first. Then:

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  cargo nextest run -p workspace-engine -E 'test(session::tests) + binary(plan) + test(finding::)'
  typos docs/specs/22_findings_model_and_panel crates/workspace-engine/src/session.rs crates/workspace-engine/src/plan.rs crates/workspace-engine/tests/plan.rs
  ```

  `Evidence` is `#[non_exhaustive]` and the shell matches it only through JSON
  (`describeEvidence` in `app.js`, which falls back to "recorded"), so no other
  crate needs a change. If clippy reports a non-exhaustive match somewhere in
  `workspace-engine`, add the arm the §13.3 rule implies and record it.

- [x] **Step 9: Update this file's Task 7 row, then show the change and the
  check results and ask before committing**

## Task 8: Record findings where checks run

**Requirements:** 1, 2, 4, and the "full output remains reachable through
`origin_ref`" criterion. **Files:** `chat.rs`, `finding.rs` (one builder),
and `app.js` (`describeEvidence` only). Facts from `context.md` §13.4:

- **Commands.** At `chat.rs:975` and `chat.rs:2260`, after `run_proposal`,
  call `findings_from_execution(&record.execution, &default_parsers(), &self.scanner)`.
  For each finding:
  - keep the range only if `repository_root.join(path)` is a file. Otherwise
    drop it, with a new `Finding` builder that removes the range and adds no
    free text;
  - take `file_hash` at the same moment;
  - attach `task.id` and `origin_ref = record.execution.id`;
  - call `record_finding`.
- **Evidence.** In the loop, push `Evidence::Findings { refs, failing }` next
  to the command's `CommandExit` evidence (`chat.rs:~2717`), where `failing`
  is the number of `Error` findings.
- **Browser.** In `run_and_record_web_diagnostic`, after
  `append_web_diagnostic`, call `findings_from_web_record`. Build the file list
  with `tree_walk::walk` only when the report has a console entry with a
  location. Attach `record.task_id` and `record.id`, and record them.
- **The standalone branch.** `/api/run-command` records nothing, because it
  has no session (`context.md` §13.4). Record that as a known gap.
- **Shell wording.** `describeEvidence` gains a `findings` wording.

Recording is best-effort only where the rest of that path is. A failure to
append a finding fails the call, like `append_web_diagnostic` does. Read the
"What to build next" → Parallel work section before starting, because this
task holds `chat.rs`.

*(Expand into full TDD steps before starting this task.)*

## Task 9: Dismissal and the scoped repair request

**Files:** `finding.rs`, `session.rs`.

`RepairRequest { findings: Vec<RepairItem>, excluded_stale: Vec<String> }`,
built from selected ids. Stale findings are excluded with a note. The request
carries the findings' summary, range, and code, never raw output (§5.5). It
renders to the model-facing prompt text. Test: dismissing a finding does not
suppress a new finding for the same problem from a later check.

## Task 10: Shell API

**Files:** `crates/desktop-shell/src/lib.rs`.

`GET /api/findings?sessionId=`, `POST /api/finding-status` (dismiss), and
`POST /api/findings-repair`, which returns the rendered request that the
panel sends as a chat message. Follow spec 20 Task 8's `serve_for_test`
pattern for the HTTP tests.

## Task 11: Findings panel

**Files:** `app.js`, `styles.css`, read against `docs/UI_STYLE_GUIDE.md`.

Group by source, then by file. Filter by severity and status. The default
view is Open plus Error. A finding with a range renders through spec 05's
existing clickable-reference mechanism, with no new navigation path. Each
finding has a dismiss control, and there is a "Fix selected" action. Stale
findings are shown but not selectable, with the reason stated. Verify in the
browser against the ignored `serves_the_ui_for_manual_inspection` harness on
port 4899, never on 4765.

## Task 12: Docs, acceptance criteria, close the spec

`USER_GUIDE.md` and `TROUBLESHOOTING.md` per §5.8. Walk every acceptance
criterion into `proposal.md` §7, including the seeded-secret criterion across
command output, browser output, and the generic fallback. Record the share of
real failures that fell through to the generic parser. Then work through the
AGENTS.md "When a spec becomes Done" checklist: `Depends on:` lines in #23,
#24, #26, #32, and #35; re-deriving "What to build next"; the CHANGELOG
`Unreleased` entry; and `npm run specs:check`. Finish with the full
seven-command gate.
