# Feature Spec: Hooks

Status: Done (started 2026-10-07; Tasks 1–10 of 10 done, the last on 2026-10-24). Split into a folder and planned on 2026-10-07. Design
unchanged from the original flat spec. Corrections where it no longer matches
the code, and the decisions it leaves open, are in [`context.md`](context.md),
not inlined here, the way specs 20, 22, 24 and 31 kept theirs. The most
important ones:
- `CommandRunner` already has a deadline, a process-group kill and a PID
  registry entry, so §2 and §5.4's "no timeout mechanism" is stale;
- `before_patch_apply`, `after_patch_apply` and `session_end` are not in
  `chat.rs`, and there is no session teardown, so `session_end` fires on
  deletion and only observes;
- what each verdict means at each event, including `request_approval` on an
  action that has no approval step (it denies);
- how hook definitions are classified per config scope, which §5.8 states
  only as an outcome.

The work is in ten tasks ([`tasks.md`](tasks.md)), all done. Only Tasks 7 and 8
edited `chat.rs`.
Order: 32 of 33
Plan: `docs/PLAN/04_phase_4_customization_and_extensibility.md`, Phase 4,
Work Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#17](../17_durable_task_state_and_crash_recovery/proposal.md)
(durable state) — built; [#22](../22_findings_model_and_panel/proposal.md) (hook findings)
— built; [#31](../31_permission_profiles/proposal.md) (the profile hooks cannot
widen) — built; [#46](../46_process_registry_and_orphan_sweep/proposal.md)
(the PID registry §5.4 attributes to #17, which moved there) — built.
Everything else named below is a cross-reference, not a prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.6 (tool and action orchestrator), section 7.8 (risk
classification and approval), section 11 (error handling). Related implementation
specs: [`10_persistent_command_approval.md`](../10_persistent_command_approval.md),
[`13_docker_command_support.md`](../13_docker_command_support.md),
[`17_durable_task_state_and_crash_recovery/proposal.md`](../17_durable_task_state_and_crash_recovery/proposal.md)
(the PID registry hook processes use, and the action markers hooks sit between),
[`20_working_modes/proposal.md`](../20_working_modes/proposal.md),
[`21_task_plan_progress_and_budget/proposal.md`](../21_task_plan_progress_and_budget/proposal.md),
[`22_findings_model_and_panel/proposal.md`](../22_findings_model_and_panel/proposal.md) (hooks return
`Finding`s), [`23_verification_loop/proposal.md`](../23_verification_loop/proposal.md),
[`31_permission_profiles/proposal.md`](../31_permission_profiles/proposal.md) (the profile a hook
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
reason around; [spec 20](../20_working_modes/proposal.md) §5.5 is explicit that repository
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
  (`model.rs:400-406`). [Spec 17](../17_durable_task_state_and_crash_recovery/proposal.md)
  §5.7 defines the session-scoped PID registry both should use.
- **`Finding`** ([spec 22](../22_findings_model_and_panel/proposal.md)) is the structured
  type a hook returns, with `SecretScanner` redaction applied at construction.
- **`AuditLog::record`** (`audit.rs:42`) redacts every field on the way in and is
  the mechanism to reuse rather than extend.
- **Config layering and its scope trust boundary** are defined in
  [spec 31](../31_permission_profiles/proposal.md) §5.2-5.3. Hook configuration is subject to
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
- Replacing the verification loop ([spec 23](../23_verification_loop/proposal.md)).
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

`findings` are `Finding` values ([spec 22](../22_findings_model_and_panel/proposal.md)),
which puts hook output in the same panel as compiler and test output and makes it
navigable and repairable. `context` additions are bounded strings that enter
context assembly as ordinary `ContextItem`s in a dedicated category
([spec 26](../26_context_assembly.md)) — not a privileged channel, subject to the
same budget and the same visibility in the inspector
([spec 27](../27_context_inspector.md)).

### 5.3 Hooks return verdicts, not edits

A hook cannot modify the patch, command, or context it is inspecting. This is a
non-goal above; the reasoning belongs here because "let the hook fix it" is the
obvious next request.

A hook that rewrites a patch would produce a diff the user never previewed and
the model never proposed, applied under the user's approval of something else.
Every guarantee in [spec 04](../04_hunk_level_patch_apply.md) and
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
[spec 17](../17_durable_task_state_and_crash_recovery/proposal.md) §5.7 PID registry with its
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
  safe in [spec 30](../30_memory_retrieval_and_lifecycle.md) §5.2.
- **A hook's own command runs under the session's mode and profile**, evaluated
  at execution as any other command is. A `before_tool` hook cannot run a
  command Code mode would refuse, and cannot run one in Ask mode at all.

An attempt to widen — an unrecognised verdict, a response field claiming
permission — is audited as a widening attempt, per the roadmap's acceptance
criterion, and treated as malformed output under §5.5.

Hook configuration is capability configuration, so it lives under
[spec 31](../31_permission_profiles/proposal.md) §5.1's capability keys: a repository cannot
add a hook that the user has not reviewed, and it cannot disable a user's hook.
A repository *can* add a hook once reviewed — which is a useful thing for a
repository to ship, and is exactly why the review gate in
[spec 31](../31_permission_profiles/proposal.md) §5.3 is itemised rather than global.

### 5.9 Output bounding and redaction

Requirement 5. A hook's stdout is read up to a byte ceiling and discarded beyond
it — a hook that produces unbounded output is a failure under §5.5, not a hook
whose output is quietly truncated into valid-looking JSON.

`reason` strings, `findings`, and `context` additions pass through
`SecretScanner` before display, persistence, or entry into context. `Finding`
already redacts at construction ([spec 22](../22_findings_model_and_panel/proposal.md) §5.6),
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
  ([spec 27](../27_context_inspector.md)) within a bounded category, not as
  instructions.
- Hook configuration is a capability key: a repository cannot add a hook without
  review, nor disable a user's hook.
- Every invocation and outcome is audited, with failure kinds distinguished.
- An extension failure does not corrupt session state — asserted by failing a
  hook at each event.
- Every quality-gate command from `AGENTS.md` passes, and the
  [spec 18](../18_local_evaluation_harness/proposal.md) baseline shows no
  regression.

## 7. Implementation Notes

Built in ten tasks, recorded in [`tasks.md`](tasks.md). The decisions that
override this proposal are in [`context.md`](context.md): `session_end` fires
on deletion and only observes, a verdict means something different at an event
with nothing to approve, the runner copies `CommandRunner`'s shape rather than
calling it, and hook configuration is a capability key with a review at a
definition hash.

### 7.1 Acceptance criteria

Numbered in §6's order. Tests are in
`crates/workspace-engine/tests/hooks.rs` unless another file is named.
Checked on 2026-10-24.

| # | Criterion | Evidence |
|---|---|---|
| 1 | A `before_patch_apply` hook can block an apply, and its reason reaches the user | `a_before_patch_apply_deny_blocks_the_apply_and_names_the_hook` (the file is unchanged, no `apply_patch` marker, no `stored_patch_applied` audit, the error names the hook and reason); `apply_patch_endpoint_reports_hook_denials_and_warnings` (`desktop-shell`, over HTTP: a 500 whose text carries the hook id and reason, then a warn hook applies and returns `hookWarnings`). The UI renders the error text it already shows for a failed apply |
| 2 | A `deny` verdict short-circuits remaining hooks for that event | `a_deny_short_circuits_the_hooks_after_it` (two real `/bin/sh` hooks; the second's marker file stays absent and only one `hook_invoked` is written) and `a_deny_short_circuits_and_nothing_else_does` (only `Deny` short-circuits; a request does not) |
| 3 | Resolution is `deny` > `request_approval` > `warn` > `allow` regardless of order — asserted by running the same hooks in both orders | `resolution_is_the_strongest_verdict_in_every_order`: every non-empty subset of the four verdicts in every permutation (all 24 orders of the full set), each also reversed and fed through the dispatcher's short-circuit loop. It resolves verdict values in both directions rather than spawning the same two hooks twice; `hooks_run_by_order_then_id_whatever_order_they_were_configured_in` covers the configured-order half |
| 4 | Ordering is deterministic across runs for the same configuration | `hooks_run_by_order_then_id_whatever_order_they_were_configured_in`: the same hooks configured forward, reversed and rotated yield `first, beta, mid, zeta, alpha`; disabled hooks and hooks for another event are excluded |
| 5 | `request_approval` makes an otherwise-automatic action approval-gated, and no verdict makes an approval-gated action automatic | `request_approval_before_tool_forces_an_automatic_command_to_a_card` (the stored proposal carries `requires_approval`, the card carries `ApprovalCause::HookRequested`, and `command_proposal_stored` records `hook_requested`); `request_approval_before_a_read_refuses_it`; `request_approval_does_not_turn_a_question_into_an_approval`; `request_approval_before_a_patch_proposal_is_a_warning`; `the_verdicts_are_exactly_the_four_and_rank_deny_highest` (no fifth variant; `approve`, `grant` and `allow_all` do not parse) |
| 6 | A mandatory hook that exits non-zero, times out, returns malformed output, or returns an unknown `hookApiVersion` blocks | `a_mandatory_failure_denies_and_an_optional_failure_warns` loops `non_zero_exit`, `timeout`, `malformed`, `version_mismatch`, `output_too_large` and `spawn_failed`; `a_hook_with_no_mode_fails_closed`; `a_timing_out_before_patch_apply_hook_blocks_or_warns_by_its_mode`; `a_mandatory_before_tool_timeout_blocks_and_an_optional_one_warns`; the state matrix fails every event three ways |
| 7 | An optional hook failing the same ways warns and the action proceeds | The same tests' optional branch: the outcome is `Warn`, a `FindingSource::Hook` warning finding exists, and the action proceeds |
| 8 | Mandatory is the default with no `mode` set | `a_user_hook_parses_with_defaults`, `a_hook_with_no_mode_fails_closed`, and `HookMode::default()` is `Mandatory` in `config.rs` |
| 9 | A hanging hook is killed by PID at its deadline, is registered with its start time, and leaves no process behind after a crash | `a_hook_past_its_deadline_is_killed_with_its_group_and_unregistered` (`sleep 30 &`/`wait` under a 300 ms deadline; the shell and its background child are both dead, the registry entry is gone); in-crate `hooks::tests::a_hook_left_running_by_a_crash_is_killed_by_the_sweep` (`mem::forget` the guard, `sweep_own` kills it). The registry records pid, start time and pgid |
| 10 | A hook attempting to widen has no effect, and the attempt is audited | `an_unknown_verdict_or_field_is_a_widening_attempt` (parser: names only); `an_unknown_verdict_is_audited_as_a_widening_attempt_and_denies` (`hook_widening_attempt` with `verdict=approve`, then treated as malformed); a `deny` cannot be turned into an allow because no such verdict exists |
| 11 | A hook's own command runs under the session's mode and profile, and is refused in Ask mode | `a_requested_command_runs_under_the_sessions_mode_and_is_bracketed_by_a_marker`; `a_requested_command_is_refused_in_ask_mode` (no proposal, no marker, `hook_command_refused`); `a_requested_command_that_needs_approval_is_refused_without_a_card_or_proposal` |
| 12 | Recursive invocation is bounded, reported as a `Finding`, and the nested action is not blocked | `recursion_stops_at_the_depth_limit_and_the_nested_command_still_runs`: two hooks that each request a command reach depth 2, six executions, two `hook_depth_exceeded` audits, a warning finding, and the nested commands still run |
| 13 | A hook's command executions do not re-enter that same hook | `a_requested_command_does_not_reenter_the_hook_that_asked` (one invocation, not three) and `a_hook_that_denies_a_requested_command_stops_it`. `HookContext.excluded` carries the requesting id and is honoured by `dispatch` |
| 14 | Hook output with a seeded fake secret is redacted before display, persistence and context | `hook_output_is_redacted_before_the_outcome_and_the_audit` (reason, findings, context and the audit file contain no seeded key); `stderr_is_redacted_and_keeps_only_its_last_8_kib`; `a_before_context_hook_adds_a_redacted_hook_context_item_to_an_edit_plan` (the context item is redacted before `add_text` charges it) |
| 15 | Output over the byte ceiling is a failure, not a truncation | `output_over_the_ceiling_is_too_large` (parser), `output_over_the_ceiling_fails_and_reading_stops` (runner), `every_bound_is_a_failure_not_a_truncation` (stdout, reason, findings, context, commands) |
| 16 | `context` additions appear in the context inspector within a bounded category, not as instructions | **Not met as written: the inspector is spec 27 and does not exist.** Met as "a bounded `hook_context` item in the turn's context plan": `a_before_context_hook_adds_a_redacted_hook_context_item_to_an_edit_plan` and `context_from_session_start_and_before_context_reaches_the_model_prompt` show it as `hook_context: hook:<id>` within the turn budget, and `context_is_kept_only_at_the_events_that_use_it` keeps it out of every other event. Pending #27, as `context.md` §10 records |
| 17 | Hook configuration is a capability key: a repository cannot add a hook without review, nor disable a user's hook | `a_repository_hook_is_pending_until_reviewed_at_its_hash`, `a_repository_hook_runs_only_after_review_at_its_hash`, `a_review_at_an_old_hash_does_not_activate_a_changed_hook`, `a_repository_cannot_touch_a_user_hook`, `a_repository_may_disable_its_own_hook`, `a_repository_cannot_record_a_review`, `admin_may_disable_a_user_hook_and_it_is_attributed_as_a_widening`, `a_profile_cannot_carry_a_hook_or_a_review`; spec 31's two hook cases in `permission_profiles.rs` |
| 18 | Every invocation and outcome is audited, with failure kinds distinguished | `every_invocation_writes_one_hook_invoked_event`, `every_failure_kind_has_its_audited_name`, `a_mandatory_failure_denies_and_an_optional_failure_warns` (each kind's `failureKind`), `an_unknown_verdict_is_audited_as_a_widening_attempt_and_denies`, `a_dispatch_with_no_hooks_spawns_nothing_and_writes_no_audit` |
| 19 | An extension failure does not corrupt session state, asserted by failing a hook at each event | `the_state_matrix_fails_every_event_without_corrupting_durable_state`: nine events × `exit 1`/timeout/malformed, then the parse-first reader, no dangling marker, a classifier-accepted task status, and exactly one `hook_invoked` with the expected `failureKind`. The three zero-hook pins (`with_no_hooks_a_patch_apply_is_todays_result`, `with_no_hooks_a_chat_turn_is_todays_result`, `with_no_hooks_a_multi_tool_turn_is_todays_result`) show zero hooks reproduce today's result |
| 20 | Every quality-gate command passes, and the spec 18 baseline shows no regression | See §7.2. The fast checks, scoped clippy for all four crates, scoped tests, the deterministic tier and `specs:check --strict` passed locally. **The workspace-wide clippy and test suite were not run locally**, per `AGENTS.md`; they are left to CI, so this criterion is not claimed until CI's Quality workflow is green on the commit |

### 7.2 Gate

Per `AGENTS.md`'s 2026-10-15 policy, the seven-command gate was **not** run
locally. The local close-out ran:

- `scripts/quality-gate.sh fmt node lint typos deny`: all five steps pass.
- Scoped clippy, `--all-targets --locked -- -D warnings`, for
  `workspace-engine`, `desktop-shell`, `damaian-cli` and `eval-harness`: pass.
- `cargo nextest run -p workspace-engine --locked`: 1468 passed, 25 skipped.
- `cargo nextest run -p desktop-shell --locked`: 167 passed, 17 skipped.
- `cargo nextest run -p damaian-cli --locked`: 2 passed.
- `cargo nextest run -p eval-harness --locked`: 89 passed, 2 skipped.
- `cargo run -p eval-harness -- run --tier deterministic`: all 23 scenarios
  pass; approval-policy violations 0, restricted-path and secret violations 0,
  unrelated files changed 0, task completion 1.000, recovery success 1.000.
  The committed `evals/baseline.json` is older (16 records) and was not
  regenerated; the metrics it asserts are unchanged, and the newer scenario
  assertions run in the tier itself.
- `npm run specs:check -- --strict`: 65 specs, dependency lines and status
  records agree.

The full seven-check gate is left to CI. Do not read the above as a quality-gate
pass until CI's **Quality** workflow is green on the commit.

`.config/nextest.toml` is new in Task 7: it gives
`a_before_task_complete_hook_that_always_denies_stops_at_the_round_budget` a
30-second slow-timeout with termination, so a regression in the round bound is
a `TIMEOUT` rather than a hung CI job. It changes no other test.

### 7.3 Timing, depth and the events that moved

**Default timeouts**, in `config.rs`
`HookEvent::default_timeout_ms`, unchanged by Task 8: 2,000 ms for
`session_start`, `before_context` and `before_tool`; 5,000 ms for `session_end`;
10,000 ms for `before_patch_apply`, `after_patch_apply`, `after_tool`,
`after_check` and `before_task_complete`. `hook.<id>.timeout_ms` may lower or
raise them up to the 60,000 ms cap.

**`before_tool` overhead**, measured 2026-10-24 with one trivial `/bin/sh` hook
that consumes its envelope and returns `allow`, 50 `read_file` calls in one
batch, against the same turn with no hooks. Debug: 7.6–11.1 ms extra per call
(50-read turn 167–312 ms baseline, 674–736 ms with the hook). Release:
11.1–11.3 ms per call steady (81–103 ms baseline, 636–654 ms with the hook; the
first run after a release build was 21.0 ms/call). The overhead is dominated by
spawning a process, not by Damaian's own code, which is why release does not
reduce it. It is usable but not free: a 50-read batch adds about 0.55 s, while
a one- or two-call round adds roughly 10–20 ms. No design change follows; a
user who needs the critical path shorter should use the per-hook timeout and
configure `before_tool` only where the rule needs it.

**The depth limit was never reached by a legitimate hook.** The recursive case
is reached only by the test that deliberately configures two hooks which each
request a command. A hook that runs its own check is excluded from the
`before_tool` its command triggers, so the common case stays at depth 1 and
needs no budget. The limit is a guard for hook chains, not a normal path.

**Events that moved or have a narrower attachment than §2 predicted:**

- **`session_end` moved to deletion.** There is no session teardown: a session
  lives until it is deleted and closing the app ends none. `delete_session`
  dispatches it and only observes; a `deny` is a warning and the delete
  proceeds. The delete now names its repository root, and a mismatch is refused
  before any hook runs (see below).
- **`after_check` has no standalone path.** It fires inside a turn's
  `record_command_findings`, only for a validation command, because the
  shell's `/api/run-command` records no findings and therefore has no point at
  which a check's results exist. It also runs before that command's action
  marker is finished, although every failure still finishes it; `context.md`
  §13's "after the marker" is literal only for `after_tool`.
- **`before_task_complete` moved inside the loop**, to the two real completion
  exits (the forced-final answer and the empty-tool-call answer), because a
  deny needs another round. It does not fire at a pause, and *Answer now* skips
  it: the user chose to end at the round budget and re-entry would be
  unbounded.
- **`before_context` also attaches to the edit flow**, with `"flow": "edit"`,
  as `context.md` §3 decided.
- **`before_tool` and `after_tool` also attach to a requested command**, in
  Task 5's dispatcher. Task 8 therefore dispatches them for the turn's own
  action only and does not double-dispatch for a hook's command.
- **The inspector criterion is not met**, because spec 27 does not exist. It is
  met as a bounded `hook_context` item in the turn's context plan (criterion
  16).

### 7.4 What the plan did not foresee

Recorded because each changes what a user or a later task should expect.

- **`ApprovalCause::HookRequested` is new.** A `before_tool`
  `request_approval` on `run_command` forces the card and names the hook and
  reason, instead of showing `unknown`. Its audit name is `hook_requested`, and
  spec 62's §5.1 cause table was extended in Task 8. Because the prompt count is
  recorded when the proposal is stored, commands use
  `propose_command_at_with_causes` so the cause is in the original
  `command_proposal_stored` record rather than a second storing.
- **An answer is not an approval.** `request_approval` refuses `ask_user` (and
  every tool with no approval step). `ask_user` sees `before_tool` but never a
  card, and a question pause does not dispatch `before_task_complete`.
- **A patch proposal's `request_approval` is a warning**, because its existing
  review step already satisfies it.
- **Deleting a session checks the repository root.** `delete_session` takes
  `(session_id, repository_root)`; a root that does not hash to the session's
  repository id returns
  `Session <id> does not belong to repository <root>` before dispatch and
  leaves the session in place. The shell route requires `repo`, and `app.js`
  sends the open folder. This is why a `curl` caller must now send it too.
- **A test run refuses an unreviewed repository hook.** `test_run` runs only an
  `Active` repository hook, so it cannot execute repository code before review;
  a user's own disabled hook can still be tested. A test run writes no
  `hook_invoked`; a review writes `hook_reviewed`.
- **A declined hook is reversible.** It is not shown in the consent notice
  again, but Settings offers **Approve** on it.

### 7.5 Known limitations and flakes

- **A hook's command line is displayed with spaces between its arguments.** The
  review dialog, the Settings list and `damaian hooks` render `command` followed
  by `args.join(" ")`, so an argument that itself contains a space cannot be
  told apart from two arguments in that display. The review hash covers the
  exact argument list (`args` is `|`-separated in config), so the decision still
  applies to exactly the definition shown; only the display is ambiguous. Recorded
  rather than fixed: it is present in three renderers (two in `app.js`, one in
  the CLI), and there is no UI test harness to hold a quoting helper in place.
- **Several hook tests have failed under heavy parallel load, all showing the
  same process-registry race, not a hook defect.** Task 9 recorded
  `after_check_fires_for_a_validation_command_and_not_for_a_read` failing once
  and passing on every rerun, without a cause. During this close-out the state
  matrix failed once at `after_check / malformed`, leaving an unfinished
  `run_command` marker, and a deliberately stressed rerun (four copies of the
  hooks test binary at once) reproduced the cause:
  `Io("process <pid> has no readable identity to record")`, from
  `ProcessRegistry::register`. A test command runs `/usr/bin/true`; under load
  the child can exit before `ProcessIdentity::of(pid)` reads its start time, so
  registration fails, `run_proposal` returns an error before the action marker
  is finished, and the test sees a dangling marker. Under the same stress,
  `after_check_payload_carries_the_recorded_finding_ids`,
  `a_mandatory_failure_denies_and_an_optional_failure_warns` and
  `after_tool_runs_on_the_command_approval_resume` failed the same way, which
  is consistent with the Task 9 observation. This is a pre-existing race in
  spec 46/47's spawn-and-register path, outside spec 32's code and outside this
  task's files; it is recorded here rather than papered over. The normal full
  suite passed on rerun (1468/1468).
- **Three eval-harness tests failed once in the same loaded background run**
  (`the_continue_keeps_state_scenario_resumes_the_same_task`,
  `the_answer_now_scenario_makes_one_forced_final_call` and
  `the_deterministic_tier_runs_every_scenario_and_passes`, the last on
  `answer_now_ends_the_turn` reaching only 2 of 8 tool calls), then passed on
  rerun (89/89) and in the direct deterministic run (23/23). Not diagnosed
  further; recorded so a later flake is not mistaken for a regression.
