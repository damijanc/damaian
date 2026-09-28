# First-Class Web App Troubleshooting — Close-out Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** what [`proposal.md`](proposal.md) still lacks, as listed in
[`context.md`](context.md) §2: §5.1's structured report and model text, §5.3's
diagnostic card, and §8's live scenario check.
**Started:** 2026-09-25

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Typed report: `WebDiagnosticDetails` parsed from the companion's JSON, artifact metadata, `redacted`, `tool_failed` | Done 2026-09-25 | Landed as planned in `web_diagnostics.rs`: `details`/`via` on the report, the six detail types (re-exported from `lib.rs`), one `serde_json` parse in `from_text`, `tool_failed`, `problem_count`, and `redacted` written as one explicit method per struct (artifacts included). `extract_artifacts_from_text` became `artifacts_from_value` + `artifact_record`: `artifact_metadata` first, then any `artifacts` path not already listed. Tests: the 7 new `web_diagnostics::tests` plus the 4 existing browser tests pass (11/11); all three Step 8 mutations fail the named test and were reverted; `cargo fmt --check` and `clippy -p workspace-engine -D warnings` clean. **Deviation — the secret fixture:** `ghp_…` does match the default generic-token rule, but the scanner skips a token preceded by a token byte, and `/` is one (`is_embedded_token_byte`). So a secret in a URL *path* (`…:5001/<secret>.js`) is never redacted, and the planned fixture could not pass. The test puts URL secrets in query values (`?t=<secret>`) instead, and also covers `url`, `method`, `resource_type`, an artifact path and `via`. **For Task 2:** that scanner gap means a credential in a URL path survives `redacted()`; it is a scanner property, not this task's, and is not fixed here. Every existing fixture is plain text, so `details` is `None` on all current engine paths until the runner stops prefixing (Task 2). A `dom_summary` with none of the known keys (e.g. `{"error": …}`) is `None`. |
| 2 · Model text rendered from the typed report; retry counting on `tool_failed` | Done 2026-09-25 | **`render_for_model`** (`web_diagnostics.rs`) is built from small helpers: `found_header`, `push_section`, `pluralize`/`noun_for`, `truncate_chars`, `WebDiagnosticDetails::problem_sections`/`push_context`, and a `model_item()` on each of console entry, failed request and step. It follows Step 3's rules exactly. Choices the plan left open: the URL line falls back to `url` when `final_url` is missing; a failed request with neither status nor failure reads `→ failed`; the 500-character cap applies to the whole item body, location included; a `tool_error` header still lists any problems the partial report carries. **`format_web_diagnostic_result`** redacts and uses the rendered text when there is one, and otherwise runs today's code byte for byte. `Artifacts:` is unchanged. **Step 6:** the `WebDiagnostic` arm was `browser_tool_result_failed`'s **only** caller, so the function was deleted. It had no test of its own. Runner failures still count, because the MCP runner returns either `Err` or `is_error: true`, and `foundation.rs`'s `repeated_failed_browser_diagnostics_are_stopped` (`StaticWebDiagnosticsRunner::error`, `is_error: true`) still passes. **Step 7:** the shaping after the call moved into `browser_report_from_tool_result(report, call, data_dir, via)` in `desktop-shell/src/lib.rs`, so it can be tested without an MCP server. There was no existing materialisation test, so a new end-to-end one was added: `a_structured_browser_report_is_rendered_and_lists_the_materialised_artifact`. It runs a real turn with a runner that calls that helper on a companion JSON whose screenshot is a real temp file under `…/runs/`. It pins that `Artifacts:` lists `web-diagnostics/<session>/<task>/run-…/20260925-1-page.png (1280x720)`, that the companion path does not appear, that there is no prose prefix, and that there is a `- Source:` line. **Step 8:** the §5.1 example was rewritten, with a sentence citing `context.md` §3.2. **Tests:** `workspace-engine` with filter `web_diagnostics + web + browser` 22/22 (includes the 6 new renderer tests; the 2 retry tests were run separately with `failing_tool + retry_limit`, 2/2), `desktop-shell` with filter `browser + web_diagnostic` 1/1. `cargo fmt --check` and `clippy -p workspace-engine -p desktop-shell --all-targets -D warnings` are clean. **Mutations:** (1) reverting Step 6 to the verbatim old helper fails `a_page_with_errors_is_not_a_failing_tool` (2 runs, not 4). (2) Always rendering `report.text` fails that test and the desktop-shell test on the header. (3) Skipping materialisation fails the desktop-shell test with the `/…/runs/` path. **Deviation — the Step 6 mutation:** once Step 4 is in place, the renderer heads a report "failed: …" exactly when `tool_failed()` is true. The old helper's `starts_with` clause therefore agrees with the new rule, as §3.2 intended. The two rules differ only on the old `contains("tool call failed" / "mcp tool reported an error")` clauses. So test 1's page error is `"Error: agent tool call failed: …"`, which the old rule wrongly counted. `a_companion_tool_error_counts_toward_the_retry_limit` cannot fail when Step 6 alone is reverted, but it did fail against the code from before Steps 4 and 6 (the Step 5 run: 4 runs, not 2). The retry tests use a new `FixedReportWebRunner` plus an `inspect_four_times` helper (one call per round). **For Task 3:** record from `report.redacted(&scanner)` at the same point in the arm where `report` is still an `Ok(WebDiagnosticReport)`, before `format_web_diagnostic_result` consumes it. The second dispatch site goes through `run_web_diagnostic_call` (report run and formatted in one step), so it needs the same split. `via` is now set on every MCP-runner report. The URL-path secret gap from Task 1 still applies to the rendered text. That gap is now closed (2026-09-25): a `ghp`, `github_pat` or `xox*` token right after `/` is redacted, while the short prefixes `sk`/`pk`/`rk` still ignore a `/`-preceded match to keep paths such as `pkg_…` and `skills_…` intact. |
| 3 · Record, persist and stream diagnostics; `/api/session` field; reveal endpoint | Done 2026-09-28 | **Landed:** `WebDiagnosticRecord` (`web_diagnostics.rs`, re-exported). `SessionStore::append_web_diagnostic` / `read_session_web_diagnostics` (`session.rs`, event `web_diagnostic_recorded`, replayed over `active_events`, keyed by task id in log order; a missing log is an empty map). `ChatOrchestrator::run_and_record_web_diagnostic` is the only place a diagnostic runs: both the agentic arm and the approval resume call it, and `run_web_diagnostic_call` was deleted. The record takes `task_id`/`session_id` from the call's `with_context`, so the resume records under the task that asked for it. A record is written only when the runner returned a report (`Ok`), not for `Err`. **Id prefix:** `"webdiag"` was already the approval proposal's id (`chat.rs`), so records use `create_id("webdiagrec")`. **Shell:** `TurnEvent::WebDiagnostic(Box<WebDiagnosticRecord>)`, `web_diagnostic_json` (`serde_json::to_string`, `{}` on the impossible error, like `plan_json`'s evidence fallback), `task_states_json` gained a `web_diagnostics` parameter before `config`, and `POST /api/reveal-web-diagnostic-artifact` loads config with `config_for_repo` (the config half of `engine_for_repo`, so no engine is built) and runs `open -R` through the pure `web_diagnostic_reveal_target`. **For Task 4 — the wire contract:** (1) SSE event name **`web_diagnostic`**; its `data` is exactly one record's JSON. (2) Record JSON: `{"id":"webdiagrec_…","taskId":"task_…","tool":"inspect_web_page"|"run_web_scenario","url":"…","recordedAtMs":<number>,"report":{"text","artifacts":[{kind,path,mime_type,width,height}],"is_error","details":{url,final_url,title,status,page_errors,console:[{level,text,location:{url,line,column}}],failed_requests,dom_summary,steps,tool_error}|null,"via"}}`: camelCase envelope, snake_case report (Global Constraints). Artifact `path`s are the materialised `web-diagnostics/<session>/<task>/run-…/…` relative paths, redacted. (3) `/api/session`: `tasks[].webDiagnostics: [record…]` in run order, **absent** (not `[]`) for a task with none. (4) Reveal: `POST /api/reveal-web-diagnostic-artifact`, form fields **`repo`** (required, may not be empty) and **`path`** (the artifact's `web-diagnostics/…` relative path, as in the record); responds `{"path":"<absolute canonical path>"}`, or the standard error JSON for a path outside `<data-dir>/web-diagnostics/`, an absolute path, or `..`. **Tests:** `session::tests` 2 new (replay per task/order/rewind, missing log); `chat::mode_refusal_tests` 2 new (`a_web_diagnostic_is_recorded_redacted_and_streamed`, `an_approved_web_diagnostic_is_recorded_at_resume`, both seeding Task 1's `ghp_abcdefghijklmnopqrstuvwxyz0123456789` in a page error and reading the `.jsonl` directly); desktop-shell 4 new (`/api/session` HTTP round trip via `serve_for_test`, no-field case, SSE text via `write_sse_event` into a `Vec<u8>`, reveal-target refusals) plus the ignored `reveal_web_diagnostic_artifact_endpoint_selects_the_file_in_finder` (run line in its doc comment; isolated temp data dir, ephemeral port). `desktop-shell` `test(web_diagnostic) + test(session)` 10/10; `workspace-engine` `test(web) + test(browser) + test(session)` 54/54; `cargo fmt --check` and `clippy -p workspace-engine -p desktop-shell --all-targets --locked -D warnings` clean; `cargo check --workspace --all-targets` clean. The ignored Finder test was not run. **Mutations:** (1) record from `report` instead of `report.redacted(..)`: both engine tests fail on the secret assertions. (2) the resume path back to the unrecorded run: `an_approved_web_diagnostic_is_recorded_at_resume` fails and the turn test still passes. (Extra) passing an empty map to `task_states_json` in `/api/session` fails the HTTP round trip. All reverted. **Deviations:** (a) `TurnProgress::WebDiagnostic` carries `Box<WebDiagnosticRecord>`, not the bare record. Clippy's `large_enum_variant` rejects the unboxed ~480-byte variant under `-D warnings`. (b) The persistence test is in `session.rs`'s inline `mod tests` as the task says, but the `read_session_plans` tests it was meant to sit beside are actually in `tests/plan.rs`. It uses the same store calls they use (`create_session`, `create_task`, `latest_event_seq`, `rewind_conversation`). (c) The shell tests only went red at compile time (they name symbols that did not exist yet), so the extra mutation above stands in for a behavioural red. |
| 4 · Diagnostic card in the desktop UI | Done 2026-09-28 | **Landed:** `renderWebDiagnosticCard(message, record)` in `app.js` next to the plan panel, built from small helpers (`webDiagnosticSections`, `webDiagnosticHeader`, `truncateWebDiagnosticText`, `webDiagnosticList`, `webDiagnosticDomPanel`, `webDiagnosticScreenshot`). Every report string goes in with `textContent`; the card never sets `innerHTML`. `processSseEvent` dispatches `web_diagnostic` to `handlers.webDiagnostic`; the live stream and both resume paths render it on `assistantMessage`, and `renderMessages` builds `diagnosticsByTask` beside `planByTask` and renders each record on the turn's last assistant bubble. Cards sit under the answer in run order, ahead of the plan panel, and skip a `data-record-id` already present. **No double thumbnails:** module-level `sessionHasRecordedDiagnostics`, set at the top of `renderMessages` (before any message runs the regex scan) and by a live event, cleared by `clearChat`; `appendWebDiagnosticArtifacts` returns early when it is set. **CSS:** `.web-diagnostic-card*` on the plan panel's tokens (`--line`, `--surface-soft`, 8px radius, `10px 12px`), no new tokens; Reveal and "Show all n" are `.btn-sm.btn-quiet`; the header is coloured `--warn`/`--danger`/`--ok` by state (failed = `tool_failed()`). **Harness:** `serves_the_ui_for_manual_inspection` seeds `recorded_web_diagnostic_session` (scenario record: 1 page error, a located console error, 6 warnings + a log, a 404 and a refused request, a failed step, a DOM summary, a real 160×90 PNG under `web-diagnostics/<session>/<task>/run-1/page.png`, plus a tool message listing that path) and `legacy_web_diagnostic_session` (no record, tool message naming its own PNG). Both tasks are marked `Complete` so the recovery sweep leaves them alone. The images come from `write_inspection_png` (stored deflate + CRC/Adler by hand, no image dependency). It prints both ids and `render_for_model`'s own header line for the recorded report. **Browser check** (built-in browser, harness on 4899 with its own data dir, rebuilt and restarted after each asset edit, killed by PID): header matches `render_for_model` word for word — **confirmed** (`Browser diagnostic found 1 page error, 1 console error, 6 console warnings, 2 failed requests, 1 failed step.`, compared against the line the harness printed from the Rust); every section renders, "Show all 7" expands to 7 and back to 5, the Page summary disclosure opens with `aria-expanded` — **confirmed**; thumbnail loads (160×90 blob) — **confirmed**; Reveal in Finder — **confirmed** (`200`, canonical path under the harness data dir, `open -R` exited 0; the user saw Finder open with the PNG selected, since AppleScript could not read Finder's selection from the agent's sandbox); reload renders the card again — **confirmed**; no screenshot twice — **confirmed** (recorded session: 1 image, inside the card, although its tool message names the path); legacy session keeps its regex thumbnail and shows no card — **confirmed**; no console errors — **confirmed** (built-in pane empty; a fresh Playwright load only logged the shell's `favicon.ico` 404); mobile preset — **card confirmed** (at 375px it wraps with no overflowing child), but the page itself is 607px wide there because `.thread-header` has a 608px min-content width, which predates this task. Viewport reset to desktop. **Wording drift:** none found. Only the counted-problems header was compared in the browser; the tool-error and clean-page headers are mirrored in code but no seeded record exercises them. **Deviations:** (a) the DOM summary and the raw `<pre>` for a runner without `details` use `createDisclosure`, not a bare `<details>`: `UI_STYLE_GUIDE.md` §7 allows one disclosure implementation. Their triggers share a footer row and the panels sit above it. (b) List items drop the model text's kind prefix (`pageerror:`, `failed request:`) because each list has its own label; console errors and warnings are one "Console" list in log order; items are capped at 500 characters like the model text, with the full text in `title`. (c) The card's screenshot grid uses `auto-fill`: the legacy `auto-fit` stretched a single screenshot across the whole card. The legacy grid's `margin-top` moved to `.message-body > .web-diagnostic-artifacts` rather than being undone inside the card. **Checks:** `node --check` OK; `npm run lint:web` clean for these files (its one info is the pre-existing template-literal hint in the spec-status script); `cargo fmt --check` OK; `clippy -p desktop-shell --all-targets --locked -D warnings` clean; `typos` on the changed files clean. **For Task 5:** (1) The live and resume `web_diagnostic` handlers were only checked by reading the code. The seeded harness exercises the reload path alone, so the live run is their first real test. (2) `docs/ui-style-guide.html` has no specimen of the card yet (§9 asks for one; that file was outside this task). (3) Pre-existing and not fixed: the legacy `.web-diagnostic-artifact figcaption` uses `var(--mono)`, which `:root` never defines; at the mobile preset `.thread-header` and long unbroken tool-message lines overflow the page; the session search popover showed open on every load in the built-in pane (not checked whether that predates this task). |
| 5 · Live scenario run, docs, and closing the spec | Blocked 2026-09-28 on spec 48 | **Step 1 was run on 2026-09-28. The diagnostics passed; the turn failed.** Full record in proposal §8. Setup: snake-game via Docker Compose on 5001. The committed code no longer has the bug, so a throwaway `updateHighScoreDisplay();` before `const game = …` recreated the load-time TDZ error; it was reverted and the image rebuilt. The shell was this checkout on 4901 with its own `DAMAIAN_DATA_DIR`, holding only the `mcp_server.playwright-mcp.*` and provider lines, with the Keychain reference kept. Provider: DeepSeek `deepseek-v4-flash`, with the user's consent. **Deviation:** the standalone `damaian-desktop-shell` cannot authenticate a browser, because the token comes only from Tauri and is never printed, and Tauri is pinned to 4765. So a throwaway launcher outside the repo called the public `desktop_shell::run_server` with a known token. **Confirmed:** same-turn page error in the model text (turn 1, `inspect_web_page`, no approval); the live card during the turn; the scenario approval showing its origin and `BROWSER-MEDIUM`; all 8 companion steps succeeding; the card rendered on the approval-resume path; two 1280×900 PNG screenshots under `<data-dir>/web-diagnostics/…`; Reveal (200 with the canonical path; Finder opened the folder); both cards and thumbnails surviving a shell restart. **Failed:** turn 2's model call after the resume streamed a complete answer over HTTP 200, and spec 48's `classify_refusal` then classified it as `provider_refused` (`Unknown`) through its body fallback. The task ended `failed`, and that turn's Approve button came back enabled. Recorded in spec 48 §7. Also noticed: the legacy `run_scenario` adapter's extra `goto` loads the page twice, so each load-time error is reported twice; turn 1's inspection returned no DOM summary or screenshot, for an unknown reason. **Not done, because the run failed:** Steps 2–6 (criteria mapping, user docs, spec 22 hand-off, status records, full gate), and the `ui-style-guide.html` card specimen. Workspace clippy was run once early and passed (exit 0, 112s warm), but that does not stand in for Step 6. **To resume:** after the spec 48 fix, re-run Step 1's two turns, then continue from Step 2. |

