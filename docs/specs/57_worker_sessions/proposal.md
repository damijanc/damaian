# Feature Spec: Worker Sessions

Status: Not started
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
(round budget), [`36_branch_and_worktree_delivery.md`](../36_branch_and_worktree_delivery.md)
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
  ordinary approved git. [#35](../35_commit_preparation.md)–[#37](../37_pull_request_creation.md)
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
`RepositoryKeyClass::Forbidden` ([`config.rs:70`](../../../crates/workspace-engine/src/config.rs)):
a repository must not be able to change how the user's sessions behave or add
text to their system prompt. Turning it off while workers are running does not
stop them; it hides the entry points for new ones, and the board stays
reachable until no worker is left on it.

**Instruction.** When the setting is on, `system_prompt`
([`chat.rs:3439`](../../../crates/workspace-engine/src/chat.rs)) appends one
fixed paragraph for every session that is **not** itself a worker:

> When work should continue in a separate session — because it is a
> self-contained task, or one of several that can run in parallel — write each
> task as a prompt in its own fenced block tagged `worker-prompt`. The user can
> start a worker from that block. A worker starts with a fresh context in its
> own git worktree and sees only the repository and that prompt, so say what to
> read, what to change, which checks to run, and when to stop. Do not use the
> `worker-prompt` tag for anything else.

The paragraph is Damaian's own text, not repository content. When the setting is
off, `system_prompt` returns exactly what it returns today; the existing test
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

**Naming.** A slug is taken from the first line of the prompt: lowercase ASCII
words joined by `-`, at most 40 characters, `worker` if empty. The branch is
`damaian/<slug>` and the worktree `<repo>/.damaian/worktrees/<slug>`. If either
exists, `-2`, `-3`, … is appended to both until neither does.

**Creation**, in order, refusing before any change if a check fails:

1. Refuse if the source is not a git repository.
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
   event to the parent (if any) carrying the worker's session id.
8. Record the same facts as a `worker_spawned` audit event.
9. Start the worker process (§5.2) with the prompt as its first message.

**Approval.** #36 requires action-specific approval for git mutations. Clicking
*Start worker* is that approval: the user initiated the action and the dialog
states what it does. The exact commands are not shown in the dialog, by the
user's decision, and are instead recorded in the audit log and shown as the
first entry of the worker's session. Repository configuration cannot start a
worker, choose its location, or skip this click.

**Removing a worktree** (board overflow, finished workers only):
`git worktree remove <path>`. If the worktree has uncommitted changes, a second
confirmation names the dirty paths first, following #36; `--force` is passed
only after that confirmation. The branch is never deleted.

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
reimplemented or bypassed.

**Control socket.** `<data_dir>/workers/<session-id>.sock`, a Unix domain socket
created with mode `0600` in a directory with mode `0700`. Line-delimited JSON
requests, one response each. Every request carries the token; a wrong or
missing token closes the connection. Commands:

| Command | Effect |
|---|---|
| `message {text}` | Starts a turn with this user message. Refused while a turn runs. |
| `pause` | Sets the pause flag (§5.4). |
| `resume {note?}` | Clears it, optionally queuing a note first. |
| `note {text}` | Queues a steering note. Refused while waiting for approval. |
| `approve {proposalId}` / `reject {proposalId}` | Resolves a pending command or plan approval (§5.6). |
| `stop` | Cancels the current turn at the next safe point. |
| `shutdown` | Stops any turn, kills child processes, exits. |
| `status` | Returns the current snapshot. |

Unknown commands are rejected. The shell stays single-threaded: each exchange is
short, and the shell never holds a socket open.

**Registration.** The worker registers its PID with the #46 process registry,
tagged with its session id, and appends a `worker_started` event.

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
   `shutdown` to every worker, waits up to 5 seconds, then `SIGTERM`s any that
   remain by registry PID. Never by name. The Tauri host has no exit hook
   today; this adds one (`RunEvent::Exit`).
5. **Parent removed.** Deleting a session sends `shutdown` to every worker
   whose parent it is. (There is no archive action in the shell today; if one
   is added, it does the same.) Their worktrees and session logs are kept, and
   their sessions remain in the sidebar.
6. **Backstop.** #46's orphan sweep at the next launch kills any registered
   worker or worker child still alive.

What persists is records, not processes: the session log, the worktree, any
pending-approval snapshot. A worker whose process ended without finishing its
turn is marked `Interrupted` through #17, and its session offers *Restart
worker* (§5.7).

**Stop escalation** from the UI: `stop`; if the turn has not ended within 10
seconds, `shutdown`; if the process has not exited 5 seconds later, `SIGTERM` to
its registry PID.

### 5.4 Pause and steering notes

`TurnSink` gains one field, `steering: Option<&'a TurnSteering>`, beside the
existing `cancel`. `TurnSteering` holds a pause flag, a queue of notes and a
condition variable, and is woken by cancellation too. Every existing host —
the shell, the eval harness, tests — passes `None`, so both hosts run identical
turn code and only the worker ever pauses.

At the existing round-boundary check, before the next model request
([`chat.rs:1574`](../../../crates/workspace-engine/src/chat.rs)):

1. If cancelled, behave exactly as today.
2. If paused: persist `TaskStatus::Paused` (new variant), emit a `status`
   progress event, and wait on the condition variable until resumed or
   cancelled. Cancelled while paused ends the turn as `Cancelled`.
3. Drain the note queue. Each note is appended to the conversation and to the
   session log as an ordinary user message (`append_message`, role `user`),
   followed by a `steering_note_added` session event naming that message's id —
   `ChatMessage` has no origin field, and adding one would change every
   message's serialized form for one marker. The note is sent to the model with
   the next request like any user message, through the same redaction and the
   composer's length limit. Emit a `note` progress event.
4. If the turn was paused, persist the running status again and continue.

A pause requested during a model call or a tool takes effect at the following
boundary; a running tool or command is never interrupted by pause. A turn
waiting for approval has already stopped its loop, so pause and notes do not
apply there.

`Paused` is a persisted, recoverable state. If the process dies while paused,
#17 reports the task `Interrupted` and the queued notes are lost with it; notes
already drained are in the session log.

### 5.5 The progress file

`<data_dir>/sessions/<id>.progress.jsonl`, one JSON object per line, each with
`seq` (strictly increasing from 1 within the file) and `timestampMs`:

| `kind` | Carries |
|---|---|
| `status` | `TaskStatus` |
| `phase` | `phase`, `label`, `round`, `maxRounds` — today's SSE payload |
| `plan` | today's `plan` payload |
| `text` | model output text, batched about every 250 ms |
| `tool` | the tool label of a `phase` event of kind `tool` — what the shell already shows while a tool runs. Durations are the gaps between events; outcome is not recorded here (the audit log has it) |
| `approval` | the pending proposal, as the approval card needs it |
| `note` | a steering note was added |

Every value passes through the same secret redaction as the session log before it
is written. The file is truncated when a worker process starts, and deleted with
the session. The audit log stays the authoritative record of tool calls; this
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
- The board polls `GET /api/workers?repository=<id>` about once a second while
  visible.

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

**Parent session.** Prompt blocks carry *Start worker* (§5.1). Once a block has
been used, its footer also shows "Started *‹worker title›* →", linking to the
worker, so a second click is a visible, deliberate second worker rather than an
accident. A one-line, quiet entry is appended where the worker was spawned:
"Spawned worker *‹title›* →". No live card.

**Shell routes:** `POST /api/workers` (spawn), `GET /api/workers`,
`GET /api/worker-progress`, `POST /api/worker-control`
(`{sessionId, command, ...}`, relayed to the socket),
`POST /api/worker-restart`, `POST /api/worker-remove-worktree`. Each answers
from files or one short socket exchange.

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
