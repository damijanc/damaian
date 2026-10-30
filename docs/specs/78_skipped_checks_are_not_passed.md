# Feature Spec: Skipped Checks Are Not Passed

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 78 of 78
Plan: none. A defect reported in use, like #73–#77: an exception to the rule
in [`README.md`](README.md).
Depends on: [#23](23_verification_loop/proposal.md) (verification loop),
built. Related: [#77](77_continue_the_task_after_verification.md).

## 1. Motivation

On 2026-10-30 a verification proposed two checks, `cargo test` at the
repository root and in `crates/workspace-engine`, and both were declined. The
session log recorded `verification_finished` with outcome `passed`.

The report's own summary said "No verification has run", so the user saw the
right words. But the outcome every other reader relies on said the opposite:
- the event in the session log;
- the report card's outcome badge, which showed "passed" in green;
- the `verificationReport.outcome` the shell returns.

#23 §3 is unambiguous: "an unrun check is never represented as passed".

The cause is in `finish_verification`. It recorded `Passed` whenever nothing
failed and nothing was stale, without asking whether anything passed. A shell
test, `resume_checks_endpoint_records_declined_checks`, had pinned that
behaviour by declining both checks and expecting `passed`.

## 2. Requirements

1. A verification where checks were selected, none failed, none is stale and
   none passed records `nothing_verified`, not `passed`.
2. Its summary says so: "No check ran: N checks skipped, so nothing was
   verified."
3. The task's status is still `complete`. Declining a check is the user's
   choice (#23 §5.3), not a failure of the task, and #77 still continues an
   open plan after it.
4. The report card shows the outcome as "nothing verified" in the same amber
   as a due verification, never in the green of `passed`.

## 3. Non-goals

- Changing how checks are selected, approved or skipped.
- A partial run. When some checks passed and others were skipped, the outcome
  is still `passed`, and the report lists each skipped check and why. That was
  already the case, and the summary names the count that passed.
- Rewriting old session logs. A `passed` recorded before this change stays as
  it was written.

## 4. Design

- `VerificationOutcome` (`verification.rs`) gains `NothingVerified`, which
  serializes as `nothing_verified`.
- `finish_verification` (`chat.rs`) chooses it over `Passed` when the
  preliminary report's `passed` list is empty, and the outcome-to-status map
  sends it to `Complete`.
- `VerificationReport::summary` has a branch for "nothing passed, something
  skipped".
- `style.css` gives `[data-outcome="nothing_verified"]` the due style. The card
  already renders any outcome name, with underscores turned into spaces.

## 5. Acceptance Criteria

- `declining_every_check_is_nothing_verified_not_passed`: declining the only
  check on the card gives:
  - the outcome `nothing_verified`;
  - an empty `passed` list and a non-empty `skipped` list;
  - a summary starting "No check ran:";
  - task status `complete`;
  - no `passed` outcome anywhere in the log.
- The shell's `resume_checks_endpoint_records_declined_checks` now expects
  `nothing_verified`.
- A verification with a passing check still records `passed`
  (`an_auto_runnable_check_runs_and_the_report_says_passed`).

## 6. Implementation Notes

#77's continuation keys on the task being `complete` with open plan steps, so
it needed no change. Its spec now lists `nothing_verified` among the outcomes
that continue. `TROUBLESHOOTING.md`'s list of `verification_finished` outcomes
and the user guide's report section name the new outcome.

## 7. Verification

The `workspace-engine`, `eval-harness` and `desktop-shell` suites passed
locally (2,455 tests). The fast checks and scoped clippy are in the change's
hand-back. The workspace clippy and test suite are left to CI.
