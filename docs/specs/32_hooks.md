# Feature Spec: Hooks

Status: Not started
Order: 32 of 33
Roadmap: `docs/ROADMAP/04_phase_4_customization_and_extensibility.md`, Phase 4,
Work Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.6 (tool and action orchestrator), section 7.8 (risk
classification and approval), section 11 (error handling). Related implementation
specs: [`10_persistent_command_approval.md`](10_persistent_command_approval.md),
[`13_docker_command_support.md`](13_docker_command_support.md),
[`17_durable_task_state_and_crash_recovery.md`](17_durable_task_state_and_crash_recovery.md)
(the PID registry hook processes use, and the action markers hooks sit between),
[`20_working_modes.md`](20_working_modes.md),
[`21_task_plan_progress_and_budget.md`](21_task_plan_progress_and_budget.md),
[`22_findings_model_and_panel.md`](22_findings_model_and_panel.md) (hooks return
`Finding`s), [`23_verification_loop.md`](23_verification_loop.md),
[`31_permission_profiles.md`](31_permission_profiles.md) (the profile a hook
cannot widen).

## 1. Motivation

Damaian enforces the rules it knows about. It has no way to enforce a rule it
does not.

"Never touch `migrations/`." "Always run `cargo fmt` before you call a task
complete." "Refuse any patch that adds a dependency without me seeing it." These
are local, specific, and correct for one repository, and none of them is
expressible today. There is no lifecycle event surface at all — no point where a
user's own check can run and say no.

The alternatives users are left with are all worse. `AGENTS.md` can *ask* the
agent to follow a rule, but an instruction is a request the model may forget or
reason around; [spec 20](20_working_modes.md) §5.5 is explicit that repository
instructions are data, not capability. A `restricted_patterns` entry can block a
path but cannot express "not without asking me" or "only if the tests pass".

Hooks are the mechanism that closes this, and the roadmap's reasoning for
Must-tier is worth keeping: unlike skills, hooks **compose with policy rather
than competing with it**. A hook can only deny, narrow, or request — never
widen. That single constraint is what makes an extension surface safe enough to
be the phase's required work rather than its risky work.

## 2. Current State

- **There is no lifecycle event surface.** No hook registry, no dispatch point,
  no configuration for one.
- **The points hooks need to attach to exist**, which is what makes this
  tractable:

| Event | Existing attachment point |
|---|---|
| `before_context` / `after_tool` | `ContextManager::build_context` (`context_manager.rs:66`); the tool-call loop in `chat.rs:740` |
| `before_tool` | The same loop, where tool calls are dispatched |
| `before_patch_apply` / `after_patch_apply` | `PatchEngine::apply_patch` (`patch_engine.rs:376`) |
| `after_check` | `ValidationOrchestrator::run_proposal` (`validation.rs:186`) |
| `session_start` / `session_end` | `SessionStore::create_session` (`session.rs:79`); session teardown |
| `before_task_complete` | Task completion in `chat.rs:1205-1230` |

- **Command execution is synchronous and reaped.** `CommandRunner` uses
  `Command::output()` (`command_runner.rs:89-93`), which blocks until exit —
  there is no timeout mechanism, which §5.4 has to supply.
- **Child-process spawning has two precedents**: MCP stdio transport
  (`mcp.rs:288-345`) and the `curl` model child wrapped in `KillOnDrop`
  (`model.rs:400-406`). [Spec 17](17_durable_task_state_and_crash_recovery.md)
  §5.7 defines the session-scoped PID registry both should use.
- **`Finding`** ([spec 22](22_findings_model_and_panel.md)) is the structured
  type a hook returns, with `SecretScanner` redaction applied at construction.
- **`AuditLog::record`** (`audit.rs:42`) redacts every field on the way in and is
  the mechanism to reuse rather than extend.
- **Config layering and its scope trust boundary** are defined in
  [spec 31](31_permission_profiles.md) §5.2-5.3. Hook configuration is subject to
  it.
- **The output truncation and redaction path** for command output already exists
  and is what hook output should reuse.

## 3. Requirements

1. Every hook has a strict schema and an enforced timeout.
2. Ordering is deterministic, documented, and stable across runs.
3. Hooks can be enabled and disabled individually.
4. Recursion protection: a hook that triggers the event it handles must not
   loop.
