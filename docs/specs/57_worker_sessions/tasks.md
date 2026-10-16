# Worker Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Started:** 2026-10-07 (planned 2026-10-04; references re-checked against the tree on 2026-10-07, after #24 Tasks 1–5 landed) — **Done:** 2026-10-16

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Setting, worker record, handoff instruction | Done 2026-10-07 | `enable_worker_sessions` (default off) is Forbidden at repository scope and a `Capability` key in #31's partition. Beyond the planned touchpoints it needed `profile.rs::split_profile_keys`, a `weakening_cases` row, and a human name in `effective_policy.rs` (`every_rule_has_a_human_name` caught that one). `WorkerRecord` plus `record_worker_spawned`, `record_worker_child`, `worker_record`, `worker_children` and `worker_sessions_for_source` on `SessionStore`. Both `worker_spawned` events share one type; the parent's carries `workerSessionId`, which is how `worker_record` tells them apart. `worker_record` reads with `parsed_events`, so a rewind cannot turn a worker back into a session that is offered handoffs. `record_steering_note` is deferred to Task 2, which writes and tests the event. `system_prompt_for_session` and `WORKER_HANDOFF_PARAGRAPH` are in `chat.rs`, and `ask_with_session` calls it. Tests: 15 filtered tests, `repository_config_trust` (48) and `permission_profiles` all pass; fmt, clippy (`workspace-engine`) and typos are clean. **Order deviation:** a second agent started this task in the same worktree and was stopped. It had already written the config implementation, so Step 2 (fail first) was not observed. Step 6 stood in for it. Each change below was made separately and reverted, and each test it names failed: paragraph in `system_prompt` → the byte-identical test; dropping `is_none()` → `a_worker_session_never_gets_the_paragraph`; repository value applied → `repository_config_cannot_enable_worker_sessions`; by-source lookup ignoring the source → its test; default `true` → `worker_sessions_are_off_by_default`. The first breakage showed that the worker test compared only against `system_prompt(Code)`. It now also asserts that the prompt has no `worker-prompt`. |
| 2 · `TurnSteering` and `TaskStatus::Paused` | Done 2026-10-09 | `steering.rs` (`TurnSteering`, 100 ms re-check), `TaskStatus::Paused` (not terminal, no side effect in flight; joins the resumable row of the crash-recovery matrix, now 45 cells), `SessionStore::record_steering_note` (deferred here from Task 1), `TurnSink.steering`, and `TurnProgress::{Paused, Resumed, NoteAdded}`. The boundary step sits at the top of the round `loop`. The pause hold runs **before** the cancel check, so a stop while paused takes the existing cancel path; the notes are drained after it. Resume restores the status the turn paused from. C5: `classify_session` withholds auto-resume for any session with a worker record. C4: 37 literals got `steering: None` (36 plus Task 1's handoff test). `turn_progress_event` now returns `Option` and drops the three new variants. **Discoveries:** (1) User prompts reach the model unredacted, which C3 did not expect. Notes are redacted anyway (stricter), and the log keeps them as written. (2) There is no composer length limit; Task 5's socket must bound note length. (3) `app.js` labels no in-flight status, so `paused` had nothing to join. (4) The `session_rewind.rs` status lists filter by property and needed no edit. **Scope addition, agreed 2026-10-08:** 7 `desktop-shell` route tests overflowed the 2 MiB stack of a spawned thread. `render_markdown_endpoint…` already overflowed before Task 1; Task 1's extra `Config` field pushed six findings-route tests over. Production had the same exposure: every turn ran on a default `thread::spawn`, and so did the desktop app's server thread. Added `desktop_shell::spawn_with_shell_stack` (8 MiB, matching the main thread) and used it for the turn worker in `stream_turn`, the `desktop-app` server thread, and the five test servers, so the tests run with the production stack. Added the test `the_shell_stack_holds_what_a_default_thread_cannot`. Merged with main on 2026-10-09, where a separate fix moved route bodies into `handle_*` functions and brought the router frame back under 2 MiB. The two are complementary. That fix covers the router frame; this one also covers the turn thread, which that fix leaves on 2 MiB. Both are kept. **Falsified:** pause moved after the model call → `pause_takes_effect_only_at_a_round_boundary` fails. That test first passed against this break; it now also asserts that round 1's tool phase precedes `Paused`. Drain dropped → both note tests fail. Stack set to 2 MiB → all 8 stack-dependent tests abort. **Checks:** `workspace-engine` 961/961; `desktop-shell` + `eval-harness` 158/158 on three consecutive runs. `a_deterministic_run_reports_the_token_figures_the_session_recorded` failed once, then passed four times: a flake, not investigated. fmt, clippy (`workspace-engine`, `desktop-shell`, `eval-harness`, `desktop-app`) and typos are clean. `app.js` is unchanged. |
| 3 · The progress file | Done 2026-10-09 | `worker_progress.rs`: `ProgressKind` (with `as_str`), `ProgressWriter::{create, append}`, `read_progress_after`, `progress_path`. `SessionStore::delete_session` also removes `<id>.progress.jsonl`. Each line is `{"seq","timestampMs","kind","payload"}`. The payload is nested, not flattened, so a caller's field cannot shadow `seq` or `kind`. Task 9 reads `event.payload.*`. `append` redacts every string in the payload at any depth (arrays and objects included), as C3 requires, and so does not depend on what the caller already redacted. `timestampMs` is `hash::now_millis()`, the session log's clock, written as a `u64`. `seq` is held in memory because the worker is the file's only writer. A missing file reads as empty. **Tests:** the six planned tests plus `a_session_without_a_progress_file_reads_as_empty` and `the_progress_file_sits_beside_the_session_log`. All 8 failed on `todo!()` stubs before the implementation. Now `worker_progress::` and `session::` 35/35, `tests/foundation.rs` 158/158 (it calls `delete_session`), fmt, clippy (`workspace-engine`) and typos are clean. **Falsified:** redaction skipped for `Tool` → the secret test fails. No recursion into arrays → it fails on the approval payload. The writer opened in append mode instead of truncating → the truncate test fails. A parse failure made fatal → the torn-line test fails. `delete_session` leaving the file → the delete test fails. All reverted. **Discovery and deviation:** a first version also cut the read at the last newline. Falsifying showed the cut did nothing. Every line ends in `}`, so a line missing only its newline is a complete object, and any shorter tear is invalid JSON that the parse-and-skip already drops. The cut was removed. A terminated line that does not parse is skipped too, because the file is a view and the audit log is the record. **For Task 6:** create the writer when the worker process starts, and pass it the session's `SecretScanner`. Text batching (about 250 ms) is the caller's job. `append` takes `&mut self`, so keep the writer on one thread or behind a `Mutex`. Two writers on one file would duplicate `seq`. |
| 4 · Worktree creation and removal | Done 2026-10-12 | `worktree.rs`: `worker_slug`, `source_changes`/`SourceChanges`, `create_worker_worktree`/`CreatedWorktree`, `remove_worker_worktree`/`RemoveOutcome`, as the Interfaces fix them, reached as `workspace_engine::worktree::*` (not re-exported, like `worker_progress`). `checkpoint::run_user_git` is `pub(crate)`, and its doc comment says it now also runs mutating commands. Beside it is `user_git_command_line`, which renders the same argv (`git --no-optional-locks -C <root> …`, quoting arguments that contain whitespace or quotes), so a recorded command cannot drift from the one run. Every git command, read-only ones included, goes through one recording wrapper into `git_commands`. Creation follows §5.1 in order. (1) `rev-parse --show-toplevel` must equal the canonical source. (2) Base: `stash create`, else `rev-parse --verify HEAD^{commit}`. Naming uses one `for-each-ref refs/heads/damaian/` plus a `symlink_metadata` probe per candidate. (3) `.damaian`, `worktrees` and `<slug>` are each checked with `symlink_metadata`: a symlink, a non-directory or an existing `<slug>` is refused. Then `ls-files -- .damaian/worktrees/<slug>`, then containment of the deepest existing ancestor. (4) The exclude file comes from `rev-parse --git-path info/exclude`, so it also works when the source is itself a linked worktree. (5) `worktree add`. (6) Untracked files come from `ls-files --others --exclude-standard -z`. Symlinks are recreated, not followed, and a symlinked directory on the way into the worktree is refused. Removal refuses any path that is not a registered worktree directly under the source's `.damaian/worktrees/` reached through real directories. It passes `--force` only when the worktree is dirty **and** confirmed. **Tests:** 17 in `tests/worktree.rs`, all of which failed on `todo!()` stubs first; now 17/17. `tests/checkpoints.rs` + `tests/checkpoint_wiring.rs` + `worktree` 60/60, `checkpoint::` lib tests 5/5. fmt, clippy (`workspace-engine`) and typos are clean. **Falsified** (each reverted): Step 4's `path.exists()` → `refuses_a_symlink_anywhere_in_the_path` fails. Its symlinks point *inside* the repository, so only the per-component check can catch them. A symlink allowed at `.damaian/worktrees` alone → the same test fails on its second case. The outside test survives either guard on its own, because the per-component check and containment back each other up; with both removed it fails. Other breaks, each failing the test that guards it: `ls-files` check removed → the tracked test; exclude always appended → the exclude test; branch or path collisions ignored → the collision test; no `stash create` → the include test; untracked not copied → both include tests; dirty removed unconfirmed → the dirty test; exclude written before the checks → three refusal tests (their nothing-changed snapshots); `worktree add` run unrecorded → the created test; mentions matched by plain substring → `source_changes` (`data.txt` ≠ `a.txt`); top-level check removed → `refuses_a_subdirectory_of_a_repository`; removal target unchecked → `removal_refuses_a_path_that_is_not_a_worker_worktree`. **Deviations**, written into proposal §5.1: the slug comes from the first *non-blank* line; non-ASCII characters are dropped (`Ändere` → `ndere`), not used as separators; step 1 also refuses a subdirectory; removal validates its target. Added tests beyond the plan: subdirectory refusal and removal-target refusal. `mentioned` is ordered by first mention in the prompt, and the rest keep git's order. **For Task 7:** (a) With *Include*, the branch starts at the `git stash create` commit, which is a two-parent `WIP on <branch>` commit (its second parent is the index commit). Merging the worker's branch therefore carries that commit into history. This follows §5.1 as written. If that is unwanted, `commit-tree <stash>^{tree} -p HEAD` would give a single-parent base, but it needs a spec decision first. (b) A failure after step 5 (an untracked copy) leaves the worktree and branch in place and returns the error. Task 7 should still record `worker_spawned`/audit with `git_commands`, or remove the worktree. Nothing reports the partial commands yet, because the error path drops them. (c) `git_commands` holds absolute paths, which show the user's home directory. That is fine for the local audit log, but decide before showing them anywhere else. |
| 5 · Worker process: handover, lifeline, control socket | Done 2026-10-12 | `desktop-shell/src/worker.rs` (new), exported as `desktop_shell::{run_worker_from_args, worker_socket_path, WORKER_FLAG}`. Both `main`s branch on `args[1] == "--worker"` first: `desktop-shell` before `install_shutdown_handler`, `desktop-app` before `tauri::Builder`. `ProcessKind::Worker` (`"worker"`) is added; nothing registers it yet (Task 7). **Step 1, measured:** the id-named path `<data_dir>/workers/<session-id>.sock` is **107 bytes** under the real `~/Library/Application Support/DamaianClient` (a real id, `session_18c1da785654f0c8_1712c_2`) and **132** under a `/var/folders/…/T/` test directory, against `sun_path`'s 104 (103 usable). Both exceed it, so the socket is `<data_dir>/workers/<first 16 hex of sha256(session id)>.sock`: 91 bytes under the real directory. `worker_socket_path` refuses, naming the length, when even that does not fit. Proposal §5.2 is updated. **What the host does:** parses `--worker --session <id> --data-dir <abs>` (the id must be `[A-Za-z0-9_-]`, at most 128 bytes; a relative data dir is refused); refuses a terminal stdin; reads one handover line (at most 16 KiB, must end in a newline, exactly `apiKey` and `token`, token 64 hex); loads config from `--data-dir` only (`load_scoped` over `<data_dir>/config/user.conf` and the admin path, then pins `data_dir` back to the argument); starts the lifeline thread; installs #46's handler; binds the socket; serves. On lifeline EOF or `shutdown`: state `shutting_down`, cancel, `sweep_own`, remove the socket, exit 0. Every refusal is a fixed string, and serde's messages are never passed on, because some quote their input. `Handover`'s `Debug` prints neither value. **Socket:** the `workers/` directory is created `0700` (a symlink or non-directory there is refused, an existing one is chmodded), and the socket is chmodded `0600` after `bind`, which is safe because the directory is already private. A leftover socket is replaced; one that still accepts a connection is refused ("a worker is already serving this session"). One connection at a time, 5 s idle timeout, a 256 KiB line limit and a 16 KiB note limit (Task 2's discovery). A malformed line, a missing or wrong token (compared in constant time) or an oversized line closes the connection unanswered. Arguments are checked against §5.2's table, and an argument it does not name is refused. **For Task 6:** `status` answers `{ok, sessionId, pid, state}`, with `state` `idle` or `shutting_down`; extend `WorkerState` for running, paused and waiting. `message` answers "this worker cannot start turns yet", and `pause`, `resume`, `note`, `approve`, `reject` and `stop` answer "no turn is running". `WorkerHost.dispatch` is where the turn driver plugs in, and `ControlCommand` already carries the validated arguments. `api_key` sits on `WorkerHost` behind `#[allow(dead_code)]` until Task 6 reads it. `WorkerHost.cancel` is a single token; make it per turn. The control thread already uses `spawn_with_shell_stack`. **For Task 7:** use `worker_socket_path` and keep test scratch names short: the hashed path is 93 and 98 bytes in this task's two test directories. Hold the child's stdin open for its lifetime, as `tests/worker_process.rs::start_worker` does. **Tests:** 15 unit tests in `worker.rs` (the five planned plus `worker_args_need_a_session_and_an_absolute_data_dir`, `the_socket_is_named_by_a_short_hash_and_must_fit_sun_path`, `the_worker_loads_config_from_its_data_dir`, `a_stale_socket_is_replaced_but_a_live_one_is_not_taken_over`, `a_symlinked_workers_directory_is_refused`, `requests_are_validated_against_the_command_table`, `turn_commands_are_refused_while_no_turn_is_hosted`, `shutdown_answers_then_signals_the_main_thread`, `an_oversized_request_closes_the_connection`) and `process_registry::tests::a_worker_is_recorded_as_worker`. Three `#[ignore]`d re-exec tests in `desktop-shell/tests/worker_process.rs` run the real `damaian-desktop-shell` binary (`CARGO_BIN_EXE`). All 15 unit tests and all 3 re-exec tests failed on `todo!()` stubs first. They clean up through a #46 registry of their own (`Harness`, which registers the worker as `ProcessKind::Worker` and sweeps on drop), so a failing run kills what it started by identity. No stray process was left after any run, the deliberately broken ones included. **Ignored tests, run by hand** (`cargo test -p desktop-shell --test worker_process -- --ignored`): `closing_stdin_exits_the_worker_and_kills_its_sleeping_child ... ok`, `the_worker_keeps_key_and_token_out_of_argv_and_output ... ok`, `a_worker_without_a_handover_exits_without_serving ... ok`, `test result: ok. 3 passed` (1.86 s). Under nextest the three also passed on four later runs (about 1.9 s each). **One unexplained failure:** the first nextest run, straight after a cold build of every `workspace-engine` test binary, failed all three on their timeouts (10 s to exit, 30 s to serve). It did not reproduce: not on the same command again, and not after forcing a relink and running at once, twice. Not investigated further. A slow first exec of a freshly linked binary is a guess, not a finding. **Falsified** (each reverted): lifeline thread removed (Step 7) → the stdin-close test fails; `sweep_own` skipped → the same test fails; token unchecked → the token test; socket chmodded `0666` → the mode test; extra handover fields allowed → the handover test; config loaded from the default user.conf → the config test; unknown commands accepted → the unknown-command test; note length unbounded → the validation test; the live-socket check removed → the stale/live test; `status` reporting a fixed state → the snapshot test; a symlinked `workers/` followed → the symlink test. **Deviations:** (1) `a_command_the_worker_runs_sees_neither_key_nor_token` moved to Task 6 (its step 2 now lists it). The worker runs no command before Task 6, and macOS shows no other process's environment (`ps -E` and `ps eww` print none, checked here), so there was no child to read. In its place, `the_worker_keeps_key_and_token_out_of_argv_and_output` reads argv with `ps -ww -o args=`, checks stdout and stderr, and greps the data dir for the key. (2) §5.2 promised a `worker_started` event that no task writes. With C6 the shell registers the worker and records `worker_spawned`, so §5.2 now says there is no separate event. (3) C11's line references had drifted (`.run` 122→129, shell thread 84→86, `main.rs:4` correct) and are now re-taken after this task's edits: 135, 92 and `main.rs:10`. **Checks:** nextest `worker::`, `process_registry` (lib and `tests/process_registry.rs`, ignored included) and `worker_process` (ignored included): 39/39. fmt, clippy `-D warnings` (`desktop-shell`, `desktop-app`, `workspace-engine`) and typos are clean. |
| 6 · Worker turns: steering, approvals, same-turn guarantee | Done 2026-10-15 | **Step 1:** `lib.rs` gains `chat_engine(config)` (`WorkspaceEngine::new` + `configure_chat_integrations`) and `chat_model_adapter(config, api_key)` (`CurlModelTransport` over `ProcessRegistry::open(data_dir)` + `OpenAICompatibleAdapter`). The key closure is called only when a real adapter is built. `run_chat_request` (now a wrapper over `run_chat_request_with`, which takes the adapter builder so a test can script the model) and the command and plan resume paths use both; so does the worker. `engine_for_repo_with_model_options` became `config_for_repo_with_model_options`. **Deviation, accepted 2026-10-15 (to revisit later):** `chat_model_adapter` honours `DAMAIAN_MOCK_MODEL_RESPONSE` as the CLI does, so the **shell** now does too, not only the worker. It is there so both hosts build turns one way and so a real worker process can be scripted. The alternative, kept open, is to move the check into the worker's adapter builder only. **Worker (`worker.rs`):** one turn thread (`spawn_with_shell_stack`) owns the `ProgressWriter` and runs jobs from an mpsc channel: `message` → `ask_with_session_with_options`, `approve`/`reject` → `resume_after_command_decision` or `resume_after_plan_decision(.., None, ..)`, all with `approved_by = "desktop_user"` as in the shell. Each job gets a fresh `CancelToken` and `TurnSteering`, replacing Task 5's single token. The repository is the worker record's `worktree_path`, read on the first job; a session without a record refuses turns, so Task 5's socket tests still start without one. Config is `load_worker_config(data_dir, Some(worktree))`, which now applies the worktree's `.damaian/config.conf` at repository scope as `Config::load_for_repository` does for the shell. The key lives only inside the adapter builder closure; `WorkerHost.api_key` and both `#[allow(dead_code)]`s are gone. **States:** `idle`, `running`, `paused` (set when the turn reports `TurnProgress::Paused`, not when `pause` is asked), `waiting_for_approval` (the snapshot adds `proposalId`), `shutting_down`. `pause`, `resume`, `note` and `stop` act only on a running or paused turn; while waiting for approval they are refused ("approve or reject it"), so `stop` does not end a waiting turn; `message` is refused unless idle. **Progress:** `status` lines are `{state, taskStatus?, error?}` (the worker's state plus `TaskStatus::as_str` when known); `phase` is the shell's `phase_json`; a `phase` of kind `tool` is followed by `tool {label}`; `plan` is `plan_json`; `note {messageId}`; text is batched at 250 ms and flushed before every other line and at the end of a turn; `approval {kind, proposal}` carries the shell's own `command`/`plan`/`patch`/`fallback` proposal JSON. A patch or a provider-fallback offer leaves the worker idle: both are answered through the shell's existing routes with the worktree as `repo` (the worker holds one provider's key, so it cannot take a fallback itself). **Tests** (all failed against a stub that refused every job first): in `lib.rs` `tests::worker_turns`, the eight planned in-process tests are `a_turn_through_the_worker_host_matches_the_shell_host`, `progress_lines_mirror_the_turn`, `a_note_while_waiting_for_approval_is_refused`, `message_is_refused_while_a_turn_runs`, `a_command_approval_continues_in_process`, `a_patch_proposal_leaves_the_worker_idle_and_alive`, plus `pause_holds_the_turn_and_a_note_reaches_it` and `stop_ends_the_running_turn_cancelled`. The ninth, `a_worker_turn_gets_the_shells_chat_integrations`, was added while falsifying. The same-turn test compares 15 session-log and 7 audit event types (`checkpoint_created`, `file_read`, `model_request_prepared`, `command_proposed`, `command_executed`, …), filtered to each session and its tasks because the audit log is shared. In `tests/worker_process.rs`, two new `#[ignore]`d re-exec tests: `a_command_the_worker_runs_sees_neither_key_nor_token` and `killing_a_worker_waiting_for_approval_leaves_a_snapshot_recovery_finds`. The kill is `Child::kill` on the test's own unreaped child after a `ProcessIdentity` check; cleanup stays with `Harness`. `reattach_pending_approvals` returns the `Command` approval. **Falsified** (each reverted): `configure_chat_integrations` bypassed → the same-turn test **still passes**, because with no MCP or browser server configured the call changes nothing a turn records; the new integrations test (a browser MCP server in a private data dir's `user.conf`, native tools on, then `inspect_web_page` must be offered) fails. Worker engine with audit off → same-turn test fails; worker turn in a fresh session → it fails. Approval always resumed as rejected → approval test; batching interval 0 → progress test; `steering: None` → pause test (the stop test still passes, correctly); notes accepted while waiting → note test; `message` accepted while running → busy test; one cancel token for every turn → stop test (its second turn ends cancelled); `tool` line dropped → progress test; key put into the worker's environment → env test; pending snapshots deleted once waiting → kill test. **Discovery:** an `sk-…` value is **redacted** on its way into the data directory, so searching the data dir for a leaked key of that shape proves nothing. The env test therefore hands over a key the scanner leaves alone, and checks that key- and token-shaped probe values survive before searching. The same caveat applies to Task 5's `the_worker_keeps_key_and_token_out_of_argv_and_output` data-dir check (its doc comment now says so); its argv and output checks are unaffected. **Ignored tests, run by hand** (`cargo test -p desktop-shell --test worker_process -- --ignored`): `a_worker_without_a_handover_exits_without_serving ... ok`, `the_worker_keeps_key_and_token_out_of_argv_and_output ... ok`, `closing_stdin_exits_the_worker_and_kills_its_sleeping_child ... ok`, `killing_a_worker_waiting_for_approval_leaves_a_snapshot_recovery_finds ... ok`, `a_command_the_worker_runs_sees_neither_key_nor_token ... ok`, `test result: ok. 5 passed` (1.19 s). No timeout this time. **Checks** (after updating to `main` on 2026-10-15): nextest `desktop-shell` 135/135 (9 skipped); deterministic eval tier 16/16 scenarios pass; fmt, clippy `-D warnings` (`desktop-shell`, `desktop-app`, `workspace-engine`), and `scripts/quality-gate.sh fmt node lint typos deny` are clean. Workspace clippy and nextest are left to CI. The Bash sandbox blocks binding even a local test port and makes `~/.cargo/advisory-dbs` read-only, so the crate tests, the re-exec tests and `cargo deny` were run outside it. **For Task 7:** (a) the worker takes the configured provider and model; the shell's `model_provider`/`model` form options are not passed to it, so a worker cannot yet use the model chosen in the composer. (b) Fixtures built through `Config::load_for_repository` run with the index watcher on, because `enable_index_watcher` is not a config-file key. (c) `status` is the place to read `proposalId`; the board's approval card also has the `approval` progress line. (d) `stop` on a waiting worker is refused: the UI's stop escalation should send `reject` first, or go straight to `shutdown`. |
| 7 · Shell supervisor: spawn, lifetime, routes | Done 2026-10-15 | **References re-checked first:** C11's `desktop-app/src/main.rs` lines (36, 92, 135) and `desktop-shell/src/main.rs:10` were right; `parse_form` had drifted from 2894 to 3274 (3318 after this task); `/api/session-delete`, `resolve_model_api_key` and its in-memory cache were as described. **What landed:** `desktop-shell/src/workers.rs` (new, `pub mod workers`): `Supervisor` (`spawn`, `restart`, `control`, `stop`, `shutdown`, `shutdown_all`, `is_alive`, `pid`), `WorkerTimeouts`, the process-wide `global()` over `current_exe()`, and `desktop_shell::shutdown_all_workers` for the app. `spawn` follows §5.1: `create_worker_worktree`, a Code-mode session keyed by the worktree's repository id and titled by the prompt's first non-blank line, `record_worker_spawned`/`record_worker_child`, the `worker_spawned` audit event (`gitCommands` as a JSON array), then the process: `current_exe() --worker --session <id> --data-dir <absolute>`, `process_group(0)`, piped stdin, stdout null, stderr inherited, registered as `ProcessKind::Worker` (owner = shell), one handover line with the key and a token from `/dev/urandom`, and stdin held beside the registration handle until the shell sees the worker exit. The prompt goes as the first `message` from a thread of its own once the socket answers, so the route never waits on a worker's start. A parent that is itself a worker is refused. `route()` serves the eight Interfaces routes; `lib.rs` adds the match arms and `handle_worker_route`, which passes `resolve_model_api_key`. `handle_session_delete` first calls `shutdown` on `workers_to_stop_for_deleted_session`: the session's `worker_children`, and the session itself if it is a worker (an addition: otherwise deleting a worker's own session would leave its process running until quit). `shutdown` sends `shutdown`, drops the lifeline, and a watchdog `SIGTERM`s after 5 s. `stop` asks `status` first: waiting for approval goes straight to `shutdown` (Task 6 (d)), idle answers with the worker's own refusal and stops there, a running or paused turn gets `stop` and the escalation (10 s, `shutdown`, 5 s, `SIGTERM`) runs on its own thread. `begin_stop`/`finish_stop` take a `StopTarget`, so the timeouts and the worker are injected in tests. Every signal goes through the new `ProcessIdentity::send_sigterm` (`process_registry.rs`): `SIGTERM` to the pid only while it still has the registered start time, audited as `worker_terminated`. `desktop-app` now ends in `.build(ctx)…run(..)` and calls `shutdown_all_workers` on `RunEvent::Exit` (C11). Restart and worktree removal are refused (409) while a live worker holds the worktree, by this shell's table or by an answering socket; a removal is audited as `worker_worktree_removed`; a failed creation as `worker_spawn_failed`. **Tests** (all 18 in `lib.rs` `tests::worker_supervisor` and the registry test failed on `todo!()` stubs first): ten of the eleven planned (the eleventh, the real stop escalation, is an ignored test below) plus `the_shell_serves_the_worker_routes_and_they_refuse_while_off` (through `handle_connection`), `shutdown_all_ends_every_worker_and_waits_for_it`, and six escalation cases (`the_stop_escalation_sends_shutdown_then_sigterm_after_its_timeouts`, `a_stop_that_ends_the_turn_goes_no_further`, `a_worker_that_exits_on_shutdown_is_not_signalled`, `a_stop_while_waiting_for_approval_goes_straight_to_shutdown`, `an_idle_worker_is_not_escalated`, `a_silent_worker_is_shut_down_then_signalled`), and `process_registry::tests::sigterm_is_refused_for_a_pid_that_no_longer_names_the_process`. The stub is a shell script that records argv, `env -0`, and the handover line's length and shape with the key masked (so the key never reaches disk), then `exec cat`s stdin; it ends by its lifeline, so no non-ignored test signals anything. Where a test needs the protocol, it binds the worker's socket itself and records requests. The key is a plain value the scanner leaves alone (checked in the fixture), per Task 6's discovery. Four `#[ignore]`d tests in `desktop-shell/tests/workers.rs`: `a_spawned_worker_runs_its_prompt_and_ends_with_shutdown_all` (the real binary, `DAMAIAN_MOCK_MODEL_RESPONSE`), `stop_escalates_to_shutdown_then_sigterm_by_registry_pid` (a stub that answers nothing, short injected timeouts, `worker_terminated` carries the registry's pid and start time), and `the_shells_signal_handler_reaches_workers_through_sweep_own` with its helper `signal_helper_spawns_a_worker_and_waits`. **`sweep_own` confirmed, not assumed:** the helper installs #46's handler exactly as `desktop-shell/src/main.rs` does, spawns a worker and is `SIGTERM`ed; the audit log then has `orphan_process_killed` with kind `worker`, the worker's pid and scope `OwnProcess`. The lifeline would end the worker too, so that record, which only `sweep_own` writes, is the evidence. The helper repeats `main`'s three lines because the binary's API token cannot be set from outside, so a test cannot drive the real server. **Ignored tests, run by hand** (`cargo test -p desktop-shell --test workers --test worker_process -- --ignored --test-threads=1`): `a_spawned_worker_runs_its_prompt_and_ends_with_shutdown_all ... ok`, `signal_helper_spawns_a_worker_and_waits ... ok`, `stop_escalates_to_shutdown_then_sigterm_by_registry_pid ... ok`, `the_shells_signal_handler_reaches_workers_through_sweep_own ... ok`, `test result: ok. 4 passed` (1.28 s); Task 5/6's five in `worker_process` also `ok. 5 passed` (1.35 s). No stub or worker process was left after any run (`ps`, by path), the deliberately broken runs included. **Falsified** (25 breaks, each run against its guarding test and reverted by a script; checksums confirmed the files restored): audit `gitCommands` emptied → the spawn-records test; key added to the worker's env → the stdin-only test (`added to the environment`); token added to argv → same test; an empty key handed over → same test (length mismatch); registered as `Command` → the registration test; no children stopped on delete → the delete test; `shutdown` replaced by `status` → the delete test; restart ignoring a live worker (both checks) → the restart test; dirty removal always confirmed → the removal test; removal while live → same; `proposalId` sent under the form name → the C7 test; no first message → the C7 test; board sorted by time before group → the list test; counts never computed → same; `after` ignored → the progress test; session id unchecked → same (`../../etc/x` answered 200); setting not enforced → the off test; `/api/worker-control` arm removed → the HTTP test (404); `shutdown_all` keeping lifelines → its test (needed `SIGTERM`); escalation skipping `shutdown` → the escalation test; escalation never signalling → same; `stop` sent while waiting → the waiting test; `send_sigterm` comparing the pid only → the registry test (the test process received the `SIGTERM`); worker not registered → the ignored `sweep_own` test (no `orphan_process_killed`); the real target's `terminate` a no-op → the ignored stop test (timed out). **Discoveries:** (1) macOS SIP strips `DYLD_*` when it runs the protected `/bin/sh`, and `sh` rewrites `PWD`, `OLDPWD`, `SHLVL` and `_`, so the env comparison exempts exactly those for the stub; the real worker is not protected. (2) `u32::MAX - 1` as a pid becomes `-2` as a `pid_t`, which `kill` reads as process group 2. The registry test uses 9 999 999 instead, so a broken identity check cannot signal a group. (3) Task 4 (b) stays open: a failure after `worktree add` returns no `git_commands`, so `worker_spawn_failed` records the error only, and the worktree and branch stay; reporting the partial commands needs `create_worker_worktree` to return them on error. (4) The board reads each worker's whole progress file on every poll; fine at today's sizes, worth a tail read if files grow. **Deviations:** `GET /api/workers-enabled` also answers while the setting is off, because it is how the UI reads the setting; `shutdown_all` takes no `data_dir` (it works on the supervisor's own table, which holds every worker this process started); `GET /api/workers` takes `repo` (a path), not §5.7's `repository=<id>`; proposal updated. **For Task 8:** spawn with `POST /api/workers` (`repo`, `prompt`, `include_uncommitted=true` for *Include*, `parent_session_id`); it answers once the worker is registered, before its first turn. `GET /api/worker-source-status` takes `repo` and `prompt`. Read the setting from `GET /api/workers-enabled`. **For Tasks 9–11:** rows group as `needs_you`/`running`/`finished`, newest first, and finished rows carry `filesChanged`/`commitsAhead`. `/api/worker-control` with `command=stop` returns at once, and the escalation continues in the background. With the setting off, `worker-control` refuses too, so a worker started before the user turned it off cannot be stopped from the UI until it is on again or the app quits: Task 9/10 should decide whether `stop`/`shutdown` stay allowed while off. *Restart worker* starts an idle process: a pending approval from the dead process is not re-attached to the new one. **Checks:** nextest `desktop-shell` 153/153 (13 skipped, the ignored ones), `workspace-engine` `process_registry` 16/16, both ignored files above; `cargo fmt --all`; clippy `-D warnings` for `desktop-shell`, `desktop-app` and `workspace-engine`; `scripts/quality-gate.sh fmt node lint typos deny` all ok. The full seven-check gate passed in CI on 2026-10-15, on `main` after the merge with #25's Task 6. The route tests, the re-exec tests and `cargo deny` ran outside the Bash sandbox, which blocks binding a local port and makes `~/.cargo/advisory-dbs` read-only. |
| 8 · UI: setting, *Start worker*, New worker dialog | Done 2026-10-16 | **References re-checked first:** `finalizeChatMessage` had drifted from `app.js:3513` to 4203, and `upsertMcpConfig`/`saveMcpServer` from 2923/2951 to 3616/3644. They are now 4214 and 3621/3649 after this task's edits, and C8/C9 are corrected. `render.rs:376` (`<pre class="code-block"><code class="language-{lang.trim()}">`) was right. `updateChatMessage`, `renderMarkdown`, `upsertConfigValue`, `saveConfigFile`, `showAppDialog`, `showRewindDialog` and the General page were as described. Task 9's `renderProjectSession` reference was 5395 and is now 6538. **What landed** (no Rust): a "Worker sessions" `.field-checkbox` with a one-line hint at the top of General (`index.html`). It reads `GET /api/workers-enabled` (`refreshWorkersEnabled`, awaited in bootstrap before the first project opens, and again after every `saveConfigFile`). It writes `enable_worker_sessions=true/false` through `upsertConfigValue` + `saveConfigFile` (`saveWorkerSessionsSetting`). The checkbox is `disabled` until `loadConfigFile` has filled the editor, because saving the empty editor would empty `user.conf`. When the effective value changes, the open session is reloaded so its blocks gain or lose the action; this does not happen during a turn. `finalizeChatMessage` now ends with `attachWorkerPromptActions(target, sessionId)`. The session id is read before the render request is sent. The action needs the setting on, a session that is not a worker, an `.assistant` message, and `code.className === "language-worker-prompt"`. It adds a `.worker-prompt-footer` row with a `.btn-sm` *Start worker* after the `pre`. `updateChatMessage` is unchanged. `showNewWorkerDialog` is built like `showRewindDialog`: its own backdrop, an editable monospace textarea, a `.worker-dialog-dirty` warning with *Include uncommitted changes* pre-selected and *Start from last commit*, a status line, *Cancel* (`.btn-quiet`) and *Start worker* (`.btn-primary`), and Escape cancels. *Start worker* stays disabled until the first `GET /api/worker-source-status` answers, so a dirty tree is never started without the choice shown. Editing the prompt re-checks after 400 ms. The warning names up to three files, the mentioned ones first. An empty prompt is refused in the dialog. `POST /api/workers` sends `include_uncommitted=true` only when the tree is dirty and *Include* is chosen, together with `parent_session_id`. **How the UI tells workers apart, without a new route:** `/api/session` carries no worker record (`origin` is `"user"` for workers too). `refreshWorkerIndex` therefore reads `GET /api/workers?repo=` for the open folder, and also for its source when the folder matches `<source>/.damaian/worktrees/<slug>`. It runs in `loadSession` before rendering, and does nothing while the setting is off. A session in those rows is a worker. Rows whose `parentSessionId` is the open session are its workers. **Parent links:** after a spawn, the clicked block's footer gets "Started *‹title›* →" and the message gets a muted "Spawned worker *‹title›* →" line after it. On reload, `renderSpawnedWorkerEntries` (just after `renderMessages`, which now returns `{message, bubble}` pairs) places each worker by the rule in proposal §5.7. The `worker-prompt` titles are found by rendering each assistant message's content with `renderMarkdown`, because it answers at once, before the async finalize attaches the links. The thread header falls back to the worker row's title, because a worker is not in its source folder's session list. **Verified in the browser** against the real UI and real workers. A scratch harness outside the repository served it: a bin depending on `desktop-shell` by path that serves `run_server` with a known token on port 4799 and handles `--worker` as `main.rs` does, because the standalone shell's token only arrives over Tauri IPC and the inspection test's `current_exe()` cannot host a worker. It ran with its own `DAMAIAN_DATA_DIR`, `DAMAIAN_MOCK_MODEL_RESPONSE` and a fixture repository with one modified tracked file and one untracked `docs/specs/demo/tasks.md`. The mock answer held untagged, `text`, `prompt`, `rust`, `worker-prompt extra`, `Worker-Prompt` and two `worker-prompt` blocks. The user message held a `worker-prompt` block. It was rebuilt and restarted, by PID, after every static change. **Checked:** with the setting off, a real turn showed no footer, no `[class*=worker]` element in the log apart from the code classes, and no `/api/workers` request. A click on the real checkbox wrote `enable_worker_sessions=true` into the editor and the file, and the reloaded history (the `/api/render-markdown` → `finalizeChatMessage` path) had exactly two footers, both on the exact `worker-prompt` blocks of the assistant message. None were on the other six blocks or on the user's block. `renderMarkdown` on the same content gives `language-worker-promptextra` and `language-Worker-Prompt`, which do not match. A wrapped `updateChatMessage` during a live turn recorded 17 streamed updates: the blocks appeared (0 → 1 → 2) with 0 footers each time, and there were 2 after finalize. Forcing `/api/render-markdown` to fail (the `renderMarkdown` fallback in `finalizeChatMessage`) still gave the two footers. A user bubble put through `finalizeChatMessage` got none. Pre-fill was exactly `"Implement Task 1 from docs/specs/demo/tasks.md.\n\nRead main.rs first. Stop after Task 1."`, and `<setup> & "quotes"` arrived decoded. The dirty dialog said "⚠ 2 uncommitted files in your tree, including `docs/specs/demo/tasks.md` and `main.rs`…" with *Include* checked, and focus was in the prompt. One line was then added in the dialog and *Start worker* clicked. The worker's first user message was the edited text. Its worktree is under `repo/.damaian/worktrees/implement-task-1-from-docs-specs-demo` on `damaian/implement-task-1-from-docs-specs-demo`, with the modified `main.rs` and the untracked `tasks.md`. `.damaian/worktrees/` was added to `info/exclude`. The progress file showed `status:running, phase, phase, text, status:idle`, the "Started" link was on the clicked block, and the entry followed its message. A second spawn with *Start from last commit* gave a worktree with the committed `main.rs` and no `docs/`, and the two records say `includedUncommitted` true and false. After a reload the links and the entries came back. The link opened the worker's session: the header showed its title, and the session had 3 `worker-prompt` blocks and no footer. The same held with the worktree opened as the folder (source derived). With the fixture committed, the dialog showed no warning. Turning the setting off from the checkbox removed every footer and entry. Each harness stop ended its workers, and nothing was left running by path afterwards. **Deviations:** (1) Step 4: the links are rebuilt from the worker rows, not from the parent's `worker_spawned` events, which no route returns. No Rust was added. A worker whose first line was edited, or two blocks with the same first line, are matched as proposal §5.7 now records. (2) Until Task 9 the link opens the worker in place. (3) `docs/ui-style-guide.html` was not given specimens for the footer, the dialog or the entry; this task's file list did not include it (style guide §9). **For Task 9:** reuse `workerRows`/`refreshWorkerIndex` (or replace them with the board's own poll) and `openWorkerSession`, which is the one place that should learn the worker's sidebar location. `isWorkerSession` depends on the rows being loaded for the folder the worker is opened from. The sidebar's "New worker" is `showNewWorkerDialog({})`, with no parent. **For Task 10:** the worker view still has the ordinary composer, which sends a shell turn that the engine refuses ("belongs to a different repository") while the source folder is open. **Checks:** `node --check` and `npm run lint:web` are clean (its one info is the existing `scripts/check-spec-status.mjs` template hint). `scripts/quality-gate.sh fmt node lint typos deny` passed on all five, with `deny` outside the Bash sandbox (read-only `~/.cargo/advisory-dbs`). No Rust changed, so no clippy or tests were run locally. The workspace clippy and test suite are left to CI. The full seven-check gate passed in CI on 2026-10-16, on `main` after the merge. |
| 9 · UI: sidebar nesting and the Workers board | Done 2026-10-16 | **References re-checked first:** none had drifted. `renderProjectSession` was at `app.js:6538` (now 7308), and `refreshWorkerIndex`, `openWorkerSession`, `isWorkerSession`, `showNewWorkerDialog`, `createContextMenu` and the `GET /api/workers` row in `workers.rs::worker_row` were as Task 8 and proposal §5.7 describe. **What landed** (no Rust): `app.js`, `style.css`, and an empty `<section id="workers-board" hidden>` in `index.html`. Rows are kept per project in `workerRowsByProject`, filled by `loadProjectWorkers` (`GET /api/workers?repo=`), by `refreshWorkerIndex` and by the board's poll, and cleared when the setting goes off. *Sidebar:* `renderProjectList` puts a `.project-workers-entry` first (badge = rows in `needs_you`), then `sidebarEntries`: sessions in their order, each worker under its parent oldest first, and parentless or orphaned workers among the sessions by `updatedAtMs`. `renderProjectWorker` draws a `.worker-dot` and the worker ⋮. `openWorkerSession` now opens a worker under its source project (from `workerSourceOf(row.worktree)`) and marks its sidebar row active, or opens it in place when only the worktree is a project. `{review: true}` scrolls to the last approval card. *Board:* `openWorkersBoard`/`hideWorkersBoard` swap it for the log and composer (`body.workers-board-open`). `renderWorkersBoard` draws a table with *Needs you*/*Running*/*Finished* `tbody`s, newest first, and redraws only when the rows changed. A 1 s ticker updates elapsed and the status line. One visible action per row (`workerRowAction`) and one menu, `workerMenu` (`createContextMenu`, `.btn-icon` + `.dots-icon`). *Routeless actions:* *Reveal* is `POST /api/reveal-in-finder` with the worktree as `repo`. *Dismiss* is client-side, in `localStorage` `damaian:dismissedWorkers`, and is undone when the worker is no longer finished. *C10:* `pollWorkersBoard` keeps one request out and schedules the next 1 s after it answers. A board reopened mid-request polls once it lands, and a hidden window does not poll. Slowness is a quiet "Updated N ago. Waiting for the shell…" line after 3 s; only a failed request says "Could not refresh". *New worker* is on the board header and in the project ⋮. **Verified in the browser** against Task 8's scratch harness, rebuilt in this session's scratchpad (a bin depending on `desktop-shell` by path; `run_server` with a known token on 4799; `--worker` as in `main.rs`), plus a `--seed-worker` mode that writes stub workers through `SessionStore`/`ProgressWriter`. It was rebuilt and restarted, by PID, after each static change. Fixture: a source repo, two real workers started from a planner's `worker-prompt` blocks, one real parentless worker from the board's *New worker*, and four stubs on real `git worktree`s: complete with 2 commits and a dirty file, failed with an error, cancelled, and parentless complete. Live states came from status lines appended to the real workers' progress files (running round 3/24, waiting for approval, paused), because only a worker this shell holds is `alive`. **Checked:** the sidebar nested five workers under the planner and listed two parentless ones with dots, and the badge read 2 for the two needs-you rows. The board's groups were *Needs you* 2 (paused, waiting), *Running* 1 and *Finished* 4, newest first. Actions were *Open*/*Review*/*Pause*/*Open*, and finished rows showed "3 files changed · 2 commits ahead" and "0 files changed · 0 commits ahead". Overflow: live rows had *Reveal* and *Stop*; finished rows had *Reveal*, *Dismiss* and *Remove worktree*, danger last. A live idle finished row had *Stop* and *Remove worktree* disabled with "Stop the worker first". *Remove* on the dirty stub: the first dialog named the path and said the branch is kept; the second said "…discards: main.rs." and *Remove anyway* removed the worktree. The branch remained, and `worker_worktree_removed` was audited. *Dismiss* hid the row (7 → 6), kept it in the sidebar and stored the id. *Pause* sent `command=pause` (the stubbed worker's real idle state answered "no turn is running"); on a real running worker it answered "pauses at its next round". *Review* hid the board, opened the worker and made its nested sidebar row active. *Stop* on a real idle worker sent `command=shutdown`; the row went to `exited` and *Remove* became enabled. *Reveal* answered with the worktree path (and opened Finder). Polling: 5 `/api/workers` requests in 5 s while open, 0 in 5 s after opening a session, and the timers were cleared. **C10, driven for real:** the harness was restarted without the mock, and a non-built-in provider `slowtest` pointed at a local endpoint that holds every request 25 s. A planner turn then streamed for about 99 s (the provider retries), with the board opened during it. Exactly one `/api/workers` request was out for those 99 s, rows stayed (7), there was no error, and the waiting line showed from 3 s. When the turn ended the request answered 200 and polls resumed at 1 s with the line cleared. C5: the real workers whose process ended while stubbed paused or waiting showed failed, "Interrupted: its process ended mid-turn" and *Open*. Setting off through `saveWorkerSessionsSetting(false)`: no `.project-workers-entry`, `.worker-dot`, `.workers-badge`, `.project-worker-item`, footer or entry; the board was hidden and emptied; no *New worker* in the project ⋮; 0 `/api/workers` requests in 5 s. Every harness stop ended its workers, and nothing was left running by path. **Deviations,** written into proposal §5.7: (1) *Stop* shows on every live row and sends `shutdown` when the worker is idle, because an idle worker refuses `stop` and nothing else could end a finished but live worker. (2) *Remove worktree* stays visible but disabled on a live finished row. (3) The sidebar has no timed poll, so with the board hidden a badge can be stale until a project draw, session load or window focus. (4) A finished row's elapsed shows "—" (discovery 2). (5) The finished dot is `--line-strong`, not `--ok`, because `--ok` beside the running `--accent` read as the same colour in the screenshot. §7 gains "serve worker routes during a stream" (C10); `TROUBLESHOOTING.md` stays with Task 11. `docs/ui-style-guide.html` was not given specimens (not in this task's files, as in Task 8). **Discoveries:** (1) A real worker's `phase` lines carry `label: ""` for the context and model phases, so the row's `doing` is empty and the board says "Working". The row does not carry the phase kind. (2) A session's `updatedAtMs` does not move while a worker's turns run, so a row has no end time. A `lastProgressAtMs` on the row would fix elapsed. (3) A data directory whose socket path exceeds `sun_path` (160 bytes here) is refused only in `start`, after the worktree, the branch, the session and the worker record exist. The fixture's first two spawns left two worktrees, branches and records with no process. Checking `worker_socket_path` before `create_worker_worktree` would refuse first. Not fixed (Rust, out of scope); `docs/PLAN/` exists only in the main checkout, so it is recorded here. (4) A `model_provider.openai.api_key_env` override still asked for `OPENAI_API_KEY`. Not investigated; a non-built-in provider was used instead. **For Task 10:** *Review* scrolls to the last `.command-approval`, `.patch-preview` or `.recovery-card` in `#chat-log`, so the worker view's approval card should use one of those or update the selector in `openWorkerSessionAt`. `workerRowFor`/`findProjectWorkerRow` give the open worker's row (state, round, branch, parent title) for the strip. Idle ends with `shutdown`, not `stop`. Task 7's open question (whether `stop`/`shutdown` stay allowed while the setting is off) is still open; with the setting off the UI shows no worker controls at all. **Checks:** `node --check` and `npm run lint:web` are clean (its one info is the existing `scripts/check-spec-status.mjs` hint). `scripts/quality-gate.sh fmt node lint typos deny` passed on all five, with `deny` outside the Bash sandbox (read-only `~/.cargo/advisory-dbs`), and the harness and slow endpoint ran outside it too (no local port binding). No Rust changed, so no clippy or tests were run locally. The workspace clippy and test suite are left to CI. The full seven-check gate passed in CI on 2026-10-16, on `main` after the merge. |
| 10 · UI: worker session control strip and composer | Done 2026-10-16 | **References re-checked first:** none had drifted in substance. `renderThreadHeader` (`app.js:299`), `sendChatPrompt` (9975 after this task) and the `ask-btn` handlers, `createCommandApprovalPreview` (9170) and `createPlanReview` (8578) with their resume streams, `renderRecoveryPrompts` (4473), `openWorkerSessionAt` (5231), `workerRowFor`, and `GET /api/worker-progress` and `POST /api/worker-restart` in `workers.rs::route` were as Task 9 and the Interfaces describe. Step 4's "with `DAMAIAN_MOCK_MODEL_RESPONSE`" was corrected: the mock cannot hold a turn or propose a command. **What landed** (`app.js`, `style.css`; no Rust, no `index.html`): `syncWorkerView` (6263), called last in `loadSession`, starts, keeps or ends a `workerView` for the open session. It exists only while the setting is on, the session is a worker and the board is hidden; `clearChat` and the poll itself end it. *Strip* (`renderWorkerStrip`, 6598) is inserted under `.thread-header`: a pill with a `.worker-dot`, `Round r/max` with the board's bar ("Round —" before the first `phase`), a note line for a failed poll or a failed turn, and the controls. *Pause* and *Stop* while running, *Resume* (the only `.btn-primary`) and *Stop* while paused, *Stop* while waiting, *Stop* while idle (sends `shutdown`, as on the board), and *Restart worker* once the process ended. *Pause* and *Stop* reuse the board's `pauseWorker`/`stopWorker`. The header gets a `.thread-worker-meta` line, "from *‹parent›* · `branch`" (`renderWorkerHeaderMeta`, 6553). *Composer* (`renderWorkerComposer`, 6715; `sendWorkerComposer`, 6755): `sendChatPrompt` hands off to it while the view is open. The round send button becomes a labelled `.is-worker-command` (*Send note* → `note`, *Resume with note* → `resume` with `note`, *Send message* → `message`), and the attach menu and model choice are hidden (`body.worker-view`). It is disabled with a reason in the placeholder while waiting, stopping or ended. The original icon, label and placeholder are restored when the view ends. *Live progress* (`pollWorkerProgress`, 6337; `applyWorkerProgress`, 6381) follows `pollWorkersBoard`'s C10 shape: one request out, the next 1 s after the answer, no error for slowness, nothing while hidden (`visibilitychange` restarts it). The first read catches up silently. Later lines feed one live assistant bubble with the shell's `startTurnIndicator`. A `note` line closes it and adds the note as a user message (text from the notes this view sent, else from `/api/session` by `messageId`). When a job ends, or `alive` drops, the session is drawn again through `loadSession`. *Approvals* (`renderWorkerApproval`, 6821): the shell's own builders gain a `decide` option that posts `approve`/`reject` with `proposal_id` to `/api/worker-control` instead of resuming a shell turn. Without it they are unchanged. A worker's command card offers no grants and its plan steps are read-only, because the socket's `approve` takes neither. The card is `.command-approval`, so the board's *Review* selector holds; it is placed in the last assistant `article` beside its body, which `finalizeChatMessage` re-renders. *Review* now waits for the view's first read (`workerViewCaughtUp`, at most 3 s). Patch and fallback offers use the shell's cards unchanged with the worktree as `repo` while the worker is idle after that turn. *Restart* (`restartWorkerFromView`, 6693): `POST /api/worker-restart`, then the view reads the new progress file from 0. **Verified in the browser** against the real UI and real workers. Task 9's harness (a bin depending on `desktop-shell` by path; `run_server` with a known token on 4799; `--worker` as `main.rs`) was copied into this session's scratchpad and pointed at this worktree. Data dir `/private/tmp/claude-502/s57t10`, fixture repo `/private/tmp/claude-502/s57t10-repo`. The configured provider was a non-built-in `scripted` (`supports_native_tools=true`) at a local Python endpoint in the scratchpad. It holds each request 8 s (60 s for a `TURN: slow` turn) and per turn answers round 1 with `run_command git status --short` (runs without approval), round 2 with `run_command touch worker-approved.txt` (needs approval) and round 3 with streamed text, logging what each request carried. Harness and endpoint were restarted by PID after each static change. **Checked:** *running*: "Running · Round 1/8 · Pause · Stop", composer *Send note*. *Note while running*: NOTE-A sent during round 1 was in round 2's request to the model, and the log shows it as a user message before round 2's answer. *Waiting*: "Waiting for approval · Round 2/8 · Stop", composer and field disabled with "Approve or reject the proposal above first…", and the card was the shell's `.command-approval` with `touch worker-approved.txt` in full and no overflow. *Approval in process*: clicked *Approve*; worker **PID 81509 before and after**, `worker-approved.txt` appeared in the worktree, round 3 ran, and the view went *Idle* with *Send message* and no card. *Ended (finished)*: after the harness restart ended 81509 by its lifeline, the view showed "Ended · Restart worker" with the composer disabled. *Restart*: new PID 85490, idle. *Pause*: during round 1 of the next turn; at the boundary the strip read "Paused · Round 1/8 · Resume · Stop" (*Resume* the only `.btn-primary`, *Stop* the only `.btn-danger`), composer *Resume with note*. *Resume with note*: NOTE-B appeared as a user message before round 2's answer. *Stop while waiting*: the confirm dialog, then straight `shutdown`; 85490 exited and its `chat/pending/cmdprop_….json` stayed on disk. The view showed "Interrupted · Restart worker" (row `taskStatus` `interrupted`). *Restart of an interrupted worker*: new PID 86793, idle, no card, toast "…The approval it was waiting for ended with its old process, so that command did not run…". *Stop while running* (`TURN: slow`, request held 60 s): *Idle* within about 2 s, the same PID 86793, task `cancelled`; the escalation went no further. *Poll*: 5 `/api/worker-progress` requests in 5 s in the view, 0 in 5 s after opening a new session, and the icon button and placeholder were back. *C10*: an `/api/ask-stream` turn in an ordinary session of the source repo, held by the endpoint and aborted after 40 s. The progress request out when it began answered after 41 s, at most one was ever in flight, the strip kept its state with no error line, and polling resumed at 1 s (40 requests in 81 s). *Parent*: a worker started with a parent showed "from *TURN: slow shell turn* · `damaian/turn-three`". *Setting off* (`saveWorkerSessionsSetting(false)` with the worker open): no `.worker-strip`, no `.thread-worker-meta`, no `body.worker-view`, no request containing `worker` in 5 s, the icon send button, "Send message" label and default placeholder, and the attach menu shown. Every harness stop ended its workers; nothing was left running by path afterwards. **Deviations,** written into proposal §5.7 Worker session: (1) *Stop* also on an idle worker (sends `shutdown`), as on the board. (2) *Restart worker* also for a worker that ended finished, not only interrupted, because nothing else lets an ended worker take a message; it is not filled. (3) Context files and model choice are hidden in a worker's composer. (4) The recovery card is not shown for a worker session (discovery 1). (5) `refreshFindings` uses the worker's worktree as `repo`, because opened under its source project every load toasted "Session … belongs to another repository", and the view now reloads at every turn end. **Discoveries:** (1) **The shell's recovery sweep classifies a live worker's turn as crashed.** `GET /api/recovery` ran while the worker was preparing context, audited `task_recovered` (`classification: interrupted`, `previousStatus: preparing_context`, `danglingAction: model_call`) for that live task, and offered *Resume* in the shell. The worker carried on and finished. The UI no longer offers that card for a worker, but the sweep's record and classification are engine behaviour and are left for Task 11 or a follow-up: it cannot tell a task owned by a live worker process from a crashed one. `docs/PLAN/` exists only in the main checkout, so this is recorded here. (2) The approval line's proposal passes through the progress file's redaction, so a secret-shaped value in a command reads redacted on the worker's card. That is stricter than the shell's card, not weaker. (3) Each worker job gets a fresh `TurnSteering` (Task 6). A note queued in a round that then stops for approval may therefore be dropped rather than carried into the resumed job. Not tested here; worth a check in Task 11. **Task 7's open question** (whether `stop`/`shutdown` stay allowed while the setting is off) stays open: with the setting off the UI offers no worker controls at all, and the route behaviour is a Rust decision. **For Task 11:** discovery 1 for `TROUBLESHOOTING.md` or a follow-up, discovery 3 to check, the strip and composer for `USER_GUIDE.md`, and the scripted endpoint (`TURN:` markers, `run_command` rounds) as a reusable way to drive a real worker turn. **Checks:** `node --check` and `npm run lint:web` are clean (its one info is the existing `scripts/check-spec-status.mjs` hint). `scripts/quality-gate.sh fmt node lint typos deny` passed on all five, with `deny`, the harness and the endpoint outside the Bash sandbox (read-only `~/.cargo/advisory-dbs`, no local port binding). No Rust changed, so no clippy or tests were run locally. The workspace clippy and test suite are left to CI. |
| 11 · Kill test, measurement, docs, close the spec | Done 2026-10-16 | **Base:** local `main` did not yet have Task 10, so this branch was cut from `main` and `worktree-spec57-task10` was merged into it at the user's choice. The one conflict was this table: `main`'s row 9 (with its CI note) and Task 10's row 10 were kept. **Carried forward, each checked against the code:** (1) **Task 10 discovery 3 confirmed and fixed.** The new `worker_turns::a_note_queued_before_an_approval_reaches_the_resumed_turn` gates round 1's model call, sends a note during it, lets the round stop for a command approval and approves it. Against the unchanged code it failed: the session log had the prompt, the approval text and "Done.", but no note, because `hand_over` gave the resumed job a fresh `TurnSteering`. The fix stays inside "same turn, different host": `hand_over` drains the old steering's undelivered notes into the new one for a `Decide` job only. A decision resumes the same turn (§5.6), and its loop's top drains them. No turn logic moved into `worker.rs`. The test passes, and reverting the carry fails it again. **Found while reading:** a note accepted in a turn's *final* round is not delivered either, because the turn completes without returning to a boundary. That is decided by reading `run_agentic_turn`, not tested; it is left as §7 item 6, because carrying it into the next `message` would put it after a later user message, and `USER_GUIDE.md` says so. (2) **Task 7's open question, decided:** `stop`/`shutdown` stay refused (403) while the setting is off, like every worker route but the list. With it off the UI offers no controls, so allowing them would serve no client. The worker still cannot outlive the app, and turning the setting on restores control. Proposal §5.7 *Setting off* records it, and §5.0's "the board stays reachable" was corrected to what Tasks 9–10 built (the board hides too). (3) **Task 4 (b) / Task 7 (b)**, no `git_commands` on a failure after `worktree add`: not fixed, because it changes `create_worker_worktree`'s error contract (Interfaces). It is §7 item 5, §5.1 points there, and `TROUBLESHOOTING.md` says where to look. (4) **Task 9 discovery 3, fixed:** `Supervisor::spawn` now checks `worker_socket_path` against the absolute data directory before `create_worker_worktree`, since the path's length depends only on the data directory. `worker_supervisor::a_data_dir_too_long_for_the_socket_is_refused_before_the_worktree` asserts no `.damaian/`, no `damaian/*` branch and no `sessions/`; with the check removed it fails. (5) **Task 6's accepted deviation stays:** the shell honours `DAMAIAN_MOCK_MODEL_RESPONSE` because both hosts build adapters through `chat_model_adapter`, and a worker inherits the shell's environment anyway, so checking only in the worker would make one host scripted and the other not. The real-binary tests depend on it, and it is the CLI's existing behaviour. `TROUBLESHOOTING.md` now says the desktop shell honours it. (6) **C12** is named in the §6 walk (`crash_recovery.rs::a_live_worker_session_is_not_recovered`, `recovery.rs::a_live_worker_is_not_offered_for_recovery`) and in `TROUBLESHOOTING.md`. **Step 1, kill test:** `desktop-shell/tests/workers.rs::sigkilling_the_shell_ends_its_worker_and_the_workers_command`, ignored, with its helper `kill_helper_runs_a_worker_command_and_waits`. The helper is the "shell": #46's handler as `main` installs it, a `Supervisor` over the real binary, and a mock model asking for `sleep 300` (approved when asked). It writes the worker's and the command's pids once the worker has registered the command. The test checks both registry entries (worker owned by the shell, command owned by the worker), `SIGKILL`s the helper (its own unreaped child, after an identity check) and requires both pids and the command's process group (`pgrep -g`) gone within 5 s. A guard `SIGTERM`s any survivor by registered identity, so a failing run leaves nothing. Run: `worker 24124 and command 24167 (group 24167) gone 44.163917ms after SIGKILL of shell 24056 ... ok`. **Falsified:** the lifeline's EOF turned into a 1 h sleep → `timed out waiting for the worker and its command to be gone` at 5 s, and the guard cleaned up (nothing left by path). The test and `ps` need to run outside the Bash sandbox (`ps` is denied in it). **Step 2, cold start** (informational, debug build): ignored `worker_cold_start_on_this_repository`, on a local clone of this checkout (333 tracked files), mock model, three runs. `spawn` returned in 163/152/143 ms, the socket answered at 406/177/169 ms, and the first `model_request_prepared` came at **7.60/7.29/7.50 s**. That is almost all the worker indexing a worktree it has never seen. Recorded in proposal §6. **Step 3, docs:** `USER_GUIDE.md` § Worker Sessions (setting, *Start worker*, include choice, what is created, board, strip, notes and their limit, pause, stop, approvals, restart, lifetime, removal, branches never deleted, turning off), and `enable_worker_sessions` added to the repository-ignored key list. `TROUBLESHOOTING.md`: the worker row in the process model, the progress file and socket in the data-dir table, the `SIGKILL` exception in the orphan section, and § Worker sessions (where a worker's files are, an `/api/workers` query, audit events, C10's held board and strip, *Interrupted*/*Ended* and *Restart worker*, C5 and C12, start refusals including the socket length and the setting-off 403, leftover worktrees after a crash, `DAMAIAN_MOCK_MODEL_RESPONSE`). `SECURITY.md` § Worker Sessions (stdin handover, the socket's token, permissions and limits, the lifeline, the `.damaian/worktrees` checks) and the key in the forbidden list. **Step 4:** §7 already had C10's follow-up (item 4, from Task 9). Added "smaller open items" 5–9: partial git commands, final-round notes, the board's row facts (Task 9 discoveries 1–2), a worker's model (Task 6 (a)), the *Include* base commit (Task 4 (a)). **Step 5:** proposal §6 *As verified* names a test or recorded manual check for every item. Two items were not covered as written: the stdin-close test is an ignored re-exec test, not "a normal test", because it spawns a real process; and the existing-path refusal has no test of its own, because naming skips existing paths (`colliding_names_get_matching_suffixes`), so the refusal is a guard nothing can reach. **Step 6, acceptance: not run.** It needs a live model and the user's key, which an agent session must not hold. §6 lists what other checks show and the one thing they do not: that a real model, told only the §5.0 paragraph, writes `worker-prompt` blocks when asked to split work in a repository without `AGENTS.md`. **Step 7:** Status, README row, "What to build next" (re-derived: no `Depends on:` names #57; the ready set is #23 alone; #38's stage-1 re-evaluation condition is now met, and that decision is the user's), the priority table's #38 row, the `Done` header, and `CHANGELOG.md` Unreleased (the worker line removed). `grep -rn 57_worker_sessions` finds only cross-references (#17, #38, #39), no `Depends on:`. **Step 8, checks** (the full gate is CI's since 2026-10-15, which overrides this step): `scripts/quality-gate.sh fmt node lint typos deny` all ok; `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean (the only crate changed); `cargo nextest run -p desktop-shell --locked` 156/156 (16 skipped, the ignored ones); the ignored worker tests by hand (`cargo test -p desktop-shell --test workers --test worker_process -- --ignored --test-threads=1`): `worker_process` 5 passed, `workers` 7 passed, including the kill test and the measurement; `npm run specs:check`: "58 specs, dependency lines agree". The crate tests, the ignored tests and `cargo deny` ran outside the Bash sandbox (local port binding, `ps`, read-only `~/.cargo/advisory-dbs`); nothing was left running (`ps`, by path). The workspace clippy and test suite are left to CI. |

