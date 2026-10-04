# Worker Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Started:** not yet (planned 2026-10-04)

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Setting, worker record, handoff instruction | Not started | |
| 2 · `TurnSteering` and `TaskStatus::Paused` | Not started | |
| 3 · The progress file | Not started | |
| 4 · Worktree creation and removal | Not started | |
| 5 · Worker process: handover, lifeline, control socket | Not started | |
| 6 · Worker turns: steering, approvals, same-turn guarantee | Not started | |
| 7 · Shell supervisor: spawn, lifetime, routes | Not started | |
| 8 · UI: setting, *Start worker*, New worker dialog | Not started | |
| 9 · UI: sidebar nesting and the Workers board | Not started | |
| 10 · UI: worker session control strip and composer | Not started | |
| 11 · Kill test, measurement, docs, close the spec | Not started | |

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
  `code_mode_system_prompt_is_byte_identical_to_todays` (`chat.rs:5360`)
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
  `crates/workspace-engine/tests/process_registry.rs:125-224` is the
  re-exec pattern to copy.
- **Falsify every load-bearing test.** Break what it guards and confirm it
  fails, then record that in the Progress row. #49's Task 5 found a test that
  passed against a broken implementation.
- **UI verification** runs the shell on a port other than 4765, with its own
  `DAMAIAN_DATA_DIR`. Static assets are `include_str!`-embedded, so rebuild and
  restart before checking. Stop the instance by its PID.
- **Scoped checks per task.** The full seven-command gate from `AGENTS.md`
  runs once, in Task 11. It takes `cargo nextest run --workspace` about 5
  minutes locally.
- **Never `git commit` unasked.** Each task ends by showing the change and the
  scoped check results, then asking. Use one subject line, with no body.

## Corrections to the proposal found while planning

Each one names the task that owns it.

