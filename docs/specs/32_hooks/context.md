# Context: Hooks

Background for [`tasks.md`](tasks.md). This file records the places where
[`proposal.md`](proposal.md) (the flat spec, unchanged in substance) no longer
matches the code, checked on 2026-10-07. It also records the decisions the
proposal leaves open. Read it before Task 1. Do not "fix" any of these back to
the proposal's wording without re-reading the code they cite.

The proposal was written before specs 46, 47, 31 and 24 landed. Its design
holds. Its §2 table points at drifted lines, its §5.4 premise is false, and it
never says what a verdict means at an event that has nothing to approve or
nothing left to block. Those gaps are §2–§5 below, and each one changes a
task.

## 1. File references, checked on 2026-10-07

These were checked on a tree where #24's Task 6 and #50's planning were
uncommitted in the same checkout, so `config.rs`, `context_manager.rs`,
`profile.rs` and `effective_policy.rs` lines will move. Re-check a line before
editing near it.

| Proposal cites | Now |
|---|---|
| `ContextManager::build_context` at `context_manager.rs:66` | `:81`. Items go through `add_text` (`:296`), which redacts and charges the budget. Callers: `chat.rs:753` and `edit.rs:404` |
| The tool-call loop at `chat.rs:740` | `run_agentic_turn` (`chat.rs:1482-3080`). The per-call loop starts at `:2144`. `action_permission` is called at `:2162`, and the action marker is opened at `:2258` |
| `PatchEngine::apply_patch` at `patch_engine.rs:376` | `:413`. Its only production caller is `EditOrchestrator::apply_stored_patch` (`edit.rs:560`, the call at `:608`), which the shell (`/api/apply-patch`, `lib.rs:890`), the CLI (`main.rs:536`) and the eval harness use |
| The hash check at `patch_engine.rs:291` | `:328-329` (`base_hash` against the current content) |
| `ValidationOrchestrator::run_proposal` at `validation.rs:186` | `:266`. Called from `chat.rs:994` (resume), `chat.rs:2380` (main loop) and the shell's `/api/run-command` (`lib.rs:1055`, the call at `:1104`) |
| `SessionStore::create_session` at `session.rs:79` | `:344`. Its only production caller is `chat.rs:696`, inside `ask_with_session_with_options`. There is no session teardown. `delete_session` (`session.rs:533`) is called from `/api/session-delete` (`lib.rs:715`) |
| Task completion at `chat.rs:1205-1230` | The status after the loop, `chat.rs:2995-3005`. `StopReason::Answered` with no pending proposal becomes `TaskStatus::Complete`. The loop has about a dozen `break`s carrying `Answered` (`:2044`, `:2073`, and one per terminal arm), and most of them are pauses, not completions |
| `CommandRunner` uses `Command::output()` at `command_runner.rs:89-93`, no timeout | False since specs 46 and 47. See §2 |
| MCP stdio spawn at `mcp.rs:288-345` | `mcp.rs:365` registers `ProcessKind::McpServer` |
| `curl` child in `KillOnDrop` at `model.rs:400-406` | `model.rs:634` registers `ProcessKind::ModelCall` |
| PID registry in spec 17 §5.7 | Moved to [spec 46](../46_process_registry_and_orphan_sweep/proposal.md): `ProcessRegistry` (`process_registry.rs`), `ProcessKind` (`:64`), `register` (`:164`) |
| `AuditLog::record` at `audit.rs:42` | Unchanged: `record(&self, event_type, fields: &[(&str, String)])`, and it redacts every field |
| `Finding` redacts at construction (spec 22 §5.6) | `Finding::new(FindingDraft, &SecretScanner)` (`finding.rs:149`) is the only constructor. `FindingSource` (`:27`) has no hook variant |
| Config layering and its trust boundary in spec 31 §5.2-5.3 | Built by spec 34 and spec 31: `ConfigScope { User, Repository, Admin, Profile }` (`config.rs:64`), `RepositoryKeyClass` (`:77`), `apply_overlay_scoped` (`:798`), `classify_overlay_fields!` (`:2222`). See §7 |

## 2. A deadline mechanism already exists; the hook runner copies its shape

