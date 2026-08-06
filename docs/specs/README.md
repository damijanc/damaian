# Feature Specifications

Status: Draft
Source: gap analysis against `ai_coding_assistant_specification.md` and `ai_coding_assistant_must_have.md`, and a review of the current implementation (2026-07-17).

These specs describe features that close gaps between the product specification and the current state of the codebase. They are meant to be implemented one at a time, in the order listed. Each spec is self-contained: motivation, current state (with file references), requirements, non-goals, design, and acceptance criteria.

Specs 1–6 came from the original gap analysis. Later entries are added as design changes come up; #7 came from a reported bug rather than the analysis.

## Implementation order

| # | Spec | Why this order |
|---|------|-----------------|
| 1 | [01_response_formatting.md](01_response_formatting.md) | **Done.** Touches every assistant response; highest visible impact for lowest risk. No architectural changes required. |
| 2 | [02_semantic_search.md](02_semantic_search.md) | **Done.** Spec-flagged open gap (`ai_coding_assistant_specification.md` §7.2, §19) — current "semantic search" is keyword overlap, not embeddings. Independent of #1. |
| 3 | [03_structured_tool_calling.md](03_structured_tool_calling.md) | **Done.** Replaces fragile text-envelope parsing (`DAMAIAN_EDIT_V1`, `DAMAIAN_COMMAND_V1`) with native tool schemas. Best done before #4, since hunk-level apply will want a clean tool-call surface for patch actions. |
| 4 | [04_hunk_level_patch_apply.md](04_hunk_level_patch_apply.md) | **Done.** Correction: hunk-level apply already existed end-to-end in the desktop app. This spec was narrowly scoped to CLI parity + an audit gap. |
| 5 | [05_clickable_file_references.md](05_clickable_file_references.md) | **Done.** Smallest, most independent change; benefits from #1's markdown renderer being in place first. |
| 6 | [06_mcp_support.md](06_mcp_support.md) | **Done.** New capability (not a gap-closer): lets users add local/remote MCP servers whose tools plug into the native tool-call loop from #3. Depends on #3's structured tool-calling surface being in place. |
| 7 | [07_generated_secret_override.md](07_generated_secret_override.md) | **Done.** Bug-driven, not from the original gap analysis: the generated-secret block false-positived on setup documentation and had no user override, so a false positive was an unrecoverable dead end. Closes the §7.10 "Override" requirement. |

Each spec's status is tracked at the top of its file: `Not started`, `In progress`, or `Done`.