**Goal:** Stop discarding the structured evidence the browser companion
already returns. Type it, lead the model's text with it, persist it, and show
it in a card, then close spec 12 against a real scenario run.

**Architecture:** Everything is built on one typed value. `WebDiagnosticReport`
gains `details: Option<WebDiagnosticDetails>`, parsed tolerantly from the
companion's JSON in `from_text` (Task 1). The model text (Task 2), the
session-log record and the live stream (Task 3), and the card (Task 4) all
read that value. None of them re-parse text. When the text is not
companion-shaped JSON, `details` is `None` and every path keeps today's
behaviour.

**Tech Stack:** Rust 2024 and `serde_json`, no new dependencies. Vanilla JS in
`app.js` and CSS in `style.css`, no new web dependency.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) before Task 1.** §1.1 is the companion's
  exact output shape. §3 records five decisions: tolerant parsing,
  tool-failed versus page-problem, the persisted record, the reveal path
  check, and what spec 22 consumes. Do not re-derive them.
- **Never worse than today.** A browser MCP server that isn't the companion
  must behave exactly as it does now. Any text that is not a JSON object with
  at least one companion field gives `details: None`, and that path is
  unchanged. Tasks 1 and 2 each assert this.
- **Redact before anything leaves the engine.** `SessionStore` is deliberately
  unredacted (spec 17 §5.4). Every `WebDiagnosticRecord` is built from
  `report.redacted(&scanner)`, never from the raw report. That covers what
  reaches the session log, SSE and `/api/session`.
