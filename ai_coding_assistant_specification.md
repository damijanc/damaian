# AI Coding Assistant Client Specification

Status: Draft  
Target platform: macOS on MacBooks  
Source requirements: `ai_coding_assistant_must_have.md`

## 1. Executive Summary

The product is a local-first macOS AI coding assistant client. It provides a chat interface for software development tasks, indexes local repositories, prepares minimal context for an AI model, previews generated edits, applies approved changes, runs approved validation commands, and records an audit trail.

The client must treat the model as a reasoning and generation engine only. The macOS client remains the authority for repository access, file reads, file writes, terminal execution, secret redaction, approvals, audit logging, and rollback.

The MVP must support a single developer working on one or more local Git repositories on a MacBook. Team administration, cloud synchronization, remote workspaces, and deep IDE integrations can be layered on later but must not be required for the first version.

## 2. Goals

- Let developers ask natural-language questions about a local codebase.
- Let developers request code generation, refactoring, debugging, test generation, and explanation.
- Retrieve relevant repository context without oversharing project data.
- Keep all file writes, terminal commands, Git operations, and dependency changes under user control.
- Provide readable diffs and selective apply or reject workflows.
- Detect and redact secrets before model calls, logs, and command output persistence.
- Run tests, linters, type checks, builds, and formatters when approved.
- Maintain a local audit trail of assistant activity.
- Integrate with OpenAI-compatible model APIs through an isolated adapter.
- Provide safe defaults for macOS laptops used by professional developers.

## 3. Non-Goals for MVP

- Fully autonomous development without user approval.
- Multi-user collaboration inside the client.
- Cloud-hosted repository execution.
- Direct production deployment.
- Automatic committing, pushing, branch creation, pull request creation, or dependency installation without explicit approval.
- Full IDE parity with VS Code, JetBrains, or Xcode.
- Complete language-server replacement.
- Guaranteed semantic understanding of every programming language.

## 4. Target Users and Environment

### 4.1 Primary User

A software developer using a MacBook for local development. The user has one or more local repositories and wants an assistant that can explain, edit, test, and review code while respecting local permissions and preserving manual control.

### 4.2 Supported Platform

- macOS 14 Sonoma or newer recommended.
- Apple silicon required for best performance; Intel Macs may be supported on a best-effort basis.
- Local Git installation required for Git-aware workflows.
- Shell support for `zsh` by default, with optional `bash` and `fish`.
- Network access required only for model provider API calls and user-approved commands that need network access.

### 4.3 Repository Types

The MVP should support common local repository layouts:

- JavaScript and TypeScript projects.
- Python projects.
- Go projects.
- PHP project.
- Rust projects.
- Java, Kotlin, and Gradle or Maven projects.
- Monorepos with multiple package roots.
- Mixed-language repositories.

## 5. Product Principles

1. User control first: the user approves important actions before execution.
2. Least context required: the model receives only the context needed for the task.
3. Local workspace is authoritative: the client owns actual file and command effects.
4. Transparent changes: generated edits are shown as diffs before application.
5. Safety by default: risky commands, secrets, and protected files are blocked or approval-gated.
6. Auditability: important actions are recorded with timestamps, status, and redaction.
7. Provider isolation: OpenAI-specific API details stay behind a model adapter interface.

## 6. High-Level Architecture

The client should be organized as a macOS desktop application with a local workspace engine.

```text
+----------------------------+
| macOS Desktop UI           |
| - Chat                     |
| - Diff viewer              |
| - Approval prompts         |
| - Command output           |
| - Settings                 |
+-------------+--------------+
              |
              v
+-------------+--------------+
| Application Orchestrator   |
| - Session state            |
| - Task lifecycle           |
| - Approval workflow        |
| - Error handling           |
+-------------+--------------+
              |
              v
+-------------+--------------+       +----------------------+
| Local Workspace Engine     |<----->| Model Adapter         |
| - Indexer                  |       | - OpenAI-compatible   |
| - Context manager          |       | - Streaming           |
| - File access controller   |       | - Tool calls          |
| - Patch engine             |       | - Retries/timeouts    |
| - Command runner           |       +----------------------+
| - Git service              |
| - Secret scanner           |
+-------------+--------------+
              |
              v
+-------------+--------------+
| Local Storage              |
| - Project indexes          |
| - Session history          |
| - Audit logs               |
| - Configuration            |
+----------------------------+
```