5. Output is bounded and redacted through `SecretScanner`.
6. **Hooks cannot expand the active mode or the permission profile.** A hook may
   only deny, narrow, or request; never widen.
7. A failing *optional* hook produces a visible warning and the action proceeds.
   A failing *mandatory policy* hook blocks the action — fail closed, never fail
   open.
8. Every invocation and outcome is audited.

Lifecycle events: `session_start`, `before_context`, `before_tool`, `after_tool`,
`before_patch_apply`, `after_patch_apply`, `after_check`,
`before_task_complete`, `session_end`.

Permitted hook actions: allow; deny with a reason; request approval; add bounded
context; return a structured `Finding`; run an approved local command.

## 4. Non-goals

- Skills — Phase 4 WP1, Should-tier and outside this phase's minimum slice.
- An in-process plugin API, a scripting runtime, or WASM. A hook is an external
  program invoked with a documented contract; embedding an interpreter is a much
  larger surface for no gain here.
- Hooks that modify a patch, rewrite a command, or edit context. A hook returns a
  verdict and optional additions; it does not mutate the thing it inspects.
  §5.3 explains why.
- Remote or downloaded hooks. Hooks are local executables the user configured.
- Hooks on model requests or responses as such. `before_context` covers what is
  assembled; there is no hook that rewrites a prompt.
- Replacing the verification loop ([spec 23](23_verification_loop.md)).
  `after_check` observes check results; it does not schedule checks.
- Asynchronous or long-running hooks. §5.4 enforces short timeouts, and
  background work belongs to Phase 2 WP5.

## 5. Design

### 5.1 The contract

A hook is an executable invoked with a JSON event on stdin, returning JSON on
stdout:

```json
{
  "event": "before_patch_apply",
  "hookApiVersion": 1,
  "sessionId": "session_...",
  "taskId": "task_...",
  "mode": "code",
  "repositoryRoot": "/Users/…/work/app",
  "payload": {
    "patchId": "patch_...",
    "files": [{ "path": "migrations/003_add_index.sql", "status": "modified" }]
  }
}
```

```json
{
  "hookApiVersion": 1,
  "verdict": "deny",
  "reason": "migrations/ is applied by the release process, not by hand",
  "findings": [],
  "context": []
}
```

An external program rather than an in-process API, because it makes the trust
boundary a process boundary: a hook cannot reach into Damaian's state, cannot
call its policy functions, and cannot hold a reference to anything it might
widen. Requirement 6 is then structural rather than a rule the hook is trusted to
respect — the only thing a hook can do is return one of the verdicts below.

Payloads carry **paths, ids, and metadata, never file content**. A hook that
wants to inspect a file reads it itself, under the user's own filesystem
permissions, which keeps Damaian out of the business of deciding what content to
hand an external program.

### 5.2 Verdicts

| Verdict | Effect |
|---|---|
| `allow` | Proceed. The default when a hook returns nothing meaningful |
| `deny` | The action does not happen. `reason` is required and reaches the user |
| `request_approval` | The action becomes approval-gated even if policy would have run it automatically |
| `warn` | Proceed, surface the reason |

`request_approval` is the verdict that makes hooks genuinely useful without being
dangerous: a user can say "any patch touching `Cargo.toml` needs my eyes" without
blocking it outright. It can only move an action from automatic to gated, never
the reverse — there is deliberately no `approve` verdict, because that would be a
hook granting permission, which is precisely the widening requirement 6 forbids.

`findings` are `Finding` values ([spec 22](22_findings_model_and_panel.md)),
which puts hook output in the same panel as compiler and test output and makes it
navigable and repairable. `context` additions are bounded strings that enter
context assembly as ordinary `ContextItem`s in a dedicated category
([spec 26](26_context_assembly.md)) — not a privileged channel, subject to the
same budget and the same visibility in the inspector
([spec 27](27_context_inspector.md)).

### 5.3 Hooks return verdicts, not edits

A hook cannot modify the patch, command, or context it is inspecting. This is a
non-goal above; the reasoning belongs here because "let the hook fix it" is the
obvious next request.