- **One parse, one renderer.** The companion JSON is parsed once, in
  `WebDiagnosticReport::from_text`. The header wording lives once, in
  `WebDiagnosticReport::render_for_model` (Task 2). The card (Task 4) mirrors
  that wording in JS because it cannot call Rust. If the two drift, it is a
  bug in the card.
- **A page error is not a tool failure** (`context.md` §3.2). Only
  `tool_failed()` counts toward `agent_tool_retry_limit`.
- **Wire shape.** Fields inside the report tree (`WebDiagnosticReport`,
  `WebDiagnosticDetails` and its children) serialise **snake_case**, matching
  the existing `WebDiagnosticReport`/`WebDiagnosticArtifact` and the
  companion. The session envelope `WebDiagnosticRecord` serialises
  **camelCase**, matching every other session event.
- **Test data directory:** tests use a temp repo with `data_dir:
  repo.join(".damaian")` and `enable_index_watcher: false`. They never touch
  the user's real Damaian data.
- **Port 4765 is the user's.** Manual shells use 4899, the harness port, or
  another free port, plus a separate `DAMAIAN_DATA_DIR`. Kill only by PID.
- **Scoped checks per task.** Each task lists its own checks. The full
  seven-check gate from `AGENTS.md` runs once, in Task 5.
- **Do not commit.** Leave changes in the working tree and report the check
  results. The user decides when to commit.

## File map

| File | Responsibility in this plan |
|---|---|
| `crates/workspace-engine/src/web_diagnostics.rs` | Typed details, parse, redact, `tool_failed`, `render_for_model`, `WebDiagnosticRecord` (Tasks 1–3) |
| `crates/workspace-engine/src/lib.rs` | Re-export the new types (Tasks 1, 3) |
| `crates/workspace-engine/src/chat.rs` | Use the renderer, count retries on `tool_failed`, record and stream at both dispatch sites, `TurnProgress::WebDiagnostic` (Tasks 2–3) |
| `crates/workspace-engine/src/session.rs` | `append_web_diagnostic`, `read_session_web_diagnostics` (Task 3) |
| `crates/desktop-shell/src/lib.rs` | Runner stops prefixing structured reports and sets `via`; SSE event; `tasks[].webDiagnostics`; reveal endpoint; harness seed (Tasks 2–4) |
| `crates/desktop-shell/static/app.js`, `style.css` | The card (Task 4) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, spec records | Task 5 |

---

## Task 1: Typed report

**Files:**
- Modify: `crates/workspace-engine/src/web_diagnostics.rs`: types, `from_text`, `extract_artifacts_from_text`, new `mod tests`
- Modify: `crates/workspace-engine/src/lib.rs`: the `pub use web_diagnostics::{…}` list

**Interfaces:**
- Produces:
  - `WebDiagnosticReport { text, artifacts, is_error, details: Option<WebDiagnosticDetails>, via: Option<String> }`. The two new fields are `#[serde(default)]`.
  - `WebDiagnosticDetails`, `WebConsoleEntry`, `WebSourceLocation`, `WebFailedRequest`, `WebDomSummary`, `WebScenarioStep`, exactly as defined in Step 3.
  - `WebDiagnosticReport::redacted(&self, scanner: &SecretScanner) -> WebDiagnosticReport`
  - `WebDiagnosticReport::tool_failed(&self) -> bool`
  - `WebDiagnosticDetails::problem_count(&self) -> usize`

