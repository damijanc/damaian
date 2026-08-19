# Feature Spec: First-Class Web App Troubleshooting

Status: Done
Order: 12 of 12
Related spec sections: `ai_coding_assistant_specification.md` section 7.5
(tool/function calling), section 7.6 (tool and action orchestrator), section 11
(error handling), and `docs/specs/06_mcp_support.md`,
`docs/specs/08_stop_and_progress.md`, `docs/specs/10_persistent_command_approval.md`.

## 1. Motivation

A real troubleshooting session against `/Users/damijancavar/development/snake-game`
exposed a gap in Damaian's support for web applications. The user reported that
the Register button did nothing. The agent checked files and HTTP endpoints, and
the APIs were healthy, but the actual failure was a browser runtime error:

```text
Cannot access 'game' before initialization
```

A single browser diagnostic run with page-error capture found the issue. Damaian
did not guide the agent there directly. The session instead spent tool rounds on
`curl`, repeated approval prompts, and a generic Playwright MCP server whose
result included screenshot file paths but not model-usable evidence. The MCP
server did open the page and save screenshots, but the model could not inspect
those images through Damaian.

For web apps, "the server returns 200" is often not enough. The assistant needs
first-class access to the browser facts that users actually observe: page errors,
console logs, failed network requests, rendered DOM state, screenshots, and the
result of simple interactions such as filling a form and clicking a button.

## 2. Current State

- The built-in model tools are repository-oriented: shell command proposals,
  patch proposals, file reads, code search, Git status, and Git diff
  (`crates/workspace-engine/src/chat.rs`).
- MCP tools are appended to the same native tool list when configured
  (`docs/specs/06_mcp_support.md`), but Damaian treats them as generic text
  tools. It does not understand browser artifacts, screenshots, console events,
  or interaction traces.
- MCP image content is flattened to placeholder text such as
  `[image content omitted]` (`crates/workspace-engine/src/mcp.rs`). Artifact
  paths from a tool result remain paths; they are not fed back as visual input.
- Commands with shell control syntax are always approval-gated and cannot be
  permanently allowlisted (`crates/workspace-engine/src/command_policy.rs`).
  That is correct for safety, but it makes browser debugging noisy when every
  diagnostic is expressed as chained shell commands.
- A turn has a hard-coded `MAX_TOOL_ROUNDS` of 6. When that limit is reached,
  Damaian removes tools from the next model request. If the model still emits
  tool-call-shaped text, the turn can look complete even though the assistant is
  still trying to gather evidence.
- The secret scanner can redact source code that reads password form values,
  because assignment-shaped detectors do not distinguish hardcoded credentials
  from ordinary runtime variables.

## 3. Requirements

1. Damaian offers first-class browser troubleshooting tools when a browser
   diagnostic runner is available.
2. The first-class tool contract is stable and Damaian-owned. The model should
   not have to know the action names of an arbitrary MCP server to inspect a web
   page.
3. A page inspection returns text evidence in the same turn: final URL, title,
   HTTP status if known, page errors, console errors and warnings, failed
   requests, selected DOM/accessibility state, and screenshot metadata.
4. An interaction scenario can navigate, fill inputs, click buttons, choose
   select values, submit forms, press keys, wait for selectors or text, and
   capture the resulting browser evidence.
5. Scenario actions are schema-constrained with explicit enums so the model
   cannot invent action names such as `wait_for_timeout` when only `wait` is
   supported.
6. Browser artifacts are stored under the Damaian data directory, associated
   with the session and task, and visible from the desktop UI.
7. The model receives enough text evidence to reason without image support.
   Vision-capable model input is optional and must not be required for phase 1.
8. All captured text and metadata pass through the existing secret scanner
   before entering the model transcript, session log, or audit log.
9. Local-first safety remains intact. Navigation to local URLs may be low risk;
   navigation to remote URLs, form submission, authenticated flows, and
   state-changing clicks require explicit user approval unless a scoped
   diagnostic approval has been granted.
10. The default tool-round cap becomes configurable. Web troubleshooting gets a
    larger but bounded budget than ordinary code Q&A.
11. Repeated failed calls to the same browser tool with substantially similar
    arguments are detected and fed back as a "change approach" instruction
    rather than consuming the whole turn.
12. Hitting the tool-round cap is not reported as an ordinary successful answer
    when the model is still trying to call tools.

## 4. Non-goals

