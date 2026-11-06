# Feature Spec: Repair Only New Failures and Stop When Repair Stalls

Status: Done (2026-11-01)
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

1. Before the first approved patch a task applies, Damaian offers to run the
   checks the approved plan declares required, together with the completion
   floor's detected checks for the roots that patch touches, against the
   untouched task-start tree. That is their task-local baseline, one run per
   required check per task. A check that is neither plan-required nor in that
   floor is baselined before the patch that first selects it, as before. A
   required check that becomes required only after the task has already changed
   files — a plan revision, or a floor check for a root first touched by a later
   patch — has no task-start baseline and is never given a post-patch baseline.
2. Baseline commands keep their ordinary approval requirements. Patch approval
   does not approve a baseline check, and declining the baseline does not
   decline the patch.
3. Every check result has a stable failure fingerprint derived from check
   identity, normalized findings and termination kind. A finding's comparison
   key is its source, severity, code, normalized path and normalized summary;
   line and column are excluded, so an edit above a finding does not create a
   new identity. The location stays in the evidence shown to the model and the
   user. The fingerprint has set semantics, but provenance compares findings as
   a multiset, so a new identical occurrence is not hidden by an existing one.
   Raw output and secrets are not part of the fingerprint.
4. Post-patch comparison classifies a result as `new_regression`,
   `pre_existing`, `worsened`, `unchanged_failure`, `environment_failure`,
   `uncertain` or `unknown_provenance`. A required check with no task-start
   baseline is `uncertain`. Only `new_regression` and the new portion of
   `worsened` enter automatic repair. A new warning or informational finding on
   a check that already failed is reported but does not make it `worsened` and
   never becomes a repair target. A repair request carries each introduced
   finding's normalized summary and location plus its already-redacted
   details — assertion left/right values, rustc's expected/found lines, a
   parser's bounded output tail — under an explicit per-finding and total
   detail budget. When a budget cuts details, the request says so. Pre-existing,
   environment and unknown findings contribute no repair evidence.
5. Spawn failures, missing executables, policy refusals and unavailable test
   infrastructure are environment failures. They are reported with an action
   for the user; they are never given to a code-repair turn.
6. If a baseline was declined or could not run, a later failure is
   `unknown_provenance`. If a required check had no task-start baseline because
   it became required after the task had already changed files, a later failure
   is `uncertain`. Both may fail completion, but neither starts automatic
   repair. `uncertain` is named in the report, and a worker asks the user whether
   one repair turn should run; the answer is not an approval of any patch or
   command.
7. Repair stops before the numeric limit when two consecutive attempts leave
   the same failure fingerprint, repeat an equivalent patch, produce no net
   task diff, or enter a detected fingerprint/diff oscillation.
8. A repair patch is checked against #98's protected paths. Touching a path
   outside the task's existing changed paths and normalized finding locations
   is a visible repair-scope expansion on the patch card. Approval of the patch
   explicitly approves that expansion once; it is not inferred from plan or
   check approval.
9. The verification report separates introduced regressions, pre-existing
   failures, environmental failures, uncertain failures with no task-start
   baseline and stalled repairs. None is collapsed into a generic `failed`
   sentence.
10. Fingerprints, classifications and stop reasons survive reload and worker
    restart without storing unredacted output in a new location. Redacted
    finding details are stored on the durable evidence so a reload renders the
    same repair request; they are excluded from identity, so carrying them
    changes no fingerprint or classification.

## 3. Non-goals

- Making every test hermetic or deciding whether a flaky test is logically
  related to a patch. An unstable fingerprint is reported as unknown unless a
  later specification adds repeat-based flake classification.
- Running a whole quality gate before every patch. Per-change baseline
  selection stays as it was; only the plan's required checks and the completion
  floor for the roots the first patch touches run at task start.
- Rebuilding the task-start tree in a temporary worktree to take a late
  baseline. A required check that became required late is `uncertain` instead.
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

Required checks are selected from the approved plan and from the completion
floor (spec 101/105 as amended by review fix R15), as well as from the proposed
paths. On the first patch a task applies, before any write, every required check
without a baseline runs against the untouched tree through the existing batch
and command-policy path. A required check that is also selected for a later
patch reuses that task-start baseline rather than taking a new pre-patch one. A
required check that first appears after the task has already changed files (a
plan revision or a floor check for a root first touched by a later patch) is
recorded as `CheckBaseline::Uncertain` instead of being run: there is no
task-start tree to compare against. That marker is durable and is what makes the
later failure `uncertain`; the per-change baseline rule is unchanged for checks
outside the required set. Classification also treats a required check with no
recorded baseline as `uncertain`, which covers a revision after the last patch
and a historical log whose first patch predates this rule. A declined or
unavailable baseline is recorded, so it still classifies as unknown.

Classification precedence is: a live environment failure first; then an
explicit `Uncertain` baseline; then declined, unavailable or absent baselines
as `unknown_provenance`; then a passed or failed baseline. `uncertain` is
therefore never silently `pre_existing`, and only the user's answer to the
worker's repair question can turn its evidence into one ordinary repair turn.
The answer is a question answer, not a patch or command approval; the repair
patch it leads to still needs its own approval.

The repair controller compares the latest result to the baseline and previous
repair results. It maintains a bounded history of `(failure_fingerprint,
patch_digest, task_diff_digest)`. Two identical consecutive states or an
alternating state within that window settles `repair_stalled`; it does not
spend the remaining attempts.

The finding comparison key excludes `start_line` and `start_column`; those
fields stay on the evidence rendered to the model and in the report, but they
are not identity. Provenance compares findings as a multiset: one pre-existing
occurrence plus one newly introduced identical occurrence is exactly one new
finding, while two occurrences that both moved without growing the count are
unchanged. Introduced repair evidence is restricted to errors, so a new warning
on an already failing check is visible without marking the result `worsened`.
Failure fingerprints are versioned: version 2 hashes the location-free
comparison key. A stored version 1 baseline or provenance record remains
loadable and still classifies from its durable findings; a version 1
repair-history entry remains loadable but does not compare equal to a version 2
value, so the first attempt after an upgrade starts a fresh stagnation window
and the numeric limit still bounds repair.