§2 and §5.4 say `CommandRunner` blocks in `Command::output()` with no
deadline, so requirement 1 "has nothing to build on". That stopped being true
when spec 46 made commands killable and spec 47 gave them a timeout.
`CommandRunner::run_at` (`command_runner.rs:201`) now does exactly what §5.4
asks for:

- `spawn()` with piped stdio and `process_group(0)` (`:248-256`);
- a `ProcessRegistry` entry with the child's identity, removed when the guard
  drops (`:257-264`, `CommandGuard` at `:389`);
- a `try_wait` poll loop against a deadline (`:276-310`, poll interval
  `COMMAND_POLL_INTERVAL` at `:93`);
- `kill_process_group` on expiry (`:157`), then a reap, so a recycled PID is
  never signalled.

**Decision:** the hook runner (Task 3) is a new function with the same shape.
It is not a call to `CommandRunner`, for four reasons:

1. `CommandRunner` runs `$shell -lc <command>`. A hook is an executable with
   arguments. A login shell would read the user's shell profile on every
   `before_tool`, which costs the critical-path budget §5.4 cares about, and
   would make `shell` (a Forbidden key) part of every hook's behaviour.
2. `CommandRunner` re-classifies through `CommandPolicy` and refuses blocked
   commands. A hook executable is not an agent command (§8).
3. A hook writes JSON to stdin. `CommandRunner` has no stdin.
4. `CommandRunner` truncates oversized output. §5.9 says oversized hook output
   is a failure, not a truncation.

What it shares: `kill_process_group` becomes `pub(crate)`, and the registry
gains `ProcessKind::Hook` (`"hook"`). Do not refactor `run_at` into a shared
helper inside this spec. That would be a change to command execution, which no
task here owns.

## 3. Where each event attaches, and which tasks touch `chat.rs`

| Event | Site | File | Task |
|---|---|---|---|
| `session_start` | Before `create_session` in `ask_with_session_with_options` (`chat.rs:695-697`), only when no session id was passed | `chat.rs` | 7 |
| `before_context` | Before `build_context` in the chat turn (`chat.rs:753`) and in the edit flow (`edit.rs:404`) | `chat.rs`, `edit.rs` | 7 (chat), 6 (edit) |
| `before_tool` | After `action_permission` (`chat.rs:2162`) and the plan-review gate, **before** the action marker (`:2258`). For an all-read-only batch, once per call before `run_read_only_batch` (`:2127`). Also before each hook-requested command (§9) | `chat.rs`, `hooks.rs` | 8, 5 |
| `after_tool` | After the marker is finished for each dispatched action (`:2868` and the terminal arms), and after the three resume branches (`:890`, `:921`, `:994`) | `chat.rs` | 8 |
| `before_patch_apply` | In `apply_stored_patch` after the mode and profile refusals (`edit.rs:579-580`), before the apply marker | `edit.rs` | 6 |
| `after_patch_apply` | In `apply_stored_patch`, after the marker is finished `ok` | `edit.rs` | 6 |
| `after_check` | In `record_command_findings` (`chat.rs:1298`), when the command is a validation command (`is_validation_command`, `command_policy.rs:475`), after its findings are recorded | `chat.rs` | 8 |
| `before_task_complete` | After the loop, where `StopReason::Answered` with no pending proposal would become `Complete` (`chat.rs:2995-3005`). It needs a loop re-entry, so it moves into the loop's `Answered` exits that are not pauses (§4) | `chat.rs` | 7 |
| `session_end` | Before `delete_session`, through an engine method that the shell's `/api/session-delete` calls | `workspace_engine.rs`, `desktop-shell/src/lib.rs` | 6 |

So **`chat.rs` is edited in Tasks 7 and 8 only**. Task 7 takes the
turn-boundary events (`session_start`, `before_context`,
`before_task_complete`). Task 8 takes the per-action events (`before_tool`,
`after_tool`, `after_check`). They are split so that each `chat.rs` window is
short and reviewable, and so that Task 7's loop re-entry is not mixed with
Task 8's per-arm changes.

**`session_end` fires on deletion and only observes.** There is no session
teardown: a session lives until it is deleted, and closing the app ends no
session. Deletion is the one moment a session ends. A hook cannot block it
(§4), because refusing a user's explicit delete is not a narrowing anyone
asked for. If this proves useless in practice, record it in §7 of the
proposal as the event that "had no clean attachment point", which §7 already
anticipates.

