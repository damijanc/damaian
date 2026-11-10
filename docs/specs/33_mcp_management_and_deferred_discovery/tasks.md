# Spec 33 tasks

**Started:** 2026-11-09

Implementation plan for [`proposal.md`](proposal.md). The design decisions and
the code survey they rest on are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md). Each task below is a fresh session; do not
  carry conversation history between them.
- `chat.rs` is the serialisation constraint: only one task edits it at a time.
  The task order below is chosen so `chat.rs` tasks are spread apart and
  sequential; do not reorder them.
- Repository configuration is untrusted: every new config key gets an explicit
  overlay classification (user/admin/repository/profile scope; restrict-only
  where it can only narrow). Never add a wildcard that silently accepts new
  keys.
- Do not weaken command policy, secret redaction, patch safety, the
  repository-config trust boundary, or the question/approval separation.
- Scoped checks per task; the workspace clippy and test suite stay with CI under
  AGENTS.md. Report which scoped checks ran and passed.

| Task | State | Durable result |
|---|---|---|
| 1 · Per-tool and MCP-management config surface | Not started | — |
| 2 · Bounded, independently configurable timeouts (defines `McpServerHealth`) | Not started | — |
| 3 · Tool inventory, token measurement, deferral ceiling | Not started | — |
| 4 · Deferred discovery surface and `mcp_tool_search` | Not started | — |
| 5 · Per-tool approval and read-only discipline (D1) | Not started | — |
| 6 · Lazy startup | Not started | — |
| 7 · Health route and Failed/retry behaviour | Not started | — |
| 8 · Sanitized invocation history and schema redaction | Not started | — |
| 9 · Servers and tools UI | Not started | — |
| 10 · Documentation, measurement (§7) and close-out | Not started | — |

## Task 1 · Per-tool and MCP-management config surface

Requirements 1, 5.2, 5.4, 6; context D1, D2, D6.

- [ ] Add `McpToolConfig` keys `mcp_tool.<server>.<tool>.enabled`,
      `mcp_tool.<server>.<tool>.require_approval`, and
      `mcp_tool.<server>.<tool>.pinned`, parsed through
      `parse_namespaced_tool_name` so the two-part server/tool name is reused.
- [ ] Give `pinned` its scope rule: it defaults to `false` and is restrict-only
      — repository and profile scopes may clear it (`false`) but never pin
      (`true`), because pinning always sends a schema and spends context budget,
      which only the user or admin may opt into.
- [ ] Add the management keys from D2 and D6 (`mcp_deferral_ceiling_tokens`,
      `mcp_connect_timeout_ms`, `mcp_list_timeout_ms`, `mcp_call_timeout_ms`)
      with explicit scope rules: repository and profile may lower the ceiling,
      never raise it; timeouts are user/admin only (repository forbidden).
- [ ] Extend `Config::set` dispatch and `to_policy_text` for every new key; keep
      the exhaustive overlay classification (AGENTS.md) for repository scope.
- [ ] Tests: round-trip parse/save, default `enabled`/`require_approval=true`
      and `pinned=false` for unknown tools, restrict-only ceiling and `pinned`,
      repository forbidden from the timeout keys, and `mcp_tool.` keys are not
      reported `Unparsable`.

Files: `crates/workspace-engine/src/config.rs`,
`crates/workspace-engine/tests/permission_profiles.rs`,
`crates/workspace-engine/tests/foundation.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test permission_profiles --test foundation`;
`cargo fmt --all -- --check`; `typos`.

## Task 2 · Bounded, independently configurable timeouts (defines `McpServerHealth`)

Requirements 1, 4; context D5, D6.

- [ ] Define `McpServerHealth`
      (`Unknown`/`Ready`/`Unauthenticated`/`Timeout`/`Unreachable`/`Failed`),
      the type Task 6 holds in memory per shell process keyed by repository and
      server (D5).
- [ ] Replace `CONNECT_TIMEOUT`/`CALL_TIMEOUT` with the three config values,
      plumbed into `McpClient::connect`, `list_tools`, and `call_tool`.
- [ ] On startup/list timeout set `McpServerHealth::Timeout` and contribute no
      tools this turn; on call timeout return the existing tool error shape so
      the model can react.
