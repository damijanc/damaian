# Hooks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
decisions it left open in [`context.md`](context.md)
**Started:** 2026-10-07

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Hook configuration and its scope classification | Done 2026-10-09 | Landed as planned. `config.rs`: `HookEvent` (`ALL`, `as_str`, `parse`, `default_timeout_ms`), `HookMode` (default `Mandatory`), `HookSource`, `HookConfig` (with `deadline_ms(event)`), `HookConfigOverlay`, `HookReview` (`sha256:<hex>` or `declined`), `MAX_HOOK_TIMEOUT_MS = 60_000`, `Config.hooks`, `Config.hook_reviews_by_repository`, `Config::hook(id)`, both overlay fields classified **Capability**. `hook.<id>.<field>` and `hook_review.<repository id>.<hook id>` parse in `set` (`validate_hook_id` is the `[a-z0-9_]{1,40}` rule; `timeout_ms` must be 1..=60,000; `events` must be non-empty and known). `apply_overlay_scoped` has a `hooks` arm with an exhaustive scope `match`: user and admin upsert, with an admin `enabled=false`, `mode=optional` or longer timeout on an existing hook recorded as a widening; repository defines only a new, complete id and otherwise refuses every present field as `Forbidden` (incomplete: `Unparsable`); profile refuses every field as `Forbidden`. Reviews apply at trusted scopes and are `UserOwned` elsewhere. `load_scoped` fails when a user or admin hook lacks `command` or `events`. Both `to_policy_text`s print hooks (`push_hook_config`, `push_hook_overlay`); `Config`'s leaves out the reviews like the other per-repository maps. `profile.rs`: `split_profile_keys` refuses hook fields (`Forbidden`) and reviews (`UserOwned`). `effective_policy.rs`: `rule_label` names `hook.<id>.<field>` and `hook_review.*`. `lib.rs` re-exports the types. `tests/hooks.rs`: 16 tests, written first and confirmed failing to compile on the missing types. `tests/permission_profiles.rs`: two weakening cases (`hooks`, `hook_reviews_by_repository`), nothing else changed. Engine `--lib` + `hooks` + `permission_profiles` + `repository_config_trust` + `foundation`: 646 pass, 5 skipped; `repository_config_trust.rs` untouched. `fmt --check`, scoped `clippy -D warnings`, `typos` clean, and `cargo check` of `damaian-cli`, `desktop-shell`, `eval-harness` (`--all-targets`) passes. Mutations, each reverted and confirmed failing: (1) `HookMode` default `Optional` fails the defaults test and three others; (2) the repository arm upserting onto an existing id fails `a_repository_cannot_touch_a_user_hook` and spec 31's weakening test; (3) profile scope treated as admin fails the profile test, spec 31's split/merge agreement test and `a_profile_cannot_loosen_anything_the_user_config_narrowed`; (4) reviews accepted at repository scope fails the review test, the weakening test and the agreement test; (5) no completeness check fails the incomplete-user-hook test; (6) admin `enabled=false` not flagged fails the admin-widening test. **Deviations:** a profile's `hook_review.*` is refused as `UserOwned`, not `Forbidden` as `context.md` §7.1 first said, so it matches how the merge already refuses the other per-repository maps at profile scope and spec 31's split/merge agreement test holds (§7.1 corrected). The policy view shows one rule per hook field (`hook.<id>.command`, `.events`, `.mode`, …), the way MCP servers appear, rather than one rule per hook. `HookEvent::default_timeout_ms` and `HookConfig::deadline_ms` landed here, not in Task 3, because judging an admin timeout as a widening needs them. **For Task 3:** use `HookConfig::deadline_ms(event)` rather than adding `default_timeout`. **For Task 4:** a repository hook's `command` is not yet checked to resolve inside the repository or to be an absolute path (`context.md` §7.1). That belongs in `activation`. `HookConfig.source` is the scope that created the hook, and an admin change to a user hook keeps it `User`. **For every later task's mutation runs:** restoring a file by renaming a backup over it keeps the backup's older mtime, and cargo then reuses the mutated build. One test kept failing after a correct restore until `config.rs` was touched. Restore by writing the content back, or `touch` the file after restoring |
| 2 · The contract: events, envelope, strict response, resolution | Done 2026-10-09 | New `hooks.rs`, pure: no process, file or config load. `HOOK_API_VERSION = 1` and the seven bounds as named constants (`MAX_HOOK_STDERR_BYTES` is unused until Task 3). `Verdict { Allow, Warn, RequestApproval, Deny }` with derived `Ord`, `as_str`/`parse`, and a doc comment on why there is no granting verdict. `resolve(impl IntoIterator<Item = Verdict>)` is `max`, empty is `Allow`; `short_circuits(verdict)` is true for `Deny` only. `ordered(hooks, event)` keeps enabled hooks listing the event, sorted by `(order, id)`. `HookEnvelope` serialises camelCase with `event` as its snake_case name. `parse_response` gives `HookResponse { verdict, reason, findings: Vec<HookFinding>, context: Vec<String>, commands: Vec<String> }` or `ResponseError { Malformed(String), VersionMismatch(u64), OutputTooLarge, Widening { attempts } }`, with `failure_kind()` (`Widening` → `Malformed`). `HookFailureKind` with `as_str`. `hooks.rs` re-exports `HookConfig`, `HookEvent`, `HookMode`, `HookSource`, `MAX_HOOK_TIMEOUT_MS` from `config.rs`; the crate root re-exports only `HookEnvelope`, `HookFailureKind`, `HookFinding`, `HookResponse`, and the rest is reached as `workspace_engine::hooks::…` so names like `Verdict` stay out of the root. `tests/hooks.rs`: 18 new tests (34 in the file), written first and confirmed failing to compile on the missing module. `hooks` + `permission_profiles` + `repository_config_trust`: 131 pass, 1 skipped; neither of the other two files edited. `fmt --check`, scoped `clippy -D warnings`, `typos` clean. Mutations, each restored by copying the content back and confirmed failing: (1) an `Approve` variant (with its `as_str` arm) does not compile the test's exhaustive `rank` helper; (2) resolve by last-seen fails `resolution_is_the_strongest_verdict_in_every_order`; (3) unknown fields ignored fails `an_unknown_verdict_or_field_is_a_widening_attempt`; (4a) stdout truncated at the ceiling instead of failing fails `output_over_the_byte_ceiling_is_too_large`; (4b) `reason` truncated instead of failing fails `every_bound_is_a_failure_not_a_truncation`; extra: (5) `request_approval` short-circuiting fails two tests, (6) `ordered` by id only with disabled hooks kept fails the order test. **Deviations:** `ResponseError::Widening` carries `attempts: Vec<WideningAttempt>` (`Field(name)` or `Verdict(name)`), not `names`, so the audit can say which it was without parsing a string; names only, never values. A finding is a closed schema too: its unknown keys are `Field("findings.<key>")`. Widening attempts are collected **before** the version is checked, because the version is the hook's own claim and `{"verdict":"approve"}` with no version must still be audited; so `{"hookApiVersion":2,"autoApprove":true}` is `Widening`, not `VersionMismatch`. `null` counts as absent for every optional field (`hookApiVersion: null` is still missing, so malformed). Beyond the plan: a non-integer `hookApiVersion`, a `verdict` that is not a string, an empty `commands` entry, a finding without `severity`/`summary` or with `line` < 1 are `Malformed`; bounds count characters, not bytes. **For Task 3:** `parse_response` checks the size itself, so the runner may hand it the `MAX_HOOK_STDOUT_BYTES + 1` bytes it read and let it return `OutputTooLarge`, or fail first; either gives the same kind. **For Task 4:** `HookFinding.path` is not yet checked to stay inside the repository (`context.md` §5); do that when building the `Finding`, which knows the root. `commands` is `Vec<String>`; Task 4's `RequestedCommand` does not exist yet, so define it there or use the strings. Audit `hook_widening_attempt` per `WideningAttempt`, then treat the response as `failure_kind()` |
| 3 · The hook runner: spawn, deadline, bounds, environment | Not started | Adds `ProcessKind::Hook` |
| 4 · The dispatcher: order, review gate, audit, findings | Not started | Adds `FindingSource::Hook` |
| 5 · Requested commands and recursion | Not started | |
| 6 · Attachment outside `chat.rs`: patch apply, edit-flow context, `session_end` | Not started | |
| 7 · `chat.rs` turn boundaries: `session_start`, `before_context`, `before_task_complete` | Not started | **Edits `chat.rs`.** Read Global Constraints first |
| 8 · `chat.rs` per action: `before_tool`, `after_tool`, `after_check` | Not started | **Edits `chat.rs`.** Read Global Constraints first. Measures `before_tool` overhead |
| 9 · Shell API, UI and CLI: hook list, review, enable, test run | Not started | |
| 10 · Docs, acceptance criteria, gate, close the spec | Not started | Full seven-command gate and the deterministic eval tier |

