# Spec 110 tasks

**Started:** not yet — specified 2026-11-11

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
| 1 · Thread per connection, a host process per session, route classes (D1–D3, D5, D6) | Not started | — |
| 2 · Shared state across processes (D4) | Not started | — |
| 3 · The UI with several working sessions | Not started | — |
| 4 · Documentation and close-out | Not started | — |

## Task 1 · Thread per connection, a host process per session, route classes (D1–D3, D5, D6)

- [ ] Decide and record whether ordinary sessions reuse spec 57's worker host
      as is (a host without a worktree) or share a common host type.
- [ ] Serve each connection on its own thread (`spawn_with_shell_stack`).
- [ ] `RouteClass` by exhaustive match; session-work routes go to the
      session's host (started on first need) or get 409 with its label; the
      shell relays the host's progress to the window.
- [ ] Archive and delete refuse a working session ("stop it first").
- [ ] Tests: with a waiting mocked turn, sessions/session/archive of another
      session/rename/settings answer promptly; a second work request for the
      same session is refused at once; two sessions in one project work at the
      same time in two processes; killing one host leaves the other and the
      shell running; every route is classified.

Files: `crates/desktop-shell/src/lib.rs`, `workers.rs`, `worker.rs`,
`desktop-app` if it has its own loop, tests. Checks:
`cargo clippy -p desktop-shell -p desktop-app --all-targets --locked -- -D warnings`;
`cargo nextest run -p desktop-shell`.

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
