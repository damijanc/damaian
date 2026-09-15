# Feature Spec: MCP Server Mode

Status: Not started
Order: 52 of 53
Plan: `docs/PLAN/04_phase_4_customization_and_extensibility.md`, Phase 4,
Work Package 6 (Could). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#31](31_permission_profiles.md) (the profile the exposed set
narrows) — **not built**; [#33](33_mcp_management_and_deferred_discovery.md)
(the MCP runtime) — **not built**. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`06_mcp_support.md`](06_mcp_support.md) (the client half, whose protocol code
and types this reuses),
[`33_mcp_management_and_deferred_discovery.md`](33_mcp_management_and_deferred_discovery.md)
(client-side management; the mirror-image rules in §5.4 come from it),
[`31_permission_profiles.md`](31_permission_profiles.md) (the profile a server
process narrows and can never widen),
[`34_repository_config_trust_boundary.md`](34_repository_config_trust_boundary.md)
(the scope rules that decide who chooses the repository),
[`47_agent_working_capability.md`](47_agent_working_capability.md) (the read
tools this exposes).

## 1. Motivation

Damaian knows things about a repository that nothing else does: an indexed
semantic search ([spec 02](02_semantic_search.md)), redaction that is applied on
every read path, a path policy, and — from Phase 3 — a repository map and a
symbol index. All of it is reachable only from Damaian's own window.

A user who spends part of their day in another tool has two options today:
switch windows, or give that tool raw filesystem access and lose every guarantee
in the list above. The roadmap deliberately defers "broad IDE replacement", and
that deferral is right; but it leaves the question of how Damaian's
understanding of a repository reaches anywhere else, and the answer should not
be "it does not".

MCP is already in the codebase as a client ([spec 06](06_mcp_support.md)). The
same protocol, the same transport and much of the same code runs in the other
direction. The cost is small; the constraint is that a server has no user
interface, and therefore no way to ask for permission — which is what makes this
a read-only feature and Could-tier rather than a larger one.

## 2. Current State

- **`mcp.rs` is client-only.** `McpClient::connect`, `list_tools`, `call_tool`
  and `McpRuntime` consume servers. `namespaced_tool_name` and
  `parse_namespaced_tool_name` already handle the naming both directions need.
- **There is no server entry point.** `crates/damaian-cli` is the command-line
  front end; no subcommand speaks MCP.
- **Read paths are already safe.** `FileAccessController` applies
  `path_policy.rs`; `SecretScanner` redacts; `AuditLog::record` records. A
  server that reuses those paths inherits all three, and one that does not would
  be building a second, unaudited way into the same repository.
- **Approval requires a human.** Every approval surface in the product is a
  dialog in the desktop shell. There is no headless approval, and Phase 5 WP8
  defines the headless posture as fail-closed.

## 3. Requirements

1. Damaian runs as an MCP server over stdio, exposing a **read-only** subset of
   its tools.
2. The repository a server process operates on is fixed at launch by whoever
   launched it. A connected client can never choose, change, or widen it.
3. Exposed capability is derived by narrowing a [spec 31](31_permission_profiles.md)
   profile. No tool requiring approval is exposed, and a profile change can only
   remove tools from the set, never add one.
4. Every call passes through the same path policy, redaction and audit as the
   equivalent call inside the app.
5. Requests from a connected client are untrusted input and are never treated as
   instructions to Damaian.
6. A server process's activity is visible and attributable afterwards in the
   app.
7. Running as a server changes nothing about the app: no shared mutable state,
   no lock contention with a running desktop session on the same repository.

## 4. Non-goals

- **Exposing anything that writes.** No `propose_patch`, no `run_command`, no
  configuration change, no commit. A write needs an approval, an approval needs
  a human, and a server has none. This is the boundary that makes the spec
  small, and it is not an interim limitation to relax later — a writing MCP
  server would need its own approval channel, which is a different spec.
- **Exposing the agent itself.** The server offers Damaian's *knowledge of a
  repository*, not a "run an agent task" tool. A client that could start a turn
  would be starting a billed, mutating process with no approval surface.
- **Remote transports.** Stdio only. An HTTP server is a listening socket with an
  authentication story, and there is no reason to open one to reach a local tool.
- **Multiple repositories per process.** One process, one repository
  (requirement 2). A client wanting two starts two.
- **Replacing the desktop app or the CLI.**
- **Any change to the client half.** [Spec 06](06_mcp_support.md) and
  [spec 33](33_mcp_management_and_deferred_discovery.md) are untouched.

## 5. Design

