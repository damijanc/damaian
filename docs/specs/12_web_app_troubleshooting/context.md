# First-Class Web App Troubleshooting — Context

Background for [`proposal.md`](proposal.md), written on 2026-09-25 when the
close-out was planned. It compares the spec with the code and with the
companion MCP server that serves as the runner. The proposal says what the
design was meant to be. This file says what exists, what is missing, and the
decisions [`tasks.md`](tasks.md) is built on.

## 1. What the runner actually is

§5.2 plans a Damaian-owned JSON protocol over stdio. The shipped runner uses a
different route: an existing browser MCP server. `McpBrowserDiagnosticsRunner`
(`crates/desktop-shell/src/lib.rs`, `impl WebDiagnosticsRunner`) connects to
every active MCP server that `looks_like_browser_diagnostics_server` accepts.
It then calls the first tool that matches `inspect_page`/`inspect_web_page` or
`run_web_scenario`/`run_scenario`.

§8 already says this. The consequence it leaves out is that **the result
contract is whatever that MCP server returns**. Damaian defines none. On this
machine the server is the user's own companion,
`~/development/mcp/playwright-mcp/server.py`, configured in `user.conf` as
`mcp_server.playwright-mcp`. It is a Python FastMCP server. §9 was the prompt
that produced it, and it met that prompt.

### 1.1 What the companion returns

Every `inspect_page` and `run_scenario` result is one JSON object, built by
`_build_report` and redacted by `redact_data`. It reaches Damaian as MCP text
content. `McpToolResult` is `{ text, is_error }` (`mcp.rs`), and the object is
the whole of `text`.

| Field | Shape |
|---|---|
| `success`, `diagnostic_ok` | bool |
| `run_id`, `url`, `final_url`, `title` | string |
| `status` | int or null |
| `page_errors` | `[string]` |
| `console` | `[{type, text, location?}]`, where `location` is `{url, lineNumber, columnNumber}` as Playwright reports it |
| `console_errors`, `console_warnings` | subsets of `console` |
| `failed_requests` | `[{kind: "requestfailed", url, method, resource_type, failure}]` or `[{kind: "response", url, method, resource_type, status, status_text}]` for status ≥ 400 |
| `dom_summary` | `{forms, buttons: [string], fields: [{tag,type,id,name,placeholder,label,required}], status_text, visible_text_excerpt}`, or `{error}` |
| `artifacts` | `[string]`: absolute paths |
| `artifact_metadata` | `[{kind, path, mime_type, width, height, caption}]` |
| `results` | scenario only: `[{step, action, success, selector?, url?, ms?, text?, artifact?, error?}]` |
| `error`, `message` | present when the call itself failed, not the page |
| `text_report` | the companion's own prose summary |

Two points the tasks depend on:

- **A page error is not a tool failure.** A page that throws still returns an
  ordinary result with `page_errors` filled and no `error` field. MCP
  `is_error` is false. The tool failed only when `is_error` is true, or when
  the object has `"error": true`.
- **Artifact dimensions are in `artifact_metadata`, not `artifacts`.**
  `extract_artifacts_from_text` reads only `artifacts`, which is a list of
  bare strings. So every artifact from this server arrives with `kind:
  "artifact"` and no width or height.

## 2. What §5.1 and §5.3 promised and what exists

| Promised | State |
|---|---|
| §5.1: `WebDiagnosticReport` carries `final_url`, `title`, `status`, `page_errors`, `console`, `failed_requests`, `dom_summary`, `artifacts` | **Not built.** `WebDiagnosticReport` is `{ text, artifacts, is_error }` (`web_diagnostics.rs`). Every field the companion sends except `artifacts` is dropped as structure. |
| §5.1: the model's text starts with the highest-signal facts | **Not built.** The runner puts `"Browser diagnostic result via MCP server `…` tool `…`:\n"` in front of the raw JSON. `format_web_diagnostic_result` (`chat.rs`) redacts it and appends an artifact list. The model reads a JSON dump with the page errors somewhere in the middle. |
| §5.3: a card with status summary, page and console errors, failed requests, DOM summary, and thumbnails with "Reveal in Finder" | **Thumbnails only.** `appendWebDiagnosticArtifacts` (`app.js`) finds `web-diagnostics/…png` paths in message text with a regex and loads each one through `GET /api/web-diagnostic-artifact`. There is no card and no reveal. `POST /api/reveal-in-finder` opens a repository root, not a file. |
| §8: a real interaction-scenario check against the configured MCP server | **Not done.** |
| Req. 8: captured text is redacted before the session log | **Partly met.** The model-facing text is redacted in `format_web_diagnostic_result`, and the tool message stores that redacted text. Nothing structured is persisted, so there is nothing else to redact yet. `SessionStore` is deliberately unredacted (spec 17 §5.4), so structured records added by this close-out must be redacted before they are appended. |

Everything else in §8's "Implemented" list was checked and holds: the round
config, `tool_budget_exhausted`, the action enum (`WEB_SCENARIO_ACTIONS`),
loopback risk, retry feedback, artifact materialisation, the session-scoped
approval, "Continue debugging", and target-origin display.

