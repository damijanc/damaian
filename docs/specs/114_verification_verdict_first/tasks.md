# Spec 114 tasks

**Started:** 2026-11-25

Implementation plan for [`proposal.md`](proposal.md). The evidence, the code
survey and the decisions are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- **Do not change** task status semantics, command policy or the set of
  introduced findings repair receives, except as requirement 4 says.
  `summary()` stays byte-identical.
- **The engine owns every word.** Renderers print the verdict, labels and
  group names; they never derive them from enums or counts. This is the rule
  `provenance_label` already states.
- **Additive formats only.** New fields and events have serde defaults. No
  historical log has them, and replay must not invent them.
- **Order:** Tasks 1–3 edit overlapping engine files
  (`verification.rs`, `provenance.rs`, `completion.rs`), so run them in
  order.
- **Parallel work with spec 33** (context D8):
  - Tasks 1–3 may run alongside spec 33 Tasks 7–8. They share no files.
  - Task 4 (`app.js`, `style.css`) never runs at the same time as spec 33
    Task 9.
  - Task 5 never runs at the same time as spec 33 Task 10.
- **Checks:** scoped checks per task; the workspace gate stays with CI.
- **UI changes** are checked in the running app with
  `serves_the_ui_for_manual_inspection`, on its own port and data dir (never
  4765), and stopped by PID.

| Task | State | Durable result |
|---|---|---|
| 1 · Verdict and whose failure it is (req 1, 2, D1, D7) | Done 2026-11-25 | **Verdict.** `VerificationReport.verdict: Option<Verdict>` (`kind: VerdictKind`, `headline`, `detail`), computed once at the end of `VerificationReport::build` (`compute_verdict`) with D1's precedence. `build_failed` keys on `outcome == BuildFailed`; `caused_by_task` counts the checks in the `new_regression`/`worsened` groups ("This task's changes break N checks"); its detail depends on the outcome (repair limit → Continue repair; stall; no repair applied; a pre-finish report with targets → repair starting); `cause_unknown` = any environment/uncertain/unknown group; `not_caused_by_task` = only `pre_existing`/`unchanged_failure`; `not_verified` = skipped + stale + failed checks with no provenance entry ("no result was recorded": the run did not finish) — "Not verified: N checks did not run or finish" — and, with N = 0, due, `no_checks_available`, `only_documentation_changed` or nothing passed (own headlines: "…verification has not run yet", "…no checks were available", "…only documentation changed", "…no check has run"); otherwise `passed`. `Option` with `serde(default)`, so a stored report without one reads back as `None` (nothing invented). `render()` now starts with headline, detail and a blank line; the reloaded text and the CLI (which prints the turn response) get it through `render()`. **`summary()` is byte-identical:** it used `provenance_count`, which D7 rewords, so its old count phrases are frozen in a private `verification::summary_count` (**deviation**: D7 says `provenance_count` follows the new wording — it does, but `summary()` no longer calls it). **D7 wording** (`provenance.rs`): labels "Caused by this task", "Made worse by this task", "Failed before this task too", "Failing as before this task" (environment/uncertain/unknown unchanged); summaries and the pre-existing/unchanged action details say "before this task's changes"; `provenance_count` counts "checks caused by this task" etc. Action labels and `VerificationActionKind` unchanged; wire names unchanged. **Baseline line:** `ProvenanceCheck.baseline` (serde default, omitted when empty) from new `provenance::baseline_line(provenance, baseline, tests)`: "Before this task's changes: passed / the N failing tests passed when run alone / failed the same way / failed, but without the new findings / failed / could not run (…) / not checked: …" or "no baseline was recorded, so Damaian cannot compare"; rendered as an indented line under the check. **Baselines:** `VerificationReport.baselines: Vec<ReportBaseline {check, result, tests, label}>` for each failed, classified check that has a baseline, in selected order, plus a `Baselines` text section. Built from `VerificationState.baselines` and a new `VerificationState.baseline_tests` (folded from `verification_baseline.tests`) — the session fold the report already reads, not `task_verification_baseline_scopes`, so it agrees with the report's provenance groups. **Progress labels (D5):** `baseline.rs` running "Checking whether {check label} failed before this task's changes[, only the N failing tests]"; finished "Before this task's changes: {check label} passed / failed / could not run / was cancelled / was skipped" via the shared `verification::baseline_finished_label` (also the report's label) and `baseline_result_word` (moved from `baseline.rs`). The check label has no backticks (`cargo test (crates/a)`), unlike D5's sketch. **Typed marker — chosen: a flag.** `TurnPhase.baseline: bool`, set by `chat/verify.rs` `baseline_turn_progress` on the running, output and finished phases; `phase_json` writes `"baseline":true` only when set. A flag, not a `PhaseKind`, so a baseline's output stays an `output` line and every existing reader of `phase` is unchanged. Worker rows (`workers.rs`) do not forward it (spec 57/99 own them). **Tests:** new `tests/verification_verdict.rs` (10): every kind, D1 precedence (caused > unknown > not caused > not verified > passed), `render()` starts with the verdict, exact `summary()` strings for three fixtures, baseline lines and `baselines`, a stored report without a verdict. Updated label pins in `repair_targets`, `verification_loop`, desktop-shell `verification_report_json` test (now also asserts the verdict JSON); `repair_baseline`'s stop-at-start test keys on `phase.baseline`; desktop-shell `a_baseline_phase_carries_its_marker`. No eval assertion pinned the old labels. **Checks passed locally:** `scripts/quality-gate.sh fmt node lint typos deny`; `cargo clippy -p workspace-engine -p desktop-shell -p damaian-cli --all-targets --locked -- -D warnings`; `cargo nextest run -p workspace-engine --test verification_loop --test repair_provenance --test repair_baseline --test repair_targets --test verification_verdict --test plan_turn --test closed_task_completion --lib` (1144 passed); `cargo nextest run -p desktop-shell -p damaian-cli -p eval-harness` (542 passed, outside the sandbox); deterministic eval tier 66/66. Workspace clippy and tests left to CI. **For later tasks:** Task 2 adds `FailureProvenance::PassesAlone`: add it to `provenance_label`/`summary`/`action`/`provenance_count`, `build_partitions`' ORDER, the frozen `summary_count`, `baseline_line`, and to `compute_verdict`'s `not_caused_by_task` set (D3). Task 4 reads `report.verdict`, `report.baselines`, `provenance[].checks[].baseline` and the phase's `baseline` flag; `app.js` does not read any of them yet and still prints `report.outcome` as the badge. |
| 2 · Tests that pass alone (req 4, D3) | Not started | — |
| 3 · A failed task explains itself (req 5, D2, D4) | Not started | — |
| 4 · Report card, Baseline tag, failed task card (req 3, 5, 6, D5, D6) | Not started | — |
| 5 · Documentation and close-out | Not started | — |