- [x] **Step 1: Write the failing tests.** Append to `web_diagnostics.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_scanner::SecretScanner;

    /// Trimmed from a real `inspect_page` result of the companion server
    /// (`context.md` §1.1). Keys and nesting are the companion's.
    const COMPANION_REPORT: &str = r#"{
      "success": false, "diagnostic_ok": false, "run_id": "20260925-1",
      "url": "http://localhost:5001/", "final_url": "http://localhost:5001/",
      "title": "Snake Game", "status": 200,
      "page_errors": ["ReferenceError: Cannot access 'game' before initialization"],
      "console": [
        {"type": "error", "text": "Failed to load resource: 404",
         "location": {"url": "http://localhost:5001/js/app.js", "lineNumber": 41, "columnNumber": 7}},
        {"type": "log", "text": "booting"}
      ],
      "failed_requests": [
        {"kind": "response", "url": "http://localhost:5001/api/me", "method": "GET",
         "resource_type": "fetch", "status": 404, "status_text": "Not Found"},
        {"kind": "requestfailed", "url": "http://localhost:5001/ws", "method": "GET",
         "resource_type": "websocket", "failure": "net::ERR_CONNECTION_REFUSED"}
      ],
      "dom_summary": {"forms": 1, "buttons": ["Log in", "Register"], "fields": [],
        "status_text": "", "visible_text_excerpt": "Snake Log in Register Score: 0"},
      "artifacts": ["/tmp/runs/20260925-1-page.png"],
      "artifact_metadata": [{"kind": "screenshot", "path": "/tmp/runs/20260925-1-page.png",
        "mime_type": "image/png", "width": 1280, "height": 720, "caption": "page"}],
      "text_report": "ignored by Damaian"
    }"#;

    #[test]
    fn a_companion_report_parses_into_typed_details() {
        let report = WebDiagnosticReport::from_text(COMPANION_REPORT, false);
        let details = report.details.expect("companion JSON has details");
        assert_eq!(details.final_url.as_deref(), Some("http://localhost:5001/"));
        assert_eq!(details.title.as_deref(), Some("Snake Game"));
        assert_eq!(details.status, Some(200));
        assert_eq!(
            details.page_errors,
            vec!["ReferenceError: Cannot access 'game' before initialization"]
        );
        assert_eq!(details.console.len(), 2);
        assert_eq!(details.console[0].level, "error");
        // Playwright's console location is 0-based; stored 1-based.
        assert_eq!(
            details.console[0].location,
            Some(WebSourceLocation {
                url: "http://localhost:5001/js/app.js".to_string(),
                line: Some(42),
                column: Some(8),
            })
        );
        assert_eq!(details.failed_requests[0].status, Some(404));
        assert_eq!(details.failed_requests[0].failure.as_deref(), Some("Not Found"));
        assert_eq!(
            details.failed_requests[1].failure.as_deref(),
            Some("net::ERR_CONNECTION_REFUSED")
        );
        let dom = details.dom_summary.expect("dom summary");
        assert_eq!(dom.buttons, vec!["Log in", "Register"]);
        assert_eq!(details.tool_error, None);
        // 1 page error + 1 console error (the `log` line is not a problem)
        // + 2 failed requests.
        assert_eq!(details.problem_count(), 4);
    }

    #[test]
    fn artifact_metadata_supplies_kind_and_dimensions() {
        let report = WebDiagnosticReport::from_text(COMPANION_REPORT, false);
        assert_eq!(
            report.artifacts,
            vec![WebDiagnosticArtifact {
                kind: "screenshot".to_string(),
                path: "/tmp/runs/20260925-1-page.png".to_string(),
                mime_type: Some("image/png".to_string()),
                width: Some(1280),
                height: Some(720),
            }]
        );
    }

    #[test]
    fn text_that_is_not_companion_json_has_no_details() {
        for text in [
            "Browser diagnostic failed: 1 page error.\n- pageerror: boom",
            "{\"unrelated\": true}",
            "[1, 2, 3]",
            "",
        ] {
            let report = WebDiagnosticReport::from_text(text, false);
            assert_eq!(report.details, None, "{text:?}");
            assert_eq!(report.text, text);
        }
    }

    #[test]
    fn a_field_of_the_wrong_type_is_dropped_and_the_rest_kept() {
        let report = WebDiagnosticReport::from_text(
            r#"{"page_errors": "not a list", "title": 7, "final_url": "http://localhost:1/",
                "console": [{"type": "error"}, 5, {"type": "warning", "text": "slow"}]}"#,
            false,
        );
        let details = report.details.expect("final_url makes it companion-shaped");
        assert!(details.page_errors.is_empty());
        assert_eq!(details.title, None);
        assert_eq!(details.console.len(), 1, "entries without text are dropped");
        assert_eq!(details.console[0].text, "slow");
    }

    #[test]
    fn tool_failed_is_the_runner_failing_not_the_page() {
        let page_broken = WebDiagnosticReport::from_text(COMPANION_REPORT, false);
        assert!(!page_broken.tool_failed());

        let companion_error = WebDiagnosticReport::from_text(
            r#"{"error": true, "success": false, "message": "Timeout 30000ms exceeded",
                "final_url": "http://localhost:5001/"}"#,
            false,
        );
        assert!(companion_error.tool_failed());
        assert_eq!(
            companion_error.details.unwrap().tool_error.as_deref(),
            Some("Timeout 30000ms exceeded")
        );

        assert!(WebDiagnosticReport::from_text("anything", true).tool_failed());
    }

    #[test]
    fn scenario_steps_are_parsed() {
        let report = WebDiagnosticReport::from_text(
            // `r##` because the selector contains `"#`, which ends an `r#` string.
            r##"{"final_url": "http://localhost:5001/", "results": [
                {"step": 0, "action": "fill", "success": true, "selector": "#u"},
                {"step": 1, "action": "click", "success": false, "error": "not visible"}]}"##,
            false,
        );
        let steps = report.details.unwrap().steps;
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[1],
            WebScenarioStep {
                step: 1,
                action: "click".to_string(),
                success: false,
                error: Some("not visible".to_string()),
            }
        );
    }

    #[test]
    fn redacted_scrubs_every_captured_string() {
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let raw = format!(
            r#"{{"final_url": "http://localhost:5001/?t={secret}", "title": "{secret}",
                "page_errors": ["token {secret}"],
                "console": [{{"type": "error", "text": "{secret}",
                  "location": {{"url": "http://localhost:5001/{secret}.js", "lineNumber": 0}}}}],
                "failed_requests": [{{"url": "http://localhost:5001/{secret}", "failure": "{secret}"}}],
                "dom_summary": {{"buttons": ["{secret}"], "status_text": "{secret}",
                  "visible_text_excerpt": "{secret}"}},
                "results": [{{"step": 0, "action": "click", "success": false, "error": "{secret}"}}],
                "error": true, "message": "{secret}"}}"#
        );
        let report = WebDiagnosticReport::from_text(raw, false);
        let redacted = report.redacted(&SecretScanner::new(Vec::new()));
        let serialized = serde_json::to_string(&redacted).unwrap();
        assert!(!serialized.contains(secret), "{serialized}");
    }
}
```

Before relying on it, check that `SecretScanner::new(Vec::new())` redacts the
`ghp_…` literal. The existing scanner tests in `secret_scanner.rs` show which
fixture tokens its default rules match. If `ghp_` is not one of them, use one
from there, and never a real credential.

- [x] **Step 2: Run to verify the tests fail**

Run: `cargo nextest run -p workspace-engine -E 'test(web_diagnostics::tests)'`
Expected: compile failure. `details`, `WebSourceLocation`, `redacted`,
`tool_failed` and the rest do not exist yet.

- [x] **Step 3: Implement the types.** In `web_diagnostics.rs`, extend the report
and add the details tree. The new structs derive `Default` so that tolerant
parsing can fill in fields one at a time:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDiagnosticReport {
    pub text: String,
    pub artifacts: Vec<WebDiagnosticArtifact>,
    pub is_error: bool,
    /// Present when `text` was a companion-shaped JSON object
    /// (`context.md` §1.1). `None` for any other runner, whose text keeps
    /// today's raw path.
    #[serde(default)]
    pub details: Option<WebDiagnosticDetails>,
    /// Which runner produced this, e.g. "MCP server `x` tool `inspect_page`".
    #[serde(default)]
    pub via: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDiagnosticDetails {
    pub url: Option<String>,
    pub final_url: Option<String>,
    pub title: Option<String>,
    pub status: Option<u16>,
    pub page_errors: Vec<String>,
    pub console: Vec<WebConsoleEntry>,
    pub failed_requests: Vec<WebFailedRequest>,
    pub dom_summary: Option<WebDomSummary>,
    /// Scenario step results; empty for an inspection.
    pub steps: Vec<WebScenarioStep>,
    /// The runner's own failure message, when the call itself failed
    /// (companion `"error": true`) — not a problem found on the page.
    pub tool_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebConsoleEntry {
    /// As the browser reported it: `error`, `warning`, `log`, `assert`, ...
    pub level: String,
    pub text: String,
    pub location: Option<WebSourceLocation>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSourceLocation {
    pub url: String,
    /// 1-based. Playwright reports 0-based; converted at parse.
    pub line: Option<u32>,
    /// 1-based, like `line`.
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebFailedRequest {
    pub url: String,
    pub method: Option<String>,
    pub resource_type: Option<String>,
    /// HTTP status for a response ≥ 400; `None` for a request that never
    /// completed.
    pub status: Option<u16>,
    /// `status_text` for a response, the network error for a failed request.
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDomSummary {
    pub forms: Option<u32>,
    pub buttons: Vec<String>,
    pub status_text: Option<String>,
    pub visible_text_excerpt: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebScenarioStep {
    pub step: u32,
    pub action: String,
    pub success: bool,
    pub error: Option<String>,
}
```

`dom_summary.fields` is left out on purpose. It is up to 40 records of form
metadata, and neither the model header nor the card uses it. Add it when
something needs it.

- [x] **Step 4: Implement the parse.** Change `from_text` to parse once and
share the value:

```rust
impl WebDiagnosticReport {
    pub fn from_text(text: impl Into<String>, is_error: bool) -> Self {
        let text = text.into();
        let value = serde_json::from_str::<Value>(text.trim()).ok();
        let artifacts = value.as_ref().map(artifacts_from_value).unwrap_or_default();
        let details = value.as_ref().and_then(details_from_value);
        Self { text, artifacts, is_error, details, via: None }
    }

    pub fn tool_failed(&self) -> bool {
        self.is_error
            || self
                .details
                .as_ref()
                .is_some_and(|details| details.tool_error.is_some())
    }
}

impl WebDiagnosticDetails {
    /// Page errors, console errors and warnings, failed requests, and failed
    /// scenario steps: what the model header counts.
    pub fn problem_count(&self) -> usize {
        self.page_errors.len()
            + self.console.iter().filter(|entry| entry.is_problem()).count()
            + self.failed_requests.len()
            + self.steps.iter().filter(|step| !step.success).count()
    }
}

impl WebConsoleEntry {
    pub fn is_problem(&self) -> bool {
        matches!(
            self.level.to_ascii_lowercase().as_str(),
            "error" | "warning" | "warn" | "assert"
        )
    }
}

/// The keys that make a JSON object companion-shaped. Any one is enough; an
/// object with none of them (`{"unrelated": true}`) is some other server's
/// output and keeps the raw path.
const COMPANION_KEYS: [&str; 6] =
    ["final_url", "page_errors", "console", "failed_requests", "dom_summary", "results"];

fn details_from_value(value: &Value) -> Option<WebDiagnosticDetails> {
    let object = value.as_object()?;
    if !COMPANION_KEYS.iter().any(|key| object.contains_key(*key)) {
        return None;
    }
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
    let list = |key: &str| object.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    Some(WebDiagnosticDetails {
        url: text("url"),
        final_url: text("final_url"),
        title: text("title"),
        status: object.get("status").and_then(Value::as_u64).and_then(|s| u16::try_from(s).ok()),
        page_errors: list("page_errors")
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        console: list("console").iter().filter_map(console_entry).collect(),
        failed_requests: list("failed_requests").iter().filter_map(failed_request).collect(),
        dom_summary: object.get("dom_summary").and_then(dom_summary),
        steps: list("results").iter().filter_map(scenario_step).collect(),
        tool_error: (object.get("error").and_then(Value::as_bool) == Some(true))
            .then(|| text("message").unwrap_or_else(|| "the diagnostic tool reported an error".to_string())),
    })
}
```

