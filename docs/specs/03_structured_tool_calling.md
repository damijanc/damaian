# Feature Spec: Structured Tool-Calling for Patch and Command Actions

Status: Not started
Order: 3 of 5
Related spec sections: `ai_coding_assistant_specification.md` §7.5 (Model Adapter — "Support tool/function-calling workflows"), §7.6 (Tool and Action Orchestrator — full action list).

## 1. Motivation

The spec's Model Adapter component (§7.5) requires "support tool/function-calling workflows," and the Tool and Action Orchestrator (§7.6) lists a full action surface the model should be able to request: read file, search codebase, propose patch, apply patch, propose command, run command, read Git status, read Git diff, generate commit message.

Today only **one** of these — running a command — is exposed to the model as a real, provider-native tool-call schema:

- `run_command_tool_definition()` (`crates/workspace-engine/src/chat.rs:583`) is the only `ToolDefinition` offered to the model, and only when `Config::supports_native_tools()` is true.
- Every other action — proposing a file edit — is not a tool call at all. It's a **free-text convention** the model must format exactly right: a `DAMAIAN_EDIT_V1` header, followed by `SUMMARY:`, then repeated `FILE:` / `STATUS:` / `CONTENT:` / `END_FILE` blocks, parsed by hand in `parse_generated_edit` (`crates/workspace-engine/src/edit.rs:374-430`).
- Even command requests have a **parallel legacy path**: when native tools aren't supported, the system prompt (`chat.rs:518`) instructs the model to emit a `DAMAIAN_COMMAND_V1` text envelope (`COMMAND:` / `REASON:` / `END_COMMAND`), parsed by `parse_command_request` (referenced at `chat.rs:333`, envelope matched at `chat.rs:611`).

This dual-path design (native tool schema *or* hand-parsed text envelope) means: (a) any model that free-forms slightly different formatting silently fails to produce a usable edit or command, with no schema validation or clear error back to the model, and (b) the file-edit workflow has **no tool-call path at all**, even for providers that fully support function-calling — it always goes through brittle text parsing regardless of `supports_native_tools()`.

## 2. Goals