## Task 1 · Verdict and whose failure it is (req 1, 2, D1, D7)

- [x] `VerificationReport.verdict` (`kind`, `headline`, `detail`), computed
      once in `verification.rs` with D1's precedence. `render()` starts with
      the headline. `summary()` is unchanged.
- [x] D7's labels, summaries and count phrases in `provenance.rs`. Each
      `ProvenanceCheck` gets an engine `baseline` line ("Before this task's
      changes: …").
- [x] `VerificationReport.baselines` (`check`, `result`, `tests`, `label`)
      from the baselines `finish_verification` already reads. Baseline
      running and finished labels per D5, and a typed marker on the baseline
      progress phase (a `PhaseKind` or a flag; choose one and record it).
- [x] Update any test, eval assertion or CLI output that pins the old
      labels. The CLI prints the verdict through `render()`.
- [x] Engine tests: every verdict kind, precedence, `summary()` unchanged.

Files: `crates/workspace-engine/src/{verification.rs,provenance.rs,baseline.rs,chat/verify.rs}`,
`crates/desktop-shell/src/json.rs` if the report JSON needs it, and tests.
Checks: `cargo clippy -p workspace-engine -p desktop-shell -p damaian-cli --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test verification_loop --test repair_provenance --test repair_baseline --lib`;
`cargo nextest run -p desktop-shell -p damaian-cli`; the deterministic eval
tier.

## Task 2 · Tests that pass alone (req 4, D3)

- [ ] **The rerun.** After a narrowed baseline records `Passed` for failing
      tests, run the same narrowed command once on the current tree, at the
      check's directory.
      - Run it only when the policy would run it without a card. Otherwise
        record it skipped.
      - It runs under the turn's cancel token. Stop records it cancelled and
        classifies as before.
- [ ] **The event.** `verification_isolated_rerun {taskId, check, tests,
      passedAlone, outcome}`.
- [ ] **Classification.** Findings that pass alone leave the introduced set.
      A check with none left is `FailureProvenance::PassesAlone`: not
      repair-eligible, with its label, summary, count phrase and action.
      Every exhaustive match is updated; let the compiler find them.
- [ ] **The gate** accepts `passes_alone` into `acceptedFailures`, with an
      explanation naming the tests.
- [ ] **Engine tests:** the three acceptance criteria for requirement 4
      (mixed, all-alone, skipped-for-approval) and a stop mid-rerun.
- [ ] **Eval:** one deterministic scenario for the all-alone case.

Files: `crates/workspace-engine/src/{baseline.rs,provenance.rs,verification.rs,completion.rs,chat/verify.rs}`,
`crates/eval-harness/` (scenario and any exhaustive match), tests.
Checks: scoped clippy for each crate touched;
`cargo nextest run -p workspace-engine --test repair_baseline --test repair_provenance --test repair_targets --test closed_task_completion --test verification_loop --lib`;
`cargo nextest run -p eval-harness`; the deterministic eval tier.

## Task 3 · A failed task explains itself (req 5, D2, D4)

- [ ] Add `CompletionReasonCode::RequiredCheckCouldNotRun` for
      `CheckOutcome::Environment`. Old reports keep `required_check_failed`.
- [ ] `TaskControl` carries `acceptedFailures` and an engine grouping of
      reasons (**Fails**, **Did not run or finish**, **Not caused by this
      task**). Use the grouping for `incomplete` too.
- [ ] `TaskControl` for `failed`:
      - the cause headline;
      - **Continue task** as its primary action, except when the
        verification ended `repair_limit_exhausted`. Then there is no
        primary, and the detail points to **Continue repair**.
- [ ] `SessionStore::continue_incomplete_task` accepts `failed` with the
      same exact-report check. The continuation prompt names the grouped
      reasons. The route and the worker transport need no change; confirm
      this and record it.
- [ ] Engine tests per the acceptance criteria. Desktop-shell route test: a
      failed task continues, and a stale report is refused.

Files: `crates/workspace-engine/src/{completion.rs,task_control.rs,session.rs}`,
the continue route in `crates/desktop-shell/src/` if it filters on status,
tests. Checks: scoped clippy;
`cargo nextest run -p workspace-engine --test closed_task_completion --lib`;
`cargo nextest run -p desktop-shell`.

## Task 4 · Report card, Baseline tag, failed task card (req 3, 5, 6, D5, D6)

- [ ] **Report card.** `createVerificationReportCard` shows the verdict
      headline and detail. The verdict is coloured by `kind` with existing
      state tokens (`--ok`, `--warn`, `--danger`); no new colour.
      - Today's rows move into one collapsed `createDisclosure` labelled
        with counts.
      - The baseline rows carry the Baseline tag.
      - **Continue repair** stays in the footer.
- [ ] **Baseline tag.** One `.baseline-tag` style. It is drawn on the
      running row (from the phase's typed marker), on a baseline check card,
      and on report baseline rows.
- [ ] **Task outcome card.** `renderTaskControlInto` draws a failed task's
      card, with or without a primary action, and its grouped reasons.
- [ ] **Repair turns and `ask_user`.** Check whether a repair turn offers
      `ask_user` (spec 113 requirement 4). Record the answer in this row. If
      it does not, record it as an observation; do not widen this task.
- [ ] **Documentation in the same task:** `docs/UI_STYLE_GUIDE.md` (verdict
      card, Baseline tag) and `docs/ui-style-guide.html` specimens.
- [ ] Desktop-shell `app_js_…` tests, then a manual check in the running app
      per the acceptance criteria. Measure the card's height before and
      after, and record both numbers.

Files: `crates/desktop-shell/static/{app.js,style.css}`,
`docs/UI_STYLE_GUIDE.md`, `docs/ui-style-guide.html`, desktop-shell tests.
Checks: `scripts/quality-gate.sh fmt node lint typos deny`;
`cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p desktop-shell`.

## Task 5 · Documentation and close-out

- [ ] **`USER_GUIDE.md` (Verification, Task incomplete):** the verdict, the
      Baseline tag, "passes when run alone", and the failed task's card with
      Continue task.
- [ ] **`TROUBLESHOOTING.md`:**
      - `verification_isolated_rerun` and `passes_alone`, with a `jq` over
        the session log;
      - `required_check_could_not_run`;
      - why a failure can read "not caused by this task".
- [ ] **Spec 102:** a dated note that its deferred flake classification is
      covered for narrowed Rust test baselines by this spec.
- [ ] **Spec 109:** a dated note on D3 (narrowed baselines and tests that pass
      alone).
- [ ] **Close-out per AGENTS.md:**
      - Status, this header, and the README row;
      - *What to build next*;
      - remove the CHANGELOG *Unreleased* line;
      - update the `Depends on:` lines that name #114.

Checks: `scripts/quality-gate.sh fmt node lint typos deny`;
`npm run specs:check -- --strict`.