Write `console_entry`, `failed_request`, `dom_summary` and `scenario_step` as
`fn(&Value) -> Option<T>`. Each drops an item without its required string:
`text` for console entries, `url` for requests, `action` for steps. Other
fields are optional, and a wrong type makes them `None` or empty. For a
console `location`, convert with `lineNumber`/`columnNumber` as u64 + 1.
`failed_request.failure` is `failure` when present, otherwise `status_text`.
A `dom_summary` that is `{"error": …}` has no known keys, so it becomes
`WebDomSummary::default()`. Return `None` in that case so the card shows no
empty section.

Rename `extract_artifacts_from_text(&str)` to `artifacts_from_value(&Value)`.
Take `artifact_metadata` first when it is a non-empty array, parsing each
record the way the object branch parses one today. Then append any
`artifacts` string whose path is not already in the list. Nothing else calls
the old function (`grep -rn extract_artifacts_from_text crates/` should find
only this file).

- [x] **Step 5: Implement `redacted`.** A method that clones the report and
runs every `String` in it through `scanner.redact(..).text`. That covers
`text`, `via`, every details field (including `location.url`, `method` and
`resource_type`), each artifact `path`, `buttons`, and step `action`/`error`.
Write it as one explicit function per struct, not a generic walker over
serialised JSON, so that a field added later without redaction is visible in
review.

- [x] **Step 6: Re-export.** Add `WebConsoleEntry, WebDiagnosticDetails,
WebDomSummary, WebFailedRequest, WebScenarioStep, WebSourceLocation` to the
`pub use web_diagnostics::{…}` list in `lib.rs`.

- [x] **Step 7: Run to verify the tests pass**

Run: `cargo nextest run -p workspace-engine -E 'test(web_diagnostics) + test(web_inspection) + test(browser)'`
Expected: the seven new tests pass. The existing browser tests in `chat.rs`
and `tests/foundation.rs` still pass unchanged, because their fixtures are
plain text and have `details: None`.

- [x] **Step 8: Mutation check.** Revert each change after checking it fails.
  - Drop the `+ 1` on `lineNumber`: `a_companion_report_parses…` fails.
  - Make `details_from_value` accept any object: `text_that_is_not_companion_json…` fails.
  - Skip redacting `location.url`: `redacted_scrubs…` fails.

- [x] **Step 9: Scoped checks.** `cargo fmt --all -- --check`, then `cargo
clippy -p workspace-engine --all-targets --locked -- -D warnings`. Update this
task's Progress row.

---

## Task 2: Model text from the typed report

**Files:**
- Modify: `crates/workspace-engine/src/web_diagnostics.rs`: `render_for_model` and tests
- Modify: `crates/workspace-engine/src/chat.rs`: `format_web_diagnostic_result` and the retry arm in `run_agentic_turn` (the `ToolAction::WebDiagnostic` dispatch, `failed_browser_calls`)
- Modify: `crates/desktop-shell/src/lib.rs`: `McpBrowserDiagnosticsRunner::call_compatible_tool`
- Modify: `docs/specs/12_web_app_troubleshooting/proposal.md` §5.1: the text example

**Interfaces:**
- Consumes: Task 1's `details`, `via`, `tool_failed()`, `problem_count()`, `WebConsoleEntry::is_problem()`
- Produces: `WebDiagnosticReport::render_for_model(&self) -> Option<String>`. It is `None` when `details` is `None`. Task 4 copies its header wording.

- [x] **Step 1: Write the failing renderer tests.** Add to `web_diagnostics.rs`'s tests:

```rust
    #[test]
    fn the_model_text_leads_with_the_problems_found() {
        let text = WebDiagnosticReport::from_text(COMPANION_REPORT, false)
            .render_for_model()
            .unwrap();
        let mut lines = text.lines();
        assert_eq!(
            lines.next(),
            Some("Browser diagnostic found 1 page error, 1 console error, 2 failed requests.")
        );
        assert!(text.contains(
            "- pageerror: ReferenceError: Cannot access 'game' before initialization"
        ));
        assert!(text.contains(
            "- console error: Failed to load resource: 404 (http://localhost:5001/js/app.js:42:8)"
        ));
        assert!(text.contains("- failed request: GET http://localhost:5001/api/me → 404 Not Found"));
        assert!(text.contains(
            "- failed request: GET http://localhost:5001/ws → net::ERR_CONNECTION_REFUSED"
        ));
        assert!(text.contains("- URL: http://localhost:5001/ (HTTP 200)"));
        assert!(text.contains("- Title: Snake Game"));
        assert!(text.contains("- Visible buttons: Log in, Register"));
        assert!(!text.contains("booting"), "non-problem console lines are omitted");
        assert!(!text.contains("ignored by Damaian"), "the companion's own prose is not used");
        assert!(!text.contains("\"page_errors\""), "no raw JSON");
    }

    #[test]
    fn a_clean_page_says_so() {
        let text = WebDiagnosticReport::from_text(
            r#"{"final_url": "http://localhost:5001/", "title": "Ok", "status": 200,
                "page_errors": [], "console": [], "failed_requests": []}"#,
            false,
        )
        .render_for_model()
        .unwrap();
        assert!(text.starts_with(
            "Browser diagnostic found no page errors, console problems, or failed requests."
        ));
    }

    #[test]
    fn a_tool_failure_is_headed_failed() {
        let text = WebDiagnosticReport::from_text(
            r#"{"error": true, "message": "Timeout 30000ms exceeded",
                "final_url": "http://localhost:5001/"}"#,
            false,
        )
        .render_for_model()
        .unwrap();
        assert!(text.starts_with("Browser diagnostic failed: Timeout 30000ms exceeded"));
    }

    #[test]
    fn failed_scenario_steps_are_listed() {
        let text = WebDiagnosticReport::from_text(
            r#"{"final_url": "http://localhost:5001/", "results": [
                {"step": 0, "action": "fill", "success": true},
                {"step": 1, "action": "click", "success": false, "error": "not visible"}]}"#,
            false,
        )
        .render_for_model()
        .unwrap();
        assert!(text.starts_with("Browser diagnostic found 1 failed step."));
        assert!(text.contains("- step 1 click failed: not visible"));
    }

    #[test]
    fn long_lists_and_long_items_are_bounded() {
        let errors: Vec<String> = (0..20).map(|i| format!("\"e{i} {}\"", "x".repeat(900))).collect();
        let text = WebDiagnosticReport::from_text(
            format!(r#"{{"final_url": "http://localhost:1/", "page_errors": [{}]}}"#, errors.join(",")),
            false,
        )
        .render_for_model()
        .unwrap();
        assert!(text.starts_with("Browser diagnostic found 20 page errors."));
        assert_eq!(text.matches("- pageerror: ").count(), 5);
        assert!(text.contains("- … 15 more page errors"));
        assert!(text.lines().all(|line| line.chars().count() <= 520));
    }

    #[test]
    fn no_details_means_no_rendering() {
        assert_eq!(WebDiagnosticReport::from_text("plain", false).render_for_model(), None);
    }
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine -E 'test(web_diagnostics::tests)'`
Expected: compile failure, because `render_for_model` does not exist yet.