**`after_check` does not fire for the shell's standalone `/api/run-command`.**
That path records no findings today (`lib.rs:1104` calls `run_proposal` and
returns), so it has no point where a check's results exist. Task 8 records
this as a limit. It does not add finding recording to that endpoint.

**`before_context` fires in the edit flow too**, with `"flow": "edit"` in the
payload. The edit flow builds context for a one-shot patch proposal, and a
rule like "never put `secrets/` into a prompt" should hold there as well.

## 4. What each verdict means at each event

The proposal defines the verdicts against "the action". Three kinds of event
do not fit that: events with no approval step, events after the action has
happened, and an event that only observes.

**Decision:**

| Event | `deny` | `request_approval` | `warn` | Mandatory failure |
|---|---|---|---|---|
| `session_start` | The turn is refused before a session is created. The user sees the reason | Treated as `deny` | Warning finding, proceed | Treated as `deny` |
| `before_context` | The turn fails with `failureKind` `hook_denied` and the reason as the assistant message | Treated as `deny` | Warning finding, proceed | Treated as `deny` |
| `before_tool`, command | No proposal is stored. The tool result is a refusal naming the hook and its reason, as a mode refusal does, and the loop continues | The proposal is forced to `requires_approval`, and the card shows the hook's reason. A blocked command stays blocked | Warning finding, proceed | Treated as `deny` |
| `before_tool`, MCP call or web diagnostic | Refusal tool result, loop continues | The call takes its existing approval card, even if the server or diagnostic would have run unasked | Warning finding, proceed | Treated as `deny` |
| `before_tool`, `propose_patch` or `edit_file` | Refusal tool result, no patch stored | Satisfied. A patch is always reviewed before it is applied. Recorded as a warning | Warning finding, proceed | Treated as `deny` |
| `before_tool`, any other tool (reads, search, git status, plan tools) | Refusal tool result | Treated as `deny`: there is no approval step to move it to | Warning finding, proceed | Treated as `deny` |
| `before_patch_apply` | The apply is refused with the hook's reason. Nothing is written and no marker is opened | Satisfied. An apply is always the user's own action. Recorded as a warning on the result | Warning on the result, apply proceeds | Treated as `deny` |
| `before_task_complete` | The task is not completed. The reason goes to the model as a user-role note and the loop continues, within the existing round and token budgets | Treated as `deny` | Warning finding, complete | Treated as `deny` |
| `after_tool`, `after_patch_apply`, `after_check` | Nothing is undone. An error-severity finding with the reason, and at `after_tool` the reason is appended to the tool result the model sees | Treated as `warn` | Warning finding | Error finding and a warning. Nothing is undone |
| `session_end` | Treated as `warn`. Audit only | Treated as `warn` | Audit only | Audit only |

Why `request_approval` denies where there is no approval step: the
alternative is to treat it as `allow`, which is the fail-open reading. A hook
that asked for a human and got silence has been ignored, which is the
outcome requirement 7 exists to prevent. Building an approval card for reads
is a much larger change and is not asked for.

Why the `after_*` events cannot block: the action has happened. Rolling a
patch back from a hook would be a hook mutating state (§5.3, non-goals). The
user rolls back through the existing rollback if they agree with the hook.
Acceptance criterion "a mandatory hook that fails … blocks the action" is
therefore tested at the `before_*` events and `session_start`, and the
`after_*` rows are tested for "nothing is undone, an error finding exists".

Why `before_task_complete` continues the loop rather than failing the turn:
"always run `cargo fmt` before you call a task complete" is the proposal's
own example, and the useful response to it is that the model runs the check.
The loop already has two budgets, so a hook that always denies ends the turn
at the round or token ceiling, not in a loop. Task 7 pins that.

The resolution order is the proposal's (§5.6), applied after this table's
per-event mapping: `deny` > `request_approval` > `warn` > `allow`.

## 5. The response contract, made strict

§5.1 and §5.5 together, with the gaps closed:

- **Exit status 0 with empty stdout (whitespace only) is `allow`.** That is
  §5.2's "default when a hook returns nothing meaningful". Any other stdout
  must parse.
