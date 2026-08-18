# Feature Spec: Scoped AGENTS.md Instructions

Status: Done
Order: 11 of 11
Related spec sections: `ai_coding_assistant_specification.md` §7.4 and
`ai_coding_assistant_must_have.md` §12.

## 1. Motivation

Damaian already included a root `AGENTS.md` file through the generic
`PROJECT_RULES` list in `ContextManager`, but that made it just another
repository document. It did not make the file's role explicit to the model, did
not support nested instruction files, and had no tests proving that edit and
chat requests received the instructions.

Users expect `AGENTS.md` to behave like scoped coding-agent instructions:
repository-wide guidance at the root, with more specific rules in subtrees.

## 2. Requirements

1. Include root `AGENTS.md` when present.
2. Include nested `AGENTS.md` files that apply to files selected for a turn.
3. Order instructions from broadest to most specific.
4. Do not include unrelated nested instruction files.
5. Apply the same behavior to chat and edit proposals.
6. Keep file access policy unchanged: restricted files and paths outside the
   selected repository remain denied unless already explicitly allowed by the
   caller's context-file flow.
7. Redact secrets from instruction files before model calls, using the existing
   scanner.
8. Explain precedence in the model prompt: user request and safety policy win;
   more specific nested instructions override broader ones.

## 3. Non-goals

- A new UI for editing `AGENTS.md`.
- Shipping global application-level agent instructions outside a repository.
- Applying instructions to shell commands or MCP tools outside the normal safety
  and approval policy.
- Parsing sections inside `AGENTS.md`; the file is treated as Markdown text.

## 4. Design

`ContextManager` treats `AGENTS.md` as a first-class context source instead of
including it in the generic project-rule list.

For each turn, the manager builds the set of context paths from explicit files,
file paths mentioned in the prompt, and search results. It then derives the
applicable instruction paths:

- `AGENTS.md`
- each ancestor directory's `AGENTS.md` for every in-repository context path

The paths are de-duplicated while preserving order, so root instructions appear
before nested instructions. They are added with context kind
`agent_instruction`, after explicit or prompt-mentioned files so user-selected
context keeps its budget priority, and before ordinary project rules such as
`README.md` and `.editorconfig`.

The chat and edit system prompts tell the model how to interpret
`agent_instruction` sections and how precedence works.

## 5. Acceptance Criteria

- A repository with root `AGENTS.md` includes it in a chat model request as
  `agent_instruction`.
- A request touching `crates/workspace-engine/src/lib.rs` includes
  `AGENTS.md`, `crates/AGENTS.md`, and
  `crates/workspace-engine/AGENTS.md` in that order.
- A nested `AGENTS.md` from an unrelated subtree is not included.
- A generated edit request receives the same instruction context.
- Existing context files, project rules, retrieval, and secret redaction continue
  to work through the same `ContextManager::add_file` path.