- [x] **Step 3: Implement `render_for_model`.** The rules the tests pin:
  - **Header when `tool_error` is `Some(m)`:** `Browser diagnostic failed: {m}`.
  - **Header otherwise:** `Browser diagnostic found {parts}.`
    - `parts` joins the non-zero counts with `", "`, in this order: page
      errors, console errors, console warnings, failed requests, failed
      steps. Console errors are levels `error` or `assert`. Console warnings
      are `warning` or `warn`.
    - Use singular and plural forms (`1 page error`, `2 page errors`).
    - With no problems, the header is
      `Browser diagnostic found no page errors, console problems, or failed requests.`
  - **Problem sections, in header order.** Each line starts with `- `:
    - `pageerror: {text}`
    - `console {level}: {text}`, followed by ` ({url}:{line}:{column})` when
      there is a location. Leave out any part of the location that is `None`.
    - `failed request: {METHOD} {url} → {status} {failure}` for a response.
      For a request that never completed, `→ {failure}`. When there is no
      method, write `{url} → …`.
    - `step {n} {action} failed: {error}`
  - **Limits.** Each section lists at most **5** items, then
    `- … {n} more {plural noun}`. Truncate each item's text to **500**
    characters with a trailing `…`.
  - **Then the context lines:**
    - `- URL: {final_url}`, plus ` (HTTP {status})` when there is a status
    - `- Title: {title}`, when present
    - `- Visible buttons: {a, b}`, when there are any
    - `- Status text: {status_text}`, when non-empty
    - `- Visible text: {excerpt truncated to 500}`, when non-empty
  - **Last:** `- Source: {via}`, when `via` is set.

  Write it as a sequence of small helpers (`pluralize`, `truncate_chars`,
  `push_section`). Keep it free of I/O, so it can be tested without a runner.

- [x] **Step 4: Use it in `format_web_diagnostic_result`** (`chat.rs`).

  When `report.render_for_model()` is `Some(text)`, redact `text` with
  `self.scanner` and use it in place of `report.text`. Skip the
  `is_error`/"Browser diagnostic failed:\n" re-prefix, because the renderer
  already chose the header. Keep the existing `Artifacts:` list after it,
  unchanged.

  When it is `None`, the code path is today's, byte for byte.

- [x] **Step 5: Write the failing retry tests** in `chat.rs`'s test module,
next to `CountingWebRunner`. Both tests use a counting runner that returns a
fixed `WebDiagnosticReport::from_text(<json>, false)`. Both script a
`MockModelAdapter` that calls `inspect_web_page` on `http://localhost:5001/`
four times with identical arguments and then answers. The existing
`CountingWebRunner` tests show how to script repeated calls. With
`agent_tool_retry_limit` at its default of 2:

1. `a_page_with_errors_is_not_a_failing_tool`. The JSON is companion-shaped
   with one page error. Assert:
   - the runner ran **4** times
   - no tool result equals `browser_retry_limit_note(2)`
   - the first tool result starts with `Browser diagnostic found 1 page error.`
2. `a_companion_tool_error_counts_toward_the_retry_limit`. The JSON is
   `{"error": true, "message": "Timeout", "final_url": "http://localhost:5001/"}`,
   with MCP `is_error` false, which is how the companion reports a timeout.
   Assert that the runner ran **2** times and the third and fourth results
   are the retry note.

Run: `cargo nextest run -p workspace-engine -E 'test(failing_tool) + test(retry_limit)'`
Expected: both fail against the text-based rule.
- Test 1 fails on the header, because the old content is the raw JSON.
- Test 2 fails on the count: raw JSON never starts with "Browser diagnostic
  failed", so the old rule never counts it and the runner runs 4 times.

- [x] **Step 6: Count retries on `tool_failed`.** In `run_agentic_turn`'s
`ToolAction::WebDiagnostic` arm, `let failed = browser_tool_result_failed(&content)`
becomes:

```rust
let report = self.run_web_diagnostic_report(&call);
let failed = match &report {
    Ok(report) => report.tool_failed(),
    // No report at all — connection or configuration failure.
    Err(_) => true,
};
let content = self.format_web_diagnostic_result(report);
```

`browser_tool_result_failed` still has callers outside this arm.
`grep -n browser_tool_result_failed crates/workspace-engine/src/chat.rs`. If
this arm was its only caller, delete it along with any test that only covers
it. Record which case applied in the Progress row. Step 5's two tests should
now pass.

- [x] **Step 7: Stop the runner prefixing structured reports.** In
`call_compatible_tool` (`desktop-shell/src/lib.rs`), after
`materialize_browser_artifacts`:

```rust
let via = format!("MCP server `{}` tool `{}`", server.config.id, tool_name);
if report.details.is_none() {
    // Unstructured output: keep the old prose prefix, which is the only
    // place the model learns which server answered.
    report.text = format!(
        "{} via {via}:\n{}",
        if report.is_error { "Browser diagnostic failed" } else { "Browser diagnostic result" },
        report.text
    );
}
report.via = Some(via);
```

`materialize_browser_artifacts` rewrites artifact paths inside `report.text`,
and it must also rewrite them inside `artifact_metadata`-derived
`report.artifacts`. It already does, because it mutates
`report.artifacts[i].path`. Check that the rewritten relative path, not the
companion's `/…/runs/…` path, is what `render_for_model`'s caller lists under
`Artifacts:`. Pin it in the existing materialisation test, or add one next to
it using a temp PNG.

- [x] **Step 8: Update proposal §5.1.** Replace the text-form example with the
new header wording ("found … / failed: …"). Add one sentence citing
`context.md` §3.2 for why a page error is not written as "failed".

- [x] **Step 9: Run and check**

Run: `cargo nextest run -p workspace-engine -E 'test(web_diagnostics) + test(web) + test(browser)'`
then `cargo nextest run -p desktop-shell -E 'test(browser) + test(web_diagnostic)'`.
Expected: all pass, including the unchanged `tests/foundation.rs` fixtures
(plain text, raw path).
Mutations:
- Revert Step 6: both retry tests fail.
- Render `report.text` even when details exist: the header assertion fails.
Then `cargo fmt --all -- --check` and `cargo clippy -p workspace-engine -p
desktop-shell --all-targets --locked -- -D warnings`. Update the Progress row.

---

## Task 3: Record, persist, stream

**Files:**
- Modify: `crates/workspace-engine/src/web_diagnostics.rs`: `WebDiagnosticRecord`
- Modify: `crates/workspace-engine/src/session.rs`: append/read pair, next to `append_plan`/`read_session_plans`
- Modify: `crates/workspace-engine/src/chat.rs`: `TurnProgress::WebDiagnostic`, `TurnSink::web_diagnostic`, one recording helper used at both dispatch sites
- Modify: `crates/desktop-shell/src/lib.rs`: `TurnEvent`, `turn_progress_event`, the SSE match, `GET /api/session`, `task_states_json`, the reveal endpoint
- Modify: `crates/workspace-engine/src/lib.rs`: re-export `WebDiagnosticRecord`

**Interfaces:**
- Consumes: Task 1's `redacted`, and Task 2's `format_web_diagnostic_result` and `tool_failed`
- Produces:
  - `WebDiagnosticRecord { id, task_id, tool, url, recorded_at_ms: u128, report: WebDiagnosticReport }`, serialised camelCase at the envelope
  - `SessionStore::append_web_diagnostic(&self, session_id: &str, record: &WebDiagnosticRecord) -> Result<()>`. The event type is `web_diagnostic_recorded`.
  - `SessionStore::read_session_web_diagnostics(&self, session_id: &str) -> Result<HashMap<String, Vec<WebDiagnosticRecord>>>`, keyed by task id, in log order, over `active_events`
  - `TurnProgress::WebDiagnostic(WebDiagnosticRecord)`, which becomes the SSE event `web_diagnostic` with the record JSON as its data
  - `/api/session` `tasks[].webDiagnostics: [record…]`. The field is absent when a task has none.
  - `POST /api/reveal-web-diagnostic-artifact` with form `{repo, path}`, returning `{"path": …}`

- [x] **Step 1: The record type.** In `web_diagnostics.rs`:

```rust
/// One diagnostic run as the session keeps it: already redacted (the
/// session log is not, spec 17 §5.4), and self-describing so the card can
/// render it without the tool message beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDiagnosticRecord {
    pub id: String,
    pub task_id: String,
    /// `inspect_web_page` or `run_web_scenario`.
    pub tool: String,
    pub url: String,
    pub recorded_at_ms: u128,
    pub report: WebDiagnosticReport,
}
```

Re-export it from `lib.rs`.

- [x] **Step 2: Write the failing persistence test.** In `session.rs`, next to
the existing `read_session_plans` tests. Find them with
`grep -n "fn .*read_session_plans\|mod .*tests" crates/workspace-engine/src/session.rs`,
and use whatever store fixture those tests use.