**Goal:** Let the user start a worker — a new session with its own process,
in its own new git worktree — from a model-suggested `worker-prompt` block,
then watch, pause, steer, approve and stop it from a per-project board,
without anything it started outliving the application.

**Architecture:** The engine gains three new modules: `steering.rs` (pause
and notes at the round boundary, carried on `TurnSink`), `worker_progress.rs`
(the progress file) and `worktree.rs` (the checked creation and removal of
`.damaian/worktrees/<slug>`). It also gains a worker record on `SessionStore`
and one optional system-prompt paragraph. `desktop-shell` gains `worker.rs`, the
`--worker` host. This is the same binary re-entered from either `main`; it
runs turns through the unchanged `ChatOrchestrator` entry points. It also
gains `workers.rs`, the shell-side supervisor that spawns, registers,
controls and lists workers through short Unix-socket exchanges. The shell stays
single-threaded. The UI work is three slices of `app.js`/`style.css`.

**Tech Stack:** Rust 2024 (workspace edition), `std::os::unix::net`. No new
dependencies are expected: the token comes from `/dev/urandom`, the TTY check
uses `std::io::IsTerminal`, and `libc` is already a `workspace-engine`
dependency (#46). If a task finds it needs a new crate, it must stop and justify
the crate in the Progress table first.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`proposal.md`](proposal.md) §2 and §5 and the corrections below
  before starting.** The corrections were found while planning, on
  2026-10-04, by reading the code. Where one contradicts the proposal's prose,
  the correction wins, and the task that acts on it updates the proposal in the
  same change.
