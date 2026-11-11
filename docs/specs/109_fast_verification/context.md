# Spec 109 context

**Specified:** 2026-11-11. Line numbers are approximate; re-read before editing.

## Evidence

Spec 33 Task 1 on Damaian v0.69.0, session `session_18e6f2fdaa423e08_14169_15`
(2026-11-11): first patch 10:35:33 (51 lines, `config.rs`); pre-apply baseline
of `cargo test` at the root and in `crates/workspace-engine` (the root run spent
over two minutes just compiling every crate, including `desktop-app`); after the
apply, a per-change "Checks need approval: cargo test in crates/workspace-engine"
card. The plan (9 steps) carried `cargo nextest run -p workspace-engine --test
permission_profiles --test foundation` and scoped clippy as required checks.
Local observations 81 (duplicate member baseline), 83 (full checks per patch;
partly fixed by spec 107 D3), 86 (main-checkout test-process overhead) and 89
(running baseline not visible).

## Survey: what exists today

- **Baseline gate** (`baseline.rs` `BaselineCoordinator::prepare` ≈193): before
  a patch applies, the checks the patch selects plus the completion floor of
  touched roots (`floor_checks`) that have no task-start baseline are put on a
  baseline card ("This patch has not been applied…", ≈270). A check is baselined
  once per task, before its first patch. `prepare_for_command` (≈289, review fix
  R18) does the same before the first mutating-capable command.
- **Provenance** (`provenance.rs` `classify` ≈100): no baseline →
  `UnknownProvenance`; `Passed` → `NewRegression`; `Uncertain` → `Uncertain`;
  `Declined`/`Unavailable` → `UnknownProvenance`; `Failed` → pre-existing /
  unchanged / worsened by comparing termination and findings.
- **Per-change selection** (`chat.rs` ≈2690 and `verification.rs`
  `propose_checks` / `propose_completion_checks`): the detected checks of the
  owning root per change; the floor (owning + enclosing roots) at completion;
  spec 107 Task 5 made a continuing planless turn per-change too.
- **Plan checks** (spec 105): a step's `PlannedCheck { command,
  working_directory, required }`; deferred to its step while open; spec 106 Task
  6 added the engine rerun of a finished step's stale check.
- **Approval:** each check goes through the command policy
  (`ValidationOrchestrator::store_proposal`); "Ask before test, build and lint
  commands" (`require_approval_for_risky_commands`, on by default) makes
  `cargo test` ask every time. "Always allow" is a persistent, user-scope rule
  (spec 34).
- **Recorded runs:** `CheckRun` carries the covered `file_hashes` and the
  mutation `generation`; `is_stale` compares them with the current tree
  (`verification.rs` ≈715).
- **Findings:** `finding/rust_test.rs` parses failing test names and locations
  from `cargo test` output; nextest output is parsed too.

## Decisions

- **D1 — lazy baseline (recommended).** Drop the pre-apply baseline card and
  R18's pre-command baseline. When a per-change or completion check fails and
  the task has no baseline for it, take one then, on the pre-patch tree, and
  classify with it. A passing check never needs one.
- **D2 — the pre-patch tree (to confirm in Task 3).** Build it in a temporary
  git worktree at `HEAD`, then apply the task's state *before* the patch under
  verification: the uncommitted changes that existed when the task started plus
  the task's earlier applied patches (the patch engine stores each applied
  patch's before-hashes and content). Never touch the user's checkout. Remove the
  worktree afterwards. If the pre-patch state cannot be reconstructed exactly
  (hash mismatch), record `CheckBaseline::Unavailable` — the existing
  `UnknownProvenance` path — rather than guess.
- **D3 — targeted rerun (recommended).** For Rust test findings, rerun the
  failing tests by name (`cargo test -- --exact <names>` / nextest `-E`
  filter) in the pre-patch tree; compare the same set. Anything else reruns the
  whole check.
- **D4 — per-change check choice (recommended).** If the plan has a required
  check whose working directory is the patch's owning root (or an enclosing
  root), run that per change instead of the detected `cargo test` for that root.
  The detected check stays in the completion floor and runs once at the end.
- **D5 — per-task approval (security; confirm before Task 5).** An approved
  check identity is remembered on the task chain (a durable event), and later
  proposals of the same identity in that chain are approved automatically and
  audited as `approved_for_task`. Restrict-only: a repository cannot turn it on
  for commands the policy refuses or blocks; it never applies to a different
  command, directory, session or task; a declined check is not remembered as
  approved. The approval card says "Run for this task".
- **D6 — reuse (recommended).** Before proposing a check, if a `CheckRun` of the
  same identity is not stale, reuse it (no command, no card) and say so in the
  report.
- **D8 — stop anything (decided by the owner, 2026-11-11).** "We need to allow
  the user to stop any ongoing action, even if it is destructive." Today
  **Cancel** exists for baselines (spec 107 Task 4) and Stop for model rounds
  (the turn indicator); the post-apply verification started from the patch card
  (`startVerification`) has neither, and commands, hooks and MCP calls cannot be
  interrupted from the UI. One stop path: the UI's Stop fires the turn's
  `CancelToken`; every runner (command, check, baseline, hook, MCP, browser)
  honours it by ending its process group; a side-effecting action's marker is
  finished with an unknown outcome so recovery and the report say what may have
  happened. No action is exempt.
- **D7 — visibility.** One running-state component for checks and baselines:
  card header indicator, Cancel replacing the run button, the status line and the
  composer line from the same state (fixes OBSERVATIONS 89).
