# Context: Working Modes

Background for [`tasks.md`](tasks.md). Corrections to `proposal.md` (the
flat spec, unchanged in substance from its original text) where its "Current
State" section and §5.1 matrix no longer match the code. Read this before
Task 1.

## 1. The tool inventory has grown since the flat spec was written

`proposal.md` §2 lists six native tools plus the two web-diagnostics ones.
The actual construction site
(`crates/workspace-engine/src/chat.rs:1267-1293`, inside
`run_agentic_turn`) builds eleven:

```
run_command, propose_patch, propose_plan, complete_step, read_file,
list_directory, search_content, edit_file, search_codebase,
read_git_status, read_git_diff
```

plus `inspect_web_page` / `run_web_scenario` when a web-diagnostics runner
is present, plus namespaced MCP tools. `propose_plan` and `complete_step`
came from spec 21 (Task Plan, Progress, and Budget); `list_directory`,
`search_content`, and `edit_file` came from a later agent-capability slice.
Both landed after spec 20 was written and before spec 20 was built — the
"every session gets the same capabilities" sentence in §2 is still true
today, which is exactly why nothing gated them.

The authoritative source for "what tools exist" is not §2's prose list, it
is `enum ToolAction` (`chat.rs:3237-3278`), which is exhaustive by
construction — every arm the tool-call parser (`chat.rs` around line 3600
onward) produces is a variant of it, and the compiler's match-exhaustiveness
check is what will catch a twelfth tool arriving later without the matrix
being told. Task 1 matches on `ToolAction` directly for this reason, rather
than introducing a second "tool class" enum that duplicates it and can drift
independently, the same trap `proposal.md` §2's own list fell into.

**Where each current tool lands in §5.1's matrix, decided here because the
flat spec cannot have decided it — these tools didn't exist yet:**

| `ToolAction` variant | Class | Reasoning |
|---|---|---|
| `ReadFile`, `ListDirectory`, `SearchContent`, `SearchCodebase`, `ReadGitStatus`, `ReadGitDiff` | Read — all four modes | Same "no mutation, no approval" shape as the six tools §5.1 already covers. `list_directory`'s own tool description ("needs no approval") and `search_content`'s ("find call sites") are read-only by construction — see their dispatch, all under `dispatch_read_only_action` (`chat.rs:2738`). |
| `ProposePatch`, `EditFile` | Mutation proposal — Code only | `EditFile` is not a second `run_command`-shaped risk, it is a second *shape* of the same capability `ProposePatch` already has in the matrix: both call `self.patch_engine.create_patch(...)` and `self.patch_store.save(...)` (`chat.rs:2019-2058` for `ProposePatch`, `chat.rs:2195-2210` for `EditFile`) and produce the same `ProposedPatch` awaiting the same user approval. §5.1's `propose_patch` row is read as "mutation proposal", covering both. |
| `ProposePlan`, `CompleteStep` | Planning — Plan and Code | Requirement 1 defines Plan as "reads and planning"; these are the planning primitives that didn't exist when that sentence was written. Ask is explicitly reads-and-explanations-only, so no planning tool. Review is explicitly report-only with no forward-looking work product, so no planning tool either — a plan is future work, which is what Review exists to not do. Code keeps them so a plan made in Plan mode can keep progressing after the mode switch §5.6 describes ("a plan produced in Plan mode stays intact when the user switches to Code to execute it") — the plan surviving the switch is spec 21's job (`SessionStore`), but the *tool to keep completing steps* has to still be offered in Code for that continuity to mean anything. |
| `Command(CommandRequest)` | Unchanged — §5.1's `run_command` row already covers it exactly | No drift here. |
| `WebDiagnostic(_)` | Unchanged — §5.1's `inspect_web_page`/`run_web_scenario` row | No drift. |
| `McpCall { .. }` | Unchanged in principle, blocked in practice — see §2 below | The matrix's rule is right; nothing implements the fact it depends on yet. |

## 2. "MCP tool — declared read-only" has nothing to read

§5.1's matrix has a row for it, and §5.2 Layer 1 needs to evaluate it while
building the tool list. But `McpTool` (`crates/workspace-engine/src/mcp.rs:102-108`)
carries only `name`, `description`, and `input_schema_json`. There is no
`read_only` field, and `McpClient::list_tools` (`mcp.rs:178-203`) does not
parse one from the server's `tools/list` response — it reads `name`,
`description`, and `inputSchema` and discards everything else in each
tool object.

