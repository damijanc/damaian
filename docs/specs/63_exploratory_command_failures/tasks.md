# Exploratory Command Failures Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md)
**Started:** 2026-10-17 (planned the same day, with references checked against the tree after #59–#64 were specified)

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `CommandPurpose`, proposal to execution | Done 2026-10-17 | New module `command_purpose.rs`: `CommandPurpose { Check, Exploration, Other }` (serde `snake_case`, `Default` = `Other`, `as_str`/`from_stored`, where an unknown value reads as `Other`) and `command_purpose(command, detected)`. Check means `detected`, `is_validation_command`, or any `default_parsers()` match. Exploration means every simple command is on the list, ignoring `cd <dir>`; a lone `cd` is `Other`. The split respects quotes and treats `2>&1` as a redirection. `git -C <dir> <read>` counts (C7). `CommandProposal.purpose` is computed in `proposal_from_classification` from `classification.command`, after the policy decided. It is stored as a trailing optional `PURPOSE` field (C2), and `store_proposal(…, detected)` is the private path that `propose_command_at` (`false`) and `propose_detected_validations` (`true`) share. `CommandExecution.purpose` comes from the new `CommandRunOptions::purpose`, which `run_proposal` fills (C3). `command_policy.rs`: one change, `is_validation_command` is now `pub(crate)`. Re-exported as `workspace_engine::{CommandPurpose, command_purpose}`. Test literals of `CommandExecution` and `CommandRunOptions` gained `purpose: Other` (finding.rs, its three parsers, one chat.rs test, command_runner.rs, tests/command_lifecycle.rs). **Tests:** 7 unit tests in `command_purpose::tests` and 5 in the new `tests/command_purpose.rs`. Among them, `purpose_never_changes_what_the_policy_decided` checks requirement 6: for 16 commands under 4 configs (default, approve-all, `command_access` ReadOnly and None), the stored proposal's command, risk, blocked, approval, reasons, effects and network flag equal `classify_command`, and `command_needs_approval` agrees. **Falsified:** dropping `detected` from `command_purpose` fails `a_detected_validation_is_a_check_whatever_its_text`. Clearing `requires_approval` for exploration in `proposal_from_classification` fails `purpose_never_changes_what_the_policy_decided`. Both were reverted. **Discovery (C4):** every command `detect_in` produces today is already on the validation list, so `detected` changes no current outcome and is tested at the function level only. **Checks (local, 2026-10-17):** fmt, node, lint and typos passed. `cargo deny` passed outside the sandbox; inside it, it cannot take its advisory lock. Scoped clippy for `workspace-engine` passed. `cargo check` of `desktop-shell`, `damaian-cli` and `eval-harness` is clean. `cargo nextest run -p workspace-engine`: 1210 of 1210 passed outside the sandbox; inside it, only the FSEvents watcher test failed. The full seven-check gate passed in CI on 2026-10-17. |
| 2 · No findings for exploration | Done 2026-10-17 | `findings_from_execution` (`finding.rs`) returns empty for `execution.purpose == Exploration` before any parser runs. The signature is unchanged, because Task 1 already put the purpose on `CommandExecution`, so `record_command_findings` (`chat.rs`) and both of its call paths are untouched. Proposal §5.2 is corrected to say so. **Tests:** in `finding::dispatch_tests`, `an_exploration_command_yields_no_findings` covers seven runs: a plain failure, one a parser explains, one with only warnings, a pass with warnings, a timeout, a signal and a cancellation. `a_check_and_an_other_command_keep_todays_findings` checks that the same runs give `Check` and `Other` exactly what they get today. In `tests/command_purpose.rs`, three chat-loop tests drive `MockModelAdapter` through a recording wrapper and approve when the policy asks. `failed_exploration_commands_record_no_finding_and_still_reach_the_model` runs the two §1 commands and `grep -rn nothing-matches .`, each with a non-zero exit in the tool result and no finding. `a_failed_other_command_records_a_generic_finding` runs `python3 script.py`. `failed_checks_record_findings_as_before` runs `cargo test` and `npm run lint`. **Deviations:** (a) The tests run under `/bin/sh`, which has no `nomatch`, so the §1 glob command fails because `grep` found nothing rather than because zsh refused it. It is the same exploration either way. (b) The search test keeps the data directory outside the fixture, because otherwise `grep -rn … .` matches its own session log. (c) With no `Cargo.toml`, `cargo test`'s finding comes from the Rust test parser reading cargo's `error:` line, not from the generic fallback, so the test asserts an error finding rather than its summary. **Discovery:** five existing tests used a failing `ls no-such-directory` to stand for "a failed command": four in `tests/finding_recording.rs` and `a_step_whose_command_failed_is_blocked_however_the_model_describes_it` in `tests/plan_turn.rs`. `ls` is exploration, so they now run an allowlisted repository script (`./list`, `./check`), which is `Other`, keeps the generic finding and still runs without approval. What they check is unchanged. Task 3 should know that the plan-blocking test no longer uses an exploration command. The eval scenario `failed_validation_retry` still runs `ls no-such-directory`. It checks the tool outcome, not findings, so it is unaffected here, but Task 5's per-purpose report will count it as exploration. **Falsified:** with the early return removed, `an_exploration_command_yields_no_findings` and the chat-level exploration test fail, and nothing else does. The early return was restored. Step 2's "confirm they fail" was shown this way, because the implementation was written before the first run. **Checks (local, 2026-10-17):** fmt, node, lint and typos passed. `cargo deny` passed outside the sandbox; inside it, it cannot take its advisory lock. Scoped clippy for `workspace-engine` passed. The verification-loop tests and `finding_recording` passed (8 of 8). `cargo nextest run -p workspace-engine`: 1247 of 1248 passed inside the sandbox; the one failure is the FSEvents watcher test, which needs a real FSEvents. Workspace clippy and tests are left to CI. |
| 3 · Plan evidence that does not block on exploration | Done 2026-10-17 | `Evidence::CommandExit` gained `purpose: CommandPurpose` with `#[serde(default)]`, so an event without it reads as `Other`. It is always written (`"purpose":"exploration"`, snake_case). `status_from_evidence` (`plan.rs`) returns not-blocked for any exploration exit before the existing `!= Some(0)` arm. Per C5, `ActionOutcome::CommandExit` now carries `(Option<i32>, CommandPurpose)`; the command arm in `chat.rs` fills it from `record.execution.purpose` and `evidence_for` copies it. The other `chat.rs` edit is the `finish_command_action` match, which ignores the purpose. No other `chat.rs` code changed. `desktop-shell/src/lib.rs`'s `sample_plan` literal and four literals in `tests/plan.rs` gained `purpose: Other`. **Decision (step 3):** an exploration exit of `None` (killed, timed out, cancelled) does not block, the default reading of §5.3. `Check` and `Other` still block on `None`. Proposal §5.3 records this. **Tests:** in `tests/plan.rs`, `a_failed_exploration_does_not_block_the_step` (exits 1, 2 and `None`), `a_failed_check_or_other_command_still_blocks`, `a_failed_check_blocks_even_beside_a_failed_exploration`, `a_command_exit_without_a_purpose_reads_as_other`, `the_purpose_is_written_in_snake_case`, and `a_pre_spec_63_log_replays_with_the_same_step_statuses`. That last test appends raw `plan_step_updated` events in the pre-spec shape (exits 0, 1 and `null`), replays them as Completed, Blocked and Blocked, all with purpose `Other`, and checks that `status_from_evidence` re-derives each stored status. In `chat.rs`, `a_failed_exploration_still_produces_evidence_carrying_its_purpose`, and the two existing `evidence_for` tests now carry a purpose. In `tests/plan_turn.rs`, `a_search_that_matched_nothing_does_not_block_its_step` runs `grep -rn nothing-matches src` through the chat loop. The step completes with exactly one `CommandExit { exit_code: Some(1), purpose: Exploration }` and no `Findings`, and the next step starts. **Deviation:** `grep` is not read-only by default, so it would raise the plan review. The test allowlists the exact command (`command_allowlist` matches whole commands, not program names). **Falsified:** without the exploration arm, `a_failed_exploration_does_not_block_the_step` and the chat-loop test fail, and nothing else does. The arm was restored. Step 2's "confirm they fail" was shown this way, because the implementation was written first. **Discovery, out of scope:** the plan panel (`app.js`, `entry.kind === "commandExit"`) does not read `purpose`. A completed step whose search found nothing shows "a command ran and exited 1". Task 6 or a follow-up may want to word it. **Checks (local, 2026-10-17):** fmt, node, lint and typos passed. `cargo deny` passed outside the sandbox; inside it, it cannot take its advisory lock. Scoped clippy for `workspace-engine` and `desktop-shell` passed. `cargo nextest run -p workspace-engine`: 1253 of 1254 passed inside the sandbox; the one failure is the FSEvents watcher test. `desktop-shell`'s plan tests passed (7 of 7). Workspace clippy and tests are left to CI. |
| 4 · The shell named in model-facing text | Not started | |
| 5 · Recording and the eval report | Done 2026-10-18 | **Engine:** `command_executed` (`command_runner.rs`) and `stored_command_executed` (`validation.rs`) carry `purpose`. `serialize_execution_summary` writes a `PURPOSE` field after `STDERR_REF`. Nothing reads summaries back, so no reader changed. **Eval harness:** `RunRecord.findings_by_purpose` (`findingsByPurpose`) holds `PurposeTally { commands, findings }` for check, exploration and other. It comes from `trace::findings_by_purpose`, which joins `stored_command_executed` (`commandId`, `purpose`; a missing purpose reads as `Other`) with each session log's `finding_recorded` `originRef`. Findings from a non-command origin (browser diagnostics) are not counted. There is a new metric, `exploration_findings` ("Findings from exploration commands", `KEYS` 21 → 22), with `count_or_no_data` over exploration commands, so it reports `no-exploration-commands` rather than a zero over nothing. The text report has a "Findings by command purpose (findings/commands)" section for runs that executed a command. A new scenario assertion, `exploration_findings`, fails when the run explored nothing. `failed_validation_retry` asserts it as `0`, `deterministic_only`. **No new scenario:** `failed_validation_retry` already fails `ls no-such-directory` eight times. `planned_task` also runs one exploration command. **Tests:** `both_execution_audit_events_and_the_stored_summary_carry_the_purpose` (`tests/command_purpose.rs`, an exploration and an `other` command through `run_proposal`). In `eval-harness/tests/harness.rs`: `findings_are_grouped_by_the_purpose_of_the_command_that_recorded_them` (hand-written logs, including a pre-spec event and a web origin), `a_failing_exploration_command_reports_zero_exploration_findings` (real run, record, text line, metric), `the_exploration_findings_metric_counts_a_regression`, `the_exploration_findings_metric_reports_no_data_when_nothing_explored` and `the_exploration_findings_assertion_fails_when_nothing_explored`. **Falsified:** with Task 2's exploration early return in `findings_from_execution` disabled, `a_failing_exploration_command_reports_zero_exploration_findings` and `control_flow_scenarios_pass` fail. So does `a_real_run_whose_commands_all_fail_reports_a_non_zero_error_rate`, which was not investigated. The deterministic tier then reports `exploration 8/8`, the metric reads 8, and the binary exits 1 on the `exploration_findings` assertion. The early return was restored. Step 1's "failing tests" was shown for the engine test (it failed before the audit fields existed) and by this falsification for the harness. **Step 3:** `cargo run -p eval-harness -- run --tier deterministic`: 16 of 16 scenarios pass. The report shows `failed_validation_retry exploration 0/8` and `planned_task exploration 0/1`, and the metric reads 0. **Not done:** `evals/baseline.json` was not regenerated, so it lacks `findingsByPurpose` and the new metric. DEVELOPMENT.md requires every number to be read before a new baseline is committed, which is the user's call. **Checks (local, 2026-10-18):** fmt, node, lint and typos passed. `cargo deny check` cannot take its advisory lock inside the sandbox; `cargo deny check licenses bans sources` passed, and no dependency changed. Scoped clippy for `workspace-engine` and `eval-harness` passed. `cargo nextest run -p eval-harness`: 71 of 71 passed, 1 skipped (the live tier). `cargo nextest run -p workspace-engine --no-fail-fast`: 1266 of 1267 passed inside the sandbox; the one failure is the FSEvents watcher test. Workspace clippy and tests are left to CI. |
| 6 · Docs and close-out | Not started | |

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`proposal.md`](proposal.md) §3 and §5 and the corrections below
  before starting.** Where a correction contradicts the proposal's prose, the
  correction wins, and the task that acts on it updates the proposal in the
  same change.
