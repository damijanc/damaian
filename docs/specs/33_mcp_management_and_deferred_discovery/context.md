# Spec 33 context

**Planned:** 2026-11-09. Companion to [`proposal.md`](proposal.md). The survey
below replaces the stale line references in the proposal's §2, and the decisions
D1–D9 are what [`tasks.md`](tasks.md) rest on.

## Survey: what exists today

`crates/workspace-engine/src/mcp.rs` (946 lines) is the spec 06 runtime this spec
manages. `crates/workspace-engine/src/chat.rs` (23,166 lines) is the orchestrator.

- **Process lifetime is already delivered** (requirement 8 is not greenfield).
  `StdioTransport::spawn` registers the child in the `ProcessRegistry` with
  `ProcessKind::McpServer`, the session id, and `child.id()`; it sets
  `process_group(0)` and the `Drop` guard kills and waits the child. The
  proposal's §2 claim of "no kill-on-drop guard (`mcp.rs:323`)" is stale.
  `tests/foundation.rs` (≈6220–6270) already has a fake stdio MCP server
  harness — an `sh` script answering `initialize`/`tools/list`/`tools/call` —
  which the "mock server claiming read-only on a write tool" acceptance test
  should reuse.
- **Timeouts exist but are two hardcoded constants.** `CONNECT_TIMEOUT` (15 s)
  is also used for `tools/list` and the readiness notify; `CALL_TIMEOUT` (60 s)
  covers `tools/call`. There is no startup/list/call split and no config key. A
  timed-out call returns `ClientError::Io("Timed out waiting for MCP server
  response")`, which `chat.rs` already maps to a tool error the model sees
  (`"MCP tool call failed: …"`, `ActionOutcome::Failed`) — but it is not
  recorded as an unknown outcome, is not marked non-retryable, and carries no
  duration.
- **`McpTool` already carries the read-only claim as `Option<bool>`**
  (`mcp.rs` ≈104–120): `name`, `description`, `input_schema_json`, and
  `read_only_hint` from `annotations.readOnlyHint`, documented as "a hint, not a
  guarantee". `to_tool_definition` namespaces to `mcp__<server>__<tool>`.
- **The read-only claim currently widens two layers, not just display.**
  `mode_permits` (`mode.rs` ≈199) allows an MCP call in Ask/Review when the hint
  is `Some(true)`; `profile_permits` (`profile.rs`, capability
  `allow_mutating_mcp_tools` at `profile.rs:175`, label at
  `effective_policy.rs:173`) refuses only when the capability is false *and* the
  hint is not `Some(true)`. D1 changes the profile half only.
- **Audit events already exist** (`McpRuntime::record`, `mcp.rs` ≈698):
  `mcp_tools_listed`, `mcp_discovery_failed`, `mcp_tool_called`,
  `mcp_tool_call_failed`. None carries arguments, result, duration, or an
  approval decision, and `mcp_tool_call_failed` covers failure but not the
  unknown-outcome classification.
- **Token resolution and redaction already exist in part.**
  `ChatOrchestrator::mcp_token_resolver` / `set_mcp_token_resolver` resolve
  `keychain:` or env-var references per active server at runtime build time;
  `build_mcp_runtime(session_id)` (`chat.rs` ≈1313) resolves the token for each
  server and builds `McpRuntime::disabled()` when none resolve. Tool *results*
  pass through `scanner.redact(&result.text)` before the model sees them, but
  tool *descriptions and schemas* are not scanned before they become
  `ToolDefinition`s.
- **The tool list is built once per turn** (`chat.rs` ≈9237): native tools plus
  `mcp.tool_definitions()` for every active server, filtered per tool by
  `permits(mode, &capabilities, &ToolAction::McpCall{…}, None, hint)` with
  `hint = mcp.tool_read_only_hint`. All schemas of every active server go into
  every request; deferral does not exist.
- **Approval is per server today.** `chat.rs` ≈11929 pauses on
  `mcp.requires_approval(&server_id)`, proposing a `PendingMcpCall` with
  `server_id`, `tool_name`, `arguments_json`; the resume path (≈5273) rebuilds
  the runtime, re-lists tools, re-checks approval and `tool_read_only_hint`,
  and calls. There is no per-tool approval decision, and the read-only claim
  plays no part in the approval prompt.