- [ ] Record a timed-out call as an unknown outcome and mark it non-retryable
      (this lands fully in Task 8's audit work; here assert the runtime
      distinguishes it from an ordinary failure).
- [ ] Tests: a slow fake stdio server (the `foundation.rs` harness) times out
      for each phase independently, and the defaults still read 15 s / 15 s /
      60 s.

Files: `crates/workspace-engine/src/mcp.rs`, `config.rs`, `chat.rs`,
`crates/workspace-engine/tests/foundation.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test foundation`; `cargo fmt --all -- --check`.

## Task 3 · Tool inventory, token measurement, deferral ceiling

Requirements 2, 5.1; context D4, D7.

- [ ] Add `McpToolInventory` (per server: tools with name, description, schema
      bytes, estimated tokens via spec 26's estimator at the structured rate,
      `readOnlyHint`, content hash, `first_seen_at`) with JSON persistence under
      `<data_dir>/mcp/inventory/` keyed by repository identity.
- [ ] Refresh the inventory on any successful `tools/list`; a server with no
      inventory reads as "unknown cost" and is deferred (nothing connects).
- [ ] Compute the total schema cost of all enabled, offered tools and decide
      direct-vs-deferred from `mcp_deferral_ceiling_tokens` without connecting.
- [ ] Add the context-size test that asserts the measured figure: schemas in
      full versus with deferral, recording both numbers for §7.

Files: `crates/workspace-engine/src/mcp.rs` (or a new
`mcp_inventory.rs`), `config.rs`, `chat.rs`, `crates/workspace-engine/tests/foundation.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test foundation`; `cargo fmt --all -- --check`.

## Task 4 · Deferred discovery surface and `mcp_tool_search`

Requirements 2, 5.1, 5.2; context D3.

- [ ] Add the `mcp_tool_search(query)` tool (name, server, one-line
      description matches) always present when any MCP server is enabled.
- [ ] Replace the unconditional `mcp.tool_definitions()` extension with a
      surface that sends: tools whose `mcp_tool.<server>.<tool>.pinned` is
      `true`; all enabled tools when under the ceiling; and, once called, the
      schema of any tool matched this turn.
- [ ] A disabled tool is absent from the tool list, not refused on call.
- [ ] Tests: under-ceiling sends directly; over-ceiling sends only the search
      tool; search returns matches; calling a matched tool adds its schema; a
      pinned tool is always sent; a disabled tool never appears.

Files: `crates/workspace-engine/src/chat.rs`, `mcp.rs`, `mode.rs` (if a new
`ToolAction` is needed), `crates/workspace-engine/tests/agent_tools.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test agent_tools`; `cargo fmt --all -- --check`.

## Task 5 · Per-tool approval and read-only discipline (D1)

Requirements 6, 7; context D1.

- [ ] `requires_approval(server_id, tool_name)` resolves the intersection:
      server default true, per-tool override, exact-tool only; a tool's
      `require_approval=false` never affects a sibling.
- [ ] Carry the tool name through `PendingMcpCall` and re-check the per-tool
      decision on resume.
- [ ] Stop the read-only claim widening the profile layer: `profile_permits`
      refuses an MCP call when `allow_mutating_mcp_tools` is false regardless
      of the hint; keep mode widening as is.
- [ ] Show the read-only claim as the server's assertion in the approval text
      and record it in invocation history.
- [ ] Tests: mock server claiming read-only on a write tool still requires
      approval (the `foundation.rs` harness with a readOnlyHint);
      `require_approval=false` on one tool leaves its sibling requiring
      approval; the narrowed-profile test at `chat.rs:21946` now withholds
      read-only-claimed tools too.

Files: `crates/workspace-engine/src/chat.rs`, `profile.rs`, `mode.rs`,
`mcp.rs`, `crates/workspace-engine/tests/permission_profiles.rs`,
`crates/workspace-engine/tests/foundation.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test permission_profiles --test foundation`;
`cargo fmt --all -- --check`.

## Task 6 · Lazy startup

Requirements 1, 3; context D5.

- [ ] Hold `McpServerHealth` (defined in Task 2) in memory per shell process,
      keyed by repository and server; `Unknown` means "never contacted this
      session".
