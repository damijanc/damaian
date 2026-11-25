# Spec 33 context

**Planned:** 2026-11-09. Companion to [`proposal.md`](proposal.md). The survey
below replaces the stale line references in the proposal's §2, and the decisions
D1–D10 are what [`tasks.md`](tasks.md) rest on.

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
  (`config.rs` ≈5446–5524). `mcp_server.<server>.tool.<tool>.` keys do not exist, and an unknown prefix
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
  **As built (Task 5, 2026-11-25):** `profile_permits` (in `mode.rs`, not
  `profile.rs`) no longer takes the claim at all. Per-tool approval is
  `Config::mcp_tool_requires_approval(server, tool)`: the tool's own
  `require_approval` when set for exactly that tool, otherwise the server's
  (default true); an unknown server asks. So the per-tool field is now
  `Option<bool>` (unset is not serialised, so a reload does not freeze it to
  the server's value), server-level `require_approval=false` keeps waiving the
  server's tools that have no setting of their own (unchanged for existing
  configs), and a tool may ask on a server that does not. Because the tool's
  value outranks the server's, a repository or profile that sets
  `mcp_server.<id>.require_approval=true` also turns that server's per-tool
  `false` waivers back to `true`, so the tightening reaches every tool. The
  approval card attributes a `readOnlyHint: true` to the server ("describes
  this tool as read-only … the server's own claim"), and `mcp_tool_called` /
  `mcp_tool_call_failed` carry `serverReadOnlyClaim`
  (`claimed_read_only` / `claimed_not_read_only` / `no_claim`).
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
  **As built (Task 4, 2026-11-21):** a matched tool's schema is sent from the
  round *after the match*, not only after the first call, so the model calls it
  with its real parameters rather than guessing them into an approval card. To
  keep "only the chosen server connects", the inventory (D4) now also records
  each tool's serialized `inputSchema` (`inputSchemaJson`), and a matched tool
  of a server not yet listed is offered from that record; the server starts
  only when the tool is called (or when an older record has no schema, in which
  case it is listed). Which tools were named — called, or returned by a search
  — is read back from the turn's own messages each round, so a turn resumed
  after an approval pause keeps them without a new pending-turn field. A search
  connects only servers with no inventory yet (unknown cost), which is how such
  a server becomes known; it returns at most 8 matches
  (`MCP_TOOL_SEARCH_MAX_MATCHES`), ranked by query words found in the tool
  name (weight 2) or in its server id or description (weight 1), and only
  tools the turn could offer (enabled, mode and profile under the recorded
  claim). A called tool's server is listed before the permission check, so the
  check reads the claim the server makes now. A disabled tool is never offered
  or matched, and a call naming one anyway is answered "not available" without
  connecting.
- **D10 — pinned tools count against the ceiling (decided in Task 4,
  2026-11-21).** The ceiling bounds the whole request's MCP schema cost, so
  `decide_deferral` keeps summing pinned tools with the rest: pinning narrows
  what else fits under the ceiling instead of spending past it. A pinned tool
  is always sent whatever the decision; if the pinned set alone exceeds the
  ceiling, every unpinned tool is deferred and the pinned ones still go. The
  alternative — excluding pinned tools from the sum — would let pinned cost
  plus a full ceiling's worth of direct schemas into one request, which is the
  unbounded growth requirement 2 forbids. Pinning is user/admin-only (Task 1),
  so a repository cannot use it to crowd out the direct surface.
- **D4 — inventory persistence (decided in Task 3, 2026-11-21).** The ceiling decision must not
  require connecting, or "a session that uses no MCP tool starts no server"
  fails. Persist a `McpToolInventory` per server — tools with `name`,
  `description`, input-schema byte length and estimated tokens,
  `readOnlyHint`, a content hash, and `first_seen_at` — under
  `<data_dir>/mcp/inventory/`, refreshed on any successful connect/list. The
  deferral decision reads the persisted figure; a server with no inventory is
  treated as "unknown cost" and deferred, so nothing connects until searched or
  called. As built: one file per server,
  `<data_dir>/mcp/inventory/<repository_id>/<server_id>.json`, so sessions
  refreshing different servers never overwrite each other; a missing or corrupt
  file reads as no inventory (deferred).
- **D5 — health persistence (decided 2026-11-10; corrects §5.3).** `McpServerHealth`
  is defined in terms of *this session* (`Unknown` = "never contacted this
  session", `Failed` = "not retried automatically this session"), so persisting
  it contradicts its own semantics. Keep health in memory per shell process,
  keyed by repository and server, served by a new `GET /api/mcp-health`; only
  the inventory (D4) persists. The proposal's §5.3 records this correction.
  **As built (Task 6, 2026-11-25), adjusted for spec 110 Task 1b's session
  hosts:** "per shell process" no longer describes where MCP runs. Since
  2026-11-25 each session's turns — and so its MCP contacts — run in a
  `--session-host` process of its own, while the shell only forwards and
  supervises. Health is therefore kept **wherever MCP runs**: a process-wide
  in-memory store in the engine (`workspace_engine::mcp_health`), keyed by
  repository id and server id, holding the latest `McpHealthObservation`
  (`health`, `observedAtMs`, the contacting `sessionId`). It is process-wide
  because the engine is rebuilt per request and `McpRuntime` per turn; the
  process is the narrowest thing that outlives a turn. In the desktop app a
  session host's store is exactly that session's health, which is what
  `Unknown` ("never contacted this session") and `Failed` ("not retried
  automatically this session") are defined against; the shell's own store
  holds only what the shell itself contacted (with hosting off — library
  tests, the CLI — every session runs in one process and shares it). Nothing
  is persisted. `McpRuntime` records on every contact: a successful
  connect-and-list or call → `Ready`; a timeout in any phase → `Timeout`; HTTP
  401/403 (now surfaced as `ClientError::AccessDenied`, never with the token)
  → `Unauthenticated`; a JSON-RPC error answer to a *call* (now
  `ClientError::InvalidInput`) leaves the server `Ready`, since it answered;
  anything else — spawn, network, handshake, a server that cannot list, a
  server that exited (no longer misreported as a timeout) → `Unreachable`; a
  stopped turn records nothing. `Failed` is set by Task 7 (below).
  **For Task 7's `GET /api/mcp-health`:** the shell cannot read hosts' memory,
  so the route must gather it. Add an `mcp_health` command to
  `serve_host_connection` in `crates/desktop-shell/src/session_host.rs`
  answering `workspace_engine::mcp_health_snapshot()`, ask every live session
  host for it through the same `exchange` the `status` command uses, add the
  shell's own `mcp_health_snapshot()`, fold them with
  `workspace_engine::merge_mcp_health` (latest `observedAtMs` per repository
  and server), and report every configured server absent from the result as
  `Unknown`. A host ended by archive, delete or quit takes its observations
  with it, which is correct: that session is over. Worker processes
  (`--worker`) run MCP too and keep their own store; decide whether the route
  asks them as well. `Failed` and its manual retry are per session, so they
  must act in the session's host (the offer path already reads
  `mcp_server_health` in the process that builds the offer); `/api/mcp-test`
  runs in the shell through `McpClient` directly and records nothing today.
  **Health route and `Failed` as built (Task 7, 2026-11-25):** the plan
  above was followed. `GET /api/mcp-health?repo=` gathers the shell's own
  snapshot, every live session host's and — decided here — every live
  worker's (a worker's turns run MCP in its own process, so leaving it out
  would show its servers as `unknown` while it uses them); a process that does
  not answer within the exchange timeout contributes nothing. `Failed` is a
  latch in the health store: three failed contacts in a row in one process
  (`MCP_FAILED_AFTER_CONSECUTIVE_FAILURES`; a success resets the count) make
  the server `Failed` there, and that process then neither contacts it, offers
  its tools, nor searches it, and answers a call to it with the reason before
  any approval card. Three, because a failing call usually costs two contacts
  (the listing before the permission check, then the call), so the second
  turn that tries latches it. **Retry routing:** the UI's retry is not tied to
  a session, and the shell cannot tell which host's latch the user means, so
  `/api/mcp-test` with `retry=true` sends `mcp_retry` to *every* live host and
  worker — each forgets that server's health, latch included, so its next
  turn that wants the server contacts it again — and then contacts the saved
  server once from the shell (`McpRuntime::retry_server`), whose recorded
  outcome is the latest observation the route reports. Clearing a latch in a
  session the user did not have in mind is harmless: a manual retry is the
  user saying the server is worth trying again.
  **Lazy startup as built:** Task 6 also stopped the *direct* and *pinned*
  offers from starting a server. A server not yet listed this turn has its
  tools offered from the inventory's recorded schemas (Task 4 stored them)
  whenever the record holds every wanted tool with a schema; only a call, or
  a search of a server of unknown cost, starts one. So a session that never
  searches or calls starts no server process even with a known server under
  the ceiling, and that server's health stays `Unknown` — and its inventory is
  refreshed only when it is next listed (a call, or an older record without
  schemas).
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
- **D8/D9 as built (Task 8).** The audit events keep their camelCase field
  style: `mcp_tool_called` and `mcp_tool_call_failed` carry `server`, `tool`,
  `sessionId`, `outcome`, `durationMs`, `approvalDecision`,
  `serverReadOnlyClaim`, `arguments`, and `result` (called) or `error`
  (failed); the timestamp is the audit log's own `timestampMs`. `outcome` is
  `ok`, `tool_error` (the server answered with an error result), `failed` (the
  server refused the request), `not_sent` (nothing reached the server: it
  could not be started, or is latched `Failed`), or `unknown` (sent, no answer:
  a timeout, a stop, or a server that exited mid-call). `approvalDecision` is
  `not_required` (waived in config) or `approved` (the card); a call the user
  declines is recorded as a third event, `mcp_tool_call_declined`
  (`approvalDecision`/`outcome` `declined`, redacted arguments), written
  straight to the audit log because building a runtime would resolve the
  server's token. Arguments, results and errors are redacted *then* bounded to
  `MCP_HISTORY_EXCERPT_CHARS` (2,000) characters, so a cut never splits a
  secret into an unrecognisable fragment; `AuditLog::record` redacts again.
  Descriptions and schemas are redacted at two points: once when a listing
  arrives (`McpRuntime::list_now`, so the cached listing, the inventory on
  disk and search results never hold the secret), and again in
  `McpTool::to_tool_definition`, which now requires the scanner, so a schema
  served from an inventory recorded before this change is scanned too. A
  schema is redacted string by string inside its parsed JSON (keys included)
  so it stays valid JSON; text that is not JSON is redacted as text.

## Open items not yet decided

None. D2's default, D3, D5 and D6 were confirmed by the owner as written on
2026-11-10, and D1 was already decided. D4 (inventory persistence) was firmed up in
Task 3 as recorded above. Task 4 refined D3 (a match's schema is sent from
the next round, from the inventory) and added D10 (pinned tools count against
the ceiling); both are open to the owner's review, as recorded above.
