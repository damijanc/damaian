# Feature Spec: MCP Management and Deferred Discovery

Status: Not started
Order: 33 of 33
Roadmap: `docs/ROADMAP/04_phase_4_customization_and_extensibility.md`, Phase 4,
Work Package 4 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.5 (model
adapter, tool calling), section 7.6 (tool and action orchestrator), section 7.8
(risk classification and approval). Related implementation specs:
[`06_mcp_support.md`](06_mcp_support.md) (the delivered runtime this manages),
[`03_structured_tool_calling.md`](03_structured_tool_calling.md),
[`17_durable_task_state_and_crash_recovery.md`](17_durable_task_state_and_crash_recovery.md)
(the PID registry MCP servers are the primary client of),
[`20_working_modes.md`](20_working_modes.md) (mode filters MCP tools),
[`26_context_assembly.md`](26_context_assembly.md) (the context budget this
protects), [`31_permission_profiles.md`](31_permission_profiles.md) (the profile
that governs external writes).

## 1. Motivation

Every enabled MCP server's complete tool schema reaches every model request.

`ChatOrchestrator` builds the tool list once per turn and extends it with
`mcp.tool_definitions()` for every active server (`chat.rs:711-731`). There is no
per-tool control, no deferral, and no bound: a server exposing forty tools
contributes forty JSON schemas to every request in the session, whether the task
needs them or not.

That is a direct conflict with [spec 26](26_context_assembly.md), which allocates
context by category and accounts for every token. Tool schemas are not part of
that accounting — they arrive in the request's `tools` array rather than as
context items — so an unmanaged MCP configuration silently consumes the window
that context assembly is carefully budgeting, and the manifest cannot even
report it. The user sees a smaller file-range budget and no explanation.

The management gaps compound it. `/api/mcp-test` tests a connection on demand
and nothing tracks health or authentication state afterwards, so a server whose
token expired presents its tools to the model and fails at call time, every turn.
Servers start eagerly. There is no way to enable one tool from a server and not
another.

And one correctness point worth stating plainly because it is easy to get wrong
in the opposite direction: a remote tool describing itself as read-only is making
an assertion about a system Damaian cannot see. Treating that claim as a
guarantee would let a remote server opt itself out of approval by describing
itself favourably.

## 2. Current State

- **All active servers' schemas go into every request.** `chat.rs:711-731`
  builds `native_tools`, then `tools.extend(mcp.tool_definitions()…)`, filtering
  only browser-diagnostic servers. No per-tool selection, no deferral.
- **Server gating is a three-way intersection already.**
  `Config::active_mcp_servers` (`config.rs:475-488`) requires `mcp_enabled`, the
  server's own `enabled`, and membership of `mcp_server_allowlist` when
  non-empty. This narrowing pattern is the precedent
  [spec 31](31_permission_profiles.md) §5.2 generalises.
- **`McpServerConfig`** (`config.rs:153-175`) carries `id`, `label`,
  `transport`, `command`, `args`, `env`, `url`, `auth_token_env`, `enabled`, and
  `require_approval`. Per-**tool** configuration does not exist.
- **`auth_token_env` is already a reference, not a value**: `keychain:<account>`
  or an environment variable name (`config.rs:165-167`), matching the
  `model_api_key_env` rule in `AGENTS.md`.
- **Servers are per-turn and eager.** `build_mcp_runtime` (`chat.rs:708`)
  connects lazily *within* a turn and tears everything down on drop, per
  [spec 06](06_mcp_support.md) — but every active server is connected to list
  tools, whether or not a tool is called.
- **Stdio servers spawn children with no kill-on-drop guard**
  (`mcp.rs:323`), which
  [spec 17](17_durable_task_state_and_crash_recovery.md) §5.7 identifies as one
  of three orphan sources and assigns to the PID registry.
- **Tools are namespaced** `mcp__<server>__<tool>`, parsed by
  `parse_namespaced_tool_name` (used at `chat.rs:726`), so per-tool addressing
  has a naming scheme already.
- **`/api/mcp-test` exists** in `crates/desktop-shell/src/lib.rs` as a manual
  connection test. There is no persistent health or auth state.
- **`McpToolDescriptor::to_tool_definition`** (`mcp.rs:108`) converts a
  discovered tool to a `ToolDefinition`.