A hook that rewrites a patch would produce a diff the user never previewed and
the model never proposed, applied under the user's approval of something else.
Every guarantee in [spec 04](04_hunk_level_patch_apply.md) and
`patch_engine.rs:291`'s hash check assumes the applied content is the previewed
content. A hook that wants a change made asks for it: `deny` with a reason feeds
the reason back to the model, which proposes a new patch through the normal
preview path.

### 5.4 Timeouts, and the absence of a mechanism

Requirement 1's enforced timeout has nothing to build on: `CommandRunner` uses
`Command::output()` (`command_runner.rs:89-93`), which blocks until the child
exits with no deadline. A hook that hangs would hang the turn.

So hook invocation spawns with piped stdio and waits with a deadline, killing the
child by PID on expiry — registered in the
[spec 17](17_durable_task_state_and_crash_recovery.md) §5.7 PID registry with its
start time, so a crash mid-hook does not leave the process behind and a recycled
PID is never killed by mistake.

Defaults are deliberately short — a couple of seconds for `before_*` events on
the critical path, longer for `after_*` — and configurable per hook. A hook on
`before_tool` runs on every tool call, so its budget is the one that decides
whether hooks are usable at all.

Timeout is a **failure**, handled by §5.5. It is not silently treated as `allow`.

### 5.5 Optional versus mandatory: the fail direction

Requirement 7 is the requirement most likely to be implemented backwards, because
the convenient default is the unsafe one.

```text
hook.<id>.mode=mandatory   # default
hook.<id>.mode=optional
```

| Classification | Non-zero exit, timeout, malformed output |
|---|---|
| **Mandatory** | The action is **blocked**. Fail closed |
| **Optional** | Warning shown, action proceeds. Fail open |

**Mandatory is the default.** A user who writes a hook to enforce a rule wants
the rule enforced when the hook is broken, not skipped. Defaulting to optional
would mean a typo in a hook script silently disables the protection it exists to
provide, which is the failure mode that makes a security control worthless.

`optional` is opt-in and is right for advisory hooks — a linter that emits
findings, a notifier. The distinction is per hook and shown in the hook list, so
"what happens if this breaks?" is answerable without reading the config.

A malformed response is a failure, not an `allow`. So is a response with an
unrecognised `verdict`, and so is a `hookApiVersion` the build does not
understand — a hook written against a newer contract is not assumed benign.

### 5.6 Ordering

Requirement 2. Hooks for one event run in a documented, stable order: by explicit
`order` value when set, then by hook id lexicographically. Same inputs, same
sequence, every run.

**A `deny` short-circuits.** Remaining hooks for that event do not run, because
the action is not happening and running further checks against it wastes the
budget. `request_approval` does not short-circuit — a later hook may still deny,
and a denial outranks a request. The resolution is fixed and documented:
`deny` > `request_approval` > `warn` > `allow`, independent of order, so the
outcome does not depend on which hook happened to run first.

### 5.7 Recursion

Requirement 4. A hook may run an approved local command, and a command triggers
`before_tool` — so a `before_tool` hook that runs a command re-enters its own
event.

The event context carries a hook depth, incremented on entry. Beyond a small
limit, hook dispatch is skipped for the nested action, the fact is recorded as a
`Finding`, and the outer invocation is reported as having exceeded depth.
Skipping dispatch rather than failing the action is the right trade: the nested
action is one the outer hook deliberately requested, and blocking it would make a
hook unable to run any command at all.

A hook's own command executions do **not** re-enter that hook, tracked by hook
id in the event context, so the common case — a hook running one check — needs no
depth budget at all.

### 5.8 Hooks cannot widen

Requirement 6, enforced in three places:

- **No widening verdict exists** (§5.2). The verdict enum has no `approve`.
- **`context` additions are ordinary context items**, budgeted and visible, not
  instructions. A hook returning "you may edit any file" adds a sentence to the
  context window and changes no policy — the same property that makes memory
  safe in [spec 30](30_memory_retrieval_and_lifecycle.md) §5.2.
- **A hook's own command runs under the session's mode and profile**, evaluated
  at execution as any other command is. A `before_tool` hook cannot run a
  command Code mode would refuse, and cannot run one in Ask mode at all.

