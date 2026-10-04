# Spec 57 Context: Worker Sessions

Companion to [`proposal.md`](proposal.md). This file records why the work exists
and what the code looks like on 2026-10-03, when the design was settled in a
brainstorming session. The proposal holds the decision.

## 1. Where this comes from

The way this repository is actually built is two kinds of session. A
**planner** session owns the specs, the requirements and verification. It writes
a task prompt — usually "Implement Task N from `docs/specs/<feature>/tasks.md`",
the shape [`AGENTS.md`](../../../AGENTS.md) prescribes — and a **worker** session
picks that prompt up in a fresh context and implements one task. When tasks are
independent, several workers run at once, each in its own git worktree.

Today the handoff is copy and paste between independent sessions in another
tool. Damaian cannot host this workflow at all, and the reason is not the
handoff — it is watching. In use, agents have been caught doing things they
should not and looping on the same failing step, which is why fire-and-forget
delegation is not acceptable here. A worker is only safe to start if the user
can see what it is doing, pause it, correct it, and stop it, and if nothing it
started can outlive the application.

That ordering came out of the brainstorming session explicitly: **watching
first, handing off second.** This spec is the watching foundation plus the
smallest possible spawn. Planner-drafted handoffs, loop detection, and letting a
planner read a worker's result are deliberately later specs (§7 of the
proposal).

## 2. Why this is not spec 38

[#38](../38_subagent_model.md) is delegation *by the agent*: a parent turn hands
part of itself to a child that runs inside the parent's task, under the parent's
capability, and returns a result the parent is accountable for. Its readiness
gates exist because that is a second execution path.

A worker session is none of that. The **user** starts it, it is an ordinary
session with its own approvals, mode, audit trail and session log, and nothing
flows from a worker back to the planner — the repository is the only channel,
exactly as `AGENTS.md` already requires between task sessions. The planner is
only where the user happened to be standing when they started it. So #38's
readiness gates do not apply, and this spec does not make #38 more or less
likely to be built.

What this spec does inherit from #38 is the one rule that matters: **a worker is
a different host for the same turn, never a different turn.** §5.2 of the
proposal makes that a tested requirement.

## 3. Current state

### 3.1 The shell runs one request at a time

`run_server_with_ready` accepts connections in a plain `listener.incoming()` loop
and calls `handle_connection` inline
([`desktop-shell/src/lib.rs:112`](../../../crates/desktop-shell/src/lib.rs)). While
an `/api/ask-stream` response is open, no other request is served — not even
`GET /api/session`. Switching sessions in the UI during a turn stalls until the
turn ends. There is no registry of running turns: `stream_turn` creates a
`CancelToken`, runs the turn on a worker thread, relays its events over an mpsc
channel to the socket, and joins the thread before returning.

This is why the design hosts workers in **separate processes** rather than as
background threads in the shell. A background-thread design was drafted first
and abandoned during the session: it needed the shell made multi-threaded, a
run registry, and an audit of every route that touches a session log for races
the single-threaded server has been hiding. A process per worker needs none of
that — the shell stays single-threaded and observes workers through files and
short socket exchanges.

### 3.2 Stop is a disconnect

There is no stop route. The UI aborts its `fetch`; the relay notices the failed
SSE write or keepalive and calls `cancel.cancel()`
([`lib.rs` `relay_turn_events`](../../../crates/desktop-shell/src/lib.rs)). The
engine checks the token at the top of each round, before the model call
([`workspace-engine/src/chat.rs:1574`](../../../crates/workspace-engine/src/chat.rs)),
and while streaming. There is no pause. `TurnSink`
([`chat.rs:204`](../../../crates/workspace-engine/src/chat.rs)) carries
`on_token`, `on_progress` and `cancel`.

### 3.3 Turn state that already survives a turn

- `TaskStatus` (`session.rs`) has fourteen variants, persisted as
  `task_status_updated` events in `<data_dir>/sessions/<id>.jsonl`.
- `PendingApprovalRef { kind, proposal_id }` is written into the status event by
  `await_approval`; today only crash recovery reads it back.
- A turn stopped for a command or plan approval writes its full state —
  messages, round, the pending call, options — to
  `<data_dir>/chat/pending/<proposal_id>.json`
  ([`chat.rs:3394`](../../../crates/workspace-engine/src/chat.rs)). Resume builds a
  fresh engine and loads that snapshot; `take()` deletes it, so a decision
  resumes once. **Resume is already stateless across processes.**
- A patch proposal ends the turn. `/api/apply-patch` applies the stored patch
  through `apply_stored_patch`; nothing resumes.

### 3.4 Progress the UI can see

Only the SSE body of the turn's own request: `session`, `phase`
(`{phase, label, round, maxRounds}`), `plan`, `web_diagnostic`, `token`, `done`,
`error`. There is no polling endpoint and no structured list of tool calls. The
sidebar list (`session_summary_json`) carries no running or waiting state.

### 3.5 The audit log is not per-session for tools

`chat.rs` events carry `sessionId`; command, navigation and file-access events
carry only `taskId`. A per-session tool list from the audit log needs a join.
The proposal's progress file supplies that list for display instead; the audit
log stays authoritative and unchanged.

### 3.6 What a separate process loses, and the credential problem

The repository index lives only in memory
([`index_cache.rs`](../../../crates/workspace-engine/src/index_cache.rs)); a new
process rebuilds it and registers the FSEvents watcher. Semantic-search vectors
are on disk; the embedding model reloads. The provider prompt cache is
server-side and survives. None of this has been measured for a worker; §6 of the
proposal records the measurement as informational.

**The API key.** A new session in the UI never prompts for Keychain access
because it runs in the same process: the shell reads the Keychain once
([`desktop-shell/src/keychain.rs`](../../../crates/desktop-shell/src/keychain.rs)),
keeps the key in `MODEL_API_KEY_CACHE`, and every later turn in every session
takes it from there (`resolve_model_api_key`). The worker reuses that cache rather
than the Keychain: the shell passes the cached key over stdin (proposal §5.2), so
a worker never reads the Keychain and never causes a prompt. Passing it in the
environment instead would hand it to every command the worker runs.

**The worker binary.** The `damaian` CLI is not shipped: the Tauri bundle in
[`desktop-app/tauri.conf.json`](../../../crates/desktop-app/tauri.conf.json)
declares no `externalBin`, so an installed `Damaian.app` contains only the app
executable. The CLI also reads only an environment variable for the key
([`damaian-cli/src/main.rs:354`](../../../crates/damaian-cli/src/main.rs)). A
worker is therefore the app's own executable re-launched in worker mode
(proposal §5.2): nothing extra to bundle, sign or notarize, always the same
version as its shell, and the same code-signing identity the user already
granted Keychain access to.

### 3.7 Worktrees

Damaian creates no worktrees. [#36](../36_branch_and_worktree_delivery.md)
leaves creation to "Phase 2 WP4", which is unspecified. #36 does record that a
worktree at a different path is indexed as its own repository. `.damaian/` is
already a repository-controlled directory — it holds the repository config
(`Config::repository_config_path`) — and is in the default index ignore list
([`config.rs:25`](../../../crates/workspace-engine/src/config.rs)).

### 3.8 Process cleanup that already exists

[#46](../46_process_registry_and_orphan_sweep/proposal.md)'s
[`process_registry.rs`](../../../crates/workspace-engine/src/process_registry.rs)
records PIDs and sweeps orphans on launch. macOS has no `PR_SET_PDEATHSIG`, so a
child does not learn its parent died unless it is built to notice.
