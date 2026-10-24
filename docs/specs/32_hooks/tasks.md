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
| 3 · The hook runner: spawn, deadline, bounds, environment | Done 2026-10-12 | `process_registry.rs`: `ProcessKind::Hook` (`"hook"`). The only `match` on `ProcessKind` is `as_str`; the registry file keeps `kind` as a string and nothing parses it back, so nothing else changed. `command_runner.rs`: `kill_process_group` is `pub(crate)`, the one-line diff. `hooks.rs`: `run_hook(hook, envelope, repository_root, data_dir, registry_owner, scanner, timeout) -> HookRun { stdout, stderr, exit, failure, duration }`. The hook is executed directly (`Command::new(&hook.command).args(&hook.args)`), `env_clear()` then `PATH`, `HOME`, `USER`, `LANG`, `TMPDIR` when set plus `DAMAIAN_HOOK_ID`, `DAMAIAN_HOOK_EVENT`, `DAMAIAN_REPOSITORY_ROOT`, at the repository root, piped stdio, `process_group(0)`, registered as `ProcessKind::Hook` under `registry_owner` behind `HookGuard` (`CommandGuard`'s shape). A registration failure kills the child and is `SpawnFailed`: an unregistered hook could not be swept. The envelope is written on its own thread and a write error is ignored. stdout and stderr are read on threads into shared buffers; stdout stops at `MAX_HOOK_STDOUT_BYTES + 1`, stderr keeps a 32 KiB tail that is redacted and then cut to its last 8 KiB. The poll loop ends when the hook has exited **and** both pipes have closed, at the ceiling (`OutputTooLarge`), or at the deadline (`Timeout`); a still-running hook is then group-killed and reaped. A non-success exit is `NonZeroExit` (precedence: timeout or ceiling, then exit). A spawn error is `SpawnFailed` with the error in `stderr`. `tests/hooks.rs`: 8 new tests (42 in the file), written first and confirmed failing to compile on the missing `run_hook`/`HookRun`: the envelope arrives on stdin; the environment holds only the named variables (and what `/bin/sh` exports itself: `PWD`, `OLDPWD`, `SHLVL`, `_`) with `MODEL_TEST_KEY` set in the test process absent, and the cwd is the root; `sleep 30 &`/`wait` with a 300 ms deadline is `Timeout` in ~0.32 s with no registry entry and neither the shell nor its background `sleep` alive; 65 KiB and endless (`yes`) output are `OutputTooLarge`, the latter well inside its deadline; `exit 3` after printing `allow` is `NonZeroExit`; a missing command is `SpawnFailed`; a 1 MiB envelope to a hook that never reads stdin completes, and one that also hangs still times out at 300 ms; stderr with a seeded AWS key is redacted and kept to its last 8 KiB. In-crate `hooks::tests::a_hook_left_running_by_a_crash_is_killed_by_the_sweep`: `spawn_hook`, `mem::forget` the guard, `sweep_own` kills the hook. `hooks` + `permission_profiles` + `repository_config_trust` + `process_registry`: 140 pass, 6 skipped; `--lib hooks:: process_registry::`: 15 pass; neither `permission_profiles.rs` nor `repository_config_trust.rs` edited. `fmt --check`, scoped `clippy -D warnings`, `typos` clean; `cargo check` of `damaian-cli`, `desktop-shell`, `eval-harness` (`--all-targets`) passes. Mutations, each restored by writing the content back and `touch`, and confirmed failing: (1) no `env_clear()` fails the environment test; (2) no deadline check hangs the timeout test, terminated by nextest's slow-timeout (`--tool-config-file` with `slow-timeout = { period = "2s", terminate-after = 2 }`) at 4 s; (3) stdout truncated at the ceiling and read on fails `output_over_the_ceiling_fails_and_reading_stops` with `None` for `OutputTooLarge`; (4) registration skipped (`RegistrationHandle::none()`) fails the sweep test; extra: (5) `child.kill()` instead of the group kill fails the timeout test on the surviving background `sleep`; (6) the envelope written synchronously hangs the never-reads-stdin test until slow-timeout. That test passed against (6) as first written, because a hook that exits turns the blocked write into a broken pipe; the hanging case was added to make it load-bearing. **Deviations:** `run_hook` and `HookRun` are `pub`, not `pub(crate)`, because the tests are in `tests/hooks.rs`; reached as `workspace_engine::hooks::run_hook`, not re-exported at the root. The poll interval is a new 5 ms `HOOK_POLL_INTERVAL`, not `COMMAND_POLL_INTERVAL`: that constant is private, exposing it would be a second change to `command_runner.rs`, and 50 ms would add up to 50 ms to every `before_tool` call. No `default_timeout`: the caller passes `Duration::from_millis(hook.deadline_ms(event))`, per Task 1. The sweep test is not `#[ignore]`d. It kills only a `/bin/sh` it spawned, registered in its own scratch data directory, like the timeout test, and it is the only test for the crash half of the criterion. **For Task 4:** call `parse_response(&run.stdout)` only when `run.failure` is `None`; otherwise the failure kind is `run.failure`. `run.stderr` is already redacted. `durationMs` is `run.duration`. `registry_owner` is the task id, or the session id where there is no task (empty for `session_start`). A hook whose leader exits while something it started holds stdout open is not group-killed after the reap (the pid may be reused, as `CommandGuard` reasons). It runs to its deadline and fails `Timeout`, and that descendant is left running. **For later mutation runs:** a mutation that hangs or forgets the guard leaves a `sleep 30` orphan that exits on its own |
| 4 · The dispatcher: order, review gate, audit, findings | Done 2026-10-23 | `finding.rs`: `FindingSource::Hook` (`"hook"`; `every_source_serialises_as_its_as_str` extended). `hooks.rs`: `HookActivation` (`Active`, `PendingReview`, `Declined`, `ChangedSinceReview`, `Disabled`), `hook_definition_hash` (command, args, events, mode, order, timeout_ms, plus `sha256` of every command or argument that canonicalises to a file inside the repository), `activation`, `HookContext { depth, excluded }`, `HookEnvelopeParts`, `RequestedCommand { hook_id, command }`, `HookOutcome`, `HookDispatcher::new`/`dispatch`. Dispatch takes `ordered`, filters `ctx.excluded`, applies `activation`, runs `run_hook` only for `Active`, and resolves with `Verdict`. A `Widening` response audits one `hook_widening_attempt` per attempt (`field`/`verdict` key, name only) and then fails as `malformed`. A mandatory failure is `Deny` with "hook \`<id>\` failed: <kind>"; an optional one is `Warn` with a warning. Every run writes one `hook_invoked` with the §11 fields (`verdict` is the effective one on failure; `reason` only for denials and requests; a successful deny response is `outcome=ok`). `ChangedSinceReview` is audited once with `durationMs=0`; pending, declined and disabled hooks run and audit nothing. Findings and warnings become `FindingSource::Hook` findings with `origin_ref=hook:<id>`; `reason`, `context` and `commands` are redacted; a finding path is kept only when repository-relative with no `..`; `context` is kept only at `session_start`/`before_context` and is otherwise dropped with a warning. `WorkspaceEngine.hooks` is built from config/scanner/audit. `effective_policy.rs`: each `hook.<id>.<field>` rule carries `activation`. `lib.rs` re-exports the dispatcher types and functions. `tests/hooks.rs`: 15 new tests (57 in the file), written first and confirmed failing to compile on the missing types. `hooks` 57 pass; `--lib` `hooks::` + `finding::tests` 18 pass; `hooks` + `permission_profiles` + `finding_recording` + `repository_config_trust` 184 pass, 1 skipped. `fmt --check`, scoped `clippy -D warnings` and `typos` clean; `node --check` and `npm run lint:web` pass; `cargo deny check` fails only on pre-existing advisory RUSTSEC-2026-0382 (`rustls-webpki 0.103.15`), untouched here. `cargo check` of `damaian-cli`, `desktop-shell`, `eval-harness` (`--all-targets`) passes. Mutations, each restored by writing the content back and confirmed failing: (1) no short-circuit after a deny fails `a_deny_short_circuits_the_hooks_after_it`; (2) a mandatory failure mapped to `Warn` fails `a_mandatory_failure_denies_and_an_optional_failure_warns`; (3) running a `ChangedSinceReview` hook fails `a_repository_hook_runs_only_after_review_at_its_hash`; (4) auditing the widening value instead of its name fails `an_unknown_verdict_is_audited_as_a_widening_attempt_and_denies`; (5) treating an unreviewed repository hook as `Active` fails `a_repository_hook_is_pending_until_reviewed_at_its_hash`, `the_policy_view_shows_each_hooks_activation` and the review-gate test; (6) excluding script content from the definition hash fails `the_definition_hash_covers_the_definition_and_the_files_it_names`; (7) honouring context at every event fails `context_is_kept_only_at_the_events_that_use_it`; (8) keeping a finding path outside the repository fails `a_hook_finding_keeps_a_path_only_when_it_stays_in_the_repository`; (9) not redacting a deny reason fails `hook_output_is_redacted_before_the_outcome_and_the_audit`; (10) ignoring `ctx.excluded` fails `a_hook_for_another_event_and_an_excluded_hook_do_not_run`; (11) omitting `reason` from `hook_invoked` fails `every_invocation_writes_one_hook_invoked_event`. **Deviations:** a repository `command` that is relative and does not canonicalise inside the repository, and a repository hook whose current hash cannot be computed, are both `ChangedSinceReview`: the enumeration has no sixth state and the fail direction must block a mandatory hook rather than skip it. That check lives in `activation`, per Task 1's note. `HookOutcome.warnings` are also findings, as §10 requires. `context` outside its two events is dropped with a warning here, because the dispatcher is where the event is known. `HookEnvelopeParts` carries the non-event envelope fields; `depth` comes from `HookContext`. `HookDispatcher::new` takes config/scanner/audit and derives `data_dir` from config rather than taking a redundant argument. The policy view puts `activation` on every field rule because Task 1 settled on one rule per field. stderr is not carried on the outcome or the audit: Task 3 already redacts it, §5.9 says it is for the user debugging the hook, and Task 9 exposes it; the redaction test therefore covers reason/findings/context and asserts no seeded key in the audit. **For Task 5:** `outcome.commands` is `Vec<RequestedCommand { hook_id, command }>` with the command already redacted; `ctx.depth` is already in each envelope as `hookDepth`, and `ctx.excluded` is honoured; `HookContext` is passed by reference. |
| 5 · Requested commands and recursion | Done 2026-10-24 | `hooks.rs`: `MAX_HOOK_DEPTH = 2`. `HookDispatcher` now also holds the `ValidationOrchestrator` and `SessionStore`, so `new` takes them; `workspace_engine.rs` builds them before the dispatcher (a 7-line move). `run_requested_commands(&mut HookOutcome, &Session, Option<&Task>, SessionMode, &Path, &HookContext) -> Vec<Finding>` takes the commands out of the outcome and runs each only when policy would run it unasked: classified at the repository root with `ValidationOrchestrator::classify_command`, then `permits(mode, &config.profile_capabilities(), &ToolAction::Command(..), Some(&classification), None)`, and `!classification.blocked && !classification.requires_approval`. A blocked, approval-needing or refused command never reaches `propose_command_at`, so it stores no proposal, opens no marker and raises no card; it becomes a warning `FindingSource::Hook` finding and one `hook_command_refused` audit (`hookId`, `command`, `reason`). A permitted command is bracketed by a `hook_command` marker (side-effecting, reference the redacted command), proposed at the repository root, run through `run_proposal(approved=false, approved_by="policy")`, finished with `finish_command_action`, and its `findings_from_execution` are recorded in the session with the task id and `hook:<requester>` origin. `before_tool` and `after_tool` are dispatched at `ctx.depth + 1` with only the requesting hook excluded; a `before_tool` `Deny` or `RequestApproval` refuses the command, an `after_tool` `Deny` becomes an error finding and undoes nothing, and commands requested by those nested outcomes are run recursively. Past `MAX_HOOK_DEPTH` dispatch is skipped, `hook_depth_exceeded` is audited (`hookId`, `event=before_tool`, `depth`), a warning finding is returned and `outcome.depth_exceeded` is set on the top-level outcome, while the command still runs. No task refuses every command. The requesting hook's verdict, reasons and warnings are otherwise untouched. `tests/hooks.rs`: 7 new tests (64 in the file), written first and confirmed failing to compile on the missing `MAX_HOOK_DEPTH`, five-argument `HookDispatcher::new` and `run_requested_commands`: a requested `cargo test` runs in Code mode with its marker started and finished `ok` and one proposal; Ask mode refuses with no proposal, no marker and the audit; a command needing approval is refused with no card or proposal; the requesting hook is invoked once; a nested `before_tool` deny stops the command; two hooks that each request a command run to depth 2 (6 executions, 2 `hook_depth_exceeded`, each hook invoked 3 times) with the depth finding and the commands still run; no task refuses. `hooks` 64 pass; `--lib hooks::` 1 pass; `hooks` + `permission_profiles` + `repository_config_trust` 184 pass, 1 skipped. `scripts/quality-gate.sh fmt node lint typos deny` all pass (deny included, unlike Task 4); scoped `clippy -p workspace-engine --all-targets -D warnings` clean; `cargo check` of `damaian-cli`, `desktop-shell`, `eval-harness` (`--all-targets`) passes. Mutations, each restored by writing the content back and `touch`, and confirmed failing: (1) skipping the `permits` check runs the command in Ask mode and fails `a_requested_command_is_refused_in_ask_mode`; (2) not excluding the requesting hook fails `a_requested_command_does_not_reenter_the_hook_that_asked` (3 invocations, not 1); (3) returning instead of running at the depth limit fails the recursion test (4 executions, not 6). **Deviations:** the plan's `HookDispatcher::new` had no orchestrator or session store and `run_requested_commands` named none, so the dispatcher now owns both; passing them per call would have made the planned signature longer, not shorter. `run_requested_commands` takes `&mut HookOutcome` because it takes the commands and sets `depth_exceeded`. The plan says a nested `before_tool` `Deny` refuses; a nested `RequestApproval` does too, because a hook never raises a card and running it would ignore the request — stricter, never a widening. The nested envelope's `profile` is derived from `config.permission_profile_by_repository` for the session's repository, falling back to `full` because no selection applies no overlay (`config.rs`, the profile merge). The recursion test needs depth 2 to be reachable, so commands requested by the nested `before_tool`/`after_tool` outcomes run too; the nested `HookContext` contains only the requester in `excluded` (not the parent's set), so a different hook can see a deeper command, which is what `§9`'s "with the requesting hook in excluded" says. Command-output findings are recorded in the session here, as "record `findings_from_execution`" says, and are not returned, so a call site that records what this method returns cannot duplicate them; the returned findings are refusals, the depth report, nested hooks' findings and an `after_tool` denial. Those findings carry the task id and `hook:<id>` origin but are not path-resolved the way `chat.rs`'s `record_findings` does it: a hook command runs at the repository root, so a printed relative path is already root-relative. `CancelToken::new()` is used because the planned signature carries no turn cancel token, so a requested command stops at its own deadline rather than the turn's cancel. **For Task 6:** build `HookContext::root()`, call `run_requested_commands` with the patch's session and task, and record the returned findings; command-output findings are already recorded. A legacy patch with no task refuses every requested command, which is what `§9` asks for. **For Task 8:** Task 5 already dispatches `before_tool` and `after_tool` for a hook-requested command, so Task 8 must not dispatch them again for that command; it dispatches for the turn's own action and then runs whatever that dispatch requested. |
| 6 · Attachment outside `chat.rs`: patch apply, edit-flow context, `session_end` | Done 2026-10-24 | `context_manager.rs`: `build_context_with_hooks(.., hook_context: &[(String, String)])` inserts each entry through `add_text` as kind `hook_context`, path `hook:<id>`, after `symbols` and before the retrieved files, so it is redacted and charged to the turn budget; `build_context` delegates with `&[]`. `hooks.rs`: `HookDispatcher::envelope_parts(session: Option<&Session>, task_id, payload)` (`pub(crate)`) builds `mode`/`profile`/ids from the session, so Tasks 6–8 do not re-derive them. `patch_engine.rs`: `PatchApplyResult.hook_warnings` (separate from `warnings`), always empty from the engine. `edit.rs`: the orchestrator holds the dispatcher (constructor gained `hooks`). `apply_stored_patch` dispatches `before_patch_apply` after both mode/profile refusals and before the marker; `Deny` returns `ClientError::PolicyBlocked("Refused by hook `<id>`: <reason>")` with nothing written, no marker and no checkpoint; `RequestApproval` adds `hook \`<id>\`: <reason>` to the result and applies; `Warn` adds its warnings. The payload is `{patchId, files:[{path,status}]}`; `after_patch_apply` adds `appliedFiles` and runs after the marker is finished `ok`, where a `Deny` records an error `FindingSource::Hook` finding (`origin_ref = hook:<id>`) and a warning and rolls nothing back. Requested commands run through `run_requested_commands` with the patch's session and task (found via `read_tasks`), their returned findings are recorded and their summaries added to `hook_warnings`; findings are recorded best-effort so an unwritable log cannot block an apply. The edit flow dispatches `before_context` with `{"flow":"edit","explicitPaths":[…]}`, treats `Deny` and `RequestApproval` as a refusal (`PolicyBlocked`, task marked failed by `record_edit_failure`, no model call, no patch), records findings, and passes `outcome.context` to `build_context_with_hooks`. `workspace_engine.rs`: `delete_session(session_id, repository_root)` reads the session, refuses the delete with `ClientError::InvalidInput("Session <id> does not belong to repository <root>")` and dispatches nothing unless `repository_id_for_root(root)` equals `session.repository_id`, then dispatches `session_end`, refuses its requested commands with no task (audited), then deletes whatever the outcome; `edit_orchestrator` is built with `hooks.clone()`. `desktop-shell/src/lib.rs`: `/api/session-delete` requires `repo` and goes through the engine; `/api/apply-patch` returns `hookWarnings`; `app.js` sends `repo` when deleting a session. `tests/hooks.rs`: 10 new tests (74 in the file), written first and confirmed failing to compile on the missing `hook_warnings`, `delete_session` and `build_context_with_hooks`: a mandatory `before_patch_apply` deny leaves the file unchanged, opens no `apply_patch` marker, writes no `stored_patch_applied` audit, and its error names the hook and reason; a timing-out mandatory hook blocks and an optional one applies with a `timeout` warning; an `after_patch_apply` deny keeps the new content, returns a warning and records an error finding at `hook:after_notes`; `request_approval` at before applies with a warning naming the hook; a requested `cargo test` from a `before_patch_apply` hook runs with the patch's task (proposal stored, `hook_command` marker, one execution); an edit-flow `before_context` context entry reaches the model prompt as `--- hook_context: hook:noter ---` with a seeded fake `api_key` redacted; a `request_approval` at edit-flow `before_context` refuses the proposal, calls no model and stores no patch; `session_end` runs on deletion, audits `verdict=deny`, and the session is still gone; with no hooks the apply's `applied_files`/`warnings`/`hook_warnings` are today's and no `hook_invoked` is written; deleting a session with another repository's root returns `does not belong`, leaves the session file in place and writes no `hook_invoked`. `desktop-shell/src/lib.rs`: one new test drives `/api/apply-patch` over HTTP: a user-config deny hook returns HTTP 500 whose error text contains the hook id and reason and leaves the file alone, then a warn hook applies and returns `hookWarnings`. `hooks` 74 pass; `hooks` + `permission_profiles` + `repository_config_trust` + `foundation` + `checkpoint_wiring` + `token_ceiling` 377 pass, 1 skipped; `repository_map` + `plan` 109 pass (context assembly and apply evidence); `-p desktop-shell` 166 pass, 17 skipped. `scripts/quality-gate.sh fmt node lint typos deny` all pass; scoped `clippy -p workspace-engine -D warnings` and `-p desktop-shell -D warnings` clean (`--all-targets`); `cargo check -p damaian-cli -p eval-harness --all-targets` passes. Mutations, each restored by writing the content back and `touch`, and confirmed failing: (1) dispatching `before_patch_apply` after the marker opens fails `a_before_patch_apply_deny_blocks_the_apply_and_names_the_hook` ("a blocked apply opened a marker"); (2) rolling back on an `after_patch_apply` deny fails `an_after_patch_apply_deny_keeps_the_content_and_records_an_error_finding` (content `before`, not `after`); (3) letting a `session_end` deny cancel the delete fails `session_end_runs_on_deletion_and_a_deny_does_not_stop_it` ("the session was not deleted"); (4) removing the repository-ownership check fails `deleting_a_session_with_another_repository_root_is_refused` (no error is returned, so `expect_err` panics). **Deviations:** the plan's `edit.rs:579-580` refusals are now at `:687-688`, its `edit.rs:404` `build_context` call at `:478`, and its shell apply-patch test pattern at `lib.rs:4459` is now the `serve_for_test`/`post_fields_for_test` pair around `:6472`; each was found by behaviour. `WorkspaceEngine::delete_session(session_id)` cannot build the envelope: a `Session` stores only `repository_id` (a one-way hash) and `session_end` hooks need a working directory and `repositoryRoot`, so the method takes `(session_id, repository_root)`, `/api/session-delete` requires a `repo` form field, and `app.js` sends `projectPath` (a one-line UI change the plan did not list; any later caller must send it too). A legacy patch with no session has no log to record requested-command refusals in, so those are reported as `hook_warnings` instead of run; a patch with a session but no task goes through `run_requested_commands` with `None` and is refused, as §9 asks. `PatchApplyResult` already had a `warnings` field, so the hook output is a second field rather than reusing it, keeping a patch problem distinguishable from a hook's advice. The refusal message picks the reason whose hook id is `deciding_hook`, because `reasons` keeps run order and a later `deny` can follow an earlier `request_approval`. Added after review: `delete_session` validates that the caller's root is the session's own repository (`repository_id_for_root(root) == session.repository_id`) before dispatch, because the shell takes the root from the request form and B's hooks would otherwise see A's session in the envelope; a mismatch is an `InvalidInput` (HTTP 500 with the message from `/api/session-delete`), leaves the session in place and runs no hook. `after_patch_apply` findings are recorded best-effort, not `?`, because the apply has already happened and a journalling failure must not turn it into an error. **For Task 7:** use `self.hooks.envelope_parts(session_or_none, task_id, payload)` (pass `None` at `session_start`); `build_context_with_hooks` is the entry point and `build_context` is a thin delegate, so `chat.rs` can pass both events' context. `before_context` must treat `RequestApproval` like `Deny` (the edit flow already does). Record `outcome.findings` and run `run_requested_commands` for any `outcome.commands` that a chat `before_context` returns, since the engine-side sites (edit flow, patch apply) show the pattern for findings and warnings. **For Task 9:** `/api/apply-patch` already returns `hookWarnings` (the UI does not render them yet) and `/api/session-delete` now requires `repo`; the Hooks settings view and the `Hook` finding-source filter remain |
| 7 · `chat.rs` turn boundaries: `session_start`, `before_context`, `before_task_complete` | Done 2026-10-24 | **Edits `chat.rs`.** Landed. `ChatOrchestrator` now holds the `HookDispatcher` (its constructor gained the argument and `workspace_engine.rs` passes `hooks.clone()` — one call site outside this task's listed files). `ask_with_session_with_options`: `session_start` dispatches before `create_session` only when no session id was passed, through `envelope_parts(None, None, {})`; `Deny`/`RequestApproval` returns `ClientError::PolicyBlocked` naming the hook and creates nothing; its findings are recorded once the session exists and its context is carried into the turn. `before_context` dispatches with `{"flow":"chat","explicitPaths":…}`; findings are recorded, `Deny`/`RequestApproval` appends the refusal as an assistant message, marks the task `Failed` with kind `hook_denied` and returns `PolicyBlocked`, after which requested commands run through `run_requested_commands`; both events' context goes to `build_context_with_hooks` as ordinary `hook_context` items. `before_task_complete` fires at the two real completions (the forced-final answer and the empty-tool-call answer), not at any pause. On `Deny`/`RequestApproval` it appends "A hook did not let this task complete: Refused by hook `<id>`: <reason>" as a user message, pushes the model's answer back as an assistant turn, increments the round and continues; `Warn` completes. Its payload's `filesChanged` comes from the turn's checkpoint manifest, the durable per-task record of paths the turn touched (the session log has no path list), with `rounds` = `action_rounds`. Tests: 10 new in `tests/hooks.rs`, written first and confirmed failing on the missing wiring: session_start deny (both `deny` and `request_approval`) creates no session or log and calls no model; its envelope carries `sessionId: null`, `taskId: null` and `{}`; before_context deny fails the task `hook_denied` with the assistant message and zero model calls; session_start and before_context context both appear as `hook_context: hook:<id>` in the model prompt; a before_context requested command opens a `hook_command` marker; deny-once-then-allow gives two model calls and `Complete` with two `hook_invoked`; always-deny stops at the round budget; command-approval and question pauses do not dispatch; zero hooks pins one call, user+assistant messages, `Complete`, no findings and no `hook_invoked`. Scoped: `hooks`+`exploration_rounds`+`round_budget`+`clarification`+`foundation`+`--lib` 821 pass, 5 skipped; `agent_tools`+`plan_turn`+`checkpoint_wiring`+`finding_recording`+`steering`+`token_ceiling`+`command_lifecycle` 89 pass, 5 skipped; whole `workspace-engine` 1,442 pass, 24 skipped; `desktop-shell` 166 pass, 17 skipped; scoped `clippy -p workspace-engine --all-targets -D warnings`; `cargo check` of cli/shell/eval; the deterministic eval tier; and `scripts/quality-gate.sh fmt node lint typos deny`, all green. Mutations, each restored by writing the content back and `touch`ing `chat.rs`, then confirmed failing: (1) dispatch `before_task_complete` at the command-approval and question pauses — both pause tests fail; (2) complete on a `Deny` (ignore the returned note) — deny-once fails at 1 vs 2 model calls; (3) drop `action_rounds += 1` on re-entry — always-deny times out. `.config/nextest.toml` gives that one test `terminate-after` 30 s, so the regression is a `TIMEOUT` rather than a hung job. **Deviations:** the spec 59 round counters — a completion re-entry dispatches nothing, so it charges `action_rounds`, not `exploration_rounds`, following the forced-final call's precedent; the plan's "always denies ends at `ToolBudgetExhausted`" is stale under spec 61 — it ends at the `round_budget_paused` pause (`round_budget_proposal`, task `WaitingForApproval`) after exactly `agent_max_tool_rounds` calls, which the test asserts; *Answer now* does **not** dispatch `before_task_complete`, because it is the user's own decision to end at the budget and re-entry would be unbounded (the pause is skipped for `round_budget_answer_now`), while the token-ceiling forced-final answer does dispatch because the ceiling bounds it; `session_start` passes `None` through `envelope_parts` as Task 6's note directed, so its envelope carries `sessionId: null` rather than an empty string; `session_start` has no task, so its requested commands are not run (§9) and its warnings/findings are recorded only after `create_session`; a `before_context` denial returns `PolicyBlocked` like the edit flow's, after the assistant message and `hook_denied` status, rather than an `Ok(ChatTurnResult)`. **For Task 8:** `self.hooks`, `HookContext::root()` and `hook_refusal` are now available in `chat.rs`; reuse them. Task 5 already dispatches `before_tool`/`after_tool` for hook-requested commands, so do not dispatch them again there. The completion re-entry increments `action_rounds`; a `before_tool` refusal must not. `round_budget_answer_now` and every other pause stay outside `before_task_complete`; do not add a per-action dispatch on the forced final call. `.config/nextest.toml` now exists for per-test timeouts. |
| 8 · `chat.rs` per action: `before_tool`, `after_tool`, `after_check` | Done 2026-10-24 | **Edits `chat.rs`.** Landed. `before_tool` runs in the per-call loop after `action_permission` and the plan-review gate and before the marker; a `Deny`/mandatory failure is a tool result naming the hook with **no marker, proposal or card**. `request_approval` forces `run_command` to a card (the new `ApprovalCause::HookRequested` carries the hook and its reason on the card and in the `command_proposal_stored` audit record the eval harness reads), forces MCP and web-diagnostic cards even when they would have run unasked, is a warning for `propose_patch`/`edit_file` (their review already satisfies it), and is `Deny` for every other tool, including `ask_user` — an answer is not an approval. An all-read batch precomputes permission and dispatches `before_tool` per call sequentially before `run_read_only_batch`; refused or hook-denied calls are removed and still get one result, and spec 59 charges a hook-denied read to `exploration_rounds` exactly as a mode refusal does. `after_tool` runs after the marker is finished in the main loop (including a terminal patch proposal) and on the command-approval resume for the command, MCP call or web diagnostic that actually dispatches; it appends `Hook \`<id>\`: <reason>` to the tool result and records an error finding, and `request_approval` after the fact is a warning. `after_check` runs in `record_command_findings` only for `is_validation_command`, after the findings are recorded, with `findingIds`; a failure records an error and undoes nothing. Task 5's requested commands run after every one of these dispatches. Tests: 16 new test functions in `tests/hooks.rs` (15 in the default suite, one ignored measurement; 100 in the file, 1 skipped) plus 3 inline `chat.rs` unit tests for the verdict/effect matrix, written first and confirmed failing before wiring: command forced to a card by `request_approval` with the hook cause and `hook_requested` audit; denied read names the hook and opens no marker; `request_approval` on a read refuses it; mandatory timeout blocks and optional warns; `after_check` fires for `cargo test` and not `ls`, with an empty `findingIds` fixture; a two-read batch runs only the allowed call; a denied read is exploration and adds no round; an `ask_user` request approval is a refused question, not a card; a patch request approval is a warning; `after_tool` deny appends and records an error without undoing; the command-approval resume dispatches `after_tool`; a terminal patch proposal dispatches it too; the nine-event × three-failure state matrix leaves a parseable log, no dangling marker and only classifier-accepted statuses; zero hooks leaves a multi-tool turn's messages, markers and findings unchanged. Scoped: whole `workspace-engine` 1,461 pass, 25 skipped; `desktop-shell` 166 pass, 17 skipped; `eval-harness` 89 pass, 2 skipped; scoped `clippy -p workspace-engine --all-targets -D warnings`; `cargo check -p damaian-cli --all-targets`; the deterministic eval tier (23/23 pass); and `scripts/quality-gate.sh fmt node lint typos deny` all green. Measurement (proposal §7): one trivial `/bin/sh` allow hook, 50 `read_file` calls in one batch, against no hooks — debug 7.6–11.1 ms extra per call (baseline 167–312 ms, with hook 674–736 ms), release 11.1–11.3 ms per call steady (baseline 81–103 ms, with hook 636–654 ms; the first post-build run was 21.0 ms/call). Defaults unchanged: 2,000 ms for `session_start`/`before_context`/`before_tool`, 5,000 ms for `session_end`, 10,000 ms for the other five events. The overhead is usable but not free — a 50-read batch adds about 0.55 s — and is dominated by spawning a process, so release does not reduce it. Mutations, each restored by writing the content back and `touch`ing `chat.rs`, then confirmed failing: (1) open a marker in the refusal branch — `a_denying_before_tool_hook_refuses_a_read_and_names_the_hook` fails at `left: 1, right: 0`; (2) let `request_approval` fall through to `Proceed` for reads — `request_approval_before_a_read_refuses_it` runs the read; (3) clear the resume branch's pending `after_tool` action — `after_tool_runs_on_the_command_approval_resume` loses the appended hook note. **Deviations:** `ApprovalCause::HookRequested { hook, reason }` is new and spec 62's §5.1 table is extended because that list is explicitly the closed set; commands use the new `ValidationOrchestrator::propose_command_at_with_causes` so the hook cause is present in the original `command_proposal_stored` record (re-storing after the audit would have double-counted the prompt and missed the harness grouping). `after_check` runs where the plan puts it, inside `record_command_findings`, which is before the command's marker is finished; the marker is still finished by the same arm, so no failure leaves it dangling, but `context.md` §13's "after the marker" is literal only for `after_tool`. Resume paths that dispatch an action: `resume_after_command_decision`'s web, MCP and command branches get `after_tool`; plan review, provider fallback, round-budget *Continue*/*Answer now* and `resume_after_answer` dispatch no action and therefore no `after_tool` (`ask_user`'s `before_tool` is the only per-action event it sees). A terminal patch proposal runs `after_tool` but has no model-visible tool result to append to, so only its findings are recorded. Command, web, MCP and question pauses do not dispatch `after_tool` until/unless the action resumes; the command resume test covers that. The mode-refused and invalid-argument paths still open and finish their existing markers but are not "dispatched", so they do not get `after_tool`. **For Task 9:** the shell already renders `cause`; a hook-forced card now supplies `hook_requested` plain words, and the eval harness groups it without changes. `after_check` remains absent from the standalone `/api/run-command`, as `context.md` §3 records. No further `chat.rs` change is expected; use the public engine helpers for the hook list, review and test-run endpoints. |
| 9 · Shell API, UI and CLI: hook list, review, enable, test run | Done 2026-10-24 | Landed. `hooks.rs`: `HookListing` (id, source, command, args, events, mode, order, timeout_ms, enabled, activation, definition hash for a repository hook) and `list_hooks(config, root)`, sorted by id; `HookReviewDecision { Approve { hash }, Decline }`; `review_hook(config, root, hook_id, decision)`, which finds the hook, requires `HookSource::Repository`, recomputes the definition hash and **refuses an approval whose hash is stale**, writes `hook_review.<repository id>.<hook>` to **user config only**, and audits `hook_reviewed` with `repositoryId`, `hookId`, `decision` and the approved `hash`; `set_hook_enabled(config, hook_id, enabled)`, which writes `hook.<id>.enabled` to user config and refuses a repository- or admin-sourced hook; `test_run(config, hook, event, root) -> HookTestRun { verdict, failure, stderr, duration_ms }`, which builds the `context.md` §6 sample payload for the event, runs the single hook through `run_hook`, parses its answer, and reports a failure as the hook's fail direction (mandatory → `deny`, optional → `warn`); stderr is the already-redacted runner output. `test_run` refuses a **repository** hook whose activation is not `Active`, so a test run cannot execute repository code before review; a user's own disabled hook can still be tested. Shell (`desktop-shell/src/lib.rs`): `GET /api/hooks?repo=`, `POST /api/hook-review` (`repo`, `hook`, `decision=approve\|decline`, `hash` for approve), `POST /api/hook-enabled` (`repo`, `hook`, `enabled`), `POST /api/hook-test` (`repo`, `hook`, `event`). None takes a config path; the review writes user config derived from the data directory. Review and enable return `{path, hooks}` with the refreshed listing; test returns `{id, event, verdict, failure, stderr, durationMs}`. `repository_config_review_json` now carries `hooks`: repository hooks whose activation is `pending_review` or `changed_since_review`, each with the full command line, events, mode and hash, so they join the existing notice; its empty answer gained `"hooks":[]` and the existing exact-string test was updated. UI: a Hooks page under a new Hooks group in Settings, listing every hook with source, events, "If it fails: blocks/warns", activation, full command line, an Enabled toggle for user hooks, Approve (and Decline while pending/changed) for repository hooks, and an event select plus Test whose result shows the verdict, failure and redacted stderr; the notice shows one consent dialog per pending/changed hook with the full command line, events and fail direction, **Decline focused as the default** and Approve unfilled, and a dismissal records no decision; a declined hook offers Approve again so a mistake is reversible; the findings panel's source label gained `Hook`; an apply shows `hookWarnings` inline in the patch card. CLI: `damaian hooks <repo>`, `damaian hook-review <repo> <id> approve\|decline`, `damaian hook-run <repo> <id> <event>`, with the same listings and redacted stderr. `docs/UI_STYLE_GUIDE.md` and `docs/ui-style-guide.html` gained a Hook review section and specimen. Tests, written first and confirmed failing to compile on the missing helpers: 7 in `tests/hooks.rs` — listing and activation; review writes user config only, audits and activates at the hash, and a decline replaces it; an old-hash review is refused and a changed script leaves the hook `changed_since_review`; only a user hook can be toggled, and toggling keeps its definition; a test run reports warn and redacted stderr; a failing test run follows the mandatory/optional fail direction, including a widening response and a version mismatch; a pending repository hook is never executed by a test run and runs once approved. `hooks` 106 pass, 1 skipped; `hooks` + `permission_profiles` + `repository_config_trust` 226 pass, 2 skipped; `desktop-shell` 167 pass, 17 skipped; `damaian-cli` 2 pass. Scoped `clippy -D warnings` clean for `workspace-engine`, `desktop-shell` and `damaian-cli`; `cargo check -p eval-harness --all-targets` passes; `scripts/quality-gate.sh fmt node lint typos deny` all pass. Mutations, each restored and confirmed failing: (1) returning the caller's stale hash instead of refusing it fails `a_review_at_an_old_hash_does_not_activate_a_changed_hook`; (2) writing the review to `Config::repository_config_path` fails `review_hook_writes_only_user_config_and_audits_the_decision`; (3) letting `test_run` skip the activation gate fails `a_test_run_never_executes_a_repository_hook_before_it_is_reviewed` (the script's marker exists); (4) removing the repository arm from `set_hook_enabled` fails `only_a_user_hook_can_be_enabled_or_disabled_from_user_config`; (5) making `/api/hook-review` honour a `path` form field fails the shell route test on the repository config byte comparison. **Manual UI check:** built the shell and ran it on port **4917** (never 4765) with `DAMAIAN_DATA_DIR=.manual-check/data`, a scratch repo at `.manual-check/repo` with a pending repository hook (`/bin/sh ./scripts/no-migrations.sh`, before_patch_apply and before_tool, mandatory) and a user hook (`noter`, optional, before_tool). The shipped shell gives its API token only to the Tauri host, so a temporary `crates/desktop-shell/examples/manual_shell.rs` pinned the token to drive the real binary; both the example and the scratch tree were deleted afterwards. Headless Chrome 155 over CDP at 1280×900 drove the real `app.js`: the served HTML/JS/CSS from the rebuilt binary contain the Hooks page, `renderSettingsHooks`, `showHookReviewDialog` and the apply-warning renderer; the review dialog showed title "This repository defines a hook", the full command `/bin/sh ./scripts/no-migrations.sh`, `Events: before_patch_apply, before_tool · If it fails: blocks`, **focus on Decline**, and closed on Decline; Settings > Hooks showed both rows with source/events/fail direction/activation and command lines; Test on the disabled user hook returned `before_tool: warn` with `debug: aws key [REDACTED_AWS_ACCESS_KEY_1a5d44a2dc]` and `data-state=warn`; Approve from the declined row changed it to `active`; screenshots inspected. By curl against the same process: a pending repository hook's test was refused 500; a stale-hash review was refused 500 and left user config untouched; an approval carrying a `path` naming the repository's `.damaian/config.conf` succeeded and left that file byte-identical while writing `hook_review.<id>.no_migrations` and `hook_reviewed` to the audit log; the reviewed hook then tested `deny`; the repository-config notice fell to `[]`; toggling the user hook wrote `hook.noter.enabled=false`, toggling the repository hook was refused 500, and the repository config stayed byte-identical. By CLI against the same tree: `hooks` printed both hooks with activation and hashes, `hook-review approve` then `hook-run` reported `deny`, `hook-review decline` showed `declined`, `hook-run` on an unknown event and an unknown hook each exited 1 with the message, and stderr stayed redacted. **Deviations:** `review_hook` takes the `Config` and repository root rather than `(data_dir, repository_id)`: the hash check needs the root, deriving the repository id from it removes a spoofable parameter, and the config supplies the audit settings and user-config path; a stale approval is refused instead of recorded, which is the same security property with a clearer state. `set_hook_enabled` takes the `Config` for the same reason, and refuses admin hooks as well as repository hooks because the admin file is applied last and would override the toggle. `test_run` takes the config, hook, event and root, and refuses an inactive repository hook rather than only the plan's `(hook, event)`. `HookListing` carries the command line, arguments, timeout and hash beyond the fields the plan named, because the review entry must show the command line and the UI must approve at the value it displayed. The findings panel has no source-filter control to extend; `FINDING_SOURCE_LABELS` is the one place a source is named, so `hook: "Hook"` there is the whole change. The apply warnings are shown inline in the patch card rather than as a toast, because the toast is a single element and would drop all but the last warning. A declined hook's row offers Approve (not Decline) so a mistaken decline can be undone from Settings. The plan listed only `ui-style-guide.html`; the guide's own rule that its prose and pixel page change together meant `UI_STYLE_GUIDE.md` changed too. **For Task 10:** document `damaian hooks`, `damaian hook-review` and `damaian hook-run`, redacted stderr, `hook_reviewed`, "changed since review", and that a pending/changed/declined repository hook cannot be test-run until approved; note that a decline is reversible from Settings and that the notice does not list declined hooks. The USER_GUIDE should say the Settings Hooks page shows "If it fails: blocks/warns", that a test run does not count as an invocation in the audit log (only `hook_reviewed` is written), and that the apply result carries hook warnings. TROUBLESHOOTING should give the CLI reproduction paths used here. No `chat.rs` change was needed. |
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
- CLI: `damaian hooks <repo>`, `damaian hook-review <repo> <id> approve\|decline`,
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
