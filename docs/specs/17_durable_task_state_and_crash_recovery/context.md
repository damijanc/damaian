# Context: Durable Task State and Crash Recovery

Why this spec exists and what the code looks like today. The decision is in
[`proposal.md`](proposal.md); the execution order is in [`tasks.md`](tasks.md).

## 1. Motivation

`TaskStatus::Running` covers context preparation, the model call, tool
execution, patch application, and validation, indiscriminately
(`crates/workspace-engine/src/session.rs:21-29`). A task killed while `Running`
therefore carries no information about what was in flight — and the question
that matters after a crash is exactly that: did the patch apply? did the command
run? was the model charged for a call whose answer was lost?

The consequence is worse than a poor status string. Without knowing whether an
action completed, there are only two options at restart, and both are wrong:
retry the action, which may apply a patch twice or run `npm publish` a second
time, or drop the task silently, which leaves the user with a half-applied
change and no record. Damaian currently does the second: nothing reconciles
incomplete tasks at launch, so a task that was `Running` when the app died stays
`Running` in the log forever and the UI shows a turn that never finishes.

[Spec 08](../08_stop_and_progress.md) made a turn stoppable by the user. This work
package makes a turn survivable when the stop was not the user's idea.

## 2. Current State

- **`TaskStatus` has seven variants**: `Created`, `Running`,
  `WaitingForApproval`, `Failed`, `Complete`, `Cancelled`,
  `ToolBudgetExhausted` (`session.rs:21-29`), with string forms in
  `TaskStatus::as_str` (`session.rs:32-42`).
- **`Task` is a thin record**: id, session id, status, user prompt, provider,
  model, created and completed timestamps (`session.rs:46-56`). Nothing records
  what phase the work reached or what action was in flight.
- **Sessions are one append-only JSONL event log.** `SessionStore`
  (`session.rs:68`) appends `task_created`, `task_status_updated`, and
  `message_appended` events. Nothing is ever rewritten in place.
- **Tasks are replayed, not stored.** `read_task_statuses` (`session.rs:237`)
  scans the log with `line.contains("\"eventType\":\"task_created\"")` and
  `json_string_field`, letting a later event overwrite an earlier one. Its doc
  comment records this design explicitly.
- **Events have no sequence number.** Order is line order, and there is no
  monotonic identifier to reconcile against.
- **Readers are substring-based and torn-line-tolerant only by accident.**
  `read_messages` (`session.rs:219`) and `read_task_statuses` filter lines with
  `contains` and parse fields individually, so a truncated final line is not
  detected as truncated — it is parsed for whatever fields survive.
- **Cancellation exists.** `CancelToken` in
  `crates/workspace-engine/src/cancel.rs`, wired through the chat loop by
  [spec 08](../08_stop_and_progress.md).
- **Command proposals already persist.** `CommandStore::save_proposal` and
  `load_proposal` (`crates/workspace-engine/src/validation.rs:49-61`) write and
  read a proposal by id under the data directory. Patch proposals persist under
  `<data_dir>/patches/` (`crates/workspace-engine/src/edit.rs:66`,
  `edit.rs:132`). What is missing is not the proposal — it is any record linking
  an interrupted task to the proposal it was waiting on.
- **Commands cannot orphan a process.** `CommandRunner` runs the configured
  shell with `Command::output()` (`crates/workspace-engine/src/command_runner.rs:89-93`),
  which blocks until exit and reaps the child.
- **Three things can orphan a process**: MCP stdio servers, spawned at
  `crates/workspace-engine/src/mcp.rs:323` with no kill-on-drop guard; the
  `curl` child used for model calls, wrapped in `KillOnDrop`
  (`crates/workspace-engine/src/model.rs:400-406`), which protects a graceful
  drop but not a `SIGKILL`; and PTY sessions held in a process-global map in
  `crates/desktop-shell/src/terminal.rs:31`.
- **The audit log records events with redacted fields**
  (`crates/workspace-engine/src/audit.rs:42`).