The Model Context Protocol itself defines this as an optional
`annotations.readOnlyHint: bool` on a tool description (servers that don't
set it are making no claim either way — the field's own semantics are a
*hint*, not a guarantee, which matters for requirement 2's "capability
boundary rather than a prompt instruction": a hint from a third-party server
is not something this spec can treat as authoritative for what a boundary
enforces). This repository has never parsed it.

**Decision, recorded here rather than assumed:** Task 2 adds
`read_only_hint: Option<bool>` to `McpTool`, parsed from
`item.get("annotations").and_then(|a| a.get("readOnlyHint")).and_then(Value::as_bool)`
in `list_tools`. The matrix's rule becomes: an MCP tool is offered in Ask
and Plan only when the server annotated it `readOnlyHint: true`; a server
that omits the annotation, or sets it `false`, is "anything else" —
mutation-class, Code only. This is the same "silence is not a green light"
posture spec 49 used for cache reporting (absence of a signal is not
treated as the favorable case), applied here to a capability boundary
rather than a cost figure, which is the higher-stakes direction to apply it
in. This is a small, self-contained addition — one field, one parse site —
not a re-opening of MCP support (spec 06), so it stays inside this work
package rather than becoming its own spec.

## 3. Layer 3's table names one refusal point where there are effectively two

§5.2's Layer 3 table lists "Patch application" → "`PatchEngine::apply_patch`
caller in the orchestrator" as the refusal point for the mutation-proposal
class. That call site (`crates/workspace-engine/src/edit.rs:538`) is real,
but it fires when the user **approves a already-created proposal** —
a completely different call path (patch review/apply flow) from the one
that creates the proposal in the first place.

Both `ProposePatch` and `EditFile` create and *store* a `ProposedPatch`
(`patch_store.save`, `chat.rs:2019-2058` and `chat.rs:2195-2210`) before any
approval exists. Refusing only at `apply_patch` would let a session in Ask
or Plan mode successfully create and persist a patch proposal that then sits
refused-at-the-last-step — a confusing halfway state, and one where Layer 1
(withholding the tool definition) is the *only* thing standing between a
model that emits an unprompted `propose_patch`/`edit_file` tool call (native
path) or `DAMAIAN_EDIT_V1` envelope (fallback path, requirement 6) and a
proposal actually landing in the patch store.

**Decision:** Layer 3 for this class checks mode at *both* points:
1. `chat.rs`'s `ToolAction::ProposePatch` and `ToolAction::EditFile` arms,
   before `create_patch` is called — refuses before a proposal is ever
   written.
2. `edit.rs:538`'s `apply_patch` caller, as the flat spec already says —
   a second, independent gate, defence in depth for the case where a
   proposal exists for some other reason (a prior mode, a resumed session)
   and the mode changed underneath it (§5.4's "captured value for the whole
   turn" rule covers the ordinary case; this covers the case where the
   proposal predates the turn entirely, e.g. resume-after-restart).

Both belong to Task 3 (Layer 3 wiring); recorded here so Task 3 does not
"discover" only the first one and call the class covered.

## 4. §5.4's borrowed pattern is cleaner than the flat spec's own caveat

§5.4 warns the existing `browser_diagnostics_allowed_for_session` reader
"uses substring matching, and this one should not copy that part." Reading
`crates/workspace-engine/src/session.rs:680-695` directly: it already
filters by the parsed `SessionEvent.event_type` field
(`event.event_type != "browser_diagnostics_approval_updated"`), an exact
`String` comparison, not `line.contains(...)`. There is no substring
matching left to avoid. Whatever prompted that caveat has already been
fixed elsewhere; Task 4 follows the pattern as it exists today with no
special care needed beyond what §5.4 already specifies (append an event,
replay by parsed `eventType`, newest wins).

## 5. Nothing here changes what a command's risk classification means

`CommandClassification` (`command_policy.rs:24-32`) has `risk: CommandRisk`
(`Low`/`Medium`/`High`/`Blocked`) and `requires_approval: bool` as separate
fields — confirmed reading `classify()` (`command_policy.rs:60`) and
`is_low_risk_read_only` (`command_policy.rs:293`). §5.3's rule — "a command
runs in Plan only if `CommandPolicy` classifies it as read-only *and* it
requires no approval" — reads directly as
`classification.risk == CommandRisk::Low && !classification.requires_approval`.
No change to `command_policy.rs` itself is in scope (Non-goals §4), Task 3
only reads these two fields.

## 6. Open question carried into Task 1, not resolved here