```rust
#[test]
fn web_diagnostics_replay_per_task_in_order_and_drop_with_a_rewind() {
    // Arrange: a store and a session with two tasks, as the plan tests do.
    // Append records r1 (task A), r2 (task B), r3 (task A).
    // Assert: read_session_web_diagnostics → {A: [r1, r3], B: [r2]}.
    // Rewind the conversation to before r3 the way the plan rewind test does,
    // then assert A: [r1] — a rewound turn's diagnostics go with it, as its
    // plan does (`context.md` §3.3).
}
```

Write it out in full against the real fixture helpers. The comment above
describes the assertions, not code to leave in the test. Build the records
with `WebDiagnosticReport::from_text(<small companion JSON>, false)`.

- [x] **Step 3: Implement the pair.** `append_web_diagnostic` serialises the
record with `serde_json::to_string` and calls
`append_session_event(session_id, "web_diagnostic_recorded", &payload)`, the
same way `append_plan` does. `read_session_web_diagnostics` walks
`active_events(&content)`, deserialises each `web_diagnostic_recorded`
payload, skips any that fail, and pushes each onto
`map.entry(record.task_id.clone())`. A missing log returns an empty map, like
`read_session_plans`.

- [x] **Step 4: Run.** `cargo nextest run -p workspace-engine -E 'test(web_diagnostics_replay)'`. It should pass.

- [x] **Step 5: Write the failing engine test.** In `chat.rs`'s test module:
  1. Run a turn whose model calls `inspect_web_page` once.
  2. Use a runner that returns companion JSON with a seeded fake secret in a
     page error. Use the same fixture token Task 1 settled on.
  3. Collect `on_progress` events.

  Assert:
  - Exactly one `TurnProgress::WebDiagnostic(record)` was emitted, with
    `record.task_id == result.task.id`, `record.tool == "inspect_web_page"`,
    and `record.report.details.unwrap().page_errors.len() == 1`.
  - `engine.session_store.read_session_web_diagnostics(&session.id)` holds
    that same record under the task.
  - Neither the emitted record nor the session log file contents contain the
    secret. Read the `.jsonl` directly, not through the reader.

  A second test covers the approval path. Use a non-loopback URL so the call
  pauses for approval, as the existing approval tests near
  `resume_after_command_decision_with_options` do. Resume with approval, and
  assert the record is appended and emitted there too.

- [x] **Step 6: Implement the helper and wire both sites.** In `ChatOrchestrator`:

```rust
/// Runs a browser diagnostic, records what it found, and returns the
/// model-facing text plus whether the *tool* failed (`context.md` §3.2).
/// Both dispatch sites — the agentic turn and the approval resume — go
/// through here, so neither can forget to persist or redact.
fn run_and_record_web_diagnostic(
    &self,
    call: &WebDiagnosticCall,
    sink: &mut TurnSink<'_>,
) -> Result<(String, bool)> {
    let report = self.run_web_diagnostic_report(call);
    let failed = report.as_ref().map_or(true, WebDiagnosticReport::tool_failed);
    if let (Ok(report), Some(session_id), Some(task_id)) =
        (&report, call.session_id.as_deref(), call.task_id.as_deref())
    {
        let record = WebDiagnosticRecord {
            id: create_id("webdiag"),
            task_id: task_id.to_string(),
            tool: call.name().to_string(),
            url: self.scanner.redact(&call.url).text,
            recorded_at_ms: now_millis(),
            report: report.redacted(&self.scanner),
        };
        self.session_store.append_web_diagnostic(session_id, &record)?;
        sink.web_diagnostic(record);
    }
    Ok((self.format_web_diagnostic_result(report), failed))
}
```

Check the `webdiag` prefix first. The survey found `webdiag` already in use as
an id prefix. `grep -rn '"webdiag"' crates/` shows where. If it names the
approval proposal, use `webdiagrec` for records so the two id kinds stay
distinct, and note the choice.

Add `TurnProgress::WebDiagnostic(WebDiagnosticRecord)` with a doc comment in
the style of `Plan`. Say it is sent once per run, not per change. Add
`TurnSink::web_diagnostic`, like `plan`. Then:
- `run_agentic_turn`: Task 2's arm calls
  `self.run_and_record_web_diagnostic(&call, sink)?`.
- The resume path (`else if approved { self.run_web_diagnostic_call(&call) }`)
  calls it with the resume's `sink` and takes `.0`.
- `run_web_diagnostic_call` then has no callers. Delete it.

`TurnProgress` derives `PartialEq, Eq`. `WebDiagnosticRecord` derives both, so
nothing else changes. Check with `cargo check` for any exhaustive match on
`TurnProgress` besides `turn_progress_event`.

- [x] **Step 7: Run.** `cargo nextest run -p workspace-engine -E 'test(web) + test(browser)'`. Everything should pass.

- [x] **Step 8: Shell plumbing.** In `desktop-shell/src/lib.rs`:
  - `TurnEvent::WebDiagnostic(Box<WebDiagnosticRecord>)`. Box it for the same
    reason `Plan` is boxed.
  - Map it in `turn_progress_event`.
  - In the SSE match, add
    `TurnEvent::WebDiagnostic(record) => write_sse_event(out, "web_diagnostic", &serde_json::to_string(&record).unwrap_or_default())`.
    If `write_sse_event`'s neighbours handle serialisation errors a
    different way, follow them.
  - `GET /api/session`: read `read_session_web_diagnostics` next to
    `task_plans`, with a comment in the style of the plan one, and pass it to
    `task_states_json`.
  - `task_states_json` gains a parameter
    `web_diagnostics: &HashMap<String, Vec<WebDiagnosticRecord>>`. Emit
    `,"webDiagnostics":[…]` only when the task has records. Update
    `task_states_json`'s other callers: `grep -n "task_states_json(" crates/desktop-shell/src/lib.rs`.

  Tests, modelled on the `session_json`/plan payload tests that spec 20 Task 8
  and spec 21 added. Use `serve_for_test` where an HTTP round trip is needed.
  1. `GET /api/session` for a session with one appended record returns
     `tasks[0].webDiagnostics[0].report.details.page_errors`.
  2. A task with no records has no `webDiagnostics` key.
  3. `turn_progress_event(TurnProgress::WebDiagnostic(r))` becomes
     `TurnEvent::WebDiagnostic`, and its SSE text is `event: web_diagnostic`
     with the record JSON. Assert on the `write_sse_event` output into a
     `Vec<u8>`.

- [x] **Step 9: The reveal endpoint.** Add `("POST", "/api/reveal-web-diagnostic-artifact")`:
  1. `parse_form`, then `required_form` for `repo` and `path`.
  2. Load the config the way `GET /api/web-diagnostic-artifact` does for
     `repo`.
  3. `let file = web_diagnostic_artifact_path(&config, &path)?;`
  4. `Command::new("open").arg("-R").arg(&file)`, with the status handling
     `reveal_in_finder` uses.
  5. Respond `{"path": …}`.

  Put the side-effect-free part in a pure helper,
  `fn web_diagnostic_reveal_target(config: &Config, path: &str) -> Result<PathBuf, String>`
  (it just delegates to `web_diagnostic_artifact_path`), and test it.
  Relative paths outside `web-diagnostics/`, absolute paths and `..` are all
  refused. A real file under the temp data dir is accepted.

  The end-to-end test that actually launches Finder is `#[ignore]`d. Give it a
  doc comment with its run line, modelled exactly on
  `reveal_in_finder_endpoint_opens_the_requested_repository_root`.

- [x] **Step 10: Checks.** `cargo nextest run -p desktop-shell -E 'test(web_diagnostic) + test(session)'`
and `cargo nextest run -p workspace-engine -E 'test(web) + test(browser) + test(session)'`.
Then `cargo fmt --all -- --check` and `cargo clippy -p workspace-engine -p
desktop-shell --all-targets --locked -- -D warnings`.
Mutations:
- Build the record from `report` instead of `report.redacted(..)`: the
  secret assertion fails.
- Record only in `run_agentic_turn`: the approval-path test fails.
Update the Progress row.

---

## Task 4: Diagnostic card

**Files:**
- Modify: `crates/desktop-shell/static/app.js`
- Modify: `crates/desktop-shell/static/style.css`
- Modify: `crates/desktop-shell/src/lib.rs`: seed one record in `serves_the_ui_for_manual_inspection`

**Interfaces:**
- Consumes:
  - The SSE event `web_diagnostic` (a record)
  - `/api/session` `tasks[].webDiagnostics`
  - `POST /api/reveal-web-diagnostic-artifact {repo, path}`
  - The existing `GET /api/web-diagnostic-artifact`
- Produces: `renderWebDiagnosticCard(message, record)`