### 6.1 Recommended Process Model

For MVP, use two logical layers:

- Desktop UI process: owns windows, chat rendering, diff display, settings, and user approvals.
- Workspace engine process or module: owns file reads, indexing, patching, command execution, Git, secret scanning, and model calls.

The workspace engine may start as an in-process module for implementation speed. The design should keep a clear boundary so it can later become a separate local service or sandboxed helper.

### 6.2 macOS Integration

The client should use standard macOS permission and storage behavior:

- User explicitly selects allowed repository folders using the system file picker.
- Repository access is limited to selected folders unless admin configuration allows broader scopes.
- Local app data is stored under the user's Application Support directory.
- Cache and index data are stored in a client-owned directory and can be cleared.
- Sensitive credentials are stored in Keychain, not plaintext config files.
- Notifications are optional and disabled by default for MVP unless needed for long-running command completion.

## 7. Core Components

### 7.1 Chat Interface

Responsibilities:

- Capture user requests.
- Display streaming model responses.
- Show code blocks, file references, command proposals, and diff summaries.
- Preserve project-scoped conversation context.
- Let users start, rename, and delete project sessions.
- Expose which files were used as context when useful.

Functional requirements:

- Markdown rendering with syntax highlighting.
- Streaming response updates.
- File reference links that open the local file in the app or configured editor.
- Distinct UI states for thinking, waiting for approval, running command, applying patch, failed, and complete.
- Clear separation between assistant suggestions and applied workspace changes.

Acceptance criteria:

- A user can ask "Explain how authentication works" and receive an answer with relevant file references.
- A user can ask for a refactor and see proposed diffs before any file is changed.
- A user can continue a project session and the assistant preserves relevant prior context.

### 7.2 Project Indexer

Responsibilities:

- Discover files in selected repositories.
- Respect `.gitignore`, global ignore rules, and client ignore configuration.
- Exclude binaries, generated artifacts, build outputs, dependencies, secrets, and very large files by default.
- Extract path metadata, language, imports, symbols, and basic project structure.
- Provide keyword and semantic search.
- Refresh changed files incrementally.

Index records should include:

- Repository ID.
- File path relative to repository root.
- Language or content type.
- Size, modified time, and content hash.
- Ignore or inclusion decision.
- Extracted symbols where supported.
- Imports and dependency hints where supported.
- Text chunks for semantic search.
- Keyword index terms.

Default exclusions:

- `.git/`
- `node_modules/`
- `vendor/`
- `.venv/`, `venv/`
- `dist/`, `build/`, `target/`, `coverage/`
- Generated lockfile-heavy or minified files when not explicitly requested.
- Binary files.
- Files above configured size limits.
- Files matching secret or credential patterns.

Implementation notes:

- Use filesystem watchers for incremental refresh.
- Fall back to periodic rescan when watcher events overflow.
- Store embeddings and keyword indexes locally.
- Recompute only changed chunks when possible.

Acceptance criteria:

- Indexing respects `.gitignore`.
- Editing a file updates search results without a full manual rescan.
- Semantic search and keyword search can both return relevant files for a coding question.

### 7.3 File Access Controller

Responsibilities:

- Enforce repository and file-scope permissions.
- Allow the assistant to read task-relevant files only.
- Prevent access to restricted files unless explicitly allowed by policy.
- Record file reads in the audit log.
- Surface context file usage to the user when useful.

Scope modes:

- Selected file.
- Selected folder.
- Whole repository.
- Multi-repository workspace.

Restricted by default:

- `.env`, `.env.*`
- Private keys and certificates.
- Credential stores.
- Production-only configuration.
- Files matching custom deny patterns.
- Files flagged by secret detection.

Acceptance criteria:

- The assistant cannot read files outside selected repositories.
- Restricted files are omitted from context by default.
- File reads are logged with path, timestamp, task ID, and redaction status.