- Building a general browser automation product.
- Shipping Node.js or a bundled Playwright runtime inside the macOS app.
- Bypassing browser security, CORS, authentication, cookie policy, or user
  consent.
- Replacing MCP. MCP remains useful for user-provided tools; this feature adds a
  Damaian-owned browser diagnostic contract above any particular driver.
- Guaranteeing visual understanding from screenshots in phase 1. Screenshots
  are artifacts and UI evidence first; text diagnostics carry the model loop.
- Remote hosted browser execution.

## 5. Design

### 5.1 Browser Diagnostic Contract

Add two built-in tools to the agentic loop when a diagnostic runner is available:

```text
inspect_web_page
run_web_scenario
```

`inspect_web_page` accepts:

```json
{
  "url": "http://localhost:5001/",
  "viewport": { "width": 1280, "height": 720 },
  "wait_ms": 1000,
  "capture": {
    "screenshot": true,
    "dom": true,
    "accessibility": true,
    "network": true,
    "console": true
  }
}
```

`run_web_scenario` accepts:

```json
{
  "url": "http://localhost:5001/",
  "viewport": { "width": 1280, "height": 720 },
  "actions": [
    { "action": "fill", "selector": "#username", "value": "tester" },
    { "action": "fill", "selector": "#password", "value": "testpass123" },
    { "action": "click", "selector": "#register-btn" },
    { "action": "wait", "ms": 1000 }
  ],
  "capture": {
    "screenshot": true,
    "dom": true,
    "network": true,
    "console": true
  }
}
```

Allowed action values are:

- `goto`
- `fill`
- `click`
- `press`
- `select`
- `submit`
- `wait`
- `wait_for_selector`
- `expect_text`
- `expect_selector`
- `screenshot`

The tool result is a `WebDiagnosticReport` serialized to text for the model and
structured JSON for the UI:

```json
{
  "url": "http://localhost:5001/",
  "final_url": "http://localhost:5001/",
  "title": "Snake Game",
  "status": 200,
  "page_errors": ["Cannot access 'game' before initialization"],
  "console": [],
  "failed_requests": [],
  "dom_summary": {
    "forms": 1,
    "buttons": ["Log in", "Register"],
    "status_text": "",
    "visible_text_excerpt": "Snake Log in Register Score: 0 ..."
  },
  "artifacts": [
    {
      "kind": "screenshot",
      "path": "web-diagnostics/session-id/task-id/run-id/page.png",
      "width": 1280,
      "height": 720
    }
  ]
}
```

The text form starts with the highest-signal facts, for example:

```text
Browser diagnostic failed: 1 page error.
- pageerror: Cannot access 'game' before initialization
- URL: http://localhost:5001/
- Title: Snake Game
- Visible buttons: Log in, Register
- Screenshot: web-diagnostics/.../page.png
```

### 5.2 Runner Architecture

Keep `workspace-engine` browser-driver agnostic. Add a trait similar in spirit
to the existing injected model adapter and MCP token resolver:

```rust
pub trait WebDiagnosticsRunner {
    fn inspect(&self, request: WebInspectRequest) -> Result<WebDiagnosticReport>;
    fn run_scenario(&self, request: WebScenarioRequest) -> Result<WebDiagnosticReport>;
}
```

`ChatOrchestrator` exposes `inspect_web_page` and `run_web_scenario` only when a
runner is configured. The CLI and tests use a no-op runner by default.

Phase 1 runner: an external browser-probe subprocess with a small Damaian-owned
JSON protocol over stdio. This avoids shipping Node.js or Playwright in the app,
but gives Damaian a stable result schema and tool descriptions. A user's
Playwright helper, Python script, or future native runner can implement this
protocol.

Phase 2 runner: a native macOS/WebKit implementation in `desktop-app`, if the
phase 1 protocol proves too dependent on external setup. The native wrapper is
the right process to own real webviews; `workspace-engine` should not learn
about Tauri or WebKit.

### 5.3 Artifact Storage and UI

Store browser artifacts under:

```text
<data-dir>/web-diagnostics/<session-id>/<task-id>/<run-id>/
```

Artifact records include kind, relative path, MIME type, dimensions when known,
and redacted captions or summaries. The session log stores references, not large
binary blobs. The desktop UI renders a diagnostic card with:

- page status summary
- page errors and console errors
- failed request list
- DOM summary
- screenshot thumbnails with "Reveal in Finder"