The matrix as extended above has six classes (Read, Mutation proposal,
Planning, Command, Web diagnostics, MCP) crossed with four modes — the
`proposal.md` §5.1 table already exists in prose form above and in the
spec; Task 1 turns it into a function and a test that crosses every
`ToolAction` variant (not every "class" — the class is what groups variants
that get the same answer, not a separate thing the function branches on)
with every mode. Whether the function signature takes the whole
`ToolAction` or a smaller derived key is Task 1's own decision to make and
record, the way spec 49 Task 2 decided `extract_usage`'s return shape.

## 7. `system_prompt()` does not teach `DAMAIAN_EDIT_V1` — there is nothing
## for Layer 2 to omit

`proposal.md` §5.2 Layer 2 says: "The `DAMAIAN_EDIT_V1` instruction block is
omitted from the system prompt in Ask, Plan, and Review." Reading
`system_prompt()` (`chat.rs:3231-3232`, one hardcoded string, no
parameters): it contains the `DAMAIAN_COMMAND_V1` block and its guidance
paragraph, and nothing else — no `DAMAIAN_EDIT_V1` instructions anywhere.
Confirmed by grepping the whole file: the only two occurrences of
`DAMAIAN_EDIT_V1` in `chat.rs` are a doc comment and a string search inside
a different function; `run_agentic_turn`'s response-handling path never
calls `parse_generated_edit` at all — only `parse_command_request`
(`chat.rs:4048`). **A non-native-tool-calling provider inside an ordinary
chat turn currently has no way to propose a file edit, in any mode.** The
only text-envelope mutation capability the chat loop has is
`DAMAIAN_COMMAND_V1`.

`DAMAIAN_EDIT_V1` is real, but it belongs to a different, older, and
entirely separate feature: `EditOrchestrator::propose_edit`
(`edit.rs:293`), with its own dedicated system prompt (`edit.rs:841`) and
its own call to `parse_generated_edit`. It is reached through
`POST /api/propose-edit` (`desktop-shell/src/lib.rs:750`) and the CLI's
`propose-edit` subcommand (`damaian-cli/src/main.rs:406,427`) — a one-shot
endpoint that takes `repo`, `prompt`, and `context_files` and returns a
patch directly. **It has no session id and no session mode to read.** This
is not an oversight this work package can fix by threading a parameter
through — it is a structurally different flow that predates the concept of
a session at all.

**Decisions, recorded here rather than assumed:**

