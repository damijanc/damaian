# Spec 106 tasks

**Started:** 2026-11-09

Implementation plan for [`proposal.md`](proposal.md). The design decisions and
the code survey they rest on are in [`context.md`](context.md).

## Global constraints

- The worker, the engine and the completion gate decide; the driver only
  answers decisions the UI would present (context D5). It never edits the
  fixture repository, never runs Git there, and never sends `continue-task`,
  `continue-repair`, a `message` follow-up, a `note` or a round-budget
  approval.
- Success is computed from durable records (context D7), never from model text.
- The live tier is `#[ignore]`d, uses an isolated data directory and a freshly
  materialized fixture, reads its key from an environment variable only, and
  documents its exact manual command in its doc comment.
- Nothing here weakens command policy, patch hash verification, protected
  paths, secret redaction or the question/approval separation.
- Each task is a fresh agent session. Scoped checks per task; the workspace
  clippy and test suite stay with CI under `AGENTS.md`.

| Task | State | Durable result |
|---|---|---|
| 1 · Gate accepts an unchanged pre-existing failure (context D1) | Done 2026-11-09 | `CompletionGateReport` gains `accepted_failures: Vec<CompletionAcceptance>` (serde default, omitted when empty). `completion::accepted_pre_existing_failure` accepts a failed required `CheckRun` only when the classification was written after its latest run (new fold flag, `VerificationState::current_provenance`), is `pre_existing`/`unchanged_failure` with nothing introduced, and the task-start baseline is `Failed`; `uncertain`, declined, unavailable and absent baselines never qualify. Discovery while implementing: the gate change alone was not enough, because `finish_verification` turned any unrepairable failure into `VerificationOutcome::Failed` and settled as `VerificationFailed`; a new `OnlyPreExistingFailures` outcome (`only_pre_existing_failures`, never `passed`) now settles through the gate when every remaining failure is accepted. The completed task card (`completion_detail`, interactive and worker) names the accepted failure, and the Continue prompt lists it with "Do not try to fix it". Tests written first and recorded failing: the two accepting gate tests and both orchestrator tests failed with `RequiredCheckFailed: "Required check cargo test (repository root) failed: exit code 1."` (`closed_task_completion::pre_existing::*`, `plan_turn::an_unchanged_pre_existing_workspace_failure_does_not_stop_a_finished_task`, `plan_turn::a_regression_over_a_pre_existing_failure_is_repaired_then_the_task_completes`); guards for introduced findings, `uncertain`, declined baselines, a stale classification and the old wire shape; `task_control::tests::an_accepted_pre_existing_failure_is_named_on_the_card_and_in_a_continuation`. Three expectations changed on purpose, each pinning the old "pre-existing means failed" behaviour: `verification_loop::a_matching_failed_baseline_is_pre_existing_and_starts_no_repair` and `a_pre_existing_error_that_shifts_lines_stays_pre_existing` now expect `OnlyPreExistingFailures`; eval scenario `verification_pre_existing_failure` now expects `verification_outcome = only_pre_existing_failures` and `final_task_status = complete`. Their no-repair, provenance, repair-target and model-call assertions are unchanged. Not covered (observation 70): a plan-declared check the model runs itself records an unclassified `CommandExit`, so a pre-existing failure there still blocks its step; Task 2's fixture names scoped checks that avoid it. Specs 101, 102, 105 updated with dated notes. Local checks (2026-11-09): four-crate `cargo nextest` 3031 run, 3028 passed before the three expectation updates, then `verification_loop` + `harness` 217 passed; deterministic eval tier 62/62; scoped clippy (4 crates), fmt, typos, `node --check` clean. Workspace gate left to CI. |
| 2 · `single_worker_spec` fixture | Done 2026-11-09 | `crates/eval-harness/fixtures/single_worker_spec/` (`fixture.toml` version 1): a standalone three-crate Cargo workspace with a committed `Cargo.lock` and `.gitignore`. `pricing::to_cents` truncates mills to cents (the task makes it round); `invoice` converts every line through it and pins truncation in `tests::a_line_of_three_items_in_cents` (3 x 1.236 = 370), so a correct change breaks it: the regression to repair; `legacy::tests::parses_prefixed_legacy_codes` already fails: the pre-existing failure. Docs: `README.md`, an `AGENTS.md` modelled on this repository (scoped check `cargo test -p pricing -p invoice`, "ask when ambiguous", `crates/legacy/` owned by another team, the task-record rule) and `docs/specs/01_rounding/{proposal.md,tasks.md}` whose requirement 4 leaves the tie rule "to be confirmed with finance". **Deviation from the plan text:** the acceptance data (`acceptance.toml`) and the worker prompt (`prompt.md`) live in `crates/eval-harness/acceptance/single_worker_spec/`, not beside or inside the fixture: the fixture is copied into the repository the worker sees, and the scripted answer must not be readable there. New `eval_harness::acceptance` module with `AcceptanceFixture::load` (prompt, protected paths, expected command families, clarification keywords/answer/answer markers, expected tie values 234 vs 235 for a half-up guess, scoped check, pre-existing failure, regression, progress-record row); Task 3 extends this module. Tests written first (`crates/eval-harness/tests/single_worker_spec_fixture.rs`; failed to compile with `unresolved import eval_harness::acceptance` before the fixture existed): the acceptance contract; the prompt parses to the protected path `crates/legacy/`; no answer marker appears anywhere in the materialized fixture and `acceptance.toml`/`prompt.md` are absent from it; with the real toolchain, the workspace `cargo test --no-fail-fast` fails with exactly one failed test line, the legacy test, and the scoped check passes. Design check done by hand, not committed: applying a correct half-even `to_cents` broke exactly `invoice::tests::a_line_of_three_items_in_cents`, and the workspace check then failed on that plus the legacy test only. No scenario added; the deterministic tier stays at 62. Local checks (2026-11-09): `cargo nextest run -p eval-harness` 127 passed, deterministic tier 62/62, scoped clippy, fmt, typos, `git diff --check`, strict spec check clean. Workspace gate left to CI. |
| 3 · Acceptance driver, criteria and report (`eval-harness::acceptance`) | Done 2026-11-09 | Four library modules under `eval_harness::acceptance`, no process or network. `policy`: D6 as code: expected command families plus read-only exploration are approved; anything else policy allows is approved and recorded; commands with external markers (`curl`, `git push`, URLs, …) or absolute paths outside the repository are rejected and recorded; a protected-path patch fails, an out-of-repository patch is rejected; the material clarification (keyword match) gets the scripted answer, anything else the neutral answer. `driver`: a `Transport` trait with only the allowed operations (`next`, `approve_plan`, `decide_command`, `decide_checks`, `answer_question` with optional option index, `apply_patch` returning `ApplyStep::Applied | Baselines`, `decide_baselines`, `reject_patch`); there is no method for Continue task, Continue repair, a follow-up, a note or a round-budget approval, so the driver cannot send one. `drive()` answers each `Pending` per policy, applies patches through baseline cards, records every decision and deviation, and ends with `FailureReason::NeedsForbiddenIntervention` on a round-budget pause or a task that ended other than `complete`, `ProtectedPathProposed` on a protected proposal, or `TooManyDecisions` at the cap (default 400). `criteria`: ten requirement-8 criteria, each naming its durable reader and observed value: final task complete with an eligible gate, plan steps finished, required checks satisfied, the pre-existing failure accepted (Task 1), introduced failures repaired (plus whether a regression was observed), protected paths untouched (patch decisions, a before/after content-hash snapshot of the protected files, and the driver outcome), task record updated, no pending decision, clarification asked and answered (via `question_state`), no forbidden intervention needed. `report`: `AcceptanceReport` (metadata, verdict, criteria, decisions, deviations, metrics from durable readers: tasks, segments, within-turn compactions, tokens and whether estimated, questions, repair attempts, decisions by kind, changed paths), written as `report.json` and `report.md` under `<data_dir>/acceptance/<run-id>/`, and `return_summary()` for #100's worker return (D9): verdict, stop reason, failed criteria and every deviation. **Discovery:** the engine requires every question to carry 2–4 options and refuses a free-text answer when `allow_free_text` is false, so `Pending::Question` carries the options and that flag, the policy picks the option naming the scripted rule (an answer marker) when free text is not allowed, a forced first option for any other question is recorded as a deviation, and the clarification criterion accepts the scripted text or a matching option. Tests: 24 unit tests (policy 9, driver 6, criteria 7 on fabricated session logs through `SessionStore`'s public writers, report 3), including the forbidden-intervention, protected-path, deviation, decision-cap and option-answer cases and one failing case per criterion. Local checks (2026-11-09): `cargo nextest run -p eval-harness` 151 passed, 2 skipped; scoped clippy, fmt, typos, `git diff --check` clean. Workspace gate left to CI. |
| 4 · Deterministic tier through the in-process worker host | Not started | |
| 5 · Live tier through the real worker binary | Not started | |
| 6 · Three consecutive live acceptance runs | Not started | |
| 7 · Documentation and close-out | Not started | |