### 7.4 Context Manager

Responsibilities:

- Select relevant context for each model request.
- Fit context within the model context window.
- Include project rules and coding standards when available.
- Include prior conversation turns selectively.
- Include errors, terminal output, and test logs when debugging.
- Redact secrets before sending context.
- Avoid sending unrelated files.

Context sources:

- User prompt.
- Current session summary.
- Relevant file chunks from index search.
- Explicitly selected files or folders.
- Git diff and status.
- Test or command output.
- Project instructions such as `README`, `CONTRIBUTING`, `.editorconfig`, formatter config, or assistant-specific rules.

Selection strategy:

1. Honor explicit user selections first.
2. Add highly relevant files from semantic and keyword search.
3. Add local project rules and test output when relevant.
4. Add current Git diff for edit, review, or debugging tasks.
5. Summarize large files and long logs.
6. Drop low-relevance context before exceeding token budget.

Acceptance criteria:

- A debugging request includes the failing error output and relevant source files.
- The client avoids sending unrelated files for narrow questions.
- Detected secrets are redacted before model calls.

### 7.5 Model Adapter

Responsibilities:

- Integrate with OpenAI-compatible APIs.
- Support streaming responses.
- Support structured outputs.
- Support tool/function-calling workflows.
- Handle provider errors, timeouts, retries, and rate limits.
- Track token usage and request metadata.
- Keep provider-specific logic isolated.
- Allow additional providers later.

Interface shape:

```text
ModelAdapter
- streamResponse(request, callbacks): ModelRun
- requestStructuredOutput(schema, request): StructuredResult
- cancel(runId): void
- estimateTokens(payload): TokenEstimate
```

Request metadata:

- Provider name.
- Model name.
- Request ID.
- Session ID and task ID.
- Token counts.
- Latency.
- Retry count.
- Error type if failed.

Failure behavior:

- Network failures show clear retryable errors.
- Rate limits display provider-specific wait information when available.
- Timeout and cancellation preserve user work.
- Partial streamed output is clearly marked if incomplete.

Acceptance criteria:

- The UI can stream a model response.
- Provider errors are displayed clearly and logged.
- The rest of the client does not depend on OpenAI-specific request shapes.

### 7.6 Tool and Action Orchestrator

Responsibilities:

- Convert model suggestions into client-controlled actions.
- Validate proposed file reads, file writes, commands, and Git operations.
- Request user approval when needed.
- Execute approved actions through local services.
- Return action results to the model when appropriate.
- Maintain task status.

Action types:

- Read file.
- Search codebase.
- Propose patch.
- Apply patch.
- Propose terminal command.
- Run terminal command.
- Read Git status.
- Read Git diff.
- Generate commit message suggestion.

The model may recommend actions, but the orchestrator decides whether the action is allowed, approval-gated, or blocked.

Acceptance criteria:

- A model-proposed risky command is not executed until approved.
- A blocked action reports a clear reason.
- Tool results can be fed back into the model for repair or explanation.

### 7.7 Diff and Patch Engine

Responsibilities:

- Convert generated edits into structured patches.
- Display proposed changes as inline or side-by-side diffs.
- Support accepting or rejecting all changes.
- Support accepting or rejecting changes per file.
- Support hunk-level acceptance when feasible.
- Apply accepted changes safely.
- Detect conflicts with user edits.
- Preserve rejected changes only in conversation history or suggestion records.

Patch safety:

- Capture pre-apply file hashes.
- Refuse to apply if target files changed since patch generation unless the user chooses to rebase or regenerate.
- Write changes atomically where possible.
- Keep a rollback snapshot for assistant-applied changes.
- Never silently overwrite user edits.

Acceptance criteria:

- Generated changes are visible before application.
- A user can apply only one file from a multi-file suggestion.
- The client warns if files changed after the diff was generated.

### 7.8 Terminal Command Runner

Responsibilities:

- Detect common test, lint, format, type-check, and build commands.
- Allow user-configured project commands.
- Require approval for risky commands.
- Execute approved commands in the workspace.
- Capture stdout, stderr, exit code, duration, and environment summary.
- Feed failures back to the assistant when requested.
- Redact secrets from command output before display persistence and logs.

