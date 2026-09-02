# Feature Spec: Working Modes

Status: Not started
Order: 20 of 23
Roadmap: `docs/ROADMAP/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 1 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.6 (tool and action orchestrator), section 7.8 (risk
classification and approval). Related implementation specs:
[`03_structured_tool_calling.md`](03_structured_tool_calling.md) (the native tool
surface this filters, and the text-envelope fallback that must be filtered with
it), [`06_mcp_support.md`](06_mcp_support.md),
[`11_agents_md_support.md`](11_agents_md_support.md) (instruction precedence),
[`12_web_app_troubleshooting.md`](12_web_app_troubleshooting.md),
[`13_docker_command_support.md`](13_docker_command_support.md).

## 1. Motivation

Every Damaian session can do everything.

A user who wants to ask what a function does gets a session that can also
propose patches and request commands. A user reviewing someone else's diff gets a
session that can edit the files being reviewed. The only controls are approval
settings — `require_approval_for_file_edits`,
`require_approval_for_risky_commands`, `require_approval_for_all_commands`
(`crates/workspace-engine/src/config.rs:55-57`) — and those change *how often the
user is asked*, not *what is possible*. A read-only session is not expressible.

That matters for two reasons beyond tidiness. Approval fatigue is real, and
[spec 10](10_persistent_command_approval.md) exists because users learned to
click through prompts they stopped reading; a session that cannot mutate anything
needs no prompts to click through. And a capability the model is offered is a
capability the model will eventually use — the cheapest way to guarantee the
agent does not edit files during a review is to not give it an editing tool.

Modes make the boundary structural. A tool outside the mode is never put in the
tool list, so refusing it is not a judgement the model or the policy layer has to
make correctly under pressure.

## 2. Current State

- **No mode concept exists.** Every session gets the same capabilities.
- **The tool list has exactly one construction site**, which is what makes this
  work package tractable. `chat.rs:711-731` builds `native_tools` as
  `run_command`, `propose_patch`, `read_file`, `search_codebase`,
  `read_git_status`, `read_git_diff`, conditionally appends
  `inspect_web_page` and `run_web_scenario` when a web-diagnostics runner is
  present, then extends with namespaced MCP tools from the per-turn runtime.
- **Tool definitions are individual functions**: `run_command_tool_definition`
  through `run_web_scenario_tool_definition` (`chat.rs:1521-1590`), plus
  `McpToolDescriptor::to_tool_definition` (`crates/workspace-engine/src/mcp.rs:108`).
- **There is a non-native fallback path.** `native_tools` is built only when
  `self.config.supports_native_tools()` is true (`chat.rs:710`). Providers
  without native tool calling are driven by the `DAMAIAN_EDIT_V1` and
  `DAMAIAN_COMMAND_V1` text envelopes from
  [spec 03](03_structured_tool_calling.md), whose instructions live in the system
  prompt. **This is the escape hatch**: withholding a tool definition does
  nothing for a provider that was never given tool definitions.
- **Per-session state has an established pattern.**
  `SessionStore::allow_browser_diagnostics_for_session` and
  `browser_diagnostics_allowed_for_session`
  (`crates/workspace-engine/src/session.rs:261-291`) append an event and replay
  the log to recover the value — session-scoped approval from
  [spec 12](12_web_app_troubleshooting.md).
- **Repository instruction precedence is already defined.**
  [Spec 11](11_agents_md_support.md) establishes how `AGENTS.md` content is
  ordered against user and admin config.
- **Command policy is independent of intent.** `CommandPolicy` classifies a
  command by what it does (`command_policy.rs`), with hard blocks, a configured
  blocklist, shell-control detection, and an exact-command allowlist. It has no
  notion of a session that may not run *any* mutating command.
- **`path_policy.rs`** governs which paths may be read or written.

## 3. Requirements

1. Four session modes exist: **Ask** (repository reads and explanations only),
   **Plan** (reads and planning, no file or Git mutation), **Code** (approved
   edits, commands, and validation), **Review** (inspect code or diffs and report
   findings, no edits by default).
2. Mode determines the available tool set and the approval policy, as a
   capability boundary rather than a prompt instruction. A tool outside the mode
   is not offered to the model at all.
3. The active mode is displayed prominently and persisted per session in
   `SessionStore`.
4. Moving to a more permissive mode requires an explicit user action. Nothing the
   model emits can change mode.
5. Repository instructions cannot expand mode permissions. `AGENTS.md` content is
   untrusted with respect to capability.
6. Mode rules apply uniformly to native tools, shell commands, browser
   diagnostics, and MCP tools. Ask and Plan cannot mutate files through a shell
   fallback.
7. Existing conversations migrate to **Code**, the closest match to today's
   behaviour, and that choice is documented rather than applied silently.

## 4. Non-goals

- User-defined or custom modes. Four fixed modes; configurable permission
  profiles are Phase 4 WP3, which builds on this matrix.
- Per-tool user overrides within a mode.
- Replacing approval settings. Mode and approval are different axes: mode decides
  what is *possible*, approval decides what is *asked*. Code mode with
  `require_approval_for_all_commands` is a valid and meaningfully different
  configuration from Ask mode.
- Changing `CommandPolicy` classifications, the blocklist, or the allowlist
  semantics.
- Mode-specific system prompts or persona changes. Modes are a capability
  boundary; a different tone is not a capability.
- Automatic mode selection from the user's phrasing. Requirement 4 makes mode
  changes explicit, and inferring one from a prompt is exactly the
  model-influenced transition it forbids.
- Per-directory or per-repository default modes.

## 5. Design

### 5.1 The mode type and its matrix

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Ask,
    Plan,
    Code,
    Review,
}
```