- **Purpose is a description, never a policy input** (requirement 6). Nothing
  in `command_policy.rs`, `mode.rs`, the approval path or `blocked` may read
  `CommandPurpose`. If a task needs that, it has left the spec. Stop and say
  so. `tests/command_purpose.rs::purpose_never_changes_what_the_policy_decided`
  must keep passing unchanged.
- **Unknown keeps today's behaviour** (requirement 4). Only `Exploration`
  changes anything. Every old record, missing field or unknown value reads as
  `Other`.
- **The split is not a parser.** `command_purpose.rs::simple_commands` is for
  description only. #64 has its own tokenizer for security decisions, and the
  two must not be merged.
- **Overlap with parallel work.** #64 rewrites `command_policy.rs`. This spec
  touches it only for the one visibility change made in Task 1. #60 edits
  `chat.rs` (tool definitions, read dispatch) and #62 rewrites the
  model-facing command text that Task 4 also edits. Keep `chat.rs` edits to
  what each task names.
- **Checks** (AGENTS.md, "The full gate runs on CI"): each task runs
  `scripts/quality-gate.sh fmt node lint typos deny`, scoped clippy for every
  crate it touches, and scoped nextest. The workspace clippy and test suite
  are left to CI.

## Corrections to the proposal found while planning

Found on 2026-10-17 by reading the code.