Risk classification:

- Low risk: read-only commands such as `git status`, `git diff`, `ls`, `pwd`, and configured safe test commands.
- Medium risk: commands that write build artifacts, run formatters, or modify generated files.
- High risk: dependency installation, network access, Git state changes, file deletion, chmod, shell scripts, and commands with unknown effects.
- Blocked: destructive commands that delete broad paths, wipe Git state, exfiltrate secrets, or bypass client controls.

Approval prompt must show:

- Exact command.
- Working directory.
- Reason for execution.
- Risk classification.
- Expected effects.
- Whether network access may be used.
- Environment variables that will be exposed, with sensitive values redacted.

Acceptance criteria:

- `npm test` can be run after user approval or configured allowlist approval.
- A dependency install requires explicit approval.
- Output, exit code, and duration are shown and logged.

### 7.9 Git Service

Responsibilities:

- Read Git status.
- Read Git diff.
- Distinguish clean, modified, staged, untracked, and conflicted files.
- Identify likely user changes versus assistant-applied changes where possible.
- Warn before overwriting existing user edits.
- Generate commit message suggestions.
- Generate patch summaries.
- Avoid commit, push, branch creation, and pull request creation without explicit approval.

MVP Git operations:

- `status`
- `diff`
- `diff --staged`
- `log` for limited recent history when relevant
- Commit message suggestion without committing

Post-MVP Git operations:

- Create branch with approval.
- Commit with approval.
- Push with approval.
- Open pull request with approval.

Acceptance criteria:

- The client shows current modified files before applying a patch.
- Existing user edits are detected before assistant changes are applied.
- Commit messages can be suggested without creating commits.

### 7.10 Secret Scanner

Responsibilities:

- Scan selected context before model calls.
- Scan generated code before applying changes.
- Scan command output before persistence.
- Scan audit log fields before writing.
- Redact or block common secrets.
- Support custom secret patterns.

Detection categories:

- API keys.
- Access tokens.
- Private keys.
- Password assignments.
- Database URLs with credentials.
- Cloud provider credentials.
- SSH keys.
- Certificates.
- `.env` style secrets.

Actions:

- Redact: replace sensitive values with stable placeholders.
- Warn: tell the user generated code may contain a hardcoded secret.
- Block: prevent sending or applying content if policy requires.
- Override: allow explicit user or admin override when policy permits.

Acceptance criteria:

- `.env` values are not sent to the model by default.
- Generated hardcoded secrets trigger warnings before apply.
- Audit logs do not contain raw detected secrets.

### 7.11 Audit Log Service

Responsibilities:

- Log user requests.
- Log assistant task status.
- Log files read and modified.
- Log commands proposed and executed.
- Log approval decisions.
- Log model provider, model name, timestamps, and token usage.
- Log errors and failed operations.
- Redact secrets and sensitive data.
- Make logs available to authorized users or admins.

Log storage:

- MVP: local append-only JSONL files under Application Support.
- Optional encryption at rest using macOS Keychain-managed keys.
- Configurable retention period.
- Export to JSONL for diagnostics.

Audit event fields:

- Event ID.
- Timestamp.
- User ID or local profile ID.
- Repository ID.
- Session ID.
- Task ID.
- Event type.
- Actor: user, assistant, system, model, command.
- Resource path or command, redacted as needed.
- Decision or status.
- Error details when applicable.

Acceptance criteria:

- Every file modification includes an audit event.
- Every command execution includes stdout/stderr summary, exit code, and duration.
- Approval decisions are logged.

### 7.12 Admin and User Configuration

Responsibilities:

- Configure allowed repositories or workspaces.
- Configure ignored files and folders.
- Configure command allowlists and blocklists.
- Configure model provider and model name.
- Configure data retention.
- Configure log enablement.
- Configure secret detection patterns.
- Configure approval requirements.

Configuration scopes:

- Built-in defaults.
- User settings.
- Repository settings.
- Admin or managed policy settings.

Precedence:

1. Admin or managed policy.
2. Repository settings.
3. User settings.
4. Built-in defaults.