- **A non-zero exit is a failure**, whatever stdout says. There is no "exit 2
  means deny" convention. A hook denies with JSON and exit 0.
- **`hookApiVersion` is required and must be `1`.** Missing is malformed.
  Another number is the `version_mismatch` failure.
- **The response is a closed schema.** Fields: `hookApiVersion`, `verdict`,
  `reason`, `findings`, `context`, `commands`. `verdict` defaults to `allow`
  when absent. `reason` is required for `deny` and `request_approval`.
- **An unrecognised `verdict` or any unknown field is a widening attempt.**
  §5.8 asks for "an unrecognised verdict, a response field claiming
  permission" to be audited as one. Deciding which unknown fields "claim
  permission" would be a heuristic a hook author can step around, so every
  unknown field counts. Each is audited as `hook_widening_attempt` with the
  field or verdict name (not its value) and then handled as malformed
  output under §5.5.
- **Bounds, all failures when exceeded** (§5.9: "a failure, not a
  truncation"):

  | Bound | Value |
  |---|---|
  | stdout | 64 KiB |
  | `reason` | 1,000 characters |
  | `findings` | 20 entries. Each one's own text is bounded by `Finding::new` |
  | `context` | 4 entries, 4,000 characters each |
  | `commands` | 3 entries |

  stderr is different. It is diagnostic, so it is kept to its last 8 KiB,
  redacted, and never a failure.
- **Failure kinds:** `spawn_failed`, `non_zero_exit`, `timeout`, `malformed`,
  `version_mismatch`, `output_too_large`, `changed_since_review` (§7). Every
  one is audited by name.

`findings` entries are `{ severity, summary, details?, path?, line? }`,
mapped onto `FindingDraft` with source `FindingSource::Hook`. A path is kept
only if it is repository-relative and stays inside the repository, the same
rule `workspace_range` applies (`finding/rust_diagnostics.rs:136`).

## 6. Event payloads

The proposal says payloads carry "paths, ids, and metadata, never file
content". Per event:

| Event | `payload` |
|---|---|
| `session_start` | `{}` |
| `before_context` | `{ "flow": "chat" \| "edit", "explicitPaths": [..] }` |
| `before_tool` / `after_tool` | `{ "tool": <tool name>, "paths": [..] }`, plus `"command"` (redacted text) for `run_command`, `"server"`/`"toolName"` for MCP, `"url"` for a web diagnostic. `after_tool` adds `"outcome"` (`ok`/`failed`/`refused`) and, for a command, `"exitCode"` and `"termination"` |
| `before_patch_apply` / `after_patch_apply` | `{ "patchId", "files": [{ "path", "status" }] }`, and `after_*` adds the applied paths |
| `after_check` | `{ "command", "exitCode", "termination", "findingIds": [..] }` |
| `before_task_complete` | `{ "rounds": n, "filesChanged": [..] }` from the session log |
| `session_end` | `{}` |

The envelope adds `profile` (the profile id) beside `mode`, and `hookDepth`
(§9).

Never sent: file content, patch content, the user's prompt text, MCP call
arguments, model output. The prompt is excluded because it is free text that
can carry anything, and no rule above needs it. The command text is sent
because rules like "never `rm -rf`" need it, and it is redacted first.

## 7. Hook configuration and how each scope may set it

Hooks are configured with the existing flat key format, like MCP servers
(`mcp_server.<id>.<field>`, `config.rs:2382`). The proposal already uses
`hook.<id>.mode`.

```text
hook.no_migrations.command=/Users/me/bin/no-migrations
hook.no_migrations.args=--strict
hook.no_migrations.events=before_patch_apply|before_tool
hook.no_migrations.mode=mandatory
hook.no_migrations.order=10
hook.no_migrations.timeout_ms=2000
hook.no_migrations.enabled=true
```

- `id` is `[a-z0-9_]{1,40}`, the custom-profile rule.
- `command` and `events` are required. A hook without them is incomplete.
- `mode` is `mandatory` (default) or `optional`.
- `order` is an `i64`, default 0.
- `timeout_ms` defaults by event and is capped at 60,000. Defaults:
  2,000 ms for `before_tool`, `before_context` and `session_start`;
  10,000 ms for `before_patch_apply`, `before_task_complete` and the
  `after_*` events; 5,000 ms for `session_end`. Task 8 measures the
  `before_tool` overhead, and the proposal's §7 records it.
- `enabled` defaults to `true`.

**Review records** live in user config, keyed by repository the way Allow
Always and the profile selection are (`command_allowlist_by_repository`,
`config.rs:251`):

```text
hook_review.<repository_id>.<hook_id>=sha256:<hex>     # approved at this hash
hook_review.<repository_id>.<hook_id>=declined
```

**The definition hash** covers `command`, `args`, `events`, `mode`, `order`
and `timeout_ms`. It also covers the content of every file the command or an
argument names that resolves inside the repository. Hashing the
configuration line alone would let a repository keep its approved
`command=./scripts/check.sh` and change the script after review.

### 7.1 The classification, by scope

Both new `ConfigOverlay` fields are **Capability** in
`classify_overlay_fields!`. A hook is an executable Damaian runs on the
user's machine. That is capability by spec 31's rule ("can grant or remove
the ability to … execute"), and spec 31 §5.1 already names "every profile
field added by spec 32" as capability.

| Key, set at… | User | Repository | Admin | Profile |
|---|---|---|---|---|
| `hook.<id>.*` for an id no earlier scope defined | Defines the hook | **Defines a pending hook**: inactive until reviewed (below) | Defines the hook | **Forbidden** |
| `hook.<id>.*` for an id user or admin config already defined, any field | Applies | **Forbidden** (redefining, disabling, retiming or making optional a user's hook) | Applies, both directions. A loosening (`enabled=false`, `mode=optional`, a longer timeout) is marked as an admin widening in the policy view | **Forbidden** |
| `hook.<id>.enabled=false` on a hook the same repository defined | — | Applies (a repository may switch off its own hook) | — | — |
| `hook_review.<repository_id>.<hook_id>` | The user's decision | **User-owned**: refused, like `command_allowlist.<repository_id>` | Applies | **Forbidden** |
| An unparsable `hook.*` key or value (bad id, unknown event, bad mode, missing `command`/`events`) | Load error (strict parse) | **Unparsable**: skipped and reported | Load error | **Forbidden** |

Notes on the table:

- **"Forbidden at profile scope"** follows the MCP-server precedent
  (`config.rs:1161-1170`): a profile narrows capability and does not
  configure executables. `split_profile_keys` (`profile.rs:322`) refuses
  `hook.*` and `hook_review.*`, so an imported profile cannot carry a hook.
  An export never contains one.
- **A repository-defined hook arrives pending, not refused.** §5.8 says "a
  repository *can* add a hook once reviewed". Its definition is applied, its
  status is `pending_review`, and it never runs until the user approves its
  current hash. Declined, it never runs. Approved and then changed (the hash
  differs), its status is `changed_since_review`, and it does **not** run the
  changed executable. Instead, it fails under its own classification with
  failure kind `changed_since_review`. A mandatory repository hook whose
  script changed therefore blocks, which is the fail-closed direction for a
  rule the user adopted.
- **A repository-defined `command` must resolve inside the repository** (a
  relative path, canonicalised, symlinks resolved) **or be an absolute
  system path**. Either way the review shows the full command line. A user
  or admin `command` is an absolute path, or a bare name resolved against
  `PATH` at load.
- **Refusals reuse spec 34's path.** A refused hook key is a
  `RejectedConfigKey` with its class, audited once per key per repository
  by `RepositoryTrustStore::review` (`repository_trust.rs:79`), and shown by
  key and class, never by value.
- **The partition test grows two capability cases** in
  `tests/permission_profiles.rs` (`weakening_cases`, `:152`): a repository
  `hook.<user id>.enabled=false` must be refused, and a repository
  `hook_review.<id>.<hook>` must be refused. `repository_config_trust.rs` is
  not edited.

## 8. Hooks run outside the mode and the profile; what they ask for does not

A hook executable is not an agent command. It runs because the user, or an
administrator, configured it, or the user reviewed it. It runs in every mode
and under every profile, Read-only and Ask included. That is safe for the
reason §5.1 gives: a hook can only return a verdict, and every verdict
narrows. Gating hooks by mode would make Ask mode the one place a user's
"never touch `migrations/`" hook stops running, which inverts the point.

What a hook *asks Damaian to do*, its `commands` (§9), is an agent action
and runs under `profile ∩ mode`, through `permits` (`mode.rs:139`), like any
other command. That is §5.8's third bullet.

The hook process environment:

- **cleared**, then only `PATH`, `HOME`, `USER`, `LANG`, `TMPDIR`, plus
  `DAMAIAN_HOOK_ID`, `DAMAIAN_HOOK_EVENT` and `DAMAIAN_REPOSITORY_ROOT`.
  Never the model key's environment variable, never an MCP
  `auth_token_env`, never anything else Damaian inherited. A hook must
  not be a way to read the user's API key;
- working directory: the repository root;
- stdin: the event JSON, then closed.

## 9. Requested commands and recursion

"Run an approved local command" (§3) is the `commands` field. **Decision:**

- **Only commands that policy would run unasked are run.** A command is
  classified at the repository root under the session's mode and profile, by
  the same `permits` check and `CommandPolicy` classification as the turn's
  own commands. If it would need approval, or is blocked, or the mode or
  profile refuses `run_command` (Ask mode, `command_access=none`), it is
  **not run**. A warning finding names it and the reason. A hook never
  raises an approval card. That would make a hook a way to put a
  model-independent prompt in front of the user mid-action, and §4 rules out
  long-running hooks.
- **A requested command is an action.** It is bracketed by an action marker
  (`start_action`, `session.rs:979`) of kind `hook_command`, so spec 17's
  recovery sees it, and its output becomes findings through
  `findings_from_execution`. At an event with no task (`session_end`, a
  legacy patch with no session) requested commands are refused.
- **Its result does not change the verdict.** A hook that wants a verdict to
  depend on a check runs the check itself and returns `deny`. The user guide
  says so with the `cargo fmt` example.
- **Recursion** (§5.7): every dispatch carries a `HookContext { depth,
  excluded: BTreeSet<hook id> }`. A requested command dispatches
  `before_tool` and `after_tool` at `depth + 1`, with the requesting hook in
  `excluded`, so a hook never sees its own command. `MAX_HOOK_DEPTH = 2`.
  Past it, dispatch is skipped for the nested action, a warning finding
  says so, `hook_depth_exceeded` is audited, and the outer outcome is marked
  `depth_exceeded`. The nested action still runs (§5.7's trade).

## 10. Findings, warnings and context

- **`FindingSource::Hook` is a new variant.** A hook's own findings, its
  `warn` reasons, an optional hook's failure and a refused requested command
  are all findings of this source, with `origin_ref` set to `hook:<id>`. That
  makes "a visible warning" (requirement 7) the findings panel, which already
  exists. The shell's source filter and label gain the variant (Task 9).
- **Outside a session there is no panel.** At `before_patch_apply` and
  `after_patch_apply`, warnings are also returned on `PatchApplyResult` so
  the shell and the CLI can show them, and a legacy patch with no session
  gets only that and the audit.
- **`context` is honoured only at `session_start` and `before_context`.**
  Context is assembled once per turn, at `build_context`, so that is where a
  hook's additions can become what §5.2 asks for: "ordinary `ContextItem`s in
  a dedicated category". Kind `hook_context`, path `hook:<id>`, added through
  `add_text` (`context_manager.rs:296`), which redacts and charges the turn's
  token budget. A hook that returns `context` at any other event gets a
  warning finding, and the context is dropped. It is not treated as
  malformed, because the response is otherwise valid.
- **The inspector criterion waits on spec 27.** "Context additions appear in
  the context inspector within a bounded category" cannot be met while
  [#27](../27_context_inspector.md) does not exist, and #27 is a
  cross-reference here, not a dependency. It is met as "a bounded
  `hook_context` item in the turn's context plan", recorded as such in the
  proposal's §7. #26 must keep `hook_context` as a category when it rewrites
  `build_context`, the way it absorbs #24's `repository_map`.
- **Deny reasons reach the user** as the tool result (which the
  conversation renders), as the refusal on an apply, or as the assistant
  message on a refused turn. The message always names the hook, so a hook
  denial is distinguishable from a mode or profile refusal (`refusal_message`,
  `mode.rs:117`, stays the mode/profile message and is not reused).

## 11. Audit events

All through `AuditLog::record`, which redacts:

| Event | Fields |
|---|---|
| `hook_invoked` | `hookId`, `event`, `sessionId`, `taskId`, `classification` (`mandatory`/`optional`), `source` (`user`/`repository`/`admin`), `verdict`, `outcome` (`ok`/`failed`), `failureKind`, `durationMs`, `reason` (denials and requests only) |
| `hook_widening_attempt` | `hookId`, `event`, `field` or `verdict` name. Never the value |
| `hook_depth_exceeded` | `hookId` (the outer one), `event`, `depth` |
| `hook_command_refused` | `hookId`, `command` (redacted), `reason` |
| `hook_reviewed` | `repositoryId`, `hookId`, `decision` (`approved`/`declined`), `hash` |

One `hook_invoked` per hook per dispatch, including failures. Pending,
declined or disabled hooks are not invoked and not audited per event. Their
state is visible in the policy view.

## 12. Ordering across scopes

All active hooks for an event form one list, sorted by `(order, id)`. Scope
does not affect order, so a repository cannot run its hook first by
virtue of where it was defined. Ids are unique across scopes (§7.1 forbids
the collision). Short-circuiting follows §5.6, and a mandatory failure that
maps to `deny` (§4) short-circuits as a `deny` does.

## 13. Durable state

- `before_*` hooks run before the action marker opens, and `session_start`
  before the session exists, so a crash during a hook leaves no in-flight
  action. Recovery has nothing to classify. The hook process is a registered
  `ProcessKind::Hook` child, so the next launch's sweep (spec 46) kills it.
- `after_*` hooks run after the marker is finished. A crash during one leaves
  a finished action and an orphaned hook process, which the sweep kills.
  Nothing re-runs the hook.
- A requested command has its own marker (§9).
- Acceptance criterion "an extension failure does not corrupt session state"
  is tested by failing a hook at each event (non-zero exit, timeout,
  malformed) and then reading the session log with the parse-first reader:
  the log parses, the task is in a terminal or waiting state the recovery
  classifier accepts, and no marker is left open (Task 8).

## 14. Parallel work and shared files

- **`chat.rs`:** Tasks 7 and 8. #32 may pair with **#25 only**. It must not
  run beside any other spec that edits `chat.rs`: #23, #50, #56, #57, or
  #24's Task 7.
- **`config.rs`, `profile.rs`, `effective_policy.rs`,
  `tests/permission_profiles.rs`:** Task 1. #24's Task 6 (in progress on
  2026-10-07) and #50's Task 2 edit the same four files to add config keys.
  That is a rebase, not a blocker, but agree the order before Task 1 starts,
  and re-run spec 31's partition tests after rebasing.
- **`context_manager.rs`:** Task 6 adds the `hook_context` items. #24's
  Task 6 adds the `repository_map` item there. Same rule.
- **`finding.rs`:** Task 4 adds `FindingSource::Hook`. Nothing else in flight
  adds a variant.
- **`process_registry.rs`:** Task 3 adds `ProcessKind::Hook`.

## 15. Things the proposal says that were checked and hold

- `SecretScanner::redact` (`secret_scanner.rs:42`) returns the redacted text
  and findings. `add_text` uses it, and so does `Finding::new`, which redacts
  before it bounds (spec 22 `context.md` §4).
- `permits(mode, &ProfileCapabilities, &ToolAction, classification,
  read_only_hint) -> Permission` (`mode.rs:139`) is the one combined check,
  and `Config::profile_capabilities()` feeds it. `ToolAction` is
  `pub(crate)` (`chat.rs:3734`), so a requested command's check builds a
  `ToolAction::Command` inside the crate.
- The patch hash check (`patch_engine.rs:328-329`) is untouched by every
  task: a hook never edits a patch (§5.3).
- `update_task_status_with_kind` (`session.rs:397`) records a failure kind,
  as spec 48 uses it. `hook_denied` is a new kind string, not a new state.
- The deterministic eval tier runs with no hooks configured, so its baseline
  cannot move unless a task changes behaviour with zero hooks. Task 10 runs
  it to confirm that.