Read [`docs/UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) before styling. The
card is a secondary surface in a message, like `section.plan-panel`, and
should reuse the plan panel's tokens rather than add new ones. Use the button
scale from spec 41 for Reveal: a quiet, secondary button.

- [x] **Step 1: The render function.** Next to `renderPlanPanel`, add
`renderWebDiagnosticCard(message, record)`. It appends a
`section.web-diagnostic-card` to the assistant bubble:

  - **Header line:** the same wording `render_for_model` produces, computed in
    JS from `record.report.details`: counts in the same order, with the same
    singular and plural forms, and "failed: …" when `tool_error` is set.
    Next to it, the tool (`Inspected` or `Scenario`) and `record.url`.
  - **Meta row:** `final_url`, `HTTP {status}`, and `title`, each only when
    present.
  - **Problem lists,** each a `<ul>` only when non-empty: page errors,
    console problems (`is_problem` levels only, each with `url:line:col` when
    there is a location), failed requests, failed steps. Show 5 items, then a
    "Show all n" toggle. All text is set with `textContent`, never
    `innerHTML`, because it came from a web page.
  - **DOM summary:** a `<details>` element with buttons, status text and the
    visible-text excerpt.
  - **Screenshots:** for each `report.artifacts` whose `path` starts with
    `web-diagnostics/` and ends in an image extension, show a thumbnail
    loaded with the existing `loadWebDiagnosticArtifact(image, path)`, a
    caption, and a "Reveal in Finder" button. The button posts
    `{repo: repo(), path}` to `/api/reveal-web-diagnostic-artifact` and shows
    the error inline if it fails.
  - **When `details` is `null`** (another runner): the header reads
    `Browser diagnostic result` (or `failed` when `is_error`), and
    `report.text` goes in a collapsed `<details><pre>`. Thumbnails render the
    same way.

- [x] **Step 2: Live and reload wiring.**
  - **Live:** in `sendChatPrompt`'s stream handler, next to the `plan`
    handler (`grep -n 'renderPlanPanel(assistantMessage' app.js`), handle
    `web_diagnostic` with
    `renderWebDiagnosticCard(assistantMessage, payload)`.
  - **Resume paths:** do the same in both of them, the other two
    `renderPlanPanel(assistantMessage, …)` call sites.
  - **Reload:** in `renderMessages`, build
    `diagnosticsByTask = new Map(tasks.filter(t => t.webDiagnostics).map(t => [t.id, t.webDiagnostics]))`
    next to `planByTask`. Render each record for the task's assistant bubble
    where the plan panel is rendered.

- [x] **Step 3: No double thumbnails** (`context.md` §3.3). Keep a
module-level `sessionHasRecordedDiagnostics` flag.
  - Reset it when a session loads, setting it true if any task has
    `webDiagnostics`.
  - Set it true when a live `web_diagnostic` event arrives.
  - `appendWebDiagnosticArtifacts` returns early when the flag is set.

  Old sessions keep their regex thumbnails. New ones show screenshots only
  in the card.

- [x] **Step 4: Seed the harness.** In `serves_the_ui_for_manual_inspection`,
append one `WebDiagnosticRecord` to a seeded session, the same way it seeds
the refused Ask-mode session. Use companion JSON with a page error, a console
error with a location, a 404, a failed step, and a DOM summary. Point
`artifacts` at a small PNG written under the harness data dir's
`web-diagnostics/<session>/<task>/run-1/page.png`. Print the session id with
the others.

- [x] **Step 5: Verify in the browser.** Run the harness per its doc comment,
on 4899 with its own data dir. Open the seeded session in the built-in
browser and confirm:
  - The card renders with the header and every section. The header wording
    matches `render_for_model`'s for the same JSON; compare it with the
    engine test's expected string.
  - The thumbnail loads.
  - Reveal in Finder opens Finder with the PNG selected.
  - A reload renders the card again.
  - No screenshot appears twice.
  - An older seeded session with regex thumbnails still shows them.
  - No console errors.
  - The layout holds at the `mobile` preset width. Reset the viewport to
    `desktop` afterwards.

  The shell embeds static files with `include_str!`, so rebuild and restart
  after every edit to `app.js` or `style.css`. Kill the harness only by its
  own PID.

- [x] **Step 6: Checks.** `node --check crates/desktop-shell/static/app.js`,
`npm run lint:web` (use `npm run lint:web:fix` for formatting, and `npm ci`
first if `node_modules` is missing), `cargo clippy -p desktop-shell
--all-targets --locked -- -D warnings`. Update the Progress row with what the
browser check showed.

---

## Task 5: Live run and close-out

This is the integration task. Start it in a fresh session and read the whole
of `proposal.md` §6 and this file's Progress table.

- [ ] **Step 1: The live scenario check** (§8's open item, acceptance
criteria 1–3, 5, 7):
  1. Start `~/development/snake-game` per its README, on a free port.
  2. Start a Damaian shell of this branch on a port other than 4765, with
     its own `DAMAIAN_DATA_DIR`. Copy only the `mcp_server.playwright-mcp.*`
     lines and the model provider settings from the user's config into it.
     Never copy API keys: the provider's `model_api_key_env` keeps its
     Keychain reference.
  3. Ask the user to confirm a real provider is OK to use for this run.
     This check needs one (`context.md` §4).
  4. Ask: "Why does the Register button do nothing on http://localhost:<port>/?"
  5. Approve the scenario, and record:
     - whether the model saw the page error in the same turn
     - the card's contents
     - that the screenshot is stored under `<data-dir>/web-diagnostics/…`
     - that Reveal works
     - that the card survives a restart of the shell
  6. If snake-game no longer has the original bug, introduce a throwaway
     `throw` in its JS for the run and revert it afterwards. Say so in the
     notes.

  Record the outcome in proposal §8, replacing "Spec completion remains
  pending a real interaction-scenario check". If it fails, stop here and
  record why. Do not close the spec.

- [ ] **Step 2: Acceptance criteria.** Walk §6's 1–11. Map each to a test or to
Step 1's live observation. Write the mapping into proposal §8. Anything
unmet reopens a task instead of being noted and closed.

- [ ] **Step 3: User docs.**
  - `docs/USER_GUIDE.md`, in the browser diagnostics section
    (`grep -n -i "browser" docs/USER_GUIDE.md`): the card, what each section
    means, Reveal in Finder, and that a page with errors is a successful
    diagnostic.
  - `docs/TROUBLESHOOTING.md`:
    - where records live (the `web_diagnostic_recorded` session events, and
      artifacts under `<data-dir>/web-diagnostics/`)
    - that a non-companion browser server gets the raw-text path and a
      minimal card
    - how to tell the two apart: the card has no problem lists

- [ ] **Step 4: Spec 22 hand-off** (`context.md` §3.5). In
`docs/specs/22_findings_model_and_panel/context.md` and its `tasks.md` Task 6,
record that:
  - the browser structure now exists as `WebDiagnosticDetails`
  - Task 6 converts `page_errors`, the `is_problem` entries in `console`
    (with `location` → `SourceRange` when it resolves inside the repository),
    and `failed_requests` into findings
  - Task 6 must not add `WebDiagnosticEntry`
  - its Implementation Notes question ("whether `WebDiagnosticReport.entries`
    was added in coordination with spec 12") now has an answer

  Change only those passages. The other session that planned #22 owns the rest
  of those files.

- [ ] **Step 5: Status records, all in this change** (`AGENTS.md` "When a spec
becomes Done"):
  - proposal `Status: Done`, and this file's header `**Started:** … — **Done:** …`
  - Every Progress row filled in.
  - `docs/specs/README.md`:
    - the #12 row becomes **Done**
    - re-derive **What to build next**: #12 was a dependency of nothing
      formally, but #22 §5.4 and #54 §5 both waited on it
  - `grep -rn "12_web_app_troubleshooting" docs/specs/*.md docs/specs/*/*.md`.
    Update every "#12 is In progress" statement: #22, #54 §5 and its
    Implementation Notes, and #51.
  - `CHANGELOG.md` `Unreleased`: remove "Finish first-class browser
    diagnostics … *(in progress)*". Do not add a release row.
  - `npm run specs:check` and read its output.

- [ ] **Step 6: The full quality gate.** All seven checks from `AGENTS.md`, not
a subset:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked
node --check crates/desktop-shell/static/app.js
npm run lint:web
typos
cargo deny check
```

Expect about 5 minutes for nextest and about 18 for a cold clippy. Record the
results in this task's Progress row. Report them to the user and leave the
change uncommitted.