## 3. Decisions

### 3.1 Damaian parses the companion's shape, tolerantly

Requirement 2 makes the contract "Damaian-owned". §1 above shows the
companion already emits a stable shape that answers §5.1 almost field for
field. So Damaian adopts that shape as its contract. `WebDiagnosticDetails`
(Task 1) is that shape as typed Rust, with every field optional or defaulted.
If a field is missing, it is empty. If a field has the wrong type, it is
dropped with the rest of the report kept. If the text is not a JSON object,
`details` is `None` and behaviour is exactly today's.

This is what keeps a different browser MCP server working. It still gets the
old raw-text path, and never gets a worse one.

The companion's `text_report` is **not** used. Damaian renders its own header
from the typed fields (Task 2). That header is what requirement 2 means by
Damaian owning the contract, and it lets the tests pin the header without
depending on the companion's wording.

### 3.2 "Failed" means the tool failed, not the page

`browser_tool_result_failed` (`chat.rs`) decides retry counting (req. 11) by
checking whether the content *text* starts with "Browser diagnostic failed".
§5.1's example text is "Browser diagnostic failed: 1 page error.", and the
fixture in `tests/foundation.rs` copies it. Under that wording a page that
throws would count as a failed call. Three inspections of a broken page would
then trip the retry limit, which is the page doing what the user said it does,
not the model repeating a broken call.

So:

- `WebDiagnosticReport::tool_failed()` is true when `is_error` is true or the
  companion object has `"error": true`. Retry counting uses it, not the
  rendered text.
- A diagnostic that ran and found problems is headed `Browser diagnostic found
  1 page error, 2 console errors.` A tool failure is headed `Browser diagnostic
  failed: …`. The second wording keeps today's meaning, so
  `browser_tool_result_failed` stays correct for the error strings that don't
  come from a report (`run_web_diagnostic_report`'s `Err` arm, MCP connection
  failures).

This corrects §5.1's example text. Task 2 updates §5.1.

### 3.3 The card needs a structured record, persisted and streamed

What the UI has today is the tool message's text. The card needs the typed
report, and it needs it after a reload too. The pattern is already there in
spec 21's plan panel:

- Live, through a `TurnProgress` variant. `desktop-shell` maps it to an SSE
  event.
- On reload, through a session-log event replayed into `/api/session`'s
  `tasks[]` and joined by `taskId`.

Task 3 adds `TurnProgress::WebDiagnostic(WebDiagnosticRecord)`, the session
event `web_diagnostic_recorded`, and `tasks[].webDiagnostics`. The record
holds the redacted `WebDiagnosticReport` plus the tool name, URL and a
`recordedAtMs`. It is redacted before it is appended, per §2's last row.
Replay uses `active_events`, the same as plans, so a rewound turn's
diagnostics go with it.

The legacy regex thumbnails stay only for messages in sessions that have no
recorded diagnostics. Old sessions keep what they show today, and a new
session doesn't show every screenshot twice.

### 3.4 Reveal in Finder uses the existing artifact path check

`web_diagnostic_artifact_path` (`lib.rs`) already restricts
`GET /api/web-diagnostic-artifact` to canonical paths under
`<data-dir>/web-diagnostics/`. The new `POST /api/reveal-web-diagnostic-artifact`
reuses that check and runs `open -R <path>`. Nothing outside the data
directory can be revealed through it. Its end-to-end test opens Finder, so it
is `#[ignore]` with a run line, like
`reveal_in_finder_endpoint_opens_the_requested_repository_root`. The path
validation gets an ordinary test.

### 3.5 This is what spec 22 consumes

Spec 22 §5.4 planned to add `WebDiagnosticEntry` to this report itself. Its
Task 6, planned the same day before this close-out, does exactly that. Once
this spec is done, `WebDiagnosticDetails` already holds each console entry
(with its source location), each page error, and each failed request as
typed values. Spec 22's Task 6 then only needs to convert these into
`Finding`s. It should not add a second browser structure. Task 5 here records
that in spec 22's `context.md`.

## 4. Test seams

- `web_diagnostics.rs` has **no** unit tests today. Task 1 adds the first
  inline `#[cfg(test)] mod tests`.
- The engine fakes are `StaticWebRunner` and `CountingWebRunner` in
  `chat.rs`'s test module, and `StaticWebDiagnosticsRunner` in
  `tests/foundation.rs`. All three build reports with
  `WebDiagnosticReport::from_text`, so a companion-shaped JSON fixture goes
  through the same parse as the real runner.
- The ignored harness `serves_the_ui_for_manual_inspection`
  (`desktop-shell/src/lib.rs`) serves the real UI on port 4899 with seeded
  sessions. Task 4 seeds a recorded diagnostic there for browser checks.
- There is no mock-model path in the desktop shell
  (`DAMAIAN_MOCK_MODEL_RESPONSE` is CLI-only; spec 20 Task 8 recorded this).
  So the live check in Task 5 uses a real provider. It has to be run by hand.
