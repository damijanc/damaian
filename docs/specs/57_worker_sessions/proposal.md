# Feature Spec: Worker Sessions

Status: Done 2026-10-16 (Tasks 1–12; see [`tasks.md`](tasks.md)). Acceptance passed with DeepSeek (`deepseek-v4-flash`) on its re-run in Task 12, after §7 items 10 and 11 were resolved (§6)
Order: 57 of 57
Plan: none. An exception to the graduation rule, like #41–#44 and #47: it came
from describing how this repository is actually built — a planner session that
owns specs and verification, and task sessions that implement — and asking what
Damaian needs to host that workflow itself. Settled in a brainstorming session
on 2026-10-03. It is user-driven and is **not** #38's agent delegation, so #38's
readiness gates do not apply; [`context.md`](context.md) §2 says why.
Depends on: [#17](../17_durable_task_state_and_crash_recovery/proposal.md) (task
state, pending-approval snapshots, recovery) — built;
[#46](../46_process_registry_and_orphan_sweep/proposal.md) (process registry and
orphan sweep) — built; [#20](../20_working_modes/proposal.md) (a worker's mode
is an ordinary session mode) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`08_stop_and_progress.md`](../08_stop_and_progress.md) (`CancelToken`, phases),
[`21_task_plan_progress_and_budget/proposal.md`](../21_task_plan_progress_and_budget/proposal.md)
(round budget), [`36_branch_and_worktree_delivery/proposal.md`](../36_branch_and_worktree_delivery/proposal.md)
(git-mutation approval and dirty-worktree removal, which this follows for the
one mutation it performs), [`34_repository_config_trust_boundary.md`](../34_repository_config_trust_boundary.md)
(`.damaian/` is repository-controlled), [`38_subagent_model.md`](../38_subagent_model.md)
(the "no second execution path" rule this inherits),
[`45_crash_recovery_prompt.md`](../45_crash_recovery_prompt.md). See also
[`SECURITY.md`](../../../SECURITY.md), [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md).

## 1. Summary

From any session, the user can start a **worker**: a new session that receives a
prompt, runs in its own new git worktree, and is hosted by its own process. A
per-project **Workers board** shows every worker, what it is doing, and which
ones need the user. In a worker's session a **control strip** pauses, resumes and
stops it, and the composer adds **steering notes** that reach the model at the
next round boundary. A worker waits for approvals without restarting. If
Damaian exits for any reason — including a crash or `SIGKILL` — no worker and
nothing a worker started keeps running.

## 2. Requirements

1. **Off unless the user turns it on.** One user-scope setting,
   `enable_worker_sessions`, default `false` (§5.0). Off, nothing in this spec
   is visible or reaches the model: no buttons, no board, no sidebar entries,
   no added instruction, and the system prompt is byte-for-byte unchanged.
   Repository configuration cannot turn it on.
2. **Spawn from a model-suggested prompt in one click.** When enabled, Damaian
   itself tells the model how to write a handoff — a `worker-prompt` fenced
   block — so the workflow does not depend on any repository's `AGENTS.md`.
   Only such a block, in an assistant message, carries a **Start worker** action
   that opens the New worker dialog pre-filled with it (§5.1). No copy and
   paste, and no button on anything that is not a model-suggested handoff. The
   dialog contains the prompt and nothing else. Confirming always creates a new
   worktree and always starts the worker, which records the session it came
   from as its **parent**. A "New worker" action in the sidebar opens the same
   dialog empty, for a worker with no parent.
3. **Uncommitted changes are never silently dropped.** If the source tree has
   tracked or untracked uncommitted changes, the dialog says so and the user
   chooses between including them and starting from the last commit (§5.1).
4. **Same turn, different host.** A worker runs the same turn function as the
   shell, under the same command policy, secret redaction, approvals, mode and
   audit log. Only where events go and where control comes from differ. This is
   tested (§6).
5. **Watchable.** Every worker's status, round, current activity and recent tool
   calls are visible without opening it, on the Workers board (§5.7).
6. **Controllable.** Pause takes effect at the next round boundary. Resume
   continues the same turn. A steering note can be sent while running or while
   paused, and enters the conversation as a user message before the next model
   call. Stop ends the turn at the next safe point, with escalation to a
   signal (§5.3, §5.4).
7. **Waits without restarting.** A worker waiting for a command or plan approval
   keeps its process, engine and index, and continues the same turn in-process
   when approved (§5.6).
8. **Nothing survives.** When the shell exits for any reason, every worker exits
   and takes every process it started with it. Deleting a parent
   session stops that parent's workers. Workers without a parent are tied only
   to the application (§5.3).
9. **Credentials never leave memory unprotected.** The API key reaches the
   worker on stdin only — never in its environment, argv, or on disk — and no
   command the worker runs can read it (§5.2).
10. **One worker per worktree.** A worktree hosts at most one worker. The source
   tree never hosts a worker under this spec.

## 3. Non-goals

- **A per-session "planner" role, or tying the workflow to Plan mode.** One
  user setting turns it on everywhere; a finer switch can be added on top later
  without redoing this one.
- **A model tool for handoffs.** The planner writes its prompt as a
  `worker-prompt` fenced block and the user starts the worker from it; there is no
  `propose_worker` tool, no new approval-card kind, and no change to the
  working-mode matrix. A structured tool is a later spec (§7).
- **Loop and no-progress detection.** The board shows plain facts — rounds,
  tool calls, status — and no heuristics. A later spec (§7).
- **The planner reading a worker.** Nothing flows from a worker to its parent.
  The repository is the only channel. A read-only result summary is a later,
  opt-in spec (§7).