The repair request is built only from `RepairTarget::evidence`, so the
pre-existing, environment and unknown partitions cannot reach it. Each finding
contributes its severity, code, summary and location line, then its redacted
`details()` when present. `MAX_REPAIR_FINDING_DETAILS_BYTES` (1,200) bounds one
finding and `MAX_REPAIR_TOTAL_DETAILS_BYTES` (4,000) bounds the whole request;
a cut emits `REPAIR_FINDING_DETAILS_TRUNCATED` or
`REPAIR_TOTAL_DETAILS_TRUNCATED` so the model never reads a partial compiler
block as complete. Every finding's summary line and the "Repair only the
failures this patch introduced" scope line are never dropped for budget.
`FindingEvidence` carries `details` durably but excludes it from
`canonical_key`, `location_key`, equality and `FailureFingerprint`; a longer
repair display therefore cannot change provenance or a stagnation stop. A
provenance or baseline record written before details were carried still loads
and renders its summary and location, with no details.

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

**Started:** 2026-11-01 — **Done:** 2026-11-01

| Task | State | Durable result |
|---|---|---|
| 1 · Durable evidence and provenance model | Done 2026-11-01 | Added the typed engine model in `workspace-engine/src/verification.rs`: `CheckTermination` (exited, signalled, timed out, cancelled, and the four requirement-5 environment cases), `FindingEvidence::from_finding` (redacted summary/source/severity/code/location only; transient ids, timestamps, origin refs and details excluded), `FailureFingerprint::compute` (canonical versioned JSON of the normalised `CheckId`, sorted and de-duplicated evidence and termination, hashed with SHA-256), the six `FailureProvenance` classes, `CheckBaseline` (`passed`, `failed`, `declined`, `unavailable`), `CheckProvenanceRecord`, and `RepairStateEntry` with `MAX_REPAIR_STATE_ENTRIES = 8`. Added `verification_baseline`, `verification_provenance` and `verification_repair_state` events; `VerificationState` now carries `baselines`, `provenance` and the bounded `repair_history`, and `SessionStore` writes/reads the three new kinds. Historical events are unchanged, so replay leaves all three collections empty. Focused tests in `tests/repair_provenance.rs` cover order/duplicate independence, material differences, termination and check identity, transient-field and absolute-path normalisation, environment termination wire shape, serde round trips, the bounded window, historical-event compatibility and replay through a fresh `SessionStore`. Local checks passed: `cargo fmt --all`, scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`, `cargo nextest run -p workspace-engine --test repair_provenance` (13 passed), the existing `verification_loop` suite (70 passed), `npm run specs:check -- --strict` and `git diff --check`; workspace-wide clippy/tests remain for CI. Task 2 discoveries: `SessionStore::append_verification_event`'s event-kind match and `read_verification_events`'s allow-list must both learn every new verification kind or a durable event replays as unknown; `CheckOutcome::Ran` still has no termination, so baseline construction should map `CommandExecution.termination`, spawn failures and policy refusals into `CheckTermination` rather than extending the old variant; `propose_checks` can fall back to an absolute working directory when `relative_location` fails, which the fingerprint now reduces to its final component; and `session.rs`'s `continuation_edges` links verification tasks only through `verification_due`/`verification_started`/`verification_finished`, so Task 2 must choose deliberately which task id records each baseline. No baseline command ran and no live repair or patch-apply behaviour changed. |
| 2 · Pre-apply baseline decisions | Done 2026-11-01 | Added `workspace-engine/src/baseline.rs`: `BaselineCoordinator` selects checks from the reviewed patch paths, reuses the patch task's durable baselines through its continuation chain, records safe runs and policy/spawn/missing-executable unavailability, and returns an engine-owned `BaselineBatch` for checks that need separate approval. `EditOrchestrator::apply_stored_patch_with_baseline` runs the gate after the protected-path/mode refusals and before hooks, the action marker, the checkpoint and every write; `PatchApplyOutcome::BaselineRequired` is returned instead of an error. A truly sessionless patch now bypasses baseline persistence even when it carries a task id, so legacy/directly-created patches keep their old apply behavior. Decline, approve, partial-selection, later-check, reload, worker-restart, stale-base-hash and malformed-retry flows are covered by `tests/repair_baseline.rs`. The desktop shell returns the batch from `/api/apply-patch` and resumes it through `/api/resume-baseline`; its baseline card renders the engine's labels plus the ordinary per-check **Always allow** control, and the app retries the exact hunk/secret snapshot the pause interrupted. *Always* is validated inside the engine against this exact pending batch: unknown, duplicate, unapproved, ineligible and no-longer-available proposals are refused before any allowlist grant is written or baseline command runs. The CLI may explicitly decline offered baselines — at a terminal it confirms each check itself, and a non-terminal stdin declines every check — so a headless CLI apply continues either way. The deterministic eval harness does not blanket-decline: it answers each offered baseline from the scenario's own `check_decisions`, approving a baseline whose command the scenario approves and declining every unapproved or unlisted check, so a scenario with approved checks records a real baseline instead of `declined`/`unknown_provenance`. Local checks passed: `cargo fmt --all -- --check`; scoped `cargo clippy -p workspace-engine -p desktop-shell -p damaian-cli -p eval-harness --all-targets --locked -- -D warnings`; `cargo nextest run -p workspace-engine --test repair_baseline --test repair_provenance --test verification_loop` (101 passed); focused desktop-shell baseline/app_js tests (3 passed); `cargo nextest run -p eval-harness -p damaian-cli` (117 passed, 2 skipped); `node --check crates/desktop-shell/static/app.js`; `npm run lint:web` (one pre-existing CSS specificity warning); `npm run specs:check -- --strict`; `git diff --check`. Workspace-wide clippy/tests remain for CI. Discoveries: a non-empty `task_id` does not imply a session, so the empty `session_id` is the authoritative sessionless test; the pending batch is bound to the canonical repository root and validated before resume; and baseline `*Always*` eligibility must be recomputed from the stored proposal because repository config can change between card render and response. Post-patch fingerprint/classification and repair-target behavior remain Task 3 and are unchanged here. |
| 3 · Provenance classification and repair targets | Done 2026-11-01 | Added `workspace-engine/src/provenance.rs`: `PostPatchResult` fingerprints through Task 1's `FailureFingerprint`, and `classify` keeps the six rules in the engine. A passing baseline makes the first failure `new_regression` with all its evidence; a matching failed baseline is `pre_existing` on the first result and `unchanged_failure` on a repeated one; a failed baseline with evidence the baseline did not have is `worsened` with only that delta as the repair target; a subset with no new evidence is `unchanged_failure`; declined, unavailable and absent baselines are `unknown_provenance`; a live spawn failure, missing executable, policy refusal or unavailable infrastructure is `environment_failure` before any baseline lookup. `CheckOutcome` now stores the live `termination` and has an `Environment` variant, so `run_verification_check` records a policy refusal or a typed command-spawn failure instead of aborting the turn. Only `CommandRunner`'s actual process-spawn error is classified as `SpawnFailed`; every unrelated error (proposal or execution store, audit, cancellation, and any other I/O) propagates rather than being disguised as an environment fact. Old `Ran` events without a recorded termination replay as an ordinary `Exited` (or `Signalled` when no code was stored), so a historical exit 127 remains a failure of unknown provenance rather than being newly inferred as a missing executable. `finish_verification` records durable `verification_provenance` events, `SessionStore::task_verification_provenance` scopes the previous-result lookup to the task chain, and repair starts only when `repair_targets` is non-empty. `VerificationReport` gained engine-owned `provenance` partitions, `repairTargets` and flattened `actions`; `summary`/`render` separate introduced regressions, worsened, pre-existing, unchanged, environment and unknown failures, and a historical failure without a baseline replays as unknown with no target. `run_repair_turn` now renders only normalized introduced evidence from `repair_targets`, never the full failed report or raw output. The desktop report card prints the engine's group labels and summaries, `ProvenanceCheck.label`, each check's reason, skipped `reasonLabel`, actions and engine-formatted `evidenceLines`; it contains no classification, evidence-formatting or repair-eligibility rule. Review fixes: `ProvenanceCheck` gained serialized `evidenceLines` populated by the canonical redacted evidence formatter, and the card renders those strings verbatim instead of rebuilding them from `FindingEvidence`; `ClientError::CommandSpawnFailed` is produced only by `CommandRunner`'s actual process spawn; `run_verification_check` and the baseline runner turn only it or policy/approval into an environment/unavailable fact, and every unrelated error (proposal or execution store, audit, cancellation, other I/O) propagates. Historical `Ran` events without a recorded termination still derive only an ordinary exit (or signalled), so an old exit 127 remains `unknown_provenance` rather than being inferred as a missing executable. Tests: `tests/repair_targets.rs` (11) covers every class, the delta, environment precedence, unchanged/pre-existing distinction, partition/action/summary rendering and historical fallback; `tests/verification_loop.rs` (78) adds declined baseline, matching failed baseline, spawn failure, missing executable, policy refusal, worker-unknown, reload, passing-baseline repair and an execution-store failure that propagates instead of becoming provenance; the engine lib suite plus `tests/repair_provenance.rs` (705 passed, 5 skipped) covers the typed spawn error and the environment-classification helper; desktop-shell (264 passed, 18 skipped) proves provenance/targets/actions/evidence lines survive JSON and the card renders engine data only. Local checks passed: `cargo fmt --all -- --check`; scoped `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`; `cargo nextest run -p workspace-engine --test verification_loop --test repair_targets --test repair_baseline` (107 passed) and `--lib --test repair_provenance` (705 passed, 5 skipped); `cargo nextest run -p desktop-shell` (264 passed, 18 skipped); `node --check crates/desktop-shell/static/app.js`; `npm run lint:web` (one pre-existing CSS specificity warning); `npm run specs:check -- --strict`; `git diff --check`. Workspace-wide clippy/tests remain for CI. Discoveries for Task 4: Task 4's bounded `repair_history` can now compare `FailureFingerprint`, patch digest and task-diff digest without re-deriving provenance; a repeated failure against a *passing* baseline remains `new_regression` in Task 3 (Task 4 owns the two-identical-attempt stop), while a repeated result against a failed baseline is already `unchanged_failure`; `CheckOutcome::Ran` events carry the exact termination from this task forward, and `CheckOutcome::termination()` is the single fallback for old logs, deliberately inferring no environment reason from a historical exit code; the repair request is `VerificationReport::repair_request()` rather than the old finding-id `RepairRequest`; and the existing `verification_loop` fixtures now mark baselines passing (or directly declined) so their repair expectations remain about repair, not baseline acquisition. CI caught one integration miss on 2026-11-01: the deterministic harness still declined every offered baseline from Task 2, so `verified_repair` and `repair_limit_exhausted` became unknown provenance and started no repair; the harness now runs a baseline whose check the scenario itself approves and leaves every other baseline declined, pinned by `runner::tests::a_baseline_follows_the_scenarios_own_check_decision`. |
| 4 · Stagnation and oscillation stops | Done 2026-11-01 | Added `workspace-engine/src/stagnation.rs`: canonical `proposed_patch_digest` (sorted normalized paths, status and redacted content), content-safe `task_diff_digest`, bounded `RepairStateEntry` history, and `detect_repair_stall` with the fixed precedence `identical_state` → `repeated_failure` → `equivalent_patch` → `unchanged_task_diff` → `oscillation`. `run_repair_turn` and the resumed paused-repair path record one state before exposing a patch and append `verification_repair_stalled` instead when the state proves no progress; `finish_verification_stalled` rebuilds the report from the durable log, takes the failed settlement, and consumes no later automatic attempt. `SessionStore::task_verification_repair_history` is scoped to the task continuation chain and includes the terminal stalled entry; replay clears due/in-progress and the continuation edge. The report gained `repairStall` (engine label, explanation and one `ReviewStalledRepair` action) and a `RepairStalled` outcome; the reload payload carries the typed stop and never offers *Continue repair*. `createVerificationReportCard` prints the stall heading and explanation and prints its action once through `report.actions`; `renderRepairStallOffer` prints the same engine strings from session verification data without classifying. Review fixes: the stall summary no longer appends a second period to the explanation's own period, pinned by `a_stall_summary_does_not_double_the_explanation_period`; the live card no longer renders `repairStall.action` twice. Local checks passed: `cargo fmt --all -- --check`; scoped `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`; `cargo nextest run -p workspace-engine --test repair_stagnation --test verification_loop` (94 passed); focused desktop-shell stall/card tests (3 passed); `node --check crates/desktop-shell/static/app.js`; `npm run lint:web` (one pre-existing CSS specificity warning); `npm run specs:check -- --strict`; `git diff --check`. Workspace-wide clippy/tests remain for CI. Discoveries: a stall is terminal without a `Finished` event, so reload discovery and continuation edges must treat `verification_repair_stalled` as the finish; `repairStall.action` is also flattened into `report.actions`, so a card that renders both duplicates it; and the paused-repair resume path reaches patch proposals outside `run_repair_turn`, so it needs the same progress check. |
| 5 · Repair scope and thin presentation | Done 2026-11-01 | Added `workspace-engine/src/repair_scope.rs`: `RepairScopeReview` is the typed engine fact (`label`, `explanation`, `one_shot`, `existing_paths`, `expansion_paths`, `reason_groups`, `actions`) and `compute` normalizes all inputs with the fingerprint's repository-path rule, unions task-changed paths with the active report's full and introduced finding locations, and marks every other proposed path as `OutsideExistingScope` with one `ApproveScopeExpansionOnce` action. `for_patch` returns `None` when the repair stays in scope; `render` is the CLI's engine-owned text. `ProposedPatch.repair_scope` is persisted in stored-patch V4 (V1/V2/V3 still load with no review), and `AgentPatchProposal.repair_scope` carries it live. Review fix: the review is computed inside the `propose_patch` and `edit_file` proposal branches, after #98's protected-path refusal and before the patch's first `PatchStore` save, response, action finish and pending-approval recording. A verification turn reads the durable active verification state, so a direct repair and a resumed repair attach the same fact, while an ordinary turn leaves it `None`. `run_repair_turn` and `close_verification_resume` no longer attach or re-save after the turn returns: the first persisted patch is already complete, so a crash or save failure cannot recover a repair card without the scope expansion the approval is meant to cover. Spec 98 still refuses a protected path as a whole proposal (visible in the turn, no patch) and the apply-time recheck refuses a stale card whose path became protected, writing nothing. No grant is stored: each review is recomputed from paths, so plan, check, baseline and earlier patch approvals cannot widen a later patch. The shell's shared `patch_payload_json` carries `repairScope` for live turns, workers and reattached cards; `createPatchPreview` prints only the supplied label, explanation, groups, paths and actions; the CLI prints `scope.render()` for verify, continue-repair, show-patch and apply-patch and never derives a path or permission. Focused tests: `repair_scope::tests` 5 plus the chat source-order review-ordering test 1; `verification_loop` 87 including in-scope no-review, typed expansion, a finding path outside the task-changed paths with no expansion on the live or stored patch, protected refusal without a patch, worker/reload/reattach equality, and stale newly-protected apply refusal; `session_rewind` 26 including V1/V2/V3 compatibility and V4 scope round-trip; desktop-shell shared-payload, JS-thinness and reloaded-protected tests 3 (the thinness assertion now requires rendering on `repairScope` presence alone and forbids `oneShot` and reason/action/decision fields); CLI thinness test 1. Local checks passed: `cargo fmt --all -- --check`; scoped `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`; `cargo nextest run -p workspace-engine --test verification_loop` (87), `--test session_rewind` (26) and `--lib repair_scope` (6); the focused patch-card desktop test; `node --check`; `npm run lint:web` (one pre-existing CSS specificity warning); `npm run specs:check -- --strict`; `git diff --check`; earlier in the task `scripts/quality-gate.sh fmt node lint typos deny` also passed. Workspace-wide clippy/tests remain for CI. |
| 6 · Integration, documentation and close-out | Done 2026-11-01 | Added five deterministic eval scenarios through the real engine: `verification_pre_existing_failure`, `verification_environment_failure`, `verification_declined_baseline`, `verification_repair_stalled_repeated` and `verification_repair_oscillation`. The harness gained `decline_baseline`, durable-baseline assertions read from the session log, and report assertions for provenance groups, repair-target count, engine action kinds and the typed stall reason; `VerificationActionKind::as_str` names the engine's actions. All ten requirements and seven acceptance criteria are mapped to code and tests in §7. Updated `USER_GUIDE.md`, `TROUBLESHOOTING.md`, `SECURITY.md`, `README.md`, the spec index, specs 103/106 and `CHANGELOG.md`. Local checks passed: `cargo fmt --all -- --check`, scoped `cargo clippy -p workspace-engine -p eval-harness --all-targets --locked -- -D warnings`, `cargo nextest run -p workspace-engine --test repair_provenance --test repair_baseline --test repair_stagnation --test verification_loop` (131 passed), `cargo nextest run -p eval-harness` (105 passed, 2 skipped, one leaky test), `cargo run -p eval-harness -- run --tier deterministic` (51/51 passed), `scripts/quality-gate.sh fmt node lint typos deny`, `npm run specs:check -- --strict` and `git diff --check`. Workspace-wide clippy and tests remain for CI. |

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

## 7. Acceptance evidence

### 7.1 Requirements

| # | Requirement | Implementation | Evidence |
|---|---|---|---|
| 1 | Baseline every required check on the untouched tree before the first patch, and any other check before the patch that first selects it. A required check that becomes required later has no task-start baseline. | `workspace-engine/src/baseline.rs` (`BaselineCoordinator::prepare`, `required_checks`, `task_touched_paths`, `task_has_changed_files`) runs from `EditOrchestrator::apply_stored_patch_with_baseline` after the protected-path/mode refusals and before hooks, the action marker, the checkpoint and every write. The first patch a task applies merges the approved plan's required checks with the completion floor for every root the patch and earlier changes touch, plus the detected per-change checks; a later patch reuses task-start baselines and records `CheckBaseline::Uncertain` for a required check that has none because files already changed. | `plan_turn`: `a_required_check_is_baselined_at_task_start_and_a_later_regression_is_repaired`, `a_required_check_first_required_after_changes_is_uncertain_and_asks_before_repair`, `a_declined_required_baseline_is_recorded_declined_not_uncertain`, `floor_checks_are_baselined_at_task_start_and_a_later_root_is_uncertain`, `a_floor_check_for_a_root_first_touched_later_is_uncertain_not_pre_existing`. `repair_baseline`: `a_safe_check_runs_as_a_passing_baseline_before_the_patch_writes`, `a_check_selected_by_a_later_patch_is_baselined_before_it_applies`, `a_partial_selection_baselines_only_the_roots_it_selects`, `a_partial_hunk_apply_still_passes_the_baseline_gate`, `a_fresh_engine_reuses_the_durable_baseline_without_running_it_again`, `a_worker_session_baseline_replays_after_the_worker_restarts`. Eval: `verification_pre_existing_failure`, `verification_declined_baseline`, `verification_environment_failure`. |
| 2 | Baseline commands keep ordinary approval; patch approval does not approve them, and declining a baseline does not decline the patch. | `baseline.rs` pauses on a `BaselineBatch` and validates every resume decision before running or granting anything; `PatchApplyOutcome::BaselineRequired` is returned instead of an error, and the patch can then apply. | `repair_baseline`: `a_declined_baseline_does_not_decline_the_patch`, `an_approved_baseline_runs_and_the_patch_applies_afterwards`, `malformed_resume_decisions_have_no_side_effects`, `always_grants_must_belong_to_the_pending_batch_and_be_approved_and_eligible`. Eval: `verification_declined_baseline`. |
| 3 | A stable failure fingerprint from check identity, normalized findings and termination; no raw output or secrets. | `FailureFingerprint::compute` and `FindingEvidence::from_finding` in `verification.rs` canonicalise the `CheckId`, the location-free finding comparison key (source, severity, code, normalized path, normalized summary) and termination. Transient ids, timestamps, origin refs, machine-local absolute paths, raw output and finding location are excluded from identity. The redacted, bounded `details` are carried for the repair request but excluded from `canonical_key`, `location_key`, equality and the fingerprint. The format is version 2; version 1 values remain loadable. | `repair_provenance`: `fingerprint_is_order_and_duplicate_insensitive`, `fingerprint_changes_with_material_evidence_but_not_location`, `fingerprint_uses_code_and_severity_but_not_location`, `fingerprint_changes_with_termination_and_check_identity`, `fingerprint_ignores_transient_finding_fields`, `fingerprint_does_not_depend_on_an_absolute_checkout_path`, `fingerprint_normalises_absolute_paths_in_text_and_check_identity`. `repair_baseline`: `a_failed_baseline_records_redacted_normalized_findings_and_a_fingerprint`. |
| 4 | Classify as `new_regression`, `pre_existing`, `worsened`, `unchanged_failure`, `environment_failure`, `uncertain` or `unknown_provenance`; repair only new regressions and the new part of worsened, with the introduced finding's redacted details under named budgets. | `workspace-engine/src/provenance.rs` (`classify`, `same_findings`, `introduced_findings`, `repair_targets`, `render_repair_request`) and `ChatOrchestrator::finish_verification`: classification compares the durable findings as a multiset without line or column, an explicit `CheckBaseline::Uncertain` maps to `uncertain` before the unknown groups, repair starts only when `repair_targets` is non-empty, only errors become introduced evidence, and a repaired result carries only the introduced delta. `render_repair_request` prints each finding's summary/location and its redacted details, with `MAX_REPAIR_FINDING_DETAILS_BYTES` / `MAX_REPAIR_TOTAL_DETAILS_BYTES` and explicit truncation markers. | `repair_targets`: `a_pre_existing_error_that_shifts_lines_is_still_pre_existing`, `a_new_identical_error_in_a_different_file_is_introduced`, `a_second_different_error_in_the_same_file_is_introduced`, `a_new_warning_on_a_failed_baseline_is_not_worsened_or_a_repair_target`, `an_extra_identical_occurrence_beyond_the_baseline_is_introduced`, `shifted_duplicates_that_do_not_grow_the_count_are_not_introduced`, `a_baseline_written_with_a_v1_fingerprint_still_classifies_by_its_findings`, `a_repair_request_bounds_one_findings_details`, `a_repair_request_bounds_total_details_across_many_findings`. `verification_loop`: `a_matching_failed_baseline_is_pre_existing_and_starts_no_repair`, `a_pre_existing_error_that_shifts_lines_stays_pre_existing`, `a_declined_baseline_makes_the_failure_unknown_and_starts_no_repair`, `a_repair_that_breaks_a_previously_passing_check_is_a_regression`, `the_repair_request_carries_normalized_evidence_not_the_raw_log`, `the_repair_request_carries_a_rustc_type_error_expected_and_found`, `the_repair_request_carries_a_generic_checks_bounded_stderr_tail`, `provenance_and_repair_targets_survive_a_fresh_store`. `plan_turn`: `a_required_check_first_required_after_changes_is_uncertain_and_asks_before_repair`, `answering_the_uncertain_repair_question_with_repair_runs_one_repair_turn_that_still_needs_approval`, `answering_the_uncertain_repair_question_with_no_leaves_it_reported_and_unrepaired`, `a_per_change_check_keeps_its_pre_patch_baseline_and_pre_existing_classification`. Eval: `verification_pre_existing_failure`, `verification_declined_baseline`, `verification_repair_stalled_repeated`. |
| 5 | Spawn failure, missing executable, policy refusal and unavailable infrastructure are environment failures with a user action, never code repair. | `CheckTermination::is_environment_failure`, `environment_termination`, `CheckOutcome::Environment`; `build_partitions` gives each environment group one `FixEnvironment` action. Only the process-spawn error is `SpawnFailed`; other errors propagate. | `verification_loop`: `a_spawn_failure_is_an_environment_failure_and_starts_no_repair`, `a_missing_executable_is_an_environment_failure_and_starts_no_repair`, `a_policy_refused_check_is_reported_as_environment_and_starts_no_repair`, `an_execution_store_failure_propagates_instead_of_becoming_environment`. `repair_baseline`: `policy_refusal_is_an_unavailable_baseline_and_the_patch_still_applies`, `a_spawn_failure_and_a_missing_executable_are_unavailable_baselines`. Eval: `verification_environment_failure`. |
| 6 | A declined or unusable baseline makes a later failure unknown, not task-caused; a required check with no task-start baseline is `uncertain`. Both may fail completion but neither starts automatic repair; a worker asks about `uncertain` and the answer is never an approval. | `classify` maps `Declined` and `Unavailable` baselines to `UnknownProvenance` and `Uncertain` to `Uncertain`; `finish_verification` settles `Failed` with no repair targets for a non-worker, and a worker pauses on the engine's repair question. `resume_repair_decision` runs one ordinary repair turn for "Repair it" and otherwise settles `Failed`; both routes still require ordinary patch/command approval. | `repair_baseline`: `a_declined_baseline_does_not_decline_the_patch`, `policy_refusal_is_an_unavailable_baseline_and_the_patch_still_applies`. `verification_loop`: `a_declined_baseline_makes_the_failure_unknown_and_starts_no_repair`, `a_worker_session_does_not_repair_an_unknown_failure`. `plan_turn`: `a_declined_required_baseline_is_recorded_declined_not_uncertain`, `a_required_check_first_required_after_changes_is_uncertain_and_asks_before_repair`, `answering_the_uncertain_repair_question_with_repair_runs_one_repair_turn_that_still_needs_approval`, `answering_the_uncertain_repair_question_with_no_leaves_it_reported_and_unrepaired`. Eval: `verification_declined_baseline`. |
| 7 | Stop before the numeric limit on a repeated fingerprint, equivalent patch, unchanged task diff or oscillation. | `workspace-engine/src/stagnation.rs` (`proposed_patch_digest`, `task_diff_digest`, `repair_state_entry`, `detect_repair_stall`), `record_repair_state_or_stall` and `finish_verification_stalled`. The fixed precedence is `identical_state` → `repeated_failure` → `equivalent_patch` → `unchanged_task_diff` → `oscillation`. The attempt fingerprint uses the location-free comparison key, so an ineffective repair that only moves the failing line still repeats. | `repair_stagnation`: `each_stop_cause_is_detected_independently`, `reason_precedence_is_fixed`, `an_a_to_b_to_a_state_oscillates`, `the_first_progressing_attempt_never_stalls`, `repair_stall_is_one_terminal_event_that_replays_as_a_stop`. `verification_loop`: `a_repeated_ineffective_repair_stalls_before_the_numeric_limit`, `a_repair_that_only_moves_the_failing_line_stalls_as_repeated_failure`, `an_a_to_b_to_a_repair_oscillation_stalls`, `a_worker_stops_on_the_same_typed_repair_stall`. Eval: `verification_repair_stalled_repeated` (limit 16, stops at `repeated_failure`), `verification_repair_oscillation` (limit 16, stops at `oscillation`). |
| 8 | A repair patch is checked against #98's protected paths; any other new path is a visible one-shot scope expansion, approved only once and not inferred. | `workspace-engine/src/repair_scope.rs` computes `RepairScopeReview` from task-changed paths plus normalized finding locations. Spec 98 still refuses a protected path as a whole proposal, and apply rechecks a stale card. The review is persisted in stored-patch V4 and carried live; the shell and CLI print only its labels, groups, paths and actions. | `verification_loop`: `a_repair_inside_the_task_scope_has_no_scope_review`, `a_new_unprotected_repair_path_is_a_typed_one_shot_expansion`, `a_protected_repair_path_is_refused_without_a_patch`, `a_worker_and_a_reload_preserve_the_same_scope_review`, `a_stale_expansion_card_newly_protected_is_refused_at_apply`, `a_repair_may_touch_a_finding_path_outside_the_task_changed_paths`. |
| 9 | The report separates introduced, pre-existing, environmental, uncertain and stalled repairs; none collapses into one failed sentence. | `VerificationReport` carries `provenance`, `repair_targets`, flattened `actions` and `repair_stall`; `summary`/`render` name each group in engine-owned words, `uncertain` sits immediately before the catch-all unknown group, and a stall replaces the repair action with its own. The web UI renders the supplied facts and classifies nothing. | `verification_loop`: `the_summary_never_says_complete_when_anything_is_wrong`, `every_selected_check_is_in_exactly_one_section`, `provenance_and_repair_targets_survive_a_fresh_store`. `repair_stagnation`: `a_stall_has_engine_owned_presentation_and_no_continue_action`, `a_stall_summary_does_not_double_the_explanation_period`. `plan_turn`: `a_required_check_first_required_after_changes_is_uncertain_and_asks_before_repair`. Eval asserts `verification_provenances`, `verification_action_kinds` and `repair_stall_reason` on all five new scenarios. |
| 10 | Fingerprints, classifications and stop reasons survive reload and worker restart without storing unredacted output in a new location. | `SessionStore` writes/reads `verification_baseline`, `verification_provenance`, `verification_repair_state` and `verification_repair_stalled`; replay folds them into `VerificationState`. Historical logs replay with no inferred baseline or classification. Redacted, bounded finding details are part of the durable provenance evidence and are excluded from identity. | `repair_provenance`: `baseline_provenance_and_repair_state_round_trip`, `historical_verification_events_replay_without_inventing_provenance`, `a_log_without_the_uncertain_value_still_loads`, `new_verification_events_round_trip_through_a_fresh_store`, `repair_history_keeps_only_the_bounded_window`, `a_legacy_fingerprint_value_still_loads_from_a_baseline_event`; `repair_targets`: `a_baseline_written_with_a_v1_fingerprint_still_classifies_by_its_findings` (a version 1 baseline still compares from its durable findings). `repair_stagnation`: `repair_history_survives_a_fresh_store_and_repair_continuations`, `repair_stall_is_one_terminal_event_that_replays_as_a_stop`. `verification_loop`: `provenance_and_repair_targets_survive_a_fresh_store` (the reloaded request still carries the finding's details), `a_worker_session_does_not_repair_an_unknown_failure`, `a_worker_and_a_reload_preserve_the_same_scope_review`. Eval asserts a durable report and durable baselines. |

### 7.2 Acceptance criteria

| Criterion | Evidence |
|---|---|
| A check failing identically before and after a patch records `pre_existing` and starts no repair turn. | `verification_loop::a_matching_failed_baseline_is_pre_existing_and_starts_no_repair`, `a_pre_existing_error_that_shifts_lines_stays_pre_existing` (the real pre-patch baseline fails at line 4, the post-patch check at line 7, and still starts no repair); `repair_targets::a_pre_existing_error_that_shifts_lines_is_still_pre_existing`; eval `verification_pre_existing_failure` (failed baseline, `pre_existing`, zero repair targets, at most two model calls). |
| A passing baseline followed by a failure starts repair with only the new findings, whose redacted details reach the request under its budgets. | `verification_loop::a_failed_check_is_repaired_and_the_rerun_passes`, `the_repair_request_carries_normalized_evidence_not_the_raw_log` (assertion left/right, no raw-output secret), `the_repair_request_carries_a_rustc_type_error_expected_and_found`, `the_repair_request_carries_a_generic_checks_bounded_stderr_tail`; `repair_targets::a_repair_request_bounds_one_findings_details`, `a_repair_request_bounds_total_details_across_many_findings`; eval `verified_repair`, plus the new stalled scenarios' passing baselines. |
| Missing `cargo`, exit-by-policy and a command spawn error never start repair. | `verification_loop::a_missing_executable_is_an_environment_failure_and_starts_no_repair`, `a_policy_refused_check_is_reported_as_environment_and_starts_no_repair`, `a_spawn_failure_is_an_environment_failure_and_starts_no_repair`; eval `verification_environment_failure` (`fix_environment`, zero repair targets). |
| A repeated ineffective repair stops after the second identical result even when the limit is sixteen. | `verification_loop::a_repeated_ineffective_repair_stalls_before_the_numeric_limit`, `a_repair_that_only_moves_the_failing_line_stalls_as_repeated_failure` (each attempt moves the reported line, still `repeated_failure` at the second attempt with limit 16); eval `verification_repair_stalled_repeated` (`agent_tool_retry_limit=16`, `repair_stalled`, `repeated_failure`, four model calls). |
| An A→B→A failure/diff oscillation stops with `repair_stalled`. | `repair_stagnation::an_a_to_b_to_a_state_oscillates`, `verification_loop::an_a_to_b_to_a_repair_oscillation_stalls`; eval `verification_repair_oscillation` (limit 16, `repair_stalled`, `oscillation`, five model calls). |
| A declined baseline makes the later failure unknown, not task-caused. | `verification_loop::a_declined_baseline_makes_the_failure_unknown_and_starts_no_repair`; `plan_turn::a_declined_required_baseline_is_recorded_declined_not_uncertain`; eval `verification_declined_baseline` (durable baseline `declined`, `unknown_provenance`, `provide_baseline`, zero repair targets). |
| A required check with no task-start baseline is `uncertain`, reported, and repaired only after the user answers a question; a per-change check keeps its pre-patch baseline. | `plan_turn::a_required_check_is_baselined_at_task_start_and_a_later_regression_is_repaired`, `a_required_check_first_required_after_changes_is_uncertain_and_asks_before_repair`, `answering_the_uncertain_repair_question_with_repair_runs_one_repair_turn_that_still_needs_approval`, `answering_the_uncertain_repair_question_with_no_leaves_it_reported_and_unrepaired`, `a_per_change_check_keeps_its_pre_patch_baseline_and_pre_existing_classification`. |
| A repair proposal for a protected path is refused by #98; a new unprotected path is clearly marked as a one-shot scope expansion. | `verification_loop::a_protected_repair_path_is_refused_without_a_patch`, `a_new_unprotected_repair_path_is_a_typed_one_shot_expansion`, `a_stale_expansion_card_newly_protected_is_refused_at_apply`. |

### 7.3 Deterministic evaluation

The new scenarios drive the real engine, not a mocked verification loop. Each
asserts facts from the durable verification report or the session log; none
asserts what the script assumed happened.

- `verification_pre_existing_failure` seeds a failing integration test before
  the turn. The baseline and the post-patch check see the same failure, so the
  report classifies it `pre_existing`, offers no repair target, and the run
  makes no repair model call.
- `verification_environment_failure` uses a shell that does not exist. Both the
  baseline and the check fail to spawn; the report records
  `environment_failure`, offers `fix_environment`, and starts no repair.
- `verification_declined_baseline` declines the offered baseline while approving
  the check after the patch. The log records `declined` and the later failure is
  `unknown_provenance` with a `provide_baseline` action.
- `verification_repair_stalled_repeated` runs with the limit at sixteen. The
  first repair changes the file without changing the compiler error; the second
  proposal repeats the fingerprint and is stopped as `repeated_failure` before
  it is applied.
- `verification_repair_oscillation` applies A, then B, then restores A. B
  differs from A by error code and summary; a change of line or column alone
  would be the same failure under requirement 3, not an oscillation. The next
  proposal repeats the first failure/diff pair and is stopped as `oscillation`
  before the limit.

### 7.4 Local checks and what remains for CI

Run locally on 2026-11-01:

- `cargo fmt --all -- --check`
- `cargo clippy -p workspace-engine -p eval-harness --all-targets --locked -- -D warnings`
- `cargo nextest run -p workspace-engine --test repair_provenance --test repair_baseline --test repair_stagnation --test verification_loop --locked` — 131 passed
- `cargo nextest run -p eval-harness --locked` — 105 passed, 2 skipped; the leak detector flagged `trace::tests::a_completion_settlement_marker_is_not_a_tool_round_or_call`
- `cargo run -p eval-harness -- run --tier deterministic` — 51 scenarios passed
- `scripts/quality-gate.sh fmt node lint typos deny` — all five steps passed
- `npm run specs:check -- --strict`
- `git diff --check`

Workspace-wide `cargo clippy --workspace` and `cargo nextest run --workspace`
were deliberately not run locally, per `AGENTS.md`; the full seven-check gate
remains for CI. The deterministic tier proves the engine transitions, not that a
real model chooses the right tools or patches; that is spec 106's live gate.

### 7.5 Review-fix note (2026-11-05)

The 2026-11-04 review of specs 101–105 found that the finding comparison key
included `start_line` and `start_column` (`verification.rs`), and that
`provenance::introduced_findings` and the stagnation `repeated_failure` check
compared on it. A patch that added lines above an existing error therefore
turned a pre-existing failure into an introduced one and started repair, and
an ineffective repair that only moved the failing line never reached the
repeated-failure stop. Both are now fixed by the location-free comparison key
in requirement 3.

Decisions recorded with the fix:

- **Severity.** A new warning or informational finding on a check that already
  failed stays in the report but does not make the result `worsened` and is not
  handed to automatic repair. Only errors are introduced repair evidence for
  the `worsened` delta.
- **Duplicates.** Provenance compares multiplicity, not a set. One pre-existing
  occurrence plus one newly introduced identical occurrence is exactly one
  introduced finding; occurrences that stayed at the same location are matched
  first so the moved or multiplied one is the repair target. The failure
  fingerprint itself keeps set semantics, so a duplicate report within one
  result is still not a new failure.
- **Versioning.** `FailureFingerprint` is now version 2 and hashes the
  location-free key. Version 1 values still deserialize. A stored version 1
  baseline or provenance record classifies from its durable findings, so a
  pre-existing failure recorded before the change still reads as
  `pre_existing`. A version 1 repair-history entry has no findings to
  re-derive, so it does not compare equal to a version 2 value; the next
  attempt starts a fresh stagnation window and the numeric limit still bounds
  repair.

Regression tests were written first and failed for these reasons:
`repair_targets::a_pre_existing_error_that_shifts_lines_is_still_pre_existing`
and `repair_provenance::fingerprint_changes_with_material_evidence_but_not_location`
reported a different hash / `Worsened` instead of `PreExisting`;
`verification_loop::a_pre_existing_error_that_shifts_lines_stays_pre_existing`
reached the repair path (`NoRepairApplied` instead of `Failed`);
`verification_loop::a_repair_that_only_moves_the_failing_line_stalls_as_repeated_failure`
exposed a second line-moving patch instead of stalling;
`repair_targets::a_new_warning_on_a_failed_baseline_is_not_worsened_or_a_repair_target`
classified `Worsened`; and
`repair_targets::a_baseline_written_with_a_v1_fingerprint_still_classifies_by_its_findings`
classified `Worsened` instead of `PreExisting`. The existing
`fingerprint_changes_with_material_evidence` assertion that a different line
changes the fingerprint contradicted this requirement and was replaced by
`fingerprint_changes_with_material_evidence_but_not_location`; that is a
corrected expectation, not a loosened assertion.

### 7.6 Review-fix note: repair request details (2026-11-05)

The 2026-11-04 review found that commit `b2a18ac` switched the repair turn to
`render_repair_request` / `evidence_line`, which printed only severity, code,
summary and location. The finding's redacted `details()` — an `assert_eq`
failure's left and right values, rustc's expected/found lines, and a generic
check's bounded stderr tail — never reached the model, so a repair turn could
not see the actual diagnosis it was asked to fix.

The request now renders each introduced finding's details below its normalized
summary and location, bounded by the named constants
`MAX_REPAIR_FINDING_DETAILS_BYTES` (1,200) and
`MAX_REPAIR_TOTAL_DETAILS_BYTES` (4,000). A cut adds
`REPAIR_FINDING_DETAILS_TRUNCATED` or `REPAIR_TOTAL_DETAILS_TRUNCATED`; summary
lines and the scope line are never dropped. Details are taken from
`FindingEvidence`, which now stores `Finding::details()` as already redacted and
bounded by `Finding::new`. They stay out of `canonical_key`, `location_key`,
equality and the fingerprint, so R5's provenance and stagnation behaviour is
unchanged. Old durable records without details still load and render their
summary and location. The generic finding's fingerprint (R10) is untouched.

Regression tests were written first and failed for these reasons:

- `verification_loop::the_repair_request_carries_normalized_evidence_not_the_raw_log`
  failed with "the repair request carries the assertion's left and right
  values"; the request contained only
  `- error: repro: assertion `left == right` failed at src/a.rs:1:1`, and the
  seeded `AKIAIOSFODNN7EXAMPLE` was checked separately.
- `verification_loop::the_repair_request_carries_a_rustc_type_error_expected_and_found`
  failed with "the repair request carries rustc's expected and found detail".
- `verification_loop::the_repair_request_carries_a_generic_checks_bounded_stderr_tail`
  failed with "the repair request carries the generic stderr tail".
- `repair_targets::a_repair_request_bounds_one_findings_details` failed with
  "the request must say the finding's details were cut".
- `repair_targets::a_repair_request_bounds_total_details_across_many_findings`
  failed with "the request must say the total details budget was reached".

The existing `repair_provenance::fingerprint_ignores_transient_finding_fields`
asserts that two findings with different raw details compare equal. Carrying
details on `FindingEvidence` therefore required a manual `PartialEq` that
compares identity only; that assertion was kept, not weakened.

### 7.7 Review-fix note: task-start baselines and `uncertain` (2026-11-05)

The 2026-11-04 review found that baselines were always taken against the tree
with the task's earlier patches applied. A regression introduced by patch 1 but
first observed by a check selected for patch 2 therefore looked `pre_existing`
and was never repaired. The decision recorded for R14 is D2 piece 1: check
levels, not a rebuilt task-start tree.

`BaselineCoordinator::prepare` now reads the approved plan's required checks,
merges them with the per-change detected checks, and establishes each required
check on the **first** patch the task applies, before any write and on the
untouched tree. A later patch reuses that durable task-start baseline instead of
taking a new pre-patch one, even when the later patch is the first to select the
check. Per-change checks the plan does not require keep exactly the earlier rule:
they are baselined immediately before the patch that first selects them.

A required check that first becomes required after the task has already changed
files has no task-start tree to compare against. It is recorded as
`CheckBaseline::Uncertain` rather than run post-patch, and classification maps
that marker to the new `FailureProvenance::Uncertain`. Classification also
treats a required check with no recorded baseline as uncertain, so a revision
after the last patch and a historical log whose first patch predates this rule
still cannot read as silently pre-existing. The classification
precedence is unchanged except for this one insertion: live environment failure
first, then `CheckBaseline::Uncertain`, then declined/unavailable/absent
baselines as `unknown_provenance`, then a passed/failed baseline. `uncertain` is
therefore never silently `pre_existing`, is named in the report immediately
before the unknown group, and contributes no automatic repair target.

A worker verification that ends with only uncertain failures pauses on an
engine-authored durable question: **Repair it** or **Leave it unrepaired**. The
answer is a question answer, not a patch or command approval. `Repair it` builds
the uncertain evidence into one ordinary repair target and calls the existing
repair turn; the patch that turn proposes still needs its own approval. **Leave
it unrepaired** settles the verification as failed and leaves the failure in the
report. A non-worker session keeps the report and starts no repair. Pending
turns written before the question kind existed load as ordinary model questions.

Old session logs remain compatible: no existing baseline or provenance wire
value changed, and a log that names only `declined`, `unavailable`,
`unknown_provenance` or the other earlier values still loads and folds without
inventing `uncertain`.

Regression tests were written first. Before the change:
`plan_turn::a_required_check_is_baselined_at_task_start_and_a_later_regression_is_repaired`
failed with `the required check is baselined before patch 1 on the untouched
tree: {}`; `a_required_check_first_required_after_changes_is_uncertain_and_asks_before_repair`,
`answering_the_uncertain_repair_question_with_repair_runs_one_repair_turn_that_still_needs_approval`
and `answering_the_uncertain_repair_question_with_no_leaves_it_reported_and_unrepaired`
failed with `the worker asks whether to repair the uncertain failure`;
`a_declined_required_baseline_is_recorded_declined_not_uncertain` failed because
the patch applied without offering the required check's baseline. The guard
`a_per_change_check_keeps_its_pre_patch_baseline_and_pre_existing_classification`
pinned the unchanged per-change rule; review fix R15 later re-pointed the
root-first-touched-later case to `uncertain` (see the R15 note below). All now
pass.

### Review fix 2026-11-06: floor checks take the task-start baseline (R15)

Review fix R15 (decision D1) makes the detected checks of every root a task
touched a completion obligation. R14's baseline rule therefore covers two
required sets, not one: the approved plan's `required: true` checks **and** the
completion floor for the roots the first patch and its earlier changes touch.
`BaselineCoordinator::prepare` merges them by `CheckId` before deciding what to
run, so a floor check for the first patch's owning or enclosing root is
baselined on the untouched tree through the ordinary policy path.

A root first touched by a later patch has no task-start baseline. Its floor check
is recorded as `CheckBaseline::Uncertain` rather than being run post-patch, so
its later failure is `uncertain`, is named before the unknown group, and never
reads as silently `pre_existing`. That supersedes R14's per-change surprise for
this case: the check is now required, and a post-patch baseline would compare
against a tree the task had already changed. The per-change baseline rule is
unchanged for a check outside the required set.

This does not change the R14 provenance precedence or the uncertain repair
question: environment failure first, then `Uncertain`, then unknown, then
passed/failed. It only changes which checks have a task-start baseline.

Tests written first and recorded failing:
`plan_turn::floor_checks_are_baselined_at_task_start_and_a_later_root_is_uncertain`
failed with `the first patch's floor check has a task-start baseline: {}`;
`plan_turn::a_floor_check_for_a_root_first_touched_later_is_uncertain_not_pre_existing`
failed with the later root recorded as `Failed { termination: Exited { exit_code:
1 }, .. }` from a post-patch baseline. Both pass now. The first test also proves
the later root is `Uncertain` and its verification failure is reported as
`uncertain`, not `pre_existing`.