MVP settings:

- Model provider API key reference stored in Keychain.
- Model name.
- Allowed repository roots.
- Ignore patterns.
- Command allowlist and blocklist.
- Require approval for file edits.
- Require approval for risky commands.
- Audit log retention.
- Secret scanner custom patterns.

Acceptance criteria:

- A user can select allowed repositories.
- API keys are not stored in plaintext.
- A configured blocklisted command cannot be executed by the assistant.

## 8. User Workflows

### 8.1 Ask About Codebase

1. User selects a repository.
2. Client indexes the repository.
3. User asks a question in chat.
4. Context manager searches index and selects relevant files.
5. Secret scanner redacts selected context.
6. Model adapter streams the answer.
7. UI displays answer with file references.
8. Audit log records request, model metadata, and files used.

Success criteria:

- The answer cites relevant files.
- No workspace files are modified.
- Restricted files are not included unless explicitly allowed.

### 8.2 Request Code Change

1. User requests a change.
2. Client gathers context and current Git status.
3. Model proposes edits.
4. Patch engine builds a structured diff.
5. UI displays diff and summary.
6. User accepts all, accepts selected files or hunks, rejects, or asks for revisions.
7. Client applies approved changes.
8. Git service verifies workspace status.
9. Audit log records modified files and approval.

Success criteria:

- No file is changed before approval.
- User can reject the suggestion without workspace impact.
- Applied files match approved diff.

### 8.3 Run Tests and Repair

1. User asks to run tests or the assistant recommends validation.
2. Client detects candidate commands.
3. User approves selected command.
4. Command runner executes in repository root or configured package root.
5. Output is captured and redacted.
6. Failures can be sent back to the assistant for explanation or repair.
7. Any repair follows the code-change workflow.

Success criteria:

- Command output and exit code are visible.
- Failed output can be used as context.
- Risky commands require approval.

### 8.4 Approve Risky Command

1. Assistant proposes a command.
2. Command runner classifies risk.
3. UI shows exact command, working directory, reason, and risk.
4. User approves, rejects, cancels, or edits the command.
5. Approved command executes.
6. Result is displayed and logged.

Success criteria:

- Unknown or risky command does not run without approval.
- Rejected commands leave the workspace unchanged.

## 9. Data Model

### 9.1 Repository

```json
{
  "id": "repo_123",
  "name": "example",
  "rootPath": "/Users/user/dev/example",
  "gitRemote": "git@github.com:org/example.git",
  "createdAt": "2026-07-03T10:00:00Z",
  "lastIndexedAt": "2026-07-03T10:05:00Z"
}
```

### 9.2 Session

```json
{
  "id": "session_123",
  "repositoryId": "repo_123",
  "title": "Refactor auth middleware",
  "createdAt": "2026-07-03T10:00:00Z",
  "updatedAt": "2026-07-03T10:10:00Z",
  "summary": "User is refactoring auth middleware and adding tests."
}
```

### 9.3 Task

```json
{
  "id": "task_123",
  "sessionId": "session_123",
  "status": "waiting_for_approval",
  "userPrompt": "Add tests for token refresh failure",
  "modelProvider": "openai",
  "modelName": "configured-model",
  "createdAt": "2026-07-03T10:12:00Z",
  "completedAt": null
}
```

### 9.4 Proposed Patch

```json
{
  "id": "patch_123",
  "taskId": "task_123",
  "summary": "Adds token refresh failure tests.",
  "files": [
    {
      "path": "tests/auth-refresh.test.ts",
      "baseHash": "sha256:...",
      "status": "modified",
      "hunks": []
    }
  ],
  "status": "pending"
}
```

### 9.5 Command Execution

```json
{
  "id": "cmd_123",
  "taskId": "task_123",
  "command": "npm test",
  "workingDirectory": "/Users/user/dev/example",
  "risk": "medium",
  "approvedBy": "local_user",
  "startedAt": "2026-07-03T10:15:00Z",
  "completedAt": "2026-07-03T10:15:42Z",
  "exitCode": 0,
  "stdoutRef": "log://cmd_123/stdout",
  "stderrRef": "log://cmd_123/stderr"
}
```

