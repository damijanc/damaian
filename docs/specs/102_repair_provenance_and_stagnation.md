# Feature Spec: Repair Only New Failures and Stop When Repair Stalls

Status: In progress (started 2026-11-01)
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

## 6. Implementation Plan

**Started:** 2026-11-01

| Task | State | Durable result |
|---|---|---|
| 1 · Durable evidence and provenance model | Done 2026-11-01 | Added the typed engine model in `workspace-engine/src/verification.rs`: `CheckTermination` (exited, signalled, timed out, cancelled, and the four requirement-5 environment cases), `FindingEvidence::from_finding` (redacted summary/source/severity/code/location only; transient ids, timestamps, origin refs and details excluded), `FailureFingerprint::compute` (canonical versioned JSON of the normalised `CheckId`, sorted and de-duplicated evidence and termination, hashed with SHA-256), the six `FailureProvenance` classes, `CheckBaseline` (`passed`, `failed`, `declined`, `unavailable`), `CheckProvenanceRecord`, and `RepairStateEntry` with `MAX_REPAIR_STATE_ENTRIES = 8`. Added `verification_baseline`, `verification_provenance` and `verification_repair_state` events; `VerificationState` now carries `baselines`, `provenance` and the bounded `repair_history`, and `SessionStore` writes/reads the three new kinds. Historical events are unchanged, so replay leaves all three collections empty. Focused tests in `tests/repair_provenance.rs` cover order/duplicate independence, material differences, termination and check identity, transient-field and absolute-path normalisation, environment termination wire shape, serde round trips, the bounded window, historical-event compatibility and replay through a fresh `SessionStore`. Local checks passed: `cargo fmt --all`, scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`, `cargo nextest run -p workspace-engine --test repair_provenance` (13 passed), the existing `verification_loop` suite (70 passed), `npm run specs:check -- --strict` and `git diff --check`; workspace-wide clippy/tests remain for CI. Task 2 discoveries: `SessionStore::append_verification_event`'s event-kind match and `read_verification_events`'s allow-list must both learn every new verification kind or a durable event replays as unknown; `CheckOutcome::Ran` still has no termination, so baseline construction should map `CommandExecution.termination`, spawn failures and policy refusals into `CheckTermination` rather than extending the old variant; `propose_checks` can fall back to an absolute working directory when `relative_location` fails, which the fingerprint now reduces to its final component; and `session.rs`'s `continuation_edges` links verification tasks only through `verification_due`/`verification_started`/`verification_finished`, so Task 2 must choose deliberately which task id records each baseline. No baseline command ran and no live repair or patch-apply behaviour changed. |
| 2 · Pre-apply baseline decisions | Done 2026-11-01 | Added `workspace-engine/src/baseline.rs`: `BaselineCoordinator` selects checks from the reviewed patch paths, reuses the patch task's durable baselines through its continuation chain, records safe runs and policy/spawn/missing-executable unavailability, and returns an engine-owned `BaselineBatch` for checks that need separate approval. `EditOrchestrator::apply_stored_patch_with_baseline` runs the gate after the protected-path/mode refusals and before hooks, the action marker, the checkpoint and every write; `PatchApplyOutcome::BaselineRequired` is returned instead of an error. A truly sessionless patch now bypasses baseline persistence even when it carries a task id, so legacy/directly-created patches keep their old apply behavior. Decline, approve, partial-selection, later-check, reload, worker-restart, stale-base-hash and malformed-retry flows are covered by `tests/repair_baseline.rs`. The desktop shell returns the batch from `/api/apply-patch` and resumes it through `/api/resume-baseline`; its baseline card renders the engine's labels plus the ordinary per-check **Always allow** control, and the app retries the exact hunk/secret snapshot the pause interrupted. *Always* is validated inside the engine against this exact pending batch: unknown, duplicate, unapproved, ineligible and no-longer-available proposals are refused before any allowlist grant is written or baseline command runs. CLI and the deterministic eval harness explicitly decline offered baselines so their headless applies continue. Local checks passed: `cargo fmt --all -- --check`; scoped `cargo clippy -p workspace-engine -p desktop-shell -p damaian-cli -p eval-harness --all-targets --locked -- -D warnings`; `cargo nextest run -p workspace-engine --test repair_baseline --test repair_provenance --test verification_loop` (101 passed); focused desktop-shell baseline/app_js tests (3 passed); `cargo nextest run -p eval-harness -p damaian-cli` (117 passed, 2 skipped); `node --check crates/desktop-shell/static/app.js`; `npm run lint:web` (one pre-existing CSS specificity warning); `npm run specs:check -- --strict`; `git diff --check`. Workspace-wide clippy/tests remain for CI. Discoveries: a non-empty `task_id` does not imply a session, so the empty `session_id` is the authoritative sessionless test; the pending batch is bound to the canonical repository root and validated before resume; and baseline `*Always*` eligibility must be recomputed from the stored proposal because repository config can change between card render and response. Post-patch fingerprint/classification and repair-target behavior remain Task 3 and are unchanged here. |
| 3 · Provenance classification and repair targets | Done 2026-11-01 | Added `workspace-engine/src/provenance.rs`: `PostPatchResult` fingerprints through Task 1's `FailureFingerprint`, and `classify` keeps the six rules in the engine. A passing baseline makes the first failure `new_regression` with all its evidence; a matching failed baseline is `pre_existing` on the first result and `unchanged_failure` on a repeated one; a failed baseline with evidence the baseline did not have is `worsened` with only that delta as the repair target; a subset with no new evidence is `unchanged_failure`; declined, unavailable and absent baselines are `unknown_provenance`; a live spawn failure, missing executable, policy refusal or unavailable infrastructure is `environment_failure` before any baseline lookup. `CheckOutcome` now stores the live `termination` and has an `Environment` variant, so `run_verification_check` records a policy refusal or a typed command-spawn failure instead of aborting the turn. Only `CommandRunner`'s actual process-spawn error is classified as `SpawnFailed`; every unrelated error (proposal or execution store, audit, cancellation, and any other I/O) propagates rather than being disguised as an environment fact. Old `Ran` events without a recorded termination replay as an ordinary `Exited` (or `Signalled` when no code was stored), so a historical exit 127 remains a failure of unknown provenance rather than being newly inferred as a missing executable. `finish_verification` records durable `verification_provenance` events, `SessionStore::task_verification_provenance` scopes the previous-result lookup to the task chain, and repair starts only when `repair_targets` is non-empty. `VerificationReport` gained engine-owned `provenance` partitions, `repairTargets` and flattened `actions`; `summary`/`render` separate introduced regressions, worsened, pre-existing, unchanged, environment and unknown failures, and a historical failure without a baseline replays as unknown with no target. `run_repair_turn` now renders only normalized introduced evidence from `repair_targets`, never the full failed report or raw output. The desktop report card prints the engine's group labels and summaries, `ProvenanceCheck.label`, each check's reason, skipped `reasonLabel`, actions and engine-formatted `evidenceLines`; it contains no classification, evidence-formatting or repair-eligibility rule. Review fixes: `ProvenanceCheck` gained serialized `evidenceLines` populated by the canonical redacted evidence formatter, and the card renders those strings verbatim instead of rebuilding them from `FindingEvidence`; `ClientError::CommandSpawnFailed` is produced only by `CommandRunner`'s actual process spawn; `run_verification_check` and the baseline runner turn only it or policy/approval into an environment/unavailable fact, and every unrelated error (proposal or execution store, audit, cancellation, other I/O) propagates. Historical `Ran` events without a recorded termination still derive only an ordinary exit (or signalled), so an old exit 127 remains `unknown_provenance` rather than being inferred as a missing executable. Tests: `tests/repair_targets.rs` (11) covers every class, the delta, environment precedence, unchanged/pre-existing distinction, partition/action/summary rendering and historical fallback; `tests/verification_loop.rs` (78) adds declined baseline, matching failed baseline, spawn failure, missing executable, policy refusal, worker-unknown, reload, passing-baseline repair and an execution-store failure that propagates instead of becoming provenance; the engine lib suite plus `tests/repair_provenance.rs` (705 passed, 5 skipped) covers the typed spawn error and the environment-classification helper; desktop-shell (264 passed, 18 skipped) proves provenance/targets/actions/evidence lines survive JSON and the card renders engine data only. Local checks passed: `cargo fmt --all -- --check`; scoped `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`; `cargo nextest run -p workspace-engine --test verification_loop --test repair_targets --test repair_baseline` (107 passed) and `--lib --test repair_provenance` (705 passed, 5 skipped); `cargo nextest run -p desktop-shell` (264 passed, 18 skipped); `node --check crates/desktop-shell/static/app.js`; `npm run lint:web` (one pre-existing CSS specificity warning); `npm run specs:check -- --strict`; `git diff --check`. Workspace-wide clippy/tests remain for CI. Discoveries for Task 4: Task 4's bounded `repair_history` can now compare `FailureFingerprint`, patch digest and task-diff digest without re-deriving provenance; a repeated failure against a *passing* baseline remains `new_regression` in Task 3 (Task 4 owns the two-identical-attempt stop), while a repeated result against a failed baseline is already `unchanged_failure`; `CheckOutcome::Ran` events carry the exact termination from this task forward, and `CheckOutcome::termination()` is the single fallback for old logs, deliberately inferring no environment reason from a historical exit code; the repair request is `VerificationReport::repair_request()` rather than the old finding-id `RepairRequest`; and the existing `verification_loop` fixtures now mark baselines passing (or directly declined) so their repair expectations remain about repair, not baseline acquisition. |
| 4 · Stagnation and oscillation stops | Not started | Persist bounded fingerprint/patch/task-diff history and stop before the numeric limit on identical states, equivalent patches, no net task diff or an A→B→A oscillation. |
| 5 · Repair scope and thin presentation | Not started | Compute repair-scope expansion in the engine from existing task paths and normalized finding locations, preserve #98 refusals, require one-shot approval for an unprotected expansion, and have desktop/CLI render the engine-owned report and controls without business logic. |
| 6 · Integration, documentation and close-out | Not started | Exercise every requirement and acceptance case across ordinary and worker sessions, add deterministic evaluation coverage, update user/security/troubleshooting and dependency records, run scoped checks, and mark the spec Done. |

### Task 1: durable evidence and provenance model

Task 1 creates the application boundary that later slices consume without yet
moving a command or starting a repair differently:

- Add typed, serializable engine data for check termination, a stable failure
  fingerprint, the six provenance classifications from requirement 4, an
  optional task-local baseline, and a bounded repair-state entry containing a
  failure fingerprint, patch digest and task-diff digest.
- Derive a failure fingerprint from a canonical representation of `CheckId`,
  normalized redacted findings and termination kind. Do not hash raw command
  output, secret-bearing text, timestamps, marker ids or machine-local paths.
  Equivalent normalized evidence must produce the same value regardless of
  ordering; a materially different finding or termination must not.
- Extend verification events and `VerificationState` replay so a baseline,
  post-patch provenance and repair history can survive reload. New fields and
  variants must remain compatible with every historical event written before
  this spec; replay must not invent a baseline or classification for them.
- Keep construction and classification rules in `workspace-engine`. Shell,
  desktop, CLI and worker code receive serialized facts later and must not
  reproduce the fingerprint or provenance rules.
- Add focused unit/integration tests for canonical fingerprint stability,
  environment termination representation, bounded history, serde round trips,
  old-event compatibility and replay after a fresh `SessionStore`.

Task 1 does not run baseline commands, alter patch approval/apply ordering,
classify a live post-patch result, change a repair prompt, detect stagnation or
add UI. Record the actual checks and any discovery needed by Task 2 in the
progress row, then stop.

### Task 2: pre-apply baseline decisions

Task 2 moves check selection early enough to establish provenance safely:

- Once a patch is approved, select checks from the approved paths while the
  patch base hashes still name the reviewed bytes. Offer baseline commands
  through their ordinary policy and approval path before the first patch that
  selects each check is applied; patch approval grants none of those commands.
- Record pass, failure, decline, policy refusal, spawn failure and unavailable
  infrastructure as task-local baseline evidence. Reuse a current baseline for
  the same check; baseline a check introduced by a later patch before that
  patch applies.
- If an approved baseline command changes a tracked file, let the existing
  patch hash protection make the reviewed patch stale and require a fresh
  preview. Never apply against bytes different from the preview.
- Cover ordinary, worker, partial patch and reload flows. Stop after updating
  the progress row; post-patch repair behavior remains unchanged until Task 3.

### Task 3: provenance classification and repair targets

Task 3 applies the model to live verification results:

- Fingerprint each post-patch result and classify it against the task-local
  baseline as `new_regression`, `pre_existing`, `worsened`,
  `unchanged_failure`, `environment_failure` or `unknown_provenance`.
- Treat missing executables, spawn failures, policy refusal and unavailable
  infrastructure as environment failures. Declined or unusable baselines
  produce unknown provenance. Neither classification may start code repair.
- Start automatic repair only for a new regression or the introduced portion
  of a worsened result, and carry only that normalized evidence into the repair
  request. Preserve the full redacted report for the user.
- Extend the verification report with separate typed partitions and actions;
  do not collapse them into one failed sentence. Stop after focused engine and
  shell transport tests and a truthful progress-row update.

### Task 4: stagnation and oscillation stops

Task 4 adds progress detection ahead of the existing absolute limit:

- Record a bounded sequence of failure fingerprint, equivalent-patch digest
  and net task-diff digest for repair attempts. Digests must be canonical,
  content-safe and durable across reload and worker restart.
- Stop with a typed `repair_stalled` reason when two consecutive states are
  identical, a proposed patch is equivalent to the previous repair, the task
  diff does not change, or a fingerprint/diff state oscillates within the
  bounded window. Pin the A→B→A case.
- A stall consumes no later automatic attempts and does not replace the hard
  numeric limit. Recovery must render the same stop and must not silently
  restart repair. Record the checks and discoveries, then stop.

### Task 5: repair scope and thin presentation

Task 5 makes scope expansion an engine-owned review fact:

- Compute the repair's existing scope from task-changed paths plus normalized
  finding locations. A protected path remains an unconditional #98 refusal.
- Mark any other new path as a typed one-shot scope expansion on the patch
  review. Approval covers that patch once; plan, check and earlier patch
  approvals never imply it.
- Return engine-owned labels, reason groups, actions and expansion facts to the
  desktop and CLI. JavaScript and adapters render them without classifying
  outcomes or deciding whether repair may proceed.
- Cover ordinary, worker, reload and stale-card behavior, update the progress
  row, and stop.

### Task 6: integration, documentation and close-out

Task 6 audits all ten requirements and seven acceptance criteria against the
integrated implementation. It adds deterministic scenarios for pre-existing,
environment, declined-baseline, ineffective-repair and oscillation paths;
updates `USER_GUIDE.md`, `TROUBLESHOOTING.md`, `SECURITY.md`, dependency status,
`README.md` and `CHANGELOG.md`; runs the repository-prescribed fast checks,
scoped Clippy/tests and deterministic evaluation; records the evidence; and
marks the spec Done. Workspace-wide Clippy and tests remain for CI.
