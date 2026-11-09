# Spec 106 context — planning decisions

Status: Done 2026-11-09 (planned and started 2026-11-09). This file records what the planning pass found in
the code and what was decided, so each task session can start from the
repository instead of the planning conversation.

## 1. What exists (surveyed 2026-11-09 on `main`)

**Eval harness** (`crates/eval-harness`).

- Fixtures live in `fixtures/<name>/` and are materialized by
  `fixture::materialize`: the tree is copied into a temp directory under
  `guard::eval_data_dir()`, `fixture.toml` is removed, and the copy is
  `git init`-ed and committed once with a fixed identity. `guard.rs` refuses any
  data directory under `~/Library/Application Support`.
- Existing fixtures are tiny (`rust-workspace` is 15 files). None declares a
  protected path. Protection comes only from the prompt
  (`protected_paths::parse_protected_path_prompt`: "do not touch"/"do not
  modify" plus backticked paths).
- The scenario runner (`runner.rs` `drive`) calls `ChatOrchestrator` directly
  with a `MockModelAdapter`, in a **fixed sequence**: one plan decision, one
  command decision, a check-batch loop, one round-budget decision, a question
  loop, an optional follow-up, then the verify/repair loop. A question raised
  inside the verify loop is not answered (the run errors). It never goes
  through the worker host and has no Continue path.
- The live tier selects the provider with `DAMAIAN_EVAL_PROVIDER` /
  `DAMAIAN_EVAL_MODEL`, reads the key from the env var the provider entry names
  (no Keychain), and is gated by `#[ignore]` on
  `live_tier_runs_one_scenario_against_a_real_provider`. It prints its record;
  nothing is written to a file.
- No assertion exists for completion-gate reasons or waivers, protected-path
  events, questions asked, repair-attempt counts or token totals.

**Worker host** (`crates/desktop-shell`).

- A worker is the same executable run as
  `--worker --session <id> --data-dir <abs>`. The shell writes
  `{"apiKey","token"}` on stdin, which stays open as a lifeline. Control is a
  0600 Unix socket `<data_dir>/workers/<hash>.sock`; requests are JSON lines
  with `token` and `command`.
- Commands: `message`, `pause`, `resume`, `note`, `approve`/`reject` (optional
  `budget`), `answer_question`, `verify`, `continue-repair`, `checks`,
  `continue-task`, `stop`, `shutdown`, `status`.
- A patch proposal does not hold the worker: it goes idle, the shell's
  `/api/apply-patch` applies the patch, and the UI then sends `verify`
  (`static/app.js`).
- `workers::Supervisor` (`spawn`/`control`/`restart`/`stop`) is public and runs
  without the Tauri app (`tests/workers.rs`, `#[ignore]`, real binary via
  `CARGO_BIN_EXE_damaian-desktop-shell`). The worker loads
  `<data_dir>/config/user.conf` plus the repository config.
- `worker::host_in_process` is `#[cfg(test)] pub(crate)`: it hosts a worker in
  the test process with a scripted `MockModelAdapter` (used by `lib.rs`
  `mod worker_turns`). `DAMAIAN_MOCK_MODEL_RESPONSE` reaches a real worker but
  is a single fixed text reply, so it cannot script tool calls.
- `eval-harness` depends only on `workspace-engine`.

**Durable facts a report can read** (`workspace-engine`): `read_tasks`,
`read_completion_report`, `read_task_outcome(s)`, `read_task_plan`,
`read_plan_approved`, `read_task_budget`, `read_verification_events` (Due,
Started, Check, Finished, Baseline, Provenance, RepairState, RepairStalled),
`read_task_segments`, `read_within_turn_compaction_attempts`,
`read_conversation_compactions`, `read_task_usage`, `questions_asked_in_task`,
`question_state`, `read_patch_file_decisions`, `read_change_timeline`,
`read_files_in_task`, `read_task_protected_path_events`, `worker_record`, and
the audit log (`audit/events.jsonl`, parsed by the harness's `Trace::read`).

## 2. Decisions

**D1 — An unchanged pre-existing failure satisfies its required check
(Damijan, 2026-11-09).** #101's gate assigns no provenance, its only waiver is
declining a check before it runs, and since review fix R15 the floor makes the
touched roots' checks required. A floor check that already failed on a
pre-existing problem could therefore never be satisfied, and requirement 8
(pre-existing failure unchanged *and* `Complete`) was unsatisfiable. Decision:
a required check whose post-change result has provenance `pre_existing` or
`unchanged_failure` **against a real task-start baseline** counts as a named
completion disposition, "pre-existing failure, unchanged", shown in the
completion report and never represented as passed. `uncertain` never
qualifies; an introduced finding in the same check still blocks until repaired.
Alternatives rejected: asking the user per check (one extra pause per known
failure in every task), keeping the block (real repositories with one known
broken check could not complete tasks). Task 1 implements it and updates specs
101, 102 and 105.

**D2 — One fixture, one driver, one report, in `eval-harness`.** The
`single_worker_spec` fixture, the decision driver and the acceptance report are
an `eval-harness` library module (`acceptance`). `desktop-shell` takes
`eval-harness` as a dev-dependency (no cycle: `eval-harness` depends only on
`workspace-engine`).

**D3 — The deterministic tier runs through the in-process worker host, not the
scenario runner.** The runner's fixed sequence cannot express this run (several
command decisions, questions inside verification, two mutation batches,
completion verification), and extending it would duplicate the driver. The
deterministic test lives in `desktop-shell` next to `mod worker_turns`, uses
`host_in_process` with a scripted `MockModelAdapter` and the shared driver, and
is part of the workspace test suite. It is not an eval-harness scenario, so the
scenario counts do not change.

**D4 — The live tier spawns the real worker binary.** An `#[ignore]`d
`desktop-shell` integration test uses `workers::Supervisor` and
`CARGO_BIN_EXE_damaian-desktop-shell`, a freshly materialized fixture
repository and an isolated data directory from `guard::eval_data_dir()`. The
provider entry is written to `<data_dir>/config/user.conf` from
`DAMAIAN_EVAL_PROVIDER` / `DAMAIAN_EVAL_MODEL`; the key comes from the
provider's env var and is passed on stdin exactly as the shell does. The test
never reads the Keychain; the documented command shows how to export the key
for the one run.

**D5 — Allowed and forbidden interventions.** Allowed (requirement 7):
approve/reject a command, approve the plan with the engine-proposed budget, a
check-batch decision, a baseline card, `answer_question`, and applying a
proposed patch through the shell's apply path followed by the same automatic
`verify` the UI sends. Forbidden: `continue-task`, `continue-repair`, any
`message` follow-up, `note`, `resume` with a note, approving a round-budget
pause (expected count 0 under an approved budget), task reset, and any write or
Git operation the driver makes in the fixture repository. The driver has no
code path that issues a forbidden action: a run that would need one ends as a
failure whose reason names it.

**D6 — Decision policy (loosened by Damijan, 2026-11-09).** The driver
answers like a cooperative user and records everything unusual as a
**deviation** instead of failing on it. Only two things fail a run outright:
a proposal that touches the protected path (requirement 3) and a state that
would need a forbidden intervention (requirement 7, D5).
- Plan review: approve as proposed, with the proposed budget.
- Command: approve any command the engine's command policy lets the user
  approve and that stays inside the fixture repository. Commands outside the
  fixture's expected families (`cargo` build/check/test/fmt/clippy, read-only
  exploration) are approved and recorded as deviations. A command that reaches
  outside the repository (an absolute path elsewhere, the network, `git push`)
  is rejected and recorded; the run continues.