- **C1 · Where the purpose lives.** The proposal put `CommandPurpose` in
  `validation.rs`. It is in a new module, `command_purpose.rs`, so that the
  rules sit apart from both the policy and the store. `validation.rs` only
  calls it. *Folded into proposal §5.1 by Task 1.*
- **C2 · Stored proposals are not serde.** `CommandProposal` is written in the
  `DAMAIAN_COMMAND_PROPOSAL_V1` text format (`serialize_proposal`). Purpose is
  a trailing optional `PURPOSE` field, read the way `REPOSITORY_ROOT` already
  is, so a proposal pending across the upgrade reads as `Other`. *Folded into
  §5.1 by Task 1.*
- **C3 · How purpose reaches the execution.** `CommandExecution` is built
  inside `CommandRunner::run_at`, which re-classifies the command. Purpose is
  passed in through `CommandRunOptions::purpose` and copied, so the runner
  never computes or reads it. The proposal is the single decision point,
  because every agent command goes through `propose_command_at` +
  `run_proposal`: the auto-run path (`chat.rs`, around the
  `propose_command_at` call in the command tool branch) and the resume after
  approval (`chat.rs`, the `run_proposal` call before the first
  `record_command_findings`). *Folded into §5.1 by Task 1.*
- **C4 · `detected` is redundant today.** `detect_in` (`command_policy.rs`)
  produces only `npm run {test,lint,typecheck,build,format}`, `npm test`,
  `pytest`, `mvn test`, `gradle test`, `go test ./...` and `cargo test`, and
  all of them are on the validation list. The flag is kept as §5.1 asks, so a
  future detector cannot fall through to `Other`, but no integration test can
  observe it.
