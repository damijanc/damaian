# AI Coding Assistant Client — Must-Have Features

## Purpose

This document defines the must-have features for the first version of an AI coding assistant client. The client is responsible for the user experience, repository access, context management, tool execution, security, approvals, and integration with the AI model provider.

The AI model should be treated as a reasoning and generation engine. The client must remain responsible for permissions, file access, tool execution, audit logging, rollback, and final user approval.

---

## 1. Chat With Codebase

The client must provide a chat interface where users can ask questions about the codebase and request coding assistance.

### Requirements

- Allow users to ask natural-language questions about the project.
- Allow users to request code generation, refactoring, debugging, and explanations.
- Preserve conversation context within a project session.
- Support streaming model responses.
- Display model responses in a readable developer-friendly format.
- Allow the assistant to reference relevant files, symbols, or code snippets when answering.

---

## 2. Project Indexing

The client must index the project so the assistant can retrieve relevant code context.

### Requirements

- Scan and index project files.
- Respect `.gitignore` and configured ignore rules.
- Exclude binaries, generated files, build artifacts, secrets, and very large files by default.
- Support semantic search across the codebase.
- Support keyword search across files.
- Track file paths, symbols, imports, and basic project structure.
- Refresh the index when files change.

---

## 3. File Read Access

The client must allow the assistant to read relevant files in a controlled way.

### Requirements

- Allow the assistant to read files required for a task.
- Restrict access based on user permissions and configured policies.
- Show which files were used as context when useful.
- Prevent access to restricted files, secrets, credentials, and production-only configuration unless explicitly allowed.
- Support user-selected file scope, such as selected file, folder, or whole repository.

---

## 4. Diff Preview

The client must show proposed changes before applying them.

### Requirements

- Display AI-generated changes as a diff.
- Show changed files clearly.
- Support side-by-side or inline diff view.
- Highlight additions, deletions, and modifications.
- Do not silently apply changes without user visibility.
- Include a short summary of proposed changes.

---

## 5. Apply and Reject Changes

The client must allow users to control which AI-generated changes are applied.

### Requirements

- Allow accepting or rejecting all changes.
- Allow accepting or rejecting changes per file.
- Ideally allow accepting or rejecting individual hunks.
- Apply accepted changes safely to the workspace.
- Preserve rejected changes only as conversation history or discarded suggestions.
- Show a clear success or failure message after applying changes.

---

## 6. Run Tests and Linters

The client must support running validation commands after code changes.

### Requirements

- Detect common test, lint, format, type-check, and build commands.
- Allow users to configure project-specific commands.
- Run approved commands in the workspace.
- Capture command output, exit codes, logs, and errors.
- Feed failures back to the assistant for explanation or repair.
- Clearly display whether checks passed or failed.

### Example commands

- `npm test`
- `npm run lint`
- `npm run typecheck`
- `pytest`
- `mvn test`
- `gradle test`
- `go test ./...`
- `cargo test`

---

## 7. Terminal Command Approval

The client must control terminal execution and require approval for risky commands.

### Requirements

- Show the exact command before execution when approval is required.
- Require approval for commands that modify files, install packages, access the network, delete data, or affect Git state.
- Block dangerous commands by default.
- Allow admins to configure command allowlists and blocklists.
- Record executed commands in the audit log.
- Capture stdout, stderr, exit code, and execution duration.

---

## 8. Basic Git Diff Support

The client must integrate with Git enough to understand and protect workspace changes.

### Requirements

- Read current Git status.
- Read current Git diff.
- Distinguish between user changes and assistant-generated changes where possible.
- Warn before overwriting existing user edits.
- Support generating commit message suggestions.
- Support creating a patch or diff summary.
- Avoid committing, pushing, or creating branches without explicit user approval.

---

## 9. Secret Detection

The client must detect and protect secrets before sending context to the model or applying changes.

### Requirements

