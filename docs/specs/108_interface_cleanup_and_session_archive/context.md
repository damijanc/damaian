# Spec 108 context

**Specified:** 2026-11-10. Line numbers are approximate; re-read before editing.

## Evidence

Local observations 71, 72, 74, 75 and 77 (`docs/PLAN/OBSERVATIONS.md`, not
committed), from the owner's screenshots of the spec 33 planning and Task 1
sessions (2026-11-09/10, v0.64.0–v0.66.0) and their session logs.

## Survey: what exists today

- **Protected paths panel.** Markup in `index.html` ≈174–200, after
  `#chat-log`. `renderProtectedPathsControl` (`app.js` ≈7327) un-hides it from
  the session payload (≈15125). The control comes from
  `protected_paths_control_json` (`lib.rs` ≈911) in `/api/session` (≈760),
  which calls `WorkspaceEngine::task_protected_paths_control`
  (`workspace_engine.rs` ≈788). That picks the newest task **of the viewed
  session** (≈794–795). Add/remove go through `add_current_task_protected_paths`
  (≈852). Observation 71's "another session's task" sighting was most likely a
  stale panel that did not refresh; it no longer matters once the panel goes.
- **The context panel's path field is a different control.** "Restrict this
  session to these paths" (`index.html` ≈244–262, `/api/context-restriction`)
  is an allow-list of what the model may *see*; protected paths are a deny-list
  of what patches may not *change*. They must not be merged.
- **Run in a worker.** Added in `createPlanReview` (`app.js` ≈13927–13934)
  before Decline when `!decideInWorker && workersEnabled && currentSessionId &&
  !isWorkerSession(currentSessionId)` (spec 94).
- **Sessions.** `toggleSessionMenu` (`app.js` ≈11550): Rename, Export as
  Markdown, Delete. Delete goes through `deleteSessionForProject` (≈11640) →
  `POST /api/session-delete` → `handle_session_delete` (`lib.rs` ≈1540), which
  stops child workers and calls `engine.delete_session`. The sidebar list is
  `GET /api/sessions` → `handle_sessions` (≈2752) →
  `SessionStore::list_sessions` (`session.rs` ≈1174), sorted by `updated_at_ms`.
  `Session` (`session.rs` ≈27–39) has no archived or hidden flag.
- **Git status card.** `renderGitStatusText` (`app.js` ≈12165) prints
  `` `- ${file.raw} ${file.path}` `` (≈12172). `raw` is the whole porcelain
  record, status plus path (`git_service.rs` `parse_porcelain_z` ≈1520), so
  the path prints twice.
- **Model record.** `SessionStore::create_task` (`session.rs` ≈788–806) stores
  provider and model once. Replay never changes them (≈1784–1795, "Nothing ever
  *changes* these"). Resumed rounds use the current config (`chat.rs` ≈2286,
  ≈4146), so a mid-task switch changes the calls but not the record.
  `task_usage_recorded` carries no model.

## Decisions

- **D1 — remove, don't move, the Protected paths control (decided by the owner,
  2026-11-10).** "No one will manually type the paths to protect." The engine
  routes stay for the CLI and the worker handoff; the `/api/session` payload
  drops the control; the panel markup, CSS and handlers are removed. Read-only
  display: the worker card lists the protected paths it was handed, and the
  existing refusal text already names the path.
- **D2 — remove Run in a worker from the plan card (decided by the owner).**
  Spec 94's user-guide text and its tests are updated; spec 94 gets a dated
  note. The worker handoff from `worker-prompt` blocks and **New worker** is
  unchanged. The always-on button is replaced by spec 107 D3's conditional
  suggestion (decided 2026-11-10), shown once per code-changing task and never
  for a planning- or documentation-only plan. Build 108 Task 2 and 107 Task 5
  so the two do not both offer it.
- **D3 — where archived sessions live (recommended).** A durable
  `session_archived` / `session_unarchived` event and an `archived` flag on
  `Session`. `list_sessions` gains a filter; the sidebar shows unarchived
  sessions and a collapsed **Archived (N)** entry at the bottom of each
  project; search includes archived sessions, labelled. Archiving a running
  worker is refused ("Stop the worker first"); the Workers board hides archived
  workers by default.
- **D4 — Git status card.** Render `file.raw` alone when present, else the
  status and path, so each path appears once.
- **D5 — per-call model (recommended).** Add `modelProvider`/`modelName` to
  `task_usage_recorded`, and derive "models used" from usage records, keeping
  `task_created`'s model as the starting model. No new event type.