- **C5 · Plan evidence goes through `ActionOutcome`.** §5.3 names only
  `Evidence::CommandExit`. The command's evidence is built by `evidence_for`
  from `ActionOutcome::CommandExit(Option<i32>)` (`chat.rs`, top of the file),
  so Task 3 must give the outcome the purpose too. `desktop-shell/src/lib.rs`
  also constructs an `Evidence::CommandExit` literal. Check it when the field
  is added.
- **C6 · The model-facing text is static and pinned.** `run_command_tool_definition()`
  takes no arguments. The two envelope paragraphs are `&'static str`.
  `system_prompt(mode, user_present)` has no config.
  `code_mode_system_prompt_is_byte_identical_to_todays` and
  `with_a_user_present_every_modes_prompt_is_byte_identical_to_before` pin the
  current text. Task 4 threads the shell name through and updates those pins
  deliberately. #62 rewrites the same strings ("sandbox-safe"), so Task 4
  should not run beside #62. Whichever lands second rebases its wording onto
  the other's.
- **C7 · The exploration list gains `git -C <dir>`.** It is how the agent reads
  another worktree, and was one of the failing session's commands. Programs
  are matched by exact first word, so `./grep` and `FOO=1 grep` are `Other`.
  *Folded into §5.1 by Task 1.*
