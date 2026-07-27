# Feature Spec: MCP (Model Context Protocol) Server Support

Status: Done
Order: 6 of 6
Related spec sections: `ai_coding_assistant_specification.md` §7.5 (Model Adapter — tool/function-calling), §7.6 (Tool and Action Orchestrator), §7.3 (path/secret policy), §7.8 (risk classification & approval).

## 1. Motivation

The client can currently offer the model only a **fixed, code-defined** set of tools (`run_command`, `propose_patch`, `read_file`, `search_codebase`, `read_git_status`, `read_git_diff` — all built in `chat.rs`). There is no way for a user to extend that surface with their own tools.

[Model Context Protocol](https://modelcontextprotocol.io) (MCP) is the emerging standard for exactly this: a JSON-RPC 2.0 contract a host speaks to a "server" that exposes tools (and resources/prompts). Users increasingly expect a coding client to let them plug in **local** MCP servers (a subprocess speaking over stdio — e.g. a filesystem, database, or Jira server) and **remote** MCP servers (an HTTP endpoint — e.g. a hosted Sentry or GitHub connector).

The settings UI already reserves a home for this: a `Server` nav group with a single `Servers` page that today is just a placeholder — `<div class="settings-empty-card">No server configuration is required.</div>` (`crates/desktop-shell/static/index.html:333-336`). This spec repurposes that section into **MCP** server management and wires discovered MCP tools into the existing agentic tool-call loop.

## 2. Current State (with references)

- **Tool set is closed and hard-coded.** `run_agentic_turn` builds `native_tools` from six `*_tool_definition()` functions (`chat.rs:299-308`), dispatches by matching the tool name to a `ToolAction` enum variant (`tool_action_from_call`, `chat.rs:802`), and there is no registry or extension point. Spec §7.6 describes a closed action list, which this spec deliberately widens.
- **Native tools flow through one request field.** `ModelRequest.tools: Option<Vec<ToolDefinition>>` (`model.rs:106`) is the single channel to the model; MCP tools only need to be appended to the same `Vec<ToolDefinition>`. Tool definitions are OpenAI-shaped: `{ name, description, parameters_json }` (`model.rs:77`).
- **The engine is synchronous and subprocess-based.** `workspace-engine` has **no async runtime** (no `tokio`/`reqwest` in `crates/workspace-engine/Cargo.toml`). HTTP to model providers is done by shelling out to `curl` with a config file so secrets never hit `argv` (`CurlModelTransport`, `model.rs:247-270`). Local commands are run via `std::process::Command` (`command_runner.rs:89`). Any MCP client must fit this model — no new async runtime in the engine.
- **Config is a layered flat key=value overlay.** `Config` (`config.rs:39`) is populated from up to three `ConfigOverlay` files applied in order user → repo → admin (`load_with_policy_paths`, `config.rs:120`), so **admin wins**. Repeated sub-objects use dotted keys, e.g. `model_provider.<id>.base_url` (`set_model_provider_config`, `config.rs:557`; serialized by `push_model_provider_config`, `config.rs:760`). Lists join with `|`. This is exactly the shape MCP server config should reuse.
- **Secrets are keychain references, never plaintext in config.** An API key is stored as `keychain:<account>` (`parse_model_api_key_reference`, `config.rs:876`) and resolved at request time via the macOS keychain (`crates/desktop-shell/src/keychain.rs`; `resolve_model_api_key`, `lib.rs:473`). MCP auth tokens must use the same mechanism.
- **There is an approval + resume flow for side-effecting tool calls.** A command that needs approval persists the in-flight turn (`PendingChatTurn`, `chat.rs:391-401`) and resumes via `resume_after_command_decision`. MCP tool calls (which can have arbitrary external side effects) should reuse this, not invent a parallel path.
- **Tool results are already redacted.** `self.scanner.redact(...)` runs on model/command output (`chat.rs:356`, `command_runner.rs:114`). MCP tool results are untrusted external data and must go through the same scanner.
- **UI pattern to mirror.** The Providers page (`index.html:338-...`, driven by `app.js`) already implements "list of configured items + add/edit form + popular presets" against form-based Tauri commands in `lib.rs` (e.g. the provider save at `lib.rs:748-753`). The MCP page copies this shape.

## 3. Goals

- Let a user configure **local (stdio)** and **remote (HTTP)** MCP servers from the renamed **MCP** settings section, persisted in the existing config-overlay format with secrets in the keychain.
- Discover each enabled server's tools (`tools/list`) and offer them to native-tool-capable providers alongside the built-in tools, **namespaced** to avoid collisions.
- Execute model-requested MCP tool calls (`tools/call`) through the **existing** approval/policy/redaction flow, feeding results back into the same agentic loop.
- Fail safe: a broken, slow, or unreachable server degrades gracefully (its tools simply aren't offered / the call returns a structured error to the model) without taking down the turn or the app.
- Preserve the security boundary: admin config can disable MCP entirely or restrict which servers are allowed; MCP tool calls are treated as side-effecting and approval-gated by default.

## 4. Non-Goals

- **MCP resources and prompts.** Phase this spec to **tools only** (`tools/list`, `tools/call`). Resources/prompts/sampling are a follow-up.
- **Acting as an MCP server.** The client is a host/client only.
- **OAuth authorization-code flows for remote servers.** Phase 1 remote auth is a static bearer token (keychain-backed). Interactive OAuth is a follow-up (see Open Questions).
- **Introducing an async runtime into `workspace-engine`.** The MCP client stays synchronous/subprocess-based to match the existing architecture.
- **Non-macOS keychain backends.** Consistent with the project's current macOS-only posture.

## 5. Design

### 5.1 Config schema

Add `mcp_servers: Vec<McpServerConfig>` to `Config` (`config.rs:39`), mirroring `model_providers` field-for-field in how it's parsed, overlaid (`apply_overlay`, upsert-by-id), and serialized (`push_*`). Dotted overlay keys under an `mcp_server.<id>.` prefix, parsed in `ConfigOverlay::set` next to the existing `model_provider.` branch (`config.rs:506`):

```
# --- a local (stdio) server ---
mcp_server.filesystem.transport=stdio
mcp_server.filesystem.command=npx
mcp_server.filesystem.args=-y|@modelcontextprotocol/server-filesystem|/Users/me/projects
mcp_server.filesystem.env=NODE_ENV=production|LOG_LEVEL=warn
mcp_server.filesystem.enabled=true
mcp_server.filesystem.require_approval=true

# --- a remote (HTTP) server ---
mcp_server.sentry.transport=http
mcp_server.sentry.url=https://mcp.sentry.example/mcp
mcp_server.sentry.auth_token_env=keychain:mcp-sentry-token
mcp_server.sentry.enabled=true
mcp_server.sentry.require_approval=true
```

`McpServerConfig` fields:

| field | type | notes |
|---|---|---|
| `id` | `String` | Stable slug; namespace root for tools. Same charset rule as provider ids (alnum + `-`/`.`). |
| `label` | `String` | Display name; defaults to `id`. |
| `transport` | enum `stdio` \| `http` | Selects the transport in §5.2. |
| `command`, `args`, `env` | `String`, `Vec<String>`, `Vec<(String,String)>` | stdio only. `args`/`env` use the `|`-joined list convention (`split_list`, `config.rs`). `env` entries are `KEY=VALUE`. |
| `url` | `String` | http only. |
| `auth_token_env` | `String` | http only; `keychain:<account>` reference (reuse `parse_model_api_key_reference`). Sent as `Authorization: Bearer <token>`. |
| `enabled` | `bool` | Default `false` — a newly added server is offered only once the user turns it on. |
| `require_approval` | `bool` | Default `true`. Gate every `tools/call` on this server through the approval flow (§5.4). |

Security note: because admin overlay is applied last (`config.rs:120`), an admin file can set `mcp_server.<id>.enabled=false` (or a new `mcp_enabled=false` global kill-switch) to override user config. Add an admin-only `mcp_server_allowlist` (list of permitted ids); when non-empty, servers not on it are dropped after overlay merge.

### 5.2 MCP client and transports (`workspace-engine/src/mcp/`)

A new synchronous `mcp` module. Wire protocol is JSON-RPC 2.0; message framing and the two transports:

- **stdio transport.** Spawn `command` + `args` with `env` applied, via `std::process::Command` with `stdin`/`stdout` piped (and `stderr` captured to the audit log for diagnostics). Messages are newline-delimited JSON per the MCP stdio spec: write a request line to the child's stdin, read response lines from stdout, correlate by JSON-RPC `id`. The child inherits **no** ambient secrets beyond the explicitly configured `env`. Kill the child on drop / turn end.
- **http transport (Streamable HTTP).** Reuse the `curl`-subprocess approach from `CurlModelTransport` (`model.rs:247`) so the bearer token stays out of `argv`: `POST` the JSON-RPC request to `url`, `Authorization: Bearer` from the keychain, parse the response (single JSON body, or the first data event of an SSE stream — sufficient for request/response `tools/list` and `tools/call`). This keeps the engine free of `reqwest`/`tokio`.

Client surface (blocking):

- `McpClient::connect(cfg) -> Result<McpConnection>` — performs the `initialize` handshake (protocol version, client info, capabilities), with a bounded timeout.
- `McpConnection::list_tools() -> Result<Vec<McpTool>>` — `tools/list`; each `McpTool { name, description, input_schema }` maps directly onto `ToolDefinition` (`input_schema` → `parameters_json`).
- `McpConnection::call_tool(name, args_json) -> Result<McpToolResult>` — `tools/call`; result content flattened to text for the tool-result message.

Every network/subprocess op is wrapped in a **timeout** (config-defaulted, e.g. 30s for a call, 10s for connect/list). A timeout or transport error is a recoverable per-server failure, never a panic.

### 5.3 Tool discovery and namespacing

At the start of `run_agentic_turn` (`chat.rs:299`), after building the built-in `native_tools`, enumerate enabled MCP servers, connect, and `list_tools()`. Append each discovered tool to the `Vec<ToolDefinition>` with a **namespaced** name:

```
mcp__<server_id>__<tool_name>
```

(`mcp__` prefix + double-underscore separators, matching the widely-used convention.) Namespacing prevents collisions between servers and with the six built-in names. The description is passed through verbatim; `input_schema` becomes `parameters_json`.

Discovery is **best-effort and cached**: a server that fails to connect/list is skipped (its failure recorded in the audit log and surfaced in the UI as an error badge) and the turn proceeds with the remaining tools. Cache the tool list per connection for the duration of a turn to avoid re-listing on every round.

Only offered when `Config::supports_native_tools()` is true (`chat.rs:299`) — MCP has no text-envelope fallback, consistent with the read-only built-in tools which are also native-only (spec 03).

### 5.4 Dispatch, approval, and policy

Add a `ToolAction::McpCall { server_id, tool_name, arguments_json }` variant (`chat.rs:722`). `tool_action_from_call` (`chat.rs:802`) recognizes any name starting with `mcp__`, splits out `server_id`/`tool_name`, and constructs the variant.

In the dispatch `match` (`chat.rs:381`):

- If the server's `require_approval` is `true` (default), treat the call **exactly like a command needing approval**: build a proposal describing the server, tool, and arguments; persist `PendingChatTurn` (`chat.rs:391`) with the matched tool call; return `WaitingForApproval`. On resume (`resume_after_command_decision`), if approved, perform `call_tool` and feed the result back; if denied, feed a "user declined" tool result so the model can adapt.
- If `require_approval` is `false`, perform `call_tool` inline within the round and feed back the result — the same immediate-execution pattern the read-only built-in tools use (`chat.rs:446-505`).

In all cases:

- The result text is run through `self.scanner.redact(...)` before it re-enters the transcript — MCP output is **untrusted external content** and may contain secrets or prompt-injection payloads.
- Results are persisted (`append_message` assistant summary + `tool` result, `chat.rs:511-518`) and pushed as `ModelMessage::assistant_with_tool_calls` + `ModelMessage::tool(call.id, ...)` (`chat.rs:520-525`), identical to existing tools.
- A failed/timed-out call returns a **structured error string as the tool result** (not a turn failure), so the model can retry or route around it within `MAX_TOOL_ROUNDS`.
- MCP calls count toward the existing `MAX_TOOL_ROUNDS` budget.
- Every call/result is audit-logged (`self.audit_log.record`) with `server_id`, `tool_name`, and outcome.

### 5.5 UI (`index.html` + `app.js` + `style.css`)

- **Rename the nav.** `Server` group label → `MCP` (`index.html:267`); `Servers` item → `MCP Servers` (`index.html:270`); keep `data-settings-page="servers"` internally or rename to `mcp` (update the allow-list at `app.js:913`). Keep the existing server icon.
- **Replace the placeholder page** (`index.html:333-336`) with a Providers-style layout:
  - **Configured servers** list: each row shows label, transport badge (stdio/http), enabled toggle, connection status (connected / N tools / error), and Edit.
  - **Server details** editor: fields for label, id, transport selector, and transport-specific inputs (command/args/env for stdio; url + auth token for http), the `require_approval` toggle, and a **Test connection** button that runs `initialize` + `tools/list` and lists discovered tool names inline.
  - Optional **presets** section (like "Popular providers") for well-known servers (filesystem, GitHub, Sentry) to prefill the form.
- **Backend commands** in `lib.rs`: form-based handlers mirroring the provider save (`lib.rs:748-753`) — `mcp_server_save` (writes overlay keys via `update_config_overlay`, stores the auth token to the keychain via `keychain::write_password`), `mcp_server_delete`, `mcp_server_test` (connect + list, returns tool names or a structured error), and `mcp_server_list` (current config + last-known status).
- Secrets are write-only in the UI (never rendered back), matching the provider API-key field.

### 5.6 Lifecycle

- **Connect lazily**, at turn start for enabled servers; reuse the connection for the whole turn; tear down stdio children at turn end (no long-lived background processes in Phase 1 — simplest and safest; a persistent connection pool is a later optimization).
- **Timeouts** on connect/list/call as in §5.2; a server exceeding them is skipped for that turn.
- **No partial-turn hangs:** discovery failures and call failures are always converted to skips / structured tool-results.

## 6. Acceptance Criteria

- A user can add, enable, edit, disable, and delete a **stdio** MCP server from the MCP settings page; config round-trips through `user.conf` in the documented `mcp_server.<id>.*` format, and no secret is written in plaintext.
- A user can add an **http** MCP server with a keychain-backed bearer token; **Test connection** reports the discovered tool names or a clear error.
- With a native-tool-capable provider and an enabled server exposing tool `search`, the model is offered `mcp__<id>__search`; when the model calls it, the client performs `tools/call` and the result appears as a tool message the model can use in the same turn.
- With `require_approval=true` (default), an MCP tool call surfaces an approval prompt describing the server/tool/args before any external call is made; denial feeds a "declined" result and the turn continues.
- A server that is unreachable, times out, or returns an error does **not** crash the turn: its tools are silently omitted from discovery, or the call returns a structured error result to the model; the failure is audit-logged and shown in the UI.
- MCP tool-call results pass through the secret scanner before entering the transcript.
- An admin config setting `mcp_enabled=false` (or an `mcp_server_allowlist`) overrides user config and prevents disallowed servers from being offered.
- Providers without native tool support behave exactly as before (no MCP tools offered), with no regression.

## 7. Open Questions / Decisions Needed

- **Approval granularity.** Per-server `require_approval` (this spec) vs. per-tool, and whether to support "always allow this tool" persistence like a command allowlist. Recommend per-server for Phase 1, revisit per-tool if noisy.
- **Remote auth beyond static tokens.** Interactive OAuth (authorization-code + refresh) is common for hosted MCP servers but needs a browser/callback flow the current architecture doesn't have. Deferred; static bearer token only in Phase 1.
- **Persistent vs. per-turn connections.** Per-turn (spec default) is simplest and avoids zombie subprocesses; a warm pool would cut per-turn latency but adds lifecycle/cleanup complexity. Start per-turn.
- **CLI parity.** Whether `damaian-cli` should also load and expose MCP tools, or MCP stays desktop-only initially. Recommend desktop-first, mirror to CLI once stable (as done for prior specs).
- **Tool-count / context budget.** A server exposing dozens of tools can bloat the request. Consider a per-server tool allowlist or a global cap in a follow-up.

## 8. Suggested Phasing

1. **Config + engine core:** `McpServerConfig`, overlay parse/serialize, `mcp` module with **stdio** transport, `initialize`/`tools/list`/`tools/call`, timeouts.
2. **Loop integration:** namespacing, `ToolAction::McpCall`, dispatch through approval + redaction + audit.
3. **UI:** rename section, build the MCP page + backend commands + Test connection.
4. **Remote (http) transport** via `curl` subprocess + keychain bearer token.
5. **Hardening:** admin kill-switch/allowlist, error surfacing, docs (USER_GUIDE), CLI parity.

## 9. Implementation Notes (as built)

Landed 2026-07-27. Phases 1–4 shipped together; phase 5 partially (kill-switch + allowlist + error surfacing done; USER_GUIDE and CLI parity deferred).

- **Config** (`config.rs`): `McpTransport` enum, `McpServerConfig` + `McpServerConfigOverlay`, and `Config.{mcp_enabled, mcp_server_allowlist, mcp_servers}`. Overlay keys `mcp_server.<id>.*` parse/serialize by mirroring the `model_provider.<id>.*` path exactly. `Config::active_mcp_servers()` applies the kill-switch + allowlist + per-server `enabled`. `normalize_mcp_server_id` forbids `_` so the `mcp__<id>__<tool>` namespace splits unambiguously.
- **MCP client** (`mcp.rs`): synchronous JSON-RPC 2.0. `StdioTransport` spawns a child with a reader thread draining stdout into a channel so `recv_timeout` can bound blocking reads; child env is exactly the configured `env` (no ambient inheritance), stderr is discarded, and the child is killed on drop. `HttpTransport` drives a `curl -sS -D -` subprocess (token stays out of `argv`), parses either a JSON or SSE body, and echoes `Mcp-Session-Id`. `McpRuntime` is the per-turn orchestrator: lazy connect, cached tool lists/connections, best-effort discovery, audit logging.
- **Loop wiring** (`chat.rs`): `run_agentic_turn` builds an `McpRuntime` up front and appends its `tool_definitions()` to `native_tools`. New `ToolAction::McpCall` recognized in `tool_action_from_call` by the `mcp__` prefix. Dispatch reuses the command-approval machinery: an approval-required call persists a `PendingChatTurn` (extended with an `mcp_call` field) and returns an `AgentCommandProposal`; `resume_after_command_decision` branches on `mcp_call` to execute (or decline) on resume. Results pass through `SecretScanner::redact`. Auth-token resolution is injected via a new `McpTokenResolver` (the engine never touches the keychain).
- **Desktop shell** (`lib.rs`): `run_chat_request`/`run_resume_command_request` inject a keychain-backed `mcp_token_resolver()`. New `POST /api/mcp-test` (connect + `tools/list`) is the only added endpoint — save/delete reuse the existing `/api/config-file` + `/api/provider-key` flows the provider editor already uses.
- **UI** (`index.html`/`app.js`/`style.css`): the placeholder "Servers" section became "MCP Servers" (nav group "MCP"). The page mirrors the Providers editor — configured-server list, transport-aware editor (stdio: command/args/env; http: url/token), enabled + require-approval toggles, and a Test-connection button. MCP state is parsed from the config-editor text, same as providers.
- **Tests**: `mcp.rs` unit tests (namespacing, content flattening, SSE/header parsing); `foundation.rs` integration tests for overlay round-trip, kill-switch/allowlist gating, and a real stdio handshake→list→call against a shell-script server.
- **Deferred**: MCP resources/prompts, interactive OAuth, persistent connection pooling, CLI parity, USER_GUIDE docs (see §7).
