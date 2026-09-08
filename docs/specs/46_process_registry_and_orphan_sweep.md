# Feature Spec: Process Registry and Orphan Sweep

Status: Not started
Order: 46 of 46
Roadmap: `docs/ROADMAP/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 2 (Must) — the process-cleanup half. That directory is local-only and
not committed, so the reference is a name rather than a link; this spec is
self-contained.
Related implementation specs:
[`17_durable_task_state_and_crash_recovery/`](17_durable_task_state_and_crash_recovery/proposal.md)
(split from it; runs its sweep at the same launch-time recovery point),
[`06_mcp_support.md`](06_mcp_support.md) and
[`33_mcp_management_and_deferred_discovery.md`](33_mcp_management_and_deferred_discovery.md)
(MCP stdio servers), [`12_web_app_troubleshooting.md`](12_web_app_troubleshooting.md).

## 1. Motivation

Split out of spec 17 before implementation. It shares no code with the state
machine, touches three unrelated subsystems, and its correctness turns on a
different question entirely: whether a recorded PID still belongs to the process
that was recorded.

That question is what makes it worth its own spec. **A PID is reused.** Killing a
stranger's process because Damaian crashed is a worse bug than the leak this
work exists to fix, so the sweep needs evidence of identity, not just a number.

## 2. Current State

The original spec 17 analysis, carried over verbatim:


Requirement 9 needs re-aiming: **commands cannot orphan a process.**
`CommandRunner` uses `Command::output()` (`command_runner.rs:89-93`), which
blocks until the child exits and reaps it. A crash during a command leaves the
child parented to `launchd`, but Damaian never held a PID to kill and the
command's outcome is unknown regardless — that is `unknown_external_outcome`,
not a cleanup problem.

The three real sources are:

| Source | Current state | Change |
|---|---|---|
| MCP stdio servers (`mcp.rs:323`) | Spawned, no kill-on-drop | Record PID in a session-scoped process registry file at spawn; kill by PID at recovery |
| `curl` for model calls (`model.rs:400`) | `KillOnDrop` — safe on graceful drop, orphaned on `SIGKILL` | Same registry, same recovery sweep |
| PTY sessions (`terminal.rs:31`) | Process-global map, lost on crash | Same registry |

The registry is a file under the data directory holding PID, spawn time, and the
owning session, written at spawn and removed on clean exit. At launch, a
recorded PID is killed **only** when it is still alive and its start time
matches what was recorded — a PID is reused, and killing a stranger's process
because Damaian crashed is a worse bug than the leak. This is the mechanical
form of the `AGENTS.md` rule against matching by name.


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

## 4. Non-goals

- Classifying what a crashed action did. That is
  [spec 17](17_durable_task_state_and_crash_recovery/proposal.md); a killed
  command's outcome is unknown whether or not its child was reaped.
- Background or long-running processes as a feature — Phase 2 WP5.
- Supervising or restarting anything. This sweep only cleans up.

## 5. Design

To be written when the work starts. Two things to settle first, both verified
rather than assumed:

- **How to read a process start time on macOS**, and whether it is stable enough
  to compare against a recorded value. This is the load-bearing mechanism and
  the reason the spec exists; if it cannot be read reliably, the design changes.
- **Whether `CommandRunner` really cannot orphan**, per the §2 analysis. It uses
  `Command::output()`, which reaps — but that was read from the code, not
  observed under `SIGKILL`.

## 6. Acceptance Criteria

- A recorded PID whose start time no longer matches is **not** killed — asserted
  by test, and the refusal is audited.
- No MCP server, `curl`, or PTY process recorded by a crashed session is still
  running after recovery.
- A clean exit leaves no registry entry.
- Anything that spawns or kills a real process is `#[ignore]`d with instructions
  for running it by hand, per `AGENTS.md`.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation.