- **No timeouts are bounded** for startup, list, or call beyond whatever the
  transport does.

## 3. Requirements

1. Add server health and authentication status; per-server enable and disable;
   **per-tool** enable and disable; approval policy per server and per tool;
   deferred tool discovery or tool search; connection diagnostics; and sanitized
   invocation history.
2. **Do not send every MCP schema in every model request.**
3. Start local servers lazily where practical.
4. Bound startup, list, and call timeouts.
5. Configured secrets never appear in model context or logs. `auth_token_env`
   holds a reference, never a value.
6. **Distinguish a remote server's read-only claim from enforced local policy.**
   A tool that says it is read-only is making an assertion, not a guarantee.
7. External writes require explicit approval unless a user profile specifically
   allows them.
8. MCP server processes are tracked and terminated by PID.

## 4. Non-goals

- Changing the MCP protocol implementation in `mcp.rs`, its transports, or its
  framing. This manages what [spec 06](06_mcp_support.md) delivered.
- An MCP server marketplace, registry, or installer.
- Authoring MCP servers.
- Inferring what a remote tool actually does. §5.5 treats every remote tool as
  potentially side-effecting and does not attempt to classify them.
- OAuth or interactive authentication flows. `auth_token_env` continues to
  resolve a reference the user configured.
- Sandboxing MCP servers. A local stdio server runs with the user's privileges,
  as it does today; that is a much larger piece of work.
- Caching tool results.
- Per-tool risk classification comparable to `CommandPolicy`'s. §5.5 explains
  why a remote tool's risk is not locally knowable.

## 5. Design

### 5.1 Deferred discovery: a search tool instead of every schema

Requirement 2 is the work package's primary purpose. Replace the unconditional
schema dump with a two-stage surface, which is the shape MCP's own ecosystem has
converged on:

**Stage 1 — always present.** One small tool:

```text
mcp_tool_search(query: string) -> list of { name, server, one-line description }
```

Its own schema is a few hundred tokens regardless of how many servers are
configured. The model calls it when it needs a capability it does not have.

**Stage 2 — on demand.** `mcp_tool_search` returns matches with names and
one-line descriptions. Fetching a tool's full schema — and thereby making it
callable — is a second step that adds only the schemas actually needed to the
turn's tool list, from that round onward.

Two cases keep this from being a regression in usability:

- **A small configuration is not deferred.** When the total schema cost of all
  enabled tools is under a configured ceiling, they are sent directly. Deferral
  costs a round trip, and paying that to save nothing would make the common
  case — one or two servers with a handful of tools — worse. The ceiling is the
  decision point, and it is measured rather than guessed (§7).
- **Explicitly pinned tools are always sent.** A user who uses one MCP tool
  constantly should not pay a search round trip for it every turn.

Requirement 1's "deferred tool discovery **or** tool search" allows either; this
picks search because it degrades better. A model that never calls the search tool
simply has fewer capabilities that turn, whereas a discovery protocol the model
must be taught to drive fails opaquely when it does not.

**The saving must be measured, not asserted.** The acceptance criterion is a
context-size test comparing tokens with and without deferral, per the roadmap.

### 5.2 Per-tool control

`mcp_tool.<server_id>.<tool_name>` config keys, reusing the existing namespacing
that `parse_namespaced_tool_name` already parses:

```text
mcp_tool.github.create_issue.enabled=false
mcp_tool.github.search_code.require_approval=false
```

Requirement 1's per-tool enable and disable, with the essential property from the
roadmap's acceptance criteria: **a disabled tool is absent from the model's tool
list, not merely refused on call.** Absent is cheaper (no schema, no tokens) and
safer (the model cannot try), and it is the same principle as
[spec 20](20_working_modes.md) §5.2 layer 1 — withholding beats refusing.

Resolution is an intersection, following
[spec 31](31_permission_profiles.md) §5.2: a tool is offered only if MCP is
enabled globally, the server is enabled and allowlisted, the tool is enabled, the
profile permits it, and the mode permits it. Any layer can remove; none can add.

Unknown tools default to **enabled** for a server the user enabled — a server
that adds a tool in an update should not silently break — but a newly appearing
tool is surfaced in the server's detail view, and if its declared side-effecting
nature differs from its siblings, it is reported rather than absorbed. The
roadmap's storage section calls for re-review when requested permissions change
on update; this is that rule applied to tools.