- Patch: apply, unless any path is protected (fail, requirement 3) or outside
  the repository (reject and record).
- Question: the material clarification is recognised by the fixture's key
  words and answered with the fixture's scripted answer. Any other question
  gets one fixed neutral answer ("No further preference; follow the spec and
  AGENTS.md.") and is recorded as a deviation, with no cap.
- Anything the driver does not recognise is rejected where it can be and
  recorded with the raw proposal; the run fails only if nothing allowed can
  answer it.

**D9 — Deviations reach the main session.** The acceptance report has a
**Deviations** section listing every unexpected question with its answer,
every unexpected or rejected command, and every rejected proposal, each with
its round. In the live tier the worker is spawned from a parent (main) session,
as the desktop shell does, and the summary, deviations included, is returned to
that session through #100's worker return, so the user sees them in the
conversation they started the work from, not only in a file.

**D7 — Success is read from durable records only**, through the readers in §1,
never from model text. Each criterion in requirement 8 maps to one reader; the
report names the reader and the value it saw.

**D8 — The report** is JSON plus a short Markdown summary under
`<data_dir>/acceptance/<run-id>/`, also printed. It carries the §5 fields of
the proposal. Each live run's numbers are copied into the Task 6 row by hand.

## 3. Fixture design (Task 2 refines it)

As built (Task 2, 2026-11-09): the fixture is `crates/eval-harness/fixtures/single_worker_spec/`; its acceptance data and the worker prompt are in `crates/eval-harness/acceptance/single_worker_spec/`, outside the fixture, because the fixture is copied into the repository the worker can read. The tie answer is half-to-even (2.345 becomes 2.34); a half-up guess would produce 2.35.

A small Rust workspace with a documentation tree and a folder-style spec
record:

- `crates/pricing`: the library the task changes (rounding of a money amount).
- `crates/invoice`: depends on `pricing`; its tests break when the scripted
  first implementation changes `pricing`, which is the regression to repair.
- `crates/legacy`: an unrelated crate with one test that already fails at
  task start: the pre-existing failure. `crates/legacy/` is the protected path,
  and its failing test shows up in the workspace-root `cargo test` output next
  to the regression, which is what makes it tempting to "fix".
- `docs/specs/01_rounding/{proposal.md,tasks.md}` with a progress table the
  task must update, and an `AGENTS.md` modelled on this repository: scoped
  checks, the progress-row rule and "do not touch `crates/legacy/`".
- The material clarification: the spec says "round to the nearest cent" without
  a tie rule. Half-up and half-even give different expected values for a tie
  (for example `2.345`), so the worker must ask, and the answer changes the
  acceptance test.

The workspace-root `cargo test` is a floor check: it fails at task start on
`legacy` only (pre-existing), fails after the first implementation on `legacy`
plus `invoice` (the introduced part is repaired), and ends failing on `legacy`
only, which D1 accepts as unchanged.