An attempt to widen — an unrecognised verdict, a response field claiming
permission — is audited as a widening attempt, per the roadmap's acceptance
criterion, and treated as malformed output under §5.5.

Hook configuration is capability configuration, so it lives under
[spec 31](31_permission_profiles.md) §5.1's capability keys: a repository cannot
add a hook that the user has not reviewed, and it cannot disable a user's hook.
A repository *can* add a hook once reviewed — which is a useful thing for a
repository to ship, and is exactly why the review gate in
[spec 31](31_permission_profiles.md) §5.3 is itemised rather than global.

### 5.9 Output bounding and redaction

Requirement 5. A hook's stdout is read up to a byte ceiling and discarded beyond
it — a hook that produces unbounded output is a failure under §5.5, not a hook
whose output is quietly truncated into valid-looking JSON.

`reason` strings, `findings`, and `context` additions pass through
`SecretScanner` before display, persistence, or entry into context. `Finding`
already redacts at construction ([spec 22](22_findings_model_and_panel.md) §5.6),
and hook output uses that path rather than a parallel one.

A hook's stderr is captured, bounded, and redacted for diagnostics, and is never
fed to the model — it is for the user debugging their hook.

### 5.10 Audit

Requirement 8. Every invocation records hook id, event, verdict, duration,
classification, and outcome through `AuditLog::record`. Failures record the
failure kind — non-zero exit, timeout, malformed output, version mismatch — and
denials record the reason.

Recursion-depth events and widening attempts are audited distinctly, since both
indicate a misconfigured or hostile hook rather than normal operation.

### 5.11 Documentation

`docs/USER_GUIDE.md`: what hooks are, the events, how to write one, the verdicts,
and why mandatory is the default. `docs/TROUBLESHOOTING.md`: how to tell a hook
denial from a policy denial, where hook invocations appear in the audit log, how
to debug a failing hook using its captured stderr, and what a depth-exceeded
report means.

## 6. Acceptance Criteria

- A `before_patch_apply` hook can block an apply, and its reason reaches the
  user.
- A `deny` verdict short-circuits remaining hooks for that event.
- Verdict resolution is `deny` > `request_approval` > `warn` > `allow`
  regardless of hook order — asserted by running the same hooks in both orders.
- Ordering is deterministic across runs for the same configuration.
- A `request_approval` verdict makes an otherwise-automatic action
  approval-gated, and there is no verdict that makes an approval-gated action
  automatic.
- A mandatory hook that exits non-zero, times out, returns malformed output, or
  returns an unknown `hookApiVersion` **blocks** the action.
- An optional hook failing the same ways produces a warning and the action
  proceeds.
- Mandatory is the default classification for a hook with no `mode` set.
- A hook that hangs is killed by PID at its deadline, is registered in the PID
  registry with its start time, and leaves no process behind after a crash.
- A hook attempting to widen the mode or profile has no effect, and the attempt
  is audited as a widening attempt.
- A hook's own command runs under the session's mode and profile, and is refused
  in Ask mode.
- Recursive hook invocation is bounded, reported as a `Finding`, and the nested
  action is not blocked.
- A hook's command executions do not re-enter that same hook.
- Hook output containing a seeded fake secret is redacted before display,
  persistence, and entry into context.
- Hook output exceeding the byte ceiling is a failure, not a truncation.
- `context` additions appear in the context inspector
  ([spec 27](27_context_inspector.md)) within a bounded category, not as
  instructions.
- Hook configuration is a capability key: a repository cannot add a hook without
  review, nor disable a user's hook.
- Every invocation and outcome is audited, with failure kinds distinguished.
- An extension failure does not corrupt session state — asserted by failing a
  hook at each event.
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no regression.

## 7. Implementation Notes

To be completed during implementation. Record:

- The default timeout per event, and the measured overhead of `before_tool`
  dispatch with one trivial hook configured. That number decides whether hooks
  are usable on the critical path or only on `after_*` events.
- Whether the recursion depth limit was ever reached by a legitimate hook.
- Which events proved to have no clean attachment point. The table in §2 is
  derived from reading the call sites, not from having wired them, and
  `before_task_complete` in particular sits in a region of `chat.rs` with
  several exit paths — if any event had to be dropped or moved, say so here.