## Task 1 · Gate accepts an unchanged pre-existing failure

Requirement 11, context D1. Engine only (`completion.rs`, reading the
provenance records spec 102 already writes).

- [x] Failing tests first, through the orchestrator: (a) a required floor check
      fails at task start and after the change with the same failure → the task
      completes with a named "pre-existing failure, unchanged" disposition in the
      completion report, not a pass; (b) the same check with an extra introduced
      finding → still blocks (repair first); after the repair → accepted;
      (c) `uncertain` (no task-start baseline) → never accepted; (d) a declined
      baseline → not accepted; (e) generic (unparsed) checks use R10's identity.
- [x] Add the disposition to `CompletionGateReport` (serde default, so old
      reports load) and render it wherever waivers are rendered (desktop task
      control, CLI, recovery audit).
- [x] Update specs 101 (the gate and its waiver text), 102 (what provenance now
      decides) and 105 (the completion obligation), each with a dated note.
- [x] Scoped checks for workspace-engine, desktop-shell, damaian-cli,
      eval-harness; the deterministic eval tier.

## Task 2 · `single_worker_spec` fixture

Requirements 1–4, context §3.

- [x] `crates/eval-harness/fixtures/single_worker_spec/` with `fixture.toml`
      (version 1), the three crates, docs, the spec record, `AGENTS.md` and the
      worker prompt the acceptance run sends (stored in `acceptance/single_worker_spec/`, outside
      the fixture, so both tiers use the same text and the worker cannot read it).