## 10. Security and Privacy Requirements

### 10.1 Local File Boundaries

- The assistant can only access selected repository roots.
- Symlinks must be resolved before access decisions.
- Files outside allowed roots are denied even if reachable through symlink traversal.
- Restricted file patterns are denied by default.

### 10.2 Prompt Safety

- Every model-bound payload passes through secret scanning.
- Redaction occurs before logging prompt payloads.
- The client should retain only metadata by default, not full prompts, unless debug logging is explicitly enabled.

### 10.3 Command Safety

- Commands execute with the user's OS permissions, so approval gates must be strict.
- The command runner should not inject hidden shell fragments.
- The approval dialog must show the exact command that will run.
- Environment variables must be minimized and redacted in logs.

### 10.4 Patch Safety

- Generated edits are data until approved.
- File writes require hash checks to avoid overwriting new user edits.
- Atomic writes or temporary file replacement should be used where possible.
- Rollback metadata should be kept for assistant-applied patches.

### 10.5 Model Provider Privacy

- The client must document what data is sent to the model provider.
- Users should be able to inspect file context selected for a task.
- API keys are stored in Keychain.
- Provider configuration is isolated from repository files.

## 11. Error Handling

Common errors and required behavior:

- Model API failure: show clear error, allow retry, preserve user prompt and context plan.
- Indexing failure: show affected path, continue with partial index when safe.
- File read denied: show denied path category without exposing sensitive content.
- Patch conflict: stop apply, show conflict reason, offer regenerate or manual review.
- Command failure: show exit code, output, and repair option.
- Secret detection block: show policy reason and safe override path when allowed.
- Audit write failure: warn user and disable risky actions if policy requires audit availability.

The client must never leave the workspace in an ambiguous state after a failed apply. The UI must clearly report whether no files changed, some files changed, or rollback was completed.

## 12. Non-Functional Requirements

### 12.1 Performance

- Initial index for a medium repository should complete in the background without blocking chat UI.
- Incremental updates should normally finish within seconds of file changes.
- Chat streaming should begin as soon as provider streaming starts.
- Diff rendering should remain responsive for large multi-file patches by virtualizing large views.

### 12.2 Reliability

- Index updates should be resumable after app restart.
- Long-running commands should survive UI navigation within the app.
- Incomplete model responses must be marked incomplete.
- Audit writes should be append-only and resilient to process crashes.

### 12.3 Usability

- The first screen should let the user select a repository and start chatting.
- Approval prompts should be specific and concise.
- Diff UI should make changed files and hunks easy to scan.
- Command output should support search and copying.
- Errors should include a direct next action when possible.

### 12.4 Maintainability

- Provider integrations must be behind adapter interfaces.
- File access, command execution, Git, and secret scanning should be separate services.
- Business logic must be testable without a running UI.
- Policy decisions should be deterministic and unit tested.

## 13. MVP Release Scope

The first release includes:

- macOS desktop client.
- Repository selection.
- Project indexing with `.gitignore` support.
- Keyword and semantic code search.
- Chat with streaming responses.
- Controlled file reads.
- Context management and redaction.
- OpenAI-compatible model adapter.
- Diff preview.
- Apply or reject all changes.
- Apply or reject per-file changes.
- Basic hunk-level support if implementation cost is acceptable.
- Test, lint, type-check, build command detection.
- Approved terminal command execution.
- Git status and diff reading.
- Commit message suggestions.
- Secret detection.
- Local audit logs.
- Minimal user and admin configuration.
- Basic error handling and retry.

## 14. Post-MVP Enhancements

- Native IDE extensions for VS Code, JetBrains, and Xcode.
- Full hunk and line-level apply.
- Background task queue.
- Team policy management.
- Remote development environments.
- Pull request creation after approval.
- Branch creation and commit execution after approval.
- More advanced language-server integration.
- Organization-wide audit export.
- Model provider marketplace.
- Offline local model support for restricted repositories.

## 15. Acceptance Test Matrix