**Goal:** Let a user enforce a local rule Damaian does not know about. A hook
is an external program that sees a lifecycle event and returns a verdict:
`allow`, `warn`, `request_approval` or `deny`. There is no verdict that
grants anything, so a hook can only narrow `profile ∩ mode`. A broken
mandatory hook blocks rather than being skipped.

**Architecture:** One new module, `hooks.rs`, holds the contract (Task 2), the
runner (Task 3), the dispatcher (Task 4) and requested commands (Task 5). The
runner spawns the hook directly, not through the user's shell, with a
cleared environment, a deadline, a process-group kill and a
`ProcessKind::Hook` registry entry, the same shape as `CommandRunner::run_at`
(`context.md` §2). The dispatcher orders active hooks, skips unreviewed
repository hooks, resolves verdicts with `deny` >
`request_approval` > `warn` > `allow`, audits every invocation, and turns
output into redacted `FindingSource::Hook` findings and `hook_context` context
items. Call sites map the resolved outcome onto their event using
`context.md` §4's table. `chat.rs` changes only in Tasks 7 and 8.
Configuration uses the flat `hook.<id>.<field>` keys. A repository may define
a hook, which stays inactive until the user reviews its definition hash, and
may never touch a user's hook (`context.md` §7).

**Tech Stack:** Rust 2024, `serde`/`serde_json` (already dependencies of
`workspace-engine`), `libc` (already used by `command_runner.rs`), the
existing `ConfigOverlay` parser, vanilla JS in `app.js`. No new dependencies.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Several of its
  decisions override the proposal. Do not re-derive any of these from
  `proposal.md` alone:
  - §2: the runner copies `CommandRunner`'s shape but is not a call to it;
  - §3: which event attaches where. `session_end` fires on deletion and only
    observes;
  - §4: what each verdict means at each event. `request_approval` with no
    approval step denies, and the `after_*` events never undo anything;
  - §5: the strict response schema, the bounds, and "every unknown field is
    a widening attempt";
  - §7: the config keys, the review hash and the scope table in §7.1;
  - §8: hooks run in every mode and profile, but their requested commands
    run under `profile ∩ mode`, and the hook environment is cleared;
  - §9: requested commands run only if policy would run them unasked;
  - §10: context is honoured only at `session_start` and `before_context`.
- **#32 edits `chat.rs` in Tasks 7 and 8, and in no other task.** Of the
  specs that edit `chat.rs`, #32 may pair with **#25 only**, and with no
  other `chat.rs` spec: not #23, #50, #56 or #57, and not #24's Task 7.
  - Before starting Task 7 or Task 8, read the progress table of every spec
    in flight. If another spec's `chat.rs` task is in progress, wait, or agree
    which track rebases.
  - Re-read every `chat.rs` line number in `context.md` §1 and §3 before
    editing near it. They were checked on 2026-10-07 and other specs edit
    this file.
- **Every cargo command runs with `CARGO_TARGET_DIR="$PWD/target"` from the
  worktree root.** Parallel worktrees must not share one target directory.
  Shell state does not persist between commands, so put the variable on every
  invocation, for example:

  ```bash
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test hooks
  ```

- **A hook only narrows.** No task adds a verdict, field or code path that
  makes an approval-gated action automatic, re-enables something mode or
  profile refused, or lets a hook edit a patch, command or context item
  (proposal §5.3, §5.8). If a task seems to need one, stop and record it.
- **Fail closed by default.** `mode` defaults to `mandatory`. A timeout,
  non-zero exit, malformed response or version mismatch is never `allow`.
- **Zero hooks configured means today's behaviour, byte for byte.** Each
  wiring task (6, 7, 8) includes a test with no hooks that pins the existing
  result, and dispatch with an empty hook list spawns nothing and writes no
  audit event.
- **Spec 34's and spec 31's tests are the floor.**
  `tests/repository_config_trust.rs` passes unchanged after every task, and
  no task edits it. `tests/permission_profiles.rs` is extended in Task 1
  only, by adding cases, never by weakening one.
- **New engine tests go in `crates/workspace-engine/tests/hooks.rs`**, which
  Task 1 creates. Fixtures that build an engine set
  `enable_index_watcher: false` and a `data_dir` inside the temp directory.
