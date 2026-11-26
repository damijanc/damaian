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
| 1b · A host process per session (D1, D1a, D6) | Done 2026-11-25 | **Plumbing (D1a):** new `host.rs` holds what both host kinds share — `HostKind` (`--worker` / `--session-host`, socket dirs `workers/` / `hosts/`), handover read/parse, the lifeline (now hands later stdin lines to a callback), `host_socket_path` / `bind_host_socket` (`0600` in `0700`; a directory another host just created is accepted — two hosts starting together raced on it), `tokens_match`, `ShutdownSignal`, `control_token`, `exchange`, `wait_for_exit`, and `start_host` (spawn, register, then hand over). `worker.rs`/`workers.rs` call it; worker behaviour unchanged. **Session host:** `session_host.rs` (`--session-host --session <id> --data-dir <abs>`, both `main`s dispatch it and call `enable_session_hosts(current_exe)`; `desktop-app` quits through `shutdown_all_hosts`). The shell (`session_hosts.rs`) claims the session's lane as before, starts the session's host on its first work request in the session's checkout (`repo`), writes the request as one JSON line on the host's socket and copies the raw HTTP response (JSON or SSE) to the window; the host runs `serve_forwarded_work` → the same `dispatch` under a lane of its own. Handlers now take `&mut dyn Write` (every `stream: &mut TcpStream` in the route files). A closed window closes the host connection, so the host's relay cancels the turn as before. **Resolution:** `route_class` now also resolves `patch_id` (the patch's `origin_session_id`, else `session_id`) and `checkpoint_id` (the manifest's session); a request still without a session (a new session's first message, a pre-session-id patch) goes to a host started under a placeholder id, bound when the stream's `session` event names the session (lane adopted, registry entry re-registered under the session), else ended after that one request. **Keys:** before forwarding, the shell resolves the Keychain references of the request's provider config, the fallback target's on an accepted fallback offer, and the repository's MCP servers, and writes `{"account","key"}` lines on the host's stdin; in a host `resolve_model_api_key`/`resolve_mcp_token` use only those (`keys_from_shell_only`). Patch apply/rollback/reject-files and rewind hand no keys. **Lane (D3, D7):** a lane can be marked remote (`RemoteWork`); label, refusal label, `working()`, cancel and notes then go to the host (`status`/`cancel`/`note` exchanges); a silent host leaves the lane's own label. **Recovery (C12, D6):** `ProcessKind::SessionHost`; `SessionStore::has_live_host` (worker or session host) replaces `has_live_worker` in the engine's `classify_session` and approval reattach. A host that dies mid-response: the shell sweeps its orphans (`sweep_orphaned_processes`), classifies its session into the running sweep snapshot (`recovery::recover_session`, sweep middle factored into `recovered_tasks`), audits `session_host_exited`, and sends the window an `error` event. Quit: each host gets `shutdown`, cancels its work and waits up to 3 s for it to end as stopped, then `sweep_own`; leftovers get `SIGTERM` after 5 s. Archive and delete end the session's idle host. **Deviations:** `POST /api/workers`, `/api/plan-to-worker` and the two recovery-decision routes stay in the shell under the lane (`Route::runs_in_session_host`): the shell's supervisor holds worker lifelines and the sweep snapshot is shell memory. Hosts are kept until archive/delete/quit, not ended when idle (OBSERVATIONS 109). The host checks its effective data directory equals the shell's rather than setting it. Hosting is off in the library's own tests (their scripted models live in-process); the real-process tests turn it on. The standalone shell's signal path still ends hosts without the graceful shutdown (OBSERVATIONS 108). **For Task 2:** two host processes now really write one data directory at once — session logs, paused turns (each resumed only by its session's host; resolution runs in both the shell and the host), the audit log, the process registry (the shell registers hosts, hosts register their children), index/map caches per repository and the carried-notes files; the real-process test exercised two sessions in one project writing separate logs correctly, nothing more. **Tests:** `tests/session_hosts.rs` (`#[ignore]`, real `damaian-desktop-shell` hosts against a held local OpenAI-compatible endpoint): two sessions in one project at once in two hosts (distinct pids, not the shell's), working list and 409 from the shell meanwhile; `SIGKILL` of one host mid-turn → its window gets `error`, the other host and the shell keep answering, `/api/recovery` offers the killed session's turn, a new request starts a new host; a new session's first turn binds its host, a note (200) and a cancel reach it through the host; quit ends every host and `classify_session` finds no crash. Unit: `host::tests`, `session_host::tests` (request line round trip, args, key lines), `session_hosts::tests` (session-event sniffing across reads, completeness, placeholder ids), `lanes::tests::a_remote_lanes_label_cancel_and_notes_come_from_its_host`, `routes::tests` (which routes run in a host). Checks: `cargo clippy -p desktop-shell -p desktop-app -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --locked` 371 passed; `--test session_hosts --run-ignored only` 3 passed; workspace-engine recovery/registry subset 30 passed; deterministic eval tier 65/65 pass; `scripts/quality-gate.sh fmt node lint typos deny` ok. The desktop app itself was not run. The full seven-check gate passed in CI on 2026-11-25 (run 45587682363, released as v0.82.0); `tests/session_hosts.rs` is `#[ignore]`, so CI does not exercise the real host processes, and the app is still to be tried by hand. **Fix 2026-11-25 (owner report, v0.82.0):** two sessions of one project paused on command approvals; after one was approved, the other, reloaded, had no card, and neither had the approved session's next pause (a plan review raised by its resumed turn) — `read_pending_approval` had been widened to `has_live_host`, and hosts live until archive/quit, so `/api/pending-card` returned `null` for every session with a host. The reload read is back to `has_live_worker` (a session host owns no approvals; its card's answer is forwarded to it); `classify_session` and the launch sweep keep `has_live_host`. Tests: `recovery::tests::a_session_with_a_live_session_host_keeps_its_pending_card`, and `tests/session_hosts.rs` `two_sessions_paused_on_approvals_each_keep_their_card` (real hosts: both cards read; approving one resumes its turn into a second pause whose card is read; the other's card is still read and its rejection resumes through its host); the unit test also covers a plan review raised in the approved session; both failed before the fix. The fix passed the full seven-check gate in CI on 2026-11-25 (run 45651712001, released as v0.83.0). |
| 1c · Messages to a working session reach the running turn (D7) | Done 2026-11-12 | `lanes.rs`: a `NoteQueue` (the engine's `TurnSteering` plus a closed flag and the queued texts) sits in the lane beside the cancel token; `queue` and `close` share one lock, so a note is either queued before the turn ended or refused, never accepted and dropped. `stream_turn` makes one per turn, sets it on the lane (and on adoption for a new session), and runs the turn with it in a turn-thread thread-local (`with_turn_notes` / `turn_notes`), so all 14 `TurnSink` sites in `lib.rs` pass it as `steering` without a signature change; a turn run any other way (tests, non-streaming fallbacks) still passes `None`. `POST /api/session-note {session_id, text}` is a UI route (`Route::SessionNote`), 200 `queued`, 409 `not_working` (idle, non-turn work, unknown session, or a paused turn — so a note can never answer or approve), 400 empty or over `worker::MAX_NOTE_BYTES` (16 KiB, as the worker socket). The relay sends `note {messageId, text}` on `NoteAdded` and `notes_returned {notes}` just before `done`/`error`. Turn end (`settle_left_notes`): a turn paused on a command, plan, question, fetch, fallback, round budget, check card or context review carries its notes; completed (a patch proposal included), failed or stopped returns them; if carrying fails they are returned. **Deviation:** carried notes are stored per session in `<data_dir>/chat/pending-notes/<session_id>.json`, not inside the paused-turn snapshot, and the session's next turn takes them — the resume, or a new message if the user does not resume; no engine change (context.md D7 "As built"). `app.js`: `streamRequest` tracks every turn stream (`runningTurnStreams`), so a note works during any turn kind, resumes included; Send/Enter while the current session has a running stream posts a note (`sendSteeringNote`), shown dashed with a waiting label until its `note` event; at stream end `settleTurnNotes` sends returned notes (oldest first, joined) as the next turn after completion or failure, refills the composer after a stop or when the stream ended without `done`, and leaves carried ones waiting; a refused stream request now shows the JSON's `error`, not the body. Known gap: Stop in the window aborts the stream, so the window uses its own copy of the waiting notes; a note carried over a pause from before a reload and then stopped before the resumed turn's first boundary is returned to a closed stream and lost. Tests (`responsive_tests.rs`, a scripted gated model): `a_note_during_a_turn_reaches_the_next_request_and_the_log`, `a_note_in_the_final_round_is_returned_for_the_next_turn`, `a_note_before_a_stop_is_returned`, `a_note_before_an_approval_reaches_the_resumed_turn` (also: 409 while paused, approval still pending), `a_note_never_answers_a_pending_question`, `a_note_to_an_idle_session_is_not_working`; `lanes::tests` for the queue and carried notes; `pause_progress_streams_nothing_and_a_note_streams` replaces `steering_progress_streams_nothing`. Checks: `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --locked` 324 passed; `scripts/quality-gate.sh fmt node lint typos deny` ok. workspace-engine untouched. The UI was not exercised in the running app (Task 3's manual check). Workspace clippy and tests left to CI. |
| 2 · Shared state across processes (D4) | Not started | — |
| 3 · The UI with several working sessions | Done 2026-11-20 | `app.js` only (no `style.css` change). **409s:** one `parseApiPayload` / `refusalError` reads every refused body — `api`, `streamRequestError` and `exportSessionDownload` (which threw the raw body) all throw the route's `error` (else `message`, else the status text), keeping `payload`, `status` and `code`; a JSON body that is not an object no longer throws a `TypeError` in `api`. **This window's turns per session:** the global `currentTurn` and `chatSubmitting` are gone. `beginWindowTurn()` records each turn the composer sends or a card resumes (`windowTurns`, its session from its running action, adopted from the `session` event); `sessionSubmitting()` is "this window sends a turn for the open session", so another session's composer stays free and `composerStopTarget` stops only `windowTurnFor(currentSessionId)` (`stopWindowTurn(turn)` replaces `stopCurrentTurn`). `followTurnSession(owner, id)` replaces every `currentSessionId = payload.sessionId` (the composer's `session`/`done`, askAgain, and all seven resume flows): the window follows a turn's session only while it still shows it; a running action keeps an adopted session id (`adoptSession`) instead of following the window. **Leaving a session mid-turn:** the turn streams on into its detached bubble; `liveTurnShown(bubble)` makes `appendProposals` (cards, `setPendingDecision`, the context manifest), the post-turn reloads, toasts and `restorePrompt` skip a left turn, and its project's sidebar is refreshed (`refreshSessionStates([turnRepo])`); a `context` event of another session no longer repoints the open context panel; notes a left turn returns are held per session (`heldComposerText`) and refill that session's composer on return, and `whenTurnsIdle(sessionId, …)` waits only for that session. **Returning:** `startTurnIndicator` records the bubble (`action.liveMessage`); `loadSession` ends with `reattachLiveTurns(currentSessionId)`, which appends the live bubble (streamed text plus the running row) below the reloaded log, scrolls it into view, re-observes its homes and re-appends undelivered waiting notes, so the row, the slot and the one Stop show the work again. **Sidebar:** `fetchProjectSessions` numbers every `/api/sessions` fetch and drops an answer older than one already applied, so the working-set refresh or a turn's end can no longer redraw an archived or renamed session from a list read before the action; `loadSessions`, `loadProjectSessions` and `refreshSessionStates` write the list only through it, and archive/rename/delete of another project fetch directly (not `loadProjectSessions`, which skipped while a load was in flight). Rename no longer selects the renamed session (it passed `payload.session.id` as the preferred session, so renaming another session switched `currentSessionId` without loading it). **Deviations:** none from the task; the sidebar marking stayed spec 112's. A turn stopped from the composer still leaves the server's lane busy until its next keepalive write, so the slot shows Busy elsewhere for a moment (spec 112 behaviour, unchanged). Tests (`tests.rs`, source shapes): new `app_js_shows_a_refusals_message_never_its_json`, `app_js_sidebar_actions_during_a_turn_update_the_sidebar_at_once`, `app_js_a_session_left_mid_turn_keeps_working_and_shows_it_on_return`; `app_js_has_one_stop_in_the_composer`, `app_js_composer_names_the_pending_decision_and_yields_to_a_question`, `app_js_draws_session_state_dots_in_the_sidebar` and the `sendChatPrompt` shape test updated for the renames. **Manual check** (2026-11-20, `serves_the_ui_for_manual_inspection` on port 4898 with its own data dir, a local slow OpenAI-compatible endpoint streaming one word every 1.5–4 s through a `model_provider.<id>` entry — `DAMAIAN_MOCK_MODEL_RESPONSE` answers too fast): during a turn, rename (13 ms) and archive (16 ms) of other sessions answered and redrew the sidebar; archive of the working session showed "This session is working (Answering a message). Stop it first, then archive it."; on another session the composer was Send, with no Stop, slot or bubble from the first; a turn that finished while away left the window where it was and showed its answer on return; returning mid-turn showed the streamed text, the running row in view, the Working dot and the Stop, and that Stop stopped it and restored the prompt; two sessions in one project streamed at once and both logs ended with their own prompt and answer. **Finding for Task 2:** none seen — the two concurrent turns in one project wrote correct, separate logs; this exercised one shell process only, so it says nothing about two host processes. Checks: `scripts/quality-gate.sh fmt node lint typos deny` ok; `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --locked` 347 passed. Workspace clippy and tests left to CI. The full seven-check gate passed in CI on 2026-11-20 (Quality run 44883576383, the `main` push that also carries the reload-card fix). |
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

- [x] Factor the host plumbing out of `workers.rs` / `worker.rs` (D1a):
      re-entry with a mode flag, registry entry, stdin key handover and
      lifeline, control socket, shutdown and clean quit for every host.
- [x] A session host per session, started on its first work request in the
      session's own checkout; the shell forwards each session-work request
      whole and the host runs the same `handle_*` function, writing the
      response back for the shell to copy to the window. Route handlers write
      to a stream type both sides can give them, not only `TcpStream`.
- [x] Resolve every session-work request to its session before forwarding:
      today a request naming only a patch, baseline batch, checkpoint or
      command proposal runs unlaned (`RouteClass::SessionWork(None)`); either
      the UI sends `session_id` on those routes or the shell resolves the id.
- [x] Each key the shell resolves goes to the host on stdin, never on the
      socket; a provider fallback works.
- [x] The lane's label, cancel and steering notes (Task 1c) come from and go
      to the host (D3, D7); the recovery sweep
      and `classify_session` skip a session whose host is alive (widen spec
      57 C12's live-worker check to session hosts).
- [x] Tests: two sessions in one project work at the same time in two
      processes; killing one host leaves the other and the shell running, and
      the killed session recovers as a crash (spec 17); quitting ends every
      host and is not a crash.

Files: `crates/desktop-shell/src/lib.rs`, `workers.rs`, `worker.rs`,
`lanes.rs`, `desktop-app/src/main.rs` (exit hook), `workspace-engine`
process registry and recovery, tests. Checks: scoped clippy for each crate
touched; `cargo nextest run -p desktop-shell`; the deterministic eval tier.

## Task 1c · Messages to a working session reach the running turn (D7)

Depends on Task 1 (lanes) only; independent of Task 1b, which must carry it.

- [x] The lane holds the running turn's `TurnSteering`; every chat turn the
      shell starts or resumes passes it as `TurnSink::steering`.
- [x] `POST /api/session-note` (UI class in `routes.rs`): queue a note, 409
      `not_working` when the session has no running turn; the relay forwards
      `NoteAdded` as an SSE `note` event.
- [x] Notes left at turn end: stored with a paused turn and seeded into the
      resumed turn; returned on completion, failure or stop so the UI can send
      or restore them.
- [x] `app.js`: while the current session works, Send posts a note; the
      message shows as waiting and becomes an ordinary user message on
      `note`; returned notes start the next turn (completed, failed) or refill
      the composer (stopped); a 409 shows its message, not JSON.
- [x] Tests: a note during a turn reaches the next request and the log; a note
      in the final round starts the next turn; a note before an approval
      reaches the resumed turn; a note before a Stop is returned; a note never
      resolves an approval or question; `not_working` when idle.

Files: `crates/desktop-shell/src/lib.rs`, `lanes.rs`, `routes.rs`, the paused
turn store, `static/app.js`, tests. Checks: scoped clippy (desktop-shell,
workspace-engine if touched); `cargo nextest run -p desktop-shell`; web checks.

## Task 2 · Shared state across processes (D4)

- [ ] Audit and fix each item of D4; one concurrency test per item (two
      sessions in two projects; two sessions in one project).
- [ ] Record what was already safe and why in this row.

Files: `crates/workspace-engine/src/session.rs`, `audit.rs`, `process_registry`,
the index/map caches, tests. Checks: scoped clippy; scoped nextest; the
deterministic eval tier.

## Task 3 · The UI with several working sessions

- [x] `app.js`: a 409 shows its message; sidebar actions during a turn update
      the sidebar at once; switching to another session while one works leaves
      the first running and shows its progress again on return.
      *(2026-11-19: "working sessions are marked in the sidebar" moved to
      [spec 112](../112_status_in_one_place/proposal.md) requirement 7, which
      draws Working, Needs you and Failed from this spec's D3 routes. Build
      spec 112 before this task: both edit `app.js` and `style.css`. Spec 112
      is Done 2026-11-19, so this task no longer waits on it.)*
- [x] Manual check in the running app (`serves_the_ui_for_manual_inspection`,
      port 4899, its own data dir): archive during a turn; two sessions working.

Files: `app.js`, `style.css`, desktop-shell tests. Checks: web checks;
`cargo nextest run -p desktop-shell`.

## Task 4 · Documentation and close-out

- [ ] `USER_GUIDE.md` / `TROUBLESHOOTING.md`: several sessions at once; the
      archive/delete exception; messages sent while Damaian works.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.
