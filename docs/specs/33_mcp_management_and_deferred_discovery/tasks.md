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
| 1 · Per-tool and MCP-management config surface | Done | Per-tool overrides `mcp_server.<server>.tool.<tool>.<field>` (enabled, require_approval, pinned) and MCP-management keys (`mcp_deferral_ceiling_tokens`, three timeouts) with restrict-only/forbidden scope rules; round-trip and weakening tests; CONFIGURATION.md docs. |
| 2 · Bounded, independently configurable timeouts (defines `McpServerHealth`) | Done | `McpServerHealth` enum (6 states) and a non-retryable `McpTimeout` error variant; the three config timeouts plumbed through `McpClient::connect`/`list_tools`/`call_tool` (replacing the two consts); a connect/list timeout contributes no tools and a call timeout surfaces as a tool error. Checks pass: clippy, fmt, `foundation` (187/187). Deviation: the slow-server timeout tests and the 15 s / 15 s / 60 s defaults test were not added (test checkbox left open), and the runtime does not yet *set* `McpServerHealth::Timeout` — the per-process health store lands in Task 6. |
| 3 · Tool inventory, token measurement, deferral ceiling | Done | New `mcp_inventory.rs`: `McpToolInventory` (per tool: name, description, schema bytes, structured-rate token estimate, `readOnlyHint`, `sha256:` content hash, `first_seen_at_ms` kept across refreshes), one JSON file per server at `<data_dir>/mcp/inventory/<repository_id>/<server_id>.json`, written atomically; a missing or corrupt file reads as no inventory. `McpRuntime::tool_definitions` refreshes it on every successful `tools/list` (best-effort; a write failure is audited as `mcp_inventory_write_failed`). `decide_deferral` + public `ChatOrchestrator::mcp_tool_surface(repository_id, mode)` decide from the inventory alone: known servers are summed against one ceiling (`Direct` at or under it), servers without inventory are unknown cost and `Deferred`, and only tools the turn would offer count (per-tool `enabled`, mode and profile via a shared `mcp_tool_permitted` helper, browser-diagnostic servers excluded). **§7 measurement** (`mcp_deferral_decision_reads_the_inventory_and_measures_the_saving`, 3 fake servers / 60 tools): schemas in full **13,665** tokens vs **125** with deferral (a stand-in `mcp_tool_search` definition), ceiling 8,000. Checks: clippy `-p workspace-engine` clean; `foundation` 192/192 (the FSEvents watcher test fails only inside the agent sandbox, passes outside it); lib `mcp`/`mode_tool_list`/`mode_refusal` 57/57; `config_reference`+`permission_profiles`+`repository_config_trust` 151/151; fast gate fmt/node/lint/typos/deny ok; `specs:check` ok. Deviations: no config key added or changed (`mcp_deferral_ceiling_tokens` landed in Task 1, so `config.rs` is untouched); `McpRuntime::new` now takes `repository_id` and an `McpTimeouts` struct (clippy's argument limit); `build_mcp_runtime` takes the session's `repository_id`. The tool list sent to the model is unchanged, so the eval tier was not run. **For Task 4:** call `mcp_tool_surface(&session.repository_id, mode)` in `tool_offer` *before* `mcp.tool_definitions()` and skip listing deferred servers (today every turn still lists all servers, which is what refreshes the inventory — once deferral is wired, refresh happens only when a server is actually listed/called); pinned tools are counted in the ceiling sum today, so decide whether to exclude them; replace the test's stand-in search definition with the real one and re-record the deferred figure. |
| 4 · Deferred discovery surface and `mcp_tool_search` | Done | `tool_offer` now builds MCP schemas from `mcp_tool_surface`'s decision instead of listing every server: a server is listed only when it is under the ceiling (direct), has a pinned tool, or has a tool the turn named; a deferred server is never contacted. New `ToolAction::McpToolSearch` / `mcp_tool_search(query)` (allowed in every mode; profile refuses it only when MCP is off; effect `Notes`, so exploration-charged and never batched) is offered whenever an active non-browser MCP server exists. It searches the persisted inventory (`search_inventory`: query words in tool name weigh 2, in server id or description 1; at most 8 matches; only enabled tools the mode and profile permit under the recorded claim) and returns JSON `{matches:[{name,server,description}], unreachableServers?, note?}`, redacted. It lists only servers with no inventory yet, which records them — so an unknown-cost server becomes known; a call does the same for its own server. Schemas of tools the turn *named* — called, or returned by a search — are added from the next round (`offer_named_mcp_tools`, re-derived from the turn's messages so an approval pause/resume keeps them). **Deviation from D3 as written (recorded in `context.md` D3 "As built"):** a match's schema is sent from the round after the match, not only after a first schema-less call, and to keep "only the called server starts" the inventory now also stores each tool's `inputSchemaJson` (serde default; an older record without it lists the server instead). A called tool's server is listed before the permission check (`prepare_mcp_call`), so mode/profile read the server's live claim; approval is unchanged (test: a searched tool still pauses for approval). A disabled tool is never offered or matched; a call naming one is answered "not available" before any connection. The resume path lists only the called server (`McpRuntime::list_server`, the new single listing/refresh point). **D10 (new in `context.md`):** pinned tools keep counting against the ceiling. **§7 re-recorded:** 3 servers / 60 tools, schemas in full **13,665** tokens vs **152** deferred (the real `mcp_tool_search` definition, measured from an actual request; Task 3's stand-in was 125), ceiling 8,000. Tests: 9 new in `agent_tools.rs` (direct under ceiling, search-only over it, search matches without connecting, matched tool's schema added and call runs, pinned always sent, disabled never appears, no-MCP-use session starts no server/registry entry/inventory, search lists and records an unknown-cost server, searched tool still needs approval); `search_inventory` unit tests in `mcp_inventory.rs`; `mode_tool_list_tests` now seed an inventory (a never-listed server is deferred); Task 3's foundation test now populates the inventory through a search turn and measures the real definition. Checks: clippy `-p workspace-engine --all-targets` clean; `nextest -p workspace-engine --lib --test agent_tools --test foundation` 1024/1025 in the sandbox, the one failure the known FSEvents watcher test, which passes outside it; deterministic eval tier 65/65 scenarios pass, 0 approval-policy violations (run outside the sandbox: it binds a stub server); fast gate fmt/node/lint/typos ok, deny ok (run outside the sandbox: advisory-db lock); `specs:check` ok. Workspace clippy and tests left to CI. **For Task 5:** `handle_mcp_call` and the resume path still ask `mcp.requires_approval(server_id)` per server; the per-tool decision plugs in there. **For Task 6:** most of lazy startup landed here — no server is contacted unless direct, pinned, named, searched (unknown cost only) or called, and `a_session_that_uses_no_mcp_tool_starts_no_server` already asserts it; what remains is the in-memory `McpServerHealth` store. **For Task 8:** search results pass through `SecretScanner`, but offered descriptions/schemas (including those served from the inventory) are not yet scanned. **For Task 10:** the inventory file grew by the stored schema. **CI:** the full seven-check gate passed in CI on 2026-11-23 (Quality run 45176771013, on main after the merge). |
| 5 · Per-tool approval and read-only discipline (D1) | Done | Approval is decided per tool by the new `Config::mcp_tool_requires_approval(server, tool)`: the tool's own `require_approval` when set for exactly that tool, otherwise the server's (default true); an unknown server asks. `McpRuntime::requires_approval(server)` is gone; `handle_mcp_call` asks the config. `McpToolConfig::require_approval` is now `Option<bool>` (unset is no longer written by `to_policy_text`, so a reload does not freeze a tool to the server's value of the day). `profile_permits` (in `mode.rs`) no longer takes the claim: an MCP call is refused under `allow_mutating_mcp_tools=false` whatever the server says; `mode_permits` is unchanged, so Ask/Review still *offer* claimed tools. The approval card adds "The server `<id>` describes this tool as read-only. That is the server's own claim, which Damaian cannot check, so the call still needs your approval." only for `readOnlyHint: true`; `mcp_tool_called`/`mcp_tool_call_failed` audit events carry `serverReadOnlyClaim` (`claimed_read_only`/`claimed_not_read_only`/`no_claim`). On resume, an approved call is re-checked against the current per-tool `enabled` (after mode/profile) and answered "not available" if the tool was disabled while the card waited. `docs/CONFIGURATION.md` rows for the server and tool `require_approval` keys updated; `context.md` D1 "As built". **Deviations / discoveries:** (1) the semantics chosen for "intersection" is *tool value when set, else server value*, not "both must waive": the proposal's own example (`tool.search_code.require_approval=false` on a default server) only works that way, and existing server-level `require_approval=false` keeps working for tools without a setting. Because a tool's value outranks its server's, a repository/profile `mcp_server.<id>.require_approval=true` now also flips that server's per-tool `false` waivers to `true` in `upsert_mcp_server_from_repository`, so the tightening still reaches every tool (repository may still never set a tool `false`: RestrictOnly, unchanged). No config key was added, so no new overlay classification, weakening case or profile/`effective_policy` entry was needed. (2) `PendingMcpCall` already carried `tool_name` (repository wins); the re-check on resume is the `enabled` check above — approval itself is what the user just gave. (3) `profile_permits` lives in `mode.rs`, not `profile.rs`; `profile.rs` is untouched. The `chat.rs:21946` test no longer exists (chat was split into `chat/`); its expectation is the `mode.rs` matrix row for `readOnlyHint: true` (read_only profile now refuses) plus a new `mode_tool_list_tests::a_read_only_claim_does_not_carry_a_tool_past_the_profile`. (4) The end-to-end approval tests live in `tests/agent_tools.rs` (beside Task 4's MCP tests, whose `mcp_engine` harness drives a whole turn), not `foundation.rs`, whose fake server cannot run a chat turn. Tests: `agent_tools` `a_tool_claiming_read_only_still_needs_approval_and_the_card_attributes_the_claim`, `a_tool_with_no_read_only_claim_gets_no_claim_line`, `require_approval_false_on_one_tool_leaves_its_sibling_asking`, `a_tool_can_ask_on_a_server_that_does_not` (also asserts the audit claim); `foundation` `mcp_tool_approval_resolves_per_tool_over_the_server` (and the Task 1 round-trip tests updated to `Option`); `permission_profiles` `a_repository_server_gate_reaches_tools_the_user_waived`; the `mode.rs` matrix row. Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest run -p workspace-engine --lib --test permission_profiles --test foundation --test agent_tools --test repository_config_trust --test config_reference` 1228/1228 (7 skipped); deterministic eval tier 66/66 scenarios pass, 0 approval-policy violations; `scripts/quality-gate.sh fmt node lint typos deny` ok. Workspace clippy and tests left to CI. **For Task 8:** the attributed claim is already in the two call audit events (`serverReadOnlyClaim`); Task 8 adds `approval_decision`, duration, outcome and redacted arguments/result beside it. **For Task 9:** a per-tool approval toggle should write `mcp_server.<server>.tool.<tool>.require_approval` and show "follows the server" when unset (`McpToolConfig::require_approval == None`). **For Task 10:** USER_GUIDE should say a tool's own setting outranks its server's, and that a read-only-claimed tool still asks. The full seven-check gate passed in CI on 2026-11-25 (Quality run 45615616976). |
| 6 · Lazy startup | Not started | — |
| 7 · Health route and Failed/retry behaviour | Not started | — |
| 8 · Sanitized invocation history and schema redaction | Not started | — |
| 9 · Servers and tools UI | Not started | — |
| 10 · Documentation, measurement (§7) and close-out | Not started | — |

## Task 1 · Per-tool and MCP-management config surface

Requirements 1, 5.2, 5.4, 6; context D1, D2, D6.

- [x] Add `McpToolConfig` keys `mcp_server.<server>.tool.<tool>.enabled`,
      `mcp_server.<server>.tool.<tool>.require_approval`, and
      `mcp_server.<server>.tool.<tool>.pinned`, parsed by stripping the
      `mcp_server.` prefix, splitting the remainder on the first `.tool.`,
      and keying the map by `namespaced_tool_name(server, tool)`; reject a
      server id containing `.tool.`.
- [x] Give `pinned` its scope rule: it defaults to `false` and is restrict-only
      — repository and profile scopes may clear it (`false`) but never pin
      (`true`), because pinning always sends a schema and spends context budget,
      which only the user or admin may opt into.
- [x] Add the management keys from D2 and D6 (`mcp_deferral_ceiling_tokens`,
      `mcp_connect_timeout_ms`, `mcp_list_timeout_ms`, `mcp_call_timeout_ms`)
      with explicit scope rules: repository and profile may lower the ceiling,
      never raise it; timeouts are user/admin only (repository forbidden).
- [x] Extend `Config::set` dispatch and `to_policy_text` for every new key; keep
      the exhaustive overlay classification (AGENTS.md) for repository scope.
- [x] Tests: round-trip parse/save, default `enabled`/`require_approval=true`
      and `pinned=false` for unknown tools, restrict-only ceiling and `pinned`,
      repository forbidden from the timeout keys, and
      `mcp_server.<server>.tool.<tool>.` keys are not reported `Unparsable`.

Files: `crates/workspace-engine/src/config.rs`,
`crates/workspace-engine/tests/permission_profiles.rs`,
`crates/workspace-engine/tests/foundation.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test permission_profiles --test foundation`;
`cargo fmt --all -- --check`; `typos`.

## Task 2 · Bounded, independently configurable timeouts (defines `McpServerHealth`)

Requirements 1, 4; context D5, D6.

- [x] Define `McpServerHealth`
      (`Unknown`/`Ready`/`Unauthenticated`/`Timeout`/`Unreachable`/`Failed`),
      the type Task 6 holds in memory per shell process keyed by repository and
      server (D5).
- [x] Replace `CONNECT_TIMEOUT`/`CALL_TIMEOUT` with the three config values,
      plumbed into `McpClient::connect`, `list_tools`, and `call_tool`.
- [x] On startup/list timeout set `McpServerHealth::Timeout` and contribute no
      tools this turn; on call timeout return the existing tool error shape so
      the model can react.
- [x] Record a timed-out call as an unknown outcome and mark it non-retryable
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

- [x] Add `McpToolInventory` (per server: tools with name, description, schema
      bytes, estimated tokens via spec 26's estimator at the structured rate,
      `readOnlyHint`, content hash, `first_seen_at`) with JSON persistence under
      `<data_dir>/mcp/inventory/` keyed by repository identity.
- [x] Refresh the inventory on any successful `tools/list`; a server with no
      inventory reads as "unknown cost" and is deferred (nothing connects).
- [x] Compute the total schema cost of all enabled, offered tools and decide
      direct-vs-deferred from `mcp_deferral_ceiling_tokens` without connecting.
- [x] Add the context-size test that asserts the measured figure: schemas in
      full versus with deferral, recording both numbers for §7.

Files: `crates/workspace-engine/src/mcp.rs` (or a new
`mcp_inventory.rs`), `config.rs`, `chat.rs`, `crates/workspace-engine/tests/foundation.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test foundation`; `cargo fmt --all -- --check`.

## Task 4 · Deferred discovery surface and `mcp_tool_search`

Requirements 2, 5.1, 5.2; context D3.

- [x] Add the `mcp_tool_search(query)` tool (name, server, one-line
      description matches) always present when any MCP server is enabled.
- [x] Replace the unconditional `mcp.tool_definitions()` extension with a
      surface that sends: tools whose `mcp_server.<server>.tool.<tool>.pinned` is
      `true`; all enabled tools when under the ceiling; and, once called, the
      schema of any tool matched this turn.
- [x] A disabled tool is absent from the tool list, not refused on call.
- [x] Tests: under-ceiling sends directly; over-ceiling sends only the search
      tool; search returns matches; calling a matched tool adds its schema; a
      pinned tool is always sent; a disabled tool never appears.

Files: `crates/workspace-engine/src/chat.rs`, `mcp.rs`, `mode.rs` (if a new
`ToolAction` is needed), `crates/workspace-engine/tests/agent_tools.rs`.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test agent_tools`; `cargo fmt --all -- --check`.

## Task 5 · Per-tool approval and read-only discipline (D1)

Requirements 6, 7; context D1.

- [x] `requires_approval(server_id, tool_name)` resolves the intersection:
      server default true, per-tool override, exact-tool only; a tool's
      `require_approval=false` never affects a sibling.
- [x] Carry the tool name through `PendingMcpCall` and re-check the per-tool
      decision on resume.
- [x] Stop the read-only claim widening the profile layer: `profile_permits`
      refuses an MCP call when `allow_mutating_mcp_tools` is false regardless
      of the hint; keep mode widening as is.
- [x] Show the read-only claim as the server's assertion in the approval text
      and record it in invocation history.
- [x] Tests: mock server claiming read-only on a write tool still requires
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
