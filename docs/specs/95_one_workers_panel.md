# Feature Spec: One Workers Panel

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 95 of 95
Plan: none. A gap reported in use, like #73–#94: an exception to the rule in
[`README.md`](README.md).
Depends on: [#57](57_worker_sessions/proposal.md) (workers and the board) and
[#68](68_ui_chrome_and_command_activity/proposal.md) (the sidebar's Workers
entry), both built.

## 1. Motivation

#57 §5.7 gave each project a **Workers** entry at the top of its sidebar list
and a board per project. In use, the entry repeats what the sidebar already
shows, since each worker is listed under the session that started it. The
board, though, only covers one project, so seeing every running worker means
opening a board per project.

## 2. Requirements

1. The sidebar has no **Workers** entry. Workers stay listed under their
   parent session, or among the sessions when they have none, with their dot
   (#57 §5.7).
2. The header's ⋮ menu has **Workers**, with Worker sessions on. It opens one
   board for every project's workers. The project ⋮ menu keeps **New worker**
   and loses **Workers board**.
3. The board:
   - lists every worker whose record exists, from every source repository;
   - adds a **Project** column naming each worker's source project;
   - keeps #57's groups, columns, row actions and ⋯ menu. Each row acts on
     its own project: **Open** and **Review** open the worker under it.
4. Opening the board does not switch project. The header reads "Workers".
   **New worker** starts a worker in the open project, and is disabled with
   no project open.
5. Each poll also refreshes the sidebar rows of every project that has
   workers, as the per-project poll did for its project.

## 3. Non-goals

- A board inside a project. The Project column says where each worker
  belongs.
- Polling while the board is hidden. The sidebar's rows refresh as before
  (#57 §5.7: on drawing a project, loading a session, focus).

## 4. Design

- **`workers.rs`:** `GET /api/workers` takes `repo` as optional. Without it,
  the route lists every session with a worker record. Each row gains
  `sourceRepository`.
- **`app.js`:**
  - `workersBoardOpen` replaces `workersBoardProject`.
  - `openWorkersBoard()` takes no project. `pollWorkersBoard` reads
    `/api/workers` and hands each project its rows through
    `setProjectWorkerRows`. A project that has no workers left gets an empty
    list.
  - `renderWorkerBoardRow` adds the Project cell and passes
    `row.sourceRepository` to the row's actions. `refreshWorkersAfterAction`
    polls the board while it is open.
  - `renderWorkersEntry` and its sidebar call are removed.
  - `toggleThreadMenu` gains a **Workers** section, hidden while the setting
    is off.
- **`style.css`:** `.project-workers-entry` is removed.
  `.workers-cell-project` is muted and truncates like the planner column.

## 5. Acceptance

- `the_workers_list_covers_every_project_without_a_repo`
  (`desktop-shell`): `GET /api/workers` with no `repo` lists workers from
  two source repositories, and each row names its `sourceRepository`.
- `the_workers_board_is_one_panel_in_the_header_menu` (`desktop-shell`):
  `app.js` has no `renderWorkersEntry`, the header menu offers **Workers**,
  and the board polls `/api/workers` without `repo`.
- The menu and board were checked in the UI inspection server.

## 6. Verification

Run locally:

- `scripts/quality-gate.sh fmt node lint typos deny`
- `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`
- `cargo nextest run -p desktop-shell --locked`

The workspace clippy and test suite are left to CI.