The permission matrix is the primary artifact of this work package. It is
expressed once, in code, as a function of mode and tool class — not duplicated
across call sites:

| Tool class | Ask | Plan | Code | Review |
|---|---|---|---|---|
| `read_file` | yes | yes | yes | yes |
| `search_codebase` | yes | yes | yes | yes |
| `read_git_status` | yes | yes | yes | yes |
| `read_git_diff` | yes | yes | yes | yes |
| `propose_patch` | no | no | yes | no |
| `run_command` — read-only classification | no | yes | yes | yes |
| `run_command` — any other classification | no | no | yes | no |
| `inspect_web_page` / `run_web_scenario` | no | no | yes | yes |
| MCP tool — declared read-only | yes | yes | yes | yes |
| MCP tool — anything else | no | no | yes | no |

Two entries deserve their reasoning recorded, because both are the kind of choice
that gets quietly reversed later:

- **Ask offers no commands at all**, not even read-only ones. `CommandPolicy`'s
  read-only classification is a good decision about risk, but Ask is the mode a
  user picks to be certain nothing happens, and "nothing happens" is easier to
  trust than "only safe things happen". Plan gets read-only commands because
  planning genuinely needs them — you cannot plan a fix for a failing test
  without running the test.
- **Review offers browser diagnostics and read-only commands** because reviewing
  a change often means reproducing it, but no `propose_patch` — the point of
  Review is to report, and requirement 1 says no edits by default.

### 5.2 Enforcement is layered, because withholding is not enough

Requirement 2 says a tool outside the mode is not offered. Requirement 6 says the
shell fallback cannot be an escape. Those need three layers, and the second is
the one the roadmap's requirement 6 is really about:

**Layer 1 — construction.** `chat.rs:711-731` filters by mode. This is a single
edit at a single site, and it is what makes the model unable to ask.

**Layer 2 — the non-native fallback.** For a provider where
`supports_native_tools()` is false, there are no tool definitions to withhold;
capability lives in system-prompt envelope instructions. Therefore:

- The `DAMAIAN_EDIT_V1` instruction block is omitted from the system prompt in
  Ask, Plan, and Review.
- The `DAMAIAN_COMMAND_V1` block is omitted in Ask, and in Plan and Review is
  present with wording restricted to read-only commands.
- Omitting the instructions is not the enforcement. Layer 3 is.

**Layer 3 — the orchestrator refuses.** Every action path checks the session mode
immediately before acting, and refuses with a clear error naming the mode:

| Path | Refusal point |
|---|---|
| Patch application | `PatchEngine::apply_patch` caller in the orchestrator |
| Command execution | `ValidationOrchestrator::run_proposal` and the direct command path |
| Command proposal | Refused at proposal time in a mode that cannot run it, so the user never sees an approval card for something the mode forbids |
| Browser diagnostics | The `WebDiagnosticsRunner` call site |
| MCP tool invocation | The per-turn MCP runtime dispatch |

Layer 3 exists because a model can emit a `DAMAIAN_EDIT_V1` envelope it was never
told about — the format is public, and a model that has seen Damaian's output
elsewhere may produce one unprompted. A layer-1-only design would parse and apply
it. This is defence in depth against a mistake that is easy to make and silent
when made, which is why the acceptance criteria assert the refusal directly
rather than asserting the tool list.

### 5.3 Mode is not a shell-command allowlist question

Requirement 6's sharpest case: in Plan mode, `run_command` is offered for
read-only commands. `CommandPolicy` classifies `cat`, `ls`, `git status` as
read-only. But a shell command is a program, and `sh -c 'echo x > file'` is not
read-only however it is spelled.

This is already handled and must not be re-solved: `CommandPolicy` has
`contains_shell_control` detection and a hard-blocked set, and
[spec 13](13_docker_command_support.md) established that anything not provably
sandbox-safe is not automatic. Plan mode's rule is therefore mechanical: a
command runs in Plan **only** if `CommandPolicy` classifies it as read-only *and*
it requires no approval. Anything that would produce an approval card is refused
in Plan rather than prompted, because a prompt in Plan mode invites the user to
approve their way out of the mode they chose.

