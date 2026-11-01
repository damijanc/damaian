# Feature Spec: Repair Only New Failures and Stop When Repair Stalls

Status: Not started
Order: 102 of 106
Plan: none. This is a failure-driven specification from the 2026-10-31
single-worker attempt.
Depends on: [#23](23_verification_loop/proposal.md) (verification and repair),
[#92](92_rerun_reuses_its_plan_steps.md) (stable check steps), and
[#98](98_task_scoped_protected_patch_paths.md) (durable no-touch paths) — #23,
#92 and #98 are built.
Everything else named below is a cross-reference, not a prerequisite.

## 1. Motivation

The verification loop starts after an applied patch. When a selected check
fails, it gives the findings to the model as a repair target. It does not first
prove that the check passed before the patch. A pre-existing failure, missing
tool or environmental error can therefore send a worker into repeated source
edits unrelated to its task.

The same incident raised the repair limit from two attempts to sixteen (#96),
but attempt count is not progress detection. A later repair proposed deleting
a file the user had explicitly excluded (#98). More attempts without
provenance, scope and stagnation checks increase the damage window.

## 2. Requirements

1. Before the first approved patch that selects a check is applied, Damaian
   offers to run that check against the current pre-patch state. The result is
   the check's task-local baseline. A check selected later is baselined before
   the patch that first selects it.
2. Baseline commands keep their ordinary approval requirements. Patch approval
   does not approve a baseline check, and declining the baseline does not
   decline the patch.
3. Every check result has a stable failure fingerprint derived from check
   identity, normalized findings and termination kind. Raw output and secrets
   are not part of the fingerprint.
4. Post-patch comparison classifies a result as `new_regression`,
   `pre_existing`, `worsened`, `unchanged_failure`, `environment_failure` or
   `unknown_provenance`. Only `new_regression` and the new portion of
   `worsened` enter automatic repair.
5. Spawn failures, missing executables, policy refusals and unavailable test
   infrastructure are environment failures. They are reported with an action
   for the user; they are never given to a code-repair turn.
6. If a baseline was declined or could not run, a later failure is
   `unknown_provenance`. It may fail completion, but automatic repair does not
   guess that the task caused it.
7. Repair stops before the numeric limit when two consecutive attempts leave
   the same failure fingerprint, repeat an equivalent patch, produce no net
   task diff, or enter a detected fingerprint/diff oscillation.
8. A repair patch is checked against #98's protected paths. Touching a path
   outside the task's existing changed paths and normalized finding locations
   is a visible repair-scope expansion on the patch card. Approval of the patch
   explicitly approves that expansion once; it is not inferred from plan or
   check approval.
9. The verification report separates introduced regressions, pre-existing
   failures, environmental failures and stalled repairs. None is collapsed
   into a generic `failed` sentence.
10. Fingerprints, classifications and stop reasons survive reload and worker
    restart without storing unredacted output in a new location.

## 3. Non-goals

- Making every test hermetic or deciding whether a flaky test is logically
  related to a patch. An unstable fingerprint is reported as unknown unless a
  later specification adds repeat-based flake classification.
- Running a whole quality gate before every patch. Only checks selected for the
  proposed paths are baselined.
- Automatically reverting a patch that introduces a failure. The user reviewed
  the patch; repair or rollback remains an explicit action.
- Replacing the absolute repair limit. It remains the final bound after the
  earlier stagnation stops.

## 4. Design

Extend each task-local `CheckId` record with an optional baseline result and a
history of redacted fingerprints. Baseline selection happens after patch
approval but before apply, using the proposed paths while the base hashes still
name the bytes that were reviewed. If a check changes a tracked source file,
the ordinary patch hash check makes the approved patch stale and requires a new
preview.

The repair controller compares the latest result to the baseline and previous
repair results. It maintains a bounded history of `(failure_fingerprint,
patch_digest, task_diff_digest)`. Two identical consecutive states or an
alternating state within that window settles `repair_stalled`; it does not
spend the remaining attempts.

## 5. Acceptance

- A check failing identically before and after a patch records `pre_existing`
  and starts no repair turn.
- A passing baseline followed by a failure starts repair with only the new
  findings.
- Missing `cargo`, exit-by-policy and a command spawn error never start repair.
- A repeated ineffective repair stops after the second identical result even
  when the configured limit is sixteen.
- An A→B→A failure/diff oscillation stops with `repair_stalled`.
- A declined baseline makes the later failure unknown, not task-caused.
- A repair proposal for a protected path is refused by #98; a new unprotected
  path is clearly marked as a one-shot scope expansion.