- Define native tool schemas (JSON Schema, matching whatever shape `ModelAdapter`'s `tools` field already expects — see `run_command_tool_definition`, `chat.rs:583`, for the existing pattern) for the full action set from §7.6 that's implementable now: `read_file`, `search_codebase`, `propose_patch`, `run_command`, `read_git_status`, `read_git_diff`.
- Route `propose_patch` through a real tool call when `Config::supports_native_tools()` is true, with the same structured fields `parse_generated_edit` currently extracts by hand (summary, per-file path/status/content), replacing free-text parsing for providers capable of function-calling.
- Keep the existing `DAMAIAN_EDIT_V1`/`DAMAIAN_COMMAND_V1` text-envelope parsers as the fallback path for providers without native tool-calling support (some OpenAI-compatible endpoints don't implement function-calling) — this is the intentional reason the dual path exists, and it should remain, not be deleted.
- Give the model structured error feedback when a tool call fails validation (e.g. a `propose_patch` call targeting a restricted file), fed back as a tool-result message so the model can self-correct within the same turn, rather than the current pattern where malformed free text just fails to parse with no repair loop.
- Preserve the existing policy boundary: the orchestrator decides whether a tool-call-driven action is allowed, approval-gated, or blocked — tool-calling changes *how the model requests* an action, not *whether the client permits it* (§7.6: "The model may recommend actions, but the orchestrator decides...").

## 3. Non-Goals

- Adding new capabilities beyond what the client already supports through other paths (e.g. this spec does not add commit/push/PR creation — those remain explicitly out of scope per §3 Non-Goals and §7.9).
- Removing the text-envelope fallback — providers without function-calling support must keep working.
- Changing the underlying `PatchEngine`/`ProposedPatch` data model (`patch_engine.rs`) — this spec changes how a patch proposal is *requested* by the model, not how it's represented or applied.
- A generic/pluggable tool-registration system for third-party tools — the tool set stays fixed and code-defined, consistent with the spec's closed action list (§7.6).

## 4. Design

### 4.1 Tool schema definitions

Add a `tools` module (or extend the existing location of `run_command_tool_definition`, `chat.rs:583`) with one `ToolDefinition` per action:

- `propose_patch(summary: string, files: [{path: string, status: "created"|"modified"|"deleted", content: string}])` — mirrors `GeneratedEdit`/`ProposedChange` (`edit.rs:17-19`, `patch_engine.rs`'s `ProposedChange`) field-for-field so the tool-call result can be converted directly into the same `ProposedChange` structs the text-envelope path already builds.
- `run_command(command: string, reason: string)` — already exists; keep as-is.
- `read_git_status()`, `read_git_diff(staged: bool)` — thin wrappers over `GitService` (§7.9) methods already used elsewhere in the codebase for the CLI's `git-status`/`git-diff` commands (`main.rs:103`).
- `read_file(path: string)`, `search_codebase(query: string, mode: "keyword"|"semantic", limit: number)` — wrap `FileAccessController` and `RepositoryIndex::keyword_search`/`semantic_search` (`indexer.rs:61`, `:92`; the latter improved by [semantic search spec](02_semantic_search.md)).

### 4.2 Orchestration changes

Extend `run_agentic_turn` (`chat.rs:257`) so `matched_tool_call` detection (`chat.rs:325-329`, currently only checks for a command request) also checks for a `propose_patch` tool call and, when found, converts it into a `GeneratedEdit`/`ProposedChange` list and routes it through the **same** `PatchEngine`/approval flow the text-envelope path already uses (`edit.rs`) — no new approval logic, just a new construction path for the input to that existing flow.

For `read_file`, `search_codebase`, `read_git_status`, `read_git_diff`: these are read-only and low-risk by nature (§7.8 risk classification: read-only commands are low risk), so they can execute immediately within the tool loop and feed results back as `ModelMessage::tool(...)` — following the exact pattern already used for sandboxed `run_command` results (`chat.rs:370-393`), without new approval-gating.

### 4.3 Structured tool-call validation errors

When a tool call fails policy (e.g. `propose_patch` targets a path outside the repository root, or `read_file` targets a restricted file per §7.3), return the rejection as a tool-result message (not a hard turn failure) so the model can see *why* and retry within the bounded round count (`MAX_TOOL_ROUNDS = 6`, `chat.rs:515`) — this is a genuine improvement over the current text-envelope path, which has no feedback loop at all when parsing fails.

### 4.4 Fallback path unchanged

`Config::supports_native_tools()` (referenced `chat.rs:270`) continues to gate whether native tool schemas are offered at all; when false, the system prompt keeps instructing the `DAMAIAN_EDIT_V1`/`DAMAIAN_COMMAND_V1` conventions and `parse_generated_edit`/`parse_command_request` keep working exactly as today.

## 5. Acceptance Criteria

- With a provider where `supports_native_tools()` is true, asking for a code change results in a `propose_patch` tool call in the raw model response (verifiable via audit log or debug output), not a `DAMAIAN_EDIT_V1` text block.
- The resulting patch is identical in structure (same `ProposedPatch`/diff shown to the user) regardless of whether it arrived via tool call or text envelope — the UI and approval flow don't need to know which path produced it.
- A provider without native tool support continues to work exactly as before, using the text-envelope conventions, with no regression.
- A tool call requesting a restricted file (e.g. `.env`) is rejected with a structured error the model receives as a tool result, and the model's next round can produce a corrected response, within the existing `MAX_TOOL_ROUNDS` bound.
- Existing acceptance criteria from §7.6 continue to hold: a risky command proposed via tool call still requires approval before execution; a blocked action still reports a clear reason.

## 6. Open Questions / Decisions Needed

- Whether `read_file`/`search_codebase`/`read_git_status`/`read_git_diff` should count toward `MAX_TOOL_ROUNDS` the same way `run_command` does, or get a separate (possibly higher) round budget since they're read-only and cheaper to allow more of — recommend starting with the same shared budget and revisiting only if it proves too restrictive in practice.
- Whether to add telemetry distinguishing "tool-call path used" vs "text-envelope path used" per request, to measure how many configured providers actually support native tools in practice before deciding whether the fallback path is still worth maintaining long-term.