| # | Proposal says | Code says | Resolution |
|---|---|---|---|
| C1 | `system_prompt` appends the paragraph (§5.0) | `fn system_prompt(mode: SessionMode) -> String` (`chat.rs:3439`) knows only the mode; its one caller is `chat.rs:760` | Keep `system_prompt(mode)` unchanged. Add `system_prompt_for_session(mode, offer_handoffs: bool)`, called at `chat.rs:760`. **Task 1** |
| C2 | Events are appended to the session (§5.1 step 7) | `append_session_event` (`session.rs:1906`) is private. Sessions are keyed by repository id, and `create_session(repository_id, title)` takes no path | Add typed public methods on `SessionStore` (Interfaces, Task 1). The worktree path lives only in the `worker_spawned` record. **Task 1** |
| C3 | Notes and progress pass "the same redaction as the session log" (§5.4, §5.5) | The session log is deliberately unredacted on write (`session.rs:1648`). Redaction happens at the call sites (`SecretScanner::redact`) | Notes are redacted where user messages are, i.e. on the way to the model. Every progress-file value goes through `SecretScanner::redact` before it is written. **Tasks 2, 3** |
| C4 | Adding `steering` to `TurnSink` (§5.4) | There are 32 `TurnSink { .. }` literals: 6 in `chat.rs`, 5 in `desktop-shell/src/lib.rs`, 1 in `eval-harness/src/runner.rs` and 20 under `workspace-engine/tests/` | Mechanical: `steering: None` at each. The compiler names them all. **Task 2** |
| C5 | `Paused` is recoverable via #17 (§5.4) | `recovery.rs::classify_session` treats any non-terminal status except `WaitingForApproval` as `Interrupted`, and may **auto-resume** it in the shell | Correct for classification. But a worker session must never auto-resume in the shell: its repository is a worktree and its host is gone. Recovery skips auto-resume for any session with a worker record, and the UI offers *Restart worker* instead. **Tasks 2, 7** |
| C6 | "The worker registers its PID … tagged with its session id" (§5.2) | `ProcessRegistry` records the **owner** as the registering process (`process_registry.rs:99`). `ProcessKind` has no `Worker` variant | The **shell** registers the worker child (`ProcessKind::Worker`, the worker's session id, owner = shell), so the launch sweep kills workers of a crashed shell. Each worker registers its own children (owner = worker), so `sweep_own` inside the worker kills exactly its children. **Tasks 5, 7** |
| C7 | `/api/worker-control` takes `{sessionId, command, ...}` (§5.7) | Every shell route takes url-encoded form fields (`parse_form`, `lib.rs:2741`) | Form fields: `session_id`, `command`, plus `text`, `note` and `proposal_id` as needed. Only the **socket** speaks JSON lines. **Task 7** |
| C8 | The tag rule is "checked by driving `renderMarkdown`" (§6) | A finished message is re-rendered **server-side** (`finalizeChatMessage`, `app.js:3123` → `render.rs:376`), which writes the trimmed info string verbatim into `class="language-…"`. `renderMarkdown` strips non-`[a-z0-9_-]` characters | Attach the action after `finalizeChatMessage`. Match `code.className === "language-worker-prompt"` exactly, which rejects `worker-prompt extra`. Verify through both renderers. **Task 8** |
| C9 | "Worker sessions" shown in Settings (§5.0) | Settings has no bool toggle. General is a raw config textarea. `/api/config` returns policy text only | A `.field-checkbox` on the General page, written through `upsertConfigValue` + `saveConfigFile` (the MCP form's pattern, `app.js:2295`). `GET /api/workers-enabled` reads the effective value. **Task 8** |
| C10 | The board polls about once a second (§5.7) | The accept loop (`lib.rs:112`) is blocked for the whole of an `/api/ask-stream` turn | **Accepted, decided 2026-10-04.** While any session in the window is streaming, the board and progress polls stall until the turn ends. Workers keep running, and only the view lags. Document this in proposal §5.7 and `TROUBLESHOOTING.md`, and name "serve worker routes during a stream" as a follow-up in §7. **Tasks 9, 11** |
| C11 | `RunEvent::Exit` hook (§5.3.4) | `desktop-app/src/main.rs:122` ends in `.run(ctx)`, and the shell runs **in-process** on a thread (line 84). `current_exe()` is the Tauri binary | Switch to `.build(ctx)?.run(\|app, event\| …)`. The `--worker` branch goes at the top of **both** `desktop-app` `main()` (line 36) and `desktop-shell` `main()`, before `install_shutdown_handler` (`desktop-shell/src/main.rs:4`). **Tasks 5, 7** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/config.rs` | `enable_worker_sessions` (copy `audit_enabled`'s eight touchpoints: field 184, default 1601, overlay 1653, `set` 1807, destructure 625, `scoped` 684, both `to_policy_text`s) |
| `crates/workspace-engine/src/session.rs` | `TaskStatus::Paused`; `WorkerRecord`; worker and steering-note event methods |
| `crates/workspace-engine/src/chat.rs` | `TurnSink.steering`; `system_prompt_for_session`; the round-boundary pause/note step; `TurnProgress` variants |
| `crates/workspace-engine/src/steering.rs` | **New.** `TurnSteering` |
| `crates/workspace-engine/src/worker_progress.rs` | **New.** Progress file writer and reader |
| `crates/workspace-engine/src/worktree.rs` | **New.** Slugs, checks, creation, untracked copy, removal, dirty summary |
| `crates/workspace-engine/src/process_registry.rs` | `ProcessKind::Worker` |
| `crates/workspace-engine/src/recovery.rs` | No auto-resume for worker sessions |
| `crates/desktop-shell/src/worker.rs` | **New.** `--worker` host: handover, lifeline, socket, turn driver |
| `crates/desktop-shell/src/workers.rs` | **New.** Supervisor: spawn, socket client, stop escalation, listing, shutdown-all |
| `crates/desktop-shell/src/lib.rs` | Routes; session-delete hook; `turn_progress_event` arms |
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
pub struct ProgressWriter; // open truncates; append assigns seq from 1; redacts every value
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
**Corrections:** C1, C2. **Files:** `config.rs`, `session.rs`, `chat.rs`,
`AGENTS.md`, `tests/repository_config_trust.rs`.

- [ ] **Step 1: Write the failing tests**
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
- [ ] **Step 2: Run them and confirm they fail**
- [ ] **Step 3: Add the setting**, following `audit_enabled`'s touchpoints
      (File Structure). The exhaustive destructure at `config.rs:605` forces
      the classification, which is the guard working. Add the key to the
      Forbidden row of
      [`../34_repository_config_trust_boundary.md`](../34_repository_config_trust_boundary.md)'s
      table.
- [ ] **Step 4: Add `WorkerRecord` and the `SessionStore` methods.** They are
      thin public wrappers over the private `append_session_event`. Do not
      make that function public.
- [ ] **Step 5: Add `system_prompt_for_session`** and switch `chat.rs:760` to
      it, with
      `offer_handoffs = config.enable_worker_sessions && worker_record(&session.id)?.is_none()`.
- [ ] **Step 6: Falsify.** Make `system_prompt` itself append the paragraph and
      confirm `code_mode_system_prompt_is_byte_identical_to_todays` fails.
      Drop the `is_none()` term and confirm the worker test fails. Revert both.
- [ ] **Step 7: `AGENTS.md`.** Change the task-prompt template's fence from
      `text` to `worker-prompt` (proposal §5.0, last paragraph).
- [ ] **Step 8: Update the proposal** for C1 and C2.
- [ ] **Step 9: Scoped checks.** Run
      `cargo nextest run -p workspace-engine -E 'test(worker) + test(system_prompt) + test(handoff)'`
      and `cargo nextest run -p workspace-engine --test repository_config_trust`,
      then `cargo fmt --all`, `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`
      and `typos`. Then show the change and ask.

## Task 2: `TurnSteering` and `TaskStatus::Paused`

**Requirements:** 6 (engine half). **Proposal:** §5.4. **Corrections:** C3,
C4, C5. **Files:** `steering.rs` (new), `chat.rs`, `session.rs`,
`recovery.rs`, every `TurnSink` literal,
`desktop-shell/src/lib.rs::turn_progress_event`.

- [ ] **Step 1: Write the failing tests** in `crates/workspace-engine/tests/steering.rs` (new).
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
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Add `TaskStatus::Paused`.** The compiler names `as_str` and
      `tests/crash_recovery.rs:834`. Also update `all()` and the string lists
      in `tests/session_rewind.rs:434-494`. It is neither terminal nor
      side-effect-in-flight. Grep `app.js` for status strings and give
      `paused` a label wherever the others have one.
- [ ] **Step 4: Recovery (C5).** Add
      `a_worker_session_is_never_auto_resumed` to `tests/crash_recovery.rs`. A
      `Paused` or `RunningTool` task in a session with a worker record is
      classified `Interrupted` and is not auto-resumed. Implement it in
      `recovery.rs`.
- [ ] **Step 5: Add `steering: None` to all 32 `TurnSink` literals** (C4). Do
      this in its own step, so the diff for the behaviour change stays
      readable.
- [ ] **Step 6: Implement** `TurnSteering` and the boundary step at
      `chat.rs:1570`, in proposal §5.4's order: cancelled → paused-wait →
      drain notes → running. Add the `TurnProgress` variants. In
      `desktop-shell`'s `turn_progress_event`, map them to no SSE event: the
      shell never steers.
- [ ] **Step 7: Falsify.** Move the pause check after the model call and
      confirm the boundary test fails. Drop the drain and confirm the note test
      fails.
- [ ] **Step 8: Update the proposal** for C3 (notes) and C5.
- [ ] **Step 9: Scoped checks.** Run `cargo nextest run -p workspace-engine`
      (whole crate, because the literal sweep touches every test file),
      `cargo nextest run -p desktop-shell -p eval-harness`, then fmt, clippy on
      all three crates, and `node --check` if `app.js` changed. Then show and
      ask.

## Task 3: The progress file

**Requirements:** 5. **Proposal:** §5.5. **Corrections:** C3. **Files:**
`worker_progress.rs` (new), `session.rs` (`delete_session` also deletes the
progress file).

- [ ] **Step 1: Write the failing tests**
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
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** Use one `write_all` of a full line per append.
      `timestampMs` comes from the same clock helper the session log uses.
      Text batching (about 250 ms) belongs to the caller, Task 6, not to this
      writer.
- [ ] **Step 4: Falsify.** Skip redaction for one kind and confirm the secret
      test fails.
- [ ] **Step 5: Update the proposal** for C3 (progress).
- [ ] **Step 6: Scoped checks, then show and ask**

## Task 4: Worktree creation and removal

**Requirements:** 3, 10. **Proposal:** §5.1 (Naming, Creation steps 1–6,
Removing). **Files:** `worktree.rs` (new), and `checkpoint.rs` if
`run_user_git` (`checkpoint.rs:1129`) is lifted to `pub(crate)`. Lift it, with
a doc comment saying that it now also runs mutating commands, rather than
writing a second git runner. Every command run is appended to
`CreatedWorktree.git_commands` exactly as it was executed.

- [ ] **Step 1: Write the failing tests** in `crates/workspace-engine/tests/worktree.rs` (new).
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
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** Run the checks in proposal §5.1's order, and run
      every refusal before the first mutation. Use `symlink_metadata` per
      component, not `canonicalize` alone, because `canonicalize` follows the
      link that the check exists to catch.
- [ ] **Step 4: Falsify.** Replace the per-component check with
      `path.exists()` and confirm the symlink tests fail.
- [ ] **Step 5: Scoped checks, then show and ask**

## Task 5: Worker process — handover, lifeline, control socket

**Requirements:** 8, 9, the process half of 10. **Proposal:** §5.2, §5.3
steps 1–3. **Corrections:** C6, C11. **Files:** `desktop-shell/src/worker.rs`
(new), both `main.rs`, `process_registry.rs` (`ProcessKind::Worker`).

This task runs **no turns**. It builds the host, and Task 6 puts the turn in it.

- [ ] **Step 1: Check the socket path length first.** macOS caps `sun_path` at
      104 bytes. `<data_dir>/workers/<session-id>.sock` under
      `~/Library/Application Support/…`, or under a test temp dir in
      `/var/folders/…`, may exceed it. Measure both, and record the numbers in
      the Progress row. If either exceeds the cap, name the socket by a short
      hash of the session id and refuse with a clear error when even that does
      not fit. Then update proposal §5.2.
- [ ] **Step 2: Write the failing tests**
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
- [ ] **Step 3: Run to verify they fail**
- [ ] **Step 4: Implement `run_worker_from_args(args) -> i32`** in `worker.rs`:
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
- [ ] **Step 5: Wire both `main`s** (C11). In `desktop-shell/src/main.rs`,
      before `install_shutdown_handler`, and in `desktop-app/src/main.rs`,
      at the top of `main()` before `tauri::Builder`: if `args[1] == "--worker"`,
      `std::process::exit(desktop_shell::run_worker_from_args(args))`.
- [ ] **Step 6: Add `ProcessKind::Worker`** (C6). It is registered by the
      **shell** in Task 7, not here. Update proposal §5.2's Registration
      paragraph.
- [ ] **Step 7: Falsify.** Remove the lifeline thread and confirm the
      stdin-close test fails (run it with `--ignored`).
- [ ] **Step 8: Scoped checks**, including the ignored tests run once by
      hand. Record their output in the Progress row. Then show and ask.

## Task 6: Worker turns — steering, approvals, same-turn guarantee

**Requirements:** 4, 6, 7. **Proposal:** §5.2 "Same turn", §5.4, §5.6.
**Files:** `worker.rs`; `desktop-shell/src/lib.rs` (tests only, plus any
shared engine-construction helper factored out of `run_chat_request`).

- [ ] **Step 1: Factor, don't copy.** The worker builds its engine and adapter
      exactly as `run_chat_request` (`lib.rs:1514`) does: same
      `engine_for_repo_with_model_options`, `configure_chat_integrations`,
      `CurlModelTransport::new(.., ProcessRegistry::open(&data_dir), ..)`, and
      the same adapter. The key comes from the handover instead of
      `resolve_model_api_key`. Extract the shared part into one function that
      both call.
- [ ] **Step 2: Write the failing tests** in `lib.rs`'s test module, using
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
- [ ] **Step 3: Run to verify they fail**
- [ ] **Step 4: Implement** the turn driver:
  - It holds one `TurnSteering` and one `CancelToken`. Socket commands
    `pause`, `resume`, `note` and `stop` drive them, and `stop` also calls
    `wake`.
  - The `TurnSink` writes to a `ProgressWriter`, with text batched about every
    250 ms.
  - On `WaitingForApproval` it emits `approval` and waits for
    `approve`/`reject`, then calls `resume_after_command_decision` or
    `resume_after_plan_decision` with the same sink.
  - On a patch proposal or completion it goes idle until `message`.
- [ ] **Step 5: Falsify.** Make the worker driver skip one audit-writing step,
      for example by bypassing `configure_chat_integrations`, and confirm
      the same-turn test fails.
- [ ] **Step 6: Run the deterministic eval tier.** Turn hosting changed:
      `cargo run -p eval-harness -- run --tier deterministic`.
- [ ] **Step 7: Scoped checks, then show and ask**

## Task 7: Shell supervisor — spawn, lifetime, routes

**Requirements:** 2 (creation), 5, 6, 8, 10. **Proposal:** §5.1 steps 7–9 and
Approval, §5.3 steps 4–6 and Stop escalation, §5.7 Shell routes.
**Corrections:** C5, C6, C7, C11. **Files:** `desktop-shell/src/workers.rs`
(new), `lib.rs` (routes, session-delete), `desktop-app/src/main.rs`.

- [ ] **Step 1: Write the failing tests**, with a fake worker. Point the
      supervisor's executable path at the test binary re-entering a stub
      `--worker` that speaks the socket protocol. Tests that start real
      processes are `#[ignore]`d.
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
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement `workers.rs`.** Spawn in proposal §5.1's order, using
      `current_exe()` with `process_group(0)` and piped stdin. Generate the
      token from `/dev/urandom` (32 bytes, hex). The handover line is written
      once, and stdin is then **held open in the supervisor for the worker's
      lifetime**. Dropping it is the lifeline signal, so store it beside the
      registration handle. Add a socket client with short timeouts, the stop
      escalation, and `shutdown_all(data_dir)`.
- [ ] **Step 4: Routes**, per the Interfaces table. The board row's
      files-changed count and commits-ahead-of-base come from one
      `git -C <worktree>` call each, for finished rows only.
- [ ] **Step 5: Lifetime hooks.** `/api/session-delete` calls `shutdown` on the
      deleted session's `worker_children`. `desktop-app` switches to
      `.build(ctx)?.run(..)` and calls `shutdown_all` on `RunEvent::Exit`
      (C11): wait up to 5 s, then `SIGTERM` by registry PID. The standalone
      shell's existing `install_shutdown_handler` already reaches workers
      through `sweep_own` once they are registered as owned by the shell.
      Confirm that with a test, don't assume it.
- [ ] **Step 6: Update the proposal** for C6, C7 and C11.
- [ ] **Step 7: Scoped checks**, plus the ignored tests by hand, then show and
      ask.

## Task 8: UI — setting, *Start worker*, New worker dialog

**Requirements:** 1, 2, 3. **Proposal:** §5.1 Entry points, Dialog, dirty
warning; §5.7 Parent session. **Corrections:** C8, C9. **Files:** `app.js`,
`style.css`, `index.html`.

- [ ] **Step 1: The toggle** (C9). Add a "Worker sessions" `.field-checkbox`
      with its one-line description on the General settings page. It reads
      `GET /api/workers-enabled` and writes through `upsertConfigValue` +
      `saveConfigFile`, following the MCP form's pattern.
- [ ] **Step 2: *Start worker*** (C8). Attach it after `finalizeChatMessage`,
      **assistant messages only**, on `code` elements whose `className ===
      "language-worker-prompt"`, and only when enabled and the current session
      is not a worker. Add a footer row with a `.btn-sm` button. Attach
      nothing during streaming (`updateChatMessage`). History goes through
      `finalizeChatMessage` too, so it gets the button for free. Confirm that
      rather than assuming it.
- [ ] **Step 3: The dialog.** "New worker" has one multi-line field
      (editable), *Cancel* (`.btn-quiet`) and *Start worker*
      (`.btn-primary`). It calls `GET /api/worker-source-status`. When the tree
      is dirty, show the warning block with *Include* pre-selected, mentioned
      files first. On confirm, call `POST /api/workers` with the parent. Build
      it from `showAppDialog`'s structure, as `showRewindDialog` does.
- [ ] **Step 4: Parent links.** A used block's footer shows "Started
      *‹worker title›* →". The parent conversation gets the one-line quiet
      "Spawned worker *‹title›* →" entry, rebuilt from the parent's
      `worker_spawned` events on reload.
- [ ] **Step 5: Verify in the browser** (Global Constraints, UI verification).
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
- [ ] **Step 6: `node --check`, `npm run lint:web`, then show and ask**

## Task 9: UI — sidebar nesting and the Workers board

**Requirements:** 5, 6. **Proposal:** §5.7 Sidebar, Workers board.
**Corrections:** C10. **Files:** `app.js`, `style.css`.

- [ ] **Step 1: Sidebar.** `renderProjectSession` (`app.js:4727`) nests
      workers under their parent and shows a status dot (running, needs you,
      paused, finished, failed). Parentless workers are listed with a dot.
      A **Workers** entry with a needs-you badge sits at the top of each
      project. Workers come from `GET /api/workers?repo=` (by source
      repository), not from `/api/sessions`.
- [ ] **Step 2: The board.**
  - Columns and groups are exactly as in proposal §5.7, newest first.
  - Each row has one visible action: *Review* opens the worker at its
    approval card, *Pause* while running, *Open* otherwise.
  - The `⋯` overflow holds *Stop*, *Reveal worktree in Finder*, *Remove
    worktree* (finished only, with the dirty confirmation naming paths) and
    *Dismiss from board*.
  - Finished rows show files changed and commits ahead.
  - The board polls about once a second while visible, and stops when hidden.
- [ ] **Step 3: C10.** Make the board tolerate a poll that hangs for the
      length of another session's turn: one request in flight at a time, no
      pile-up, and no error state for a slow answer.
- [ ] **Step 4: Verify in the browser.** Seed workers in each state with stub
      progress files and session records, and check groups, ordering, the
      overflow and the badge count. Record what was checked.
- [ ] **Step 5: `node --check`, `npm run lint:web`, then show and ask**

## Task 10: UI — worker session control strip and composer

**Requirements:** 6, 7. **Proposal:** §5.7 Worker session. **Files:**
`app.js`, `style.css`.

- [ ] **Step 1: The control strip** sits under the header:
  - It shows a status pill and `Round r/max` with a bar.
  - Its controls depend on state: *Pause* and *Stop* while running, *Resume*
    and *Stop* while paused, *Stop* while waiting.
  - *Stop* is `.btn-danger`, and *Resume* is the strip's only `.btn-primary`.
  - The header shows "from *‹parent›*" and the branch.
- [ ] **Step 2: The composer** sends `note` while running, `resume` with a note
      while paused, and `message` when idle. Its button label says which one.
      It is disabled while waiting for approval, since notes are refused then.
      Approval cards post `approve`/`reject` through `/api/worker-control`
      instead of the shell's resume routes.
- [ ] **Step 3: Live progress** comes from polling
      `GET /api/worker-progress?session=&after=` about once a second, and
      history comes from the session log as today. When the process has ended
      unfinished: show *Restart worker* and disable the composer.
- [ ] **Step 4: Verify in the browser** against a real worker on a non-4765
      port, with `DAMAIAN_MOCK_MODEL_RESPONSE`. Cover:
      - the strip in each state;
      - a note while running and while paused, with the note appearing as a
        user message before the next round;
      - an approval continuing without a restart, with the same worker PID
        before and after;
      - Stop.

      Record the PIDs observed.
- [ ] **Step 5: `node --check`, `npm run lint:web`, then show and ask**

## Task 11: Kill test, measurement, docs, close the spec

A fresh session that reads the whole proposal, this file and the final code.
It evaluates; it does not inherit.

- [ ] **Step 1: The ignored end-to-end kill test** (proposal §6). Spawn a
      shell process with a worker running a long command, `SIGKILL` the shell,
      and assert the worker and its child are gone within 5 seconds. Include a
      doc comment with the exact command, run it, and record the output.
- [ ] **Step 2: Cold-start measurement.** Measure the worker's time from spawn
      to its first model request on this repository. It is informational,
      recorded here and in proposal §6, and is not a gate.
- [ ] **Step 3: Docs.**
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
- [ ] **Step 4: Proposal §7.** Add the C10 follow-up: serve worker routes while
      a turn streams.
- [ ] **Step 5: Walk proposal §6** and name the test or the recorded manual
      check that covers each item. An item with neither is not met.
- [ ] **Step 6: Acceptance.** In a fixture repository with **no** `AGENTS.md`
      and the setting on, a session asked to split work yields `worker-prompt`
      blocks, and *Start worker* → confirm starts one without copying. With
      the setting off, nothing of this spec is visible or sent. This needs a
      live model, so record whether it was run and with which provider, or
      that it could not be.
- [ ] **Step 7: Close out**, per `AGENTS.md` "When a spec becomes Done":
      - update the proposal's `Status:`, its `docs/specs/README.md` row, this
        Progress table and the `**Done:**` header;
      - grep for `57_worker_sessions` in `Depends on:` lines;
      - re-derive "What to build next";
      - remove #57 from `CHANGELOG.md` `Unreleased`;
      - run `npm run specs:check`.
- [ ] **Step 8: Full quality gate.** Run all seven commands from `AGENTS.md`
      and report the test count.
- [ ] **Step 9: Show the change and the gate result, and ask before committing**
