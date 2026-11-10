# Spec 108 tasks

**Started:** 2026-11-10

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
| 2 · Remove Run in a worker from the plan card (D2) | Not started — button removed by spec 107 Task 5 | Spec 107 Task 5 (2026-11-10) removed the always-on button from `createPlanReview`: the plan card now shows **Run in a worker** only inside the engine's one-time `workerSuggestion` row for a code-changing plan (see spec 107's Task 5 row), and desktop-shell `app_js_suggests_a_worker_only_where_the_engine_says` asserts the card has no unconditional button. Left for this task: spec 94's dated note and the user guide's plan review section. |
| 3 · Archive: engine and routes (D3) | Done 2026-11-10 | `Session.archived`, replayed from `session_archived` / `session_unarchived` events (each carries the full session payload with the flag; the event type decides, so a payload cannot contradict it). A log from before spec 108 reads as not archived; rename keeps the flag. `archived` is `#[serde(default)]` because a pending turn on disk serialises its `Session`. `archive_session(id, worker_running)` / `unarchive_session(id)` on `SessionStore`; repeating either writes nothing; `updated_at_ms` is left alone so an unarchived session returns to its place. `worker_running` is supplied by the shell from `workers::global().is_alive` (the engine cannot see the supervisor); refused with `ARCHIVE_RUNNING_WORKER_REFUSAL` ("Stop the worker first…"), which the route returns as 409. **Deviation:** `list_sessions` keeps its signature and returns every session; the filter is `list_sessions_filtered(repo, ArchivedFilter::{Include,Exclude,Only})`, so recovery and existing callers are unchanged. `GET /api/sessions` takes optional `archived=include|exclude|only` (default include, so the current UI is unchanged until Task 4); every session object (listing and `/api/session`, via `session_fields_json`) carries `archived`. `POST /api/session-archive` / `/api/session-unarchive` take `session_id` and answer `{session}`; unknown session → 400. Delete is unchanged and deletes an archived session. Tests: `workspace-engine/tests/session_archive.rs` (8: round-trip across a fresh store, rename keeps the flag, idempotence, filters, running worker refused, unknown session, delete, legacy log, a serialised `Session` without the field); desktop-shell `session_archive_routes_round_trip_and_filter_the_listing`, `session_archive_route_refuses_an_unknown_filter_and_session`, `archiving_a_running_worker_is_a_conflict`. For Task 4: the sidebar should call `/api/sessions?archived=exclude` and `?archived=only` (or one include call split on the flag). Checks (local, 2026-11-10): `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell` 311 tests (291 passed, 20 skipped by default filter, the 3 new ones included); `cargo nextest run -p workspace-engine` filtered to the session store, pending-turn and batch tests 183 passed; `scripts/quality-gate.sh fmt node lint typos deny` ok. Workspace clippy and tests left to CI. |
| 4 · Archive: sidebar, menu and board (D3) | Not started | — |
| 5 · Git status card names each path once (D4) | Not started | — |
| 6 · Record the model of each call (D5) | Done 2026-11-10 | `task_usage_recorded` carries optional `modelProvider`/`modelName`; both absent when the call's model is unknown (a call lost to a crash), so such an event reads exactly like one written before spec 108. `record_task_usage` / `record_task_usage_for_task_id` take `model: Option<CallModel>` as their second/third argument (`CallModel` is borrowed; `CallModel::configured(&config)`). **Deviation/discovery:** callers record the *configured* provider and model, not `ModelRun.provider`/`model` — the adapter's provider is its own name (`mock` in tests), which would have made every mocked turn look like a switch; the config is the same source `create_task` uses, and a resumed round's engine is built from the current config. Read side: `TaskUsage.models: Vec<TaskModel>` (`{provider, name}`), derived in `read_task_usage`: the `task_created` model first, then each distinct model the calls named in order of first use; `read_budget_usage` unions the chain's models. `TaskUsage` is no longer `Copy` (it now owns a `Vec`); `.copied()` callers in `chat.rs`, `task_outcomes.rs` and `eval-harness/src/runner.rs` became clones/borrows. Shell: there was no per-task model in `/api/session` or the UI before, so "where the task's model is shown" became the usage payload — `task_usage_json` (the one funnel for the live turn response, the reloaded session's task entries and recovery) always carries `"models":[{provider,name}…]`. UI: the turn's usage line (`markMessageUsage`, via `turnModelsText`) adds the model names joined by " → ", one name for a single-model turn; nothing else in `app.js` changed (spec 107 Task 3 owns the status line and plan panel). Recovery's spend line does not show models. Tests: `token_accounting.rs` `models_used` (4: mid-task switch records both, starting model first even when no call used it, a hand-written log without the fields reads, an unknown model writes no fields); desktop-shell `a_resume_on_a_switched_model_records_both_models` (a question paused on the fixture model, answered through an engine built from a `deepseek`/`deepseek-v4-pro` form as the answer route does; usage lists both and `task_usage_json` carries them), `the_session_payload_carries_every_model_a_task_used`, and `the_session_payload_carries_each_tasks_usage` now asserts `"models":[]`. Checks (local, 2026-11-10): `cargo clippy -p workspace-engine -p desktop-shell -p eval-harness --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell -p eval-harness` 444 passed (22 skipped by default filter; port-binding tests need to run outside the agent sandbox); `cargo nextest run -p workspace-engine` 2626 passed (30 skipped); `scripts/quality-gate.sh fmt node lint typos deny` ok. **Not done:** the usage line was not checked in the running app. Workspace clippy and tests left to CI. |
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

- [x] Remove the button and its handler from `createPlanReview` (done by spec 107 Task 5; the handler now serves the suggestion row).
- [ ] Update spec 94 (dated note) and the user guide's plan review section.
- [ ] Tests: the plan card offers Approve and Decline only.

Files: `app.js`, desktop-shell tests, `docs/specs/94_run_a_plan_in_a_worker.md`,
`docs/USER_GUIDE.md`. Checks: as Task 1.

## Task 3 · Archive: engine and routes (D3)

- [x] `session_archived` / `session_unarchived` events; `Session.archived`;
      `list_sessions` filter; refuse archiving a running worker.
- [x] `POST /api/session-archive` and `/api/session-unarchive`; `/api/sessions`
      returns the flag.
- [x] Tests: archive/unarchive round-trip survives a fresh store; listing
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

- [x] Add the model to `task_usage_recorded`; expose "models used" where the
      task's model is shown.
- [x] Tests: a mid-task switch records both models; older logs without the
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
