# Spec 110 context

**Specified:** 2026-11-11. Line numbers are approximate; re-read before editing.

## Evidence

Spec 33 Task 1 restart on the release after spec 109 Task 2 (2026-11-11): the
owner archived two old sessions from the sidebar while Damaian worked; the UI
did not change. Their logs (`session_18e6ada3…`, `session_18e69e39…`) record
`session_archived` at 20:36:02.522 and .529: both clicks executed together
when the turn ended. Local OBSERVATIONS 94.

## Survey

- **Accept loop.** `serve` (`desktop-shell/src/lib.rs` ≈160) iterates
  `listener.incoming()` and calls `handle_connection` inline: one connection at
  a time.
- **Turns.** `stream_turn` (≈6734) runs the turn on a `damaian-turn` thread but
  relays its events with `relay_turn_events` on the calling (accept) thread
  until the turn ends, then joins the worker. The accept loop is blocked for
  the whole turn. Stop works today only because the relay notices the client
  disconnect and fires the turn's `CancelToken`.
- **Engine per request.** Routes build a `WorkspaceEngine` per request
  (`default_engine()` ≈4884); there is no shared engine lock. The single
  thread has been the only thing serialising work.
- **Desktop app.** `desktop-app` embeds the shell; check whether it calls the
  same `serve`.
- **Workers** (spec 57) already run each worker session in its own process:
  `workers.rs` spawns the same executable re-entered with `--worker`
  (`worker.rs`), registers it with the process registry (spec 46), hands the
  API key over stdin, holds stdin as the lifeline and talks to it over a
  control socket; the host runs turns through the same `ChatOrchestrator`
  entry points. Progress reaches the UI through `worker_progress`. Several
  workers already write the same data directory at once, so cross-process
  safety of session logs, the audit log and the process registry exists in
  part. A worker always works in its own worktree; a session host would work in
  the session's own checkout.

## Decisions

- **D1 — the shell serves the UI; each session works in its own process
  (decided by the owner, 2026-11-11).** The shell serves each connection on
  its own thread and never runs a turn itself. A session's work goes to a
  session host process for that session, built on spec 57's worker host
  (`--worker` re-entry, registry entry, stdin key handover and lifeline,
  control socket, progress file), started on the session's first work request
  and kept while the session is open or working. The host owns the session's
  lane: one turn at a time; a work request while it is busy gets HTTP 409 with
  the running work's label. The shell relays the host's progress to the open
  window as today's stream does. Whether ordinary sessions and workers share
  one host type (a worker = a host in a worktree) is decided in Task 1.
- **D2 — classification.** `RouteClass { SessionWork(session_id), Ui }` by an
  exhaustive match over the route table. Session work: turn streams and every
  resume route (answer, plan review, patch decision, command/check/baseline
  approval, continue repair), command and check runs, verification, patch
  apply/reject, rewind/checkpoint restore, starting a worker from a session.
  UI: everything else, including rename, settings and config writes, pins and
  context restriction. Archive and delete are UI routes that refuse a session
  whose lane is busy (requirement 3); today's running-worker refusal for
  archive stays.
- **D3 — reaching running work.** Each host reports its session id, a label
  ("Running `cargo test` in crates/workspace-engine", "Waiting for the model")
  and accepts a cancel on its control socket. A UI route can list working
  sessions and cancel one; spec 109 Task 6 adds the Stop route that uses it.
- **D4 — shared state across processes (Task 2 audits each).** Session logs: append-only JSONL;
  confirm appends are single `write` calls and readers skip an incomplete
  trailing line. Pending turns: `load`/`take` of one proposal must not succeed twice (the
  session's host is the only process that resumes it; prove it).
  Audit log and process registry: already shared with worker processes. Index cache, repository-map store, LSP hub, vector index:
  shared per repository; make access safe for two host processes in one
  project (per-process caches are fine; on-disk stores need file locking or
  atomic replace).
  Config writes: last write wins, as today. Two sessions in one checkout: the
  patch engine's hash check already refuses a stale patch; no new guarantee.
- **D5 — thread stacks.** Connection threads use `spawn_with_shell_stack` (the
  shell's larger stack).
- **D6 — crash isolation.** A host that dies leaves the shell and other
  sessions running; its session shows spec 17's crash recovery; quitting the
  app ends every host through its lifeline and is not reported as a crash
  (see OBSERVATIONS 85 on duplicate crash cards).
