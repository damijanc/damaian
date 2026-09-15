# Feature Spec: Process Registry and Orphan Sweep

Status: Not started
Order: 46 of 46
Plan: `docs/PLAN/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 2 (Must) — the process-cleanup half. That directory is local-only and
not committed, so the reference is a name rather than a link; this spec is
self-contained.
Depends on: [#17](../17_durable_task_state_and_crash_recovery/proposal.md) (the
session scope a registry is keyed on) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`17_durable_task_state_and_crash_recovery/`](../17_durable_task_state_and_crash_recovery/proposal.md)
(split from it; its sweep runs at launch, earlier than spec 17's — see §5.7),
[`06_mcp_support.md`](../06_mcp_support.md) and
[`33_mcp_management_and_deferred_discovery.md`](../33_mcp_management_and_deferred_discovery.md)
(MCP stdio servers), [`12_web_app_troubleshooting.md`](../12_web_app_troubleshooting.md).

## 1. Motivation

Split out of spec 17 before implementation. It shares no code with the state
machine, touches three unrelated subsystems, and its correctness turns on a
different question entirely: whether a recorded PID still belongs to the process
that was recorded.

That question is what makes it worth its own spec. **A PID is reused.** Killing a
stranger's process because Damaian crashed is a worse bug than the leak this
work exists to fix, so the sweep needs evidence of identity, not just a number.

## 2. Current State

The original spec 17 analysis claimed **commands cannot orphan a process**,
reasoning that `CommandRunner`'s `Command::output()` (`command_runner.rs:89-93`)
blocks until the child exits and reaps it, so Damaian never holds a PID to kill.

**That was read from the code and is wrong**, which is why §5 required measuring
it. `output()` reaps on the *normal* path only. A process doing
`Command::output()` on `sh -c "sleep 120"`, sent `SIGKILL`, leaves the child
running and reparented to `launchd`:

```
before:  1061 (parent)  →  1063 sleep
after:   survivor pid=1063 ppid=1
```

Holding no PID is a consequence of `output()` hiding it, not a property of
commands. `spawn()` + `wait_with_output()` exposes the same PID at no
behavioural cost, so commands join the other three sources.

| Source | Current state | Change |
|---|---|---|
| MCP stdio servers (`mcp.rs:323`) | Spawned, killed on `Drop`, orphaned on `SIGKILL` | Record identity in a process registry file at spawn; kill at recovery |
| `curl` for model calls (`model.rs:513`) | `KillOnDrop` — safe on graceful drop, orphaned on `SIGKILL` | Same registry, same recovery sweep |
| PTY sessions (`terminal.rs:31`) | Process-global map, lost on crash | Same registry |
| Shell commands (`command_runner.rs:89`) | `Command::output()`, orphaned on `SIGKILL` as measured above | `spawn()` + `wait_with_output()`, same registry |

The registry is a set of files under the data directory holding PID, start time,
process group and the owning session, written at spawn and removed on clean
exit. At launch, a recorded PID is killed **only** when it is still alive and
its start time matches what was recorded — a PID is reused, and killing a
stranger's process because Damaian crashed is a worse bug than the leak. This is
the mechanical form of the `AGENTS.md` rule against matching by name.


## 3. Requirements

1. Processes owned by a crashed session are cleaned up. PIDs are tracked at
   spawn and killed by PID — never by process name, per `AGENTS.md`.
2. A recorded PID is killed **only** when it is still alive *and* its start time
   matches what was recorded.
3. The registry survives a `SIGKILL` of the owning process — it is written at
   spawn, not at exit.
4. A registry entry for a process that exited cleanly is removed.
5. Sweep decisions, including refusals to kill on a start-time mismatch, are
   recorded through `AuditLog::record`.
6. A sweep never touches a process owned by a Damaian instance that is still
   running. Two instances can run at once, so "recorded but not mine" is not
   evidence of a crash.

## 4. Non-goals

- Classifying what a crashed action did. That is
  [spec 17](../17_durable_task_state_and_crash_recovery/proposal.md); a killed
  command's outcome is unknown whether or not its child was reaped.
- Background or long-running processes as a feature — Phase 2 WP5.
- Supervising or restarting anything. This sweep only cleans up.

## 5. Design

Both of the questions this section was held open for were measured before
anything was designed. §2 records the `CommandRunner` result, which changed the
scope; §5.1 records the other, which did not change the design but is the
mechanism the whole spec rests on.

### 5.1 Reading a process start time

`proc_pidinfo(pid, PROC_PIDTBSDINFO, …)` fills a `proc_bsdinfo` whose
`pbi_start_tvsec` and `pbi_start_tvusec` give the start time to the microsecond.
It is one `libc` call — `libc` is already in the dependency tree under an
allowed license — and it is the only `unsafe` in this work.

Measured on macOS 25.6 rather than assumed:

| PID state | Result |
|---|---|
| Live child we own | A stable value, identical across repeated reads |
| Reaped / dead | Nothing |
| Zombie (killed, not yet reaped) | Nothing |
| Live but owned by another user (`launchd`, pid 1) | Nothing |

The last three rows are why this primitive was chosen over parsing `ps`, which
reports whole seconds and would need a policy for each of them. Every state that
is not *provably a live process of ours* collapses to the same answer, and that
answer is the safe one: do not kill. The fail-closed direction falls out of the
API instead of being imposed on top of it.

`ProcessIdentity { pid, start_time_us }` wraps this, with
`ProcessIdentity::of(pid) -> Option<Self>`. `None` means dead, a zombie, or a
stranger, and the sweep never distinguishes them because it treats all three
identically.

### 5.2 One file per process

`<data_dir>/processes/<pid>-<start_time_us>.json`, created with
`create_new(true)` *before* `register` returns, and unlinked when the process
exits cleanly:

```json
{"pid":1063,"startTimeUs":1789393267792219,"pgid":1063,"kind":"command",
 "sessionId":"ses_…","registeredAtMs":…,
 "ownerPid":1061,"ownerStartTimeUs":1789393266566459}