- **Test hooks are `/bin/sh` with a script path as the argument.** Do not
  `chmod +x` a freshly written script and execute it: freshly written
  executables stall on this Mac (`docs/PLAN/OBSERVATIONS.md` observation 25,
  as spec 24's tasks record). `/bin/sh` reading a script file is not a login
  shell and needs no exec permission on the script. Never put the script
  inline in `args`, because `args` is `|`-separated.
- **Tests never spawn a real login shell.** A test that needs a requested
  command to run sets `shell` to `/usr/bin/true` and asserts on
  `CommandExecution`, as spec 31 Task 5 did.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record the mutation in the progress row. This repository has a
  history of tests that passed without testing anything (spec 47 §7.2,
  spec 21's error rate).
- **Scope per-task checks. Run the full seven-command gate from `AGENTS.md`
  once, in Task 10.** `cargo nextest run --workspace` takes about 5 minutes
  locally, and `cargo clippy --workspace --all-targets` up to 18 minutes cold.
- **Never `git commit` unasked, and never push.** Each task ends by showing
  the change and the scoped check results, then asking. When asked, write one
  subject line with no body and no trailer.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/hooks.rs` | New. Contract types and resolution (Task 2). `run_hook` (Task 3). `HookDispatcher`, the review gate and the definition hash (Task 4). Requested commands and `HookContext` (Task 5) |
| `crates/workspace-engine/src/config.rs` | `HookConfig`, `HookConfigOverlay`, `hook.*` and `hook_review.*` parsing, printing and the scope-aware merge (Task 1) |
| `crates/workspace-engine/src/profile.rs` | `split_profile_keys` refuses both hook keys (Task 1) |
| `crates/workspace-engine/src/effective_policy.rs` | Hooks with source, mode, events and review state, and `rule_label` (Task 1) |
| `crates/workspace-engine/src/process_registry.rs` | `ProcessKind::Hook` (Task 3) |
| `crates/workspace-engine/src/command_runner.rs` | `kill_process_group` becomes `pub(crate)` and nothing else (Task 3) |
| `crates/workspace-engine/src/finding.rs` | `FindingSource::Hook` (Task 4) |
| `crates/workspace-engine/src/workspace_engine.rs` | The dispatcher on the engine (Task 4). `delete_session` with `session_end` (Task 6) |
| `crates/workspace-engine/src/context_manager.rs` | `build_context_with_hooks`, adding `hook_context` items. `build_context` delegates with none (Task 6) |
| `crates/workspace-engine/src/edit.rs` | `before_patch_apply`/`after_patch_apply` in `apply_stored_patch`, and `before_context` in the edit flow (Task 6) |
| `crates/workspace-engine/src/patch_engine.rs` | `PatchApplyResult.hook_warnings` only (Task 6). The apply and its hash check are untouched |
| `crates/workspace-engine/src/chat.rs` | **Task 7:** `session_start`, `before_context`, `before_task_complete`. **Task 8:** `before_tool`, `after_tool`, `after_check` |
| `crates/workspace-engine/src/lib.rs` | Module registration and re-exports (Tasks 1–5) |
| `crates/workspace-engine/tests/hooks.rs` | New (Task 1). Every later engine task adds to it |
| `crates/workspace-engine/tests/permission_profiles.rs` | Two weakening cases (Task 1) |
| `crates/damaian-cli/src/main.rs` | `hooks`, `hook-review`, `hook-run` (Task 9) |
| `crates/desktop-shell/src/lib.rs` | Hook list, review and enable endpoints. `session-delete` through the engine. Apply warnings in the response (Tasks 6, 9) |
| `crates/desktop-shell/static/app.js`, `index.html`, `style.css`, `docs/ui-style-guide.html` | Hooks in Settings, the review prompt, the `Hook` finding source, apply warnings (Task 9) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md` | Proposal §5.11, plus the hook boundary in `SECURITY.md` (Task 10) |

## Interface reference

Checked on 2026-10-07 against `main` with #24's Task 6 uncommitted beside it.
Re-check a line before editing near it.

- `CommandRunner::run_at` (`command_runner.rs:201`): spawn `:248-256`,
  registry `:257-264`, deadline loop `:276-310`. `kill_process_group(pid)`
  (`:157`, private). `CommandGuard` (`:389`). `COMMAND_POLL_INTERVAL` (`:93`).
- `ProcessKind { McpServer, ModelCall, Terminal, Command }`
  (`process_registry.rs:64`), `as_str` (`:72`).
  `ProcessRegistry::open(data_dir)` (`:131`),
  `register(kind, task_or_session_id, pid) -> RegistrationHandle` (`:164`).
- `AuditLog::record(&self, event_type, fields: &[(&str, String)])`
  (`audit.rs:42`).
- `SecretScanner::redact(&self, text) -> Redaction { text, findings }`
  (`secret_scanner.rs:42`).
- `FindingSource` (`finding.rs:27`), `FindingDraft { source, severity,
  summary, details, range, code }` (`:112`), `Finding::new(draft, &scanner)`
  (`:149`), `findings_from_execution` (`:298`).
  `SessionStore::record_finding(session_id, &Finding)` (`session.rs:1391`).
- `SessionStore::create_session` (`session.rs:344`), `delete_session`
  (`:533`), `update_task_status_with_kind` (`:397`), `start_action`
  (`:979`), `finish_action(marker, outcome)` (`:1050`).
- `ContextItem { kind, path, content, tokens, redaction_status }`
  (`context_manager.rs:24`), `build_context` (`:81`), `add_text` (`:296`).
  Callers: `chat.rs:753`, `edit.rs:404`.
- `EditOrchestrator::apply_stored_patch` (`edit.rs:560`): mode/profile
  refusals `:579-580`, apply marker after them, `patch_engine.apply_patch` at
  `:608`. `refuse_unless_mode_and_profile_permit_patches` (`:300`).
- `ValidationOrchestrator`: `classify_command` (`validation.rs:168`),
  `propose_command` (`:191`), `run_proposal` (`:266`).
- `permits(mode, &ProfileCapabilities, &ToolAction, Option<&CommandClassification>, Option<bool>) -> Permission`
  (`mode.rs:139`), `Permission` (`:67`), `refusal_message` (`:117`).
  `ToolAction` is `pub(crate)` (`chat.rs:3734`). `Config::profile_capabilities()`.
- `is_validation_command` (`command_policy.rs:475`, private).
- Config: `ConfigScope` (`config.rs:64`), `RepositoryKeyClass` (`:77`),
  `AppliedKey` (`:126`), `Config.mcp_servers` (`:353`),
  `command_allowlist_by_repository` (`:251`), `apply_overlay_scoped`
  (`:798`), the MCP server merge per scope (`:1131-1175`),
  `upsert_mcp_server_from_repository` (`:1712`), `ConfigOverlay` (`:2110`),
  `classify_overlay_fields!` (`:2222`, list at `:2232`), `ConfigOverlay::set`
  (`:2378`, `mcp_server.` prefix at `:2382`), `push_mcp_server_config`
  (`:3144`). `split_profile_keys` (`profile.rs:322`). `rule_label`
  (`effective_policy.rs:123`), `EffectivePolicy::from_load` (`:239`).
  `RepositoryTrustStore::review` (`repository_trust.rs:79`).
- `chat.rs`, for Tasks 7 and 8:
  `ask_with_session_with_options` (`:669`), `create_session` call (`:695-697`),
  `build_context` call (`:753`); resume branches (web diagnostic `:841-890`,
  MCP `:907-921`, command `:956-994`); `record_command_findings` (`:1298`);
  `run_agentic_turn` (`:1482`); the `Answered` exits at `:2044` and `:2073`;
  `run_read_only_batch` call (`:2127`); per-call loop (`:2144`);
  `action_permission` call (`:2162`); action marker (`:2258`);
  `propose_command` (`:2312`); `run_proposal` (`:2380`); `create_patch`
  (`:2407`, `:2591`); web diagnostic arm (`:2634`); MCP arm (`:2738`,
  approval at `:2748`); marker finished `ok` (`:2868`); final status
  (`:2995-3005`); `action_permission` (`:3150`).
- Shell routes (`desktop-shell/src/lib.rs`): `/api/effective-policy` (`:292`),
  `/api/session-delete` (`:715`), `/api/apply-patch` (`:890`),
  `/api/run-command` (`:1055`), `/api/repository-config-review` (`:1172`).
- Spec 31's partition tests (`tests/permission_profiles.rs`):
  `every_overlay_field_is_classified_exactly_once` (`:86`),
  `weakening_cases` (`:152`),
  `every_capability_key_resists_weakening_from_repository_scope` (`:397`).

---

## Task 1: Hook configuration and its scope classification

**Requirements:** 3 (per-hook enable), and the configuration half of 6. The
criteria "mandatory is the default classification for a hook with no `mode`
set" and "hook configuration is a capability key: a repository cannot add a
hook without review, nor disable a user's hook" (the config half). Implements
`context.md` §7 and §7.1. **Files:** `config.rs`, `profile.rs`,
`effective_policy.rs`, `lib.rs`, create `tests/hooks.rs`, extend
`tests/permission_profiles.rs`.

Nothing runs a hook yet. This task makes the configuration exist, merge per
scope and show in the policy view.

**Interfaces produced** (later tasks rely on these names):

- `pub enum HookEvent { SessionStart, BeforeContext, BeforeTool, AfterTool, BeforePatchApply, AfterPatchApply, AfterCheck, BeforeTaskComplete, SessionEnd }`,
  with `as_str`/`parse` using the proposal's snake_case names, and
  `HookEvent::ALL`. In `config.rs`, because config parses it. Task 2
  re-exports it from `hooks.rs`.
- `pub enum HookMode { Mandatory, Optional }`, `Default` = `Mandatory`.
- `pub enum HookSource { User, Repository, Admin }`.
- `pub struct HookConfig { id, command, args: Vec<String>, events: Vec<HookEvent>, mode: HookMode, order: i64, timeout_ms: Option<u64>, enabled: bool, source: HookSource }`
  and `Config.hooks: Vec<HookConfig>`.
- `Config.hook_reviews_by_repository: BTreeMap<String, BTreeMap<String, HookReview>>`,
  `pub enum HookReview { Approved { hash: String }, Declined }`.
- `HookConfigOverlay` and `ConfigOverlay.hooks: Vec<HookConfigOverlay>`,
  `ConfigOverlay.hook_reviews_by_repository`.
- `pub const MAX_HOOK_TIMEOUT_MS: u64 = 60_000`.

- [x] **Step 1: Write the failing tests** in a new
  `crates/workspace-engine/tests/hooks.rs`, with the `temp_dir` helper and a
  `load(user, repository, admin)` helper modelled on
  `permission_profiles.rs`' `load` (`:52`). Cases:
  - `a_user_hook_parses_with_defaults`: only `command` and `events` set gives
    `mode == Mandatory`, `order == 0`, `timeout_ms == None`,
    `enabled == true`, `source == User`.
  - `a_hook_without_command_or_events_is_a_user_config_error` and the same
    lines in repository config are skipped as `Unparsable`.
  - `an_unknown_event_or_mode_is_unparsable_at_repository_scope_and_fatal_at_user_scope`.
  - `a_repository_hook_is_defined_as_repository_sourced`: `source ==
    Repository`. Activation is Task 4's.
  - `a_repository_cannot_touch_a_user_hook`: for each of `enabled=false`,
    `mode=optional`, `timeout_ms=60000`, `order=-1` and `command=/bin/false`
    on a user hook's id, the user's value stands and the key is rejected as
    `Forbidden`.
  - `a_repository_may_disable_its_own_hook`.
  - `a_repository_cannot_record_a_review`: a repository
    `hook_review.<id>.<hook>` is rejected as `UserOwned`.
  - `admin_may_disable_a_user_hook_and_it_is_attributed_as_a_widening`.
  - `a_profile_cannot_carry_a_hook_or_a_review`: `split_profile_keys`
    refuses both as `Forbidden`.
  - `hooks_round_trip_through_the_overlay_text`, including `args` with
    several entries and a review entry.
  - `the_policy_view_lists_each_hook_with_its_source_mode_and_events`.
  - `an_id_outside_the_profile_id_rule_is_rejected`.

  In `tests/permission_profiles.rs`, add two `WeakeningCase`s to
  `weakening_cases`: a repository `hook.<user id>.enabled=false` and a
  repository `hook_review.<repository id>.<hook>=sha256:…`. Do not change an
  existing case.

- [x] **Step 2: Run the tests and confirm they fail to compile** on the
  missing types:
  `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test hooks`.

- [x] **Step 3: Implement.**
  - Parse `hook.<id>.<field>` in `ConfigOverlay::set` beside the
    `mcp_server.` prefix (`config.rs:2382`), and
    `hook_review.<repository_id>.<hook_id>` beside the
    per-repository allowlist keys. `args` and `events` are `|`-separated
    through `split_list`. `events` values are parsed with `HookEvent::parse`.
  - Classify both fields `Capability` in `classify_overlay_fields!`, with a
    comment pointing at `context.md` §7.1.
  - In `apply_overlay_scoped`, add a `hooks` arm with an exhaustive `match`
    on `ConfigScope`, following the MCP server arm (`:1131-1175`):
    `User`/`Admin` upsert (admin loosening is recorded as a widening on the
    `AppliedKey`), `Repository` defines only a new id and otherwise
    rejects every field as `Forbidden` (except `enabled=false` on its own
    hook), and `Profile` rejects everything as `Forbidden`.
    `hook_reviews_by_repository` is applied like
    `command_allowlist_by_repository`: user/admin only, `UserOwned` at
    repository scope, `Forbidden` at profile scope.
  - Validate completeness after all overlays: a user or admin hook without
    `command` or `events` is a load error. A repository one is dropped with
    an `Unparsable` rejection.
  - Print both in `to_policy_text` (both destructures have no `..`, so the
    compiler finds them). Add `push_hook_config` beside
    `push_mcp_server_config` (`:3144`).
  - `split_profile_keys` (`profile.rs:322`): refuse both.
  - `effective_policy.rs`: a rule per hook (`hook.<id>`), its source,
    mode, events and enabled flag, and `rule_label` entries such as "Hook
    `<id>` (mandatory) on before_patch_apply, before_tool". Review state is
    Task 4's, so leave a `review` field `None` for now.

- [x] **Step 4: Run the tests and confirm they pass.** Then confirm nothing
  else moved:
  `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test hooks --test permission_profiles --test repository_config_trust`.
  `repository_config_trust.rs` is untouched (`git diff --stat`).

- [x] **Step 5: Mutation-test.** Record each in the progress row:
  1. Make `HookMode::default()` `Optional`. The defaults test fails.
  2. Let the repository arm upsert onto an existing id. The "cannot touch a
     user hook" test and spec 31's new weakening case fail.
  3. Treat `ConfigScope::Profile` like `Admin` in the hooks arm. The profile
     test fails.
  4. Accept `hook_review.*` at repository scope. Both the review test and the
     weakening case fail.

- [x] **Step 6: Scoped checks.**

  ```bash
  cargo fmt --all -- --check
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  typos
  ```

- [x] **Step 7: Update this file's Task 1 row, then show the change and the
  check results and ask before committing.** Suggested subject:
  `Configure hooks with a repository-safe scope classification`.

## Task 2: The contract: events, envelope, strict response, resolution

**Requirements:** 1 (strict schema), 2 (ordering), 6 (no widening verdict),
and proposal §5.1, §5.2, §5.5, §5.6. Criteria "verdict resolution is
`deny` > `request_approval` > `warn` > `allow` regardless of hook order",
"ordering is deterministic across runs", "`request_approval` … there is no
verdict that makes an approval-gated action automatic", and the parsing half
of "malformed output, unknown `hookApiVersion`" and of "output exceeding the
byte ceiling is a failure". Implements `context.md` §5, §6 and §12.
**Files:** create `hooks.rs`, `lib.rs`, `tests/hooks.rs`.

Pure code: no process, no file, no config load. Everything here is unit-testable
with strings.

- `pub const HOOK_API_VERSION: u32 = 1`, and the bounds from `context.md` §5
  as named constants (`MAX_HOOK_STDOUT_BYTES`, `MAX_HOOK_REASON_CHARS`,
  `MAX_HOOK_FINDINGS`, `MAX_HOOK_CONTEXT_ENTRIES`, `MAX_HOOK_CONTEXT_CHARS`,
  `MAX_HOOK_COMMANDS`, `MAX_HOOK_STDERR_BYTES`).
- `pub enum Verdict { Allow, Warn, RequestApproval, Deny }` with `Ord` in
  that order, so resolution is `max`. There is no fifth variant, and a
  comment says why (proposal §5.2).
- `pub struct HookEnvelope { event, hook_api_version, session_id, task_id, mode, profile, repository_root, hook_depth, payload: serde_json::Value }`,
  serialised camelCase exactly as proposal §5.1 shows.
- `pub fn parse_response(stdout: &[u8]) -> Result<HookResponse, ResponseError>`:
  - empty or whitespace-only gives `allow`;
  - over `MAX_HOOK_STDOUT_BYTES` gives `OutputTooLarge`. The runner also
    stops reading at the ceiling (Task 3);
  - parse to a `serde_json::Value` first, then check each top-level key
    against the closed list. Every unknown key and an unknown `verdict` string
    become `ResponseError::Widening { names }`, so the dispatcher can audit
    them before treating the response as malformed (`context.md` §5);
  - a missing `hookApiVersion` is `Malformed`, another number is
    `VersionMismatch`;
  - bounds are enforced, and exceeding one is `Malformed`, not a truncation;
  - `reason` is required for `deny` and `request_approval`.
- `pub enum HookFailureKind { SpawnFailed, NonZeroExit, Timeout, Malformed, VersionMismatch, OutputTooLarge, ChangedSinceReview }`,
  with `as_str` giving the `context.md` §5 names.
- `pub fn ordered<'a>(hooks: &'a [HookConfig], event: HookEvent) -> Vec<&'a HookConfig>`,
  by `(order, id)`, enabled only. Review filtering is Task 4's.
- `pub fn resolve(outcomes) -> Verdict`, and the short-circuit rule as a
  function the dispatcher calls after each hook: stop after a `Deny`.

Tests: resolution over every permutation of four hooks returning the four
verdicts (criterion 3, "asserted by running the same hooks in both orders");
`ordered` is stable for equal `order` values and ignores input order; each
`parse_response` branch, including `{"verdict":"approve"}`,
`{"hookApiVersion":1,"verdict":"allow","grant":"all"}` (both `Widening`),
`hookApiVersion: 2` (`VersionMismatch`), a 1,001-character reason
(`Malformed`), 21 findings (`Malformed`) and 64 KiB + 1 bytes
(`OutputTooLarge`); the envelope serialises to proposal §5.1's field names.

Mutations: add `Approve` to `Verdict` and confirm a test asserts the variant
list (a `match` with no wildcard in a test helper); resolve by last-seen
rather than `max`; ignore unknown fields; truncate instead of failing at the
ceiling.

## Task 3: The hook runner: spawn, deadline, bounds, environment

**Requirements:** 1 (enforced timeout), 5 (bounded output), §5.4. Criteria "a
hook that hangs is killed by PID at its deadline, is registered in the PID
registry with its start time, and leaves no process behind after a crash",
and the runner half of "output exceeding the byte ceiling is a failure".
Implements `context.md` §2 and §8's environment. **Files:** `hooks.rs`,
`process_registry.rs`, `command_runner.rs` (visibility only), `lib.rs`,
`tests/hooks.rs`.

- `ProcessKind::Hook`, `as_str` `"hook"`. Check every `match` on
  `ProcessKind` and the registry file parser.
- `kill_process_group` becomes `pub(crate)`. No other change to
  `command_runner.rs`.
- `pub(crate) fn run_hook(hook: &HookConfig, envelope: &HookEnvelope, repository_root: &Path, data_dir: &Path, registry_owner: &str, scanner: &SecretScanner, timeout: Duration) -> HookRun`,
  where `HookRun { stdout: Vec<u8>, stderr: String /* redacted, last 8 KiB */, exit: Option<ExitStatus>, failure: Option<HookFailureKind>, duration: Duration }`:
  - `Command::new(&hook.command).args(&hook.args)`, **not** through `shell`;
  - `env_clear()`, then only the variables in `context.md` §8;
  - `current_dir(repository_root)`, piped stdin, stdout and stderr,
    `process_group(0)`;
  - registered as `ProcessKind::Hook` with the task or session id, behind a
    guard like `CommandGuard`;
  - write the envelope to stdin on a thread and close it, so a hook that
    never reads its stdin cannot deadlock the write;
  - read stdout up to `MAX_HOOK_STDOUT_BYTES + 1`, and stop reading and kill
    the group if it gets there (`OutputTooLarge`);
  - poll `try_wait` against the deadline at `COMMAND_POLL_INTERVAL`. On
    expiry, `kill_process_group`, reap, and fail with `Timeout`;
  - a spawn error is `SpawnFailed`, a non-zero exit is `NonZeroExit`.
- `pub fn default_timeout(event: HookEvent) -> Duration`, from `context.md`
  §7, and the effective timeout is `min(hook.timeout_ms, MAX_HOOK_TIMEOUT_MS)`.

Tests (hooks are `/bin/sh <script>`, per Global Constraints): a script that
echoes its stdin back as `context` shows the envelope arrived; a script that
prints `env` (inside a valid response's `context`) run with
`MODEL_TEST_KEY=…` set in the test process shows the variable absent; a
`sleep 30` script with a 300 ms timeout returns `Timeout` in well under a
second, and afterwards the registry holds no entry and `kill(pid, 0)` fails;
a script that writes 65 KiB returns `OutputTooLarge`; `exit 3` returns
`NonZeroExit`; a missing command returns `SpawnFailed`; a hook that never
reads stdin still completes; stderr with a seeded fake secret is redacted.

The crash half of the criterion ("leaves no process behind after a crash")
is the registry entry plus spec 46's sweep. Test it by registering a hook,
dropping the runner's guard without reaping (simulating a crash with
`mem::forget`), and running `ProcessRegistry::sweep_own`: the hook process is
killed. If that needs a test-only seam, keep it `#[cfg(test)]` inside
`hooks.rs`.

Mutations: inherit the environment (the key test fails); no deadline (the
timeout test hangs, so run it with nextest's slow-timeout); truncate at the
ceiling instead of failing; skip the registry registration (the sweep test
fails).

## Task 4: The dispatcher: order, review gate, audit, findings

**Requirements:** 2, 3, 5, 6, 7, 8, §5.5, §5.8, §5.10. Criteria "a `deny`
verdict short-circuits remaining hooks", "mandatory … blocks / optional …
warns" (at the dispatcher level, as an outcome), "mandatory is the default",
"a hook attempting to widen … has no effect, and the attempt is audited",
"hook output containing a seeded fake secret is redacted before display,
persistence, and entry into context", "a repository cannot add a hook
without review", "every invocation and outcome is audited, with failure kinds
distinguished". Implements `context.md` §7 (the review gate), §10 and §11.
**Files:** `hooks.rs`, `finding.rs`, `workspace_engine.rs`,
`effective_policy.rs` (review state), `lib.rs`, `tests/hooks.rs`.

- `FindingSource::Hook`. Check every `match` on `FindingSource`, the
  serialised name (`"hook"`) and the findings tests in
  `tests/finding_recording.rs`.
- `pub fn hook_definition_hash(hook: &HookConfig, repository_root: &Path) -> Result<String>`
  per `context.md` §7: the definition fields, plus the content of every
  command or argument path that resolves inside the repository.
- `pub enum HookActivation { Active, PendingReview, Declined, ChangedSinceReview, Disabled }`,
  and `pub fn activation(config: &Config, hook: &HookConfig, repository_id: &str, repository_root: &Path) -> HookActivation`.
  User and admin hooks are `Active` when enabled. A repository hook is
  `Active` only when its review hash equals its current hash.
- `pub struct HookContext { depth: u32, excluded: BTreeSet<String> }`,
  `HookContext::root()`. Task 5 uses the depth.
- `pub struct HookOutcome { verdict: Verdict, deciding_hook: Option<String>, reasons: Vec<(String /* hook id */, String)>, warnings: Vec<String>, findings: Vec<Finding>, context: Vec<(String, String)>, commands: Vec<RequestedCommand>, depth_exceeded: bool }`,
  every string already redacted.
- `pub struct HookDispatcher { config, scanner, audit_log, data_dir }`, built
  from the engine's parts, and
  `pub fn dispatch(&self, event, envelope_parts, repository_root, ctx: &HookContext) -> HookOutcome`:
  - `ordered` hooks for the event, minus `ctx.excluded`, with `activation`
    deciding which run. `ChangedSinceReview` does **not** run. It produces a
    `ChangedSinceReview` failure handled by its mode (`context.md` §7.1);
  - each run is `run_hook`, then `parse_response`. A `Widening` error audits
    `hook_widening_attempt` per name and then counts as `Malformed`;
  - a failure of a mandatory hook becomes `Deny` with the reason "hook
    `<id>` failed: <kind>". A failure of an optional hook becomes a warning;
  - one `hook_invoked` audit per hook run, with the `context.md` §11 fields;
  - findings and warnings become `Finding::new(FindingDraft { source: Hook,
    .. })`, so redaction happens at construction. `context` and `reason`
    pass through `SecretScanner::redact`;
  - stop after a `Deny` (§5.6);
  - an empty hook list returns `HookOutcome::allow()` without spawning or
    auditing.
- The engine holds the dispatcher (`WorkspaceEngine.hooks`). The effective
  policy view gains each hook's `activation`.

This task does **not** map outcomes onto events. That is §4's table, applied
at the call sites in Tasks 6–8.

Tests: two deny hooks with orders 1 and 2, only the first runs (the second
script touches a marker file that must not exist); mandatory and optional
versions of each failure kind give `Deny` and a warning respectively; a hook
with no `mode` behaves as mandatory; `{"verdict":"approve"}` gives `Deny` for
a mandatory hook and audits `hook_widening_attempt` with `verdict=approve`
and no reason text; a seeded fake key in `reason`, `findings`, `context` and
stderr is redacted in the outcome and in the audit file; a repository hook
is skipped while pending, runs once approved at its hash, and after its
script changes is not executed (its marker file stays absent) and fails with
`ChangedSinceReview`; a declined hook never runs; every invocation writes
exactly one `hook_invoked` with the right `failureKind`; zero hooks write no
audit event.

Mutations: no short-circuit; failure of a mandatory hook mapped to `Warn`;
run a `ChangedSinceReview` hook anyway; audit the widening value instead of
its name (assert the value is absent from the audit file).

## Task 5: Requested commands and recursion

**Requirements:** 4 (recursion), the "run an approved local command" action
in §3, §5.7, §5.8's third bullet. Criteria "a hook's own command runs under
the session's mode and profile, and is refused in Ask mode", "recursive hook
invocation is bounded, reported as a `Finding`, and the nested action is not
blocked", and "a hook's command executions do not re-enter that same hook".
Implements `context.md` §9. **Files:** `hooks.rs`, `lib.rs`,
`tests/hooks.rs`.

- `pub const MAX_HOOK_DEPTH: u32 = 2`.
- `HookDispatcher::run_requested_commands(&self, outcome, session, task: Option<&Task>, mode, repository_root, ctx) -> Vec<Finding>`:
  - no task gives a refusal for each command (`hook_command_refused`);
  - classify through `ValidationOrchestrator::classify_command`, and check
    `permits(mode, &config.profile_capabilities(), &ToolAction::Command(..), Some(&classification), None)`.
    Anything refused, blocked or needing approval is not run, and gives a
    warning finding plus `hook_command_refused`;
  - otherwise: dispatch `before_tool` at `ctx.depth + 1` with the requesting
    hook in `excluded`; if that resolves to `Deny`, do not run; else open a
    `hook_command` action marker, propose and run through
    `ValidationOrchestrator`, finish the marker, record
    `findings_from_execution`, then dispatch `after_tool` the same way;
  - past `MAX_HOOK_DEPTH`, skip the nested dispatch, add a warning finding,
    audit `hook_depth_exceeded`, set `depth_exceeded`, and still run the
    command (§5.7).
- The result never changes the requesting hook's verdict (`context.md` §9).

Tests (`shell=/usr/bin/true`): in Code mode with `cargo test` allowlisted, a
requested `cargo test` runs and its marker is finished; the same in Ask mode
is refused with no execution and a `hook_command_refused` audit; a command
that needs approval is refused and raises no approval card or stored
proposal; a `before_tool` hook that requests a command does not see that
command's `before_tool` (its invocation count stays 1); two hooks that each
request a command run to depth 2 and stop there, with a depth finding, a
`hook_depth_exceeded` audit and the nested command still executed; at
`session_end`-like dispatch with no task, every requested command is refused.

Mutations: skip the `permits` check (the Ask test fails); do not exclude the
requesting hook (the count test fails); block the nested action at the depth
limit instead of skipping dispatch (the "not blocked" assertion fails).

## Task 6: Attachment outside `chat.rs`: patch apply, edit-flow context, `session_end`

**Requirements:** the `before_patch_apply`, `after_patch_apply`,
`session_end` and edit-flow `before_context` rows of `context.md` §3 and §4.
Criterion "a `before_patch_apply` hook can block an apply, and its reason
reaches the user". Implements `context.md` §10's context items. **Files:**
`edit.rs`, `patch_engine.rs` (one result field), `context_manager.rs`,
`workspace_engine.rs`, `desktop-shell/src/lib.rs` (the apply response and
`session-delete`), `tests/hooks.rs`.

- `ContextManager::build_context_with_hooks(.., hook_context: &[(String, String)])`
  adds each entry through `add_text` with kind `hook_context` and path
  `hook:<id>`, before the retrieved files, so it is charged to the turn
  budget like any item. `build_context` delegates with `&[]`, so its callers
  are unchanged.
- `apply_stored_patch`: after the two refusals (`edit.rs:579-580`) and
  before the marker, dispatch `before_patch_apply`. `Deny` returns
  `ClientError::PolicyBlocked` with the hook id and reason, and nothing is
  written, no marker is opened and no checkpoint is taken.
  `RequestApproval` and `Warn` add to `PatchApplyResult.hook_warnings`.
  After the marker is finished `ok`, dispatch `after_patch_apply`. Its
  `Deny` becomes an error finding and a warning. Nothing is rolled back.
  Requested commands run through Task 5 with the patch's task.
- The edit flow (`edit.rs:404`): dispatch `before_context` with
  `"flow": "edit"`. `Deny` refuses the proposal with the reason. Context
  entries go to `build_context_with_hooks`.
- `WorkspaceEngine::delete_session(session_id)` dispatches `session_end`
  (outcomes are audit-only per §4) and then deletes. `/api/session-delete`
  calls it.
- The shell's `/api/apply-patch` returns `hookWarnings`, and a hook refusal's
  message is the error text the UI already shows.

Tests: a mandatory `before_patch_apply` hook that denies `migrations/` leaves
the file unchanged, opens no `apply_patch` marker, and returns an error whose
text contains the hook id and the reason (criterion 1, engine half). The
shell half: the `/api/apply-patch` test pattern at `lib.rs:4459` returns that
message. Others: a timing-out mandatory hook blocks the apply, and an
optional one lets it proceed with a warning; an `after_patch_apply` deny
leaves the applied content in place and records an error finding;
`request_approval` at `before_patch_apply` applies and returns a warning; an
edit-flow `before_context` hook's context is a `hook_context` item in the
plan, redacted and within the budget; `session_end` runs on deletion and a
`deny` does not stop it; with no hooks, `apply_stored_patch`'s result equals
today's (pin).

Mutations: dispatch `before_patch_apply` after the marker (the marker
assertion fails); roll back on `after_patch_apply` deny (the content
assertion fails); let `session_end`'s deny cancel the delete.

## Task 7: `chat.rs` turn boundaries: `session_start`, `before_context`, `before_task_complete`

**This task edits `chat.rs`. Read the `chat.rs` bullet in Global
Constraints first.**

**Requirements:** the `session_start`, chat `before_context` and
`before_task_complete` rows of `context.md` §3 and §4. **Files:** `chat.rs`,
`tests/hooks.rs`.

- `session_start`: in `ask_with_session_with_options`, when no session id is
  passed, dispatch before `create_session` (`chat.rs:695-697`), with an
  empty `sessionId` in the envelope. `Deny` (or `RequestApproval`, or a
  mandatory failure) returns `ClientError::PolicyBlocked` naming the hook,
  and no session is created. Context entries are kept for
  `before_context`.
- `before_context`: dispatch before `build_context` (`:753`). `Deny` ends the
  turn with `update_task_status_with_kind(.., Failed, "hook_denied")` and an
  assistant message naming the hook and reason. Context from both events goes
  to `build_context_with_hooks`. Warnings and findings are recorded to the
  session.
- `before_task_complete`: at each `Answered` exit that is a completion (the
  model answered with no tool call, `:2044`, and the undecodable-free answer
  at `:2073`), not a pause. Dispatch, and on `Deny` push the reason as a
  user-role message ("A hook did not let this task complete: …"), record it
  in the session log, increment the round and `continue`. The existing round
  and token budgets bound the re-entries. Any other verdict completes as
  today. Do not dispatch on a pause (an approval card, a plan review), on a
  budget stop, or on cancel.

Tests (mock model via `MockModelAdapter`, as in `chat.rs`' own tests, or
`DAMAIAN_MOCK_MODEL_RESPONSE`): a denying `session_start` hook creates no
session file; a denying `before_context` hook fails the task with
`hook_denied` and never calls the model (the mock's call count is 0); a
`before_context` hook's context reaches the model prompt as a `hook_context`
item; a `before_task_complete` hook that denies once and then allows (a
counter file) produces two model calls and a `Complete` task; one that always
denies ends at `ToolBudgetExhausted` after `agent_max_tool_rounds`, not in a
loop; a turn that pauses for command approval does not dispatch
`before_task_complete`; with no hooks, a turn's messages and final status
equal today's (pin).

Mutations: dispatch `before_task_complete` on pauses; complete on `Deny`;
skip the budget on the re-entry (the always-deny test hangs, so give it a
nextest timeout).

## Task 8: `chat.rs` per action: `before_tool`, `after_tool`, `after_check`

**This task edits `chat.rs`. Read the `chat.rs` bullet in Global
Constraints first.**

**Requirements:** the `before_tool`, `after_tool` and `after_check` rows of
`context.md` §3 and §4, §13's state test, and proposal §7's measurement.
Criteria "a `request_approval` verdict makes an otherwise-automatic action
approval-gated", "an extension failure does not corrupt session state —
asserted by failing a hook at each event", and the turn half of "mandatory …
blocks". **Files:** `chat.rs`, `tests/hooks.rs`.

- `before_tool`, in the per-call loop after `action_permission` (`:2162`)
  and the plan-review gate, **before** the marker (`:2258`). A
  mode/profile refusal comes first, so a refused action never reaches a hook.
  - `Deny` or a mandatory failure: the tool result is "Refused by hook
    `<id>`: <reason>", recorded like a mode refusal, with no marker, no
    stored proposal and no card. The loop continues.
  - `RequestApproval`: for `run_command`, the stored proposal gets
    `requires_approval = true` and the reason in the card's rationale
    (a blocked command stays blocked). For MCP and web diagnostics, take
    the existing approval path even if it would have run unasked. For
    `propose_patch`/`edit_file`, a warning. For any other tool, as `Deny`
    (`context.md` §4).
  - For an all-read-only batch, dispatch per call before
    `run_read_only_batch` (`:2127`). A denied call is removed from the batch
    and given its refusal result.
- `after_tool`, after each dispatched action's marker is finished, in the
  main loop and in the three resume branches. A `Deny` appends "Hook `<id>`:
  <reason>" to the tool result the model sees and records an error finding.
- `after_check`, in `record_command_findings` (`:1298`), for validation
  commands only, after the findings are recorded, with their ids.
- Run Task 5's requested commands after each dispatch that returned any.

Tests: an allowlisted command that runs unasked today is forced to an
approval card by a `request_approval` hook, and no verdict exists that does
the reverse (criterion 5: assert over `Verdict`'s variants that none removes
`requires_approval`); a denying `before_tool` hook on `read_file` refuses
with the hook's name, and the refusal text differs from a mode refusal's; a
mandatory hook that times out on `before_tool` blocks the command and an
optional one lets it run with a warning finding; `after_check` fires for
`cargo test` and not for `ls`; a batch of two reads with one denied runs only
the other; **the state matrix**: for each of the nine events and each of
`exit 1`, a timeout and malformed output, run a turn (or the patch apply,
or the delete) with a mandatory hook failing that way, then read the session
log with the parse-first reader. Assert that it parses, that no marker is
left unfinished, and that the task's status is one the recovery classifier
accepts; with no hooks, a multi-tool turn's session log events equal
today's (pin).

**Measure** the `before_tool` overhead: one trivial hook (`/bin/sh` printing
nothing), 50 `read_file` calls, debug and release, against no hooks. Record
the per-call overhead and the default timeouts in proposal §7 and this row.
If the overhead makes `before_tool` unusable, say so there. Do not change
the design in this task.

Mutations: dispatch `before_tool` after the marker opens (the state matrix
or a marker assertion fails); let `request_approval` fall through to `allow`
for reads; skip `after_tool` on the resume branches (a resume test fails).

## Task 9: Shell API, UI and CLI: hook list, review, enable, test run

**Requirements:** 3 (enable and disable individually), §5.5's "shown in the
hook list", §5.8's review, §5.11's "how to debug a failing hook". **Files:**
`desktop-shell/src/lib.rs`, `static/app.js`, `index.html`, `style.css`,
`docs/ui-style-guide.html`, `damaian-cli/src/main.rs`, `tests/hooks.rs`
(engine helpers only).

- Engine helpers (in `hooks.rs`): `list_hooks(config, repository_root) -> Vec<HookListing>`
  with id, source, events, mode, order, enabled and activation;
  `review_hook(data_dir, repository_id, hook_id, decision)` writing
  `hook_review.*` to **user** config and auditing `hook_reviewed`;
  `set_hook_enabled(user config, id, bool)` for user hooks; and
  `test_run(hook, event)` that dispatches one hook against a sample envelope
  and returns the verdict, failure kind and redacted stderr.
- Shell: `GET /api/hooks`, `POST /api/hook-review`, `POST /api/hook-enabled`,
  `POST /api/hook-test`. Pending repository hooks join the existing
  repository-config notice (`/api/repository-config-review`, `lib.rs:1172`)
  as itemised review entries showing the full command line, events and mode.
- UI: a Hooks group in Settings listing each hook, with source, events, "If
  it fails: blocks" or "If it fails: warns", the enable toggle for user hooks,
  and Approve/Decline for pending ones. The findings panel's source filter
  gains "Hook". The apply-patch result shows `hookWarnings`. Follow
  [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md): Decline is the
  default action, Approve is not primary-styled.
- CLI: `damaian hooks <repo>`, `damaian hook-review <repo> <id> approve|decline`,
  `damaian hook-run <repo> <id> <event>`.

Tests: engine tests for each helper, including that `review_hook` writes only
user config and that a review taken at an old hash does not activate a
changed hook; a shell test that `/api/hook-review` with a repository's own
`.damaian/config.conf` as the target is impossible (the endpoint has no path
parameter). UI is checked by hand per the `feedback_verifying_desktop_shell_ui`
practice (rebuild, restart on a spare port with a scratch `DAMAIAN_DATA_DIR`,
drive `app.js` from the console). Record what was checked.

## Task 10: Docs, acceptance criteria, gate, close the spec

**Requirements:** §5.11, §6, §7. **Files:** `docs/USER_GUIDE.md`,
`docs/TROUBLESHOOTING.md`, `SECURITY.md`, `proposal.md`, this file,
`docs/specs/README.md`, `CHANGELOG.md`, and any `Depends on:` line naming #32
(none on 2026-10-07; re-check with
`grep -rn "32_hooks" docs/specs/*.md docs/specs/*/*.md`).

- `USER_GUIDE.md`: what hooks are, the nine events, a worked `/bin/sh` hook,
  the verdicts and what each means per event (`context.md` §4), why
  mandatory is the default, repository hooks and review, and the `cargo fmt`
  example done the right way (the hook runs the check and denies).
- `TROUBLESHOOTING.md`: telling a hook denial from a mode or profile refusal
  (the message names the hook), `hook_invoked` and its failure kinds in the
  audit log, `damaian hook-run` and redacted stderr, "changed since review",
  and what a depth-exceeded finding means.
- `SECURITY.md`: the hook boundary: no widening verdict, the cleared
  environment, repository hooks only after review at a hash, and that a hook
  executable is user-trusted code running outside the mode and the profile.
- Proposal §7: per-criterion evidence table (as spec 31's §7.1), the default
  timeouts and the measured `before_tool` overhead from Task 8, whether the
  depth limit was ever reached by a legitimate hook, and which events moved
  (`session_end` to deletion, `after_check`'s missing standalone path, the
  inspector criterion pending #27).
- Run the full gate, all seven, plus `npm run specs:check` and
  `cargo run -p eval-harness -- run --tier deterministic`, each with
  `CARGO_TARGET_DIR="$PWD/target"`. The baseline must show no regression.
- Close per `AGENTS.md` "When a spec becomes Done": Status lines, README row
  and **What to build next** (#32 unblocks nothing), remove #32 from
  `CHANGELOG.md`'s `Unreleased`, and this file's header and table.