| Area | Scenario | Expected Result |
|---|---|---|
| Chat | Ask a question about a repository | Streaming answer with relevant file references |
| Indexing | Repository has `.gitignore` exclusions | Ignored files are not indexed |
| File access | Assistant requests restricted `.env` file | Access denied or explicit policy override required |
| Context | Prompt includes possible secret | Secret is redacted before model call |
| Diff | Assistant proposes code edits | Diff appears before any file changes |
| Apply | User accepts one changed file | Only that file is modified |
| Conflict | File changes after diff generation | Apply is blocked or requires regeneration |
| Commands | Assistant proposes `npm test` | Approval prompt appears unless allowlisted |
| Risky command | Assistant proposes dependency install | Explicit approval required |
| Git | Workspace has user edits | Client warns before overlapping assistant edits |
| Audit | Command is executed | Command, approval, output summary, exit code, and duration are logged |
| Model | Provider rate limit occurs | Clear retryable error is shown |
| Config | Admin blocklists a command | Command cannot be executed |

## 16. Suggested Implementation Milestones

### Milestone 1: Local Workspace Foundation

- Repository picker and allowed-root storage.
- File scanner with ignore rules.
- Basic keyword index.
- Git status and diff read support.
- Local configuration storage.

### Milestone 2: Chat and Model Adapter

- Chat UI with streaming.
- OpenAI-compatible adapter.
- Session persistence.
- Context manager v1.
- File read audit events.

### Milestone 3: Edits and Diff Workflow

- Model-generated patch format.
- Diff viewer.
- Apply or reject all.
- Per-file apply or reject.
- Patch conflict detection.
- File modification audit events.

### Milestone 4: Commands and Validation

- Command detection.
- Approval prompts.
- Command execution and output capture.
- Failure feedback loop.
- Command audit events.

### Milestone 5: Safety and Admin Controls

- Secret scanner.
- Redaction pipeline.
- Command allowlist and blocklist.
- Restricted file policies.
- Audit retention settings.
- Basic admin policy precedence.

## 17. Key Architectural Decisions

### 17.1 Local-First Client

The client should run indexing, file access, diffs, commands, Git, and audit logging locally. This keeps repository control on the MacBook and reduces unnecessary data exposure.

### 17.2 Explicit Approval Gates

File writes, risky commands, dependency changes, Git state changes, pushes, pull requests, and external side effects require explicit approval. This protects the user's workspace and makes behavior explainable.

### 17.3 Adapter-Based Model Integration

The first provider is OpenAI-compatible, but model calls should go through a provider-neutral adapter. This avoids coupling product behavior to one API shape.

### 17.4 Policy Before Execution

Every action must pass a policy decision before execution. The model can suggest; the client validates, requests approval, executes, logs, and reports results.

### 17.5 Structured Patches Instead of Direct Writes

Generated edits should be represented as structured patches. Direct model-to-file writes are not allowed. This enables diff preview, conflict checks, selective apply, and audit logging.

## 18. Open Questions

- Should MVP be a fully native SwiftUI app, an Electron app, or a Tauri app? A native app gives best macOS integration; Electron may speed UI development; Tauri may offer a smaller footprint.
- Which semantic search backend should be used locally for embeddings and vector search?
- Should full prompt payloads be retained for debugging, or should logs store metadata only by default?
- Which macOS versions must be supported at launch?
- Should the app provide a bundled shell environment, or always use the user's configured shell?
- Should repository-level assistant rules use an existing file convention or a product-specific config file?
- What admin policy mechanism is required for managed enterprise MacBooks?

## 19. Recommended MVP Technology Direction

For a MacBook-focused MVP, prefer:

- Native macOS or Tauri desktop shell for a small, local-first app footprint.
- A workspace engine written in a memory-safe language such as Swift, Rust, or Go.
- SQLite for local metadata, sessions, audit events, and configuration.
- A local vector index for semantic search.
- Keychain for API keys and encryption keys.
- Git CLI integration initially, with a libgit2-based implementation later if needed.
- Tree-sitter or language-specific parsers for symbol extraction where practical.

The critical architectural requirement is not the exact UI framework. The critical requirement is a strict boundary between model reasoning and client-controlled execution.
