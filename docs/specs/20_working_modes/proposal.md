# Feature Spec: Working Modes

Status: Done (2026-09-25). Split into a folder and planned on 2026-09-23,
built in ten tasks from 2026-09-23 to 2026-09-25. The design is unchanged from
the original flat spec. Its corrections to "Current State" and §5.1's matrix
(the tool inventory grew after this spec was written and before it was
built) are in [`context.md`](context.md), not inlined here, the way spec 49
kept its own corrections separate. §7 below records what was built, the two
real bugs planning found, and the gaps left open. Read it before relying on
§5.2 or §6.
Order: 20 of 23
Plan: `docs/PLAN/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 1 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#16](../16_session_checkpoints_and_rewind.md) (checkpoints) — built;
[#17](../17_durable_task_state_and_crash_recovery/proposal.md) (durable task
state) — built; [#18](../18_local_evaluation_harness/proposal.md) (the harness) —
built; [#19](../19_token_and_cost_accounting/proposal.md) (token accounting) —
built. Everything else named below is a cross-reference, not a prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.6 (tool and action orchestrator), section 7.8 (risk
classification and approval). Related implementation specs:
[`03_structured_tool_calling.md`](../03_structured_tool_calling.md) (the native tool
surface this filters, and the text-envelope fallback that must be filtered with
it), [`06_mcp_support.md`](../06_mcp_support.md),
[`11_agents_md_support.md`](../11_agents_md_support.md) (instruction precedence),
[`12_web_app_troubleshooting.md`](../12_web_app_troubleshooting.md),
[`13_docker_command_support.md`](../13_docker_command_support.md).

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
[spec 10](../10_persistent_command_approval.md) exists because users learned to
click through prompts they stopped reading; a session that cannot mutate anything
needs no prompts to click through. And a capability the model is offered is a
capability the model will eventually use — the cheapest way to guarantee the
agent does not edit files during a review is to not give it an editing tool.

Modes make the boundary structural. A tool outside the mode is never put in the
tool list, so refusing it is not a judgement the model or the policy layer has to
make correctly under pressure.

## 2. Current State

> The tool list below is what existed when this spec was written. Five more
> native tools (`propose_plan`, `complete_step`, `list_directory`,
> `search_content`, `edit_file`) landed after, from specs built in the
> meantime, and before this one. [`context.md`](context.md) §1 has the
> current, authoritative list and where each one lands in §5.1's matrix.
> Treat the six-tool list and the `chat.rs`/`mcp.rs` line numbers below as
> history, not current fact — re-read the actual files before relying on
> either.

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
  [spec 03](../03_structured_tool_calling.md), whose instructions live in the system
  prompt. **This is the escape hatch**: withholding a tool definition does
  nothing for a provider that was never given tool definitions.
- **Per-session state has an established pattern.**
  `SessionStore::allow_browser_diagnostics_for_session` and
  `browser_diagnostics_allowed_for_session`
  (`crates/workspace-engine/src/session.rs:261-291`) append an event and replay
  the log to recover the value — session-scoped approval from
  [spec 12](../12_web_app_troubleshooting.md).
- **Repository instruction precedence is already defined.**
  [Spec 11](../11_agents_md_support.md) establishes how `AGENTS.md` content is
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
across call sites. This table is as originally written; [`context.md`](context.md)
§1 extends it with the five tools that didn't exist yet (`propose_plan`,
`complete_step` join a new "planning" row between `read_git_diff` and
`propose_patch`, Plan and Code only; `list_directory` and `search_content`
join the read row above; `edit_file` joins the `propose_patch` row as the
same "mutation proposal" class) — read both together, not this table alone:

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
[spec 13](../13_docker_command_support.md) established that anything not provably
sandbox-safe is not automatic. Plan mode's rule is therefore mechanical: a
command runs in Plan **only** if `CommandPolicy` classifies it as read-only *and*
it requires no approval. Anything that would produce an approval card is refused
in Plan rather than prompted, because a prompt in Plan mode invites the user to
approve their way out of the mode they chose.

`command_allowlist` and `Allow Always` entries from
[spec 10](../10_persistent_command_approval.md) do **not** widen a mode. An
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
`line.contains(...)`, per [spec 17](../17_durable_task_state_and_crash_recovery/proposal.md)
§5.2 — the existing browser-diagnostics reader uses substring matching, and this
one should not copy that part.

### 5.5 Repository instructions cannot widen a mode

[Spec 11](../11_agents_md_support.md) establishes `AGENTS.md` precedence. Requirement
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
to execute it ([spec 21](../21_task_plan_progress_and_budget/proposal.md) owns the plan).

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
- Every quality-gate command from `AGENTS.md` passes, and the
  [spec 18](../18_local_evaluation_harness/proposal.md) baseline shows no increase
  in approval-policy violations.

## 7. Implementation Notes

Built in ten tasks; [`tasks.md`](tasks.md)'s Progress table has the per-task
detail. This section is the summary a security-focused reader needs without
re-reading those rows.

### 7.1 Where each layer lives

- **The matrix is one function.** `mode_permits(mode, &ToolAction,
  Option<&CommandClassification>, mcp_read_only: Option<bool>) -> Permission`
  in `crates/workspace-engine/src/mode.rs`. It matches on `ToolAction`
  directly, so a new tool variant fails to compile until the matrix is told
  about it. `context.md` §1's five extra tools are in it: `list_directory` and
  `search_content` are reads, `edit_file` is in the mutation-proposal class
  with `propose_patch`, and `propose_plan`/`complete_step` are planning (Plan
  and Code).
- **MCP read-only** is parsed from the tool's `annotations.readOnlyHint`
  (`mcp.rs`, `parse_mcp_tool`). Only `Some(true)` counts. A server that says
  nothing is treated as mutation-class.
- **Persistence**: `SessionStore::set_session_mode` / `session_mode`
  (`session.rs`). The event is `session_mode_set`, the newest one wins, and
  the default is Code. It is read from `parsed_events`, not `active_events`, so
  a rewind does not reset the mode.
- **Layer 1**: `run_agentic_turn` reads the mode once and filters *every* tool
  definition through `mode_permits`, reads included. The browser-MCP filter
  and the mode filter are one closure. `run_command` is offered when a
  best-case classification of `pwd` would be permitted, so Plan and Review
  still see it.
- **Layer 2**: `system_prompt(mode)`. Code is byte-identical to the
  pre-spec prompt, so both spec 49 prompt-cache guards passed unmodified. Ask
  drops the `DAMAIAN_COMMAND_V1` paragraph. Plan and Review keep it but say a
  command needing approval is refused, not queued. There was no
  `DAMAIAN_EDIT_V1` block to omit (`context.md` §7).
- **Layer 3**: nine refusal points (`context.md` §8), not the five §5.2 names:

  | # | Path | Where it landed |
  |---|---|---|
  | 1, 3, 4, 5, 6 | Mutation proposal, command, planning, web diagnostic, MCP (main loop) | One `action_permission` check at the top of `run_agentic_turn`'s per-call loop, before any `match` arm runs and before spec 21's plan-review gate. A command is classified with `classify_command` (no stored proposal) before `propose_command`, so a refusal leaves no proposal, no approval card and no run. |
  | 2 | Stored-patch apply | `edit.rs`, `apply_stored_patch`, before the apply marker and snapshot, with a fresh read of `patch.session_id`'s mode. Refuses with `ClientError::AccessDenied`, not `PolicyBlocked`. |
  | 7, 8, 9 | Resume of a paused command, web diagnostic, MCP call | `resume_after_command_decision_with_options`, one fresh mode read for all three branches. Checked at resume time, not at proposal time (`context.md` §8). |

  One main-loop site rather than five per-arm checks was a deliberate
  deviation: a future `ToolAction` variant cannot reach its arm unchecked.
  Read-only actions get no Layer 3 check, because the matrix allows them in
  every mode.
- **UI**: a `<select>` in the thread header (`app.js`, `renderModeControl`),
  `POST /api/session-mode`, and `"mode"` on the single-session payloads. The
  session *list* deliberately does not carry the mode, since that would read
  every session log twice. A refusal reaches the user as the existing grey
  tool-result bubble; no new element was needed.

### 7.2 Two real bugs this planning pass found and fixed

Neither was anticipated by the flat spec. Both would have shipped as holes in
a security claim.

1. **An allowlisted command widened Plan and Review** (`context.md` §5, Task 7).
   `classify_pattern`'s allowlist branch and its genuinely-read-only branch
   produce the same `risk: Low, requires_approval: false`, so `mode_permits`
   could not tell `Allow Always` on `npm run build` from `git status`. §5.3's
   rule, read as those two fields, let an allowlisted mutating command run in
   Plan and Review. Fixed by also requiring
   `command_policy::is_low_risk_read_only(&command)`, a predicate over the
   command text alone that the allowlist cannot influence. No classification
   `CommandPolicy` produces changed.
2. **A mode-refused command was invisible to the audit log** (`context.md` §9,
   Task 9). A human decline at resume called `reject_proposal`. A mode refusal
   at resume did nothing to the stored proposal. So if that id was later run
   (see 7.3), `approval_policy_violations` could not count it: the metric pairs
   `stored_command_rejected` with `stored_command_executed`, and no rejection
   existed. Fixed by calling `reject_proposal(id, "mode_policy")` in the
   mode-refusal branch too.

### 7.3 Paths not covered, named rather than implied

- **A rejected proposal can still be run by id**
  (`OBSERVATIONS.md` #10, open). `run_proposal` never checks rejected state,
  and `/api/run-command`'s standalone branch has no session to read a mode
  from. 7.2's fix makes such a run *visible* as a violation. It does not
  prevent it.
- **`git diff|log|show --output=<file>` passes the read-only check**
  (`OBSERVATIONS.md` #11, open). `is_low_risk_read_only` matches by prefix, so
  in Plan and Review this writes a file inside the repository. It predates
  this spec (Code auto-runs it too), but 7.2's fix made that predicate the
  mode's read-only signal. Closing it is a `CommandPolicy` classification
  change, which §4 rules out without its own spec change.
- **The desktop app's edit-request shortcut ignored the mode. Fixed after
  close-out, on 2026-09-25.** Found in Task 10: a prompt `looksLikeEditRequest`
  matches (`app.js`) goes to `/api/propose-edit` instead of a chat turn. At the
  time, `EditOrchestrator::propose_edit` always created its own new session,
  which reads as Code, so an Ask-mode conversation could still yield an
  applicable patch preview. `context.md` §7 ruled `propose_edit` out of scope
  as having "no session", which undersold it. Now `app.js` sends the open
  `session_id`, and `propose_edit` takes it as `origin_session_id`. An unknown
  id is an `InvalidInput`, because it would otherwise read as Code. The origin
  session's mode is checked before any session, checkpoint or model call
  exists. The proposal still writes into its own edit session, so your
  conversation's history is untouched. The origin is stored on the patch as
  `ProposedPatch::origin_session_id`, which bumped the stored format to
  `DAMAIAN_STORED_PATCH_V3`; V1 and V2 still load. `apply_stored_patch`
  re-checks both sessions, so switching the conversation to Ask after the
  preview appears refuses the apply, the same as a chat patch (layer 3 point
  2). Tests in `tests/foundation.rs`:
  `propose_edit_from_an_ask_or_plan_session_is_refused_before_the_model_is_called`,
  `an_edit_patch_is_refused_at_apply_after_its_origin_session_switches_to_ask`
  and `propose_edit_rejects_an_unknown_origin_session`; V2 compatibility is in
  `tests/session_rewind.rs`. **Still open:** the CLI's `propose-edit` has no
  session, so no mode applies to it.
- **Smaller, recorded in the Task 6 and Task 9 rows**: a refused call is still
  bracketed by `start_action`/finish as a failed attempt; a refused web
  diagnostic still widens that turn's round limit (`web_debug_mode`); and the
  mode-refusal `stored_command_rejected` event says `actor: user`, because
  `reject_proposal` hardcodes it, with only `rejectedBy` saying `mode_policy`.

### 7.4 Acceptance criteria (§6), each with the test that covers it

Tests are in `crates/workspace-engine/src/` unless marked `desktop-shell`.

| Criterion | Covered by | Verdict |
|---|---|---|
| Every session reports a mode; no event reads as Code | `session::tests::a_session_with_no_mode_event_reads_as_code`, `set_session_mode_round_trips`, `the_newest_mode_event_wins`; `desktop-shell` `session_json_includes_the_mode` | Met |
| In Ask and Plan, no mutation-capable tool is in the constructed list | `chat::mode_tool_list_tests::ask_mode_offers_no_run_command_tool_definition_at_all`, `plan_mode_offers_run_command_but_not_propose_patch_or_edit_file`, `an_mcp_tool_without_a_read_only_hint_is_withheld_in_ask` (asserted against `adapter.requests[0].tools`) | Met, with one reading stated: Plan is offered `run_command` because §5.1 gives it read-only commands. The mutating half of that tool is refused at Layer 3, not withheld. |
| The matrix is crossed with every mode and tool class | `mode::tests::the_permission_matrix_matches_the_spec_table` (every `ToolAction` variant × four modes; mutation-tested in Task 1), plus the command and MCP sub-cases in `mode::tests` | Met |
| A file-writing command is refused in Ask and Plan even when allowlisted | `chat::mode_refusal_tests::an_allowlisted_command_that_writes_a_file_is_refused_in_ask_and_plan` (end to end through a real `command_allowlist`; added in Task 10 and mutation-tested against 7.2's fix), `mode::tests::an_allowlisted_mutating_command_is_still_refused_in_plan` / `_in_review` | **Met for the allowlist, not for every file-writing command.** `git diff --output=x` still runs in Plan (7.3, #11). |
| An approval-needing command is refused outright in Plan, with no card | `chat::mode_refusal_tests::plan_mode_still_refuses_a_command_needing_approval_outright`, `mode::tests::plan_refuses_a_command_that_would_require_approval_even_if_low_risk` | Met |
| A `DAMAIAN_EDIT_V1` envelope in Ask, Plan or Review is not applied | `chat::mode_refusal_tests::a_damaian_edit_v1_envelope_emitted_unprompted_is_never_applied_in_any_mode` (`MockModelAdapter`, all four modes) | Met, but by absence: the chat loop never parses the envelope (`context.md` §7). The test guards the day someone wires it in. It was not mutation-tested, since that would mean wiring it in. |
| An `AGENTS.md` instructing edits has no effect in Ask | `chat::mode_refusal_tests::an_agents_md_granting_edits_has_no_effect_in_ask_mode` (asserts the instruction reached the model, the tool list, the refusal and the mode). Added in Task 10: no earlier task had a test for this bullet. Mutation-tested by letting an `AGENTS.md` force Code. | Met |
| Nothing the model emits changes the mode | `chat::mode_refusal_tests::nothing_the_model_emits_changes_the_session_mode` | Met |
| A mid-session change does not take effect within a running turn | `chat::mode_refusal_tests::a_mode_change_mid_turn_does_not_affect_the_tool_round_already_in_progress` (mutation-tested in Task 6) | Met |
| Existing sessions load in Code after migration | `session::tests::a_session_with_no_mode_event_reads_as_code`; checked in the browser in Task 8 on a session with no mode event | Met |
| A refusal names the blocking and the allowing mode | `mode::tests::a_refusal_names_the_blocking_mode_and_the_permitting_mode`, `a_refusal_message_names_both_modes_in_the_words_a_person_reads`; every `mode_refusal_tests` case via `assert_refused`; `desktop-shell` `a_refused_call_reaches_the_model_and_the_session_payload` | Met |
| The quality gate passes, and the baseline shows no increase in approval-policy violations | The seven-command gate and the deterministic tier on 2026-09-25 (Task 10's Progress row); `chat::mode_refusal_tests::a_command_refused_at_resume_is_marked_rejected_in_the_audit_log` and `a_mode_refused_proposal_that_is_later_run_by_id_counts_as_an_approval_policy_violation` | Met: 0 violations in all 16 scenarios, as in `evals/baseline.json`. Every deterministic scenario runs in the default Code mode, so the baseline shows modes changed nothing there. It does not exercise a non-Code mode. What Task 9 earned is that a mode-refused command which later runs now *counts*, so the metric can move. Before Task 9 it could not. |

**`evals/baseline.json` was not regenerated.** No scenario sets a mode or
references one, no commit in this slice touched `evals/` or
`crates/eval-harness`, and Code's prompt and tool list are byte-identical to
before. The Task 9 and Task 10 runs matched every recorded number except
wall-clock latency.
