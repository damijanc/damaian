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