- **Same turn, different host** (requirement 4). A worker calls the same
  `ChatOrchestrator` methods the shell calls. If a task finds itself copying
  turn logic, approval logic, redaction or command policy into `worker.rs`, it
  has left the spec. Stop and say so.
- **Off means byte-for-byte off** (requirement 1). With
  `enable_worker_sessions = false`, `system_prompt` output, every existing
  route's response and the rendered UI are unchanged.
  `code_mode_system_prompt_is_byte_identical_to_todays` (`chat.rs:5526`)
  passes **unmodified**. Do not edit that test or its constant.
- **Nothing survives** (requirement 8). Every process kill goes through the #46
  registry and a `ProcessIdentity` comparison. Never kill by name: no `pkill`,
  no `killall`. This also applies to cleaning up processes you started while
  testing.
- **The key never touches env, argv or disk** (requirement 9). The worker reads
  it from one stdin line. No test, log line, progress line, audit field or error
  message may contain it.
- **`.damaian/` is untrusted** ([#34](../34_repository_config_trust_boundary.md)).
  Every path under it is checked before use (§5.1 step 3). No new config key
  may be anything but `Forbidden` in repository scope.
- **Tests:** fixture repositories set `enable_index_watcher: false`. Every test
  uses its own data directory. Model-dependent paths use
  `MockModelAdapter` or `DAMAIAN_MOCK_MODEL_RESPONSE`. Anything that spawns a
  real worker process, binds a socket outside a temp dir, or `SIGKILL`s
  something is `#[ignore]`d, with the exact manual command in its doc comment.
  `crates/workspace-engine/tests/process_registry.rs:125-261` is the
  re-exec pattern to copy (the test, its helpers and `register_silent_child`).
- **Falsify every load-bearing test.** Break what it guards and confirm it
  fails, then record that in the Progress row. #49's Task 5 found a test that
  passed against a broken implementation.
- **UI verification** runs the shell on a port other than 4765, with its own
  `DAMAIAN_DATA_DIR`. Static assets are `include_str!`-embedded, so rebuild and
  restart before checking. Stop the instance by its PID.
- **A private Cargo target directory per worktree.** Tasks run in parallel
  worktrees. Every `cargo` command is prefixed with
  `CARGO_TARGET_DIR="$PWD/target"` and run from the worktree root, so that
  builds neither share nor thrash one target directory. Shell state does not
  persist between an agent's commands, so put the prefix on each invocation,
  e.g. `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine`.
- **Scoped checks per task.** The full seven-command gate from `AGENTS.md`
  runs once, in Task 11. It takes `cargo nextest run --workspace` about 5
  minutes locally.
- **Never `git commit` unasked.** Each task ends by showing the change and the
  scoped check results, then asking. Use one subject line, with no body.

## Corrections to the proposal found while planning

Each one names the task that owns it.

| # | Proposal says | Code says | Resolution |
|---|---|---|---|
| C1 | `system_prompt` appends the paragraph (§5.0) | `fn system_prompt(mode: SessionMode) -> String` (`chat.rs:3580`) knows only the mode; its one caller is `chat.rs:765` | Keep `system_prompt(mode)` unchanged. Add `system_prompt_for_session(mode, offer_handoffs: bool)`, called at `chat.rs:765`. **Task 1** |
| C2 | Events are appended to the session (§5.1 step 7) | `append_session_event` (`session.rs:1906`) is private. Sessions are keyed by repository id, and `create_session(repository_id, title)` takes no path | Add typed public methods on `SessionStore` (Interfaces, Task 1). The worktree path lives only in the `worker_spawned` record. **Task 1** |
| C3 | Notes and progress pass "the same redaction as the session log" (§5.4, §5.5) | The session log is deliberately unredacted on write (`session.rs:1649`). Redaction happens at the call sites (`SecretScanner::redact`) | Notes are redacted where user messages are, i.e. on the way to the model. Every progress-file value goes through `SecretScanner::redact` before it is written. **Tasks 2, 3** |
| C4 | Adding `steering` to `TurnSink` (§5.4) | There are 36 `TurnSink { .. }` literals (32 when planned; #31 added `tests/permission_profiles.rs`, and more were added since): 6 in `chat.rs`, 5 in `desktop-shell/src/lib.rs`, 1 in `eval-harness/src/runner.rs` and 24 under `workspace-engine/tests/` | Mechanical: `steering: None` at each. The compiler names them all. **Task 2** |
| C5 | `Paused` is recoverable via #17 (§5.4) | `recovery.rs::classify_session` treats any non-terminal status except `WaitingForApproval` as `Interrupted`, and may **auto-resume** it in the shell | Correct for classification. But a worker session must never auto-resume in the shell: its repository is a worktree and its host is gone. Recovery skips auto-resume for any session with a worker record, and the UI offers *Restart worker* instead. **Tasks 2, 7** |
| C6 | "The worker registers its PID … tagged with its session id" (§5.2) | `ProcessRegistry` records the **owner** as the registering process (`process_registry.rs:99`). `ProcessKind` has no `Worker` variant | The **shell** registers the worker child (`ProcessKind::Worker`, the worker's session id, owner = shell), so the launch sweep kills workers of a crashed shell. Each worker registers its own children (owner = worker), so `sweep_own` inside the worker kills exactly its children. **Tasks 5, 7** |
| C7 | `/api/worker-control` takes `{sessionId, command, ...}` (§5.7) | Every shell route takes url-encoded form fields (`parse_form`, `lib.rs:3318` after Task 7; `:2894` when planned, `:3274` before Task 7) | Form fields: `session_id`, `command`, plus `text`, `note` and `proposal_id` as needed. Only the **socket** speaks JSON lines. Done in Task 7; proposal §5.7 updated. **Task 7** |
| C8 | The tag rule is "checked by driving `renderMarkdown`" (§6) | A finished message is re-rendered **server-side** (`finalizeChatMessage`, `app.js:4214` after Task 8, `:4203` before it, `:3513` when planned → `workspace-engine/src/render.rs:376`, unchanged), which writes the trimmed info string verbatim into `class="language-…"`. `renderMarkdown` strips non-`[a-z0-9_-]` characters | Attach the action after `finalizeChatMessage`. Match `code.className === "language-worker-prompt"` exactly, which rejects `worker-prompt extra`. Verify through both renderers. Done in Task 8: the server writes `language-worker-prompt extra`, the client `language-worker-promptextra`, and neither matches. **Task 8** |
| C9 | "Worker sessions" shown in Settings (§5.0) | Settings has no bool toggle. General is a raw config textarea. `/api/config` returns policy text only | A `.field-checkbox` on the General page, written through `upsertConfigValue` + `saveConfigFile` (the MCP form's pattern, `upsertMcpConfig`/`saveMcpServer`, `app.js:3621`/`3649` after Task 8, `3616`/`3644` before it, `2923`/`2951` when planned). `GET /api/workers-enabled` reads the effective value. Done in Task 8. **Task 8** |
| C10 | The board polls about once a second (§5.7) | The accept loop (`lib.rs:115`) is blocked for the whole of an `/api/ask-stream` turn | **Accepted, decided 2026-10-04.** While any session in the window is streaming, the board and progress polls stall until the turn ends. Workers keep running, and only the view lags. Document this in proposal §5.7 and `TROUBLESHOOTING.md`, and name "serve worker routes during a stream" as a follow-up in §7. Task 9 built the board's tolerance (one request out, no error for slowness) and wrote §5.7 and §7; `TROUBLESHOOTING.md` is Task 11's. **Tasks 9, 11** |
| C11 | `RunEvent::Exit` hook (§5.3.4) | `desktop-app/src/main.rs:135` ends in `.run(tauri::generate_context!())`, and the shell runs **in-process** on a thread (line 92). `current_exe()` is the Tauri binary | Switch to `.build(ctx)?.run(\|app, event\| …)`. The `--worker` branch goes at the top of **both** `desktop-app` `main()` (line 36) and `desktop-shell` `main()`, before `install_shutdown_handler` (`desktop-shell/src/main.rs:10`). Lines re-checked after Task 5 added both branches (2026-10-12); before it they were 129, 86 and 4. Re-checked at the start of Task 7 (2026-10-15): unchanged. Task 7 made the switch: `.build(tauri::generate_context!())` at line 135, `RunEvent::Exit` → `desktop_shell::shutdown_all_workers()` at 140; proposal §5.3 step 4 updated. **Tasks 5, 7** |
| C12 | "A worker whose process ended without finishing its turn is marked `Interrupted` through #17" (§5.3) | `classify_all` classifies every non-terminal task in the data directory, and the shell runs it on the first `GET /api/recovery`, which can follow a worker's spawn. Found 2026-10-16 in Task 10's browser checks: a live worker's `preparing_context` task was audited `task_recovered`, `interrupted`, dangling `model_call`, and offered *Resume* | **Decided 2026-10-16.** `classify_session` and `reattach_pending_approvals` skip a session with a worker record and a live `ProcessKind::Worker` registry entry (`process_registry::live_worker_registered`). A dead worker is classified as before; skipping every worker session was rejected because a dead worker's task would then never become `Interrupted` and its lost model call would go unaccounted. Proposal §5.3. Tests: `crash_recovery.rs::a_live_worker_session_is_not_recovered`, `desktop-shell` `recovery.rs::a_live_worker_is_not_offered_for_recovery`. The full seven-check gate passed in CI on 2026-10-16, on `main` after the merge; scoped checks passed locally: `crash_recovery`, `recovery_prompt`, `token_accounting`, `process_registry` and the shell's `recovery` tests, scoped clippy for both crates, and fmt, node, lint, typos and deny. **Task 11** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/config.rs` | `enable_worker_sessions` (copy `audit_enabled`'s touchpoints: field 265, default 2058, overlay field 2118, `set` 2417, destructure 827, `scoped` 902, `classify_overlay_fields!` 2238 (Capability, added by #31), and both `to_policy_text`s, 1930 and 2754 with its destructure at 2637) |
| `crates/workspace-engine/src/profile.rs`, `tests/permission_profiles.rs` | `split_profile_keys` refuses the key (348, 384); a `weakening_cases` row (228). Both added by #31 after this plan's line numbers were first taken |
| `crates/workspace-engine/src/session.rs` | `TaskStatus::Paused`; `WorkerRecord`; worker and steering-note event methods |
| `crates/workspace-engine/src/chat.rs` | `TurnSink.steering`; `system_prompt_for_session`; the round-boundary pause/note step; `TurnProgress` variants |
| `crates/workspace-engine/src/steering.rs` | **New.** `TurnSteering` |
| `crates/workspace-engine/src/worker_progress.rs` | **New.** Progress file writer and reader |
| `crates/workspace-engine/src/worktree.rs` | **New.** Slugs, checks, creation, untracked copy, removal, dirty summary |
| `crates/workspace-engine/src/process_registry.rs` | `ProcessKind::Worker` |
| `crates/workspace-engine/src/recovery.rs` | No auto-resume for worker sessions |
| `crates/desktop-shell/src/worker.rs` | **New.** `--worker` host: handover, lifeline, socket, turn driver |
| `crates/desktop-shell/src/workers.rs` | **New.** Supervisor: spawn, socket client, stop escalation, listing, shutdown-all |
| `crates/desktop-shell/src/lib.rs` | Routes; session-delete hook; `turn_progress_event` arms; `spawn_with_shell_stack` (Task 2: request and turn threads get 8 MiB). A new worker or supervisor thread that runs a turn or `handle_connection` must use it |
| `crates/desktop-shell/src/main.rs`, `crates/desktop-app/src/main.rs` | `--worker` branch; `RunEvent::Exit` |
| `crates/desktop-shell/static/{app.js,style.css,index.html}` | Toggle, *Start worker*, dialog, sidebar, board, control strip |
| `AGENTS.md` | Task-prompt template fence `text` → `worker-prompt` |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md` | Task 11 |

## Interfaces

Names fixed here so that tasks run in separate sessions agree. A task that
needs to change one records the change in its Progress row and updates this
section.

```rust
// config.rs
pub enable_worker_sessions: bool, // default false; Forbidden in repository scope

// session.rs
pub enum TaskStatus { /* … */ Paused } // as_str "paused"; not terminal; no side effect in flight

pub struct WorkerRecord {
    pub parent_session_id: Option<String>,
    pub source_repository: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub base_commit: String,
    pub included_uncommitted: bool,
    pub git_commands: Vec<String>,
}
impl SessionStore {
    /// `worker_spawned` on the worker's own log.
    pub fn record_worker_spawned(&self, worker_session_id: &str, record: &WorkerRecord) -> Result<()>;
    /// `worker_spawned` on the parent's log, carrying `workerSessionId`.
    pub fn record_worker_child(&self, parent_session_id: &str, worker_session_id: &str) -> Result<()>;
    pub fn worker_record(&self, session_id: &str) -> Result<Option<WorkerRecord>>;
    pub fn worker_children(&self, parent_session_id: &str) -> Result<Vec<String>>;
    /// Every worker session whose record names `source_repository`, across all repository ids.
    pub fn worker_sessions_for_source(&self, source_repository: &Path) -> Result<Vec<Session>>;
    /// Added by Task 2, with the event it writes (deferred from Task 1).
    pub fn record_steering_note(&self, session_id: &str, message_id: &str) -> Result<()>;
}

// chat.rs
pub struct TurnSink<'a> { /* existing three fields */ pub steering: Option<&'a TurnSteering> }
pub enum TurnProgress { /* existing */ Paused, Resumed, NoteAdded { message_id: String } }
fn system_prompt_for_session(mode: SessionMode, offer_handoffs: bool) -> String;
pub const WORKER_HANDOFF_PARAGRAPH: &str; // proposal §5.0, verbatim

// steering.rs
pub struct TurnSteering; // Mutex<{ paused: bool, notes: VecDeque<String> }> + Condvar
impl TurnSteering {
    pub fn new() -> Self;
    pub fn pause(&self);
    pub fn resume(&self);
    pub fn queue_note(&self, text: String);
    pub fn is_paused(&self) -> bool;
    pub fn drain_notes(&self) -> Vec<String>;
    /// Blocks while paused. Returns false if `cancel` fired. Uses a 100 ms
    /// `wait_timeout` re-check, so a missed `wake` cannot hang a turn.
    pub fn wait_while_paused(&self, cancel: &CancelToken) -> bool;
    pub fn wake(&self);
}

// worker_progress.rs
pub enum ProgressKind { Status, Phase, Plan, Text, Tool, Approval, Note }
pub struct ProgressWriter; // open truncates; append assigns seq from 1; redacts every string in the payload
// each line: {"seq": u64, "timestampMs": u64, "kind": ProgressKind::as_str(), "payload": <redacted value>}
impl ProgressWriter {
    pub fn create(data_dir: &Path, session_id: &str, scanner: SecretScanner) -> Result<Self>;
    pub fn append(&mut self, kind: ProgressKind, payload: serde_json::Value) -> Result<u64>;
}
pub fn read_progress_after(data_dir: &Path, session_id: &str, after: u64) -> Result<Vec<serde_json::Value>>;
pub fn progress_path(data_dir: &Path, session_id: &str) -> PathBuf; // <data_dir>/sessions/<id>.progress.jsonl

// worktree.rs
pub fn worker_slug(prompt: &str) -> String;
pub struct SourceChanges { pub paths: Vec<String>, pub mentioned: Vec<String> } // mentioned ⊆ paths, listed first
pub fn source_changes(repo: &Path, prompt: &str) -> Result<SourceChanges>;
pub struct CreatedWorktree { pub slug: String, pub path: PathBuf, pub branch: String, pub base_commit: String, pub git_commands: Vec<String> }
pub fn create_worker_worktree(repo: &Path, prompt: &str, include_uncommitted: bool) -> Result<CreatedWorktree>;
pub enum RemoveOutcome { Removed, Dirty { paths: Vec<String> } }
pub fn remove_worker_worktree(repo: &Path, worktree: &Path, force_after_confirmation: bool) -> Result<RemoveOutcome>;
```

**Socket protocol** (Task 5 defines it, and Tasks 6 and 7 consume it). One JSON
object per line in each direction: request
`{"token": "…", "command": "status", …}`, response
`{"ok": true, …}` or `{"ok": false, "error": "…"}`. Commands and arguments
are exactly proposal §5.2's table.

**Shell routes** (Task 7). All use form bodies, per C7.

| Route | Form / query | Answers |
|---|---|---|
| `GET /api/workers-enabled` | — | `{"enabled": bool}` |
| `GET /api/worker-source-status` | `repo`, `prompt` | `{"paths": [...], "mentioned": [...]}` |
| `POST /api/workers` | `repo`, `prompt`, `include_uncommitted`, `parent_session_id?` | `{"sessionId", "title", "branch", "worktree"}` |
| `GET /api/workers` | `repo` | worker rows (Task 9's columns) |
| `GET /api/worker-progress` | `session`, `after` | `{"events": [...], "alive": bool}` |
| `POST /api/worker-control` | `session_id`, `command`, `text?`, `note?`, `proposal_id?` | the socket's response |
| `POST /api/worker-restart` | `session_id` | `{"status": "started"}` or a refusal |
| `POST /api/worker-remove-worktree` | `session_id`, `confirmed_dirty?` | `{"status": "removed"}` or `{"status": "dirty", "paths": [...]}` |

As built in Task 7 (`desktop-shell/src/workers.rs`, `route`): with
`enable_worker_sessions` off, every route answers 403 except
`GET /api/workers` and `GET /api/workers-enabled`, the route the UI reads
the setting from. A refusal is `{"error": "…"}` with 400 (bad input), 403
(off), 409 (the worker is not running, or a live worker holds the worktree)
or 500. `/api/worker-control` accepts `message`, `note`, `pause`, `resume`,
`approve`, `reject`, `status`, `stop` (the §5.3 escalation, answered after
its first exchanges) and `shutdown`. A `GET /api/workers` row's fields are
listed in proposal §5.7.

## Order and parallelism

```
1 ──┬──> 2 ──┐
3 ──┤        ├──> 5 ──> 6 ──> 7 ──> 8 ──> 9 ──> 10 ──> 11
4 ──┘────────┘
```

Tasks 1, 3 and 4 touch disjoint files and may run in parallel worktrees. Task
2 rewrites the `TurnSink` literals in many files, so run it alone, and **not
beside any other spec that touches `chat.rs`** (`docs/specs/README.md`,
"Parallel work"). Tasks 8–10 all edit `app.js` and run in sequence.

---

## Task 1: Setting, worker record, handoff instruction

**Requirements:** 1, 2 (the instruction half). **Proposal:** §5.0, §5.8.
**Corrections:** C1, C2. **Files:** `config.rs`, `profile.rs`, `session.rs`, `chat.rs`,
`AGENTS.md`, `tests/repository_config_trust.rs`, `tests/permission_profiles.rs`.

- [x] **Step 1: Write the failing tests**
  - `config.rs`: `worker_sessions_are_off_by_default`.
  - `repository_config_trust.rs`: `repository_config_cannot_enable_worker_sessions`.
    In repository scope the key is dropped and recorded as a forbidden key,
    and the user value stands.
  - `session.rs`: `a_worker_record_round_trips`,
    `a_session_without_a_worker_record_has_none`,
    `a_parent_lists_its_worker_children`, and
    `worker_sessions_are_found_by_source_repository_across_repository_ids`.
    The last one is two worktrees with different repository ids, both naming
    one source.
  - `chat.rs`: `the_handoff_paragraph_is_appended_when_offered` (present, and
    verbatim from proposal §5.0),
    `no_mode_changes_when_handoffs_are_not_offered`
    (`system_prompt_for_session(m, false) == system_prompt(m)` for every
    `SessionMode`), and two turn-level tests through `MockModelAdapter` that
    read `adapter.requests[0].messages[0]`:
    `a_non_worker_session_gets_the_paragraph_when_enabled` and
    `a_worker_session_never_gets_the_paragraph`.
- [x] **Step 2: Run them and confirm they fail**
- [x] **Step 3: Add the setting**, following `audit_enabled`'s touchpoints
      (File Structure). The exhaustive destructures at `config.rs:802` and
      `config.rs:2637`, and `classify_overlay_fields!` at `config.rs:2212`, force
      the classification, which is the guard working. The #31 kind is
      `Capability`: a key repository scope does not apply freely is a
      capability key (`config.rs:2178`). Add the key to the
      Forbidden row of
      [`../34_repository_config_trust_boundary.md`](../34_repository_config_trust_boundary.md)'s
      table.
- [x] **Step 4: Add `WorkerRecord` and the `SessionStore` methods.** They are
      thin public wrappers over the private `append_session_event`. Do not
      make that function public.
- [x] **Step 5: Add `system_prompt_for_session`** and switch `chat.rs:765` to
      it, with
      `offer_handoffs = config.enable_worker_sessions && worker_record(&session.id)?.is_none()`.
- [x] **Step 6: Falsify.** Make `system_prompt` itself append the paragraph and
      confirm `code_mode_system_prompt_is_byte_identical_to_todays` fails.
      Drop the `is_none()` term and confirm the worker test fails. Revert both.
- [x] **Step 7: `AGENTS.md`.** Change the task-prompt template's fence from
      `text` to `worker-prompt` (proposal §5.0, last paragraph).
- [x] **Step 8: Update the proposal** for C1 and C2.
- [x] **Step 9: Scoped checks.** Run
      `cargo nextest run -p workspace-engine -E 'test(worker) + test(system_prompt) + test(handoff)'`
      and `cargo nextest run -p workspace-engine --test repository_config_trust`,
      then `cargo fmt --all`, `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`
      and `typos`. Then show the change and ask.

## Task 2: `TurnSteering` and `TaskStatus::Paused`

**Requirements:** 6 (engine half). **Proposal:** §5.4. **Corrections:** C3,
C4, C5. **Files:** `steering.rs` (new), `chat.rs`, `session.rs`,
`recovery.rs`, every `TurnSink` literal,
`desktop-shell/src/lib.rs::turn_progress_event`.

- [x] **Step 1: Write the failing tests** in `crates/workspace-engine/tests/steering.rs` (new).
      Drive a `MockModelAdapter` script of three rounds. Pause, resume and
      queue from a second thread, and gate the timing on a `TurnProgress::Phase`
      callback, not on sleeps.
  - `pause_takes_effect_only_at_a_round_boundary`. The model call in flight
    completes, and the next one does not start while paused.
  - `a_paused_turn_persists_paused_then_running_again`. Read
    `task_status_updated` events.
  - `resume_continues_the_same_turn`. The same task id, and the round count
    continues.
  - `a_note_becomes_a_user_message_before_the_next_model_request`. Assert on
    `adapter.requests[n].messages`, plus a `steering_note_added` event naming
    that message id.
  - `cancel_while_paused_ends_cancelled`.
  - `a_turn_without_steering_is_unchanged`. With `steering: None`, the request
    sequence is identical to a run before this task.
  - In `steering.rs` unit tests: `wait_while_paused_returns_on_cancel_without_wake`.
- [x] **Step 2: Run to verify they fail**
- [x] **Step 3: Add `TaskStatus::Paused`.** The compiler names `as_str` and
      `tests/crash_recovery.rs:836`. Also update `all()` and the string lists
      in `tests/session_rewind.rs:435-495`. It is neither terminal nor
      side-effect-in-flight. Grep `app.js` for status strings and give
      `paused` a label wherever the others have one.
- [x] **Step 4: Recovery (C5).** Add
      `a_worker_session_is_never_auto_resumed` to `tests/crash_recovery.rs`. A
      `Paused` or `RunningTool` task in a session with a worker record is
      classified `Interrupted` and is not auto-resumed. Implement it in
      `recovery.rs`.
- [x] **Step 5: Add `steering: None` to all 36 `TurnSink` literals** (C4). Do
      this in its own step, so the diff for the behaviour change stays
      readable.
- [x] **Step 6: Implement** `TurnSteering` and the boundary step at
      `chat.rs:1690`, in proposal §5.4's order: cancelled → paused-wait →
      drain notes → running. Add the `TurnProgress` variants. In
      `desktop-shell`'s `turn_progress_event`, map them to no SSE event: the
      shell never steers.
- [x] **Step 7: Falsify.** Move the pause check after the model call and
      confirm the boundary test fails. Drop the drain and confirm the note test
      fails.
- [x] **Step 8: Update the proposal** for C3 (notes) and C5.
- [x] **Step 9: Scoped checks.** Run `cargo nextest run -p workspace-engine`
      (whole crate, because the literal sweep touches every test file),
      `cargo nextest run -p desktop-shell -p eval-harness`, then fmt, clippy on
      all three crates, and `node --check` if `app.js` changed. Then show and
      ask.

## Task 3: The progress file

**Requirements:** 5. **Proposal:** §5.5. **Corrections:** C3. **Files:**
`worker_progress.rs` (new), `session.rs` (`delete_session` also deletes the
progress file).

- [x] **Step 1: Write the failing tests**
  - `seq_strictly_increases_from_one`.
  - `creating_a_writer_truncates_the_previous_file`. A restarted worker starts
    again from 1.
  - `reading_after_n_returns_exactly_the_later_lines`, for `after = 0`, a
    middle value, the last value, and a value past the end.
  - `a_seeded_secret_never_reaches_the_file`. Seed a token shape that
    `SecretScanner` detects into a `text`, a `tool` label and an `approval`
    payload, then grep the raw file bytes.
  - `a_torn_last_line_is_skipped_not_fatal`. The reader runs while the writer
    appends, so a partial final line is expected.
  - `deleting_a_session_deletes_its_progress_file`.
- [x] **Step 2: Run to verify they fail**
- [x] **Step 3: Implement.** Use one `write_all` of a full line per append.
      `timestampMs` comes from the same clock helper the session log uses.
      Text batching (about 250 ms) belongs to the caller, Task 6, not to this
      writer.
- [x] **Step 4: Falsify.** Skip redaction for one kind and confirm the secret
      test fails.
- [x] **Step 5: Update the proposal** for C3 (progress).
- [x] **Step 6: Scoped checks, then show and ask**

## Task 4: Worktree creation and removal

**Requirements:** 3, 10. **Proposal:** §5.1 (Naming, Creation steps 1–6,
Removing). **Files:** `worktree.rs` (new), and `checkpoint.rs` if
`run_user_git` (`checkpoint.rs:1129` when planned; `:1134` after this task) is lifted to `pub(crate)`. Lift it, with
a doc comment saying that it now also runs mutating commands, rather than
writing a second git runner. Every command run is appended to
`CreatedWorktree.git_commands` exactly as it was executed.

- [x] **Step 1: Write the failing tests** in `crates/workspace-engine/tests/worktree.rs` (new).
      Each test uses a fresh `git init` fixture in a temp dir.
  - `worker_slug`: ASCII lowercase words joined by `-`, at most 40 characters,
    `worker` when empty, and non-ASCII dropped.
  - `a_worktree_is_created_under_damaian_worktrees_on_a_damaian_branch`.
  - `the_exclude_line_is_added_once`. Create two worktrees, then count lines in
    `.git/info/exclude`. `.gitignore` is untouched.
  - `colliding_names_get_matching_suffixes`. Both the branch and the path take
    `-2`, including when only the branch exists.
  - Refusals, each asserting **nothing changed**: no new branch, no exclude
    edit, and no directory:
    `refuses_a_symlink_anywhere_in_the_path` (`.damaian` itself is a symlink,
    and `.damaian/worktrees` is a symlink),
    `refuses_a_tracked_path_under_worktrees`, `refuses_a_non_git_source`, and
    `refuses_a_path_that_resolves_outside_the_repository`.
  - `include_carries_tracked_edits_and_untracked_files`, and ignored files stay
    behind. `start_from_last_commit_carries_neither`.
    `include_with_no_tracked_changes_bases_on_head` (`git stash create`
    printed nothing).
  - `the_source_tree_is_untouched_by_include`. Compare its working tree, index
    and stash list before and after.
  - `source_changes_lists_files_the_prompt_mentions_first`.
  - `removing_a_clean_worktree_keeps_its_branch`.
    `removing_a_dirty_worktree_reports_paths_and_does_not_force`, and only
    `force_after_confirmation` removes it.
- [x] **Step 2: Run to verify they fail**
- [x] **Step 3: Implement.** Run the checks in proposal §5.1's order, and run
      every refusal before the first mutation. Use `symlink_metadata` per
      component, not `canonicalize` alone, because `canonicalize` follows the
      link that the check exists to catch.
- [x] **Step 4: Falsify.** Replace the per-component check with
      `path.exists()` and confirm the symlink tests fail.
- [x] **Step 5: Scoped checks, then show and ask**

## Task 5: Worker process — handover, lifeline, control socket

**Requirements:** 8, 9, the process half of 10. **Proposal:** §5.2, §5.3
steps 1–3. **Corrections:** C6, C11. **Files:** `desktop-shell/src/worker.rs`
(new), both `main.rs`, `process_registry.rs` (`ProcessKind::Worker`).

This task runs **no turns**. It builds the host, and Task 6 puts the turn in it.

- [x] **Step 1: Check the socket path length first.** macOS caps `sun_path` at
      104 bytes. `<data_dir>/workers/<session-id>.sock` under
      `~/Library/Application Support/…`, or under a test temp dir in
      `/var/folders/…`, may exceed it. Measure both, and record the numbers in
      the Progress row. If either exceeds the cap, name the socket by a short
      hash of the session id and refuse with a clear error when even that does
      not fit. Then update proposal §5.2.
- [x] **Step 2: Write the failing tests**
  - Unit: `the_handover_line_parses_and_rejects_extra_fields`,
    `a_wrong_or_missing_token_closes_the_connection`,
    `an_unknown_command_is_rejected`,
    `the_socket_is_0600_in_a_0700_directory`, and
    `status_answers_with_the_current_snapshot`. Run the socket server on a
    thread against a temp data dir.
  - Re-exec tests (pattern: `tests/process_registry.rs:133`). These spawn a
    real process, so they are `#[ignore]`d with the manual command:
    `closing_stdin_exits_the_worker_and_kills_its_sleeping_child`, and
    `a_command_the_worker_runs_sees_neither_key_nor_token`, which reads
    `env` from a child and the worker's argv from `ps -o args= -p`.
    `worker_mode_refuses_a_terminal_stdin` is a unit test on the check
    function, not a real TTY.
- [x] **Step 3: Run to verify they fail**
- [x] **Step 4: Implement `run_worker_from_args(args) -> i32`** in `worker.rs`:
  - Parse `--worker --session <id> --data-dir <dir>`.
  - Refuse when `stdin().is_terminal()`.
  - Read one handover line: `{"apiKey", "token"}`, with unknown fields
    refused.
  - Start the lifeline thread, which blocks on stdin and treats EOF as
    shutdown.
  - Install #46's `install_shutdown_handler`.
  - Bind the socket and serve a single-connection-at-a-time loop.
  - On shutdown: cancel, then `sweep_own`, then `exit`.
  - Load the worker's config from `--data-dir`, never the default (C11).
- [x] **Step 5: Wire both `main`s** (C11). In `desktop-shell/src/main.rs`,
      before `install_shutdown_handler`, and in `desktop-app/src/main.rs`,
      at the top of `main()` before `tauri::Builder`: if `args[1] == "--worker"`,
      `std::process::exit(desktop_shell::run_worker_from_args(args))`.
- [x] **Step 6: Add `ProcessKind::Worker`** (C6). It is registered by the
      **shell** in Task 7, not here. Update proposal §5.2's Registration
      paragraph.
- [x] **Step 7: Falsify.** Remove the lifeline thread and confirm the
      stdin-close test fails (run it with `--ignored`).
- [x] **Step 8: Scoped checks**, including the ignored tests run once by
      hand. Record their output in the Progress row. Then show and ask.

## Task 6: Worker turns — steering, approvals, same-turn guarantee

**Requirements:** 4, 6, 7. **Proposal:** §5.2 "Same turn", §5.4, §5.6.
**Files:** `worker.rs`; `desktop-shell/src/lib.rs` (tests only, plus any
shared engine-construction helper factored out of `run_chat_request`);
`desktop-shell/tests/worker_process.rs` (the re-exec tests).

- [x] **Step 1: Factor, don't copy.** The worker builds its engine and adapter
      exactly as `run_chat_request` (`lib.rs:1825` after this task; `:1534`
      when planned) does: same `configure_chat_integrations`,
      `CurlModelTransport::new(.., ProcessRegistry::open(&data_dir), ..)`, and
      the same adapter. The key comes from the handover instead of
      `resolve_model_api_key`. Extract the shared part into one function that
      both call. As built: `chat_engine(config)` and
      `chat_model_adapter(config, api_key)` (`lib.rs:1897`, `:1909`).
      `engine_for_repo_with_model_options` became
      `config_for_repo_with_model_options`, because the engine is now built by
      `chat_engine`.
- [x] **Step 2: Write the failing tests** in `lib.rs`'s test module, using
      `DAMAIAN_MOCK_MODEL_RESPONSE` and an in-process worker driver: the turn
      loop of `worker.rs` called on a thread, not a spawned binary.
  - **`a_turn_through_the_worker_host_matches_the_shell_host`** (requirement
    4). One scripted turn through `run_chat_request` and through the worker
    driver should produce the same sequence of audit event types and the same
    session-log event types, ignoring ids and timestamps. This is the
    load-bearing test of the spec.
  - `progress_lines_mirror_the_turn`: `phase` carries `round`/`maxRounds`;
    `tool` carries the tool label; `text` is batched; `status` is written on
    each transition.
  - `a_note_while_waiting_for_approval_is_refused`.
  - `message_is_refused_while_a_turn_runs`.
  - `a_command_approval_continues_in_process`. The snapshot
    `chat/pending/<id>.json` exists while waiting and is gone after
    `approve`. There is no second process and the same PID.
  - `a_patch_proposal_leaves_the_worker_idle_and_alive`, and
    `/api/apply-patch` with `repo = worktree` applies it.
  - `#[ignore]`d re-exec test:
    `killing_a_worker_waiting_for_approval_leaves_a_snapshot_recovery_finds`.
  - `#[ignore]`d re-exec test, moved here from Task 5:
    `a_command_the_worker_runs_sees_neither_key_nor_token`. Have a worker turn
    run `env` (through `DAMAIAN_MOCK_MODEL_RESPONSE`) and assert that neither
    value is in its output. Task 5 had no command to run, and macOS shows no
    other process's environment (`ps -E` prints none), so this is the first
    task that can check the environment at all.
- [x] **Step 3: Run to verify they fail**
- [x] **Step 4: Implement** the turn driver:
  - It holds one `TurnSteering` and one `CancelToken`. Socket commands
    `pause`, `resume`, `note` and `stop` drive them, and `stop` also calls
    `wake`.
  - The `TurnSink` writes to a `ProgressWriter`, with text batched about every
    250 ms.
  - On `WaitingForApproval` it emits `approval` and waits for
    `approve`/`reject`, then calls `resume_after_command_decision` or
    `resume_after_plan_decision` with the same sink.
  - On a patch proposal or completion it goes idle until `message`.
- [x] **Step 5: Falsify.** Make the worker driver skip one audit-writing step,
      for example by bypassing `configure_chat_integrations`, and confirm
      the same-turn test fails. (Bypassing it does **not** fail the same-turn
      test; see the Progress row. `a_worker_turn_gets_the_shells_chat_integrations`
      was added to catch it.)
- [x] **Step 6: Run the deterministic eval tier.** Turn hosting changed:
      `cargo run -p eval-harness -- run --tier deterministic`.
- [x] **Step 7: Scoped checks, then show and ask**

## Task 7: Shell supervisor — spawn, lifetime, routes

**Requirements:** 2 (creation), 5, 6, 8, 10. **Proposal:** §5.1 steps 7–9 and
Approval, §5.3 steps 4–6 and Stop escalation, §5.7 Shell routes.
**Corrections:** C5, C6, C7, C11. **Files:** `desktop-shell/src/workers.rs`
(new), `lib.rs` (routes, session-delete), `desktop-app/src/main.rs`.

- [x] **Step 1: Write the failing tests**, with a fake worker. Point the
      supervisor's executable path at the test binary re-entering a stub
      `--worker` that speaks the socket protocol. Tests that start real
      processes are `#[ignore]`d. As built: the stub is a shell script that
      holds stdin and records argv, env and the handover's shape. Where the
      protocol matters, the test serves the worker's socket itself. The
      real-binary tests are `#[ignore]`d in `tests/workers.rs`; see the
      Progress row.
  - `spawn_records_worker_spawned_on_worker_parent_and_audit`. All three
    records carry the exact `git_commands`.
  - `spawn_hands_over_the_cached_key_on_stdin_only`. The child's argv has no
    key and the child's env equals the shell's env.
  - `the_shell_registers_the_worker_as_process_kind_worker` (C6).
  - `deleting_a_parent_sends_shutdown_to_its_workers_and_keeps_their_worktrees`.
  - `restart_is_refused_while_a_live_worker_holds_the_worktree`.
  - `stop_escalates_to_shutdown_then_sigterm_by_registry_pid`. Use a stub
    worker that ignores `stop`, and inject timeouts, rather than waiting 10 + 5
    seconds.
  - `remove_worktree_requires_the_second_confirmation_when_dirty`.
  - Route tests: `worker_control_takes_form_fields` (C7),
    `workers_list_groups_by_source_repository`, and
    `worker_progress_pages_with_after`.
  - `every_worker_route_refuses_when_the_setting_is_off`, except `GET
    /api/workers`, which still lists running workers (proposal §5.0:
    turning it off does not stop them).
- [x] **Step 2: Run to verify they fail**
- [x] **Step 3: Implement `workers.rs`.** Spawn in proposal §5.1's order, using
      `current_exe()` with `process_group(0)` and piped stdin. Generate the
      token from `/dev/urandom` (32 bytes, hex). The handover line is written
      once, and stdin is then **held open in the supervisor for the worker's
      lifetime**. Dropping it is the lifeline signal, so store it beside the
      registration handle. Add a socket client with short timeouts, the stop
      escalation, and `shutdown_all(data_dir)`.
- [x] **Step 4: Routes**, per the Interfaces table. The board row's
      files-changed count and commits-ahead-of-base come from one
      `git -C <worktree>` call each, for finished rows only.
- [x] **Step 5: Lifetime hooks.** `/api/session-delete` calls `shutdown` on the
      deleted session's `worker_children`. `desktop-app` switches to
      `.build(ctx)?.run(..)` and calls `shutdown_all` on `RunEvent::Exit`
      (C11): wait up to 5 s, then `SIGTERM` by registry PID. The standalone
      shell's existing `install_shutdown_handler` already reaches workers
      through `sweep_own` once they are registered as owned by the shell.
      Confirm that with a test, don't assume it.
- [x] **Step 6: Update the proposal** for C6, C7 and C11.
- [x] **Step 7: Scoped checks**, plus the ignored tests by hand, then show and
      ask.

## Task 8: UI — setting, *Start worker*, New worker dialog

**Requirements:** 1, 2, 3. **Proposal:** §5.1 Entry points, Dialog, dirty
warning; §5.7 Parent session. **Corrections:** C8, C9. **Files:** `app.js`,
`style.css`, `index.html`.

- [x] **Step 1: The toggle** (C9). Add a "Worker sessions" `.field-checkbox`
      with its one-line description on the General settings page. It reads
      `GET /api/workers-enabled` and writes through `upsertConfigValue` +
      `saveConfigFile`, following the MCP form's pattern.
- [x] **Step 2: *Start worker*** (C8). Attach it after `finalizeChatMessage`,
      **assistant messages only**, on `code` elements whose `className ===
      "language-worker-prompt"`, and only when enabled and the current session
      is not a worker. Add a footer row with a `.btn-sm` button. Attach
      nothing during streaming (`updateChatMessage`). History goes through
      `finalizeChatMessage` too, so it gets the button for free. Confirm that
      rather than assuming it.
- [x] **Step 3: The dialog.** "New worker" has one multi-line field
      (editable), *Cancel* (`.btn-quiet`) and *Start worker*
      (`.btn-primary`). It calls `GET /api/worker-source-status`. When the tree
      is dirty, show the warning block with *Include* pre-selected, mentioned
      files first. On confirm, call `POST /api/workers` with the parent. Build
      it from `showAppDialog`'s structure, as `showRewindDialog` does.
- [x] **Step 4: Parent links.** A used block's footer shows "Started
      *‹worker title›* →". The parent conversation gets the one-line quiet
      "Spawned worker *‹title›* →" entry, rebuilt from the parent's
      `worker_spawned` events on reload. As built: rebuilt from the
      `GET /api/workers` rows, because no route returns the parent's events;
      see proposal §5.7 Parent session and the Progress row.
- [x] **Step 5: Verify in the browser** (Global Constraints, UI verification).
      Drive both renderers from the console: `renderMarkdown` and the
      `/api/render-markdown` → `finalizeChatMessage` path. *Start worker*
      appears on a `worker-prompt` block in an assistant message and on
      nothing else:
      - not on untagged, `text`, `prompt`, `rust`, or `worker-prompt extra`
        blocks;
      - not on a `worker-prompt` block in a user message;
      - not while streaming;
      - not in a worker session;
      - not with the setting off.

      Also check the exact pre-fill text, the dialog's clean and dirty states,
      and the "Started" link. Record what was driven in the Progress row.
- [x] **Step 6: `node --check`, `npm run lint:web`, then show and ask**

## Task 9: UI — sidebar nesting and the Workers board

**Requirements:** 5, 6. **Proposal:** §5.7 Sidebar, Workers board.
**Corrections:** C10. **Files:** `app.js`, `style.css`.

- [x] **Step 1: Sidebar.** `renderProjectSession` (`app.js:7308` after Task 9, `6538` after Task 8; `5395` when planned) nests
      workers under their parent and shows a status dot (running, needs you,
      paused, finished, failed). Parentless workers are listed with a dot.
      A **Workers** entry with a needs-you badge sits at the top of each
      project. Workers come from `GET /api/workers?repo=` (by source
      repository), not from `/api/sessions`.
- [x] **Step 2: The board.**
  - Columns and groups are exactly as in proposal §5.7, newest first.
  - Each row has one visible action: *Review* opens the worker at its
    approval card, *Pause* while running, *Open* otherwise.
  - The `⋯` overflow holds *Stop*, *Reveal worktree in Finder*, *Remove
    worktree* (finished only, with the dirty confirmation naming paths) and
    *Dismiss from board*.
  - Finished rows show files changed and commits ahead.
  - The board polls about once a second while visible, and stops when hidden.
- [x] **Step 3: C10.** Make the board tolerate a poll that hangs for the
      length of another session's turn: one request in flight at a time, no
      pile-up, and no error state for a slow answer.
- [x] **Step 4: Verify in the browser.** Seed workers in each state with stub
      progress files and session records, and check groups, ordering, the
      overflow and the badge count. Record what was checked.
- [x] **Step 5: `node --check`, `npm run lint:web`, then show and ask**

## Task 10: UI — worker session control strip and composer

**Requirements:** 6, 7. **Proposal:** §5.7 Worker session. **Files:**
`app.js`, `style.css`.

- [x] **Step 1: The control strip** sits under the header:
  - It shows a status pill and `Round r/max` with a bar.
  - Its controls depend on state: *Pause* and *Stop* while running, *Resume*
    and *Stop* while paused, *Stop* while waiting.
  - *Stop* is `.btn-danger`, and *Resume* is the strip's only `.btn-primary`.
  - The header shows "from *‹parent›*" and the branch.
- [x] **Step 2: The composer** sends `note` while running, `resume` with a note
      while paused, and `message` when idle. Its button label says which one.
      It is disabled while waiting for approval, since notes are refused then.
      Approval cards post `approve`/`reject` through `/api/worker-control`
      instead of the shell's resume routes.
- [x] **Step 3: Live progress** comes from polling
      `GET /api/worker-progress?session=&after=` about once a second, and
      history comes from the session log as today. When the process has ended
      unfinished: show *Restart worker* and disable the composer.
- [x] **Step 4: Verify in the browser** against a real worker on a non-4765
      port. `DAMAIAN_MOCK_MODEL_RESPONSE` answers every call at once with
      plain text, so notes, pause and approvals need a local scripted
      OpenAI-compatible endpoint that answers after a delay (see the Progress
      row). Cover:
      - the strip in each state;
      - a note while running and while paused, with the note appearing as a
        user message before the next round;
      - an approval continuing without a restart, with the same worker PID
        before and after;
      - Stop.

      Record the PIDs observed.
- [x] **Step 5: `node --check`, `npm run lint:web`, then show and ask**

## Task 11: Kill test, measurement, docs, close the spec

A fresh session that reads the whole proposal, this file and the final code.
It evaluates; it does not inherit.

- [x] **Step 1: The ignored end-to-end kill test** (proposal §6). Spawn a
      shell process with a worker running a long command, `SIGKILL` the shell,
      and assert the worker and its child are gone within 5 seconds. Include a
      doc comment with the exact command, run it, and record the output.
- [x] **Step 2: Cold-start measurement.** Measure the worker's time from spawn
      to its first model request on this repository. It is informational,
      recorded here and in proposal §6, and is not a gate.
- [x] **Step 3: Docs.**
  - `docs/USER_GUIDE.md`: turning workers on, *Start worker*, the dialog's
    include choice, the board, the strip, notes, approvals, restart, removing
    a worktree, and that branches are never deleted.
  - `docs/TROUBLESHOOTING.md`:
    - the board stalls while another session streams (C10);
    - a worker shows *Interrupted*, and what *Restart worker* does;
    - where a worker's files are (`.damaian/worktrees/`, the progress file,
      the socket);
    - leftover worktrees after a crash.
  - `SECURITY.md`: the stdin key handover, the socket token and its
    permissions, and the `.damaian/worktrees` checks.
- [x] **Step 4: Proposal §7.** Add the C10 follow-up: serve worker routes while
      a turn streams.
- [x] **Step 5: Walk proposal §6** and name the test or the recorded manual
      check that covers each item. An item with neither is not met.
- [x] **Step 6: Acceptance.** In a fixture repository with **no** `AGENTS.md`
      and the setting on, a session asked to split work yields `worker-prompt`
      blocks, and *Start worker* → confirm starts one without copying. With
      the setting off, nothing of this spec is visible or sent. This needs a
      live model, so record whether it was run and with which provider, or
      that it could not be.
- [x] **Step 7: Close out**, per `AGENTS.md` "When a spec becomes Done":
      - update the proposal's `Status:`, its `docs/specs/README.md` row, this
        Progress table and the `**Done:**` header;
      - grep for `57_worker_sessions` in `Depends on:` lines;
      - re-derive "What to build next";
      - remove #57 from `CHANGELOG.md` `Unreleased`;
      - run `npm run specs:check`.
- [x] **Step 8: Full quality gate.** Run all seven commands from `AGENTS.md`
      and report the test count. Overridden by `AGENTS.md` since 2026-10-15:
      fast checks and scoped checks locally, and the full gate on CI.
- [x] **Step 9: Show the change and the gate result, and ask before committing**