- **C8 · Recording has three places, and the eval harness has no findings
  report.** Command audit events are `command_executed`
  (`command_runner.rs`) and `stored_command_executed` (`validation.rs`), and
  `serialize_execution_summary` writes the stored summary. The eval harness
  `Run` (`eval-harness/src/runner.rs`) carries no findings at all, so §5.5's
  "findings per run grouped by purpose" is new reporting, not a new column. *Folded into §5.5 by Task 5.*

## File Structure

| File | Tasks |
|---|---|
| `crates/workspace-engine/src/command_purpose.rs` (new) | 1 |
| `crates/workspace-engine/src/validation.rs` | 1, 5 |
| `crates/workspace-engine/src/command_runner.rs` | 1, 5 |
| `crates/workspace-engine/src/command_policy.rs` (visibility only) | 1 |
| `crates/workspace-engine/src/finding.rs` | 2 |
| `crates/workspace-engine/src/chat.rs` | 2 (`record_command_findings`), 3 (`ActionOutcome`, `evidence_for`), 4 (`run_command` description, envelope paragraphs, `system_prompt`) |
| `crates/workspace-engine/src/plan.rs` | 3 |
| `crates/eval-harness/src/*` | 5 |
| `crates/workspace-engine/tests/command_purpose.rs` (new) | 1, 2, 3 |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `CHANGELOG.md`, `docs/specs/README.md` | 6 |

## Order and parallelism

```
1 ──┬──> 2 ──┐
    ├──> 3 ──┼──> 5 ──> 6
    └──> 4 ──┘
```

Task 1 comes first. Tasks 2 and 3 both edit `chat.rs` in different places
and may run in sequence or in parallel worktrees. Task 4 is independent of 2
and 3 but must not run beside #62 (C6). Task 5 records what 2 and 3 decide.
Task 6 closes the spec.

---

## Task 1: `CommandPurpose`, proposal to execution

**Requirements:** 1, 6. **Proposal:** §5.1. **Corrections:** C1, C2, C3, C4, C7.
**Files:** `command_purpose.rs` (new), `validation.rs`, `command_runner.rs`,
`command_policy.rs` (visibility), `lib.rs`, test literals in `finding.rs`,
`finding/{biome,rust_test,rust_diagnostics}.rs`, `chat.rs` (one test),
`tests/command_lifecycle.rs`, `tests/command_purpose.rs` (new).