### 5.3 Health, auth, and lazy startup

Requirements 1 and 3. Per-server state, persisted per repository alongside other
per-repository data:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerHealth {
    /// Never contacted this session.
    Unknown,
    Ready,
    /// Reachable, but authentication failed.
    Unauthenticated,
    /// Startup, list, or call exceeded its timeout.
    Timeout,
    Unreachable,
    /// Failed repeatedly; not retried automatically this session.
    Failed,
}
```

Startup is lazy per requirement 3: a server is contacted when one of its tools is
searched for or called, not when a session opens. Combined with §5.1, a session
that never uses an MCP tool starts no MCP server at all — which is the common
case and currently costs a process spawn per enabled server.

Lazy startup interacts with health honestly: `Unknown` means "not contacted", not
"healthy". The UI distinguishes them, because a green indicator for a server
nobody has spoken to is a lie.

A server in `Failed` is not retried automatically for the rest of the session and
its tools are not offered — with the reason visible, so the user knows why a
capability disappeared rather than watching the model fail to use it. Manual
retry is one action, and `/api/mcp-test` remains the diagnostic.

### 5.4 Bounded timeouts

Requirement 4, with three separate budgets because they fail differently:

| Phase | Default | On expiry |
|---|---|---|
| Startup / connect | short | `Timeout` health, tools not offered this turn |
| `tools/list` | short | Server contributes no tools this turn |
| `tools/call` | longer | The call fails as a tool error the model can react to |

A call timeout is reported to the model as a tool error rather than failing the
turn — a remote system being slow is a normal condition the agent can work
around, and it is also, per
[spec 17](17_durable_task_state_and_crash_recovery.md), an **unknown outcome**:
the request may have been received and acted on. A timed-out call that could have
written something is recorded as such and never automatically retried.

### 5.5 A read-only claim is an assertion, not a guarantee

Requirement 6, and the requirement most likely to be implemented as its opposite,
because a `readOnlyHint` field in a tool descriptor is right there and trusting it
is one line of code.

**A remote tool's self-description carries no local authority.** The claim comes
from the server, the server is configured by the user but authored by someone
else, and the system it touches is one Damaian cannot inspect. A server that
wanted its writes unapproved need only describe itself as read-only.

So:

- **Every remote tool is treated as potentially side-effecting** for approval
  purposes. `require_approval` defaults to true per server and per tool.
- **A read-only claim may only lower a display emphasis, never an approval
  requirement.** It is shown to the user as *"the server describes this as
  read-only"* — attributed, not asserted — and is recorded in the invocation
  history.
- **The user may set `require_approval=false` for a specific tool** they have
  decided to trust, which is a local decision recorded in local config. That is
  the only way a remote tool becomes automatic, and it is exact-tool, mirroring
  [spec 10](10_persistent_command_approval.md)'s exact-command rule.

Requirement 7's "unless a user profile specifically allows them" is therefore a
profile capability key ([spec 31](31_permission_profiles.md) §5.1), subject to
that spec's rule that repository config cannot widen it: a cloned repository
cannot enable an MCP server, nor drop a tool's approval requirement.

**A local stdio server is not safer for being local.** It runs as a child process
with the user's privileges (`mcp.rs:323`), so a local server's tools are governed
by the same rule. Sandboxing is a non-goal, which makes the approval requirement
the only control there is.

### 5.6 Secrets

Requirement 5. `auth_token_env` already holds a reference (`config.rs:165-167`).
The additions are about everything built around it:

- The resolved token value is used only to construct the `Authorization` header
  and is never placed in a struct that is serialised, logged, audited, or
  rendered.
- **Invocation history stores arguments and results redacted through
  `SecretScanner`**, bounded, and never the auth header.
- Connection diagnostics report the reference (`keychain:github-token`) and the
  outcome, never the value — and an authentication failure says "authentication
  failed for `keychain:github-token`", which is diagnostic without being a
  disclosure.
- Tool descriptions and schemas from a server are model context, so they pass
  through the same redaction path as other context: a server whose tool
  description embeds a credential does not get to put it in the window.

That last case is worth the line. Everything else here guards secrets Damaian
holds; this one guards against a secret arriving *from* the server.

### 5.7 Invocation history

Requirement 1's "sanitized invocation history": per call, the server, tool,
timestamp, duration, outcome, approval decision, and redacted bounded arguments
and result. Written through `AuditLog::record` (`audit.rs:42`), which redacts
field values on the way in, rather than a parallel log — the roadmap is explicit
that the audit log is the mechanism to reuse.

This is what makes "what has this server actually done?" answerable, which is the
question a user has after enabling something authored by a third party.

### 5.8 Process lifetime

Requirement 8. Stdio children (`mcp.rs:323`) register their PID and start time in
the [spec 17](17_durable_task_state_and_crash_recovery.md) §5.7 registry, are
killed by PID, and are start-time-checked before killing so a recycled PID is
never someone else's process.

`AGENTS.md`'s prohibition on matching processes by name applies with force: MCP
server binaries are commonly shared with the user's other tooling, and a
name-based kill would take out their editor's language server or another client's
session.

Lazy startup (§5.3) reduces the exposure — fewer servers are started at all — but
does not remove it, since a server started for one call outlives that call within
the session.

### 5.9 UI

A servers list showing per server: label, transport, health with its distinction
between `Unknown` and `Ready`, auth reference, tool count, and enabled state.
Expanding a server lists its tools with enable and approval controls, and marks
any tool that appeared since the user last looked.

The context cost of the current MCP configuration is shown as a token figure, so
the thing requirement 2 exists to control is visible rather than inferred.

### 5.10 Documentation

`docs/USER_GUIDE.md`: managing servers and tools, what the health states mean,
why a tool the server calls read-only still asks for approval, and how deferred
discovery changes what the agent can reach. `docs/TROUBLESHOOTING.md`: reading
connection diagnostics, what each health state implies, why a tool is not
offered, where invocation history lives, and how to interpret an
authentication failure without exposing the token.

## 6. Acceptance Criteria

- With several servers configured, tokens spent on tool schemas per request are
  bounded and **measurably lower** than sending all schemas — asserted by a
  context-size test comparing both, with the measured figures recorded in §7.
- A configuration under the deferral ceiling sends schemas directly, so a small
  setup pays no search round trip.
- An explicitly pinned tool is always sent.
- A disabled tool is **absent from the model's tool list**, not merely refused on
  call.
- Tool availability is an intersection: global switch, server enabled, server
  allowlisted, tool enabled, profile, and mode. Any layer can remove a tool; no
  layer can add one — asserted per layer.
- A repository config cannot enable an MCP server or clear a tool's approval
  requirement ([spec 31](31_permission_profiles.md) §5.2).
- A session that uses no MCP tool starts no MCP server process.
- `Unknown` health is distinguished from `Ready` in the UI and in the API.
- A server in `Failed` is not retried automatically, its tools are not offered,
  and the reason is visible.
- Startup, list, and call timeouts are each bounded and independently
  configurable.
- A timed-out `tools/call` is reported to the model as a tool error, is recorded
  as an unknown outcome, and is never automatically retried.
- A tool whose descriptor claims read-only **still requires approval**, and the
  claim is displayed as the server's assertion — asserted with a mock server
  claiming read-only on a write tool.
- Setting `require_approval=false` for one tool does not affect any other tool
  on the same server.
- A configured token value never appears in model context, logs, audit fields,
  invocation history, or connection diagnostics — asserted with a seeded fake
  token.
- A tool description arriving from a server that embeds a secret is redacted
  before entering context.
- Invocation history records server, tool, outcome, approval decision, and
  redacted bounded arguments through `AuditLog::record`.
- A newly appearing tool on an enabled server is surfaced rather than silently
  absorbed.
- No MCP server process outlives the session that started it, and a recorded PID
  whose start time no longer matches is not killed.
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no regression and no
  increase in approval-policy violations.

## 7. Implementation Notes

To be completed during implementation. Record:

- **The measured token figures**: schemas for the test configuration sent in
  full, versus with deferral, and the deferral ceiling chosen. This is the
  work package's primary claim and the one number that proves it.
- How often the model called `mcp_tool_search` versus failing to find a
  capability it needed. A model that never searches has effectively lost those
  tools, and that is a regression the token saving does not justify — if it
  happens, record it before tuning the ceiling upward.
- Whether lazy startup broke any server that expected an early handshake.
- Which servers were tested, and whether any declared `readOnlyHint` on a tool
  that turned out to write.