```

A file per process rather than the single append-only log used for sessions and
the audit trail, because this file is written by *concurrent Damaian instances*
and a shared log would have one instance compacting while another appends. With
a file per process there is no shared mutable state at all: each writer creates
and unlinks only its own paths. It also makes requirement 4 a literal unlink
rather than a tombstone record plus a compaction pass.

The filename carries both halves of the identity, so a recycled PID cannot
collide with the entry it recycled.

Requirement 3 is satisfied by construction: the file exists before the child is
returned to its caller, so there is no window in which a process is running and
unrecorded, and nothing needs to be written at exit for the record to survive.

### 5.3 The owner, and why the sweep is not "everything at launch"

Each entry records the identity of the Damaian process that spawned it, not just
the owning session.

A session is the wrong key. Two Damaian instances can run at once — `AGENTS.md`
treats a busy port 4765 as evidence the user's own app is running — and a sweep
keyed on sessions would kill the live instance's MCP servers and terminals on
the other instance's launch. That is the same class of bug as killing a stranger
after PID reuse, just one level up, so it gets the same test: an entry is only
considered when its **owner** is provably gone, by exactly the `ProcessIdentity`
comparison §5.1 applies to the children.

### 5.4 The sweep

`ProcessRegistry::sweep` reads `processes/` and, per entry:

| Condition | Action | Audit event |
|---|---|---|
| File does not parse | Unlink | `process_registry_entry_unreadable` |
| Owner alive and start time matches | Leave the file, skip | none |
| Child identity absent | Unlink | `orphan_process_already_exited` |
| Child alive, start time **differs** | **Refuse to kill**, unlink | `orphan_process_kill_refused` |
| Child alive, start time matches | Kill, unlink | `orphan_process_killed` |

An unparsable file is audited rather than ignored: a half-written entry means a
crash landed between `create_new` and the write, which is evidence about the
crash being recovered from, and the same reasoning `recovery.rs` applies to a
torn session-log tail.

The refusal records both start times. Requirement 5 asks for refusals to be
audited, and a refusal is only checkable after the fact if it says what it
compared.

Killing is `SIGTERM`, then `ProcessIdentity` polled for up to 200 ms, then
`SIGKILL`. A straight `SIGKILL` would satisfy the acceptance criterion just as
well, but a swept entry is frequently a shell command, and a `git` or `npm` part
way through writing the user's repository is worth the bounded delay. The
escalation means the criterion still holds unconditionally.

### 5.5 Process groups, gated on the leader

Killing a PID does not kill its children. `sh -lc "a | b"` leaves the pipeline;
an MCP server started through `npx` leaves the worker it exec'd. So every source
is spawned into its own process group (`process_group(0)`; the PTY child is
already a session leader, so `portable_pty` has done it), the leader's PID is
recorded as `pgid`, and the sweep kills the group.

The group is only killed when `pgid == pid` **and** that PID's identity matched.
Group ids are recycled exactly as PIDs are, so a group whose leader is gone may
already belong to someone else; gating on the leader means the widened blast
radius never outruns the identity evidence. Where the two differ the bare PID is
killed instead.

This costs something on one path. In an interactive `damaian` CLI session,
Ctrl-C currently reaches a running command because `sh -lc` runs without job
control, so the shell and its pipeline members all sit in Damaian's foreground
process group and every one of them gets the `SIGINT`. In its own group the
command gets nothing. §5.6 is what pays that back.

### 5.6 Shutdown on a caught signal

A drop guard does not cover this. `SIGINT`'s default action terminates without
unwinding, so `CommandRunner` gains a kill-on-drop guard like `curl`'s for
panics and ordinary returns, and that guard is irrelevant the moment a signal
arrives.

The exposure is narrower than it first looks, and was measured rather than
guessed. Child stdio is piped, so the kernel closes the read ends when Damaian
dies:

| Command | Outcome with no handler |
|---|---|
| Produces output while running | `SIGPIPE` on its next write, gone in well under a second |
| Silent (`sleep 300`, output redirected to a file) | Survives, reparented to `launchd` |

`cargo build`, `npm install` and `git` all stream progress and take themselves
out. The gap is real but it is the silent commands only.

It is closed with a self-pipe. `ProcessRegistry::install_shutdown_handler` is
called from `main` in the CLI and the shell binary, never from the library, so
a Tauri host and the test harness are unaffected.

- The handler, for `SIGINT`/`SIGTERM`/`SIGHUP`, does exactly one thing:
  `write` one byte to a pipe created at install time. `write` is on POSIX's
  async-signal-safe list, and nothing else in the path has to be.
- A watchdog thread blocks on the read end. When it wakes it is **ordinary
  code**, so it runs the same identity-gated kill as §5.4 — `proc_pidinfo`
  included. There is one implementation of "kill a registered process safely",
  shared by the launch sweep and by shutdown, rather than a second one written
  under signal constraints.
- The two differ in **which entries they consider**, and only that. The launch
  sweep takes entries whose owner is provably gone; shutdown takes entries whose
  owner is *this* process, which is alive by definition — §5.4's owner rule
  would otherwise skip every one of them. So the scope is a parameter and the
  per-entry identity check is not.
- It then restores `SIG_DFL` and re-raises, so the process reports the correct
  `WIFSIGNALED` status to whatever invoked it. A second signal therefore forces
  an immediate exit, so a wedged shutdown is always escapable.

The alternative — a preallocated static of group ids killed inside the handler —
was rejected because it would have to kill by *number*. `proc_pidinfo` is not
async-signal-safe, so the handler could not check identity, and this would
become the one place in the design that kills without evidence. That is the
exact bug §1 says is worse than the leak.

Registry entries are deliberately **not** unlinked at shutdown. `unlink` is
async-signal-safe and the watchdog could do it, but a killed process is gone, so
the next launch's sweep finds no identity for it and unlinks it as
`orphan_process_already_exited` anyway. Leaving the entry is also the honest
record if the watchdog is itself killed part way through.

Verified end to end before being written down: a `sh -lc "sleep 300"` in its own
group — precisely the silent case that survives a bare `SIGINT` — is cleaned up,
and the parent still exits signalled.

### 5.7 Where the sweep runs

Eagerly, in `run_server_with_ready` immediately after `verify_data_dir_schema`
and before the port is bound, and at CLI startup.

This spec's header originally placed the sweep at "the same launch-time
recovery point" as spec 17. That was inaccurate about the code that shipped:
`desktop_shell::recovery::sweep_once` is memoized on first
use and fires on the first HTTP request, not at launch. Folding this sweep into
it would leave orphans running for as long as the UI stays unopened, and would
couple a pass that needs no session log to one that is entirely a function of
it — against the seam the split was made along. Running it early is also what
makes it safe to run from both front ends: an entry whose owner is alive is
skipped, so an extra sweep is a no-op.

### 5.8 Wiring

| Source | Change | Call sites |
|---|---|---|
| MCP stdio (`mcp.rs:323`) | Own group, register after spawn; the existing `Drop` (`mcp.rs:395`) releases the handle. `McpClient::connect` takes a `&ProcessRegistry`. | 4 |
| `curl` (`model.rs:513`) | Own group, register; `KillOnDrop` carries the handle. `CurlModelTransport::new` takes a registry. | 11, of which 5 are tests |
| `CommandRunner` (`command_runner.rs:89`) | `output()` → `spawn()` + `wait_with_output()`, own group, register, kill-on-drop guard. Builds its own registry from `config.data_dir`. | 0 |
| PTY (`terminal.rs:48`) | `open` takes a data directory; `portable_pty::Child::process_id()` supplies the PID. | 2 |

### 5.9 Testing

`sweep` takes its identity lookup as an injected function. Every row of §5.4's
table — the refusal on a start-time mismatch above all, which acceptance
criterion 1 requires a test to assert — is then an ordinary unit test that
spawns nothing, needs no privileges, and runs in the default `cargo test`. The
decision logic is the part that must not regress, and it is the part that does
not need a real process to exercise.

Tests that do spawn or kill something are `#[ignore]`d with the manual command
in a doc comment, per `AGENTS.md`. Two end-to-end cases, both reusing the
re-exec harness already built for
`a_real_sigkill_mid_action_leaves_a_readable_log_and_an_unknown_outcome`:

- **Crash.** Register a real child, `SIGKILL` the owner, sweep from a fresh
  process, assert the child is gone.
- **Signal.** Register a silent child (`sleep`, so `SIGPIPE` cannot be what
  kills it and the test is actually measuring the handler), `SIGINT` the owner,
  assert the child is gone *without* a sweep and that the owner exited
  signalled.

The watchdog's own decision path needs no separate test: §5.6 routes it through
the same identity-gated kill as the sweep, so §5.4's table covers both.

## 6. Acceptance Criteria

- A recorded PID whose start time no longer matches is **not** killed — asserted
  by test, and the refusal is audited.
- No MCP server, `curl`, PTY or shell-command process recorded by a crashed
  session is still running after recovery.
- An entry whose owning Damaian instance is still running is left alone —
  asserted by test.
- A clean exit leaves no registry entry.
- `SIGINT` to the CLI leaves no registered process running, including one that
  produces no output — asserted by test, and the process still exits signalled.
- Anything that spawns or kills a real process is `#[ignore]`d with instructions
  for running it by hand, per `AGENTS.md`.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation.
