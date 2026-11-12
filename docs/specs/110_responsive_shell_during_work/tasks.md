# Spec 110 tasks

**Started:** 2026-11-12

Implementation plan for [`proposal.md`](proposal.md). The decisions and the code
survey are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- Each session works in its own host process, one turn at a time (D1);
  sessions run independently, in one project or several.
- No change to what needs approval or how a paused turn is resumed.
- Scoped checks per task; workspace gate on CI. Kill only processes you started,
  by PID; never touch port 4765.

| Task | State | Durable result |
|---|---|---|
| 1 · Thread per connection, route classes, a lane per session (D1a, D2, D3, D5) | Done 2026-11-12 | Split at a clean boundary: this task lands the shell side with each session's work still in the shell process behind an in-process lane; Task 1b moves the work into host processes. D1a (common host type, not the worker host as is) is recorded in context.md. `serve_listener` spawns a `damaian-connection` thread per connection (`spawn_with_shell_stack`). `routes.rs`: a `Route` enum, the `ROUTES` table (the only place a path is spelled), `Route::is_session_work` and the dispatch in `lib.rs` are exhaustive matches with no wildcard. `route_class` resolves the session from `session_id` (`parent_session_id` for `POST /api/workers`), else from a plain `proposal_id`'s paused turn (new `PausedTurns::session_id`); routes that name only a patch, batch, checkpoint or command proposal resolve to no session and run unlaned, as before — Task 1b must resolve them. `lanes.rs`: claim/label/cancel per session; a busy session gets 409 `session_working` with the running label; a turn started without a session adopts its new session's lane on the `session` event; phase labels update the lane. New UI routes `GET /api/working-sessions` and `POST /api/session-work-cancel` (D3; spec 109 Task 6 builds its Stop on them). Archive and delete refuse a working session with "Stop it first" (409). The recovery sweep skips a working session, since a first `/api/recovery` can now arrive mid-turn. Tests: `responsive_tests.rs` (a model gated per test by its model name: UI routes answer under 1 s during a turn, second turn 409, archive/delete 409, two sessions in one project work at once, cancel from a UI request, resume classification), `routes::tests`, `lanes::tests`, `recovery::tests::a_working_session_is_not_swept_until_its_work_ends`, engine `paused_turn_session_tests`. Checks: scoped clippy (desktop-shell, desktop-app, workspace-engine) clean; `cargo nextest run -p desktop-shell` 320 passed; fast checks ok. Deterministic eval tier not run: no turn entry point changed. Workspace clippy and tests left to CI. |
| 1b · A host process per session (D1, D1a, D6) | Not started | — |
| 2 · Shared state across processes (D4) | Not started | — |
| 3 · The UI with several working sessions | Not started | — |
| 4 · Documentation and close-out | Not started | — |

## Task 1 · Thread per connection, route classes, a lane per session (D1a, D2, D3, D5)

Split on 2026-11-12: the host processes are Task 1b. This task's lane is the
interface the host keeps (claim, label, cancel, refusal).

- [x] Decide and record whether ordinary sessions reuse spec 57's worker host
      as is (a host without a worktree) or share a common host type (D1a: a
      common host type).
- [x] Serve each connection on its own thread (`spawn_with_shell_stack`).
- [x] `RouteClass` by exhaustive match; session-work routes hold the session's
      lane or get 409 with its label.
- [x] A UI route lists working sessions and cancels one (D3).
- [x] Archive and delete refuse a working session ("stop it first").
- [x] Tests: with a waiting mocked turn, sessions/session/archive of another
      session/rename/settings answer promptly; a second work request for the
      same session is refused at once; two sessions in one project work at the
      same time; every route is classified.

Files: `crates/desktop-shell/src/lib.rs`, `routes.rs`, `lanes.rs`,
`recovery.rs`, `responsive_tests.rs`; `workspace-engine/src/chat.rs`
(`PausedTurns::session_id`). `desktop-app` calls `run_server_with_ready` and
has no loop of its own. Checks:
`cargo clippy -p desktop-shell -p desktop-app --all-targets --locked -- -D warnings`;
`cargo nextest run -p desktop-shell`.

## Task 1b · A host process per session (D1, D1a, D6)

- [ ] Factor the host plumbing out of `workers.rs` / `worker.rs` (D1a):
      re-entry with a mode flag, registry entry, stdin key handover and
      lifeline, control socket, shutdown and clean quit for every host.
- [ ] A session host per session, started on its first work request in the
      session's own checkout; the shell forwards each session-work request
      whole and the host runs the same `handle_*` function, writing the
      response back for the shell to copy to the window. Route handlers write
      to a stream type both sides can give them, not only `TcpStream`.
- [ ] Resolve every session-work request to its session before forwarding:
      today a request naming only a patch, baseline batch, checkpoint or
      command proposal runs unlaned (`RouteClass::SessionWork(None)`); either
      the UI sends `session_id` on those routes or the shell resolves the id.
- [ ] Each key the shell resolves goes to the host on stdin, never on the
      socket; a provider fallback works.
- [ ] The lane's label and cancel come from the host (D3); the recovery sweep
      and `classify_session` skip a session whose host is alive (widen spec
      57 C12's live-worker check to session hosts).
- [ ] Tests: two sessions in one project work at the same time in two
      processes; killing one host leaves the other and the shell running, and
      the killed session recovers as a crash (spec 17); quitting ends every
      host and is not a crash.

Files: `crates/desktop-shell/src/lib.rs`, `workers.rs`, `worker.rs`,
`lanes.rs`, `desktop-app/src/main.rs` (exit hook), `workspace-engine`
process registry and recovery, tests. Checks: scoped clippy for each crate
touched; `cargo nextest run -p desktop-shell`; the deterministic eval tier.

## Task 2 · Shared state across processes (D4)

- [ ] Audit and fix each item of D4; one concurrency test per item (two
      sessions in two projects; two sessions in one project).
- [ ] Record what was already safe and why in this row.

Files: `crates/workspace-engine/src/session.rs`, `audit.rs`, `process_registry`,
the index/map caches, tests. Checks: scoped clippy; scoped nextest; the
deterministic eval tier.

## Task 3 · The UI with several working sessions

- [ ] `app.js`: a 409 shows its message; sidebar actions during a turn update
      the sidebar at once; working sessions are marked in the sidebar (from
      D3); switching to another session while one works leaves the first
      running and shows its progress again on return.
- [ ] Manual check in the running app (`serves_the_ui_for_manual_inspection`,
      port 4899, its own data dir): archive during a turn; two sessions working.

Files: `app.js`, `style.css`, desktop-shell tests. Checks: web checks;
`cargo nextest run -p desktop-shell`.

## Task 4 · Documentation and close-out

- [ ] `USER_GUIDE.md` / `TROUBLESHOOTING.md`: several sessions at once; the
      archive/delete exception.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.