The model transcript receives the redacted text report. It does not receive raw
binary data in phase 1.

### 5.4 Approval and Policy

Classify diagnostics by risk:

- Low risk: local URL inspection with no interactions, limited to
  `localhost`, `127.0.0.1`, and `[::1]`.
- Medium risk: local interaction scenarios that fill or click controls but do
  not submit known destructive actions.
- High risk: remote URLs, authenticated sessions, arbitrary form submission,
  downloads, file uploads, or actions outside loopback.

Add a session-scoped approval option for browser diagnostics:

```text
Allow browser diagnostics for this session
```

This is not a global allowlist. It expires with the session and is recorded in
the audit log.

### 5.5 Tool Round Policy

Replace the hard-coded limit with config values:

```conf
agent_max_tool_rounds=8
agent_web_debug_max_tool_rounds=12
agent_tool_retry_limit=2
```

Ordinary turns use `agent_max_tool_rounds`. A turn enters web debug mode when:

- the model calls `inspect_web_page` or `run_web_scenario`,
- the user supplies a URL and asks why a web page is broken, or
- recent command output contains a local web URL and the user reports a UI
  symptom.

The absolute cap is 16 rounds, reachable only through an explicit UI action:

```text
Continue debugging
```

When the cap is reached, the task status becomes `needs_user_decision` or
`tool_budget_exhausted`, not `complete`, if the final model content contains
tool-call-shaped markup or an unresolved diagnostic request.

### 5.6 MCP Interop

MCP remains supported, but Damaian should not expose a browser MCP tool's raw
schema directly when the user wants normal web debugging. Instead:

- the Damaian browser probe protocol can be implemented by a Playwright MCP
  project or a thin wrapper around it;
- Damaian maps the stable first-class browser tools to that runner;
- generic MCP tools remain available for advanced workflows.

This keeps the assistant prompt simple and prevents wasted rounds from invalid
MCP-specific action names.

### 5.7 Secret Redaction Follow-up

Refine assignment-shaped redaction so ordinary source-code reads like
`const password = passwordInput.value;` are not redacted as hardcoded secrets.
The scanner should continue to redact string literals, environment values,
tokens, private keys, and database passwords. A value derived from a variable,
property access, function call, or DOM input should be treated as code, not a
secret value.

## 6. Acceptance Criteria

1. With a configured browser runner, a user can ask "why does this local page
   not work?" and Damaian can inspect `http://localhost:<port>/` without
   repeated shell-command approvals.
2. A page with a JavaScript module error produces a model-visible diagnostic
   containing the page error text in the same turn.
3. A scenario that fills two inputs and clicks a button returns page errors,
   console errors, failed requests, visible text, and a screenshot artifact.
4. The model cannot request unsupported scenario actions; invalid actions are
   rejected by schema validation before execution.
5. Screenshot artifacts appear in the session UI and are stored under
   `<data-dir>/web-diagnostics/...`.
6. Remote URL diagnostics require approval and show the target origin before
   navigation.
7. Captured console text, DOM text, network metadata, and artifact captions are
   secret-redacted before persistence and before re-entering the model
   transcript.
8. `agent_max_tool_rounds=8` and `agent_web_debug_max_tool_rounds=12` are
   honored by the orchestrator.
9. Three near-identical failing browser tool calls stop with a "change approach"
   tool result instead of consuming more rounds.
10. A turn that ends because tools were removed at the round cap is not marked
    as ordinary `complete` when the model's final answer is raw tool-call
    markup.
11. Existing MCP behavior continues to work for non-browser tools.

## 7. Suggested Phasing

1. **Spec and config:** add tool-round config values, task status for exhausted
   tool budget, and the browser diagnostic data structures.
2. **Runner trait and fake runner tests:** wire first-class tool definitions
   into `ChatOrchestrator` behind an injected runner; verify model-visible
   reports and approval decisions without launching a browser.
3. **External browser-probe runner:** implement the Damaian JSON-over-stdio
   protocol and artifact storage. A Playwright helper can satisfy this without
   shipping Node.js.
4. **Desktop UI cards:** render browser diagnostic reports and screenshot
   artifacts in sessions.
5. **Retry and cap hardening:** configurable round limits, per-tool retry
   detection, and non-success terminal state for exhausted tool budgets.
6. **Secret scanner refinement:** stop redacting non-literal runtime assignments
   while preserving hardcoded secret detection.