### 5.1 Entry point

`damaian mcp-serve --repository <path>`, a subcommand of the existing CLI rather
than a fourth binary — it shares configuration loading, the engine construction
and the audit log, and a separate binary would duplicate all three.

The repository comes from the launching argument. It is not in the protocol,
there is no tool that sets it, and a path argument in any tool call is resolved
relative to it through `path_policy.rs` — so a client sending `../../` gets the
same refusal the agent gets. Requirement 2 is enforced by there being no code
path that accepts a repository from the wire.

Configuration loads with the repository's own scope rules intact
([spec 34](34_repository_config_trust_boundary.md)): a repository config cannot
widen what the server exposes, for exactly the reasons it cannot widen what the
agent may do.

### 5.2 The exposed set

| Tool | Why it is safe to expose |
|---|---|
| `search_codebase` | Read-only, path-policed, redacted |
| `read_file` (ranged, per [spec 47](47_agent_working_capability.md)) | Read-only, path-policed, redacted |
| `list_directory` ([spec 47](47_agent_working_capability.md)) | Read-only, path-policed |
| `read_git_status`, `read_git_diff` | Read-only; the diff redacts, and [spec 35](35_commit_preparation.md)'s warning about redacted diffs concerns *committing* from one, which this cannot do |
| Repository map (Phase 3 WP2), symbol lookup (Phase 3 WP3) | Read-only, derived, when they exist |

The set is defined as the intersection of "tools marked read-only" and "tools the
active profile permits" — computed, not listed, so a tool added later is
excluded until someone marks it read-only, rather than included until someone
remembers to exclude it. That default is the whole of requirement 3.

### 5.3 Visibility and audit

A server process opens a session of its own on launch, named for the client
connection, and every call writes an audit record through the same
`AuditLog::record` the app uses, tagged with the transport and the process id.
The result, which is requirement 6: a user who wonders what read their
repository at 14:20 finds it in the same place they would find anything else.

Sessions from server processes are distinguishable in the session list so they
do not clutter the user's own — a distinction
[spec 53](53_session_search_and_export.md) has to accommodate, which is why the
two are written in the same pass.

### 5.4 The client is untrusted, in both directions

[Spec 33](33_mcp_management_and_deferred_discovery.md) establishes that a remote
MCP server's claims about itself cannot lower an approval requirement. The
mirror image holds here and is easy to forget: **a connected client's requests
are data.**

Concretely — a tool call's arguments are values, never instructions; a client
cannot cause Damaian to read outside the repository, start a turn, spend a
token, or write a file; text a client sends is never appended to a prompt; and a
client-supplied name or description never reaches a model. The server's only
output is the result of a read.

Nothing is logged that a client sends without redaction, because a client can
send anything, including something that looks like a credential.

### 5.5 Isolation from a running app

Requirement 7. The server takes no exclusive lock on the data directory; it
appends to the audit log through the same append-only path that already
tolerates concurrent writers, and it does not write session state for the user's
own sessions. The index is read; where a persisted index exists (Phase 3 WP1) it
is opened read-only, and where it does not the server builds its own in memory
rather than writing one another process is reading.

[Spec 46](46_process_registry_and_orphan_sweep/proposal.md) governs process
lifetime. A server started by another tool is that tool's child and is not in
Damaian's registry — it is not Damaian's to sweep, and killing a process the app
did not start is precisely the mistake spec 46 exists to prevent.

### 5.6 Documentation

`docs/USER_GUIDE.md`: how to launch it, what it exposes, and explicitly that it
cannot write or run anything. A worked configuration example for a generic MCP
client.

## 6. Acceptance Criteria

- An MCP client connects over stdio, lists tools, and reads a file and a search
  result from the configured repository.
- The advertised tool list contains no tool that can write, run a command, or
  start a turn — asserted against the computed set, so a newly added tool fails
  the test until classified.
- A tool call with a path outside the repository is refused by `path_policy.rs`,
  identically to the in-app refusal.
- A file containing a seeded secret is returned redacted.
- No protocol message can change the repository, the profile, or any
  configuration value.
- A repository config attempting to widen the exposed set is rejected and
  reported.
- Every call appears in the audit log tagged with transport and process id.
- A server process and a desktop session can run against the same repository at
  once without either failing or corrupting state.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation. Record:

- Which clients were tested against, and any protocol assumption that turned out
  not to be portable.
- Whether the read-only intersection in §5.2 needed an explicit marker on each
  tool or could be derived from an existing property. If a marker was added,
  say where, because that marker is now load-bearing for a security property.
