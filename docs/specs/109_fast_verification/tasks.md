# Spec 109 tasks

**Started:** 2026-11-11

Implementation plan for [`proposal.md`](proposal.md). The decisions and the code
survey are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- Provenance meanings and the completion gate's requirements do not change;
  every task proves it against the existing provenance and gate tests.
- No command runs without the policy's approval where the policy asks (D5 is the
  only change: a session-scoped approval the user chooses, restrict-only).
- Never touch the user's checkout to build a pre-patch tree (D2).
- Confirm D2 at the start of Task 3 and D5's definition with the owner before
  Task 5 (both confirmed 2026-11-12). Task 5 builds on spec 111 Tasks 1–2.
- Scoped checks per task; workspace gate on CI. Compare timings within one
  checkout only (OBSERVATIONS 86).

| Task | State | Durable result |
|---|---|---|
| 1 · Reuse a result for an unchanged tree (D6) | Done 2026-11-11 | `CheckRun::reusable_for` (stricter than `!is_stale`: same generation **and** the same changed-file map with the same hashes, so a file changed after the run cannot hide; only an exited `Ran` outcome — passed or failed — is reusable; signalled, timed-out, cancelled, environment, skipped and browser runs rerun). `CheckReuse` is built in `verify_due` from the task chain's folded state (minus checks the plan defers to an open step) and passed to `propose_checks` / `propose_completion_checks` / `propose_checks_exactly` / `with_plan_checks`, which drop reused commands from the restricted map before `propose_detected_validations`, so nothing is stored, audited as proposed or run; all-reused is `Selection::Proposed([])`, not "no checks available". New event `verification_reused` (`VerificationEvent::Reused`, also accepted by `read_verification_events`) and audit `verification_check_reused`; it writes no `Check`, so the completion gate and provenance read the original `CheckRun`. `VerificationState::reused` (cleared by `Started` and by a later run) feeds `VerificationReport::reused`; the text report adds a "Checks reused" line (and drops "all ran") only when something was reused; the panel shows the same row. The spec 105 once-per-batch filter in `run_verification_checks` is unchanged — Task 2 owns selection. Tests (`verification_loop`): reusability rules, a reused check is never proposed, fold/report/serialisation, second verification of an unchanged tree runs nothing and the gate evidence is identical, a changed file reruns, a failed run is reused as failed. Checks passed locally: scoped clippy (`-p workspace-engine`); `cargo nextest run -p workspace-engine --test verification_loop --test plan_turn --test closed_task_completion` (239 passed, 19s wall in this worktree); `cargo check -p desktop-shell -p eval-harness --tests`; deterministic eval tier (62/62 pass; needs a local port, so it ran outside the agent sandbox); `scripts/quality-gate.sh fmt node lint typos deny` (deny outside the sandbox, which blocks its advisory-DB lock). Workspace clippy and tests left to CI. The full seven-check gate passed in CI on 2026-11-11 (Build macOS DMG workflow run 43083664918, Quality jobs, on the merged main). |
| 2 · Plan's scoped check per change (D4) | Done 2026-11-11 | `verification::scoped_selection` splits a change's owning roots (`select_roots`) into roots a plan check covers (its `CheckId` directory is the owning root or an enclosing map root) and roots that keep their detected checks; `propose_scoped_checks` proposes the uncovered roots' detected checks as `propose_checks` does and adds the covering plan checks through the same path as `with_plan_checks` (now a shared `append_plan_checks`, with reuse), via the new `ValidationOrchestrator::propose_scoped_plan_check` (same policy path, its own stored reason). With no plan checks it is exactly `propose_checks`. `verify_due`'s per-change branch calls it with the **approved** plan's `required` checks (`plan_required_checks` in `chat.rs`, normalized like `plan_defers_check`); an unapproved plan narrows nothing. Completion branch, answer-time exact floor run, `completion.rs` and `baseline.rs` unchanged. **Decision (recorded in context.md D4): deferral wins** — a covering check deferred to its open step still displaces the detected checks but is not proposed, because a loop `CheckRun` would override the step's later `CommandExit` in the gate; such a verification runs nothing for that root (`Proposed([])`). **Review follow-up (same day): an all-deferred verification says so.** Before, a per-change run whose only covering check was deferred ended `nothing_verified` with the generic summary "No verification has run." — not a false pass, but silent about where the check went. New event `verification_deferred` (`VerificationEvent::Deferred { check, step }`, accepted by `read_verification_events`), appended after `Started` for every check the run leaves to its open step — the D4-displaced plan checks and spec 105's existing deferral filter in `run_verification_checks` (`plan_deferring_step` names the step). It writes no `Check` and is not selected, so it is never gate or `complete_step` evidence. `VerificationState::deferred` (cleared by `Started` and by a later run) feeds `VerificationReport::deferred` (`DeferredCheck { check, step }`, omitted on the wire when empty). With nothing run the summary is "No check ran for this change: `<command>` in `<dir>` is left to its plan step \"<title>\"."; the text report and the panel (`app.js`) add a "Checks deferred" row only when present, so other reports render as before. The outcome stays the existing `nothing_verified` (never `passed`); the turn shows no card and continues the plan. `plan_turn::a_scoped_check_deferred_to_its_open_step_displaces_the_detected_check` now asserts the outcome, the summary and rendered text, that no `Check`/`CheckRun` exists for the deferred check, no check card, the carried report, and that the continued turn's gate reason is only `open_plan_steps`. Discoveries for later tasks: the completion floor of a member change in a virtual workspace is the root's `cargo test` only (spec 90 drops the member's); a scoped check on an early step whose command evidence goes stale when the next step begins is rerun by spec 106 Task 6 at completion — the step's `CommandExit` is not a `CheckRun`, so D6 reuse cannot cover it. Tests: `verification_loop::a_plan_check_covers_its_own_and_enclosed_roots_only`, `verification_loop::a_scoped_plan_check_is_reused_for_an_unchanged_tree`, `plan_turn::a_plans_scoped_check_verifies_a_change_and_the_floor_runs_once_at_completion` (spec 33 shape: the per-change run is only `cargo nextest run -p member --test a --test b`; completion runs the root `cargo test` once; the task completes), `plan_turn::a_scoped_check_deferred_to_its_open_step_displaces_the_detected_check`; a plan without a check keeps today's selection (`a_member_change_runs_only_the_members_check_per_change_and_the_root_check_at_completion`, the planless tests, and the no-plan-check case of the unit test). Checks passed locally (all re-run after the follow-up): scoped clippy (`-p workspace-engine`); `cargo check -p desktop-shell -p eval-harness --tests`; `cargo nextest run -p workspace-engine --test verification_loop --test plan_turn --test closed_task_completion` (243 passed, 18s; again 243 after the follow-up, 27s); deterministic eval tier (62/62, again after the follow-up, outside the sandbox for its local port); `scripts/quality-gate.sh fmt node lint typos deny`. Workspace clippy and tests left to CI. The full seven-check gate passed in CI on 2026-11-11 (Build macOS DMG workflow run 43083664918, Quality jobs, on the merged main). **Fix (2026-11-12): a finished step's check is not a per-change check.** As landed, `scoped_selection` took every approved required check covering the root while `plan_defers_check` defers only open steps' checks, so each finished step's check was proposed and run after every later patch (the owner's spec 33 plan: 5 steps, every check at the repository root); spec 106 Task 6's `plan_checks_needing_rerun` was also added to every post-apply verification, a second per-change path for the same checks. Now: `scoped_selection(map, changed, plan_checks, current)` — any approved required check still marks a root covered (no detected check per change), but only `current`, the in-progress step's required checks (`plan_current_step_checks` in `chat.rs`), stands in; a covered root with no current covering check goes to `ScopedSelection::completion_roots`. `propose_scoped_checks` now takes the `ScopedSelection` (clippy's argument limit) and runs nothing for those roots. Their detected checks (`detected_checks_of_roots`, nothing stored) are recorded as the new `verification_deferred_to_completion` event (`VerificationEvent::DeferredToCompletion`, `VerificationState`/`VerificationReport::deferred_to_completion`); the summary says "`<check>` is left to task completion" and the text report and panel share the "Checks deferred" row. `verify_due` adds `plan_checks_needing_rerun` only to a completion run (all primary steps terminal, or the answer-time due). To keep the worker's continuation moving, `CompletionGateReport::evaluate` no longer reports a finished step's stale check as `required_check_stale` while primary plan work is open or blocked — the same treatment as the floor; a failed or missing one still is, and completion requirements are unchanged (an open step already makes the task ineligible; the answer-time run reruns the stale check once, as before). D4, spec 105's Task 6 note and `USER_GUIDE.md` record the rule. Tests: `plan_turn::a_finished_steps_check_is_not_rerun_per_change_only_once_at_completion` (replaces `a_plans_scoped_check_verifies_a_change_and_the_floor_runs_once_at_completion`, which asserted the defect: three steps, every check at the root, driven through the model — a patch in step 2 runs no step-1 check and reports step 2's as left to its step; a patch in step 3, which has no check, runs nothing and reports `cargo test in crates/member` left to completion; both gates read only `open_plan_steps`; completion reruns each finished step's check once and the root floor once, and the task completes); `plan_turn::a_check_made_stale_by_a_later_step_is_rerun_once_at_completion` (renamed: 0 per-change runs, 1 at completion); `verification_loop::a_plan_check_covers_its_own_and_enclosed_roots_only` gains the finished-step and mixed cases; `closed_task_completion::every_blocking_fact_produces_its_stable_reason_code` asserts the stale reason only once no step is open; the four R14 baseline/provenance tests in `plan_turn` close the last step before patch 2's verification (`close_the_last_step`), because the loop's run of the finished "Validate" check now happens in the completion run. Checks passed locally for the fix: scoped clippy for `workspace-engine`, `desktop-shell` and `eval-harness`; `cargo nextest run -p workspace-engine --test verification_loop --test plan_turn --test closed_task_completion` (246 passed, 17s); deterministic eval tier (64/64 pass, outside the sandbox for its local port); `scripts/quality-gate.sh fmt node lint typos` and `cargo deny check` (outside the sandbox for its advisory-DB lock). Workspace clippy and tests left to CI. |
| 3 · Lazy baseline on a pre-patch worktree (D1, D2) | Done 2026-11-12 | D2 confirmed by the owner 2026-11-11 (recorded in context.md). No baseline before a patch or a command any more: `finish_verification` calls the new `BaselineCoordinator::prepare_for_failures` for each selected check that failed with a code result and has no baseline in the task chain, then classifies with the unchanged `provenance::classify`; a passing, skipped, declined, environment-failed, cancelled or browser check takes none. The baseline runs the same command through the ordinary policy in a **task-start tree** (new `pre_task_tree.rs`): a shared clone (`git clone --shared --no-checkout`, hooks off, nothing written to the user's `.git`) at `HEAD`, the user's other changed/untracked paths copied as they are now, and every path the task chain changed put back to its earliest recorded pre-change bytes — a patch's rollback snapshot (`patch_engine::read_pre_apply_content`, verified against the reviewed base hash) or a turn checkpoint's snapshot (verified against its hash). Nothing new is captured at task start. Anything inexact (no record, hash mismatch, a command's change without checkpoint coverage, not a Git checkout, submodules) is `CheckBaseline::Unavailable` → `UnknownProvenance`. The tree lives under `<data_dir>/baselines/trees/` and is removed on every exit path (Drop guard). `ValidationOrchestrator::run_proposal_in_tree` runs the stored, already-decided proposal there and rewrites tree paths in its output to the repository's. Baselines run with `CARGO_TARGET_DIR=<data_dir>/baselines/cargo-target/<hex>` (hex only: a `:` broke cargo's `DYLD_FALLBACK_LIBRARY_PATH`). A baseline that needs approval pauses the verification on the existing check card with an engine title and note ("This check failed. Run it on the files as they were before this task's changes…"); R18's `command_baseline` pause is repurposed as `failure_baseline`; the answer runs spec 107's streamed `resume_streamed` under the turn's cancel token (Stop → `Declined`) and finishes the same verification. **Deviation from D2's wording:** the tree is the task-start state, not the state just before the patch under verification ("task-start uncommitted state + earlier patches"): a baseline is stored once per check per task chain and reused, and a completion failure has no single patch, so only task-start has one meaning; it also keeps a patch-1 regression first seen after patch 3 `new_regression` (context.md D2). **Known limit:** a path the task did not change is taken as it is now, so the user's own mid-task edit counts as task-start state. Removed as dead: `prepare`, `prepare_for_command`, `PatchApplyOutcome` / `apply_stored_patch_with_baseline` and the patch-baseline edit methods, the patch card's baseline card and its JS helpers/CSS, `/api/resume-baseline(-stream)`, the `baseline` pending-decision kind, `command_floor_checks`, `RepositoryMap::roots_for_command`, `is_read_only_pipeline`, `is_source_writing_command`, the CLI baseline prompt and the acceptance driver's `Baselines` step. `Uncertain` stays (old logs; a cancelled required check). Tests: `repair_baseline` rewritten (11 tests, real Git fixtures and a fake suite: spec 102's four classes — new regression, pre-existing (from uncommitted task-start state), worsened with only the new finding introduced, unchanged; a passing check never baselines; a reconstruction mismatch is `UnknownProvenance` and runs nothing; the checkout and its `.git` are byte-identical; approval asks once with the why and then classifies; cancel before and during a run is `Declined` with the tree removed; no Git is `Unavailable`; every test asserts no tree is left). `plan_turn`: R18 tests removed, the command-first test converted (baseline from the command's checkpoint → `new_regression`), R14/R15 tests converted to the task-start classification, the uncertain-question tests now seed a historical `Uncertain` baseline. `verification_loop`: fixture helpers mark baselines from `floor_checks`, the real-baseline helper commits a Git task-start state, R16 (f) now asserts no baseline before apply. Shell: `a_check_card_shows_the_engines_title_and_note_when_it_has_them`. Eval: `verification_environment_failure` now expects no baseline (an environment failure takes none); scenario count unchanged (63). Checks passed locally: scoped clippy (`-p workspace-engine -p desktop-shell -p eval-harness -p damaian-cli --all-targets`); `cargo nextest run -p workspace-engine` for `repair_baseline`, `verification_loop`, `plan_turn`, `closed_task_completion`, `deferred_check_waiver`, `repair_provenance`, `repair_targets`, `repair_stagnation`, `generic_failure_identity`, `repository_map`, `checkpoints`, `crash_recovery`, `command_lifecycle` (445 passed) and `--lib` (736 passed); `cargo nextest run -p desktop-shell -p eval-harness -p damaian-cli` (469 passed, outside the sandbox for local ports); deterministic eval tier 63/63 (33s wall, outside the sandbox); `scripts/quality-gate.sh fmt node lint typos deny`. Workspace clippy and tests left to CI. For later tasks: Task 4's targeted rerun belongs in `run_in_task_start_tree`/`run_baseline` (`baseline.rs`), which already has the failed check; Task 6's running state can reuse `baseline_turn_progress` (phase `Running baseline: …`); Task 7 should document the cargo cache and the user-edit limit in USER_GUIDE/TROUBLESHOOTING. |
| 4 · Rerun only the failing tests (D3) | Not started | — |
| 5 · Approve for this session (D5) | Not started | — |
| 6 · Running state and Stop for every running action (D7, D8) | Not started | — |
| 9 · A build check after every change (D10) | Done 2026-11-13 | `PlannedCheck` gains an optional per-step `build` (plan format version bumped to 2; a version 1 plan loads with none). `propose_plan`'s tool schema and parsing (`chat.rs`, `planned_check_from_json`, shared with `check`) accept a `build` object per step and force `required = false` on it, since a build is feedback only; the planning prompt tells the model to give each code-changing step one when the project has a compile command. The shell's `plan_json`/`check_json` carry and show a step's `build` beside its `check`; the plan review card (`app.js`, `planBuildTitle`) renders it as "Build check: …", never marked required. Fast path: `build_checks_for_change` (`verification.rs`) returns, per touched root, the current step's build check when its directory covers the root (D4's coverage rule) else `CARGO_BUILD_CHECK` ("cargo check --tests") when the root has a `Cargo.toml` manifest, else nothing — no language list (D9). `verify_due`'s per-change branch (`verify_with_task`, split out of `verify_due_report`) runs these builds first, before any check is selected: each is reused from the task chain's latest run over the same tree (D6, `reusable_build`/`BuildRecord::reusable_for`, the same rule as `CheckRun::reusable_for`), or proposed through the ordinary policy (`ValidationOrchestrator::propose_build_check`) and run. A build that needs approval pauses the turn on its own card (title "Build check", an engine note) **before anything else is proposed**, via `PendingBuildStage`/`BuildStage`; approving or declining it resumes through the same `resume_after_check_decision` → `resume_build_stage` → `verify_with_task` path, which then continues the same verification (selecting its checks next). Every build (run or reused) is recorded as the new `verification_build` event (`VerificationEvent::Build`, `BuildRecord`) — never a `Check`/`CheckRun`, so the gate, `complete_step`, a plan step's evidence and provenance never read it; `VerificationState::builds`/`build_results` and `VerificationReport::builds` (`BuildReport`) carry it to the report's new "Build" row (text report and the shell panel), which never counts toward `passed`/`failed`. On a build failure (`BuildRecord::failed`, an exited non-zero run) the verification ends at once with the new outcome `build_failed`: nothing else is selected, no baseline is taken (`finish_build_failed` calls `finish_verification_without_checks`, never `finish_verification`), and `VerificationReport::build_failure_prompt` hands the continuation turn (`continue_task_after_verification`) the compiler's errors — parsed from the same `findings_from_execution` a check's findings use (so already secret-redacted), `BuildErrors::from_findings` bounding them to the first 5 in full plus a count of the rest (`MAX_BUILD_ERRORS_SHOWN`, `MAX_BUILD_ERROR_DETAIL_CHARS`), each located error resolved against the task's changed-file set and marked `outside_task` when it is not one the task touched. Tests (`verification_loop`): a failing build reaches the next model request with its errors and takes no baseline and no card; a passing build is never gate/`complete_step`/provenance evidence (a build whose command equals the step's required check still writes no `CheckRun`); an unchanged tree builds once and a changed file builds again; a non-Cargo root with no build check runs none; a build needing approval pauses on its own card before any check is selected and the verification continues correctly whichever way it is decided; the event's wire shape, fold and report rendering, including the "Build" row and the `BuildFailed` summary phrasing. Unit tests (`chat.rs`, `plan.rs`): `propose_plan` parses a `build` object and forces it non-required, drops one with no command; a version 1 plan loads with no `build` and round-trips; the shell's `plan_json` test (`lib.rs`) asserts the card's `build` key. Deviation from the written plan: the build card's **title** doubles as the stage marker read by `card_decisions` in the eval harness and by `resume_build_stage` in the engine (any batch whose title is `BUILD_CHECK_CARD_TITLE`), rather than a separate pending-decision kind, since every build card in a verification is this stage and nothing else shares its shape. Existing-test discovery: several already-passing fixtures detect a Cargo root and now get an auto-runnable `cargo check --tests` card where none ran before, so their `command_allowlist`s (`verification_loop.rs`, `plan_turn.rs`) gained the matching allow entries (including `cd <member> && cargo check --tests` for monorepo member roots, mirroring the existing `cargo test` entries) rather than changing behavior to avoid them. Eval: `crates/eval-harness/scenarios/build_check_after_each_change.toml` — a planless three-patch task on the `rust-workspace` fixture (patch 1 builds and passes; patch 2 references an undefined constant and fails the fast-path build, ending `build_failed` with the compiler's `cannot find value` error and no baseline; patch 3 defines it, the build and the check both pass, the task completes) — needed two small harness additions to assert it: `card_decisions` in `runner.rs` now treats only a title other than `BUILD_CHECK_CARD_TITLE` (re-exported from `workspace_engine`) as the failure-baseline card, so a build card is decided the ordinary way; a new `build_failure_request_contains` assertion (`scenario.rs`/`assertions.rs`/`runner.rs`, located by the engine's own `verification_build_recorded` audit event whose `result` starts with "does not build", mirroring `continuation_request_contains`'s pattern) proves the compiler's error reached the model's next request. Both scenario counts in `eval-harness/tests/harness.rs` moved 64→65 and 63→64. The repair-oscillation scenario's continuation loop budget (`run.rs`) doubled 8→16, since every per-change verification in a Cargo fixture may now pause once more for its build card before its check card; the `propose_plan` tool's `build` schema text was kept terse (object shape only, no worked examples) after the first draft pushed `bounded_worker_long_read_reread`'s system-prompt token count over its 60000-token `context_window`, flipping it from `task_compaction_cannot_fit`'s absence to its presence — a token-budget interaction worth knowing before any later task grows a tool description. Checks passed locally: scoped clippy for `workspace-engine`, `desktop-shell`, `eval-harness` (`--all-targets --locked -- -D warnings`, all clean); `cargo nextest run -p workspace-engine --test verification_loop --test plan_turn --test closed_task_completion` (242 passed); `cargo nextest run -p desktop-shell` (326 passed, 20 skipped — real-side-effect tests, unrelated to this task); deterministic eval tier (65/65 pass); `scripts/quality-gate.sh fmt node lint typos deny` (all `ok`; `lint`'s one pre-existing `noDescendingSpecificity` warning on an unrelated selector at `style.css:1780` predates this change, confirmed by stashing this task's `style.css` edit and rerunning). Workspace clippy and the full workspace test suite are left to CI. For Task 7: no measurement of the spec 33 Task 1 shape was taken in this task — that re-run belongs to Task 7's own close-out, now with a build check in the mix. |
| 8 · Verification with no detected check (D9) | Not started | — |
| 7 · Measurement, documentation and close-out | Not started | — |

## Task 1 · Reuse a result for an unchanged tree (D6)

- [x] Before proposing a check, reuse a non-stale `CheckRun` of the same
      identity; record the reuse in the verification events and the report.
- [x] Tests: a second verification of an unchanged tree runs nothing; any file
      change or mutation generation makes it run.

Files: `verification.rs`, `chat.rs`, tests. Checks: scoped clippy;
`cargo nextest run -p workspace-engine --test verification_loop --test plan_turn`.

## Task 2 · Plan's scoped check per change (D4)

- [x] Per-change selection prefers the plan's required check for the owning (or
      enclosing) root over the detected root check; the floor still runs once
      at completion.
- [x] Tests: the spec 33 shape (plan with `cargo nextest … --test a --test b`
      in the crate) verifies a patch with the scoped check only; completion runs
      the floor; a plan without a check keeps today's selection.

Files: `chat.rs`, `verification.rs`, `completion.rs` (if the gate's evidence
lookup needs it), tests. Checks: as Task 1 plus `--test closed_task_completion`.

## Task 3 · Lazy baseline on a pre-patch worktree (D1, D2)

- [x] Confirm D2 with the owner. (Confirmed 2026-11-11.)
- [x] Remove the pre-apply baseline card and R18's pre-command baseline; take a
      baseline only when a check fails and the task has none for it.
- [x] Build the pre-patch tree in a temporary worktree (task-start uncommitted
      state + the task's earlier patches, hash-verified); `Unavailable` when it
      cannot be reconstructed; always cleaned up. (Built as the task-start
      tree in a shared clone; see the progress row and context.md D2.)
- [x] Tests: the spec 102 provenance fixtures produce the same classifications;
      a passing check never baselines; a reconstruction mismatch yields
      `UnknownProvenance`; the user's checkout is untouched.

Files: `baseline.rs`, `edit.rs`, `verification.rs`, `chat.rs`, tests.
Checks: scoped clippy; `--test repair_baseline --test verification_loop
--test plan_turn`; `cargo run -p eval-harness -- run --tier deterministic`.

## Task 4 · Rerun only the failing tests (D3)

- [ ] For Rust test / nextest findings, the failure-time baseline runs only the
      failing tests by name; other checks rerun whole.
- [ ] Tests: two failing tests rerun alone; a mixed failure (one pre-existing,
      one introduced) classifies as worsened with the right introduced finding.

Files: `baseline.rs`, `finding/`, tests. Checks: as Task 3.

## Task 5 · Approve for this session (D5)

- [x] **Confirm with the owner** D5's definition of a command that does not
      change the repository: decided by the policy's approval causes and each
      run's observed effect, with no ecosystem list, plus a user config entry
      (confirmed 2026-11-12).
- [ ] Policy: `session_approvable(command, dir)` from the command's
      `ApprovalCause`s and `is_source_writing_command` (D5), with no ecosystem
      list (Task 3 removed `is_source_writing_command` as unused; restore it
      from history); tests for each list
      entry and each refusal reason.
- [ ] Built on spec 111 Tasks 1–2: **Approve for this session** writes a
      session-scoped `allow` rule of the exact parsed command and directory;
      user additions are ordinary spec 111 user rules.
- [ ] Session events and the approval path: choosing it records the approval;
      later proposals of the same command and directory in the session run
      without a card, audited; repository config can only disable it.
- [ ] Backstop: before/after repository state around a covered run; a change
      revokes the approval and is reported.
- [ ] UI: the Approve chevron with **Approve for this session** on command,
      check and baseline cards; the run row says so; **Revoke session
      approvals** in the session ⋮.
- [ ] Tests: approved once per session; another command, directory or session
      still asks; a `&&` chain, a `--fix` and an unknown command are never
      offered; unknown programs from several ecosystems (`stack test`, `lein
      test`, `go test`, `phpunit`, `ctest`) are offered and a `--fix` variant
      is not; a covered run that changes a file revokes it; a restart keeps
      it; archive ends it; a repository cannot enable or widen it.

Files: `command_policy.rs`, `validation.rs`, `session.rs`, `chat.rs`,
`app.js`, desktop-shell `routes.rs` (classify any new route), tests.
Checks: scoped clippy; the policy tests and `--test plan_turn`;
`cargo nextest run -p desktop-shell`; web checks.

## Task 6 · Running state and Stop for every running action (D7, D8)

- [ ] One running-state component for checks and baselines: header indicator,
      Cancel in place of the run button, status line and composer line from the
      same state.
- [ ] D8: a visible **Stop** for every running action, including the
      post-apply verification started from the patch card, commands, hooks, MCP
      calls and browser diagnostics; all runners honour the turn's cancel token
      by ending their process group; a stopped side-effecting action is recorded
      with an unknown outcome and named in the report; nothing retries
      automatically.
- [ ] Tests per runner: Stop ends the process group, records the outcome, and
      leaves no running child (process registry empty).
- [ ] Manual check in the running app; UI tests.

Files: `app.js`, `style.css`, `lib.rs` (if the stream needs a field), tests.
Checks: web checks; `cargo nextest run -p desktop-shell`.

## Task 9 · A build check after every change (D10)

Ready now: depends only on Task 2 (done). Highest value of the open tasks for a
running Damaian; build it before Tasks 4, 6 and 8 unless one of them is already
in progress. Task 5 later removes its per-patch approval.

- [x] `PlannedCheck` gains an optional per-step `build` (plan format version
      bump, old plans load without one); `propose_plan` and the plan review
      card carry and show it; the planning prompt asks for one per step.
- [x] Fast path: no step build check and a detected Cargo root → `cargo check
      --tests` for the owning package.
- [x] `verify_due`'s per-change branch runs the build checks first, with reuse
      (D6), recording `verification_build` (no `CheckRun`); a failure ends the
      verification `build_failed` with no baseline and hands the bounded,
      redacted errors to the continuation turn, marking errors outside the
      task's changed files.
- [x] Report and panel: a "Build" row.
- [x] Tests: a passing build is never gate or `complete_step` evidence; a
      failing build reaches the next model request with its errors and takes no
      baseline; an unchanged tree builds once; a step without a build check on a
      Cargo root uses the fast path; a non-Cargo root without one runs none; a
      deterministic eval scenario whose second patch breaks the build (update
      both scenario counts in `eval-harness/tests/harness.rs`).

Files: `plan.rs`, `chat.rs` (`verify_due`, the planning prompt),
`verification.rs`, `validation.rs`, `app.js` (plan card, report row), tests,
eval scenario. Checks: scoped clippy; `cargo nextest run -p workspace-engine
--test verification_loop --test plan_turn --test closed_task_completion`;
`cargo nextest run -p desktop-shell`; deterministic eval; web checks.

## Task 8 · Verification with no detected check (D9)

Build before Task 7.

- [ ] Completion floor: a touched root with no detected check takes the
      approved plan's required checks covering it.
- [ ] User-scope `project_checks` (exhaustive overlay classification; a
      repository may only narrow), added to the floor; editable in Settings.
- [ ] Planning prompt: name the repository's declared checks (CI files,
      `Makefile`/`justfile`, README) as single commands and say where they came
      from.
- [ ] Tests: a fixture repository with only a `Makefile` (and one with a
      `stack.yaml`) is verified by its plan's checks, not "no checks
      available"; a `project_checks` entry joins the floor; a repository value
      cannot add one; the deterministic eval gains one such scenario (update
      both scenario counts in `eval-harness/tests/harness.rs`).

Files: `completion.rs`, `verification.rs`, `config.rs`, the planning prompt,
`app.js`, tests, eval scenario. Checks: scoped clippy; `--test
closed_task_completion --test verification_loop --test plan_turn`;
`cargo nextest run -p desktop-shell`; deterministic eval.

## Task 7 · Measurement, documentation and close-out

- [ ] Re-run the spec 33 Task 1 shape on Damaian's own repository and record
      wall time and the checks run per patch against the 2026-11-11 run.
- [ ] `USER_GUIDE.md` (verification and baselines), `TROUBLESHOOTING.md`.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.