- Task 5 (Layer 2) is scoped to `DAMAIAN_COMMAND_V1` only. There is no
  `DAMAIAN_EDIT_V1` teaching to omit in the chat loop's system prompt,
  because none exists in any mode today. Do not add one — that would be
  new capability, out of this work package's "changes no request" spirit
  (borrowed from spec 49's own framing) applied to prompt content.
- Task 6 (Layer 3)'s acceptance criterion — "a `DAMAIAN_EDIT_V1` envelope
  emitted by a model in Ask, Plan, or Review mode is refused by the
  orchestrator and not applied, even though the instructions for it were
  never sent" — is, in the chat loop, **currently true by accident**: the
  envelope is not parsed at all, so nothing applies it, in any mode,
  today. Task 6 should turn this into an intentional, tested guarantee
  (assert a `MockModelAdapter` response containing a `DAMAIAN_EDIT_V1`
  block produces no patch and no mutation, in every mode including Code)
  rather than leave it resting on the absence of a feature — the day
  something *does* wire `parse_generated_edit` into `run_agentic_turn`,
  an untested accident becomes a real hole with nothing guarding it.
- `EditOrchestrator::propose_edit` and `/api/propose-edit` are **out of
  scope for this work package.** It has no session, so it has no mode to
  enforce, by construction — extending it to accept and honor a session
  mode would be new design work (does an edit proposed outside any
  session inherit a mode at all? from where?) that `proposal.md` never
  anticipated and this plan's tasks do not cover. Flag this explicitly in
  Task 10's documentation and implementation notes rather than letting a
  reader assume it was checked and found fine — it was found *out of
  scope*, a different thing. Whether it needs its own follow-up spec is a
  product decision, not one this plan makes silently by omission.

## 8. Layer 3 is nine call sites, not the flat spec's five or §3's six

Tracing every path that actually executes a mutating action (not just
where one is *proposed*), planning against the current `chat.rs` (post
Task 5) found two more gaps than §3 already corrected.

**Gap A — `proposal.md` §5.1's table has no row for planning at all.**
`propose_plan`/`complete_step` didn't exist when the flat spec was written
(§1 above). Their dispatch arms — `ToolAction::ProposePlan` (`chat.rs`,
around line 2168 as of Task 5) and `ToolAction::CompleteStep` (around line
2221) — are a fifth main-loop refusal point Layer 3 must add, alongside the
four the flat spec names (mutation-proposal creation, command, web
diagnostic, MCP call).

**Gap B — three of the five main-loop refusal points have a second,
independent execution path that never touches `run_agentic_turn`'s `mode`
variable at all.** `resume_after_command_decision_with_options`
(`chat.rs`, around line 752 as of Task 5) is the single entry point a
user's approval or decline of a *paused* action resumes through — and,
despite its name, it handles three different kinds of pending action, not
just commands, distinguished by which optional field `PendingChatTurn`
carries:

- `pending.web_diagnostic_call` — calls `self.run_web_diagnostic_call(&call)` directly (around line 805).
- `pending.mcp_call` — calls `mcp.call_tool(...)` directly (around line 818).
- neither — the original shell-command path — calls `self.validation_orchestrator.run_proposal(...)` directly (around line 866).

None of these three branches is inside `run_agentic_turn`; this function
never calls it. `mode` is not in scope here by any means Task 4/5 already
established. This is the resume-path twin `proposal.md` §5.2's "Command
execution" row already gestures at with "`ValidationOrchestrator::run_proposal`
**and the direct command path**" — but the flat spec did not know the same
function also resumes MCP and web-diagnostic approvals, so it only named
the command half.

**Decision, recorded here rather than assumed: check mode at resume time,
not at proposal time.** A paused action was classified and offered under
the mode active when the model first requested it, but approving or
declining it is itself a new, separate user action that can happen
arbitrarily later — long enough for the user to have switched modes in the
meantime. Re-reading `self.session_store.session_mode(&pending.session.id)`
fresh at the top of `resume_after_command_decision_with_options` and
refusing before any of the three branches acts is the reading consistent
with requirement 2's "capability boundary rather than a prompt instruction":
the boundary is a property of the session's *current* configuration, and a
mutation that would be refused if requested right now should not become
approvable merely because it was requested earlier under a laxer mode. This
is a stricter reading than "a turn captures its mode at start" strictly
requires (that rule is about a mode change not preempting a turn already
mid-flight synchronously) — resuming after a human pause is not "mid-flight",
it is a new decision point, and this plan treats it as one.

**Complete list of refusal points for Task 6**, superseding both
`proposal.md` §5.2's original table and §3 above:

| # | Path | Location | Mode value |
|---|---|---|---|
| 1 | Mutation-proposal creation (`propose_patch`/`edit_file`) | `chat.rs`, `ToolAction::ProposePatch`/`ToolAction::EditFile` arms, before `create_patch` | `run_agentic_turn`'s `mode` |
| 2 | Mutation-proposal apply | `edit.rs`, `apply_stored_patch`, before `self.patch_engine.apply_patch(...)` | fresh read from `patch.session_id` |
| 3 | Command proposal + execution (unified) | `chat.rs`, `ToolAction::Command` arm, immediately after `propose_command` returns, before either the approval-card branch or the auto-run branch | `run_agentic_turn`'s `mode` |
| 4 | Planning (`propose_plan`/`complete_step`) | `chat.rs`, `ToolAction::ProposePlan`/`ToolAction::CompleteStep` arms | `run_agentic_turn`'s `mode` |
| 5 | Web diagnostic (main loop) | `chat.rs`, `ToolAction::WebDiagnostic` arm | `run_agentic_turn`'s `mode` |
| 6 | MCP call (main loop) | `chat.rs`, `ToolAction::McpCall` arm | `run_agentic_turn`'s `mode` |
| 7 | Command (resume) | `resume_after_command_decision_with_options`, the no-`web_diagnostic_call`-no-`mcp_call` branch, before `run_proposal` | fresh read from `pending.session.id` |
| 8 | Web diagnostic (resume) | same function, `pending.web_diagnostic_call` branch, before `run_web_diagnostic_call` | fresh read from `pending.session.id` |
| 9 | MCP call (resume) | same function, `pending.mcp_call` branch, before `mcp.call_tool` | fresh read from `pending.session.id` |

Points 7-9 share one function and one fresh mode read — reading mode once
at the top of `resume_after_command_decision_with_options` and reusing it
across whichever one of the three branches actually runs is one read, not
three, even though the table lists three refusal points for it.

**Read-only actions (`read_file`, `list_directory`, `search_content`,
`search_codebase`, `read_git_status`, `read_git_diff`) need no Layer 3
check anywhere** — `mode_permits` returns `Allowed` for all four modes on
every one of them (Task 1's crossing test already pins this), so a check
there would always pass and add nothing; Task 6 does not add one.