7. **Optional native runner:** evaluate a macOS/WebKit implementation in
   `desktop-app` once the external-runner path proves the product shape.

## 8. Implementation Notes

The first implementation slice adds the stable engine-owned tools
`inspect_web_page` and `run_web_scenario`, exposed only when the desktop shell
can configure a browser diagnostics runner. The shell runner delegates to an
active Playwright/browser-like MCP server and maps the stable Damaian contract to
compatible MCP tools such as `inspect_page`, `inspect_web_page`,
`run_web_scenario`, or `run_scenario`.

Implemented:

- configurable `agent_max_tool_rounds`, `agent_web_debug_max_tool_rounds`, and
  `agent_tool_retry_limit`;
- `tool_budget_exhausted` task status when the model still requests tools at the
  configured cap;
- first-class browser diagnostic tool schemas with constrained scenario action
  enums;
- low-risk automatic local page inspection and approval-gated scenarios or
  remote inspections;
- retry-limit feedback for repeated failed browser diagnostics;
- best-effort screenshot artifact materialization under
  `<data-dir>/web-diagnostics/<session-id>/<task-id>/<run-id>/`;
- an authenticated artifact read endpoint and lightweight session UI thumbnails
  for Damaian-relative diagnostic image paths;
- redaction of diagnostic text before it re-enters the transcript;
- assignment scanner refinement so runtime source expressions such as
  `const password = passwordInput.value;` are not treated as hardcoded secrets.
- a session-scoped "Allow browser diagnostics for this session" approval option,
  recorded as a session event and audit-logged as a browser diagnostic approval
  decision;
- an explicit "Continue debugging" UI affordance for `tool_budget_exhausted`
  tasks, which starts a new turn in the same session with a one-turn budget up
  to the absolute 16-round cap;
- target-origin display in browser diagnostic approval prompts.

The native macOS/WebKit runner remains an optional future phase, not a blocker
for this spec. Phase 1 is intentionally satisfied by the stable
Damaian-owned diagnostic contract backed by an external MCP/browser runner, so
the app still does not ship Node.js or a bundled Playwright runtime.

## 9. Prompt for the Companion Playwright MCP Work

Use this prompt in the Playwright MCP repository to improve the browser helper
that can later back Damaian's first-class browser diagnostics:

```text
You are working on my local Playwright MCP server. Improve it for coding-agent
web-app troubleshooting, using the recent Damaian snake-game failure as the
reference case.

Observed failure:
- User reported: "Register button does nothing."
- HTTP endpoints returned 200/201 and looked healthy.
- Browser page rendered, but JavaScript module execution failed with:
  "Cannot access 'game' before initialization"
- The existing MCP server saved screenshots, but the agent could not inspect
  them through Damaian.
- The model wasted rounds by guessing unsupported actions: wait_for_timeout and
  evaluate.

Goals:
1. Make run_scenario self-describing and schema-constrained. The tool schema
   must expose the exact allowed action enum: goto, click, fill, press, select,
   submit, wait, wait_for_selector, expect_text, expect_selector, screenshot.
2. Add page diagnostics to every scenario result: final URL, title, page errors,
   console messages, failed requests, selected visible text, form/button summary,
   and artifact metadata.
3. Add a direct inspect_page tool for one-shot diagnostics of a URL with no
   interaction.
4. Add support for evaluate only if it is safe and deliberately scoped. If not,
   document that it is unsupported and provide DOM summary alternatives.
5. Fix screenshot naming so a screenshot action without ".png" still writes a
   valid PNG path and does not fail with "Unsupported screenshot mime type".
6. Return screenshot artifact paths plus dimensions and a short textual caption
   or DOM/visible-text summary so a text-only host can still reason.
7. Capture page.on("pageerror"), console events, request failures, and relevant
   response failures.
8. Keep secrets out of logs. Redact password field values and obvious tokens in
   returned diagnostics.
9. Add tests or a small demo page proving that a runtime JS error is reported in
   the tool result.
10. Update README examples to show: inspect a local page, fill a login/register
    form, click a button, wait, and collect diagnostics.

Implementation constraints:
- Keep the MCP server stdio-compatible.
- Do not require Damaian-specific APIs, but make the output easy for Damaian to
  map into a first-class WebDiagnosticReport later.
- Prefer one diagnostic-rich tool call over many small tool calls.
- Do not break existing tools unless replacing them with a clearer compatible
  schema.
```
