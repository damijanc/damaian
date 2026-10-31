# Feature Spec: A Rerun Reuses Its Plan Steps

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 92 of 92
Plan: none. A defect reported in use, like #73–#91.
Depends on: [#23](23_verification_loop/proposal.md) and
[#90](90_one_check_per_place_it_runs.md), both built.

## 1. Motivation

On 2026-10-31 a verification rerun said "Stopped before finishing" while its
`verification_finished` said `passed`.

- **Attempt 0:** `verify_1` ran `cargo test` at the repository root and
  `verify_2` ran it in `crates/workspace-engine`. Both failed.
- **Attempt 1:** after a repair patch, the rerun selected three checks: the
  root, `crates/desktop-shell` and `crates/workspace-engine`. Its
  `plan_revised` kept `verify_1` and `verify_2` and appended three more steps,
  numbered from `verify_1` again. The plan showed five steps, two of them
  duplicates.
- **The outcome:** the root passed and the two member checks were declined.
  Steps are closed by id, and the first `verify_1` is the old one, so the pass
  marked attempt 0's step and the new root step stayed open. The plan read
  "2 steps outstanding, 1 finished, 2 skipped".

#90 drops a member's `cargo test` when the root's covers it. That makes the
case rarer but does not fix the step bookkeeping.

## 2. Requirements

1. A rerun reuses the step of a check an earlier attempt already has. The
   same check is the same `CheckId`, meaning the same working directory and
   command.
2. A reused step is reset for the new attempt: pending, with no times and no
   evidence.
3. A step is added only for a check that is new in this attempt. Its id is the
   first `verify_N` the plan does not use yet.
4. Each check's latest outcome lands on its one step. When verification
   finishes, no `verify_` step of a selected check is left open.

## 3. Non-goals

- Plans already written with duplicate steps. They stay as logged. A later
  rerun of such a plan reuses the first matching step.
- Steps of checks an earlier attempt selected and this one did not. They keep
  their last outcome.

## 4. Design

- **`chat.rs`, `run_verification_checks`:** the step of a check is found by
  its detail, `Verification check: <label>`, which the check's `CheckId`
  decides. The browser check keeps its one `verify_browser` step and takes the
  new attempt's title.
- **`next_verify_step_id`:** the first `verify_N` past the highest the plan
  uses.
- **`reset_verify_step`:** clears status, times and evidence. The evidence
  goes because the repair request reads findings from every `verify_` step,
  and an earlier attempt's findings are what the repair already answered.
- **Spec 23 §5.5** says the rerun reuses the check's step.

## 5. Acceptance

`a_rerun_reuses_the_step_of_a_check_it_already_has` in
`tests/verification_loop.rs`: attempt 0 runs the root's `cargo test` and
fails. The repair changes the root and `packages/web`, so attempt 1 selects
both. After the rerun passes, the plan has one step per check, no duplicate
ids, and both steps are Completed. Before the fix it failed on a second
`verify_1`, left Pending.

## 6. Verification

Local checks, 2026-10-31: the fast checks, scoped clippy for workspace-engine,
and `cargo nextest run -p workspace-engine --locked`. The workspace clippy and
test suite are left to CI.