- [x] **Step 1: Write the classification tests** in `command_purpose::tests`:
  the two commands from proposal §1 are `Exploration`; searches, listings
  and git reads are `Exploration`; validations and parser commands
  (`cargo clippy`, `npx biome check`) are `Check`; a detected command is
  `Check` whatever its text; unknown and mixed commands (`ls && rm -rf
  build`, `git branch -D`, a lone `cd`, `FOO=1 grep`) are `Other`; quoted
  separators do not split; stored values round-trip and an unknown one reads
  as `Other`.
- [x] **Step 2: Add `command_purpose.rs`** with `CommandPurpose` and
  `command_purpose`. Make `is_validation_command` `pub(crate)`. That is the
  only change to `command_policy.rs`.
- [x] **Step 3: Store the purpose on `CommandProposal`.** Compute it in
  `proposal_from_classification` from `classification.command`. Route
  `propose_detected_validations` through a private `store_proposal(…, true)`.
  Write `PURPOSE` as the last optional field, and read its absence as `Other`.
- [x] **Step 4: Carry it to `CommandExecution`** through
  `CommandRunOptions::purpose`, filled by `run_proposal`. Every other
  `CommandRunOptions` and `CommandExecution` literal gets `Other`.
- [x] **Step 5: Integration tests** in `tests/command_purpose.rs`:
  `purpose_never_changes_what_the_policy_decided` (requirement 6),
  `a_proposal_records_its_purpose`, `detected_validations_are_checks` (also
  after reloading), `a_proposal_stored_before_purposes_reads_as_other`, and
  `the_execution_carries_the_stored_purpose` (a failing `ls -d` under
  `/bin/sh`).
- [x] **Step 6: Falsify.** Drop `detected` and confirm the detected test
  fails. Make `proposal_from_classification` clear `requires_approval` for
  exploration and confirm the requirement-6 test fails. Revert both.
- [x] **Step 7: Scoped checks.** `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`,
  `cargo nextest run -p workspace-engine --locked`, and
  `cargo check -p desktop-shell -p damaian-cli -p eval-harness --all-targets --locked`
  (they use the changed structs). Results: see the progress row.

## Task 2: No findings for exploration

**Requirements:** 2, 3, 4. **Proposal:** §5.2. **Files:** `finding.rs`,
`chat.rs` (`record_command_findings` only), `tests/command_purpose.rs`, and
the existing finding tests.

- [x] **Step 1: Write the failing tests.** In `finding.rs`: a failed
  `Exploration` execution yields no findings, whatever its exit code or
  termination. `Check` and `Other` are unchanged: the existing parser and
  generic-fallback tests pass with the purpose set to each. Through the chat
  loop with `MockModelAdapter`: the two commands from §1 and
  `grep -rn nothing-matches .` record no finding, and their output still
  reaches the model as the tool result. `python3 script.py` failing records
  one generic finding. `cargo test` and `npm run lint` failing record
  findings as before.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.** `findings_from_execution` reads
  `execution.purpose` (it is already on the execution, so the signature need
  not change; if a parameter reads better, pass it from
  `record_command_findings` on both call paths). For `Exploration` it returns
  empty before any parser runs.
- [x] **Step 4: Falsify** by returning the generic draft for exploration
  again, and confirm the chat-level test fails.
- [x] **Step 5: Confirm #23 is untouched.** Run the verification-loop tests
  (`cargo nextest run -p workspace-engine -E 'test(verification)'`) and
  `tests/finding_recording.rs`.
- [x] **Step 6: Scoped checks**, then update this row and the proposal.

## Task 3: Plan evidence that does not block on exploration

**Requirements:** 5. **Proposal:** §5.3. **Corrections:** C5. **Files:**
`plan.rs`, `chat.rs` (`ActionOutcome::CommandExit`, `evidence_for` and its
callers), `desktop-shell/src/lib.rs` (the `Evidence::CommandExit` literal),
`tests/plan_turn.rs` or `tests/command_purpose.rs`.