- [ ] Lazy startup: a server is contacted only when a tool is searched for or
      called, or when its tools are to be offered under the ceiling; the
      deferred path starts nothing until used.
- [ ] Set the health on contact (`Ready`, `Unauthenticated`, `Unreachable`;
      `Timeout` is set by Task 2) so the in-memory state reflects what actually
      happened.
- [ ] Tests: a session that never searches or calls starts no server process
      (assert the registry is empty).

Files: `crates/workspace-engine/src/mcp.rs`, `chat.rs`,
`crates/workspace-engine/tests/foundation.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test foundation`; `cargo fmt --all -- --check`.

## Task 7 · Health route and Failed/retry behaviour

Requirements 1, 3; context D5.

- [ ] A `Failed` server is not retried automatically for the session and its
      tools are not offered; one manual retry action re-contacts it.
- [ ] `GET /api/mcp-health` (and extending `/api/mcp-test` as the manual
      retry/diagnostic) serves `Unknown` vs `Ready` distinctly, with the auth
      reference and outcome, never the token value.
- [ ] Tests: `Unknown` != `Ready` in the API; `Failed` withholds tools until
      manual retry.

Files: `crates/workspace-engine/src/mcp.rs`, `chat.rs`,
`crates/desktop-shell/src/lib.rs`, `crates/workspace-engine/tests/foundation.rs`,
`crates/desktop-shell/tests/*`.
Checks: `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test foundation` and the desktop-shell health
route tests; `cargo fmt --all -- --check`.

## Task 8 · Sanitized invocation history and schema redaction

Requirements 1, 5.6, 5.7; context D8, D9.

- [ ] Extend the `mcp_tool_called`/`mcp_tool_call_failed` audit events with
      `duration_ms`, `outcome` (including `unknown` on timeout),
      `approval_decision`, redacted bounded `arguments`/`result`, and the
      attributed read-only claim, through `AuditLog::record`.
- [ ] Scan a server's tool descriptions and schemas through `SecretScanner`
      before they become `ToolDefinition`s.
- [ ] Tests: a seeded fake token never appears in any audit field, invocation
      history copy, or connection diagnostic; a server-supplied description
      embedding a secret is redacted before entering context.

Files: `crates/workspace-engine/src/audit.rs`, `mcp.rs`, `chat.rs`,
`crates/workspace-engine/tests/foundation.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test foundation`; `cargo fmt --all -- --check`.

## Task 9 · Servers and tools UI

Requirement 5.9.

- [ ] A servers list (label, transport, health with `Unknown` vs `Ready`, auth
      reference, tool count, enabled state) expanding to per-tool enable and
      approval toggles, marking tools that appeared since last viewed (from
      `first_seen_at`, D4).
- [ ] Show the context cost of the current MCP configuration as a token figure
      from the inventory (D4/D7).
- [ ] Wire the panel to `/api/mcp-health` and the existing settings/config
      routes; no new framework or build step.

Files: `crates/desktop-shell/src/lib.rs`, `crates/desktop-shell/static/app.js`,
`crates/desktop-shell/static/index.html` (and `*.css` as needed).
Checks: `node --check crates/desktop-shell/static/app.js`; `npm run lint:web`;
`cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p desktop-shell` for any new route tests.

## Task 10 · Documentation, measurement (§7) and close-out

- [ ] `docs/USER_GUIDE.md`: managing servers and tools, health states, why a
      read-only-claimed tool still asks, and deferred discovery.
- [ ] `docs/TROUBLESHOOTING.md`: connection diagnostics, health states, why a
      tool is not offered, where invocation history lives, reading an
      authentication failure without exposing the token.
- [ ] Record §7's measured token figures (full vs deferred, and the chosen
      ceiling), the search-hit observation, and any lazy-startup findings.
- [ ] Update `docs/specs/README.md` (row and status), remove the Unreleased
      item from `CHANGELOG.md`, and set Status Done in the proposal, this file
      and `docs/specs/README.md` only after CI is green.
- [ ] `npm run specs:check -- --strict` and `typos`.

Files: `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `CHANGELOG.md`,
`docs/specs/README.md`, this folder.
Checks: `npm run specs:check -- --strict`; `typos`; `cargo fmt --all -- --check`;
the fast checks from `scripts/quality-gate.sh fmt node lint typos deny`.