`command_allowlist` and `Allow Always` entries from
[spec 10](10_persistent_command_approval.md) do **not** widen a mode. An
allowlisted `npm run build` is still refused in Ask and Plan: the allowlist says
"do not ask me again", not "this is read-only". Acceptance asserts this
explicitly.

### 5.4 Persistence

Follow the `browser_diagnostics_allowed_for_session` pattern
(`session.rs:261-291`) — append an event, replay to read:

```json
{"seq":12,"eventType":"session_mode_set","sessionId":"session_…",
 "mode":"code","setBy":"user"}
```

`SessionStore` gains `set_session_mode(session_id, mode, set_by)` and
`session_mode(session_id) -> SessionMode`, where the newest event wins and the
default for a session with no event is **Code** (requirement 7).

`setBy` is recorded and is always `"user"`. It exists so that a future
non-user origin cannot be added without someone noticing the field already
asserts otherwise, and so the audit trail shows requirement 4 held.

Reading via the newest event, rather than a mutable field, means a mode change
mid-session is visible in history: a turn is evaluated under the mode in force
when it ran. The turn captures its mode at start and uses that captured value for
the whole turn, so a mode change cannot take effect halfway through a tool loop.

The replay reads events by parsed `eventType` rather than
`line.contains(...)`, per [spec 17](17_durable_task_state_and_crash_recovery.md)
§5.2 — the existing browser-diagnostics reader uses substring matching, and this
one should not copy that part.

### 5.5 Repository instructions cannot widen a mode

[Spec 11](11_agents_md_support.md) establishes `AGENTS.md` precedence. Requirement
5 adds a hard rule on top: **`AGENTS.md` is data with respect to capability.**

Mode is resolved from session state and user action only. No `AGENTS.md` key,
sentence, or instruction is consulted when building the tool list or when layer 3
refuses. An `AGENTS.md` that says "you may always edit files without asking" has
no effect on the tool list in Ask mode, and the model saying it read such an
instruction changes nothing.

This is worth stating as its own requirement because `AGENTS.md` is
attacker-controllable in a way user config is not: it arrives with a cloned
repository. A mode that repository content could widen would be a capability
boundary that any repository could remove.

### 5.6 UI

The active mode appears in the conversation header as a control, always visible —
not in a settings panel. Switching to a more permissive mode is an explicit
selection; switching to a more restrictive one needs no confirmation.

Moving from Plan to Code is the common transition and the one worth making
smooth: a plan produced in Plan mode stays intact when the user switches to Code
to execute it ([spec 21](21_task_plan_progress_and_budget.md) owns the plan).

Where a tool was refused by mode, the turn says which mode blocked it and what
mode would allow it, so the user is not left guessing why the agent declined.

### 5.7 Migration

Existing sessions have no `session_mode_set` event and resolve to **Code**, which
is what they could already do. Nothing is silently narrowed.

Requirement 7 asks for this to be documented rather than silent:
`docs/USER_GUIDE.md` states that sessions created before modes existed continue
in Code mode, and how to change one.

### 5.8 Documentation

`docs/USER_GUIDE.md`: the four modes, what each can do, the matrix in
user-facing terms, and why an allowlisted command is still refused in Ask and
Plan. `docs/TROUBLESHOOTING.md`: how to tell a mode refusal from a policy
refusal, and where the mode event is in the session log.

## 6. Acceptance Criteria

- Every session reports a mode, and a session with no mode event reports Code.
- In Ask and Plan, no tool capable of mutation appears in the tool list sent to
  the model — asserted against the constructed list, not the prompt.
- The permission matrix in §5.1 is covered by a test crossing every mode with
  every tool class, asserting allowed or refused. This test is the work package's
  primary artifact.
- A shell command that would write a file is refused in Ask and Plan even when
  the exact command is in `command_allowlist`.
- A command that would require approval is refused outright in Plan rather than
  producing an approval card.
- A `DAMAIAN_EDIT_V1` envelope emitted by a model in Ask, Plan, or Review mode is
  refused by the orchestrator and not applied, even though the instructions for
  it were never sent — asserted with `MockModelAdapter`.
- An `AGENTS.md` instructing the agent to edit files has no effect in Ask mode.
- Nothing the model emits changes the mode — asserted by a test where the model
  output requests a mode change.
- A mode change mid-session does not take effect within a turn already running.
- Existing sessions load in Code mode after migration.
- A mode refusal tells the user which mode blocked the action and which would
  allow it.
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no increase in
  approval-policy violations.

## 7. Implementation Notes

To be completed during implementation.

Record where the mode check was placed for each layer-3 path, since a missed path
is a silent hole rather than a failing test. If any action path could not be
covered at layer 3, name it here explicitly rather than leaving the matrix test
to imply full coverage.