- **Merging a worker's branch.** The user merges, or asks a session to, through
  ordinary approved git. [#35](../35_commit_preparation/proposal.md)–[#37](../37_pull_request_creation.md)
  own delivery. The board only shows commits ahead.
- **Running interactive sessions out of process.** Sessions the user chats in
  directly keep running in the shell exactly as today.
- **Workers in the source tree,** or a choice of branch, base, mode or budget at
  spawn time. All are deliberately absent from the dialog (§5.1).
- **Surviving the application.** No headless workers, no reattaching after
  launch.

## 4. Decisions taken during design

Recorded because each was a real choice with a rejected alternative.

| Decision | Rejected alternative | Why |
|---|---|---|
| A process per worker | Background turns on shell threads | The shell is single-threaded (context §3.1); threading it means a run registry and a race audit of every session-log route. A process isolates a hung or looping worker from the UI and makes stop a PID signal. |
| Worker stays alive between turns and waits for approvals | Exit at every approval and resume from the snapshot | The user's choice: no re-index and no restart per approval. The snapshot is still written for crash recovery. |
| Worktrees in `<repo>/.damaian/worktrees/` | Under the data directory; beside the repository | Easy to find and manage, already inside an allowed root, already excluded from indexing. Costs four safeguards (§5.1). |
| One user setting, off by default | A per-session planner role; tying it to Plan mode | It is one person's way of working, not a property of a repository or a mode. Off costs nothing; on applies everywhere. |
| Damaian supplies the handoff instruction | Relying on the repository's `AGENTS.md` | Without it the button only appears in repositories whose `AGENTS.md` happens to teach the convention. |
| *Start worker* on `worker-prompt` blocks only | Also on untagged and `text` blocks; pasting into an empty dialog; a `propose_worker` model tool | The user's rule: better no button than a button on the wrong thing, and models emit untagged blocks constantly. Pasting is the step the workflow already has. A tool needs a mode-matrix decision, a new approval card and eval coverage; a tagged block needs only the instruction. |
| Prompt-only dialog, automatic names, always start | Branch, base, mode, budget and start-mode fields; showing the git command | The user's choice: the dialog is for the prompt. The click is the approval; the command is recorded instead (§5.1). |
| Warn on uncommitted changes and let the user choose | Carry them silently; refuse until clean | A planner's `tasks.md` is usually uncommitted, so silently starting from `HEAD` hands the worker a task it cannot see — but the choice stays visible. |
| Dedicated Workers board | Inline cards in the planner's conversation | Scales to several parallel workers; the planner's conversation gets a one-line link instead. |
| Control strip in the worker session | Composer-driven controls | Stop always visible in one place. |
| Stop in the board row's `⋯` overflow | A visible Stop on every running row | The board's visible action is the common one; the strip is where Stop is always visible. |

## 5. Design

### 5.0 The setting and the handoff instruction

**Setting.** `enable_worker_sessions: bool` in `Config`, default `false`, shown
in Settings as "Worker sessions" with a one-line description. It is classified
`RepositoryKeyClass::Forbidden` ([`config.rs:77`](../../../crates/workspace-engine/src/config.rs)),
and so a `Capability` key in #31's partition:
a repository must not be able to change how the user's sessions behave or add
text to their system prompt. Turning it off while workers are running does not
stop them; it hides the entry points for new ones. As built (Tasks 9–10), it
also hides the board, the sidebar entries and the worker controls, and every
worker route except the list refuses, `stop` and `shutdown` included (decided
in Task 11, §5.7 Setting off). A running worker then ends with its turn or when
the application quits, or is controlled again by turning the setting back on.

**Instruction.** When the setting is on, one fixed paragraph
(`WORKER_HANDOFF_PARAGRAPH`) is appended to the system prompt of every session
that is **not** itself a worker. `system_prompt(mode)` only knows the mode, so
it is left unchanged; the turn calls `system_prompt_for_session(mode,
offer_handoffs)` instead, with `offer_handoffs = enable_worker_sessions &&
worker_record(session).is_none()`
([`chat.rs`](../../../crates/workspace-engine/src/chat.rs), correction C1 in
[`tasks.md`](tasks.md)). Since [#76](../76_worktree_tasks_go_to_a_worker.md),
the paragraph is preceded by `WORKTREE_TASK_PARAGRAPH`, which sends a request
to work in a new branch or worktree to a worker:

> When work should continue in a separate session — because it is a
> self-contained task, or one of several that can run in parallel — write each
> task as a prompt in its own fenced block whose opening line is exactly
> ```` ```worker-prompt ````, with the tag on the fence line itself and not
> inside the block. The user can start a worker from that block. A worker starts with a fresh context in its
> own git worktree and sees only the repository and that prompt, so say what to
> read, what to change, which checks to run, and when to stop. Do not use the
> `worker-prompt` tag for anything else.

The paragraph is Damaian's own text, not repository content. When the setting is
off, the turn sends exactly what `system_prompt` returns today; the existing test
pinning today's Code-mode prompt (`TODAYS_CODE_SYSTEM_PROMPT`) keeps passing
unmodified, and that is part of the proof. Worker sessions never get the
paragraph, so a worker is not invited to hand off further work.

This repository's [`AGENTS.md`](../../../AGENTS.md) shows its task-prompt template
as a `text` block; it changes to `worker-prompt` in the same change that
implements this section, so the two instructions do not disagree.

### 5.1 Spawning a worker

**Entry points.**

- **From a prompt block (the main path).** In an **assistant** message, a
  fenced block tagged exactly `worker-prompt` gets a footer row with a *Start
  worker* button (`.btn-sm`). No other block does — not untagged, not `text`,
  not in a user message — because a button on the wrong thing is worse than no
  button. The action appears once the message has finished streaming. It opens the dialog with the block's exact text, and
  the session holding the message becomes the parent. Several blocks in one
  message give several buttons, which is how parallel tasks are handed off.
- **From the sidebar.** "New worker" opens the dialog empty, with no parent.
- **Not inside a worker.** Worker sessions show neither entry point, so workers
  do not spawn workers in this spec.
- **Not when disabled.** With `enable_worker_sessions` off, neither entry point
  exists, and a `worker-prompt` block renders as an ordinary code block.

**Dialog.** Title "New worker", one multi-line field for the prompt, *Cancel*
(`.btn-quiet`) and *Start worker* (`.btn-primary`). The prompt stays editable
when pre-filled; what is sent is what the field holds on confirm.

**When the source tree is dirty**, the dialog adds one warning block, built from
`git status --porcelain=v1 --untracked-files=all` on the source repository:

> ⚠ *N* uncommitted files in your tree, including `<files the prompt mentions>`.
> The worker won't see them unless you include them.
> ◉ Include uncommitted changes ○ Start from last commit

Files the prompt mentions by path are named first. *Include* is pre-selected;
this is a content choice, not a consent escalation, so
[`UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) §5's rule against pre-selecting
approval does not apply.

**Naming.** A slug is taken from the first non-blank line of the prompt:
lowercase ASCII words joined by `-`, at most 40 characters (cut at a word
boundary), `worker` if empty. Non-ASCII characters are dropped, not treated as
separators. The branch is
`damaian/<slug>` and the worktree `<repo>/.damaian/worktrees/<slug>`. If either
exists, `-2`, `-3`, … is appended to both until neither does.

**Creation**, in order, refusing before any change if a check fails:

1. Refuse if the source is not a git repository, or is a subdirectory of one
   rather than its top level.
2. Resolve the base. *Start from last commit*: `HEAD`. *Include*:
   `git stash create`, which records tracked changes as a commit object without
   touching the working tree or index; if it prints nothing, there were no
   tracked changes and the base is `HEAD`.
3. Refuse if any component of `.damaian/worktrees/<slug>` is a symlink, if the
   path exists, if `git ls-files` reports anything tracked under it, or if the
   resolved path is not inside the repository. `.damaian/` is
   repository-controlled ([#34](../34_repository_config_trust_boundary.md)); a
   cloned repository could ship a symlink there.
4. Ensure `.damaian/worktrees/` is a line in `.git/info/exclude` (local, never
   committed; `.gitignore` is never edited). Added once.
5. `git worktree add -b damaian/<slug> <path> <base>`.
6. *Include* only: copy each untracked, non-ignored file into the same relative
   path in the worktree. `git stash create` does not capture untracked files,
   and a new spec folder is usually untracked.
7. Create the worker session (repository root: the worktree path) in Code mode
   with the configured round budget, and append a `worker_spawned` event to it
   recording parent, source repository, worktree path, branch, base commit, the
   include choice, and the exact git commands run. Append a `worker_spawned`
   event to the parent (if any) carrying the worker's session id. Both go
   through typed `SessionStore` methods (`record_worker_spawned`,
   `record_worker_child`), since `append_session_event` stays private.
   Sessions are keyed by repository id, and each worktree is its own
   repository id, so the worktree path and source repository live only in the
   worker's `worker_spawned` record. `worker_record`, `worker_children` and
   `worker_sessions_for_source` read them back (correction C2 in
   [`tasks.md`](tasks.md)).
8. Record the same facts as a `worker_spawned` audit event. Its
   `gitCommands` field is the JSON array of the commands, in order.
9. Start the worker process (§5.2) with the prompt as its first message. The
   route answers once the process is started and registered; the prompt is
   sent as `message` from a thread of its own once the worker's socket
   answers, so a slow worker start never holds the shell's one request
   thread. The session's title is the prompt's first non-blank line, at most
   80 characters.

A creation that fails is recorded as a `worker_spawn_failed` audit event with
the error. A failure after step 5 leaves the worktree and its branch in place,
and the commands run so far are not in that event, because
`create_worker_worktree` does not return them on an error (a follow-up, §7).
Before step 1 the shell checks that the worker's control socket path fits
(§5.2). Its length depends only on the data directory, so a data directory too
long for it is refused before anything exists (Task 11; until then it was
refused only at step 9, after the worktree, branch and records).

**Approval.** #36 requires action-specific approval for git mutations. Clicking
*Start worker* is that approval: the user initiated the action and the dialog
states what it does. The exact commands are not shown in the dialog, by the
user's decision, and are instead recorded in the audit log and shown as the
first entry of the worker's session. Repository configuration cannot start a
worker, choose its location, or skip this click.

**Removing a worktree** (board overflow, finished workers only):
`git worktree remove <path>`, refused unless the path is a registered worktree
directly under the source's `.damaian/worktrees/`, reached through no symlink.
If the worktree has uncommitted changes, a second
confirmation names the dirty paths first, following #36; `--force` is passed
only after that confirmation. The branch is never deleted. It is refused while
a live worker holds the worktree, and a removal is audited as
`worker_worktree_removed`.

**One worker per worktree.** Spawning always makes a new worktree, so the rule
is enforced where it can still be broken: *Restart worker* (§5.7) refuses if a
live worker already holds that session's worktree.

### 5.2 The worker process

**Binary.** The application's own executable (`std::env::current_exe()`),
re-launched as `--worker --session <id> --data-dir <dir>`. Not the `damaian` CLI,
which is not in the app bundle (context §3.6). One binary means nothing extra to
bundle, sign or notarize, the worker is always the same version as its shell,
and it carries the code-signing identity the user already granted Keychain
access to. Worker mode never opens a window and never starts the HTTP shell. It
refuses to start if stdin is a terminal, so it cannot be run by hand without the
handover. In development, where the shell may run as `damaian-desktop-shell`,
that binary takes the same `--worker` mode.

**Handover.** The shell takes the API key from the same in-memory cache its own
turns use (`resolve_model_api_key`), so spawning a worker never causes a
Keychain prompt, and generates a 256-bit random
**control token**. It writes one JSON line to the worker's stdin:
`{"apiKey": "...", "token": "..."}`. The worker reads that line and **keeps stdin
open** as its lifeline (§5.3). The key is held only in memory. The worker's
environment is the shell's environment, unchanged: no key is added.

**Same turn.** The worker builds an engine for the session's repository and runs
turns through the same `ChatOrchestrator` entry points the shell uses. Its
`TurnSink` sends progress to the progress file (§5.5) instead of SSE, and its
`TurnSteering` (§5.4) and its cancel token are driven by the control socket instead of a connection.
Nothing in command policy, redaction, approval, mode enforcement or audit is
reimplemented or bypassed. Both hosts build a turn's engine and adapter with
the same two functions in `desktop-shell` (`chat_engine` and
`chat_model_adapter`); the shell passes the key it resolves and the worker
passes the handed-over one. Like the CLI, `chat_model_adapter` answers from
`DAMAIAN_MOCK_MODEL_RESPONSE` when it is set, for both hosts. The worktree is
the worker record's `worktree_path`, and its repository config applies at
repository scope exactly as it does for the shell. Each turn gets a fresh
cancel token and `TurnSteering`, and runs on one turn thread, which alone writes
the progress file. The worker takes the configured provider and model.

**Control socket.** `<data_dir>/workers/<hash>.sock`, where `<hash>` is the
first 16 hex characters of the SHA-256 of the session id
(`desktop_shell::worker_socket_path`, which the shell uses too). A Unix domain
socket created with mode `0600` in a directory with mode `0700`. It is not named
by the session id itself because that does not fit: macOS caps `sun_path` at 104
bytes including its NUL, and the id-named path measured 107 bytes under the real
`~/Library/Application Support/DamaianClient` and 132 under a `/var/folders/…`
test directory (2026-10-12). The hashed path is 91 bytes under the real
directory. When even that does not fit, the worker refuses to start and says
how long the path is, and the shell refuses the spawn before creating anything
(§5.1). A leftover socket from a crashed worker is replaced; one
that still answers is not, because a live worker holds it. Line-delimited JSON
requests, one response each, one connection at a time; a connection idle for 5
seconds is closed. Every request carries the token; a wrong or missing token
closes the connection unanswered. Requests are bounded: a line at 256 KiB, a
note at 16 KiB. A request with an argument the table does not name is refused.
Commands:

| Command | Effect |
|---|---|
| `message {text}` | Starts a turn with this user message. Refused unless idle: while a turn runs or waits for approval. |
| `pause` | Sets the pause flag (§5.4). |
| `resume {note?}` | Clears it, optionally queuing a note first. |
| `note {text}` | Queues a steering note. Refused while waiting for approval. |
| `approve {proposalId}` / `reject {proposalId}` | Resolves a pending command or plan approval (§5.6). |
| `stop` | Cancels the current turn at the next safe point. Refused while waiting for approval: reject the approval, or `shutdown`. |
| `shutdown` | Stops any turn, kills child processes, exits. |
| `status` | Returns `{sessionId, pid, state}`, `state` being `idle`, `running`, `paused`, `waiting_for_approval` or `shutting_down`, plus `proposalId` while waiting. |

Unknown commands are rejected. The shell stays single-threaded: each exchange is
short, and the shell never holds a socket open.

**Registration** (correction C6 in [`tasks.md`](tasks.md)). The #46 registry
records the registering process as an entry's owner, so the **shell** registers
each worker it spawns, as `ProcessKind::Worker` with the worker's session id.
The launch sweep then kills the workers of a crashed shell, and the shell's own
`sweep_own` reaches them on a signal: the standalone shell's
`install_shutdown_handler` records `orphan_process_killed` with kind `worker`
for them, which a test confirms
(`the_shells_signal_handler_reaches_workers_through_sweep_own`). Each worker
registers its own children, with itself as owner, so the worker's `sweep_own`
kills exactly those. The spawn itself is recorded as `worker_spawned` (§5.1
steps 7–8); there is no separate `worker_started` event.

The shell starts the worker with `process_group(0)`, piped stdin, stdout to
`/dev/null` and stderr inherited, so a worker's refusals reach the shell's own
log. It keeps the worker's stdin beside the registration handle for the
worker's whole life (`desktop-shell/src/workers.rs`), and both go when the
shell sees the worker exit.

### 5.3 Lifetime: nothing survives

1. **Lifeline.** A worker thread blocks reading stdin. When the shell exits for
   any reason, including `SIGKILL`, the kernel closes the pipe and the read
   returns EOF. The worker then shuts down immediately (step 3). This does not
   depend on the shell running any cleanup code.
2. **Children are found through the registry, not a process group.** Every
   command the worker runs already gets its own process group
   (`command_runner.rs`, `process_group(0)`) and is registered with #46 with the
   worker as its owner, and model calls are registered the same way. So the
   worker's own children are exactly the registry entries it owns.
3. **Shutdown** — on lifeline EOF, `shutdown`, or `SIGTERM`: cancel the turn,
   then `ProcessRegistry::sweep_own`, which terminates every process this
   worker registered (`SIGTERM` to its group, then `SIGKILL` after the
   registry's grace period); then exit. The worker also installs #46's
   `install_shutdown_handler`, so `SIGTERM`/`SIGINT`/`SIGHUP` take the same
   path.
4. **Clean quit.** When the application quits normally, the shell sends
   `shutdown` to every worker, lets go of every lifeline, waits up to 5
   seconds, then `SIGTERM`s any that remain by registry PID after an identity
   check. Never by name. The Tauri host had no exit hook; it now ends in
   `.build(ctx)…run(..)` and calls `desktop_shell::shutdown_all_workers` on
   `RunEvent::Exit` (correction C11 in [`tasks.md`](tasks.md)). The standalone
   shell has no such hook: its signal handler's `sweep_own` reaches the workers
   (§5.2, Registration).
5. **Parent removed.** Deleting a session sends `shutdown` to every worker
   whose parent it is, and to the session's own worker if it is one; each
   also loses its lifeline and is `SIGTERM`ed by identity if it has not exited
   5 seconds later. (There is no archive action in the shell today; if one is
   added, it does the same.) Their worktrees and session logs are kept, and
   their sessions remain in the sidebar.
6. **Backstop.** #46's orphan sweep at the next launch kills any registered
   worker or worker child still alive.

What persists is records, not processes: the session log, the worktree, any
pending-approval snapshot. A worker whose process ended without finishing its
turn is marked `Interrupted` through #17, and its session offers *Restart
worker* (§5.7).

**A live worker is not crash debris** (correction C12 in
[`tasks.md`](tasks.md)). #17's sweep assumes every non-terminal task in the
data directory was left by a process that died, and the shell runs that sweep
lazily, on the first `GET /api/recovery`, which can come after it has started a
worker. So `classify_session` and `reattach_pending_approvals` skip a session
that has a worker record **and** a live `ProcessKind::Worker` registry entry
for it (`process_registry::live_worker_registered`: kind `worker`, that
session id, and a pid whose start time still matches). Such a session gets no
`task_recovered` event, no `lost_to_crash` usage for its in-flight model call,
no recovery card and no reattached approval that the shell could answer behind
the worker's back. A worker whose process has ended is still classified as
before: that is how its task becomes `Interrupted`, how a lost model call is
accounted for, and why skipping every worker session outright was rejected.
The registry decides rather than the supervisor because the guarantee belongs
in the engine, where every caller of the classifier gets it, and the shell
registers a worker before handing it anything to run, so there is no window in
which a worker has a task and no entry.

**Stop escalation** from the UI: `stop`; if the turn has not ended within 10
seconds, `shutdown`; if the process has not exited 5 seconds later, `SIGTERM` to
its registry PID. As built (`Supervisor::stop`), the shell asks `status`
first. A worker waiting for approval refuses `stop`, so it goes straight to
`shutdown`; an idle worker has no turn to stop, so the worker's own refusal
is the answer and nothing follows; a worker that does not answer goes
straight to `shutdown`. The route answers after these first exchanges, and the
waiting runs on a thread of its own, so the shell's one request thread is
never held for the 15 seconds. The signal is sent only while the pid still has
the registered start time (`ProcessIdentity::send_sigterm`), and is audited as
`worker_terminated` with that identity.

### 5.4 Pause and steering notes

`TurnSink` gains one field, `steering: Option<&'a TurnSteering>`, beside the
existing `cancel`. `TurnSteering` holds a pause flag, a queue of notes and a
condition variable, and is woken by cancellation too. Every existing host —
the shell, the eval harness, tests — passes `None`, so both hosts run identical
turn code and only the worker ever pauses.

At the existing round-boundary check, before the next model request
(the top of the round `loop` in `run_agentic_turn`,
[`chat.rs`](../../../crates/workspace-engine/src/chat.rs)):

1. If cancelled, behave exactly as today.
2. If paused: persist `TaskStatus::Paused` (new variant), emit a `status`
   progress event, and wait on the condition variable until resumed or
   cancelled. Cancelled while paused ends the turn as `Cancelled`. As built,
   the pause hold sits just before the cancel check rather than after it, so a
   stop that arrives while paused leaves the wait and goes through the existing
   cancel path unchanged. The progress events are `TurnProgress::Paused`,
   `Resumed` and `NoteAdded { message_id }`.
3. Drain the note queue. Each note is appended to the conversation and to the
   session log as an ordinary user message (`append_message`, role `user`),
   followed by a `steering_note_added` session event naming that message's id —
   `ChatMessage` has no origin field, and adding one would change every
   message's serialized form for one marker. The note is sent to the model with
   the next request, passed through `SecretScanner::redact` first; the session
   log keeps it as written, as it keeps every user message (correction C3 in
   [`tasks.md`](tasks.md)). Planning assumed user messages are redacted on the
   way to the model. The code says they are not: a user's prompt is sent as
   typed. A note is redacted anyway, which is stricter than the prompt and
   never weaker. There is no composer length limit to reuse; the worker's
   socket (Task 5) is where a note's length is bounded. Emit a `note` progress
   event.

   A note reaches the turn only at a boundary. A round that ends in a command or
   plan approval never returns to the loop's top, so the worker carries the
   notes it has accepted but not delivered into the resumed job: `approve` and
   `reject` continue the same turn (§5.6), and its next boundary drains them
   (Task 11, `a_note_queued_before_an_approval_reaches_the_resumed_turn`; before
   that they were dropped with the job's `TurnSteering`). A note accepted during
   a turn's final round, which then completes with no further model request, is
   not delivered: no boundary follows. Carrying it into the next `message` would
   put it after a message the user wrote later, so that case is a follow-up
   (§7), and the user guide says so.
4. If the turn was paused, persist the running status again and continue.

A pause requested during a model call or a tool takes effect at the following
boundary; a running tool or command is never interrupted by pause. A turn
waiting for approval has already stopped its loop, so pause and notes do not
apply there.

`Paused` is a persisted, recoverable state. If the process dies while paused,
#17 reports the task `Interrupted` and the queued notes are lost with it; notes
already drained are in the session log. Recovery never **auto-resumes** a task
in a session that has a worker record, whatever its status (correction C5): its
repository is a worktree and its host is gone, so the shell must not pick the
turn up on its own. The user restarts the worker instead (§5.7).

### 5.5 The progress file

`<data_dir>/sessions/<id>.progress.jsonl`, one JSON object per line:
`{"seq", "timestampMs", "kind", "payload"}`. `seq` strictly increases from 1
within the file. `timestampMs` uses the session log's clock. `payload`
carries the columns below, nested so that a payload field can never collide
with `seq` or `kind`:

| `kind` | Carries |
|---|---|
| `status` | `{state, taskStatus?, error?}`: the worker's state (as `status` reports it), the turn's `TaskStatus` when known, and the error of a turn that failed to run. Written when a job starts, on pause and resume, and when a job ends |
| `phase` | `phase`, `label`, `round`, `maxRounds` — today's SSE payload |
| `plan` | today's `plan` payload |
| `text` | model output text, batched about every 250 ms |
| `tool` | the tool label of a `phase` event of kind `tool` — what the shell already shows while a tool runs. Durations are the gaps between events; outcome is not recorded here (the audit log has it) |
| `approval` | `{kind, proposal}`: `kind` is `command`, `plan`, `patch` or `fallback`, and `proposal` is the shell's own JSON for it |
| `note` | a steering note was added |

Every string in the payload, at any depth, passes through
`SecretScanner::redact` before it is written. This is not "the same redaction
as the session log": that log is unredacted on write and redacts at its call
sites (correction C3), so this file redacts for itself and does not trust its
caller. The file is truncated when a worker process starts, and deleted with
the session (`SessionStore::delete_session`). A reader skips any line that does
not parse, because it may read while the worker is appending. Each line is
written whole by one `write_all`, so only the last line can be torn. The audit log stays the authoritative record of tool calls; this
file exists so the board can show a per-session list without joining audit
events on `taskId` (context §3.5).

### 5.6 Approvals

- **Command and plan approvals.** The turn stops exactly as it does in the
  shell: it writes the `chat/pending/<proposal_id>.json` snapshot, persists
  `WaitingForApproval`, and returns its proposal. The worker emits an
  `approval` progress event and **stays alive**, with its engine, index and key
  still loaded, waiting for `approve` or `reject` on the socket. On a decision
  it calls the same `resume_after_command_decision` /
  `resume_after_plan_decision` the shell uses, in the same process — which
  takes the snapshot as it does today, so a decision still resumes once, and
  there is still exactly one resume path. If the process dies while waiting,
  the snapshot is there for #17's recovery.
- **Patch approvals.** A patch proposal ends the turn, as it does in the shell,
  and the worker stays alive and idle until the next `message`. The patch is
  applied through the existing `/api/apply-patch` route with the worktree as
  `repo`, unchanged: it already carries file and hunk selection and the
  generated-secret override, and calls the hash-verified `apply_stored_patch`
  on whatever repository it is given. No turn is running while it applies, so
  nothing races the worker. Continuing a turn after a patch is applied would be new turn
  semantics; if wanted, it belongs in the engine for both hosts, not here.
- **Provider fallback offers** (#56) end the turn the same way, with the
  worker idle. The worker holds only the configured provider's key, so the
  offer is answered through the shell's existing route with the worktree as
  `repo`.

### 5.7 UI

**Sidebar.** Workers are nested under their parent session with a status dot
(running, needs you, paused, finished, failed). Parentless workers are listed
like any other session, with a dot. A **Workers** entry at the top of each
project opens the board and carries a badge counting workers that need the
user. Workers belong to the project of their *source* repository, not to their
worktree path: a worker session's own repository is its worktree, so the
session list for a project adds the sessions whose `worker_spawned` event names
that project's repository as the source.

**Workers board** (one per project), a table:

- Columns: status dot, worker (session title), planner (parent title, or —),
  rounds bar with `round/maxRounds`, *Doing* (current phase or tool label, or
  why it stopped), elapsed time.
- Grouped under *Needs you* (waiting for approval, paused), *Running*, and
  *Finished* (complete, failed, budget exhausted, cancelled, interrupted),
  newest first in each group.
- One visible action per row: *Review* for an approval, which opens the worker
  session at the approval card, because consent content is shown in full and a
  table row cannot do that; *Pause* while running; *Open* otherwise.
- `⋯` overflow: *Stop* (running, paused, waiting), *Reveal worktree in Finder*,
  *Remove worktree* (finished only, §5.1), *Dismiss from board* (finished only;
  the session stays in the sidebar).
- Finished rows show files changed and commits ahead of the base.
- The board polls `GET /api/workers?repo=<path>` about once a second while
  visible. The source repository is compared by its canonical path, as the
  worker record stores it.

As built (Task 9), with no new route:

- **Where it shows.** The board takes the conversation's place in the main
  pane (`#workers-board`): the log, the composer and the header's ⋮ step
  aside, and the header reads "Workers · ‹project›". Opening any session,
  starting a new one or switching project leaves it. Opening the board of a
  project that is not open opens that project first.
- **Sidebar.** Under an expanded project: the Workers entry first, then the
  sessions in their usual order, each worker indented under its parent
  (oldest first). A worker without a parent in that list, because it had
  none or its parent was deleted, is placed among the sessions by its
  `updatedAtMs`. A worker row opens the worker under its source project
  (`openWorkerSession`), which becomes the active row; if only the worktree
  was added as a project, the worker opens in place. Its ⋮ is the board's
  menu without *Dismiss*. The sidebar's rows are read when a project is drawn,
  when a session loads, on every board poll and when the window regains focus.
  There is no timed poll behind them, so with the board hidden a badge can be
  stale until one of those happens.
- **Dots.** *Needs you* for waiting for approval, a hollow *paused* for
  paused, *running*, then for finished rows *failed* when the row carries an
  error or ended `failed`, `interrupted`, `unknown_external_outcome` or with a
  budget used up, and *finished* otherwise (complete, cancelled). A worker
  whose process ended while paused or waiting is reported `interrupted`
  (C5), so it shows failed with "Interrupted: its process ended mid-turn" and
  *Open*. *Restart worker* is the worker session's (Task 10).
- **Stop** shows for every live worker, not only running, paused and waiting
  ones. An idle worker refuses `stop` (it has no turn), so *Stop* on an idle
  row sends `shutdown`. That is also how a finished but still-live worker is
  ended. *Remove worktree* stays visible on such a row but disabled with
  "Stop the worker first", because the shell refuses removal while a live
  worker holds the worktree. Both confirm first; removal shows the worktree
  path and says the branch is kept, and a `dirty` answer is confirmed a second
  time naming the paths, then resent with `confirmed_dirty=true`.
- **Reveal worktree in Finder** is the existing `POST /api/reveal-in-finder`
  with the row's `worktree` as `repo`.
- **Dismiss from board** is client-side: session ids in the webview's
  `localStorage` (`damaian:dismissedWorkers`). It hides a finished row from
  the board only. A dismissed worker that runs again (*Restart worker*) is
  taken off the list and shows again.
- **New worker** is on the board's header and in the project row's ⋮ (§5.1
  Entry points: empty, no parent).
- **Elapsed** runs live for unfinished rows. A row has no end time, and the
  session's `updatedAtMs` does not move while a worker's turns run, so a
  finished row shows "—" rather than a duration that reads as zero.
- **A held shell (C10).** The shell serves one request at a time, so a poll
  can wait for the whole of a turn streaming in another session. The board
  keeps one request out at a time and schedules the next a second after the
  last one answered, so a held shell gets one poll, not a pile. A slow
  answer is not an error. The rows stay as they were, and after three seconds
  a quiet line says how old they are and that the shell answers once a turn
  running in another session ends. Only a request that fails shows "Could not
  refresh: …". Workers keep running meanwhile; only the view lags. The board
  does not poll while the window is hidden.

**Worker session.** The ordinary session view with two additions:

- A **control strip** under the header: status pill, `Round r/max` with a bar,
  and the controls for the current state — *Pause* and *Stop* while running,
  *Resume* and *Stop* while paused, *Stop* while waiting. *Stop* is
  `.btn-danger`; *Resume* is the strip's one `.btn-primary`. The header shows
  "from *‹parent›*" and the branch.
- The **composer** sends a `note` while a turn runs, a `resume` with note while
  paused, and a `message` when idle. Its button label says which.
- Live progress comes from `GET /api/worker-progress?session=<id>&after=<seq>`,
  polled about once a second; history comes from the session log as today.
- A worker whose process has ended unfinished shows *Restart worker*, which
  starts a new process for the same session and worktree (refused if another
  live worker holds it). Its composer is disabled until then.

As built (Task 10), with no new route:

- **Strip.** It sits under the header while the open session is a worker and
  the setting is on, and is removed otherwise. Its state comes from the
  progress file's `status` lines and the route's `alive`, its round from the
  last `phase` line ("Round —" until one arrives). *Stop* also shows for an
  idle worker and sends `shutdown` there, as on the board (Task 9), and
  confirms first. A worker whose process ended shows *Interrupted* when it
  ended mid-turn (the row's `taskStatus`, C5) and *Ended* otherwise. Both
  offer *Restart worker*, because no other control can bring an ended
  worker back to take a message; it is not filled, as *Resume* stays the
  strip's one primary.
- **Composer.** Its round send button becomes a labelled one (*Send note*,
  *Resume with note*, *Send message*), and the attach menu and model choice
  step aside, because a worker takes the configured provider and model and
  its messages carry text only. While the worker waits for approval, is
  stopping or has ended, the field and the button are disabled and the
  placeholder says why. `sendChatPrompt` hands everything to the worker
  while its view is open, so no path from the worker view reaches the
  shell's turn routes.
- **Live progress.** The poll follows the board's C10 rules: one request
  out, the next a second after the answer, nothing while the window is
  hidden, and it ends when another session, a new one, the board or the
  setting going off replaces the view. A slow answer shows nothing; a
  failed one says "Could not refresh". The first read catches up silently,
  because what finished is already in the log. After that, a running turn
  draws one assistant bubble with the shell's turn indicator, fed by
  `phase`, `text` and `plan` lines. A `note` line closes that bubble and adds
  the note as a user message, then the next round gets a new bubble. When a
  job ends (`idle` or `waiting_for_approval`) or the process exits, the
  conversation is drawn again from the session log, as the shell does after
  a turn of its own.
- **Approvals.** The worker's card is the shell's own: a command shows
  `createCommandApprovalPreview`, a plan shows `createPlanReview`, with
  consent content in full. A `decide` option sends the decision to
  `/api/worker-control` (`approve`/`reject` with `proposal_id`) instead of
  the shell's resume routes. Without it the cards are unchanged. Because the
  worker's `approve` carries no grant or step revision, the command card
  offers no "always allow" overflow and the plan's steps are shown, not
  edited. A command or plan card shows only while the live process waits.
  A patch or fallback offer shows the shell's card unchanged, with the
  worktree as `repo`, while the worker is idle after that turn. The card
  sits beside the last answer's body, so *Review* on the board finds it;
  *Review* waits for the view's first read (at most 3 s).
- **Recovery.** The shell's crash-recovery card is not shown for a worker
  session: its *Resume* would continue the worker's turn in the shell, which
  C5 rules out, and the strip's *Restart worker* takes its place. Findings
  for a worker are read with its worktree as `repo`.
- **Restart.** A restarted worker is idle, with a new progress file. When the
  old process was waiting for approval, the toast says that approval ended
  with it and its command did not run (Task 7), and the card is gone.
- **Setting off.** Workers show as ordinary sessions: no strip, no header
  line, no poll, and the composer as it was. **Decided in Task 11:** the
  shell keeps refusing `stop` and `shutdown` (403) while the setting is off,
  like every worker route but the list. With it off the UI offers no worker
  controls at all, so accepting them would serve no client, and "off" stays
  one rule with no exceptions to explain. The worker still cannot outlive
  the application, and turning the setting back on restores its controls.

**Parent session.** Prompt blocks carry *Start worker* (§5.1). Once a block has
been used, its footer also shows "Started *‹worker title›* →", linking to the
worker, so a second click is a visible, deliberate second worker rather than an
accident. A one-line, quiet entry is appended where the worker was spawned:
"Spawned worker *‹title›* →". No live card.

As built (Task 8), with no new route: the UI reads `GET /api/workers` for the
open folder, and also for its source when the folder is itself a worktree
(`<source>/.damaian/worktrees/<slug>`). A session whose id is in those rows is
a worker and shows neither entry point. The rows with the open session as
`parentSessionId` are its workers. No route says which block started a
worker, and the parent's `worker_spawned` event does not reach the UI. So on
reload each worker is placed at the latest assistant message created before
it that holds a `worker-prompt` block whose first non-blank line (at most 80
characters, the shell's title rule) is the worker's title. That block gets the
"Started" link and the message gets the entry. A worker whose prompt was
edited in its first line matches no block. Its entry then follows the last
message created before it, and no block gets a link. Two identical blocks
give the link to the later one. Right after a spawn, the clicked block and its
message get both, exactly. The link opens the worker where the sidebar lists
it, under its source project (Task 9). The shell refuses an ordinary turn sent
to it from the source folder, because the session belongs to its worktree;
the worker's own composer is Task 10's.

**Shell routes** (the table is in [`tasks.md`](tasks.md), Interfaces):
`GET /api/workers-enabled`, `GET /api/worker-source-status`,
`POST /api/workers` (spawn), `GET /api/workers`, `GET /api/worker-progress`,
`POST /api/worker-control`, `POST /api/worker-restart`,
`POST /api/worker-remove-worktree`. Each answers from files or one short
socket exchange. Like every shell route they take url-encoded form fields,
not JSON (correction C7): `/api/worker-control` takes `session_id`,
`command`, and `text`, `note` or `proposal_id` as the command needs, and
forwards only the fields the command takes, under the socket's names (`text`,
`note`, `proposalId`). Only the socket speaks JSON lines. `stop` runs the
escalation (§5.3) and `shutdown` the parent-removed path; every other command
is relayed and the socket's answer returned as it is.

With `enable_worker_sessions` off, every route refuses (403) except
`GET /api/workers`, which keeps running workers reachable (§5.0), and
`GET /api/workers-enabled`, which is how the UI reads the setting. A session
id is checked against the worker's own id rules before any path is built from
it. A `GET /api/workers` row carries `sessionId`, `title`, `parentSessionId`,
`parentTitle`, `branch`, `worktree`, `alive`, `state` (the worker's last
reported state, or `exited`), `taskStatus` (`interrupted` when the process
ended part way through a turn), `group` (`needs_you`, `running` or
`finished`), `round`, `maxRounds`, `doing`, `createdAtMs`, `updatedAtMs`, and
for finished rows `filesChanged` (`git diff --name-only <base>`) and
`commitsAhead` (`git rev-list --count <base>..HEAD`), one `git -C <worktree>`
call each. A live worker that is idle before its first turn has finished is
*Running*; one idle after a finished turn is *Finished*.

### 5.8 Trust boundary

- The one configuration field, `enable_worker_sessions`, is `Forbidden` for
  repository scope (§5.0). Any further field added during implementation is
  classified the same way: a repository must not be able to enable workers,
  start them, move worktrees, change worker lifetime, or add to the system
  prompt.
- Repository-supplied `AGENTS.md` reaches a worker exactly as it reaches any
  session ([#11](../11_agents_md_support.md)). It may constrain the worker and
  cannot widen its mode.
- The worktree path is created by Damaian under checks that treat `.damaian/`
  as untrusted (§5.1 step 3).

## 6. Testing and acceptance

All fixture repositories set `enable_index_watcher: false`. Model-dependent
paths use `DAMAIAN_MOCK_MODEL_RESPONSE`, and every test uses its own data
directory.

**Engine.**

- `TurnSteering`: pause takes effect only at a round boundary; `Paused` is
  persisted; resume continues the same turn; notes become steering-marked user
  messages before the next model request; cancel while paused yields
  `Cancelled`; a note while waiting for approval is refused.
- **Same-turn guarantee:** one scripted turn run through the shell host and
  through the worker host produces the same sequence of audit event types and
  the same session-log events, ignoring ids and timestamps.
- Progress file: `seq` strictly increases, `after=` paging is exact, and a
  seeded secret never appears in the file.

**Worker process.**

- A command run by the worker sees neither the API key nor the control token in
  its environment, and neither appears in the worker's argv.
- Closing the worker's stdin makes it exit and kills a sleeping child command it
  started — a normal test, since it needs no real shell to die.
- The socket file is `0600` and its directory `0700`; a wrong token and an
  unknown command are rejected.
- A command approval continues in-process; the snapshot exists while waiting and
  is gone after; killing the worker while waiting leaves a snapshot #17's
  recovery finds.
- A patch proposal leaves the worker idle and alive, and `/api/apply-patch`
  with the worktree as `repo` applies it.

**Spawning and worktrees.**

- Creation under `.damaian/worktrees/<slug>`, the exclude line added once, and
  name-collision suffixes.
- Refusals: symlink in the path, existing path, tracked path, path resolving
  outside the repository, non-git source.
- *Include* carries tracked edits and untracked non-ignored files; *Start from
  last commit* carries neither.
- The `worker_spawned` events (worker, parent, audit) record the exact git
  commands.
- *Restart worker* refuses while a live worker holds the worktree.
- Deleting a parent stops its workers and keeps their worktrees.
- Removing a dirty worktree requires the second confirmation and is never
  forced without it.

**Ignored, with side effects** (each with a doc comment naming the command that
runs it): spawn a shell process with a worker running a long command, `SIGKILL`
the shell, and assert the worker and its child are gone within 5 seconds.

**UI**, verified in the running shell on a port other than 4765 with its own
`DAMAIAN_DATA_DIR`: board groups and ordering, *Stop* in the row overflow, the
control strip in each state, a note while running and while paused, the dialog's
clean and dirty states, the parent's one-line link. *Start worker* appears on a
`worker-prompt` block in an assistant message and on nothing else — not an
untagged, `text`, `prompt` or `rust` block, not a `worker-prompt` block in a
user message — not while the message is still streaming, and not in a worker
session; it pre-fills
the dialog with the block's exact text; a used block shows its "Started" link.
There is no JavaScript test runner, so the tag rule is checked by driving `renderMarkdown` from the browser console. With the setting off: no
board, no sidebar entries, no button on a `worker-prompt` block.

**Setting and instruction.**

- With the setting off, `system_prompt` is unchanged for every mode;
  `TODAYS_CODE_SYSTEM_PROMPT` passes unmodified.
- With it on, the §5.0 paragraph is present for a non-worker session and absent
  for a worker session.
- `enable_worker_sessions` in a repository config is dropped and recorded as a
  forbidden key, and the user value stands.

**Also:** `cargo run -p eval-harness -- run --tier deterministic` (turn hosting
and the system prompt changed); a recorded, informational measurement of worker cold start on this
repository; and the full seven-check quality gate from `AGENTS.md`.

**Acceptance.** With "Worker sessions" turned on, in a repository with no
`AGENTS.md`, asking a session to split work into tasks yields `worker-prompt`
blocks; the user clicks *Start worker* on one and confirms, without copying
anything. With it off, nothing of this spec is visible or sent to the model. The worker runs in its own worktree and appears on the Workers board. The user can see what
it is doing, pause it, steer it with a note, and stop it. It waits for approvals
without restarting. If Damaian exits for any reason, nothing it started keeps
running.

### As verified (Task 11, 2026-10-16)

Each item above, with the test or the recorded manual check that covers it.
`ws` is `crates/workspace-engine`, `ds` is `crates/desktop-shell`, and "ignored"
means a real-process test run by hand (`tasks.md` rows 5–7 and 11).

- **`TurnSteering`.** `ws/tests/steering.rs`:
  `pause_takes_effect_only_at_a_round_boundary`,
  `a_paused_turn_persists_paused_then_running_again`,
  `resume_continues_the_same_turn`,
  `a_note_becomes_a_user_message_before_the_next_model_request`,
  `cancel_while_paused_ends_cancelled`, `a_turn_without_steering_is_unchanged`;
  `steering.rs::wait_while_paused_returns_on_cancel_without_wake`. Note refused
  while waiting: `ds` `worker_turns::a_note_while_waiting_for_approval_is_refused`.
  Added in Task 11: `a_note_queued_before_an_approval_reaches_the_resumed_turn`.
- **Same-turn guarantee.** `worker_turns::a_turn_through_the_worker_host_matches_the_shell_host`,
  plus `a_worker_turn_gets_the_shells_chat_integrations`, which catches what
  that comparison cannot see (Task 6).
- **Progress file.** `worker_progress.rs`: `seq_strictly_increases_from_one`,
  `reading_after_n_returns_exactly_the_later_lines`,
  `a_seeded_secret_never_reaches_the_file`.
- **Key and token.** Ignored `ds/tests/worker_process.rs`:
  `a_command_the_worker_runs_sees_neither_key_nor_token` (environment) and
  `the_worker_keeps_key_and_token_out_of_argv_and_output` (argv).
- **Stdin close.** Ignored `closing_stdin_exits_the_worker_and_kills_its_sleeping_child`.
  It spawns a real process, so it is ignored rather than "a normal test" as
  planned.
- **Socket.** `worker.rs`: `the_socket_is_0600_in_a_0700_directory`,
  `a_wrong_or_missing_token_closes_the_connection`,
  `an_unknown_command_is_rejected`.
- **Command approval.** `worker_turns::a_command_approval_continues_in_process`
  (snapshot present while waiting, gone after, same pid). Killed while
  waiting: ignored `killing_a_worker_waiting_for_approval_leaves_a_snapshot_recovery_finds`.
- **Patch proposal.** `worker_turns::a_patch_proposal_leaves_the_worker_idle_and_alive`.
- **Creation.** `ws/tests/worktree.rs`:
  `a_worktree_is_created_under_damaian_worktrees_on_a_damaian_branch`,
  `the_exclude_line_is_added_once`, `colliding_names_get_matching_suffixes`.
- **Refusals.** `refuses_a_symlink_anywhere_in_the_path`,
  `refuses_a_tracked_path_under_worktrees`,
  `refuses_a_path_that_resolves_outside_the_repository`,
  `refuses_a_non_git_source`. An existing path: naming skips it, so it is
  never reused (`colliding_names_get_matching_suffixes`). The refusal behind
  naming has no test of its own, because naming never hands it an existing
  path.
- **Include / last commit.** `include_carries_tracked_edits_and_untracked_files`,
  `start_from_last_commit_carries_neither`.
- **`worker_spawned` records.** `ds` `worker_supervisor::spawn_records_worker_spawned_on_worker_parent_and_audit`.
- **Restart refusal.** `worker_supervisor::restart_is_refused_while_a_live_worker_holds_the_worktree`.
- **Parent deleted.** `worker_supervisor::deleting_a_parent_sends_shutdown_to_its_workers_and_keeps_their_worktrees`.
- **Dirty removal.** `worker_supervisor::remove_worktree_requires_the_second_confirmation_when_dirty`
  and `worktree.rs::removing_a_dirty_worktree_reports_paths_and_does_not_force`.
- **Kill test.** Ignored `ds/tests/workers.rs::sigkilling_the_shell_ends_its_worker_and_the_workers_command`,
  added in Task 11. Run 2026-10-16: the worker and its `sleep` command were
  gone 44 ms after the shell's `SIGKILL`. Falsified: with the lifeline's EOF
  ignored it fails at 5 s.
- **UI.** Each item is a recorded manual check in the running shell on port 4799
  with its own data directory. Task 8's row: the tag rule through both
  renderers (untagged, `text`, `prompt`, `rust`, `worker-prompt extra`,
  `Worker-Prompt`, a user message's block, while streaming, in a worker
  session, setting off), the exact pre-fill, the dialog clean and dirty, and
  the "Started" link and entry. Task 9's row: board groups and ordering, *Stop*
  in the overflow, and setting off (no board, no sidebar entries, no button).
  Task 10's row: the strip in each state, a note while running and while
  paused, approval with the same pid, and Stop.
- **Setting and instruction.** `chat.rs`:
  `code_mode_system_prompt_is_byte_identical_to_todays` (unmodified),
  `no_mode_changes_when_handoffs_are_not_offered`,
  `the_handoff_paragraph_is_appended_when_offered`,
  `a_non_worker_session_gets_the_paragraph_when_enabled`,
  `a_worker_session_never_gets_the_paragraph`;
  `ws/tests/repository_config_trust.rs::repository_config_cannot_enable_worker_sessions`.
- **Also.** The deterministic eval tier ran 16/16 in Task 6, when turn hosting
  changed. Task 11 changed only worker bookkeeping, and the tier also runs
  in the workspace test suite on CI. The full seven-check gate runs on CI
  (`AGENTS.md`).
- **C12** (a live worker is not crash debris): `ws/tests/crash_recovery.rs::a_live_worker_session_is_not_recovered`,
  `ds` `recovery.rs::a_live_worker_is_not_offered_for_recovery`.

**Cold start**, informational: ignored `workers.rs::worker_cold_start_on_this_repository`,
a debug build on the development Mac, 2026-10-16. Source: a local clone of this
repository (333 tracked files). The model was `DAMAIAN_MOCK_MODEL_RESPONSE`, so
no provider time is counted. In three runs, `spawn` returned in 143–163 ms
(worktree, records, process), the socket answered at 169–406 ms, and the first
model request was prepared at **7.3–7.6 s**. Nearly all of it is the worker
building its index of a worktree it has never seen. No index is shared with
the source, which is the cost of a process per worker (§4).

**Acceptance, run 2026-10-16 with DeepSeek (`deepseek-v4-flash`), not met on
its first clause.** It was run in a scratch shell on port 4799 with its own data
directory, a throwaway key the user put in the Keychain, Worker sessions on, and
a fixture repository with no `AGENTS.md`. Four requests to split two tasks
(a `--reset` flag, a README usage section):

1. "Split this work into separate tasks … add a --reset flag … add a Usage
   section to README.md": the web UI's `looksLikeEditRequest` (an edit verb
   plus a code word) sent it to the one-shot patch flow (`/api/propose-edit`),
   which has its own prompt without the §5.0 paragraph. It ended "Generated
   edit did not contain any file changes".
2. Reworded without those words: a chat turn, which carries the paragraph.
   The model wrote the plan as a patch adding `TASKS.md`.
3. and 4. Asked not to touch the repository and to hand each task to its own
   session: the model wrote two handoff prompts each time, as a bare fence
   with `worker-prompt` on the first line *inside* it, not as the fence's info
   string. Damaian matches the info string exactly (§5.1), so neither answer
   had a *Start worker* button.

Nothing in the engine rewrites fences, so this is the model's formatting. With
the setting turned off, the UI showed no Workers entry, board, dot or button,
and `/api/workers-enabled` answered false. No worker was started from a real
model's answer, so *Start worker* → confirm on a model-written block is shown
only against a scripted answer (Task 8). Two findings follow, §7 items 10 and
11. Each answer was also one sample, from one provider.

**Acceptance, re-run 2026-10-16 with DeepSeek (`deepseek-v4-flash`) after
Task 12: met.** Same set-up as above (port 4799, its own data directory, a
throwaway key the user put in the Keychain, a fresh fixture with no
`AGENTS.md`), with the two fixes: the §5.0 paragraph now shows the literal
opening fence (item 10), and with Worker sessions on `looksLikeEditRequest`
leaves planning requests to the chat turn (item 11).

1. Request 1's exact wording ("Split this work into separate tasks that can
   run in parallel: add a --reset flag to main.rs, and add a Usage section to
   README.md") went to `/api/ask-stream`, not `/api/propose-edit`. The one
   answer held two blocks rendered as `language-worker-prompt`, each with a
   *Start worker* button.
2. *Start worker* on the README block opened the dialog with the block's text.
   Confirming started a worker on `damaian/add-a-usage-section-to-readme-md-in-this`
   in `.damaian/worktrees/…`, as process 52345. It appeared on the board, in
   the sidebar under its parent and in the parent's "Spawned worker" line.
3. After an unapproved `ls -la`, it stopped for approval on each command it
   proposed (`high`, "cannot run in sandbox mode"), shown as *Needs you* /
   *Waiting for approval*. After the first approval it was still process
   52345 (checked by pid). Three more approvals followed in the same turn,
   with no *Interrupted* state.
4. *Pause*, clicked as soon as a round started, took effect at its boundary
   (*Paused*, round 4). A note sent while paused reached the model, which
   acted on it in round 5 by rewriting `README.md` as the note asked.
5. *Stop* during an approval ended it (`exited`, *Finished*). 52345 was gone,
   and the worktree held the steered README. The source tree was unchanged.
6. A second worker was started from the other block (process 55341, with a
   model request in flight as `curl` 55435). `SIGKILL` of the shell's own pid
   left neither the worker nor the `curl` running 0.5 s later. Its worktree
   and branch were kept, as §5.3 says.
7. Restarted with the setting off, `/api/workers-enabled` was false, and
   neither the Workers entry nor a *Start worker* footer was shown. The same
   blocks rendered as ordinary code blocks, and request 1's wording routed to
   the patch flow again, as before Task 12.

Also seen, not investigated (outside this spec): the worker's
`cat > README.md <<'EOF'` was stored, approved and run as that first line only
(the audit's `command` field), so the heredoc's body was lost and the file
came out empty. This was one sample, from one provider.

## 7. Follow-up specs, not part of this one

In the order agreed during design:

1. **Loop and no-progress signals** — flags on the board for a repeated identical
   tool call, rounds that change no file and read nothing new, and the same
   failing command re-run. Needs false-positive tuning against the eval harness.
2. **Structured handoffs** — a `propose_worker` tool, so the planner emits a
   titled worker proposal as its own card instead of a fenced block, and
   `AGENTS.md` can require handing off that way. The card's action opens this
   spec's dialog pre-filled, exactly as a prompt block does.
3. **Opt-in result summary** — a parent may read a finished worker's final
   status and closing summary, never its transcript, through the same
   redaction as everything else leaving a session.
4. **Serve worker routes during a stream** (C10) — the shell's accept loop is
   held for the whole of an `/api/ask-stream` turn, so the board and the
   progress polls wait until it ends. Answering the read-only worker routes
   while a turn streams would remove the lag.

Smaller open items, recorded so they are not lost (Task 11):

5. **Partial git commands on a failed creation** (Tasks 4 and 7, (b)). A failure
   after `git worktree add` leaves the worktree and branch in place, and
   `worker_spawn_failed` records only the error. `create_worker_worktree` would
   have to return the commands it ran with its error, or remove what it made.
6. **A note accepted in a turn's final round** is not delivered (§5.4). The
   worker could report it as undelivered in the job's closing `status` line,
   or carry it into the next turn. The choice is a design decision.
7. **The board's row facts** (Task 9 discoveries 1–2). A `phase` line's empty
   label for the context and model phases makes *Doing* read "Working". A
   finished row has no end time; a `lastProgressAtMs` on the row would give it
   one.
8. **A worker's model** (Task 6 (a)). A worker takes the configured provider and
   model, not the one chosen in the parent's composer.
9. **The *Include* base commit** (Task 4 (a)). `git stash create` makes a
   two-parent commit, which merging the worker's branch carries into history.
   `commit-tree <stash>^{tree} -p HEAD` would give a single-parent base.

Found by the acceptance run (§6), and blocking its first clause. These two were **not** follow-ups: they were Task 12 of this spec, which resolved both and re-ran acceptance (decided 2026-10-16):

10. **The handoff's exact form.** DeepSeek put `worker-prompt` on the first line
    inside a bare fence, twice out of two handoffs. Either the paragraph shows
    the literal opening fence (```` ```worker-prompt ````), or *Start worker*
    also accepts a bare fence whose first line is exactly `worker-prompt` and
    drops that line from the prompt. The second loosens the "no button on the
    wrong thing" rule (§4), so it is a decision, not a fix.
    **Resolved in Task 12 (user's decision):** the paragraph shows the
    literal fence, and matching stays exact.
11. **Planning prompts routed to the patch flow.** `looksLikeEditRequest`
    (`app.js`) sends any prompt with an edit verb and a code word to
    `/api/propose-edit`, whose prompt has no §5.0 paragraph. "Split adding X
    to the README into tasks" is such a prompt. Either that flow gets the
    paragraph too, or the heuristic leaves planning requests to the chat
    turn. This predates spec 57 and affects every provider.
    **Resolved in Task 12 (user's decision):** with Worker sessions on, a
    prompt saying *split*, *plan*, *tasks*, *hand off*, *parallel*, *worker*
    or *session* is not an edit request. With the setting off, routing is
    unchanged. The patch flow keeps its own prompt, since it can only answer
    with a diff.
