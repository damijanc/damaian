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
| 1 · Remove the Protected paths panel (D1) | Done 2026-11-11 | Removed `#protected-paths-panel` from `index.html`, its CSS block from `style.css`, and from `app.js` `renderProtectedPathsControl`, `addProtectedPaths`, `removeProtectedPath`, their helpers, the `clearChat` reset and the form's submit listener. `/api/session` no longer carries `protectedPathsControl` (`protected_paths_control_json` deleted). **Deviation:** the shell routes `GET /api/protected-paths`, `POST /api/protected-paths-add` and `/api/protected-paths-remove` were removed too (only the panel called them; nothing in `desktop-app`, the worker or the CLI did), with `handle_protected_path_change` and `parse_protected_path_lines`; they now answer 404. Side effect worth knowing: `/api/session` no longer checks its `repo` parameter against the session — that check existed only through the control (spec 98 Task 4). Engine functions (`task_protected_paths_control*`, `add_/remove_current_task_protected_paths`, …), the CLI control, the worker handoff snapshot and enforcement are unchanged. Worker card: the `/api/workers` row carries `protectedPaths` (the `WorkerRecord.inherited_protected_paths` snapshot, `{path, kind}`); `workerProtectedPathsText` renders "Protected: README.md, docs/" (a directory gets a trailing `/`) under the branch on the Workers board row and after the branch in the worker session's header meta; nothing when none were handed. Spec 98 has a dated note at its top plus inline markers in §2.1, §4 and the Task 4 routes paragraph. Tests: the five route/panel tests of spec 98 Task 4 in `lib.rs` were replaced by `the_protected_paths_control_is_gone_from_the_session_payload_and_page` (no payload key, routes 404, no markup/CSS/handlers, the engine still holds the task's paths) and `worker_supervisor::a_workers_card_lists_the_protected_paths_it_was_handed`; the handoff test `a_parent_task_snapshot_crosses_the_handoff_and_prompt_text_cannot_broaden_it` is unchanged. Checked in the running UI (the `serves_the_ui_for_manual_inspection` server on port 4899 with its own data dir, stopped by PID): a loaded session has no panel and its payload has no `protectedPathsControl`; a board row rendered from a row with two handed paths shows the Protected line, one with none shows nothing; no console errors. Live worker spawn was not exercised (needs a model). Checks (local, 2026-11-11): `cargo clippy -p desktop-shell -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell` 302 passed, 20 skipped; spec 98's engine tests (`-p workspace-engine -E 'test(protected) \| …'`) 32 passed; `scripts/quality-gate.sh fmt node lint typos deny` ok (`cargo deny` run outside the sandbox for its advisory-db lock; Biome's one warning is the pre-existing `.findings-panel` specificity note). Left for Task 7: `USER_GUIDE.md` / `TROUBLESHOOTING.md` (neither mentions the panel today). Workspace clippy and tests left to CI. |
| 2 · Remove Run in a worker from the plan card (D2) | Not started — button removed by spec 107 Task 5 | Spec 107 Task 5 (2026-11-10) removed the always-on button from `createPlanReview`: the plan card now shows **Run in a worker** only inside the engine's one-time `workerSuggestion` row for a code-changing plan (see spec 107's Task 5 row), and desktop-shell `app_js_suggests_a_worker_only_where_the_engine_says` asserts the card has no unconditional button. Left for this task: spec 94's dated note and the user guide's plan review section. |
| 3 · Archive: engine and routes (D3) | Done 2026-11-10 | `Session.archived`, replayed from `session_archived` / `session_unarchived` events (each carries the full session payload with the flag; the event type decides, so a payload cannot contradict it). A log from before spec 108 reads as not archived; rename keeps the flag. `archived` is `#[serde(default)]` because a pending turn on disk serialises its `Session`. `archive_session(id, worker_running)` / `unarchive_session(id)` on `SessionStore`; repeating either writes nothing; `updated_at_ms` is left alone so an unarchived session returns to its place. `worker_running` is supplied by the shell from `workers::global().is_alive` (the engine cannot see the supervisor); refused with `ARCHIVE_RUNNING_WORKER_REFUSAL` ("Stop the worker first…"), which the route returns as 409. **Deviation:** `list_sessions` keeps its signature and returns every session; the filter is `list_sessions_filtered(repo, ArchivedFilter::{Include,Exclude,Only})`, so recovery and existing callers are unchanged. `GET /api/sessions` takes optional `archived=include|exclude|only` (default include, so the current UI is unchanged until Task 4); every session object (listing and `/api/session`, via `session_fields_json`) carries `archived`. `POST /api/session-archive` / `/api/session-unarchive` take `session_id` and answer `{session}`; unknown session → 400. Delete is unchanged and deletes an archived session. Tests: `workspace-engine/tests/session_archive.rs` (8: round-trip across a fresh store, rename keeps the flag, idempotence, filters, running worker refused, unknown session, delete, legacy log, a serialised `Session` without the field); desktop-shell `session_archive_routes_round_trip_and_filter_the_listing`, `session_archive_route_refuses_an_unknown_filter_and_session`, `archiving_a_running_worker_is_a_conflict`. For Task 4: the sidebar should call `/api/sessions?archived=exclude` and `?archived=only` (or one include call split on the flag). Checks (local, 2026-11-10): `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell` 311 tests (291 passed, 20 skipped by default filter, the 3 new ones included); `cargo nextest run -p workspace-engine` filtered to the session store, pending-turn and batch tests 183 passed; `scripts/quality-gate.sh fmt node lint typos deny` ok. Workspace clippy and tests left to CI. |
| 4 · Archive: sidebar, menu and board (D3) | Done 2026-11-11 | `app.js`: `sessionArchiveMenuItem` puts **Archive** (or **Unarchive** for an archived session) in the sidebar row ⋮ (`toggleSessionMenu`), the header ⋮ (`toggleThreadMenu`) and the worker ⋮ (`toggleWorkerMenu`), in the Rename/Export section; Delete stays alone in the danger section. `setSessionArchived` posts `/api/session-archive` / `-unarchive` and reloads the project's sessions (and workers). **Deviation from the Task 3 hint:** the sidebar keeps one `/api/sessions` call (default `include`) and splits on `archived` client-side, so `projectSessionsByPath` still holds every session — `currentSession()`, `loadSessions` (opening an archived session) and the search label read it. `renderProjectList` lists only unarchived sessions and workers; `renderArchivedEntry` adds a collapsed **Archived (N)** toggle at the bottom of each project (open state in memory only, so it starts collapsed), listing archived sessions and archived workers newest first, each with Open (its title) and **Unarchive**. Search (`renderSessionSearchResults`) appends an "Archived" label to hits from archived sessions. Workers board: hides `row.archived` by default; a **Show archived (N)** / **Hide archived** button in the head when any are hidden (`workersBoardShowArchived`, not stored). **Shell change:** `/api/workers` rows carry `archived` (`workers.rs`), needed because worker sessions live under their worktree, not in the project's `/api/sessions`. The running-worker refusal: the worker menu's Archive stays enabled and the 409 message ("Stop the worker first…") is shown through `runSessionAction`'s toast, unchanged. CSS: `.project-archived-toggle`, `.project-archived-item`, `.session-search-hit-archived`. Tests: `app_js_archives_sessions_out_of_the_sidebar` (menus, sidebar filter, Archived entry, search label, board default) and `worker_supervisor::a_worker_row_carries_its_archived_flag`. Checked in the running UI (`serves_the_ui_for_manual_inspection`, port 4899, its own data dir, stopped by PID): Archive from a row ⋮ moved the session into a collapsed Archived (1); opening it from there loaded it and the header ⋮ offered Unarchive; archiving another from the header ⋮ gave Archived (2); a search for "retry" labelled that session's hits Archived; Unarchive returned the first to the list; a page reload kept the remaining one archived; an archive error from the route (unknown session, 400) was shown as the route's message. The board was checked by rendering it from two injected rows (workers are off in that fixture): one archived row hidden, Show archived (1) revealed it. Not exercised: a live running worker's 409 in the UI (route covered by Task 3's `archiving_a_running_worker_is_a_conflict`). Checks (local, 2026-11-11): `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell` 304 passed, 20 skipped (run outside the agent sandbox for loopback binds; 17 s wall); `scripts/quality-gate.sh fmt node lint typos deny` ok (Biome's one warning is the pre-existing descending-specificity note). Workspace clippy and tests left to CI. Left for Task 7: user guide and troubleshooting text for archive. |
| 5 · Git status card names each path once (D4) | Not started | — |
| 6 · Record the model of each call (D5) | Done 2026-11-10 | `task_usage_recorded` carries optional `modelProvider`/`modelName`; both absent when the call's model is unknown (a call lost to a crash), so such an event reads exactly like one written before spec 108. `record_task_usage` / `record_task_usage_for_task_id` take `model: Option<CallModel>` as their second/third argument (`CallModel` is borrowed; `CallModel::configured(&config)`). **Deviation/discovery:** callers record the *configured* provider and model, not `ModelRun.provider`/`model` — the adapter's provider is its own name (`mock` in tests), which would have made every mocked turn look like a switch; the config is the same source `create_task` uses, and a resumed round's engine is built from the current config. Read side: `TaskUsage.models: Vec<TaskModel>` (`{provider, name}`), derived in `read_task_usage`: the `task_created` model first, then each distinct model the calls named in order of first use; `read_budget_usage` unions the chain's models. `TaskUsage` is no longer `Copy` (it now owns a `Vec`); `.copied()` callers in `chat.rs`, `task_outcomes.rs` and `eval-harness/src/runner.rs` became clones/borrows. Shell: there was no per-task model in `/api/session` or the UI before, so "where the task's model is shown" became the usage payload — `task_usage_json` (the one funnel for the live turn response, the reloaded session's task entries and recovery) always carries `"models":[{provider,name}…]`. UI: the turn's usage line (`markMessageUsage`, via `turnModelsText`) adds the model names joined by " → ", one name for a single-model turn; nothing else in `app.js` changed (spec 107 Task 3 owns the status line and plan panel). Recovery's spend line does not show models. Tests: `token_accounting.rs` `models_used` (4: mid-task switch records both, starting model first even when no call used it, a hand-written log without the fields reads, an unknown model writes no fields); desktop-shell `a_resume_on_a_switched_model_records_both_models` (a question paused on the fixture model, answered through an engine built from a `deepseek`/`deepseek-v4-pro` form as the answer route does; usage lists both and `task_usage_json` carries them), `the_session_payload_carries_every_model_a_task_used`, and `the_session_payload_carries_each_tasks_usage` now asserts `"models":[]`. Checks (local, 2026-11-10): `cargo clippy -p workspace-engine -p desktop-shell -p eval-harness --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell -p eval-harness` 444 passed (22 skipped by default filter; port-binding tests need to run outside the agent sandbox); `cargo nextest run -p workspace-engine` 2626 passed (30 skipped); `scripts/quality-gate.sh fmt node lint typos deny` ok. **Not done:** the usage line was not checked in the running app. Workspace clippy and tests left to CI. |
| 7 · Documentation and close-out | Not started | — |
| 8 · Plan review: read-only steps, decline asks for changes (D6) | Not started | — |

## Task 1 · Remove the Protected paths panel (D1)

- [x] Remove `#protected-paths-panel` markup, its CSS and `app.js` handlers;
      drop the control from `/api/session`.
- [x] Keep the engine functions and CLI; list a worker's protected paths
      read-only on its card.
- [x] Spec 98: dated note. Tests: no panel in the payload; spec 98's
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

- [x] **Archive** in the session ⋮ menu; archived sessions leave the sidebar;
      a collapsed **Archived (N)** entry per project with Open and **Unarchive**;
      search labels archived results; the Workers board hides archived workers.
- [x] Manual check in the running app; UI tests.

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

## Task 8 · Plan review: read-only steps, decline asks for changes (D6)

Build before Task 7, so the docs describe the new card.

- [ ] `createPlanReview`: read-only numbered steps with their checks as text;
      remove the title inputs, ↑ ↓ ×, the check editor and the raw
      `deferredAction` chip (a muted "Next: …" line at most).
- [ ] Decline opens "How should the plan be changed?" (text field, **Send**,
      and **Decline without changes**); the card stays the pending decision
      (spec 107) until one is sent.
- [ ] Engine and route: `changes` on `/api/resume-plan`; `changes_requested`
      decision keeps steps outstanding and asks the model for a revised plan;
      empty `changes` keeps today's decline.
- [ ] Tests: engine — changes requested → no patch applied, a new plan review
      pauses with the revised plan, steps not skipped; decline without changes
      unchanged; desktop-shell — the card has no editing controls, Decline
      shows the question; a deterministic eval scenario for the round trip.
- [ ] Dated notes in specs 66 and 105; manual check in the running app.

Files: `app.js`, `style.css`, `crates/desktop-shell/src/lib.rs`,
`crates/workspace-engine/src/chat.rs`, tests, eval scenario.
Checks: `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test plan_turn`; `cargo nextest run -p desktop-shell`;
`cargo run -p eval-harness -- run --tier deterministic`; web checks.

## Task 7 · Documentation and close-out

- [ ] `USER_GUIDE.md`: archive, the plan card (read-only steps, Decline asks how to change it), where protected paths now show;
      `TROUBLESHOOTING.md`: finding an archived session; why a patch was refused
      without a visible Protected paths panel.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.
