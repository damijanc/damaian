# Spec 108 tasks

**Started:** not yet — specified 2026-11-10

Implementation plan for [`proposal.md`](proposal.md). The decisions and the code
survey are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md). Build after spec 107 or before it, never
  at the same time: both edit `app.js`.
- Protected-path **enforcement** (spec 98) must not change: its engine tests
  pass unchanged.
- Web assets stay vanilla JavaScript; `node --check` and Biome lint pass; UI
  changes are checked in the running app and recorded in the task row.
- Scoped checks per task; the workspace gate stays with CI.

| Task | State | Durable result |
|---|---|---|
| 1 · Remove the Protected paths panel (D1) | Not started | — |
| 2 · Remove Run in a worker from the plan card (D2) | Not started | — |
| 3 · Archive: engine and routes (D3) | Not started | — |
| 4 · Archive: sidebar, menu and board (D3) | Not started | — |
| 5 · Git status card names each path once (D4) | Not started | — |
| 6 · Record the model of each call (D5) | Not started | — |
| 7 · Documentation and close-out | Not started | — |

## Task 1 · Remove the Protected paths panel (D1)

- [ ] Remove `#protected-paths-panel` markup, its CSS and `app.js` handlers;
      drop the control from `/api/session`.
- [ ] Keep the engine functions and CLI; list a worker's protected paths
      read-only on its card.
- [ ] Spec 98: dated note. Tests: no panel in the payload; spec 98's
      enforcement and handoff tests pass unchanged.

Files: `crates/desktop-shell/static/{index.html,app.js,style.css}`,
`crates/desktop-shell/src/lib.rs`, `docs/specs/98_task_scoped_protected_patch_paths.md`.
Checks: `node --check crates/desktop-shell/static/app.js`; `npm run lint:web`;
`cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p desktop-shell`.

## Task 2 · Remove Run in a worker from the plan card (D2)

- [ ] Remove the button and its handler from `createPlanReview`.
- [ ] Update spec 94 (dated note) and the user guide's plan review section.
- [ ] Tests: the plan card offers Approve and Decline only.

Files: `app.js`, desktop-shell tests, `docs/specs/94_run_a_plan_in_a_worker.md`,
`docs/USER_GUIDE.md`. Checks: as Task 1.

## Task 3 · Archive: engine and routes (D3)

- [ ] `session_archived` / `session_unarchived` events; `Session.archived`;
      `list_sessions` filter; refuse archiving a running worker.
- [ ] `POST /api/session-archive` and `/api/session-unarchive`; `/api/sessions`
      returns the flag.
- [ ] Tests: archive/unarchive round-trip survives a fresh store; listing
      filters; a running worker is refused; delete still deletes an archived
      session.

Files: `crates/workspace-engine/src/session.rs`, `workspace_engine.rs`,
`crates/desktop-shell/src/lib.rs`, tests.
Checks: `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
scoped `cargo nextest run` for the session store and routes.

## Task 4 · Archive: sidebar, menu and board (D3)

- [ ] **Archive** in the session ⋮ menu; archived sessions leave the sidebar;
      a collapsed **Archived (N)** entry per project with Open and **Unarchive**;
      search labels archived results; the Workers board hides archived workers.
- [ ] Manual check in the running app; UI tests.

Files: `app.js`, `index.html`, `style.css`, desktop-shell tests. Checks: as Task 1.

## Task 5 · Git status card names each path once (D4)

- [ ] `renderGitStatusText` prints each path once with its status.
- [ ] Test with a deletion, a modification and an untracked directory.

Files: `app.js`, desktop-shell tests. Checks: as Task 1.

## Task 6 · Record the model of each call (D5)

- [ ] Add the model to `task_usage_recorded`; expose "models used" where the
      task's model is shown.
- [ ] Tests: a mid-task switch records both models; older logs without the
      field still read.

Files: `crates/workspace-engine/src/session.rs`, `chat.rs`, `lib.rs`, `app.js`, tests.
Checks: `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
scoped nextest; web checks.

## Task 7 · Documentation and close-out

- [ ] `USER_GUIDE.md`: archive, the plan card, where protected paths now show;
      `TROUBLESHOOTING.md`: finding an archived session; why a patch was refused
      without a visible Protected paths panel.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.