- **Config today.** `McpServerConfig` (`config.rs` ≈153–175) has
  `enabled`/`require_approval` per server. `active_mcp_servers` (`config.rs`
  ≈2681) is the three-way intersection (global `mcp_enabled`, server `enabled`,
  allowlist). `upsert_mcp_server_from_repository` (`config.rs` ≈2813) narrows
  `mcp_server.<id>.enabled`/`require_approval` at repository scope and records
  `mcp_server.<id>.<field>` in `OverlayOutcome`. `Config::set` dispatches the
  `mcp_server.` prefix (`config.rs` ≈3949) and `to_policy_text` serialises it
  (`config.rs` ≈5446–5524). `mcp_tool.` keys do not exist, and an unknown prefix
  in an untrusted scope is reported as `Unparsable` (`config.rs` ≈3920) — the
  exhaustive classification in AGENTS.md must be extended for every new key.
- **UI.** Only `/api/mcp-test` exists (`crates/desktop-shell/src/lib.rs`), a
  manual connection test. No servers/tools panel, health route, or token-cost
  figure.

## Design decisions

- **D1 — read-only claim vs profile (decided by the owner).** Stop the claim
  widening the **profile** layer only: `profile_permits` refuses an MCP call
  when `allow_mutating_mcp_tools` is false, regardless of the hint. Mode
  widening (`mode.rs` ≈199) stays, so Ask/Review still *offer* read-only-claimed
  tools; offering is not approval. This changes `profile.rs` and its tests, plus
  `chat.rs:21946`'s narrowed-profile expectation.
- **D2 — deferral ceiling (decided 2026-11-10).** New config key
  `mcp_deferral_ceiling_tokens` (estimated tokens, `usize`), default **8,000**
  so one or two servers with a handful of tools stay direct. Repository and
  profile scopes may lower it, never raise it (raising spends context budget;
  lowering is the safe direction). The default is a placeholder until §7's
  measurement; the measured figures replace it at close-out.
- **D3 — how stage 2 fetches schemas (decided 2026-11-10).** No separate fetch tool.
  `mcp_tool_search` returns `{ name, server, one-line description }` matches;
  when the model then calls a matched tool by its `mcp__<server>__<tool>` name,
  the orchestrator connects and lists that server on demand, executes the call,
  and adds the tool's schema to the turn's tool list from that round onward.
  This is the spec's "second step", costs no extra round trip, and only
  connects the server whose tool was actually chosen.
- **D4 — inventory persistence (recommended).** The ceiling decision must not
  require connecting, or "a session that uses no MCP tool starts no server"
  fails. Persist a `McpToolInventory` per server — tools with `name`,
  `description`, input-schema byte length and estimated tokens,
  `readOnlyHint`, a content hash, and `first_seen_at` — under
  `<data_dir>/mcp/inventory/`, refreshed on any successful connect/list. The
  deferral decision reads the persisted figure; a server with no inventory is
  treated as "unknown cost" and deferred, so nothing connects until searched or
  called.
- **D5 — health persistence (decided 2026-11-10; corrects §5.3).** `McpServerHealth`
  is defined in terms of *this session* (`Unknown` = "never contacted this
  session", `Failed` = "not retried automatically this session"), so persisting
  it contradicts its own semantics. Keep health in memory per shell process,
  keyed by repository and server, served by a new `GET /api/mcp-health`; only
  the inventory (D4) persists. The proposal's §5.3 records this correction.
- **D6 — timeout config (decided 2026-11-10).** Three keys,
  `mcp_connect_timeout_ms` (default 15,000), `mcp_list_timeout_ms` (default
  15,000), `mcp_call_timeout_ms` (default 60,000), replacing the two consts.
  User and admin scopes only; repository config is forbidden from setting them
  (they control process and network behaviour, not a narrowing the repository
  may impose). This preserves the current defaults.
- **D7 — token estimate.** Reuse spec 26's estimator on `description` +
  `input_schema_json` (structured rate 2.4 B/token), the same measurement the
  context budget trusts; store bytes and tokens in the inventory (D4).
- **D8 — audit.** Extend the existing `mcp_*` events rather than a parallel
  log: `mcp_tool_called`/`mcp_tool_call_failed` gain `duration_ms`, `outcome`
  (including `unknown` on timeout), `approval_decision`, redacted bounded
  `arguments`/`result`, and the attributed read-only claim.
- **D9 — redaction.** Scan a server's tool descriptions and schemas through
  `SecretScanner` before they become `ToolDefinition`s, closing the
  description-embeds-a-secret hole (proposal §5.6). Results are already
  redacted; extend the same path to the bounded invocation-history copy.

## Open items not yet decided

None. D2's default, D3, D5 and D6 were confirmed by the owner as written on
2026-11-10, and D1 was already decided. D4 (inventory persistence) remains a
recommendation to firm up in Task 3, not an open security or behaviour decision.