- Scan files and selected context for possible secrets.
- Detect common secret patterns such as API keys, tokens, private keys, passwords, and credentials.
- Prevent secrets from being sent to the model unless explicitly allowed by policy.
- Warn users when generated code appears to contain hardcoded secrets.
- Redact detected secrets in logs, prompts, and audit output.
- Allow configuration of custom secret patterns.

---

## 10. Audit Logs

The client must maintain an audit trail of assistant actions.

### Requirements

- Log user requests.
- Log files read by the assistant.
- Log files modified by the assistant.
- Log commands proposed and executed.
- Log approval decisions.
- Log model provider, model name, timestamps, and task status.
- Log errors and failed operations.
- Make logs accessible to administrators or authorized users.
- Redact secrets and sensitive data from logs.

---

## 11. Model Adapter for OpenAI

The client must integrate with the AI model provider through a dedicated adapter layer.

### Requirements

- Support calling OpenAI-compatible model APIs.
- Support streaming responses.
- Support structured outputs where needed.
- Support tool/function-calling workflows.
- Handle provider errors, timeouts, retries, and rate limits.
- Track token usage and request metadata.
- Keep provider-specific logic isolated from the rest of the client.
- Allow future support for additional model providers.

---

## 12. Context Management

The client must prepare relevant context for the model without oversharing unnecessary data.

### Requirements

- Select relevant files, snippets, symbols, errors, and conversation history for each request.
- Keep prompts within the model context window.
- Avoid sending unrelated files.
- Prefer minimal, task-relevant context.
- Include project rules and coding standards when available.
- Include test output or terminal logs when debugging.
- Redact secrets before context is sent to the model.

---

## 13. User Approval Workflow

The client must keep the user in control of important actions.

### Requirements

- Require user approval before applying file changes.
- Require user approval before running risky commands.
- Require user approval before committing, pushing, opening pull requests, or changing dependencies.
- Clearly explain what action is being requested.
- Allow users to approve, reject, cancel, or modify the requested action.
- Store approval decisions in the audit log.

---

## 14. Basic Error Handling

The client must handle common failures gracefully.

### Requirements

- Show clear errors when model calls fail.
- Show clear errors when file operations fail.
- Show clear errors when commands fail.
- Preserve user work when failures occur.
- Allow retrying failed operations.
- Avoid leaving the workspace in an inconsistent state.

---

## 15. Minimal Admin Configuration

The client must provide basic configuration for safe operation.

### Requirements

- Configure allowed repositories or workspaces.
- Configure ignored files and folders.
- Configure command allowlists and blocklists.
- Configure model provider and model name.
- Configure data retention settings.
- Configure whether logs are enabled.
- Configure secret detection patterns.
- Configure whether file edits and terminal commands require approval.

---

## MVP Summary

The first version should include the following must-have capabilities:

| Priority | Feature |
|---|---|
| Must-have | Chat with codebase |
| Must-have | Project indexing |
| Must-have | File read access |
| Must-have | Context management |
| Must-have | Diff preview |
| Must-have | Apply/reject changes |
| Must-have | Run tests and linters |
| Must-have | Terminal command approval |
| Must-have | Basic Git diff support |
| Must-have | Secret detection |
| Must-have | Audit logs |
| Must-have | Model adapter for OpenAI |
| Must-have | User approval workflow |
| Must-have | Basic error handling |
| Must-have | Minimal admin configuration |

---

## Boundary Between Model and Client

### Model provider responsibility

The model provider supplies reasoning and generation capabilities, including:

- Code generation
- Code explanation
- Refactoring suggestions
- Debugging suggestions
- Test generation
- Review comments
- Task planning
- Structured responses
- Tool-call recommendations

### Client responsibility

The client owns product behavior, safety, and execution, including:

- User interface
- Repository access
- Context selection
- File reading and writing
- Tool execution
- Terminal command handling
- Git integration
- Diff display
- Approval workflow
- Audit logging
- Secret protection
- Rollback and error handling
- Admin configuration
- Model API integration

## Key Principle

The client must treat the AI model as a reasoning engine, not as the authority. The client controls what the assistant can read, change, run, store, or publish.