- [x] The tie-rounding ambiguity and its two possible answers, with the expected
      test value for each, recorded in `acceptance/single_worker_spec/acceptance.toml` (question
      key words, scripted answer, expected value, protected path, expected
      command families).
- [x] A fixture test (not a scenario): materialize it, then confirm the
      workspace-root `cargo test` fails only on `legacy`, the `pricing` and
      `invoice` checks pass, and the protected path parses from the prompt.
- [x] No scenario count changes.

## Task 3 · Acceptance driver, criteria and report

Context D2, D5–D8. Library code in `eval-harness`, no process or network.

- [x] A transport trait the two tiers implement (status, approve/reject,
      checks, answer, apply-patch-then-verify), so the driver is shared and has
      no forbidden action to call.
- [x] The decision policy (D6) and the forbidden list (D5) as data, with unit
      tests: each forbidden action is unreachable; a protected-path proposal
      fails the run with its reason; an unexpected question, an unexpected
      command and an out-of-repository command are each answered or rejected
      and recorded as deviations without failing the run.
- [x] The report's Deviations section (D9), and a summary shaped for #100's
      worker return so it can be delivered into the parent session.
- [x] Criteria (requirement 8) evaluated from durable records (D7), each
      reporting the reader and value; unit tests on fabricated session logs for
      each criterion's pass and fail.
- [x] Report (§5 fields) as JSON plus Markdown under
      `<data_dir>/acceptance/<run-id>/`.

## Task 4 · Deterministic tier through the in-process worker host

Requirement 5, context D3.

- [ ] `desktop-shell` dev-depends on `eval-harness`; check `cargo deny`.
- [ ] One scripted run through `host_in_process` and the shared driver that
      exercises every transition: pre-plan exploration, plan review with
      budget, the clarification question and resume, two mutation batches,
      task-start baselines (pre-existing and introduced), one repair, an
      internal segment boundary, completion verification, the task-record
      update, and final `Complete` with D1's disposition.
- [ ] Negative guards: a scripted protected-path patch fails the run; an
      unexpected command fails it; a run left needing Continue fails with that
      reason.
- [ ] Runs in the workspace test suite; no side effects outside temp dirs.

## Task 5 · Live tier through the real worker binary

Requirement 6, context D4.

- [ ] An `#[ignore]`d `desktop-shell` integration test that materializes the
      fixture, writes `user.conf` for the selected provider, creates a parent
      (main) session and spawns the real worker from it with `Supervisor`,
      drives it with the shared driver over its socket, applies patches the way
      the shell does, writes the report and returns its summary, deviations
      included, to the parent session (D9).
- [ ] Doc comment with the exact command, including exporting the key for one
      run (for DeepSeek, from the Keychain item `deepseek-api-key`).
- [ ] A wiring smoke run with `DAMAIAN_MOCK_MODEL_RESPONSE` proving spawn,
      socket, shutdown and report writing (also `#[ignore]`, it spawns a
      process).

## Task 6 · Three consecutive live acceptance runs

Requirements 8–10. Needs Damijan's choice of provider and model and his
go-ahead for the cost before the first run.

- [ ] Run the live tier until three consecutive successes from fresh state.
      Record each run here: provider/model, duration, rounds, segments,
      compactions, tokens, approvals by kind, questions, repair attempts,
      outcome.
- [ ] A failed run resets the count; its cause is diagnosed from the report and
      recorded, and a product defect becomes its own fix before the next
      attempt.

## Task 7 · Documentation and close-out

- [ ] `USER_GUIDE.md`: what the acceptance gate proves and does not prove;
      `TROUBLESHOOTING.md`: running the live tier and reading its report.
- [ ] `docs/specs/README.md`, `CHANGELOG.md` Unreleased, dependency lines.
- [ ] Status Done only after Task 6's three runs **and** a green CI run
      (requirement 10).