- [x] **Step 1: Write the failing tests.** `status_from_evidence` does not
  block on a non-zero exploration exit, and still blocks on a non-zero
  `Check` or `Other` exit and on `None`. An `Evidence::CommandExit` JSON
  without `purpose` deserialises as `Other`. A plan replayed from a pre-spec
  session log has the same step statuses as before. Through the chat loop, a
  `grep` with no match leaves its step unblocked.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.** `Evidence::CommandExit { marker_id, exit_code,
  purpose }` with `#[serde(default)]`. `ActionOutcome::CommandExit` carries
  the purpose from the execution. In `status_from_evidence`, a non-zero exit
  blocks unless the purpose is `Exploration`. A `None` exit from exploration
  is still not a pass, and whether it blocks is a decision to record here.
  The proposal says only non-zero, and `None` is not zero, so the default
  reading is "does not block".
- [x] **Step 4: Falsify** by dropping the exploration arm and confirming the
  test fails.
- [x] **Step 5: Scoped checks** (include `cargo clippy -p desktop-shell`),
  then update this row and the proposal.

## Task 4: The shell named in model-facing text

**Requirements:** 7. **Proposal:** §5.4. **Corrections:** C6. **Files:**
`chat.rs` (`run_command_tool_definition`, the two
`command_envelope_paragraph_*` functions, `system_prompt` and its callers).

- [ ] **Step 1: Coordinate with #62.** If #62 has landed, take its wording as
  the base. If it is in progress, do not run this task beside it.
- [ ] **Step 2: Write the failing tests.** With `shell = "/bin/zsh"`, the
  `run_command` description and the envelope paragraph name `zsh -lc` and
  contain the glob-quoting sentence. With `/bin/bash`, they name `bash -lc`
  and omit it. Only the basename appears, never the full path.
- [ ] **Step 3: Implement.** Pass the shell basename into the description and
  the paragraphs. Update the byte-identical pins deliberately, and say in the
  progress row what text changed.
- [ ] **Step 4: Scoped checks**, then update this row and the proposal.

## Task 5: Recording and the eval report

**Requirements:** 1 (recorded). **Proposal:** §5.5. **Corrections:** C8.
**Files:** `command_runner.rs`, `validation.rs`, `crates/eval-harness/src/*`.

- [x] **Step 1: Write the failing tests.** `command_executed` and
  `stored_command_executed` audit entries carry `purpose`, and so does the
  stored execution summary (a trailing optional field). The eval report
  shows findings per run grouped by purpose, with exploration at zero for a
  scenario whose exploratory command fails.
- [x] **Step 2: Implement.** Decide where the eval harness gets findings
  (the session log the run wrote is the likely source) and add the grouping
  to the report. Add one deterministic scenario with a failing exploratory
  command, if the existing fixtures cannot show it.
- [x] **Step 3: Run** `cargo run -p eval-harness -- run --tier deterministic`
  (AGENTS.md: required after changing tool dispatch behaviour).
- [x] **Step 4: Scoped checks** (include `eval-harness`), then update this row
  and the proposal.

## Task 6: Docs and close-out

**Proposal:** §5.6, §6. Fresh session (AGENTS.md, final integration tasks).

- [ ] **Step 1: Walk §6's acceptance criteria** against the tree, and record
  how each is shown (test name or manual check).
- [ ] **Step 2: Docs.** In `docs/USER_GUIDE.md`, the findings section says
  which commands produce findings. In `docs/TROUBLESHOOTING.md`, add an entry
  for a failed command with no finding, and why.
- [ ] **Step 3: Close the spec** (AGENTS.md, "When a spec becomes Done"):
  Status, the README row and "What to build next", `Depends on:` references
  (`grep -rn "63_exploratory" docs/specs`), remove the `Unreleased`
  CHANGELOG item if one exists, the header of this file, and
  `npm run specs:check`.
- [ ] **Step 4: Checks.** Fast checks plus scoped clippy and nextest for every
  crate touched. The full gate runs on CI. Record the CI result here when it
  is green.
