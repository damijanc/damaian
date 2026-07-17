# Feature Specifications

Status: Draft
Source: gap analysis against `ai_coding_assistant_specification.md` and `ai_coding_assistant_must_have.md`, and a review of the current implementation (2026-07-17).

These specs describe features that close gaps between the product specification and the current state of the codebase. They are meant to be implemented one at a time, in the order listed. Each spec is self-contained: motivation, current state (with file references), requirements, non-goals, design, and acceptance criteria.

## Implementation order

| # | Spec | Why this order |
|---|------|-----------------|
| 1 | [01_response_formatting.md](01_response_formatting.md) | **Done.** Touches every assistant response; highest visible impact for lowest risk. No architectural changes required. |
| 2 | [02_semantic_search.md](02_semantic_search.md) | **Done.** Spec-flagged open gap (`ai_coding_assistant_specification.md` §7.2, §19) — current "semantic search" is keyword overlap, not embeddings. Independent of #1. |
| 3 | [03_structured_tool_calling.md](03_structured_tool_calling.md) | **Done.** Replaces fragile text-envelope parsing (`DAMAIAN_EDIT_V1`, `DAMAIAN_COMMAND_V1`) with native tool schemas. Best done before #4, since hunk-level apply will want a clean tool-call surface for patch actions. |
| 4 | [04_hunk_level_patch_apply.md](04_hunk_level_patch_apply.md) | Correction: hunk-level apply already exists end-to-end in the desktop app. This spec is now narrowly scoped to CLI parity + an audit gap, and is small/independent — could be done anytime. |
| 5 | [05_clickable_file_references.md](05_clickable_file_references.md) | Smallest, most independent change; benefits from #1's markdown renderer being in place first. |

Each spec's status is tracked at the top of its file: `Not started`, `In progress`, or `Done`.
